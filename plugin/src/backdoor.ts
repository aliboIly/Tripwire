// Backdoor scan collector. The server owns the detection logic; this handler only
// gathers the suspect scripts for it. The server passes the high-signal keywords it
// cares about, and we ship a script only when its source contains one, so a clean place
// sends nothing and a backdoored one sends only the few that matter. Reading Source is
// allowed from a plugin (only writing it is blocked).

import { BridgeCommand, CommandResult } from "protocol";
import { resolveInstance } from "read";
import { asScript } from "scripts";

// Keep the bridge payload bounded; a huge place should not push megabytes at once.
const MAX_SCRIPTS = 200;
const MAX_TOTAL_BYTES = 1500000;

function containsAny(source: string, keywords: string[]): boolean {
	for (const keyword of keywords) {
		const [found] = source.find(keyword, 1, true);
		if (found !== undefined) return true;
	}
	return false;
}

export function handleBackdoor(cmd: BridgeCommand): CommandResult | undefined {
	if (cmd.type !== "scan_backdoors") return undefined;
	const p = cmd.payload as { keywords?: string[]; path?: string };
	const root = resolveInstance(p.path);
	if (root === undefined) return { ok: false, error: `no instance at path: ${p.path ?? "game"}` };

	const keywords = p.keywords ?? [];
	const scripts: Array<{ path: string; source: string }> = [];
	let totalBytes = 0;
	let truncated = false;

	for (const descendant of root.GetDescendants()) {
		const scriptInstance = asScript(descendant);
		if (scriptInstance === undefined) continue;
		const source = scriptInstance.Source;
		if (!containsAny(source, keywords)) continue;
		if (scripts.size() >= MAX_SCRIPTS || totalBytes + source.size() > MAX_TOTAL_BYTES) {
			truncated = true;
			break;
		}
		scripts.push({ path: scriptInstance.GetFullName(), source });
		totalBytes += source.size();
	}

	return { ok: true, data: { scripts, truncated } };
}
