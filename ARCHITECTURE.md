# Architecture

This is a map of how Tripwire fits together, for people working on the code. It is shorter than the internal notes; it covers what you need to find your way around.

## The pieces

Tripwire is a small monorepo with two build systems.

- `server/` is the MCP server, written in Rust on the rmcp SDK and tokio. It registers every MCP tool, runs a local HTTP bridge the plugin talks to, calls Open Cloud, runs the headless test harness, and does the static security analysis. This is the orchestrator. Everything routes through it.
- `plugin/` is the Studio plugin, written in roblox-ts and compiled to Luau. It long-polls the server's bridge, runs the read, write, and playtest actions against the open place, and posts results back. It acts on command; it does not decide anything on its own.
- `runner/` is a small Luau template injected into a running playtest. The plugin cannot reach into a live game, so in-play work (input, runtime state, stop) is done by this runner, which polls the same bridge. The server embeds the template at build time.
- `sample-game/` is a server-authoritative roblox-ts game used as the target for the test harness and the security reviewer. It has one deliberate client-trust hole so the reviewer has something to flag.

## The two-DataModel rule

The one thing to understand first: the plugin and a running playtest are separate DataModels. The plugin cannot call into the playing game. So anything during a playtest (simulated input, reading runtime state, stopping) goes through the injected runner over HTTP polling, never a direct plugin call. Code that assumes the plugin can read the running game is wrong.

A second boundary: Open Cloud execution is a server context, not Studio. `RunService:IsStudio()` is false there and plugin APIs are absent. It runs server and gameplay logic and the security tests. It cannot test Studio-plugin behaviour.

## The bridge

The plugin and the runner reach the server over a local HTTP bridge (axum, in `server/src/bridge.rs`). The plugin long-polls for commands, runs them, and posts results back. The protocol is versioned: a mismatch between the server and the plugin fails loudly rather than dropping fields. The bridge keys its queues by peer role (plugin, server runner, client runner) so the plugin and the runners do not take each other's commands.

The plugin reports a clear error when Allow HTTP Requests is off (Game Settings, Security). That is the most common setup failure.

## Source layout

```
server/src/
  main.rs        registers and dispatches the MCP tools, plus the review CLI
  bridge.rs      the local HTTP bridge the plugin and runner poll
  cloud.rs       Open Cloud Luau Execution client
  opencloud.rs   Open Cloud REST tools (data stores, messaging, memory, platform)
  harness.rs     headless test harness over the cloud client
  playtest.rs    playtest lifecycle: inject the runner, start, stop, input
  security.rs    static security analysis of a Rojo source tree (swc)
plugin/src/      long-polls the bridge, runs data-model and playtest actions
runner/          the injected in-play runner (Luau template)
sample-game/src/ server, client, shared
```

## Two sources of truth

- For Studio state during a session, the server is authoritative. The plugin executes what the server sends and reports back.
- For the sample game's code, the filesystem (Rojo) is authoritative, the same as any roblox-ts project. Do not edit the sample game live in Studio; change the `.ts` and let Rojo sync, or the next sync clobbers it.
