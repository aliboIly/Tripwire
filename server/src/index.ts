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
import { publishPlace } from "./publish.js";
import {
  OcResult,
  listDatastores,
  listDatastoreEntries,
  getDatastoreEntry,
  setDatastoreEntry,
  deleteDatastoreEntry,
  incrementDatastoreEntry,
  listOrderedEntries,
  getOrderedEntry,
  setOrderedEntry,
  incrementOrderedEntry,
  publishMessage,
  memorySortedMapSet,
  memorySortedMapGet,
  memorySortedMapList,
  memorySortedMapDelete,
  memoryQueueAdd,
  memoryQueueRead,
  memoryQueueDiscard,
  getUniverse,
  getPlace,
  getUser,
  getGroup,
  listInventory,
  sendNotification,
  getSubscription,
} from "./opencloud.js";
import { config as loadEnv } from "dotenv";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// Load secrets from a .env at the repo root if present, so the Open Cloud keys are
// available whether the server runs manually or as an MCP server (the MCP launcher
// does not source a shell profile). process.env still wins, so an MCP "env" block
// overrides the file. Studio tools need no key; this is only for the Open Cloud
// features (run_luau, the headless tests, upload_asset). Must run before any env
// read below. quiet:true suppresses dotenv's banner, which would otherwise corrupt
// the MCP stdout stream.
loadEnv({ path: join(dirname(fileURLToPath(import.meta.url)), "..", "..", ".env"), quiet: true });

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
  "publish_place",
  {
    description:
      "Publish a local place file (.rbxl or .rbxlx) as a new version of the configured experience via Open Cloud. Needs ROBLOX_OPEN_CLOUD_KEY (with the universe-places write scope), ROBLOX_UNIVERSE_ID, and ROBLOX_PLACE_ID in the server env. Use it to put a built test place (harness + specs) into the place that run_tests reads. versionType 'Published' (default) goes live; 'Saved' uploads a draft. Fails with a conflict if Studio has the place open with an active session.",
    inputSchema: { filePath: z.string(), versionType: z.enum(["Published", "Saved"]).optional() },
  },
  async ({ filePath, versionType }) => {
    const r = await publishPlace({ filePath, versionType });
    return { content: [{ type: "text", text: r.ok ? `Published version ${r.versionNumber}.` : `Error: ${r.error}` }] };
  },
);

// Open Cloud REST tools (opencloud.ts). Each works when the key has the matching
// scope and returns Roblox's error (insufficient-scope 401/403, 404, etc.) otherwise.
const ocText = (r: OcResult) => ({
  content: [{ type: "text" as const, text: r.ok ? JSON.stringify(r.data, undefined, 2) : `Error: ${r.error}` }],
});

// --- Standard DataStores (universe-scoped) ---
server.registerTool(
  "list_datastores",
  {
    description:
      "List the standard data stores in the configured universe (scope universe-datastores.control:list). Optional name prefix and pagination (pass nextPageToken back as pageToken).",
    inputSchema: { prefix: z.string().optional(), maxPageSize: z.number().int().optional(), pageToken: z.string().optional() },
  },
  async (a) => ocText(await listDatastores(a)),
);
server.registerTool(
  "list_datastore_entries",
  {
    description:
      "List entry keys in a data store (scope universe-datastores.objects:list). Keys only; read a value with get_datastore_entry. Optional prefix and pagination.",
    inputSchema: {
      datastore: z.string(),
      prefix: z.string().optional(),
      maxPageSize: z.number().int().optional(),
      pageToken: z.string().optional(),
    },
  },
  async (a) => ocText(await listDatastoreEntries(a)),
);
server.registerTool(
  "get_datastore_entry",
  {
    description: "Read a data store entry's value and metadata (scope universe-datastores.objects:read). 404 means no such key.",
    inputSchema: { datastore: z.string(), entry: z.string() },
  },
  async (a) => ocText(await getDatastoreEntry(a)),
);
server.registerTool(
  "set_datastore_entry",
  {
    description:
      "Create or overwrite a data store entry (upsert; scope universe-datastores.objects:update, plus create for a new key). value is any JSON. users/attributes are cleared if omitted, so pass them every time you set them.",
    inputSchema: {
      datastore: z.string(),
      entry: z.string(),
      value: z.unknown(),
      users: z.array(z.string()).optional(),
      attributes: z.record(z.unknown()).optional(),
    },
  },
  async (a) => ocText(await setDatastoreEntry(a)),
);
server.registerTool(
  "delete_datastore_entry",
  {
    description: "Soft-delete a data store entry (scope universe-datastores.objects:delete); it is purged after 30 days.",
    inputSchema: { datastore: z.string(), entry: z.string() },
  },
  async (a) => ocText(await deleteDatastoreEntry(a)),
);
server.registerTool(
  "increment_datastore_entry",
  {
    description:
      "Atomically add an integer to a numeric data store entry (scope universe-datastores.objects:create+update); creates it if missing. The existing value must be an integer.",
    inputSchema: {
      datastore: z.string(),
      entry: z.string(),
      amount: z.number().int(),
      users: z.array(z.string()).optional(),
      attributes: z.record(z.unknown()).optional(),
    },
  },
  async (a) => ocText(await incrementDatastoreEntry(a)),
);

// --- Ordered DataStores (non-negative integers; default scope "global") ---
server.registerTool(
  "list_ordered_entries",
  {
    description:
      "List ordered data store entries by value (scope universe.ordered-data-store.scope.entry:read). descending sorts high to low. filter is a numeric range like 'entry >= 10 && entry <= 50'.",
    inputSchema: {
      store: z.string(),
      scope: z.string().optional(),
      descending: z.boolean().optional(),
      maxPageSize: z.number().int().optional(),
      pageToken: z.string().optional(),
      filter: z.string().optional(),
    },
  },
  async (a) => ocText(await listOrderedEntries(a)),
);
server.registerTool(
  "get_ordered_entry",
  {
    description: "Read one ordered data store entry's integer value (scope universe.ordered-data-store.scope.entry:read).",
    inputSchema: { store: z.string(), scope: z.string().optional(), entry: z.string() },
  },
  async (a) => ocText(await getOrderedEntry(a)),
);
server.registerTool(
  "set_ordered_entry",
  {
    description:
      "Set (overwrite/upsert) an ordered data store entry to a non-negative integer (scope universe.ordered-data-store.scope.entry:write).",
    inputSchema: { store: z.string(), scope: z.string().optional(), entry: z.string(), value: z.number().int() },
  },
  async (a) => ocText(await setOrderedEntry(a)),
);
server.registerTool(
  "increment_ordered_entry",
  {
    description:
      "Atomically add an integer to an ordered data store entry (scope universe.ordered-data-store.scope.entry:write); the result must stay non-negative.",
    inputSchema: { store: z.string(), scope: z.string().optional(), entry: z.string(), amount: z.number().int() },
  },
  async (a) => ocText(await incrementOrderedEntry(a)),
);

// --- MessagingService ---
server.registerTool(
  "publish_message",
  {
    description:
      "Publish a message to a MessagingService topic in the universe (scope universe-messaging-service:publish). Reaches only running production servers subscribed to the topic; no read side. topic <= 80 chars, message <= 1 KiB.",
    inputSchema: { topic: z.string(), message: z.string() },
  },
  async (a) => ocText(await publishMessage(a)),
);

// --- Memory Stores ---
server.registerTool(
  "memory_sorted_map_set",
  {
    description:
      "Set (upsert) a Memory Store sorted-map item (scope memory-store.sorted-map:write). value is any JSON; ttlSeconds sets expiry; stringSortKey/numericSortKey set the sort order.",
    inputSchema: {
      map: z.string(),
      item: z.string(),
      value: z.unknown(),
      ttlSeconds: z.number().int().optional(),
      stringSortKey: z.string().optional(),
      numericSortKey: z.number().optional(),
    },
  },
  async (a) => ocText(await memorySortedMapSet(a)),
);
server.registerTool(
  "memory_sorted_map_get",
  {
    description: "Read a Memory Store sorted-map item (scope memory-store.sorted-map:read). 404 if absent or expired.",
    inputSchema: { map: z.string(), item: z.string() },
  },
  async (a) => ocText(await memorySortedMapGet(a)),
);
server.registerTool(
  "memory_sorted_map_list",
  {
    description:
      "List Memory Store sorted-map items in sort order (scope memory-store.sorted-map:read). descending reverses; filter is a CEL range over id/sortKey. Paginated.",
    inputSchema: {
      map: z.string(),
      descending: z.boolean().optional(),
      maxPageSize: z.number().int().optional(),
      pageToken: z.string().optional(),
      filter: z.string().optional(),
    },
  },
  async (a) => ocText(await memorySortedMapList(a)),
);
server.registerTool(
  "memory_sorted_map_delete",
  {
    description: "Delete a Memory Store sorted-map item (scope memory-store.sorted-map:write).",
    inputSchema: { map: z.string(), item: z.string() },
  },
  async (a) => ocText(await memorySortedMapDelete(a)),
);
server.registerTool(
  "memory_queue_add",
  {
    description:
      "Add an item to a Memory Store queue (scope memory-store.queue:add). data is any JSON (required); higher priority dequeues first; ttlSeconds sets expiry.",
    inputSchema: { queue: z.string(), data: z.unknown(), priority: z.number().optional(), ttlSeconds: z.number().int().optional() },
  },
  async (a) => ocText(await memoryQueueAdd(a)),
);
server.registerTool(
  "memory_queue_read",
  {
    description:
      "Read items from a Memory Store queue (scope memory-store.queue:dequeue); returns a readId. Pass that readId to memory_queue_discard before the invisibility window (default 30s) elapses, or the items reappear.",
    inputSchema: {
      queue: z.string(),
      count: z.number().int().optional(),
      invisibilitySeconds: z.number().int().optional(),
      allOrNothing: z.boolean().optional(),
    },
  },
  async (a) => ocText(await memoryQueueRead(a)),
);
server.registerTool(
  "memory_queue_discard",
  {
    description:
      "Permanently remove the items from a memory_queue_read batch (scope memory-store.queue:discard), using its readId.",
    inputSchema: { queue: z.string(), readId: z.string() },
  },
  async (a) => ocText(await memoryQueueDiscard(a)),
);

// --- Platform info (read-only) ---
server.registerTool(
  "get_universe",
  { description: "Get the configured universe's metadata (name, visibility, owner, root place, etc.).", inputSchema: {} },
  async () => ocText(await getUniverse()),
);
server.registerTool(
  "get_place",
  { description: "Get the configured place's metadata (name, server size, etc.).", inputSchema: {} },
  async () => ocText(await getPlace()),
);
server.registerTool(
  "get_user",
  {
    description:
      "Get a user's public profile (name, displayName, etc.). idVerified and social profiles need the user.advanced:read / user.social:read scopes.",
    inputSchema: { userId: z.string() },
  },
  async (a) => ocText(await getUser(a)),
);
server.registerTool(
  "get_group",
  { description: "Get a group's metadata (name, owner, member count, etc.).", inputSchema: { groupId: z.string() } },
  async (a) => ocText(await getGroup(a)),
);
server.registerTool(
  "list_inventory",
  {
    description:
      "List a user's inventory items (scope user.inventory-item:read; also gated by the user's inventory privacy). filter selects types, e.g. 'inventoryItemAssetTypes=HAT,TSHIRT_ACCESSORY' or 'assetIds=1,2,3'. Paginated.",
    inputSchema: { userId: z.string(), filter: z.string().optional(), maxPageSize: z.number().int().optional(), pageToken: z.string().optional() },
  },
  async (a) => ocText(await listInventory(a)),
);

// --- Engagement ---
server.registerTool(
  "send_notification",
  {
    description:
      "Send an experience notification to a user (scope user.user-notification:write). messageId is a notification-string template authored in the Creator Dashboard, not free text. parameters fills the template's placeholders. One notification per user per day per experience.",
    inputSchema: {
      userId: z.string(),
      messageId: z.string(),
      parameters: z
        .record(z.object({ stringValue: z.string().optional(), int64Value: z.number().int().optional() }))
        .optional(),
      launchData: z.string().optional(),
    },
  },
  async (a) => ocText(await sendNotification(a)),
);
server.registerTool(
  "get_subscription",
  {
    description:
      "Read a user's subscription to a subscription product (scope universe.subscription-product.subscription:read for the caller, or universe:write to read any user). userId is the subscriber. full returns state and timestamps, not just active/willRenew.",
    inputSchema: { subscriptionProductId: z.string(), userId: z.string(), full: z.boolean().optional() },
  },
  async (a) => ocText(await getSubscription(a)),
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
