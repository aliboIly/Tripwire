// Tripwire Studio plugin. Phase 0 handshake (roblox-ts).
//
// This is the plugin's entry script. The plugin/ project is already scaffolded
// (roblox-ts plugin template). Build it from plugin/ with:
//   npm install && npx rbxtsc && rojo build --output Tripwire.rbxmx
// (The old `rbxtsc init` scaffolder is gone; today you'd scaffold a fresh
//  plugin project with `npm create roblox-ts@latest` and pick the plugin template.)
//
// It long-polls the local Tripwire bridge, executes commands against the place,
// and posts results back. Authored in TypeScript (roblox-ts), like the rest of the plugin.

import { HttpService } from "@rbxts/services";

const BRIDGE = "http://127.0.0.1:44331";

const toolbar = plugin.CreateToolbar("Tripwire");
const button = toolbar.CreateButton("Tripwire", "Connect to the Tripwire MCP bridge", "");
let running = false;

function post(path: string, body: object): void {
	pcall(() => {
		HttpService.RequestAsync({
			Url: `${BRIDGE}${path}`,
			Method: "POST",
			Headers: { "Content-Type": "application/json" },
			Body: HttpService.JSONEncode(body),
		});
	});
}

interface BridgeCommand {
	id: string;
	type: string;
	payload: unknown;
}

function handle(cmd: BridgeCommand): void {
	// Phase 0 implements only 'ping'. Later phases dispatch real tools here.
	if (cmd.type === "ping") {
		post("/result", { id: cmd.id, ok: true, data: { place: game.Name, payload: cmd.payload } });
	} else {
		post("/result", { id: cmd.id, ok: false, error: `unknown command: ${cmd.type}` });
	}
}

function loop(): void {
	while (running) {
		const [ok, err] = pcall(() => {
			const res = HttpService.RequestAsync({ Url: `${BRIDGE}/poll`, Method: "GET" });
			if (res.Success && res.StatusCode === 200 && res.Body !== "") {
				handle(HttpService.JSONDecode(res.Body) as BridgeCommand);
			}
		});
		if (!ok) {
			warn(`[Tripwire] bridge unreachable: ${err}. Is the server running and HTTP requests allowed?`);
			task.wait(3);
		}
	}
}

button.Click.Connect(() => {
	running = !running;
	button.SetActive(running);
	if (running) {
		post("/hello", { placeName: game.Name });
		print("[Tripwire] connected");
		task.spawn(loop);
	} else {
		print("[Tripwire] disconnected");
	}
});
