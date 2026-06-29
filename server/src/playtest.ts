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

interface PeerLogEntry {
  message: string;
  type: string;
  timestamp: number;
  peer: Role;
}

// Aggregates the output log across the server and client runners. A peer that is
// not present (for example no client under F8) simply contributes nothing rather
// than failing the call. Entries are merged, deduped by timestamp+message, and
// sorted oldest first; the merge happens here, never in-engine.
export async function getPlaytestOutput(bridge: Bridge): Promise<BridgeResult> {
  const peers: Role[] = ["server", "client"];
  const settled = await Promise.allSettled(peers.map((role) => bridge.send("get_logs", {}, role, 5000)));

  const entriesFrom = (index: number): Array<{ message: string; type: string; timestamp: number }> => {
    const result = settled[index];
    if (result.status !== "fulfilled" || !result.value.ok) return [];
    const data = result.value.data as { entries?: Array<{ message: string; type: string; timestamp: number }> } | undefined;
    return data?.entries ?? [];
  };

  // Keep every server entry (including legitimate repeats), then keep client
  // entries except those that echo a server message (Studio mirrors server output
  // into the client log). This removes cross-peer duplicates without collapsing a
  // peer's own repeated lines.
  const entries: PeerLogEntry[] = [];
  const serverKeys = new Set<string>();
  for (const entry of entriesFrom(0)) {
    serverKeys.add(`${entry.timestamp}|${entry.message}`);
    entries.push({ message: entry.message, type: entry.type, timestamp: entry.timestamp, peer: "server" });
  }
  for (const entry of entriesFrom(1)) {
    if (serverKeys.has(`${entry.timestamp}|${entry.message}`)) continue;
    entries.push({ message: entry.message, type: entry.type, timestamp: entry.timestamp, peer: "client" });
  }
  entries.sort((a, b) => a.timestamp - b.timestamp);
  return { ok: true, data: { entries } };
}
