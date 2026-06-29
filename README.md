# Tripwire

Tripwire is an MCP server for Roblox Studio, built around the part other Studio MCPs do not have:
headless tests-as-code that run in the real engine through Open Cloud, a CI action that runs them
on every pull request, and an automated security reviewer that flags client-trust holes and
unvalidated remotes and proposes the server-side fix. On top of that it covers the full Studio
surface, reading, editing, inspecting, and playtesting an open place from an MCP client (Claude
Code, Codex, Gemini, Cursor).

Scope is honest about its edges. Stopping an F5 playtest is best effort, in-play actions go
over an injected runner bridge, and headless tests cover server and gameplay logic rather than
client input. None of those are bugs to chase; they fall out of how Studio works.

## Prior art

Tripwire's runtime approach, driving Studio across the edit and playtest DataModels, was informed
by prior open-source Roblox MCP servers, notably boshyxd/robloxstudio-mcp and
Chrrxs/robloxstudio-mcp. Tripwire is an independent, from-scratch implementation: where a
technique is forced by the engine it is re-derived here rather than ported, and the test, CI, and
security layer is original to Tripwire.

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
