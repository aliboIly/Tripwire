#!/usr/bin/env node
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { Bridge, BridgeResult } from "./bridge.js";
import { runLuau } from "./cloud.js";

// This process speaks MCP over stdout. Never console.log to stdout, since it
// corrupts the protocol stream. Diagnostics go to stderr.
//
// Targets @modelcontextprotocol/sdk v1 (registerTool API). inputSchema is a raw
// shape: an object of zod schemas, not z.object(...).

const BRIDGE_PORT = Number(process.env.TRIPWIRE_BRIDGE_PORT ?? 44331);
const bridge = new Bridge();
bridge.start(BRIDGE_PORT);

const server = new McpServer({ name: "tripwire", version: "0.0.1" });

// Bridge results carry arbitrary tool data; render it as pretty JSON for the
// model, or surface the error text when the plugin reported a failure.
function asText(r: BridgeResult): { content: Array<{ type: "text"; text: string }> } {
  const text = r.ok ? JSON.stringify(r.data, undefined, 2) : `Error: ${r.error}`;
  return { content: [{ type: "text", text }] };
}

server.registerTool(
  "studio_status",
  {
    description: "Report whether the Tripwire Studio plugin is connected, and the open place name.",
    inputSchema: {},
  },
  async () => ({
    content: [
      {
        type: "text",
        text: bridge.connected
          ? `Connected. Place: ${bridge.placeName}`
          : bridge.lastError
            ? `Not connected: ${bridge.lastError}`
            : 'Plugin not connected. Open Studio, install the Tripwire plugin, click its toolbar button, and enable "Allow HTTP Requests".',
      },
    ],
  }),
);

server.registerTool(
  "ping_studio",
  {
    description: "Round-trip a ping through the Studio plugin to confirm the live bridge works (Phase 0a).",
    inputSchema: { message: z.string().default("hello") },
  },
  async ({ message }) => {
    const r = await bridge.send("ping", { message });
    return {
      content: [
        {
          type: "text",
          text: r.ok ? `Studio replied: ${JSON.stringify(r.data)}` : `Bridge error: ${r.error}`,
        },
      ],
    };
  },
);

server.registerTool(
  "run_luau",
  {
    description:
      "Run a Luau script headlessly in the configured place via Open Cloud Luau Execution; returns logs (Phase 0c).",
    inputSchema: { script: z.string() },
  },
  async ({ script }) => {
    const r = await runLuau(script);
    const lines: string[] = [r.ok ? "OK" : `FAILED: ${r.error}`];
    if (r.results && r.results.length > 0) lines.push(`return: ${JSON.stringify(r.results)}`);
    if (r.logs.length > 0) lines.push("logs:", ...r.logs);
    return { content: [{ type: "text", text: lines.join("\n") }] };
  },
);

server.registerTool(
  "get_file_tree",
  {
    description:
      "List the instance tree of the open place from a path (default the whole game), bounded by depth. Read-only. Path is dot-separated from the data model root, e.g. 'Workspace.Folder'.",
    inputSchema: { path: z.string().optional(), maxDepth: z.number().int().min(0).optional() },
  },
  async ({ path, maxDepth }) => asText(await bridge.send("get_file_tree", { path, maxDepth })),
);

server.registerTool(
  "get_instance_children",
  {
    description:
      "List the direct children (name and className) of the instance at the given path (default the whole game). Read-only.",
    inputSchema: { path: z.string().optional() },
  },
  async ({ path }) => asText(await bridge.send("get_instance_children", { path })),
);

server.registerTool(
  "get_instance_properties",
  {
    description:
      "Read an instance's name, className, full data-model path, and attributes at the given path. Read-only.",
    inputSchema: { path: z.string().optional() },
  },
  async ({ path }) => asText(await bridge.send("get_instance_properties", { path })),
);

const transport = new StdioServerTransport();
await server.connect(transport);
