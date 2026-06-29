// Open Cloud REST tools beyond Luau execution: DataStores, Ordered DataStores,
// MessagingService, Memory Stores, platform info, and engagement.
//
// Same env-only credentials as cloud.ts (ROBLOX_OPEN_CLOUD_KEY, ROBLOX_UNIVERSE_ID,
// and ROBLOX_PLACE_ID where a place is involved). Each call works when the key has
// the matching scope and surfaces Roblox's own error (an insufficient-scope 401/403,
// a 404, etc.) otherwise, so capabilities are gated by the key, not by us. Endpoint
// shapes were confirmed against the live docs: cloud/v2 for most, plus the separate
// ordered-data-stores/v1 base. cloud/v2 carries value/users/attributes as JSON body
// fields (the content-md5 and roblox-entry-* headers were the old v1 API).

const CLOUD = "https://apis.roblox.com/cloud/v2";
const ORDERED = "https://apis.roblox.com/ordered-data-stores/v1";
const DEFAULT_SCOPE = "global";
const TRANSIENT_ATTEMPTS = 3;
const RETRY_BASE_MS = 1000;

export interface OcResult {
  ok: boolean;
  data?: unknown;
  error?: string;
}

function key(): string | undefined {
  return process.env.ROBLOX_OPEN_CLOUD_KEY;
}
function universe(): string | undefined {
  return process.env.ROBLOX_UNIVERSE_ID;
}
function place(): string | undefined {
  return process.env.ROBLOX_PLACE_ID;
}

// Returns a "Missing env: ..." message if any named var is unset, else undefined.
function need(...vars: Array<[string, string | undefined]>): string | undefined {
  const missing = vars.filter(([, v]) => v === undefined || v === "").map(([n]) => n);
  return missing.length ? `Missing env: set ${missing.join(", ")}.` : undefined;
}

interface OcRequest {
  method: string;
  url: string;
  body?: unknown;
}

// Adds x-api-key, retries transient failures (429/5xx), returns parsed JSON (or {}
// for an empty 2xx body), and throws a clear message on a non-transient error. The
// key is never logged.
async function oc(req: OcRequest): Promise<unknown> {
  const headers: Record<string, string> = { "x-api-key": key() as string };
  let payload: string | undefined;
  if (req.body !== undefined) {
    headers["Content-Type"] = "application/json";
    payload = JSON.stringify(req.body);
  }
  let lastError = "";
  for (let attempt = 1; attempt <= TRANSIENT_ATTEMPTS; attempt++) {
    let res: Response | undefined;
    try {
      res = await fetch(req.url, { method: req.method, headers, body: payload });
    } catch (err) {
      lastError = err instanceof Error ? err.message : String(err);
    }
    if (res) {
      const text = await res.text();
      if (res.ok) {
        if (text.trim() === "") return {};
        try {
          return JSON.parse(text);
        } catch {
          return {};
        }
      }
      const detail = `${res.status} ${text}`.trim();
      if (res.status !== 429 && res.status < 500) throw new Error(`${req.method} ${req.url} -> ${detail}`);
      lastError = detail;
    }
    if (attempt < TRANSIENT_ATTEMPTS) await sleep(attempt * RETRY_BASE_MS);
  }
  throw new Error(`${req.method} ${req.url} failed after ${TRANSIENT_ATTEMPTS} attempts: ${lastError}`);
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// Gates an operation on the env it needs, runs it, and normalizes errors to OcResult.
async function run(envError: string | undefined, op: () => Promise<unknown>): Promise<OcResult> {
  if (envError !== undefined) return { ok: false, error: envError };
  try {
    return { ok: true, data: await op() };
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : String(err) };
  }
}

function enc(s: string): string {
  return encodeURIComponent(s);
}

function qs(params: Record<string, string | number | boolean | undefined>): string {
  const parts = Object.entries(params)
    .filter(([, v]) => v !== undefined && v !== "")
    .map(([k, v]) => `${k}=${encodeURIComponent(String(v))}`);
  return parts.length ? `?${parts.join("&")}` : "";
}

function asRecord(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
}

// ===== Standard DataStores (cloud/v2, universe-scoped) =====

export function listDatastores(p: { prefix?: string; maxPageSize?: number; pageToken?: string }): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({
      method: "GET",
      url:
        `${CLOUD}/universes/${u}/data-stores` +
        qs({ maxPageSize: p.maxPageSize, pageToken: p.pageToken, filter: prefixFilter(p.prefix) }),
    }),
  );
}

export function listDatastoreEntries(p: {
  datastore: string;
  prefix?: string;
  maxPageSize?: number;
  pageToken?: string;
}): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({
      method: "GET",
      url:
        `${CLOUD}/universes/${u}/data-stores/${enc(p.datastore)}/entries` +
        qs({ maxPageSize: p.maxPageSize, pageToken: p.pageToken, filter: prefixFilter(p.prefix) }),
    }),
  );
}

export function getDatastoreEntry(p: { datastore: string; entry: string }): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({ method: "GET", url: `${CLOUD}/universes/${u}/data-stores/${enc(p.datastore)}/entries/${enc(p.entry)}` }),
  );
}

// Upsert (SetAsync semantics): value is always sent; users/attributes are cleared if
// omitted, so they are always echoed.
export function setDatastoreEntry(p: {
  datastore: string;
  entry: string;
  value: unknown;
  users?: string[];
  attributes?: Record<string, unknown>;
}): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({
      method: "PATCH",
      url: `${CLOUD}/universes/${u}/data-stores/${enc(p.datastore)}/entries/${enc(p.entry)}?allowMissing=true`,
      body: { value: p.value, users: p.users ?? [], attributes: p.attributes ?? {} },
    }),
  );
}

export function deleteDatastoreEntry(p: { datastore: string; entry: string }): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({ method: "DELETE", url: `${CLOUD}/universes/${u}/data-stores/${enc(p.datastore)}/entries/${enc(p.entry)}` }),
  );
}

// Custom verb: the action follows a colon on the resource path (not URL-encoded).
export function incrementDatastoreEntry(p: {
  datastore: string;
  entry: string;
  amount: number;
  users?: string[];
  attributes?: Record<string, unknown>;
}): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({
      method: "POST",
      url: `${CLOUD}/universes/${u}/data-stores/${enc(p.datastore)}/entries/${enc(p.entry)}:increment`,
      body: { amount: p.amount, users: p.users ?? [], attributes: p.attributes ?? {} },
    }),
  );
}

function prefixFilter(prefix?: string): string | undefined {
  return prefix !== undefined && prefix !== "" ? `id.startsWith("${prefix}")` : undefined;
}

// ===== Ordered DataStores (ordered-data-stores/v1, separate base) =====

export function listOrderedEntries(p: {
  store: string;
  scope?: string;
  descending?: boolean;
  maxPageSize?: number;
  pageToken?: string;
  filter?: string;
}): Promise<OcResult> {
  const u = universe();
  const scope = p.scope ?? DEFAULT_SCOPE;
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({
      method: "GET",
      url:
        `${ORDERED}/universes/${u}/orderedDataStores/${enc(p.store)}/scopes/${enc(scope)}/entries` +
        qs({
          max_page_size: p.maxPageSize,
          page_token: p.pageToken,
          order_by: p.descending ? "desc" : undefined,
          filter: p.filter,
        }),
    }),
  );
}

export function getOrderedEntry(p: { store: string; scope?: string; entry: string }): Promise<OcResult> {
  const u = universe();
  const scope = p.scope ?? DEFAULT_SCOPE;
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({
      method: "GET",
      url: `${ORDERED}/universes/${u}/orderedDataStores/${enc(p.store)}/scopes/${enc(scope)}/entries/${enc(p.entry)}`,
    }),
  );
}

// Overwrite (upsert when allow_missing). Ordered stores hold non-negative integers.
export function setOrderedEntry(p: { store: string; scope?: string; entry: string; value: number }): Promise<OcResult> {
  const u = universe();
  const scope = p.scope ?? DEFAULT_SCOPE;
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({
      method: "PATCH",
      url: `${ORDERED}/universes/${u}/orderedDataStores/${enc(p.store)}/scopes/${enc(scope)}/entries/${enc(p.entry)}?allow_missing=true`,
      body: { value: p.value },
    }),
  );
}

export function incrementOrderedEntry(p: {
  store: string;
  scope?: string;
  entry: string;
  amount: number;
}): Promise<OcResult> {
  const u = universe();
  const scope = p.scope ?? DEFAULT_SCOPE;
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({
      method: "POST",
      url: `${ORDERED}/universes/${u}/orderedDataStores/${enc(p.store)}/scopes/${enc(scope)}/entries/${enc(p.entry)}:increment`,
      body: { amount: p.amount },
    }),
  );
}

// ===== MessagingService =====

// Reaches only running production servers subscribed via MessagingService; there is
// no read side, and it does not reach Studio.
export function publishMessage(p: { topic: string; message: string }): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({ method: "POST", url: `${CLOUD}/universes/${u}:publishMessage`, body: { topic: p.topic, message: p.message } }),
  );
}

// ===== Memory Stores (cloud/v2). ttl is a duration string like "300s". =====

function ttlString(seconds?: number): string | undefined {
  return seconds !== undefined ? `${seconds}s` : undefined;
}

export function memorySortedMapSet(p: {
  map: string;
  item: string;
  value: unknown;
  ttlSeconds?: number;
  stringSortKey?: string;
  numericSortKey?: number;
}): Promise<OcResult> {
  const u = universe();
  const body: Record<string, unknown> = { value: p.value };
  const ttl = ttlString(p.ttlSeconds);
  if (ttl !== undefined) body.ttl = ttl;
  if (p.stringSortKey !== undefined) body.stringSortKey = p.stringSortKey;
  if (p.numericSortKey !== undefined) body.numericSortKey = p.numericSortKey;
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({
      method: "PATCH",
      url: `${CLOUD}/universes/${u}/memory-store/sorted-maps/${enc(p.map)}/items/${enc(p.item)}?allowMissing=true`,
      body,
    }),
  );
}

export function memorySortedMapGet(p: { map: string; item: string }): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({ method: "GET", url: `${CLOUD}/universes/${u}/memory-store/sorted-maps/${enc(p.map)}/items/${enc(p.item)}` }),
  );
}

export function memorySortedMapList(p: {
  map: string;
  descending?: boolean;
  maxPageSize?: number;
  pageToken?: string;
  filter?: string;
}): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), async () => {
    const data = asRecord(
      await oc({
        method: "GET",
        url:
          `${CLOUD}/universes/${u}/memory-store/sorted-maps/${enc(p.map)}/items` +
          qs({
            maxPageSize: p.maxPageSize,
            pageToken: p.pageToken,
            orderBy: p.descending ? "value desc" : undefined,
            filter: p.filter,
          }),
      }),
    );
    // Live spec drift: the array is documented as memoryStoreSortedMapItems but the
    // server has returned it as items; surface both.
    return { items: data.items ?? data.memoryStoreSortedMapItems ?? [], nextPageToken: data.nextPageToken };
  });
}

export function memorySortedMapDelete(p: { map: string; item: string }): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({ method: "DELETE", url: `${CLOUD}/universes/${u}/memory-store/sorted-maps/${enc(p.map)}/items/${enc(p.item)}` }),
  );
}

export function memoryQueueAdd(p: {
  queue: string;
  data: unknown;
  priority?: number;
  ttlSeconds?: number;
}): Promise<OcResult> {
  const u = universe();
  const body: Record<string, unknown> = { data: p.data };
  if (p.priority !== undefined) body.priority = p.priority;
  const ttl = ttlString(p.ttlSeconds);
  if (ttl !== undefined) body.ttl = ttl;
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({ method: "POST", url: `${CLOUD}/universes/${u}/memory-store/queues/${enc(p.queue)}/items`, body }),
  );
}

// Read-and-remove is two calls: read here, then discard the returned readId before
// the invisibility window elapses or the items reappear.
export function memoryQueueRead(p: {
  queue: string;
  count?: number;
  invisibilitySeconds?: number;
  allOrNothing?: boolean;
}): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), async () => {
    const data = asRecord(
      await oc({
        method: "GET",
        url:
          `${CLOUD}/universes/${u}/memory-store/queues/${enc(p.queue)}/items:read` +
          qs({
            count: p.count,
            allOrNothing: p.allOrNothing,
            invisibilityWindow: p.invisibilitySeconds !== undefined ? `${p.invisibilitySeconds}s` : undefined,
          }),
      }),
    );
    // Live spec drift: readId has come back as id, and items as queueItems.
    return { readId: data.readId ?? data.id, items: data.items ?? data.queueItems ?? [] };
  });
}

export function memoryQueueDiscard(p: { queue: string; readId: string }): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({
      method: "POST",
      url: `${CLOUD}/universes/${u}/memory-store/queues/${enc(p.queue)}/items:discard`,
      body: { readId: p.readId },
    }),
  );
}

// ===== Platform info (read-only) =====

export function getUniverse(): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({ method: "GET", url: `${CLOUD}/universes/${u}` }),
  );
}

export function getPlace(): Promise<OcResult> {
  const u = universe();
  const pl = place();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u], ["ROBLOX_PLACE_ID", pl]), () =>
    oc({ method: "GET", url: `${CLOUD}/universes/${u}/places/${pl}` }),
  );
}

export function getUser(p: { userId: string }): Promise<OcResult> {
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()]), () => oc({ method: "GET", url: `${CLOUD}/users/${enc(p.userId)}` }));
}

export function getGroup(p: { groupId: string }): Promise<OcResult> {
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()]), () =>
    oc({ method: "GET", url: `${CLOUD}/groups/${enc(p.groupId)}` }),
  );
}

export function listInventory(p: {
  userId: string;
  filter?: string;
  maxPageSize?: number;
  pageToken?: string;
}): Promise<OcResult> {
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()]), () =>
    oc({
      method: "GET",
      url:
        `${CLOUD}/users/${enc(p.userId)}/inventory-items` +
        qs({ maxPageSize: p.maxPageSize, pageToken: p.pageToken, filter: p.filter }),
    }),
  );
}

// ===== Engagement =====

// messageId is a notification-string template authored in the Creator Dashboard, not
// free text. Each user can receive only one notification per day per experience.
export function sendNotification(p: {
  userId: string;
  messageId: string;
  parameters?: Record<string, { stringValue?: string; int64Value?: number }>;
  launchData?: string;
}): Promise<OcResult> {
  const u = universe();
  const payload: Record<string, unknown> = { type: "MOMENT", messageId: p.messageId };
  if (p.parameters !== undefined) payload.parameters = p.parameters;
  if (p.launchData !== undefined) payload.joinExperience = { launchData: p.launchData };
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({
      method: "POST",
      url: `${CLOUD}/users/${enc(p.userId)}/notifications`,
      body: { source: { universe: `universes/${u}` }, payload },
    }),
  );
}

// subscriptionId is the subscriber's user id. Reading other users' subscriptions
// needs the broader universe:write scope on the key.
export function getSubscription(p: {
  subscriptionProductId: string;
  userId: string;
  full?: boolean;
}): Promise<OcResult> {
  const u = universe();
  return run(need(["ROBLOX_OPEN_CLOUD_KEY", key()], ["ROBLOX_UNIVERSE_ID", u]), () =>
    oc({
      method: "GET",
      url:
        `${CLOUD}/universes/${u}/subscription-products/${enc(p.subscriptionProductId)}/subscriptions/${enc(p.userId)}` +
        qs({ view: p.full ? "FULL" : undefined }),
    }),
  );
}
