// Script read handlers (Phase 1). Reading a script's source is allowed from a
// plugin; only writing Script.Source is blocked, which is why the write path
// (Phase 2) uses ScriptEditorService:UpdateSourceAsync.

import { BridgeCommand, CommandResult } from "protocol";
import { clampMatchLimit, containsIgnoreCase, resolveInstance } from "read";

const MAX_LINE_LENGTH = 240;

// Source lives on the concrete script classes, not the LuaSourceContainer base.
function asScript(instance: Instance): Script | LocalScript | ModuleScript | undefined {
	if (instance.IsA("Script") || instance.IsA("LocalScript") || instance.IsA("ModuleScript")) return instance;
	return undefined;
}

function trimLine(line: string): string {
	return line.size() > MAX_LINE_LENGTH ? line.sub(1, MAX_LINE_LENGTH) : line;
}

function notFound(path: string | undefined): CommandResult {
	return { ok: false, error: `no instance at path: ${path ?? "game"}` };
}

export function handleScripts(cmd: BridgeCommand): CommandResult | undefined {
	if (cmd.type === "get_script_source") {
		const p = cmd.payload as { path?: string };
		const instance = resolveInstance(p.path);
		if (instance === undefined) return notFound(p.path);
		const scriptInstance = asScript(instance);
		if (scriptInstance === undefined) {
			return { ok: false, error: `${instance.GetFullName()} is a ${instance.ClassName}, not a script` };
		}
		return {
			ok: true,
			data: {
				path: scriptInstance.GetFullName(),
				className: scriptInstance.ClassName,
				source: scriptInstance.Source,
			},
		};
	}

	if (cmd.type === "grep_scripts") {
		const p = cmd.payload as { pattern: string; path?: string; limit?: number };
		const root = resolveInstance(p.path);
		if (root === undefined) return notFound(p.path);
		const limit = clampMatchLimit(p.limit);
		const matches: Array<{ path: string; line: number; text: string }> = [];
		for (const descendant of root.GetDescendants()) {
			if (matches.size() >= limit) break;
			const scriptInstance = asScript(descendant);
			if (scriptInstance === undefined) continue;
			const fullName = scriptInstance.GetFullName();
			let lineNumber = 0;
			for (const line of scriptInstance.Source.split("\n")) {
				lineNumber += 1;
				if (matches.size() >= limit) break;
				if (containsIgnoreCase(line, p.pattern)) {
					matches.push({ path: fullName, line: lineNumber, text: trimLine(line) });
				}
			}
		}
		return { ok: true, data: { matches, truncated: matches.size() >= limit } };
	}

	return undefined;
}
