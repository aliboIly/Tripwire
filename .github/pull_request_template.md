## What this changes

<!-- One or two sentences. Link the issue it closes, if any. Closes #123. -->

## Why

<!-- The reason behind the change. The diff shows what; this explains why. -->

## Scope

- [ ] One logical change. Unrelated work is in its own pull request.
- [ ] Commits follow `type(scope): summary` (feat, fix, refactor, chore, docs).

## Gates

Tick what applies to the packages you touched.

Server (`server/`):
- [ ] `cargo build --release` is clean
- [ ] `cargo clippy --all-targets -- -D warnings` is clean
- [ ] `cargo fmt --check` passes

roblox-ts (`plugin/`, `sample-game/`):
- [ ] `npm run build` (rbxtsc) passes
- [ ] ESLint passes

## Docs

- [ ] README or tool descriptions updated if behavior or the tool surface changed
- [ ] No secrets, keys, or `.env` values in the diff
