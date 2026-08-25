<div align="center">

# Tripwire

**made by alibolly**

[![npm](https://img.shields.io/npm/v/tripwire-roblox?logo=npm&logoColor=white)](https://www.npmjs.com/package/tripwire-roblox)
[![CI](https://github.com/aliboIly/Tripwire/actions/workflows/ci.yml/badge.svg)](https://github.com/aliboIly/Tripwire/actions/workflows/ci.yml)
[![MCP Registry](https://img.shields.io/badge/MCP_Registry-io.github.aliboIly%2Ftripwire-0b7285)](https://registry.modelcontextprotocol.io)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

An MCP server that gives an AI coding agent real control of Roblox Studio and Roblox Open Cloud,
with a test-and-security layer no other Studio MCP has.

</div>

Tripwire lets an assistant read, write, and edit the data model, drive playtests with simulated
input, run tests-as-code headlessly in the real engine, flag client-trust exploits in game code,
and call the Open Cloud APIs (DataStores, MessagingService, Memory Stores, and more). The Studio
tools need no API key; the headless test, asset, and Open Cloud tools use an Open Cloud key.

---

## Requirements

- An MCP client: Claude Code, Codex, Gemini, or any client that speaks MCP over stdio.
- Roblox Studio, for the Studio tools. These need no API key.
- Node.js, only if you run the server with `npx`. The prebuilt binary needs no Node.
- For the headless test, asset, and Open Cloud tools: a published place and a Roblox Open Cloud API key. See [Open Cloud setup](#open-cloud-setup).

## Quickstart

Tripwire is for Roblox developers who drive Studio through an AI coding agent.

1. Wire the server into your MCP client (one command or a small config block, see [Install](#install) below).
2. Install the Studio plugin so the Studio tools can reach Studio (see the plugin step in Install).
3. Open your place in Studio, turn on Game Settings > Security > Allow HTTP Requests, then click the Tripwire toolbar button and press Connect in the panel. The panel shows Connected and the Output prints `[Tripwire v...] connected`.
4. Ask your agent to run `studio_status`. A connected Studio confirms the bridge works.
5. Optional: add an Open Cloud key for the headless test, asset, and Open Cloud tools (see Open Cloud setup).

---

## Install

Tripwire's server is a single binary. The easiest way to run it is with `npx`, which fetches the
prebuilt binary for your platform, so there is no Rust toolchain to install. Prefer a manual binary
or a source build? See the Alternatives at the end of this section.

<details>
<summary><b>Claude Code</b></summary>

One command:

```bash
claude mcp add --transport stdio tripwire -- npx -y tripwire-roblox
```

Or add it to a project `.mcp.json` (or `~/.claude.json`):

```json
{
  "mcpServers": {
    "tripwire": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "tripwire-roblox"]
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
command = "npx"
args = ["-y", "tripwire-roblox"]
```

Or: `codex mcp add tripwire -- npx -y tripwire-roblox`

</details>

<details>
<summary><b>Gemini</b></summary>

Add to `~/.gemini/settings.json` (or a project `.gemini/settings.json`):

```json
{
  "mcpServers": {
    "tripwire": {
      "command": "npx",
      "args": ["-y", "tripwire-roblox"]
    }
  }
}
```

Or: `gemini mcp add tripwire npx -y tripwire-roblox`

</details>

<details>
<summary><b>Other MCP clients</b></summary>

Any client that speaks MCP over stdio can run it:

```
command: npx
args:    ["-y", "tripwire-roblox"]
```

</details>

<details>
<summary><b>Alternatives: prebuilt binary, or build from source</b></summary>

**Prebuilt binary (no Node).** Download the archive for your platform from the
[Releases](https://github.com/aliboIly/Tripwire/releases) page (for example
`tripwire-server-vX.Y.Z-aarch64-apple-darwin.tar.gz`), extract it, and point your client's
`command` at the extracted `tripwire-server` with empty `args`.

**Build from source** (needs [Rust](https://rustup.rs)):

```bash
git clone https://github.com/aliboIly/Tripwire.git
cd Tripwire/server
cargo build --release   # produces server/target/release/tripwire-server
```

Then point your client's `command` at that binary path.

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

Restart Studio, enable **Game Settings > Security > Allow HTTP Requests**, then click the
**Tripwire** toolbar button and press **Connect** in the panel. The panel shows Connected and
the Output prints `[Tripwire v...] connected`.

</details>

<details>
<summary><b>Open Cloud key (for headless tests, assets, and Open Cloud tools)</b></summary>

The Studio tools need no key. The Open Cloud tools do. See [Open Cloud setup](#open-cloud-setup)
below for the full walkthrough.

</details>

---

## Open Cloud setup

Most of Tripwire needs no credentials. These tools do, because they call Roblox Open Cloud:
`run_luau`, the headless tests (`run_tests`, `run_test_file`, `list_tests`), and every tool in the
Open Cloud sections below (assets, publishing, data stores, messaging, memory, universe and
servers, the Engine Instance API, moderation, secrets, monetization, Creator Store, groups,
analytics, platform, and `open_cloud_request`). They authenticate with a Roblox Open Cloud API key.

> **Use at your own risk.** An Open Cloud key is a real credential with real power over your
> experience. Depending on the scopes you grant it, it can read and overwrite your live DataStores,
> publish new versions of your place, upload assets to your account, and message your servers. Treat
> it like a password: grant only the scopes you actually use, restrict it to your own IP, never
> commit it, and revoke it if it leaks. You are responsible for what you do with it. Tripwire is not
> affiliated with or endorsed by Roblox.

### 1. Create the key

1. Go to [create.roblox.com/dashboard/credentials](https://create.roblox.com/dashboard/credentials) and sign in.
2. Click **Create API Key** and name it (for example `Tripwire`).
3. Under **Access Permissions**, add only the API systems for the tools you want, and grant each the operation it needs, scoped to your experience. Every system the dashboard offers has a tool; the map is in [Permissions to tools](#permissions-to-tools) below. The short version:
   - **Luau Execution** (write): `run_luau` and the headless tests.
   - **universe-places** (write): `publish_place`.
   - **Assets** (read + write) and **Asset Permissions**: the asset tools.
   - **DataStores** and **Ordered DataStores**: the data-store tools.
   - **Messaging Service** (publish) and **Memory Stores**: those tools.
   - **Universe**, **Place**, **Instance**, **User Restrictions**, **Secrets**, **Analytics**: the universe, server, instance, moderation, secrets, and analytics tools.
   - **Developer Products**, **Game Passes**, **Creator Store**: the monetization and store tools.
   - **User/Group/Inventory/Subscription/Notification**: the platform, group, and engagement tools.
   - Anything else (the legacy and experimental systems): `open_cloud_request`.
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
<details><summary><code>get_instance_properties</code></summary>Read an instance's name, class, full path, attributes, and a curated set of common engine properties (Position, Size, Color, Material, Anchored, Text, and so on).</details>
<details><summary><code>search_objects</code></summary>Find instances whose name contains a query, optionally filtered by exact class or by class-and-subclasses (isA).</details>
<details><summary><code>search_by_property</code></summary>Find instances whose property equals a value, optionally filtered by exact class or by class-and-subclasses (isA).</details>
<details><summary><code>get_script_source</code></summary>Read the source of a Script, LocalScript, or ModuleScript.</details>
<details><summary><code>grep_scripts</code></summary>Search script sources for a substring; returns path, line number, and line.</details>
<details><summary><code>get_output_log</code></summary>Recent Studio Output entries (message, type, timestamp).</details>
<details><summary><code>get_selection</code></summary>The instances currently selected in Studio.</details>
<details><summary><code>get_class_info</code></summary>Look up a Roblox class's members (properties, methods, events) with their types, inherited members folded in. Answered from a bundled API reflection dump, so it needs no Studio and no key.</details>

### Spatial (read-only)
<details><summary><code>raycast</code></summary>Cast a ray and report the first hit (instance, position, normal, material, distance) or no hit, with an optional excluded subtree.</details>
<details><summary><code>get_bounding_box</code></summary>The world-space bounding box (center and size) of a Model or BasePart.</details>
<details><summary><code>find_spawns</code></summary>List the SpawnLocations under a path: position, whether each is enabled, and whether it is neutral.</details>
<details><summary><code>capture_screenshot</code></summary>Capture the Studio viewport as a JPEG image so the agent can see the scene. Needs the plugin connected and Allow Mesh / Image APIs enabled (Game Settings, Security). Edit mode only.</details>

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
<details><summary><code>get_playtest_output</code></summary>The running playtest's output log, aggregated across the server and client peers. Pass the <code>cursor</code> from the previous call as <code>since</code> to get only new lines; the buffer is bounded so long sessions do not freeze it.</details>
<details><summary><code>reset_playtest_output</code></summary>Clear the playtest's output buffers so the next read starts fresh, without restarting Studio.</details>
<details><summary><code>run_luau_live</code></summary>Evaluate Luau in the live F5 playtest server and return the result, so you can inspect the running game (a Humanoid's state, an NPC's position, a path's waypoints) without adding a print and replaying. Needs an active playtest and ServerScriptService.LoadStringEnabled on in the test place. Distinct from run_luau, which is headless against the published place.</details>

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
<details><summary><code>get_asset</code></summary>An asset's metadata: type, name, description, moderation state, current revision.</details>
<details><summary><code>update_asset</code></summary>Change an asset's name or description, or (Models) upload new content as a new version.</details>
<details><summary><code>list_asset_versions</code></summary>An asset's versions with their moderation state.</details>
<details><summary><code>rollback_asset_version</code></summary>Roll an asset back to an earlier version.</details>
<details><summary><code>archive_asset</code> / <code>restore_asset</code></summary>Hide an asset from the site and experiences, or bring it back.</details>
<details><summary><code>list_asset_quotas</code></summary>Your upload quotas and how much of each is used.</details>
<details><summary><code>download_asset</code></summary>Download an asset's content (a model, image, or place file) to a local path.</details>
<details><summary><code>grant_asset_permissions</code></summary>Grant a user, group, roleset, universe, or everyone a permission (Use, Edit, Download) on assets you own.</details>
<details><summary><code>publish_place</code></summary>Publish a local place file (.rbxl/.rbxlx) as a new version of the experience.</details>

### DataStores (Open Cloud)
<details><summary><code>list_datastores</code></summary>List the standard data stores in the universe.</details>
<details><summary><code>list_datastore_entries</code></summary>List entry keys in a data store. Optional <code>scope</code>; <code>-</code> lists every scope.</details>
<details><summary><code>get_datastore_entry</code></summary>Read an entry's value and metadata, or an older <code>revision</code> of it.</details>
<details><summary><code>set_datastore_entry</code></summary>Create or overwrite an entry (value plus optional users/attributes).</details>
<details><summary><code>delete_datastore_entry</code></summary>Soft-delete an entry (purged after 30 days).</details>
<details><summary><code>increment_datastore_entry</code></summary>Atomically add an integer to a numeric entry.</details>
<details><summary><code>list_datastore_entry_revisions</code></summary>An entry's revision history, newest first.</details>
<details><summary><code>delete_datastore</code> / <code>undelete_datastore</code></summary>Schedule a whole data store for deletion in 30 days, or cancel that.</details>
<details><summary><code>snapshot_datastores</code></summary>Snapshot every data store so the next write to each key keeps a versioned backup. Run it before a migration.</details>
<details><summary><code>list_ordered_entries</code></summary>List ordered data store entries by value, ascending or descending.</details>
<details><summary><code>get_ordered_entry</code></summary>Read one ordered data store entry.</details>
<details><summary><code>set_ordered_entry</code></summary>Set (upsert) an ordered data store entry to a non-negative integer.</details>
<details><summary><code>increment_ordered_entry</code></summary>Atomically add to an ordered data store entry.</details>
<details><summary><code>delete_ordered_entry</code></summary>Delete an ordered data store entry.</details>

### Messaging and memory (Open Cloud)
<details><summary><code>publish_message</code></summary>Publish a message to a MessagingService topic (reaches running production servers).</details>
<details><summary><code>memory_sorted_map_set</code></summary>Set (upsert) a Memory Store sorted-map item, with TTL and sort keys.</details>
<details><summary><code>memory_sorted_map_get</code></summary>Read a Memory Store sorted-map item.</details>
<details><summary><code>memory_sorted_map_list</code></summary>List sorted-map items in sort order.</details>
<details><summary><code>memory_sorted_map_delete</code></summary>Delete a sorted-map item.</details>
<details><summary><code>memory_queue_add</code></summary>Add an item to a Memory Store queue, with priority and TTL.</details>
<details><summary><code>memory_queue_read</code></summary>Read items from a queue; returns a readId for the discard call.</details>
<details><summary><code>memory_queue_discard</code></summary>Permanently remove a read batch by its readId.</details>
<details><summary><code>flush_memory_store</code></summary>Wipe every Memory Store structure in the universe (LIVE or TEST scope) and wait for it.</details>

### Universe, place, and servers (Open Cloud)
<details><summary><code>get_universe</code></summary>The configured universe's metadata.</details>
<details><summary><code>update_universe</code></summary>Voice chat, private server price, and per-platform join toggles.</details>
<details><summary><code>get_place</code></summary>The configured place's metadata.</details>
<details><summary><code>update_place</code></summary>The place's name, description, or server size.</details>
<details><summary><code>restart_servers</code></summary>Move live servers to the newest published version, outdated ones only by default, with an optional bleed-off.</details>
<details><summary><code>list_game_servers</code></summary>The live servers running a place version: players, uptime, job ids.</details>
<details><summary><code>get_game_server_logs</code></summary>A live server's log lines by job id. The production counterpart of <code>get_playtest_output</code>.</details>
<details><summary><code>translate_text</code></summary>Translate text into one or more languages with Roblox's translation service.</details>

### Engine Instance API (Open Cloud, no Studio needed)
<details><summary><code>cloud_list_instance_children</code></summary>List an instance's children in the published place. Start from <code>root</code>.</details>
<details><summary><code>cloud_get_instance</code></summary>Read an instance's details in the published place.</details>
<details><summary><code>cloud_update_instance</code></summary>Rename an instance or set a script's Source, Enabled, or RunContext in the published place.</details>

### Moderation (Open Cloud)
<details><summary><code>list_user_restrictions</code></summary>Users who have ever been banned from the universe or a place.</details>
<details><summary><code>get_user_restriction</code></summary>One user's ban state and reasons.</details>
<details><summary><code>set_user_restriction</code></summary>Ban (permanent or timed, with reasons, optionally not extended to alts) or unban a user. Kicks them from live servers.</details>
<details><summary><code>list_user_restriction_logs</code></summary>The audit log of ban and unban changes.</details>

### Secrets (Open Cloud)
<details><summary><code>list_secrets</code></summary>The universe's secrets, metadata only.</details>
<details><summary><code>create_secret</code> / <code>update_secret</code></summary>Store a value for <code>HttpService:GetSecret</code>. It is sealed with the universe's public key on your machine before upload.</details>
<details><summary><code>delete_secret</code></summary>Permanently delete a secret.</details>

### Monetization and Creator Store (Open Cloud)
<details><summary><code>list_developer_products</code> / <code>get_developer_product</code></summary>Developer products with prices and sale state.</details>
<details><summary><code>create_developer_product</code> / <code>update_developer_product</code></summary>Create or edit a developer product: name, description, Robux price, sale state, icon.</details>
<details><summary><code>list_game_passes</code> / <code>get_game_pass</code></summary>Game passes with prices and sale state.</details>
<details><summary><code>create_game_pass</code> / <code>update_game_pass</code></summary>Create or edit a game pass.</details>
<details><summary><code>search_creator_store</code></summary>Search the Creator Store for models, plugins, audio, decals, meshes, video, or fonts; returns asset ids for <code>insert_model</code>.</details>
<details><summary><code>get_creator_store_asset</code></summary>A store listing's creator, votes, price, and asset details.</details>
<details><summary><code>get_creator_store_product</code></summary>One of your own store products: prices, published state, restrictions.</details>

### Groups (Open Cloud)
<details><summary><code>get_group</code></summary>A group's metadata.</details>
<details><summary><code>list_group_memberships</code></summary>A group's members and their roles, filterable by user or role.</details>
<details><summary><code>list_group_roles</code></summary>A group's roles and ranks.</details>
<details><summary><code>list_group_join_requests</code> / <code>resolve_group_join_request</code></summary>Pending join requests, and accept or decline one.</details>
<details><summary><code>set_group_role</code></summary>Assign a role to a member, or remove one.</details>

### Platform, engagement, and analytics (Open Cloud)
<details><summary><code>get_user</code></summary>A user's public profile.</details>
<details><summary><code>generate_user_thumbnail</code></summary>A user's avatar headshot as an image URL.</details>
<details><summary><code>list_inventory</code></summary>A user's inventory items, filterable by type or id.</details>
<details><summary><code>send_notification</code></summary>Send an experience notification to a user (from a Creator Dashboard template).</details>
<details><summary><code>get_subscription</code></summary>Read a user's subscription to a subscription product.</details>
<details><summary><code>query_analytics</code></summary>A metric (DAU, sessions, revenue, and so on) over a time range, broken down and filtered by dimension; or the values a dimension can take.</details>

### Everything else (Open Cloud)
<details><summary><code>open_cloud_request</code></summary>Call any endpoint under <code>apis.roblox.com</code> with the key attached: method, host-relative path, query, JSON body. This is how the legacy and experimental systems are reached (badges, localization tables, Team Create, game events, thumbnails, ads, place version history, creator store products). The key is only ever sent to that host.</details>

### Permissions to tools

Every API system you can add to a key on the Creator Dashboard, and the tools that use it. Scope
names are the ones the dashboard and Roblox's error messages use.

| Permission (scope) | Tools |
| --- | --- |
| Luau Execution (`universe.place.luau-execution-session`) | `run_luau`, `run_tests`, `run_test_file`, `list_tests` |
| Places (`universe-places:write`, `universe.place`) | `publish_place`, `update_place`, `get_place` |
| Universe (`universe:read`, `universe:write`) | `update_universe`, `restart_servers`, `translate_text`, `list_game_servers`, `get_game_server_logs` |
| Instance (`universe.place.instance`) | `cloud_list_instance_children`, `cloud_get_instance`, `cloud_update_instance` |
| Data Stores (`universe-datastores.control`, `.objects`, `.versions`) | the DataStores section: entries, revisions, store delete, snapshot |
| Ordered Data Stores (`universe.ordered-data-store.scope.entry`) | `list/get/set/increment/delete_ordered_entry` |
| Memory Stores (`memory-store.sorted-map`, `.queue`, `:flush`) | the sorted-map and queue tools, `flush_memory_store` |
| Messaging Service (`universe-messaging-service:publish`) | `publish_message` |
| User Restrictions (`universe.user-restriction`) | `list/get/set_user_restriction`, `list_user_restriction_logs` |
| Secrets (`universe.secret`) | `list/create/update/delete_secret` |
| Assets (`asset:read`, `asset:write`) | `upload_asset`, `get_asset`, `update_asset`, `list_asset_versions`, `rollback_asset_version`, `archive_asset`, `restore_asset`, `list_asset_quotas` |
| Asset Permissions (`asset-permissions:write`) | `grant_asset_permissions` |
| Legacy Assets (`legacy-asset:manage`) | `download_asset` |
| Developer Products (`developer-product`) | `list/get/create/update_developer_product` |
| Game Passes (`game-pass`) | `list/get/create/update_game_pass` |
| Creator Store (`creator-store-product`) | `search_creator_store`, `get_creator_store_asset`, `get_creator_store_product`; create and update via `open_cloud_request` |
| Groups (`group:read`, `group:write`) | `get_group`, `list_group_memberships`, `list_group_roles`, `list_group_join_requests`, `resolve_group_join_request`, `set_group_role` |
| Users and Inventory (`user.advanced:read`, `user.social:read`, `user.inventory-item:read`) | `get_user`, `generate_user_thumbnail`, `list_inventory` |
| Notifications (`user.user-notification:write`) | `send_notification` |
| Subscriptions (`universe.subscription-product.subscription:read`) | `get_subscription` |
| Analytics (`universe.analytics:read`) | `query_analytics` |
| Everything else: Ads, Game Events, Thumbnails, Creator Store saves, Legacy Badges, Legacy Develop, Legacy Followings, Legacy Game Internationalization, Legacy Groups, Legacy Localization Tables, Legacy Publish, Legacy Team Collaboration, Legacy Users | `open_cloud_request` |

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

## Known limits

These are platform limits, not bugs. They are written down here so you know going in.

- F5 playtest stop is best-effort. `stop_playtest` may not take, because the plugin and the running game are separate DataModels. `stop_simulation` (F8) stops cleanly. If an F5 playtest will not stop, press Stop in Studio.
- In-play actions go through an injected runner. Input, runtime state, and stop during a playtest are relayed over the bridge, not called directly on the plugin.
- Headless tests run a server context. Open Cloud runs your published place on a server, where `RunService:IsStudio()` is false and plugin APIs are absent. Use it for server and gameplay logic and the security tests, not for Studio-plugin or client-input behaviour.
- Tool parity with Roblox's built-in Assistant is maintained by hand.
- Several Open Cloud systems are beta or experimental on Roblox's side (the Instance API, user restrictions, secrets, game servers, analytics, and everything behind `open_cloud_request`). Their shapes can move, and `open_cloud_request` gives you the raw response, not a tidied one.

Tripwire is maintained by one person in spare time. Issues and pull requests usually get a reply within about a week. A slow reply is not a no.

---

## Troubleshooting

**The Studio tools time out or report no connected Studio.** The plugin is not connected. Install `Tripwire.rbxmx`, restart Studio, click the Tripwire toolbar button, and press Connect in the panel. The panel status and the Output line `[Tripwire v...] connected` both confirm it.

**The plugin reports that HTTP is blocked.** Turn on Game Settings > Security > Allow HTTP Requests on the open place. This is the most common setup failure. Studio cannot reach the local bridge without it.

**The Output shows a version mismatch between the plugin and the server.** The installed `.rbxmx` is stale. Rebuild it (`npx rbxtsc && rojo build --output Tripwire.rbxmx`), copy it into your Plugins folder, and restart Studio. The plugin prints its compiled version, so the prefix tells you what is actually installed; the panel header shows the same version.

**An Open Cloud tool returns `401 Invalid API Key`.** The key itself is not accepted: it was regenerated, expired, or pasted incompletely. Every tool fails the same way. Create or copy the key again on the Creator Dashboard, replace `ROBLOX_OPEN_CLOUD_KEY`, and reconnect the MCP server.

**An Open Cloud tool returns a 403 or scope error.** The key is missing the scope that tool needs, the universe or place id is wrong, or the place is not published. Add the scope on the Creator Dashboard (the tool description names it), confirm the ids, and reconnect the MCP server. The error text is Roblox's own, so it names what is missing.

**`run_tests` does not see your latest change.** It reads the published place. Publish first; `rojo serve` only updates the live edit session, not what Open Cloud runs.

**`npx` cannot fetch the server.** You have no Node.js, or no network for the first download. Install Node, or use the prebuilt binary from the Releases page and point `command` at it (see Alternatives under Install).

**The server says the bridge port is busy.** A previous server is still holding port 44331. Close the old MCP session or the stale process, then reconnect.

---

## Prior art

The Studio runtime approach (a plugin that long-polls a local server, an injected in-play runner)
follows ideas from [boshyxd/robloxstudio-mcp](https://github.com/boshyxd/robloxstudio-mcp) and
[Chrrxs/robloxstudio-mcp](https://github.com/Chrrxs/robloxstudio-mcp). Tripwire is an independent,
from-scratch implementation; the headless test harness, the CI security reviewer, and the Open Cloud
tooling are its own.

---

## Contributing

Bug reports, feature ideas, and patches are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for
the setup, the build gates, and the branch and commit rules, and [ARCHITECTURE.md](ARCHITECTURE.md)
for how the pieces fit together. By taking part you agree to the
[Code of Conduct](CODE_OF_CONDUCT.md).

Found a security issue? Do not open a public issue. See [SECURITY.md](SECURITY.md) for the private
disclosure path.

## Changelog

Release notes for each version are on the [Releases](https://github.com/aliboIly/Tripwire/releases) page.

---

## License

MIT. See [LICENSE](LICENSE).
