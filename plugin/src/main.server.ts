// Tripwire Studio plugin. Phase 0 handshake plus Phase 1 read commands (roblox-ts).
//
// It long-polls the local Tripwire bridge, runs commands against the open place,
// and posts results back. The toolbar button opens the Tripwire panel (a dock
// widget in the style of Rojo's); connecting and disconnecting happen there.

import { HttpService } from "@rbxts/services";
import { BridgeCommand, CommandResult, PROTOCOL_VERSION, TRIPWIRE_VERSION } from "protocol";
import { INSTANCE_ID, installId } from "identity";
import { handleBackdoor } from "backdoor";
import { handleCapture } from "capture";
import { handleEdit } from "edit";
import { handlePlaytest, sweepRunners } from "playtest";
import { handleRead } from "read";
import { handleScripts } from "scripts";
import { handleSpatial } from "spatial";
import { handleStudio } from "studio";
import { createPanel } from "ui";

const BRIDGE = "http://127.0.0.1:44331";
const RECONNECT_WAIT_SECONDS = 3;
const LOG = `[Tripwire v${TRIPWIRE_VERSION}]`;

const toolbar = plugin.CreateToolbar("Tripwire");
const button = toolbar.CreateButton("Tripwire", "Open the Tripwire panel", "");

let running = false;
// Bumped on every connect and disconnect. A poll that was in flight across a
// quick disconnect and reconnect would otherwise revive its old loop and leave
// two loops polling the bridge.
let session = 0;

const panel = createPanel(plugin, BRIDGE, () => {
	if (running) disconnect();
	else connect();
});

button.Click.Connect(() => panel.toggle());
panel.onVisibleChanged((visible) => button.SetActive(visible));
button.SetActive(panel.isVisible());

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
		handleRead(cmd) ??
		handleScripts(cmd) ??
		handleStudio(cmd) ??
		handleEdit(cmd) ??
		handlePlaytest(cmd) ??
		handleBackdoor(cmd) ??
		handleSpatial(cmd) ??
		handleCapture(cmd);
	if (handled !== undefined) return handled;
	return { ok: false, error: `unknown command: ${cmd.type}` };
}

function handle(cmd: BridgeCommand): void {
	panel.noteCommand(cmd.type);
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
// Returns undefined on success and a human-readable failure otherwise (unreachable
// bridge, version mismatch), which the caller logs and shows in the panel.
function announce(): string | undefined {
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
		if (res.Success && body.ok === true) return undefined;
		return `handshake failed: ${body.error ?? `HTTP ${res.StatusCode}`}`;
	} catch (err) {
		return `cannot reach the bridge: ${err}. Is the server running and Allow HTTP Requests on?`;
	}
}

function loop(mySession: number): void {
	while (running && session === mySession) {
		try {
			const res = HttpService.RequestAsync({
				Url: `${BRIDGE}/poll?studio=${INSTANCE_ID}&role=plugin`,
				Method: "GET",
			});
			if (session !== mySession) {
				// The user disconnected while this poll was in flight. If it carried
				// a command, fail it now: the server already dequeued it, so nothing
				// else will answer and the pending call would sit out its timeout.
				if (res.Success && res.StatusCode === 200 && res.Body !== "") {
					const cmd = HttpService.JSONDecode(res.Body) as BridgeCommand;
					post("/result", { id: cmd.id, ok: false, error: "plugin disconnected" });
				}
				break;
			}
			if (res.Success) {
				if (res.StatusCode === 205) {
					// The server has no registration for us (it restarted). Re-announce;
					// back off only if that fails, to avoid a re-hello storm on a mismatch.
					const failure = announce();
					// announce() yields too; a disconnect during it must not repaint the panel.
					if (session !== mySession) break;
					if (failure !== undefined) {
						warn(`${LOG} ${failure}`);
						panel.setStatus("reconnecting", failure);
						task.wait(RECONNECT_WAIT_SECONDS);
					} else {
						panel.setStatus("connected");
					}
				} else if (res.StatusCode === 200 && res.Body !== "") {
					// 204 means the long-poll window elapsed with nothing queued.
					panel.setStatus("connected");
					handle(HttpService.JSONDecode(res.Body) as BridgeCommand);
				} else {
					panel.setStatus("connected");
				}
			} else {
				// Back off on an unexpected status so a broken endpoint cannot busy-spin.
				warn(`${LOG} poll returned HTTP ${res.StatusCode}; retrying.`);
				panel.setStatus("reconnecting", `The bridge returned HTTP ${res.StatusCode}. Retrying.`);
				task.wait(RECONNECT_WAIT_SECONDS);
			}
		} catch (err) {
			if (session !== mySession) break;
			warn(`${LOG} bridge unreachable: ${err}. Retrying.`);
			panel.setStatus("reconnecting", "Bridge unreachable. Is the server still running?");
			task.wait(RECONNECT_WAIT_SECONDS);
		}
	}
}

function connect(): void {
	panel.setStatus("connecting");
	const failure = announce();
	if (failure !== undefined) {
		warn(`${LOG} ${failure}`);
		panel.setStatus("disconnected", failure);
		return;
	}
	sweepRunners(); // clear any runner scripts left behind by a crashed session
	running = true;
	session += 1;
	panel.setStatus("connected");
	print(`${LOG} connected`);
	const current = session;
	task.spawn(() => loop(current));
}

function disconnect(): void {
	running = false;
	session += 1;
	panel.setStatus("disconnected");
	print(`${LOG} disconnected`);
}
