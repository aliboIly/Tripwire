#!/usr/bin/env node
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { Bridge } from "./bridge.js";
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

const transport = new StdioServerTransport();
await server.connect(transport);
