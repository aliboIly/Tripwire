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

import { BridgeCommand, CommandResult, TRIPWIRE_VERSION } from "protocol";
import { INSTANCE_ID } from "identity";

const ServerScriptService = game.GetService("ServerScriptService");
const StarterPlayer = game.GetService("StarterPlayer");
const ScriptEditorService = game.GetService("ScriptEditorService");

const RUNNER_NAME = "TripwireRunner";

// Both take a required args Variant (the test can read it back via GetTestArgs).
// args is non-optional on purpose: omitting it throws "Argument 1 missing or nil".
interface StudioTestServiceLike {
	ExecutePlayModeAsync(args: unknown): unknown;
	ExecuteRunModeAsync(args: unknown): unknown;
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

function injectRunner(source: string, kind: "server" | "client", parent: Instance): void {
	let runner: Script | LocalScript;
	if (kind === "server") {
		const serverScript = new Instance("Script");
		serverScript.RunContext = Enum.RunContext.Server;
		runner = serverScript;
	} else {
		// A LocalScript runs once, when copied into PlayerScripts. A Script with
		// RunContext.Client in StarterPlayerScripts runs twice (once in the container,
		// once in the copy), which would double every relayed input.
		runner = new Instance("LocalScript");
	}
	runner.Name = RUNNER_NAME;
	runner.Parent = parent;
	// Write source through the editor service, never Script.Source directly.
	ScriptEditorService.UpdateSourceAsync(runner, () => source);
}

// Runs ExecutePlayMode/RunModeAsync on a spawned thread (it blocks until the
// session ends) and cleans up the injected scripts afterward.
function startSession(source: string, mode: "play" | "run"): CommandResult {
	removeRunners(); // clear any leftovers from a previous crashed session

	// Fill in our instanceId so the injected peers attach to this Studio, and the
	// build version for the runner's log prefix. The server templated the port and
	// protocol version; these two are known only here.
	const [withId] = source.gsub("{{INSTANCE_ID}}", INSTANCE_ID);
	const [filled] = withId.gsub("{{VERSION}}", TRIPWIRE_VERSION);
	injectRunner(filled, "server", ServerScriptService);
	if (mode === "play") {
		const clientContainer = clientScriptsContainer();
		if (clientContainer !== undefined) injectRunner(filled, "client", clientContainer);
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
		// Surface a start failure in the Output. Execute*Async blocks until the session
		// ends, so on success this returns only afterwards (nothing to warn); an
		// immediate throw (API rejected the call) is reported here instead of vanishing.
		// Pass an empty args table; the runner self-bootstraps and does not read it.
		const [ok, err] = pcall(() =>
			mode === "play" ? studioTest.ExecutePlayModeAsync({}) : studioTest.ExecuteRunModeAsync({}),
		);
		if (!ok) warn(`[Tripwire v${TRIPWIRE_VERSION}] start ${mode} failed: ${err}`);
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
