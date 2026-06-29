# Tripwire

An MCP server for Roblox Studio. It lets an MCP client (Claude Code, Codex, Gemini, Cursor)
read, edit, inspect, and playtest an open place, run Luau tests headlessly through Open Cloud,
and check game code for client-trust and security holes.

Scope is honest about its edges. Stopping an F5 playtest is best effort, in-play actions go
over an injected runner bridge, and headless tests cover server and gameplay logic rather than
client input. None of those are bugs to chase; they fall out of how Studio works.

## Install the plugin

Download the latest `Tripwire.rbxmx` from the Releases page and drop it into your Studio
plugins folder, or build it from source with `Docs/Dev/SETUP.md`. CI builds the server and the
plugin on every pull request, and pushing a `vX.Y.Z` tag publishes a release with the `.rbxmx`
attached. Every pull request also gets an automated security review comment that flags
client-trust holes in the game code.

## Read first

- `Docs/ARCHITECTURE.md` explains how the repo fits together, file by file.
- `Docs/Dev/SETUP.md` has every command to install, build, and run.

## Layout

- `server/`: the MCP server (Node TypeScript). Tool dispatch, the local bridge, the Open Cloud client.
- `plugin/`: the Studio plugin (roblox-ts, compiled to Luau). Long-polls the bridge and runs actions.
- `runner/`: the in-play runner (Luau), injected at playtest start.
- `sample-game/`: a server-authoritative Rojo game used as the test and security-review target. It carries one deliberate client-trust hole for the reviewer to flag.
