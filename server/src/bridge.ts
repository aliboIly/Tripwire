import http from "node:http";
import { randomUUID } from "node:crypto";

// Wire-contract version. Bumped to 2 for the multi-instance hello (instanceId is
// now required). A peer sending the wrong version is rejected via the 409 path so
// a stale plugin fails loudly instead of registering without an id.
export const PROTOCOL_VERSION = 2;

// Who a command is for, within a Studio. The edit-side plugin polls as "plugin" and
// the injected in-play server runner polls as "server". "client" is a relay role:
// the client cannot poll (HttpService is server-only), so the server runner forwards
// client commands over a RemoteEvent. No client peer ever polls the bridge.
export type Role = "plugin" | "server" | "client";
const ROLES: readonly Role[] = ["plugin", "server", "client"];

const POLL_PARK_MS = 20000;
const DEFAULT_TIMEOUT_MS = 30000;
// A peer is considered live if seen within this window. Must stay above the poll
// park window plus slack, or a healthy idle peer flickers to disconnected.
const STALE_MS = 45000;

export interface Command {
  id: string;
  type: string;
  payload: unknown;
}

export interface BridgeResult {
  ok: boolean;
  data?: unknown;
  error?: string;
}

export interface StudioSummary {
  instanceId: string;
  placeName: string;
  placeId?: number;
  userId?: number;
  connected: boolean;
  lastSeenMsAgo: number;
  active: boolean;
  playtestActive: boolean;
}

interface Pending {
  resolve: (v: BridgeResult) => void;
  reject: (e: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

type Waiter = (c: Command | null) => void;

// One connected Studio. The role-keyed broker (queues + waiters) lives per Studio
// so two Studios, and a playtest's server/client peers, never take each other's work.
interface Studio {
  instanceId: string;
  installId?: string;
  placeName: string;
  placeId?: number;
  userId?: number;
  connectedAt: number;
  queues: Map<Role, Command[]>;
  waiters: Map<Role, Waiter[]>;
  peers: Map<Role, number>; // role -> last-seen epoch ms
}

export class Bridge {
  private studios = new Map<string, Studio>();
  private pending = new Map<string, Pending>();
  private activeId?: string;
  lastError?: string;
  private bridgeReady = false;

  send(
    type: string,
    payload: unknown,
    target: Role = "plugin",
    timeoutMs = DEFAULT_TIMEOUT_MS,
  ): Promise<BridgeResult> {
    if (!this.bridgeReady) {
      return Promise.reject(new Error(this.lastError ?? "Tripwire bridge is not running."));
    }
    const studio = this.resolveActive();
    if (studio === undefined) {
      return Promise.reject(
        new Error("No Studio connected. Open Studio, click the Tripwire button, and enable Allow HTTP Requests."),
      );
    }
    // Fail fast on a missing peer rather than waiting out the timeout: the plugin
    // must be live, and a runner target must have an active playtest.
    if (target === "plugin") {
      if (!this.isLive(studio, "plugin")) return Promise.reject(new Error(this.staleMessage(studio)));
    } else if (!this.isLive(studio, target)) {
      return Promise.reject(new Error(`No live ${target} peer for '${studio.placeName}'. Start a playtest first.`));
    }

    const id = randomUUID();
    const cmd: Command = { id, type, payload };
    return new Promise<BridgeResult>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        // Drop it from the targeted Studio's queue so a command the caller gave up
        // on cannot be delivered later.
        studio.queues.set(target, queueFor(studio, target).filter((c) => c.id !== id));
        reject(
          new Error(
            `Tripwire bridge: command '${type}' to ${target} on '${studio.placeName}' timed out after ${timeoutMs}ms.`,
          ),
        );
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      const waiter = waitersFor(studio, target).shift();
      if (waiter) waiter(cmd);
      else queueFor(studio, target).push(cmd);
    });
  }

  start(port: number): http.Server {
    const srv = http.createServer((req, res) => {
      const url = req.url ?? "/";
      const path = url.split("?")[0];
      if (req.method === "POST" && path === "/hello") return this.handleHello(req, res);
      if (req.method === "GET" && path === "/poll") {
        const params = pollParams(url);
        return this.handlePoll(res, params.studio, params.role);
      }
      if (req.method === "POST" && path === "/result") return this.handleResult(req, res);
      json(res, 404, { ok: false, error: "not found" });
    });
    srv.on("error", (err: NodeJS.ErrnoException) => {
      this.bridgeReady = false;
      this.lastError =
        err.code === "EADDRINUSE"
          ? `bridge port ${port} is already in use, most likely by another Tripwire server. ` +
            `Close the other one, then reconnect; Studio tools are unavailable until then.`
          : `bridge failed to start: ${err.message}`;
      process.stderr.write(`[tripwire] ${this.lastError}\n`);
    });
    srv.on("listening", () => {
      this.bridgeReady = true;
    });
    srv.listen(port, "127.0.0.1");
    return srv;
  }

  // The active Studio's status line for studio_status.
  statusText(): string {
    const active = this.resolveActive();
    if (active === undefined) {
      return this.lastError
        ? `Not connected: ${this.lastError}`
        : "No Studio connected. Open Studio, click the Tripwire button, and enable Allow HTTP Requests.";
    }
    const others = [...this.studios.values()].filter((s) => s !== active && this.isLive(s, "plugin")).length;
    const suffix = others > 0 ? ` (+${others} other connected studio${others === 1 ? "" : "s"})` : "";
    return `Connected. Active place: ${active.placeName}${suffix}`;
  }

  listStudios(): StudioSummary[] {
    const now = Date.now();
    return [...this.studios.values()]
      .map((s) => ({
        instanceId: s.instanceId,
        placeName: s.placeName,
        placeId: s.placeId,
        userId: s.userId,
        connected: this.isLive(s, "plugin"),
        lastSeenMsAgo: now - (s.peers.get("plugin") ?? 0),
        active: s.instanceId === this.activeId,
        // Only the server peer polls during a playtest (the client is relayed), so its
        // liveness is what "a playtest is running" means.
        playtestActive: this.isLive(s, "server"),
      }))
      .sort((a, b) => a.lastSeenMsAgo - b.lastSeenMsAgo);
  }

  // Selects the active Studio by exact instanceId, a unique id prefix, or a unique
  // place name (in that order), among live Studios.
  setActiveStudio(identifier: string): { ok: boolean; message?: string; error?: string } {
    const live = [...this.studios.values()].filter((s) => this.isLive(s, "plugin"));
    if (live.length === 0) return { ok: false, error: "no Studio is currently connected" };
    let matches = live.filter((s) => s.instanceId === identifier);
    if (matches.length === 0) matches = live.filter((s) => s.instanceId.startsWith(identifier));
    if (matches.length === 0) matches = live.filter((s) => s.placeName === identifier);
    if (matches.length === 0) return { ok: false, error: `no connected studio matches '${identifier}'` };
    if (matches.length > 1) {
      const names = matches.map((s) => `${s.placeName} (${s.instanceId.slice(0, 8)})`).join(", ");
      return { ok: false, error: `'${identifier}' matches several studios: ${names}. Be more specific.` };
    }
    this.activeId = matches[0].instanceId;
    return { ok: true, message: `active studio set to ${matches[0].placeName} (${matches[0].instanceId.slice(0, 8)})` };
  }

  private handleHello(req: http.IncomingMessage, res: http.ServerResponse): void {
    this.readBody(req, (b) => {
      const info = safeJson(b) as {
        instanceId?: string;
        role?: Role;
        protocolVersion?: number;
        installId?: string;
        placeName?: string;
        placeId?: number;
        userId?: number;
      } | null;
      if (info?.protocolVersion !== PROTOCOL_VERSION) {
        this.lastError =
          `protocol mismatch: peer sent ${info?.protocolVersion ?? "none"}, ` +
          `server speaks ${PROTOCOL_VERSION}. Rebuild and reinstall the plugin.`;
        return json(res, 409, { ok: false, error: this.lastError });
      }
      if (info.instanceId === undefined) return json(res, 400, { ok: false, error: "hello is missing instanceId" });
      const role: Role = info.role === "server" || info.role === "client" ? info.role : "plugin";

      let studio = this.studios.get(info.instanceId);
      if (studio === undefined) {
        studio = newStudio(info.instanceId, info.placeName ?? "(unknown)");
        this.studios.set(info.instanceId, studio);
      }
      if (info.installId !== undefined) studio.installId = info.installId;
      if (info.placeName !== undefined) studio.placeName = info.placeName;
      if (info.placeId !== undefined) studio.placeId = info.placeId;
      if (info.userId !== undefined) studio.userId = info.userId;
      studio.peers.set(role, Date.now());

      if (role === "plugin") {
        this.lastError = undefined;
        // Default the active Studio to this one if there isn't a live active already.
        const active = this.activeId ? this.studios.get(this.activeId) : undefined;
        if (active === undefined || !this.isLive(active, "plugin")) this.activeId = info.instanceId;
      }
      json(res, 200, { ok: true });
    });
  }

  private handlePoll(res: http.ServerResponse, instanceId: string | undefined, role: Role): void {
    const studio = instanceId !== undefined ? this.studios.get(instanceId) : undefined;
    if (studio === undefined) {
      // No registration for this peer (server restarted, or never saw its hello).
      // 205 tells the peer to re-announce; do not park.
      res.writeHead(205).end();
      return;
    }
    studio.peers.set(role, Date.now()); // heartbeat at arrival, before parking

    const next = queueFor(studio, role).shift();
    if (next) {
      json(res, 200, next);
      return;
    }
    const waiter: Waiter = (c) => {
      clearTimeout(timer);
      if (c) json(res, 200, c);
      else res.writeHead(204).end();
    };
    waitersFor(studio, role).push(waiter);
    const timer = setTimeout(() => {
      const arr = waitersFor(studio, role);
      const i = arr.indexOf(waiter);
      if (i >= 0) arr.splice(i, 1);
      res.writeHead(204).end();
    }, POLL_PARK_MS);
  }

  private handleResult(req: http.IncomingMessage, res: http.ServerResponse): void {
    this.readBody(req, (b) => {
      const r = safeJson(b) as ({ id: string } & BridgeResult) | null;
      if (r && this.pending.has(r.id)) {
        const p = this.pending.get(r.id)!;
        clearTimeout(p.timer);
        this.pending.delete(r.id);
        p.resolve({ ok: r.ok, data: r.data, error: r.error });
      }
      json(res, 200, { ok: true });
    });
  }

  private isLive(studio: Studio, role: Role): boolean {
    const seen = studio.peers.get(role);
    return seen !== undefined && Date.now() - seen < STALE_MS;
  }

  private staleMessage(studio: Studio): string {
    const seen = studio.peers.get("plugin");
    const ago = seen !== undefined ? `${Math.round((Date.now() - seen) / 1000)}s ago` : "never";
    return `active Studio '${studio.placeName}' last seen ${ago}; click the Tripwire button and enable Allow HTTP Requests.`;
  }

  // The active Studio if it is live, otherwise the most-recently-connected live one
  // (which it adopts as active), otherwise undefined.
  private resolveActive(): Studio | undefined {
    const active = this.activeId !== undefined ? this.studios.get(this.activeId) : undefined;
    if (active !== undefined && this.isLive(active, "plugin")) return active;
    let best: Studio | undefined;
    for (const s of this.studios.values()) {
      if (this.isLive(s, "plugin") && (best === undefined || s.connectedAt > best.connectedAt)) best = s;
    }
    if (best !== undefined) this.activeId = best.instanceId;
    return best;
  }

  private readBody(req: http.IncomingMessage, cb: (body: string) => void): void {
    let data = "";
    req.on("data", (c) => (data += c));
    req.on("end", () => cb(data));
  }
}

function newStudio(instanceId: string, placeName: string): Studio {
  return {
    instanceId,
    placeName,
    connectedAt: Date.now(),
    queues: new Map(ROLES.map((r) => [r, []])),
    waiters: new Map(ROLES.map((r) => [r, []])),
    peers: new Map(),
  };
}

function queueFor(studio: Studio, role: Role): Command[] {
  return studio.queues.get(role)!;
}

function waitersFor(studio: Studio, role: Role): Waiter[] {
  return studio.waiters.get(role)!;
}

function pollParams(url: string): { studio?: string; role: Role } {
  const query = url.split("?")[1];
  const params = query !== undefined ? new URLSearchParams(query) : new URLSearchParams();
  const role = params.get("role");
  return {
    studio: params.get("studio") ?? undefined,
    role: role === "server" || role === "client" ? role : "plugin",
  };
}

function json(res: http.ServerResponse, code: number, body: unknown): void {
  res.writeHead(code, { "content-type": "application/json" }).end(JSON.stringify(body));
}

function safeJson(s: string): unknown {
  try {
    return JSON.parse(s);
  } catch {
    return null;
  }
}
