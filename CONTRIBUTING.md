# Contributing to Tripwire

Thanks for looking at Tripwire. This file covers how to build it from source, run the checks, and send a change. If you only want to use Tripwire, the README has the install steps and you do not need any of this.

Tripwire has one maintainer (alibolly). Expect a reply on issues and pull requests within about a week. The project is worked on in spare time, so review can be slower during busy stretches. A slow reply is not a no.

## Before you open a pull request

Open an issue first for anything past a small fix. It saves you writing code that does not fit. Tripwire is an MCP server plus a Studio plugin that gives an AI coding agent control of Roblox Studio and Open Cloud, with headless tests and a static security reviewer on top. Changes that serve that are welcome. Changes that turn it into something else are likely out of scope, and it is faster to hear that on an issue than on a finished branch.

### Known limits that are not bugs

Some rough edges come from the Roblox platform, not from Tripwire. Please do not file these as bugs or send patches that claim to fix them.

- F5 playtest stop is best-effort. The plugin and the running game are separate DataModels, so a programmatic stop of an F5 playtest often does not take. `stop_simulation` (F8) stops cleanly; `stop_playtest` (F5) may need a human to press Stop.
- In-play actions go through the injected runner, not the plugin. Input, runtime state, and stop during a playtest are relayed over HTTP polling, because the plugin cannot reach into the running game.
- Headless tests cover server and gameplay logic, not client input. Open Cloud runs a server context where `RunService:IsStudio()` is false and plugin APIs do not exist.
- Tool parity with Roblox's built-in Assistant is maintained by hand. There is no auto-sync.

If one of these bites in a new way, an issue that sharpens the docs is welcome. A patch that pretends the limit is gone is not.

## Toolchain

Each part of the repo needs different tools.

- Server (`server/`): Rust, via rustup (https://rustup.rs). Stable toolchain with rustfmt and clippy.
- Plugin (`plugin/`) and sample game (`sample-game/`): Node.js 20 (for rbxtsc) plus the Roblox tools Rojo and Lune.
- Rojo and Lune are pinned in `rokit.toml`. Install Rokit (https://github.com/rojo-rbx/rokit), then run `rokit install` at the repo root to get the pinned versions. Local and CI match that way.

The server is a normal cargo crate. Do not run rbxtsc or tsc on it. The plugin and sample game are roblox-ts; build them with rbxtsc, not plain tsc (plain tsc cannot resolve the `@rbxts` libraries).

## Build from source

### Server

```bash
cd server
cargo build --release        # produces server/target/release/tripwire-server
```

### Plugin

```bash
rokit install                # once, from the repo root, installs rojo + lune
cd plugin
npm install
npx rbxtsc                    # compiles src/ TypeScript to out/ Luau
rojo build --output Tripwire.rbxmx
```

Copy `Tripwire.rbxmx` into your Studio plugins folder to test it:

- macOS: `~/Documents/Roblox/Plugins/`
- Windows: `%LOCALAPPDATA%\Roblox\Plugins\`

During plugin work you can `rojo serve` instead of rebuilding the `.rbxmx` each time.

### Sample game

The sample game is the target the test harness and the security reviewer run against. It is server-authoritative and carries one deliberate client-trust hole so the reviewer has something concrete to catch.

```bash
cd sample-game
npm install
npx rbxtsc
```

## Run it locally

Point your MCP client at the binary you built instead of the published npm package. For Claude Code:

```bash
claude mcp add --transport stdio tripwire -- /absolute/path/to/server/target/release/tripwire-server
```

The Studio tools need the plugin installed and running, with Allow HTTP Requests on (Game Settings, Security). The headless and Open Cloud tools need an Open Cloud key in a `.env` at the repo root. Copy `.env.example` to `.env` and fill it in. The README has the full Open Cloud walkthrough.

## Checks

Run the same gates CI runs before you push.

Server:

```bash
cd server
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Plugin and sample game:

```bash
cd plugin && npm run build         # rbxtsc is the type gate
cd ../sample-game && npm run build
```

There is a pre-push hook that rebuilds the plugin `.rbxmx` so it never lags the source version. Turn it on once per clone:

```bash
git config core.hooksPath .githooks
```

The headless test suite and the security review run through the server against a published place over Open Cloud. They need your key, so they are not part of the local gate. CI runs the security review on every pull request.

## Commit and pull request workflow

- Work on a branch, then open a pull request against `main`. Do not push to `main`.
- One pull request per logical change.
- Conventional commits with a scope: `type(scope): summary`. Types are feat, fix, refactor, chore, docs. Scopes match the area you touched (server, plugin, runner, bridge, cloud, harness, security, sample, ci, build, docs).
- Keep each commit building. Land a type change together with its call sites.
- The docs are part of the code. If you change behaviour, update the README and any affected doc in the same change.
- Plain ASCII in code, comments, and commit messages. No em-dashes, no emoji.

## Reporting security issues

Do not open a public issue for a vulnerability. See [SECURITY.md](SECURITY.md) for the private disclosure path.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). By taking part you agree to uphold it.
