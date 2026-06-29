// Playtest tools (Phase 3). These route through the bridge to the right peer:
// start goes to the plugin (which injects the runner and starts the test); stop
// goes to the server runner (which calls StudioTestService:EndTest from inside the
// running game, since the plugin cannot reach into a separate play DataModel).
//
// Honest limits, surfaced in the tool descriptions: F5 stop is best-effort
// (tearing down two DataModels can outlast the confirmation window), and none of
// this is headless. The server ships the runner source in the start payload so the
// one hand-written Luau file (runner/runner.luau) stays the single source of truth.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Bridge, BridgeResult, PROTOCOL_VERSION, Role } from "./bridge.js";

const STOP_TIMEOUT_MS = 15000;

// server/dist -> repo root/runner/runner.luau
const RUNNER_PATH = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "runner", "runner.luau");

function runnerSource(port: number): string {
  const template = readFileSync(RUNNER_PATH, "utf8");
  return template
    .replace(/\{\{PORT\}\}/g, String(port))
    .replace(/\{\{PROTOCOL_VERSION\}\}/g, String(PROTOCOL_VERSION));
}

export async function startPlaytest(bridge: Bridge, port: number): Promise<BridgeResult> {
  return bridge.send("start_playtest", { runnerSource: runnerSource(port) }, "plugin");
}

export async function startSimulation(bridge: Bridge, port: number): Promise<BridgeResult> {
  return bridge.send("start_simulation", { runnerSource: runnerSource(port) }, "plugin");
}

// Both stops ask the server runner to call EndTest. F8 stops cleanly; F5 is
// best-effort, so a timeout is reported as "sent, may need a manual Stop" rather
// than a hard failure.
async function requestStop(bridge: Bridge, kind: "playtest" | "simulation"): Promise<BridgeResult> {
  try {
    return await bridge.send("stop", { kind }, "server", STOP_TIMEOUT_MS);
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    if (kind === "playtest") {
      return {
        ok: true,
        data: {
          stopped: "unconfirmed",
          note: `Stop was sent but not confirmed within ${STOP_TIMEOUT_MS / 1000}s (F5 teardown is best-effort). If the playtest is still running, press Stop. (${reason})`,
        },
      };
    }
    return { ok: false, error: reason };
  }
}

export async function stopPlaytest(bridge: Bridge): Promise<BridgeResult> {
  return requestStop(bridge, "playtest");
}

export async function stopSimulation(bridge: Bridge): Promise<BridgeResult> {
  return requestStop(bridge, "simulation");
}

interface LogEntry {
  message: string;
  type: string;
  timestamp: number;
}

interface PeerLogEntry extends LogEntry {
  peer: Role;
}

// Asks the server runner for the output log. Only the server peer talks to the
// bridge, so it returns its own entries plus the client's (fetched over the relay)
// as two arrays. The merge and dedup happen here (Node), never in-engine. A missing
// client (for example under F8) just yields an empty client array.
export async function getPlaytestOutput(bridge: Bridge): Promise<BridgeResult> {
  let result: BridgeResult;
  try {
    // 40s exceeds the runner's relay ceiling (8s client-ready wait plus 25s reply
    // window), so a slow client never trips this before the server returns its logs.
    result = await bridge.send("get_logs", {}, "server", 40000);
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
  if (!result.ok) return result;
  const data = result.data as { serverEntries?: LogEntry[]; clientEntries?: LogEntry[] } | undefined;

  // Keep every server entry (including legitimate repeats), then keep client
  // entries except those that echo a server message (Studio mirrors server output
  // into the client log). This removes cross-peer duplicates without collapsing a
  // peer's own repeated lines.
  const entries: PeerLogEntry[] = [];
  const serverKeys = new Set<string>();
  for (const entry of data?.serverEntries ?? []) {
    serverKeys.add(`${entry.timestamp}|${entry.message}`);
    entries.push({ ...entry, peer: "server" });
  }
  for (const entry of data?.clientEntries ?? []) {
    if (serverKeys.has(`${entry.timestamp}|${entry.message}`)) continue;
    entries.push({ ...entry, peer: "client" });
  }
  entries.sort((a, b) => a.timestamp - b.timestamp);
  return { ok: true, data: { entries } };
}
