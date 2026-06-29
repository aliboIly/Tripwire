// Read-only command handlers (Phase 1). These traverse the open place's data
// model and never modify it. The server validates tool input before sending, so
// here we only narrow the payload to the shape each command expects.

import { BridgeCommand, CommandResult } from "protocol";

const DEFAULT_TREE_DEPTH = 4;
const MAX_TREE_DEPTH = 25;

// children is omitted when empty. Luau's JSONEncode turns an empty table into
// "{}", not "[]", so leaving an empty array here would serialize as an object and
// break the shape. Omitting it keeps a leaf node unambiguous.
interface TreeNode {
	name: string;
	className: string;
	children?: TreeNode[];
}

// Resolves a dot path from the data model root, for example "Workspace.Folder.Part".
// An empty path, or "game", returns the data model itself.
function resolveInstance(path: string | undefined): Instance | undefined {
	if (path === undefined || path === "" || path === "game") return game;
	let current: Instance = game;
	for (const segment of path.split(".")) {
		if (segment === "" || segment === "game") continue;
		const found = current.FindFirstChild(segment);
		if (found === undefined) return undefined;
		current = found;
	}
	return current;
}

function clampDepth(requested: number | undefined): number {
	if (requested === undefined) return DEFAULT_TREE_DEPTH;
	if (requested < 0) return 0;
	if (requested > MAX_TREE_DEPTH) return MAX_TREE_DEPTH;
	return requested;
}

function buildTree(instance: Instance, depth: number): TreeNode {
	const node: TreeNode = { name: instance.Name, className: instance.ClassName };
	if (depth > 0) {
		const kids = instance.GetChildren();
		if (kids.size() > 0) {
			node.children = kids.map((child) => buildTree(child, depth - 1));
		}
	}
	return node;
}

// Attributes are JSON friendly, so keep primitives as-is and stringify the rest
// (Vector3, Color3, and so on) rather than dropping them.
function readAttributes(instance: Instance): Record<string, unknown> {
	const out: Record<string, unknown> = {};
	for (const [key, value] of instance.GetAttributes()) {
		const primitive = typeIs(value, "string") || typeIs(value, "number") || typeIs(value, "boolean");
		out[key] = primitive ? value : tostring(value);
	}
	return out;
}

function notFound(path: string | undefined): CommandResult {
	return { ok: false, error: `no instance at path: ${path ?? "game"}` };
}

// Returns a result for read commands, or undefined when the command is not one
// of ours so the caller can try another handler group.
export function handleRead(cmd: BridgeCommand): CommandResult | undefined {
	if (cmd.type === "get_file_tree") {
		const p = cmd.payload as { path?: string; maxDepth?: number };
		const instance = resolveInstance(p.path);
		if (instance === undefined) return notFound(p.path);
		return { ok: true, data: { tree: buildTree(instance, clampDepth(p.maxDepth)) } };
	}

	if (cmd.type === "get_instance_children") {
		const p = cmd.payload as { path?: string };
		const instance = resolveInstance(p.path);
		if (instance === undefined) return notFound(p.path);
		const children = instance.GetChildren().map((child) => ({
			name: child.Name,
			className: child.ClassName,
		}));
		return { ok: true, data: { children } };
	}

	if (cmd.type === "get_instance_properties") {
		const p = cmd.payload as { path?: string };
		const instance = resolveInstance(p.path);
		if (instance === undefined) return notFound(p.path);
		return {
			ok: true,
			data: {
				name: instance.Name,
				className: instance.ClassName,
				fullName: instance.GetFullName(),
				attributes: readAttributes(instance),
			},
		};
	}

	return undefined;
}
