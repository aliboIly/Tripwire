// Write/edit command handlers (Phase 2). These mutate the open place, so each
// data-model mutation is wrapped as one Studio undo step. Script source is written
// through ScriptEditorService:UpdateSourceAsync (never Script.Source directly).
//
// roblox-ts needs three boundary casts to do dynamic work the type system cannot
// express: constructing an instance from a runtime class string, writing a
// property by a runtime key, and looking an enum up by name. They are quarantined
// here and each decodes a typed value first, so no `any` leaks into the codebase.

import { BridgeCommand, CommandResult } from "protocol";
import { resolveInstance } from "read";

// These Studio-only services are not re-exported by @rbxts/services; fetch them
// through GetService.
const ChangeHistoryService = game.GetService("ChangeHistoryService");
const ScriptEditorService = game.GetService("ScriptEditorService");

// A property value on the wire is a tagged union, so the type is explicit rather
// than guessed from the property name (a 3-number array is ambiguous between
// Vector3 and Color3, and Color3 components differ between 0..1 and 0..255).
export type WireValue =
	| { type: "primitive"; value: string | number | boolean }
	| { type: "Vector3"; value: [number, number, number] }
	| { type: "Color3"; value: [number, number, number]; rgb255?: boolean }
	| { type: "UDim2"; value: [number, number, number, number] }
	| { type: "CFrame"; value: number[] }
	| { type: "EnumItem"; enum: string; item: string }
	| { type: "instance"; path: string };

// Throws a string on a bad value; callers pcall it and report the message.
function convert(value: WireValue): unknown {
	switch (value.type) {
		case "primitive":
			return value.value;
		case "Vector3":
			return new Vector3(value.value[0], value.value[1], value.value[2]);
		case "Color3":
			return value.rgb255
				? Color3.fromRGB(value.value[0], value.value[1], value.value[2])
				: new Color3(value.value[0], value.value[1], value.value[2]);
		case "UDim2":
			return new UDim2(value.value[0], value.value[1], value.value[2], value.value[3]);
		case "CFrame": {
			const c = value.value;
			return c.size() >= 12
				? new CFrame(c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7], c[8], c[9], c[10], c[11])
				: new CFrame(c[0], c[1], c[2]);
		}
		case "EnumItem": {
			// FromName returns undefined on a bad name; Enum[type][name] would throw.
			const enums = Enum as unknown as Record<string, { FromName(name: string): EnumItem | undefined }>;
			const group = enums[value.enum];
			const item = group !== undefined ? group.FromName(value.item) : undefined;
			if (item === undefined) throw `unknown enum ${value.enum}.${value.item}`;
			return item;
		}
		case "instance": {
			const target = resolveInstance(value.path);
			if (target === undefined) throw `no instance at path: ${value.path}`;
			return target;
		}
	}
}

function assignProperty(instance: Instance, name: string, value: WireValue): void {
	(instance as unknown as Record<string, unknown>)[name] = convert(value);
}

// Wraps a data-model mutation so Studio treats it as a single undo step. The
// recording can be undefined (one already active, or recording disabled); the
// mutation still runs, just without a waypoint.
function asUndoStep(displayName: string, mutate: () => CommandResult): CommandResult {
	const recording = ChangeHistoryService.TryBeginRecording(`tripwire_${displayName}`, displayName);
	// pcall the mutation so a throw can never leak an open recording, which would
	// otherwise disable undo for the rest of the session.
	const [ok, result] = pcall(mutate);
	const finished = ok ? (result as CommandResult) : { ok: false, error: `${displayName} failed: ${result}` };
	if (recording !== undefined) {
		ChangeHistoryService.FinishRecording(
			recording,
			finished.ok ? Enum.FinishRecordingOperation.Commit : Enum.FinishRecordingOperation.Cancel,
		);
	}
	return finished;
}

function notFound(path: string | undefined): CommandResult {
	return { ok: false, error: `no instance at path: ${path ?? "game"}` };
}

interface CreatePayload {
	className: string;
	parentPath?: string;
	name?: string;
	properties?: Array<{ name: string; value: WireValue }>;
}

function createInstance(p: CreatePayload): CommandResult {
	const parent = resolveInstance(p.parentPath);
	if (parent === undefined) return notFound(p.parentPath);

	let path = "";
	let className = "";
	const propertyErrors: string[] = [];
	const [ok, err] = pcall(() => {
		const created = new Instance(p.className as keyof CreatableInstances);
		if (p.name !== undefined) created.Name = p.name;
		if (p.properties !== undefined) {
			for (const prop of p.properties) {
				const [setOk, setErr] = pcall(() => assignProperty(created, prop.name, prop.value));
				if (!setOk) propertyErrors.push(`${prop.name}: ${setErr}`);
			}
		}
		created.Parent = parent; // Parent last, after name and properties.
		path = created.GetFullName();
		className = created.ClassName;
	});
	if (!ok) return { ok: false, error: `cannot create "${p.className}": ${err}` };
	return { ok: true, data: { path, className, propertyErrors } };
}

function deleteInstance(p: { path?: string }): CommandResult {
	const instance = resolveInstance(p.path);
	if (instance === undefined) return notFound(p.path);
	if (instance === game) return { ok: false, error: "refusing to destroy the DataModel (game)" };
	const fullName = instance.GetFullName(); // capture before Destroy locks the instance
	const [ok, err] = pcall(() => instance.Destroy());
	if (!ok) return { ok: false, error: `destroy failed: ${err}` };
	return { ok: true, data: { destroyed: fullName } };
}

function setProperty(p: { path?: string; name: string; value: WireValue }): CommandResult {
	const instance = resolveInstance(p.path);
	if (instance === undefined) return notFound(p.path);
	const [ok, err] = pcall(() => assignProperty(instance, p.name, p.value));
	if (!ok) return { ok: false, error: `cannot set ${p.name}: ${err}` };
	return { ok: true, data: { path: instance.GetFullName(), property: p.name } };
}

// UpdateSourceAsync silently no-ops when the only delta is line endings, so
// normalize CRLF and lone CR to LF before writing.
function normalizeEol(source: string): string {
	const [crlf] = source.gsub("\r\n", "\n");
	const [lf] = crlf.gsub("\r", "\n");
	return lf;
}

function updateScriptSource(p: { path?: string; source: string }): CommandResult {
	const target = resolveInstance(p.path);
	if (target === undefined) return notFound(p.path);
	if (!target.IsA("LuaSourceContainer")) {
		return { ok: false, error: `${target.GetFullName()} is a ${target.ClassName}, not a script` };
	}
	const newSource = normalizeEol(p.source);
	// The callback may be re-invoked if an open editor was stale and must not yield;
	// for a whole-document write, returning the precomputed source is correct.
	const [ok, err] = pcall(() => ScriptEditorService.UpdateSourceAsync(target, () => newSource));
	if (!ok) return { ok: false, error: `UpdateSourceAsync failed: ${err}` };
	return { ok: true, data: { path: target.GetFullName() } };
}

export function handleEdit(cmd: BridgeCommand): CommandResult | undefined {
	if (cmd.type === "create_instance") {
		return asUndoStep("create_instance", () => createInstance(cmd.payload as CreatePayload));
	}
	if (cmd.type === "delete_instance") {
		return asUndoStep("delete_instance", () => deleteInstance(cmd.payload as { path?: string }));
	}
	if (cmd.type === "set_property") {
		return asUndoStep("set_property", () => setProperty(cmd.payload as { path?: string; name: string; value: WireValue }));
	}
	// Script edits go through the editor, which manages its own undo history, so
	// they are not wrapped in a ChangeHistoryService recording.
	if (cmd.type === "update_script_source") {
		return updateScriptSource(cmd.payload as { path?: string; source: string });
	}
	return undefined;
}
