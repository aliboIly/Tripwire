// Tripwire Studio plugin. Phase 0 handshake plus Phase 1 read commands (roblox-ts).
//
// It long-polls the local Tripwire bridge, runs commands against the open place,
// and posts results back. The plugin is authored in TypeScript, like the server.

import { HttpService } from "@rbxts/services";
import { BridgeCommand, CommandResult, PROTOCOL_VERSION, TRIPWIRE_VERSION } from "protocol";
import { INSTANCE_ID, installId } from "identity";
import { handleEdit } from "edit";
import { handlePlaytest, sweepRunners } from "playtest";
import { handleRead } from "read";
import { handleScripts } from "scripts";
import { handleStudio } from "studio";

const BRIDGE = "http://127.0.0.1:44331";
const RECONNECT_WAIT_SECONDS = 3;
const LOG = `[Tripwire v${TRIPWIRE_VERSION}]`;

const toolbar = plugin.CreateToolbar("Tripwire");
const button = toolbar.CreateButton("Tripwire", "Connect to the Tripwire MCP bridge", "");
let running = false;

function post(path: string, body: object): void {
	pcall(() =>
		HttpService.RequestAsync({
			Url: `${BRIDGE}${path}`,
			Method: "POST",
			Headers: { "Content-Type": "application/json" },
			Body: HttpService.JSONEncode(body),
		}),
	);
}

function dispatch(cmd: BridgeCommand): CommandResult {
	if (cmd.type === "ping") {
		return { ok: true, data: { place: game.Name, payload: cmd.payload } };
	}
	const handled =
		handleRead(cmd) ?? handleScripts(cmd) ?? handleStudio(cmd) ?? handleEdit(cmd) ?? handlePlaytest(cmd);
	if (handled !== undefined) return handled;
	return { ok: false, error: `unknown command: ${cmd.type}` };
}

function handle(cmd: BridgeCommand): void {
	// pcall the dispatch so a thrown handler error is caught and returned rather
	// than killing the poll loop, and print any failure to the Studio Output.
	const [ok, result] = pcall(() => dispatch(cmd));
	const finished: CommandResult = ok ? (result as CommandResult) : { ok: false, error: `${result}` };
	if (!finished.ok) warn(`${LOG} ${cmd.type} failed: ${finished.error}`);
	post("/result", { id: cmd.id, ok: finished.ok, data: finished.data, error: finished.error });
}

// Announces this plugin (its session instanceId plus place metadata) and confirms
// the server speaks the same protocol version. Re-callable: used on connect and
// again when a poll returns 205 (the server lost our registration after a restart).
// Returns false with a clear warning on an unreachable bridge or a version mismatch.
function announce(): boolean {
	try {
		const res = HttpService.RequestAsync({
			Url: `${BRIDGE}/hello`,
			Method: "POST",
			Headers: { "Content-Type": "application/json" },
			Body: HttpService.JSONEncode({
				protocolVersion: PROTOCOL_VERSION,
				role: "plugin",
				instanceId: INSTANCE_ID,
				installId: installId(plugin),
				placeName: game.Name,
				placeId: game.PlaceId,
				userId: plugin.GetStudioUserId(),
			}),
		});
		// The server returns JSON on both success and a version mismatch (HTTP 409),
		// so read the error message out of the body rather than the raw status.
		const body = res.Body !== "" ? (HttpService.JSONDecode(res.Body) as { ok?: boolean; error?: string }) : {};
		if (res.Success && body.ok === true) return true;
		warn(`${LOG} handshake failed: ${body.error ?? `HTTP ${res.StatusCode}`}`);
		return false;
	} catch (err) {
		warn(`${LOG} cannot reach the bridge: ${err}. Is the server running and Allow HTTP Requests on?`);
		return false;
	}
}

function loop(): void {
	while (running) {
		try {
			const res = HttpService.RequestAsync({
				Url: `${BRIDGE}/poll?studio=${INSTANCE_ID}&role=plugin`,
				Method: "GET",
			});
			if (res.Success) {
				if (res.StatusCode === 205) {
					// The server has no registration for us (it restarted). Re-announce;
					// back off only if that fails, to avoid a re-hello storm on a mismatch.
					if (!announce()) task.wait(RECONNECT_WAIT_SECONDS);
				} else if (res.StatusCode === 200 && res.Body !== "") {
					// 204 means the long-poll window elapsed with nothing queued.
					handle(HttpService.JSONDecode(res.Body) as BridgeCommand);
				}
			} else {
				// Back off on an unexpected status so a broken endpoint cannot busy-spin.
				warn(`${LOG} poll returned HTTP ${res.StatusCode}; retrying.`);
				task.wait(RECONNECT_WAIT_SECONDS);
			}
		} catch (err) {
			warn(`${LOG} bridge unreachable: ${err}. Retrying.`);
			task.wait(RECONNECT_WAIT_SECONDS);
		}
	}
}

button.Click.Connect(() => {
	if (running) {
		running = false;
		button.SetActive(false);
		print(`${LOG} disconnected`);
		return;
	}
	if (!announce()) {
		button.SetActive(false);
		return;
	}
	sweepRunners(); // clear any runner scripts left behind by a crashed session
	running = true;
	button.SetActive(true);
	print(`${LOG} connected`);
	task.spawn(loop);
});
