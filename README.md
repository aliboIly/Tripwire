<div align="center">

# Tripwire

**made by alibolly**

[![npm](https://img.shields.io/badge/npm-coming_soon-cb3837?logo=npm&logoColor=white)](https://github.com/aliboIly/Tripwire/releases)
[![CI](https://github.com/aliboIly/Tripwire/actions/workflows/ci.yml/badge.svg)](https://github.com/aliboIly/Tripwire/actions/workflows/ci.yml)

An MCP server that gives an AI coding agent real control of Roblox Studio and Roblox Open Cloud,
with a test-and-security layer no other Studio MCP has.

</div>

Tripwire lets an assistant read, write, and edit the data model, drive playtests with simulated
input, run tests-as-code headlessly in the real engine, flag client-trust exploits in game code,
and call the Open Cloud APIs (DataStores, MessagingService, Memory Stores, and more). The Studio
tools need no API key; the headless test, asset, and Open Cloud tools use an Open Cloud key.

---

## Install

Tripwire is a Node stdio MCP server. Build it once, then point your client at it.

```bash
git clone https://github.com/aliboIly/Tripwire.git
cd Tripwire/server
npm install
npm run build        # produces server/dist/index.js
```

Replace `/ABS/PATH` below with the absolute path to your clone.

<details>
<summary><b>Claude Code</b></summary>

One command:

```bash
claude mcp add --transport stdio tripwire -- node /ABS/PATH/Tripwire/server/dist/index.js
```

Or add it to a project `.mcp.json` (or `~/.claude.json`):

```json
{
  "mcpServers": {
    "tripwire": {
      "type": "stdio",
      "command": "node",
      "args": ["/ABS/PATH/Tripwire/server/dist/index.js"]
    }
  }
}
```

</details>

<details>
<summary><b>Codex</b></summary>

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.tripwire]
command = "node"
args = ["/ABS/PATH/Tripwire/server/dist/index.js"]
```

Or: `codex mcp add tripwire -- node /ABS/PATH/Tripwire/server/dist/index.js`

</details>

<details>
<summary><b>Gemini</b></summary>

Add to `~/.gemini/settings.json` (or a project `.gemini/settings.json`):

```json
{
  "mcpServers": {
    "tripwire": {
      "command": "node",
      "args": ["/ABS/PATH/Tripwire/server/dist/index.js"]
    }
  }
}
```

Or: `gemini mcp add tripwire node /ABS/PATH/Tripwire/server/dist/index.js`

</details>

<details>
<summary><b>Other MCP clients</b></summary>

Any client that speaks MCP over stdio can run it. Launch:

```
command: node
args:    ["/ABS/PATH/Tripwire/server/dist/index.js"]
```

</details>

<details>
<summary><b>Studio plugin (required for the Studio tools)</b></summary>

The Studio tools reach Studio through a small plugin that long-polls the local server. Grab
`Tripwire.rbxmx` from the [Releases](https://github.com/aliboIly/Tripwire/releases) page, or build it:

```bash
cd Tripwire/plugin
npm install
npx rbxtsc
rojo build --output Tripwire.rbxmx
cp Tripwire.rbxmx ~/Documents/Roblox/Plugins/   # macOS; Windows: %LOCALAPPDATA%\Roblox\Plugins
```

Restart Studio, click the **Tripwire** toolbar button, and enable **Game Settings > Security >
Allow HTTP Requests**. The Output shows `[Tripwire v...] connected`.

</details>

<details>
<summary><b>Open Cloud key (for headless tests, assets, and Open Cloud tools)</b></summary>

The Studio tools need no key. The Open Cloud tools do. See [Open Cloud setup](#open-cloud-setup)
below for the full walkthrough.

</details>

---

## Open Cloud setup

Most of Tripwire needs no credentials. These tools do, because they call Roblox Open Cloud:
`run_luau`, the headless tests (`run_tests`, `run_test_file`, `list_tests`), `upload_asset`,
`publish_place`, and the DataStore, Ordered DataStore, MessagingService, Memory Store, platform,
and engagement tools. They authenticate with a Roblox Open Cloud API key.

> **Use at your own risk.** An Open Cloud key is a real credential with real power over your
> experience. Depending on the scopes you grant it, it can read and overwrite your live DataStores,
> publish new versions of your place, upload assets to your account, and message your servers. Treat
> it like a password: grant only the scopes you actually use, restrict it to your own IP, never
> commit it, and revoke it if it leaks. You are responsible for what you do with it. Tripwire is not
> affiliated with or endorsed by Roblox.

### 1. Create the key

1. Go to [create.roblox.com/dashboard/credentials](https://create.roblox.com/dashboard/credentials) and sign in.
2. Click **Create API Key** and name it (for example `Tripwire`).
3. Under **Access Permissions**, add only the API systems for the tools you want, and grant each the operation it needs, scoped to your experience:
   - **Luau Execution** (write): `run_luau` and the headless tests.
   - **universe-places** (write): `publish_place`.
   - **Assets** (read + write): `upload_asset`.
   - **DataStores** and **Ordered DataStores**: the data-store tools.
   - **Messaging Service** (publish) and **Memory Stores**: those tools.
   - **User/Group/Inventory/Subscription/Notification**: the platform and engagement tools.
4. Under **Security**, set **Accepted IP Addresses** to your machine's IP, or `0.0.0.0/0` to allow any (simplest for local use). Set an expiration if you want.
5. Click **Save & Generate Key** and copy the key string. It is shown only once.

### 2. Find your universe and place IDs

In the Studio command bar (View, then Command Bar), run:

```lua
print("universe", game.GameId, "place", game.PlaceId)
```

`GameId` is your `ROBLOX_UNIVERSE_ID`; `PlaceId` is your `ROBLOX_PLACE_ID`. The place must be
**published** to Roblox for Open Cloud to act on it.

### 3. Give Tripwire the credentials

Create a `.env` at the repo root. It is gitignored and the server loads it automatically:

```bash
ROBLOX_OPEN_CLOUD_KEY=paste_the_key_here
ROBLOX_UNIVERSE_ID=000000
ROBLOX_PLACE_ID=000000
ROBLOX_CREATOR_USER_ID=000000   # only for upload_asset (your user id)
```

Or put the same variables in your MCP client's `env` block instead (those take precedence).
Reconnect the MCP server after changing either. Each tool works when the key grants its scope and
returns Roblox's own error if a scope is missing, so you can add scopes as you go.

---

## Tools

### Connection
<details><summary><code>studio_status</code></summary>Whether a Studio is connected, the active place, and any other connected studios.</details>
<details><summary><code>ping_studio</code></summary>Round-trip a ping through the plugin to confirm the live bridge works.</details>
<details><summary><code>list_studios</code></summary>List every connected (or recently seen) Studio: place, whether it is active, last-seen, and playtest state.</details>
<details><summary><code>set_active_studio</code></summary>Choose which connected Studio the tools target (by id, id prefix, or place name). Automatic with one Studio.</details>

### Read and inspect
<details><summary><code>get_file_tree</code></summary>List the instance tree from a path (default the whole game), bounded by depth.</details>
<details><summary><code>get_instance_children</code></summary>List the direct children (name and class) of an instance.</details>
<details><summary><code>get_instance_properties</code></summary>Read an instance's name, class, full path, and attributes.</details>
<details><summary><code>search_objects</code></summary>Find instances whose name contains a query, optionally filtered by class.</details>
<details><summary><code>search_by_property</code></summary>Find instances whose property equals a value, optionally filtered by class.</details>
<details><summary><code>get_script_source</code></summary>Read the source of a Script, LocalScript, or ModuleScript.</details>
<details><summary><code>grep_scripts</code></summary>Search script sources for a substring; returns path, line number, and line.</details>
<details><summary><code>get_output_log</code></summary>Recent Studio Output entries (message, type, timestamp).</details>
<details><summary><code>get_selection</code></summary>The instances currently selected in Studio.</details>

### Edit (each is one undo step)
<details><summary><code>create_instance</code></summary>Create an instance of a class with an optional name and initial properties.</details>
<details><summary><code>delete_instance</code></summary>Destroy an instance and its descendants.</details>
<details><summary><code>set_property</code></summary>Set one typed property (primitive, Vector3, Color3, UDim2, CFrame, EnumItem, or an instance reference).</details>
<details><summary><code>update_script_source</code></summary>Replace a script's source through the script editor (the supported write path).</details>
<details><summary><code>insert_model</code></summary>Insert an asset by id, with optional reposition or unpack.</details>
<details><summary><code>mass_create</code></summary>Create many instances in one undo step (atomic, or best-effort with per-item results).</details>
<details><summary><code>mass_set_property</code></summary>Set one property on many instances in one undo step (atomic or best-effort).</details>

### Playtest and input
<details><summary><code>start_playtest</code></summary>Start an F5 playtest (server and client DataModels with a player); injects the in-play runner.</details>
<details><summary><code>stop_playtest</code></summary>Stop the F5 playtest (best-effort; F5 teardown can outlast the confirmation).</details>
<details><summary><code>start_simulation</code></summary>Start an F8 run (server-only simulation, no client or player).</details>
<details><summary><code>stop_simulation</code></summary>Stop the F8 run (clean).</details>
<details><summary><code>simulate_mouse_input</code></summary>Click or move the mouse at screen coordinates during an F5 playtest.</details>
<details><summary><code>simulate_keyboard_input</code></summary>Press a key (tap/press/release) or type text during an F5 playtest.</details>
<details><summary><code>character_navigation</code></summary>Walk the local character toward a world position; reports whether it reached the goal.</details>
<details><summary><code>get_playtest_output</code></summary>The running playtest's output log, aggregated across the server and client peers.</details>

### Tests and headless execution (Open Cloud)
<details><summary><code>run_luau</code></summary>Run a Luau script headlessly in the published place; returns the results and logs.</details>
<details><summary><code>run_tests</code></summary>Run the headless test suite in the published place and report passed/failed with messages.</details>
<details><summary><code>run_test_file</code></summary>Run a single spec by name, headlessly.</details>
<details><summary><code>list_tests</code></summary>List the spec files and their cases discovered in the published place.</details>
<details><summary><code>write_test</code></summary>Write a roblox-ts test spec to disk; rebuild and publish, then run_tests picks it up.</details>

### Security review (static analysis, no key)
<details><summary><code>review_security</code></summary>Review the game source for client-trust and unvalidated-remote issues, each with a suggested server-side fix.</details>
<details><summary><code>scan_remotes</code></summary>List server remote handlers and the client-controlled parameters of each.</details>
<details><summary><code>scan_client_trust</code></summary>Flag server handlers that use client-supplied values without validating them.</details>

### Assets and publishing (Open Cloud)
<details><summary><code>upload_asset</code></summary>Upload a local file as a Roblox asset (Decal, Audio, Model, Animation, or Video); returns the assetId.</details>
<details><summary><code>publish_place</code></summary>Publish a local place file (.rbxl/.rbxlx) as a new version of the experience.</details>

### DataStores (Open Cloud)
<details><summary><code>list_datastores</code></summary>List the standard data stores in the universe.</details>
<details><summary><code>list_datastore_entries</code></summary>List entry keys in a data store.</details>
<details><summary><code>get_datastore_entry</code></summary>Read an entry's value and metadata.</details>
<details><summary><code>set_datastore_entry</code></summary>Create or overwrite an entry (value plus optional users/attributes).</details>
<details><summary><code>delete_datastore_entry</code></summary>Soft-delete an entry (purged after 30 days).</details>
<details><summary><code>increment_datastore_entry</code></summary>Atomically add an integer to a numeric entry.</details>
<details><summary><code>list_ordered_entries</code></summary>List ordered data store entries by value, ascending or descending.</details>
<details><summary><code>get_ordered_entry</code></summary>Read one ordered data store entry.</details>
<details><summary><code>set_ordered_entry</code></summary>Set (upsert) an ordered data store entry to a non-negative integer.</details>
<details><summary><code>increment_ordered_entry</code></summary>Atomically add to an ordered data store entry.</details>

### Messaging and memory (Open Cloud)
<details><summary><code>publish_message</code></summary>Publish a message to a MessagingService topic (reaches running production servers).</details>
<details><summary><code>memory_sorted_map_set</code></summary>Set (upsert) a Memory Store sorted-map item, with TTL and sort keys.</details>
<details><summary><code>memory_sorted_map_get</code></summary>Read a Memory Store sorted-map item.</details>
<details><summary><code>memory_sorted_map_list</code></summary>List sorted-map items in sort order.</details>
<details><summary><code>memory_sorted_map_delete</code></summary>Delete a sorted-map item.</details>
<details><summary><code>memory_queue_add</code></summary>Add an item to a Memory Store queue, with priority and TTL.</details>
<details><summary><code>memory_queue_read</code></summary>Read items from a queue; returns a readId for the discard call.</details>
<details><summary><code>memory_queue_discard</code></summary>Permanently remove a read batch by its readId.</details>

### Platform and engagement (Open Cloud)
<details><summary><code>get_universe</code></summary>The configured universe's metadata.</details>
<details><summary><code>get_place</code></summary>The configured place's metadata.</details>
<details><summary><code>get_user</code></summary>A user's public profile.</details>
<details><summary><code>get_group</code></summary>A group's metadata.</details>
<details><summary><code>list_inventory</code></summary>A user's inventory items, filterable by type or id.</details>
<details><summary><code>send_notification</code></summary>Send an experience notification to a user (from a Creator Dashboard template).</details>
<details><summary><code>get_subscription</code></summary>Read a user's subscription to a subscription product.</details>

---

## What you can do

- **Build scenes from a prompt.** Create and mass-create instances, set typed properties, insert
  models, and write scripts, each as a clean undo step.
- **Test gameplay in CI.** Write specs, run them headlessly in the real engine through Open Cloud,
  and gate pull requests on the results.
- **Catch exploits before they ship.** The security reviewer flags server handlers that trust client
  input and suggests the server-side fix; the same check runs automatically on every PR.
- **Drive a real playtest.** Enter Play mode, send keyboard and mouse input, walk the character to a
  spot, read the combined server/client output, then stop.
- **Inspect a live place.** Read the instance tree, search by name or property, read and grep
  scripts, and pull the Output log.
- **Automate Open Cloud.** Seed DataStores for test fixtures, publish a place, broadcast a
  MessagingService topic, or look up users, groups, and inventory.

---

## Prior art

The Studio runtime approach (a plugin that long-polls a local server, an injected in-play runner)
follows ideas from [boshyxd/robloxstudio-mcp](https://github.com/boshyxd/robloxstudio-mcp) and
[Chrrxs/robloxstudio-mcp](https://github.com/Chrrxs/robloxstudio-mcp). Tripwire is an independent,
from-scratch implementation; the headless test harness, the CI security reviewer, and the Open Cloud
tooling are its own.

---

## License

MIT. See [LICENSE](LICENSE).
