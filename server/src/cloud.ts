// Open Cloud Luau Execution client (Phase 0c).
//
// Runs a Luau script in the configured place on a real RCC server through Open
// Cloud, then returns its console logs and any return values. This is the
// backend the headless test harness builds on.
//
// Contract verified against the official guide:
//   https://create.roblox.com/docs/cloud/reference/features/luau-execution
// Credentials come from env only: ROBLOX_OPEN_CLOUD_KEY, ROBLOX_UNIVERSE_ID,
// ROBLOX_PLACE_ID. The key is sent in the x-api-key header and never logged.

const CLOUD_BASE = "https://apis.roblox.com/cloud/v2";
const POLL_INTERVAL_MS = 3000;
const TASK_MAX_DURATION_MS = 5 * 60 * 1000; // platform hard cap per task
const POLL_DEADLINE_MS = TASK_MAX_DURATION_MS + 30_000; // a little headroom for the queue
const TRANSIENT_ATTEMPTS = 3;
const LOG_PAGE_SIZE = 10_000;

// Open Cloud reports progress through this field. QUEUED and PROCESSING are
// in-flight; the other three are terminal. The stub used to loop only while
// PROCESSING, which wrongly treated a still-QUEUED task as finished.
type TaskState = "STATE_UNSPECIFIED" | "QUEUED" | "PROCESSING" | "COMPLETE" | "FAILED" | "CANCELLED";
const TERMINAL_STATES: ReadonlySet<TaskState> = new Set<TaskState>(["COMPLETE", "FAILED", "CANCELLED"]);

interface LuauTask {
  // Canonical resource path returned by create. The session id in it is
  // server-generated, so we reuse this path verbatim for polling and logs
  // rather than rebuilding the URL.
  path: string;
  state: TaskState;
  output?: { results?: unknown[] };
  error?: { code?: string; message?: string };
}

interface TaskLogsResponse {
  luauExecutionSessionTaskLogs?: Array<{ messages?: string[] }>;
  nextPageToken?: string;
}

export interface LuauResult {
  ok: boolean;
  logs: string[];
  results?: unknown[];
  error?: string;
}

export async function runLuau(script: string): Promise<LuauResult> {
  const creds = readCredentials();
  if (!creds.ok) return { ok: false, logs: [], error: creds.error };

  let task: LuauTask;
  try {
    task = await createTask(creds.key, creds.universe, creds.place, script);
  } catch (err) {
    return { ok: false, logs: [], error: `create task failed: ${messageOf(err)}` };
  }

  let final: LuauTask;
  try {
    final = await pollUntilTerminal(creds.key, task.path);
  } catch (err) {
    return { ok: false, logs: [], error: `polling failed: ${messageOf(err)}` };
  }

  // Logs are a separate channel from return values and are best effort: a failed
  // log fetch should not turn an otherwise good run into a failure.
  const logs = await fetchLogs(creds.key, final.path).catch(() => []);

  if (final.state === "COMPLETE") {
    return { ok: true, logs, results: final.output?.results ?? [] };
  }
  const reason =
    final.state === "FAILED" ? final.error?.message ?? "unknown error" : final.state.toLowerCase();
  return { ok: false, logs, error: `task ${final.state}: ${reason}` };
}

type Credentials =
  | { ok: true; key: string; universe: string; place: string }
  | { ok: false; error: string };

function readCredentials(): Credentials {
  const key = process.env.ROBLOX_OPEN_CLOUD_KEY;
  const universe = process.env.ROBLOX_UNIVERSE_ID;
  const place = process.env.ROBLOX_PLACE_ID;
  if (!key || !universe || !place) {
    return {
      ok: false,
      error: "Missing env: set ROBLOX_OPEN_CLOUD_KEY, ROBLOX_UNIVERSE_ID, ROBLOX_PLACE_ID.",
    };
  }
  return { ok: true, key, universe, place };
}

async function createTask(
  key: string,
  universe: string,
  place: string,
  script: string,
): Promise<LuauTask> {
  // No version segment, so this runs against the latest published place version.
  // Open Cloud cannot see unsaved Studio edits, so the place must be published.
  const url = `${CLOUD_BASE}/universes/${universe}/places/${place}/luau-execution-session-tasks`;
  const res = await cloudFetch(key, url, { method: "POST", body: JSON.stringify({ script }) });
  const task = (await res.json()) as Partial<LuauTask>;
  if (!task.path) throw new Error("create response had no task path");
  return { path: task.path, state: task.state ?? "QUEUED", output: task.output, error: task.error };
}

async function pollUntilTerminal(key: string, path: string): Promise<LuauTask> {
  const url = `${CLOUD_BASE}/${path}`;
  const deadline = Date.now() + POLL_DEADLINE_MS;
  for (;;) {
    const task = (await (await cloudFetch(key, url, { method: "GET" })).json()) as LuauTask;
    if (TERMINAL_STATES.has(task.state)) return task;
    if (Date.now() > deadline) {
      throw new Error(
        `task did not finish within ${Math.round(POLL_DEADLINE_MS / 1000)}s (last state ${task.state})`,
      );
    }
    await sleep(POLL_INTERVAL_MS);
  }
}

async function fetchLogs(key: string, path: string): Promise<string[]> {
  const lines: string[] = [];
  let pageToken: string | undefined;
  do {
    const url = new URL(`${CLOUD_BASE}/${path}/logs`);
    url.searchParams.set("maxPageSize", String(LOG_PAGE_SIZE));
    if (pageToken) url.searchParams.set("pageToken", pageToken);
    const body = (await (await cloudFetch(key, url.toString(), { method: "GET" })).json()) as TaskLogsResponse;
    for (const entry of body.luauExecutionSessionTaskLogs ?? []) {
      if (entry.messages) lines.push(...entry.messages);
    }
    pageToken = body.nextPageToken || undefined;
  } while (pageToken);
  return lines;
}

interface FetchInit {
  method: string;
  body?: string;
}

// Wraps fetch with the auth header and a short retry on transient failures
// (network errors, 429, 5xx). The key lives only in the header, so URLs and
// error text never carry it.
async function cloudFetch(key: string, url: string, init: FetchInit): Promise<Response> {
  const headers: Record<string, string> = { "x-api-key": key };
  if (init.body !== undefined) headers["content-type"] = "application/json";

  let lastError = "";
  for (let attempt = 1; attempt <= TRANSIENT_ATTEMPTS; attempt++) {
    let res: Response | undefined;
    try {
      res = await fetch(url, { method: init.method, headers, body: init.body });
    } catch (err) {
      lastError = messageOf(err);
    }
    if (res) {
      if (res.ok) return res;
      const detail = `${res.status} ${await safeText(res)}`.trim();
      if (!isTransient(res.status)) throw new Error(`${init.method} ${url} responded ${detail}`);
      lastError = detail;
    }
    if (attempt < TRANSIENT_ATTEMPTS) await sleep(attempt * POLL_INTERVAL_MS);
  }
  throw new Error(`${init.method} ${url} failed after ${TRANSIENT_ATTEMPTS} attempts: ${lastError}`);
}

function isTransient(status: number): boolean {
  return status === 429 || status >= 500;
}

function messageOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

async function safeText(res: Response): Promise<string> {
  try {
    return await res.text();
  } catch {
    return "";
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
