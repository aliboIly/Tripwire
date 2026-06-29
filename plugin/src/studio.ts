// Studio-context read handlers (Phase 1): the output log and the current
// selection. Both are plugin-context APIs and have no headless equivalent.

import { LogService } from "@rbxts/services";
import { BridgeCommand, CommandResult } from "protocol";

// Selection is a Studio-only service that @rbxts/services does not re-export, so
// fetch it through GetService.
const Selection = game.GetService("Selection");

const DEFAULT_LOG_LIMIT = 100;
const MAX_LOG_LIMIT = 500;

export function handleStudio(cmd: BridgeCommand): CommandResult | undefined {
	if (cmd.type === "get_output_log") {
		const p = cmd.payload as { limit?: number };
		const limit = p.limit !== undefined ? math.clamp(p.limit, 1, MAX_LOG_LIMIT) : DEFAULT_LOG_LIMIT;
		const history = LogService.GetLogHistory();
		const start = math.max(0, history.size() - limit);
		const entries: Array<{ message: string; type: string; timestamp: number }> = [];
		for (let i = start; i < history.size(); i++) {
			const entry = history[i];
			entries.push({ message: entry.message, type: entry.messageType.Name, timestamp: entry.timestamp });
		}
		return { ok: true, data: { entries } };
	}

	if (cmd.type === "get_selection") {
		const selected = Selection.Get().map((instance) => ({
			name: instance.Name,
			className: instance.ClassName,
			path: instance.GetFullName(),
		}));
		return { ok: true, data: { selected } };
	}

	return undefined;
}
