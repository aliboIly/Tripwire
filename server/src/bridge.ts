import http from "node:http";
import { randomUUID } from "node:crypto";

// Wire-contract version. The plugin and runners send their version on /hello; a
// mismatch is rejected loudly so a stale peer fails clearly instead of dropping
// fields. Bump this on every peer whenever a command or result shape changes.
export const PROTOCOL_VERSION = 1;

// Who a command is for. The edit-side plugin polls as "plugin"; the injected
// in-play runners poll as "server" or "client". Routing by role is what stops the
// plugin and the runners from dequeuing each other's commands.
export type Role = "plugin" | "server" | "client";
const ROLES: readonly Role[] = ["plugin", "server", "client"];

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

interface Pending {
  resolve: (v: BridgeResult) => void;
  reject: (e: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

type Waiter = (c: Command | null) => void;

const POLL_PARK_MS = 20000;
const DEFAULT_TIMEOUT_MS = 30000;

// The bridge is the local HTTP server the Studio plugin and the injected in-play
// runners long-poll. MCP tool handlers call send() with a target role; each peer
// pulls only its own role's commands from /poll and posts results to /result.
export class Bridge {
  private queues: Map<Role, Command[]> = new Map(ROLES.map((r) => [r, []]));
  private waiters: Map<Role, Waiter[]> = new Map(ROLES.map((r) => [r, []]));
  private pending = new Map<string, Pending>();
  connected = false;
  placeName = "(unknown)";
  lastError?: string;
  peers = new Set<Role>();
  private bridgeReady = false;

  send(
    type: string,
    payload: unknown,
    target: Role = "plugin",
    timeoutMs = DEFAULT_TIMEOUT_MS,
  ): Promise<BridgeResult> {
    // Fail fast if the bridge never bound (for example another Tripwire server
    // already holds the port) rather than making every call wait out the timeout.
    if (!this.bridgeReady) {
      return Promise.reject(new Error(this.lastError ?? "Tripwire bridge is not running."));
    }
    const id = randomUUID();
    const cmd: Command = { id, type, payload };
    return new Promise<BridgeResult>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        // Drop it from the target's queue too, so a command the caller gave up on
        // cannot be delivered and run later.
        this.queues.set(target, this.queueFor(target).filter((c) => c.id !== id));
        reject(
          new Error(
            `Tripwire bridge: command '${type}' to ${target} timed out after ${timeoutMs}ms. ` +
              `Is the ${target} peer connected and "Allow HTTP Requests" enabled?`,
          ),
        );
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      const waiter = this.waitersFor(target).shift();
      if (waiter) waiter(cmd);
      else this.queueFor(target).push(cmd);
    });
  }

  start(port: number): http.Server {
    const srv = http.createServer((req, res) => {
      const url = req.url ?? "/";
      const path = url.split("?")[0];
      if (req.method === "POST" && path === "/hello") return this.handleHello(req, res);
      if (req.method === "GET" && path === "/poll") return this.handlePoll(res, roleFromUrl(url));
      if (req.method === "POST" && path === "/result") return this.handleResult(req, res);
      json(res, 404, { ok: false, error: "not found" });
    });
    // Handle a bind failure instead of letting the unhandled error event crash the
    // whole MCP server. The most common cause is another Tripwire server on this
    // port; the MCP still serves every tool that does not need Studio.
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

  private handleHello(req: http.IncomingMessage, res: http.ServerResponse): void {
    this.readBody(req, (b) => {
      const info = safeJson(b) as { placeName?: string; protocolVersion?: number; role?: Role } | null;
      if (info?.protocolVersion !== PROTOCOL_VERSION) {
        this.connected = false;
        this.lastError =
          `protocol mismatch: peer sent ${info?.protocolVersion ?? "none"}, ` +
          `server speaks ${PROTOCOL_VERSION}. Rebuild and reinstall the plugin.`;
        return json(res, 409, { ok: false, error: this.lastError });
      }
      const role: Role = info.role === "server" || info.role === "client" ? info.role : "plugin";
      this.peers.add(role);
      // "connected" tracks the edit-side plugin specifically, which is what
      // studio_status reports; the runners are transient per playtest.
      if (role === "plugin") {
        this.connected = true;
        this.lastError = undefined;
        if (info.placeName) this.placeName = info.placeName;
      }
      json(res, 200, { ok: true });
    });
  }

  private handlePoll(res: http.ServerResponse, role: Role): void {
    const next = this.queueFor(role).shift();
    if (next) {
      json(res, 200, next);
      return;
    }
    // Long-poll: park this role's request for a window, answering when a command
    // for that role arrives, or empty on timeout.
    const waiter: Waiter = (c) => {
      clearTimeout(timer);
      if (c) json(res, 200, c);
      else res.writeHead(204).end();
    };
    this.waitersFor(role).push(waiter);
    const timer = setTimeout(() => {
      const arr = this.waitersFor(role);
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

  private queueFor(role: Role): Command[] {
    return this.queues.get(role)!;
  }

  private waitersFor(role: Role): Waiter[] {
    return this.waiters.get(role)!;
  }

  private readBody(req: http.IncomingMessage, cb: (body: string) => void): void {
    let data = "";
    req.on("data", (c) => (data += c));
    req.on("end", () => cb(data));
  }
}

function roleFromUrl(url: string): Role {
  const query = url.split("?")[1];
  if (!query) return "plugin";
  const role = new URLSearchParams(query).get("role");
  return role === "server" || role === "client" ? role : "plugin";
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
