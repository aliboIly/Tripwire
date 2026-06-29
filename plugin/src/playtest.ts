// Playtest lifecycle handlers (Phase 3). These run in the EDIT DataModel. The
// plugin injects the in-play runner (whose source the server ships in the command
// payload) into the place BEFORE starting the test, because the place is cloned at
// launch and a script cannot be pushed in afterward. It then starts the test on a
// spawned thread, since StudioTestService:Execute*Async blocks for the whole
// session, and cleans up the injected scripts when the session ends.
//
// StudioTestService is recent, so it is fetched defensively: if it is absent there
// is no programmatic F5, and start returns a result telling the human to press
// Play (the injected runner still attaches once the test runs).

import { BridgeCommand, CommandResult } from "protocol";
import { INSTANCE_ID } from "identity";

const ServerScriptService = game.GetService("ServerScriptService");
const StarterPlayer = game.GetService("StarterPlayer");
const ScriptEditorService = game.GetService("ScriptEditorService");

const RUNNER_NAME = "TripwireRunner";

interface StudioTestServiceLike {
	ExecutePlayModeAsync(args?: unknown): unknown;
	ExecuteRunModeAsync(args?: unknown): unknown;
}

// StudioTestService may not be in the typed Services map, so reach it through a
// cast on GetService and feature-detect at runtime.
function getStudioTestService(): StudioTestServiceLike | undefined {
	const [ok, service] = pcall(() =>
		(game as unknown as { GetService(name: string): unknown }).GetService("StudioTestService"),
	);
	if (!ok || service === undefined) return undefined;
	return service as StudioTestServiceLike;
}

function clientScriptsContainer(): Instance | undefined {
	return StarterPlayer.FindFirstChildOfClass("StarterPlayerScripts");
}

function removeRunners(): void {
	for (const container of [ServerScriptService, clientScriptsContainer()]) {
		if (container === undefined) continue;
		const existing = container.FindFirstChild(RUNNER_NAME);
		if (existing !== undefined) existing.Destroy();
	}
}

function injectRunner(source: string, runContext: Enum.RunContext, parent: Instance): void {
	const runner = new Instance("Script");
	runner.Name = RUNNER_NAME;
	runner.RunContext = runContext;
	runner.Parent = parent;
	// Write source through the editor service, never Script.Source directly.
	ScriptEditorService.UpdateSourceAsync(runner, () => source);
}

// Runs ExecutePlayMode/RunModeAsync on a spawned thread (it blocks until the
// session ends) and cleans up the injected scripts afterward.
function startSession(source: string, mode: "play" | "run"): CommandResult {
	removeRunners(); // clear any leftovers from a previous crashed session

	// Fill in our instanceId so the injected peers attach to this Studio. The server
	// templated the port and version; the instanceId is known only here.
	const [filled] = source.gsub("{{INSTANCE_ID}}", INSTANCE_ID);
	injectRunner(filled, Enum.RunContext.Server, ServerScriptService);
	if (mode === "play") {
		const clientContainer = clientScriptsContainer();
		if (clientContainer !== undefined) injectRunner(filled, Enum.RunContext.Client, clientContainer);
	}

	const studioTest = getStudioTestService();
	if (studioTest === undefined) {
		return {
			ok: true,
			data: {
				started: false,
				note: "StudioTestService is unavailable; press F5/F8 to start. The injected runner will connect on its own.",
			},
		};
	}

	// Defer, not spawn: task.spawn would run the closure synchronously up to its
	// first yield, and Execute*Async yields immediately, entering play mode before
	// this handler's start ack is posted (which could time out the start call).
	// task.defer lets the handler post its result first, then runs the blocking call.
	task.defer(() => {
		pcall(() => (mode === "play" ? studioTest.ExecutePlayModeAsync() : studioTest.ExecuteRunModeAsync()));
		removeRunners(); // the session ended; remove the injected scripts from edit
	});
	return { ok: true, data: { started: true, mode } };
}

export function handlePlaytest(cmd: BridgeCommand): CommandResult | undefined {
	if (cmd.type === "start_playtest") {
		const p = cmd.payload as { runnerSource: string };
		return startSession(p.runnerSource, "play");
	}
	if (cmd.type === "start_simulation") {
		const p = cmd.payload as { runnerSource: string };
		return startSession(p.runnerSource, "run");
	}
	return undefined;
}

// Sweep leftover runners when the plugin connects, in case a previous session
// crashed before cleanup.
export function sweepRunners(): void {
	removeRunners();
}
