// Open Cloud Luau Execution client (Phase 0c).
//
// Runs a Luau script headlessly in the configured place, in the real Roblox
// engine, and returns its logs. This is the headless-test backend.
//
// Verify the exact endpoint paths, request body, and response field names
// against the live docs before relying on this, since they can change:
//   https://create.roblox.com/docs/cloud/reference/features/luau-execution
//   reference impl: https://github.com/Roblox/place-ci-cd-demo
//
// Reads credentials from env: ROBLOX_OPEN_CLOUD_KEY, ROBLOX_UNIVERSE_ID, ROBLOX_PLACE_ID.

const BASE = "https://apis.roblox.com/cloud/v2";

export interface LuauResult {
  ok: boolean;
  logs: string[];
  error?: string;
}

export async function runLuau(script: string): Promise<LuauResult> {
  const key = process.env.ROBLOX_OPEN_CLOUD_KEY;
  const universe = process.env.ROBLOX_UNIVERSE_ID;
  const place = process.env.ROBLOX_PLACE_ID;
  if (!key || !universe || !place) {
    return {
      ok: false,
      logs: [],
      error: "Missing env: set ROBLOX_OPEN_CLOUD_KEY, ROBLOX_UNIVERSE_ID, ROBLOX_PLACE_ID.",
    };
  }

  const headers = { "x-api-key": key, "content-type": "application/json" };

  // 1. Create the execution task.
  const createUrl = `${BASE}/universes/${universe}/places/${place}/luau-execution-session-tasks`;
  const createRes = await fetch(createUrl, {
    method: "POST",
    headers,
    body: JSON.stringify({ script }),
  });
  if (!createRes.ok) {
    return { ok: false, logs: [], error: `create task failed: ${createRes.status} ${await createRes.text()}` };
  }
  const task = (await createRes.json()) as { path: string; state?: string };

  // 2. Poll until the task leaves a running state (5 min hard cap upstream).
  let state = task.state ?? "PROCESSING";
  for (let i = 0; i < 90 && (state === "PROCESSING" || state === "QUEUED"); i++) {
    await sleep(2000);
    const r = await fetch(`${BASE}/${task.path}`, { headers });
    const t = (await r.json()) as { state: string };
    state = t.state;
  }

  // 3. Fetch logs.
  const logsRes = await fetch(`${BASE}/${task.path}/logs`, { headers });
  const logsJson = (await logsRes.json()) as {
    luauExecutionSessionTaskLogs?: Array<{ messages: string[] }>;
  };
  const logs = (logsJson.luauExecutionSessionTaskLogs ?? []).flatMap((l) => l.messages);

  return {
    ok: state === "COMPLETE",
    logs,
    error: state === "COMPLETE" ? undefined : `task state: ${state}`,
  };
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}
