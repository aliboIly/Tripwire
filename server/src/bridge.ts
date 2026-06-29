import http from "node:http";
import { randomUUID } from "node:crypto";

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

// The bridge is the local HTTP server the Studio plugin long-polls.
// MCP tool handlers call send(); the plugin pulls commands from /poll,
// executes them in Studio, and posts results to /result.
//
// PHASE 3 TODO: route commands by target (plugin vs injected in-play runner).
// Right now both pollers would compete for the same queue. The runner is only
// injected during play, so this is fine for Phase 0 but must be fixed before
// in-play tooling lands.
export class Bridge {
  private queue: Command[] = [];
  private pending = new Map<string, Pending>();
  private waiters: Array<(c: Command | null) => void> = [];
  connected = false;
  placeName = "(unknown)";

  send(type: string, payload: unknown, timeoutMs = 30000): Promise<BridgeResult> {
    const id = randomUUID();
    const cmd: Command = { id, type, payload };
    return new Promise<BridgeResult>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(
          new Error(
            `Tripwire bridge: command '${type}' timed out after ${timeoutMs}ms. ` +
              `Is the Studio plugin connected and "Allow HTTP Requests" enabled?`,
          ),
        );
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      const waiter = this.waiters.shift();
      if (waiter) waiter(cmd);
      else this.queue.push(cmd);
    });
  }

  start(port: number): http.Server {
    const srv = http.createServer((req, res) => {
      const url = req.url ?? "/";
      if (req.method === "POST" && url === "/hello") {
        return this.readBody(req, (b) => {
          const info = safeJson(b) as { placeName?: string } | null;
          this.connected = true;
          if (info?.placeName) this.placeName = info.placeName;
          json(res, 200, { ok: true });
        });
      }
      if (req.method === "GET" && url === "/poll") return this.handlePoll(res);
      if (req.method === "POST" && url === "/result") {
        return this.readBody(req, (b) => {
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
      json(res, 404, { ok: false, error: "not found" });
    });
    srv.listen(port, "127.0.0.1");
    return srv;
  }

  private handlePoll(res: http.ServerResponse): void {
    const next = this.queue.shift();
    if (next) {
      json(res, 200, next);
      return;
    }
    // Long-poll: park the request for up to 20s waiting for a command.
    const waiter = (c: Command | null): void => {
      clearTimeout(timer);
      if (c) json(res, 200, c);
      else res.writeHead(204).end();
    };
    this.waiters.push(waiter);
    const timer = setTimeout(() => {
      const i = this.waiters.indexOf(waiter);
      if (i >= 0) this.waiters.splice(i, 1);
      res.writeHead(204).end();
    }, 20000);
  }

  private readBody(req: http.IncomingMessage, cb: (body: string) => void): void {
    let data = "";
    req.on("data", (c) => (data += c));
    req.on("end", () => cb(data));
  }
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
