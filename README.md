# Tripwire

An MCP server for Roblox Studio. It lets an MCP client (Claude Code, Codex, Gemini, Cursor)
read, edit, inspect, and playtest an open place, run Luau tests headlessly through Open Cloud,
and check game code for client-trust and security holes.

Scope is honest about its edges. Stopping an F5 playtest is best effort, in-play actions go
over an injected runner bridge, and headless tests cover server and gameplay logic rather than
client input. None of those are bugs to chase; they fall out of how Studio works.

## Read first

`Docs/Dev/SETUP.md` has every command to install, build, and run.

## Layout

- `server/`: the MCP server (Node TypeScript). Tool dispatch, the local bridge, the Open Cloud client.
- `plugin/`: the Studio plugin (roblox-ts, compiled to Luau). Long-polls the bridge and runs actions.
- `runner/`: the in-play runner (Luau), injected at playtest start.
- `sample-game/`: a Rojo test target (lands in Phase 4).
