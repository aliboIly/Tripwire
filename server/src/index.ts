#!/usr/bin/env node
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { Bridge, BridgeResult } from "./bridge.js";
import { runLuau } from "./cloud.js";
import { reviewSecurity, formatReport, SecurityReport } from "./security.js";
import {
  runTests as harnessRunTests,
  runTestFile,
  listTests as harnessListTests,
  writeTest,
  formatHarness,
  formatTestList,
} from "./harness.js";
import {
  startPlaytest,
  startSimulation,
  stopSimulation,
  stopPlaytest,
  getPlaytestOutput,
} from "./playtest.js";
import { uploadAsset } from "./assets.js";

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
    description: "Report whether a Studio plugin is connected, the active place, and any other connected studios.",
    inputSchema: {},
  },
  async () => ({ content: [{ type: "text", text: bridge.statusText() }] }),
);

server.registerTool(
  "list_studios",
  {
    description:
      "List every connected (or recently seen) Studio: instanceId, place, whether it is connected and active, how long ago it was last seen, and whether a playtest is running. Pure server state; works even with nothing connected.",
    inputSchema: {},
  },
  async () => ({ content: [{ type: "text", text: JSON.stringify(bridge.listStudios(), undefined, 2) }] }),
);

server.registerTool(
  "set_active_studio",
  {
    description:
      "Choose which connected Studio subsequent tools target, by exact instanceId, a unique id prefix, or a unique place name. With one Studio connected this is automatic.",
    inputSchema: { studio: z.string() },
  },
  async ({ studio }) => {
    const r = bridge.setActiveStudio(studio);
    return { content: [{ type: "text", text: r.ok ? (r.message ?? "ok") : `Error: ${r.error}` }] };
  },
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
  "run_tests",
  {
    description:
      "Run the headless test suite in the published place via Open Cloud and report passed/failed with failure messages. The place must be published (rbxtsc, rojo build, publish) and the Open Cloud env must be set.",
    inputSchema: {},
  },
  async () => ({ content: [{ type: "text", text: formatHarness(await harnessRunTests()) }] }),
);

server.registerTool(
  "run_test_file",
  {
    description: "Run a single spec by its ModuleScript name (for example 'economy.spec') headlessly via Open Cloud.",
    inputSchema: { file: z.string() },
  },
  async ({ file }) => ({ content: [{ type: "text", text: formatHarness(await runTestFile(file)) }] }),
);

server.registerTool(
  "list_tests",
  {
    description: "List the spec files and their cases discovered in the published place. Runs no tests.",
    inputSchema: {},
  },
  async () => ({ content: [{ type: "text", text: formatTestList(await harnessListTests()) }] }),
);

server.registerTool(
  "write_test",
  {
    description:
      "Write a roblox-ts test spec to disk as <name>.spec.ts under a directory (default sample-game/src/shared). It does not run the test; rebuild (rbxtsc) and publish the place, then run_tests picks it up. There is no watch_tests: Open Cloud runs the published place, so the loop is edit, rebuild, publish, run_tests.",
    inputSchema: { name: z.string(), source: z.string(), dir: z.string().optional() },
  },
  async ({ name, source, dir }) => {
    const r = writeTest({ name, source, dir });
    const text = r.ok
      ? `Wrote ${r.path}. Rebuild (rbxtsc) and publish the place, then run_tests.`
      : `Error: ${r.error}`;
    return { content: [{ type: "text", text }] };
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

server.registerTool(
  "search_objects",
  {
    description:
      "Find instances whose name contains a query (case-insensitive) under a path (default the whole game), optionally filtered by className. Read-only.",
    inputSchema: {
      query: z.string(),
      path: z.string().optional(),
      className: z.string().optional(),
      limit: z.number().int().min(1).optional(),
    },
  },
  async (args) => asText(await bridge.send("search_objects", args)),
);

server.registerTool(
  "search_by_property",
  {
    description:
      "Find instances whose property equals a value, under a path (default the whole game), optionally filtered by className. The value is a primitive (string, number, or boolean); datatype properties are matched by their string form. Read-only.",
    inputSchema: {
      property: z.string(),
      value: z.union([z.string(), z.number(), z.boolean()]),
      path: z.string().optional(),
      className: z.string().optional(),
      limit: z.number().int().min(1).optional(),
    },
  },
  async (args) => asText(await bridge.send("search_by_property", args)),
);

server.registerTool(
  "get_script_source",
  {
    description: "Read the source of a Script, LocalScript, or ModuleScript at the given path. Read-only.",
    inputSchema: { path: z.string() },
  },
  async ({ path }) => asText(await bridge.send("get_script_source", { path })),
);

server.registerTool(
  "grep_scripts",
  {
    description:
      "Search script sources for a substring (case-insensitive) under a path (default the whole game). Returns path, line number, and line text for each match. Read-only.",
    inputSchema: { pattern: z.string(), path: z.string().optional(), limit: z.number().int().min(1).optional() },
  },
  async (args) => asText(await bridge.send("grep_scripts", args)),
);

server.registerTool(
  "get_output_log",
  {
    description: "Return recent Studio output log entries (message, type, timestamp). Read-only.",
    inputSchema: { limit: z.number().int().min(1).optional() },
  },
  async ({ limit }) => asText(await bridge.send("get_output_log", { limit })),
);

server.registerTool(
  "get_selection",
  {
    description: "List the instances currently selected in Studio (name, className, full path). Read-only.",
    inputSchema: {},
  },
  async () => asText(await bridge.send("get_selection", {})),
);

// A property value is a tagged union so its Roblox datatype is explicit. The
// plugin reconstructs the real datatype from this before assigning.
const wireValue = z.union([
  z.object({ type: z.literal("primitive"), value: z.union([z.string(), z.number(), z.boolean()]) }),
  z.object({ type: z.literal("Vector3"), value: z.tuple([z.number(), z.number(), z.number()]) }),
  z.object({
    type: z.literal("Color3"),
    value: z.tuple([z.number(), z.number(), z.number()]),
    rgb255: z.boolean().optional(),
  }),
  z.object({ type: z.literal("UDim2"), value: z.tuple([z.number(), z.number(), z.number(), z.number()]) }),
  z.object({
    type: z.literal("CFrame"),
    value: z.array(z.number()).refine((a) => a.length === 3 || a.length === 12, "CFrame needs 3 or 12 numbers"),
  }),
  z.object({ type: z.literal("EnumItem"), enum: z.string(), item: z.string() }),
  z.object({ type: z.literal("instance"), path: z.string() }),
]);

server.registerTool(
  "create_instance",
  {
    description:
      "Create an instance of a class under a parent path (default the whole game), with an optional name and initial properties. One undo step. Returns the new instance's path.",
    inputSchema: {
      className: z.string(),
      parentPath: z.string().optional(),
      name: z.string().optional(),
      properties: z.array(z.object({ name: z.string(), value: wireValue })).optional(),
    },
  },
  async (args) => asText(await bridge.send("create_instance", args)),
);

server.registerTool(
  "delete_instance",
  {
    description: "Destroy the instance at the given path and its descendants. One undo step. Refuses to destroy the data model.",
    inputSchema: { path: z.string() },
  },
  async ({ path }) => asText(await bridge.send("delete_instance", { path })),
);

server.registerTool(
  "set_property",
  {
    description:
      "Set one property on the instance at the given path. The value is a typed datatype (primitive, Vector3, Color3, UDim2, CFrame, EnumItem, or an instance path). One undo step.",
    inputSchema: { path: z.string(), name: z.string(), value: wireValue },
  },
  async (args) => asText(await bridge.send("set_property", args)),
);

server.registerTool(
  "update_script_source",
  {
    description:
      "Replace the source of a Script, LocalScript, or ModuleScript at the given path, via the script editor (the supported write path). Reading source uses get_script_source.",
    inputSchema: { path: z.string(), source: z.string() },
  },
  async (args) => asText(await bridge.send("update_script_source", args)),
);

const createItem = z.object({
  className: z.string(),
  parentPath: z.string().optional(),
  name: z.string().optional(),
  properties: z.array(z.object({ name: z.string(), value: wireValue })).optional(),
});

server.registerTool(
  "insert_model",
  {
    description:
      "Insert an asset by id under a parent path (default Workspace). method 'load_asset' (default) loads creator-owned or Roblox assets; 'load_asset_async' can load public free Creator Store models, but the place must enable Game Settings > Security > Allow Loading Third Party Assets and such assets' scripts are sandboxed. Optional name, pivotTo (a CFrame to reposition), and unpack (reparent the wrapper's children, then remove it). One undo step.",
    inputSchema: {
      assetId: z.number().int(),
      parentPath: z.string().optional(),
      method: z.enum(["load_asset", "load_asset_async"]).optional(),
      name: z.string().optional(),
      unpack: z.boolean().optional(),
      pivotTo: wireValue.optional(),
    },
  },
  async (args) => asText(await bridge.send("insert_model", args, "plugin", 60000)),
);

server.registerTool(
  "mass_create",
  {
    description:
      "Create many instances in one undo step. atomic:true rolls all back if any item fails; otherwise it is best-effort and returns per-item successes and failures.",
    inputSchema: { items: z.array(createItem), atomic: z.boolean().optional() },
  },
  async (args) => asText(await bridge.send("mass_create", args, "plugin", 60000)),
);

server.registerTool(
  "mass_set_property",
  {
    description:
      "Set one property on each of many instances in one undo step. atomic:true rolls all back if any item fails; otherwise best-effort with per-item results.",
    inputSchema: {
      items: z.array(z.object({ path: z.string(), name: z.string(), value: wireValue })),
      atomic: z.boolean().optional(),
    },
  },
  async (args) => asText(await bridge.send("mass_set_property", args, "plugin", 60000)),
);

server.registerTool(
  "upload_asset",
  {
    description:
      "Upload a local file as a Roblox asset via Open Cloud (Decal, Audio, Model, Animation, or Video) and return its assetId. Needs ROBLOX_OPEN_CLOUD_KEY (assets read+write scope) and ROBLOX_CREATOR_USER_ID or ROBLOX_CREATOR_GROUP_ID in the server env. The upload is asynchronous; this polls until it is processed, and reports a moderation rejection as an error.",
    inputSchema: {
      filePath: z.string(),
      assetType: z.enum(["Decal", "Audio", "Model", "Animation", "Video"]),
      displayName: z.string(),
      description: z.string().optional(),
      contentType: z.string().optional(),
    },
  },
  async (args) => {
    const r = await uploadAsset(args);
    const text = r.ok
      ? `Uploaded. assetId: ${r.assetId}${r.revisionId !== undefined ? `, revisionId: ${r.revisionId}` : ""}`
      : `Error: ${r.error}`;
    return { content: [{ type: "text", text }] };
  },
);

server.registerTool(
  "start_playtest",
  {
    description:
      "Start an F5 playtest (separate server and client DataModels with a player). The plugin injects the in-play runner first, then starts the test via StudioTestService; if that is unavailable, press F5 and the runner still attaches. Live Studio only, not headless.",
    inputSchema: {},
  },
  async () => asText(await startPlaytest(bridge, BRIDGE_PORT)),
);

server.registerTool(
  "start_simulation",
  {
    description:
      "Start an F8 run (server-only simulation, no client peer or player). The plugin injects the server runner and starts the run. Input simulation is not available under F8. Live Studio only.",
    inputSchema: {},
  },
  async () => asText(await startSimulation(bridge, BRIDGE_PORT)),
);

server.registerTool(
  "stop_simulation",
  {
    description: "Stop an F8 run. The server runner calls StudioTestService:EndTest; this is a clean, reliable stop.",
    inputSchema: {},
  },
  async () => asText(await stopSimulation(bridge)),
);

server.registerTool(
  "stop_playtest",
  {
    description:
      "Stop an F5 playtest. Best-effort: F5 tears down two DataModels and can outlast the confirmation window, in which case you may need to press Stop. The server runner calls StudioTestService:EndTest.",
    inputSchema: {},
  },
  async () => asText(await stopPlaytest(bridge)),
);

server.registerTool(
  "simulate_mouse_input",
  {
    description:
      "Simulate a mouse click or move at screen coordinates during an F5 playtest. Runs on the client peer, so it requires start_playtest (not start_simulation). action 'click' presses and releases; 'move' just moves the cursor.",
    inputSchema: {
      x: z.number(),
      y: z.number(),
      button: z.enum(["left", "right", "middle"]).optional(),
      action: z.enum(["click", "move"]).optional(),
    },
  },
  // Routed to the server peer, which relays it to the client (only the server can
  // reach the bridge); the timeout covers the relay's client-ready and reply waits.
  async (args) => asText(await bridge.send("mouse_input", args, "server", 40000)),
);

server.registerTool(
  "simulate_keyboard_input",
  {
    description:
      "Simulate keyboard input during an F5 playtest: a key by KeyCode name (for example 'Space' or 'W') with action tap/press/release, or typed text. Runs on the client peer, so it requires start_playtest.",
    inputSchema: {
      key: z.string().optional(),
      text: z.string().optional(),
      action: z.enum(["tap", "press", "release"]).optional(),
    },
  },
  async (args) => asText(await bridge.send("keyboard_input", args, "server", 40000)),
);

server.registerTool(
  "character_navigation",
  {
    description:
      "Walk the local player's character toward a world position during an F5 playtest (Humanoid:MoveTo). Returns whether it reached the goal; reached=false can mean the ~8s move timeout. Client peer, so it requires start_playtest.",
    inputSchema: { x: z.number(), y: z.number(), z: z.number() },
  },
  // Routed to the server peer, which relays to the client. 40s covers the relay's
  // client-ready wait (8s) plus the reply window (25s, which includes the Humanoid
  // wait and the ~8s MoveTo self-timeout).
  async (args) => asText(await bridge.send("character_navigation", args, "server", 40000)),
);

server.registerTool(
  "get_playtest_output",
  {
    description:
      "Return the running playtest's output log, aggregated across the server and client peers and tagged by peer.",
    inputSchema: {},
  },
  async () => asText(await getPlaytestOutput(bridge)),
);

// The security tools run a local static analysis over a Rojo source tree. They
// need no Studio session or Open Cloud key. The default target assumes the server
// runs from the repo root.
const DEFAULT_SECURITY_TARGET = "sample-game/src";

function securityText(
  path: string | undefined,
  render: (report: SecurityReport) => string,
): { content: Array<{ type: "text"; text: string }> } {
  try {
    return { content: [{ type: "text", text: render(reviewSecurity(path ?? DEFAULT_SECURITY_TARGET)) }] };
  } catch (err) {
    return { content: [{ type: "text", text: `Error: ${err instanceof Error ? err.message : String(err)}` }] };
  }
}

server.registerTool(
  "scan_remotes",
  {
    description:
      "List RemoteEvent and RemoteFunction server handlers in a Rojo source tree (default sample-game/src), with the client-controlled parameters of each. Static analysis, read-only.",
    inputSchema: { path: z.string().optional() },
  },
  async ({ path }) => securityText(path, (r) => JSON.stringify(r.handlers, undefined, 2)),
);

server.registerTool(
  "scan_client_trust",
  {
    description:
      "Scan a Rojo source tree (default sample-game/src) for client-trust holes: server handlers that use client-supplied values without validating them. Static analysis, read-only.",
    inputSchema: { path: z.string().optional() },
  },
  async ({ path }) => securityText(path, (r) => JSON.stringify(r.findings, undefined, 2)),
);

server.registerTool(
  "review_security",
  {
    description:
      "Review a Rojo source tree (default sample-game/src) for client-trust and unvalidated-remote issues, and report each finding with a suggested server-side fix. Static analysis, read-only.",
    inputSchema: { path: z.string().optional() },
  },
  async ({ path }) => securityText(path, formatReport),
);

const transport = new StdioServerTransport();
await server.connect(transport);
