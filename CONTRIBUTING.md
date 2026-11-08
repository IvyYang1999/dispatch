# Contributing

Dispatch is small on purpose: a Rust core, a command line and MCP server on top of it, a Tauri
app, and two plugins. Read `docs/message-format.md` and `docs/adapters.md` first; they say
what the files on disk mean and how a message travels.

## Build and test

```
cargo test --workspace                         # the core's tests
cargo build --release -p dispatch              # the command line
cd apps/desktop && npm install && npm run tauri dev
```

The app embeds the command line as a sidecar, so a build of the app needs it in place first:
`node scripts/sidecar.mjs` builds it and copies it to `apps/desktop/src-tauri/binaries/`.

CI (`.github/workflows/ci.yml`) runs the tests on macOS, Linux and Windows. Linux needs the
WebKitGTK and appindicator development packages Tauri lists; the workflow installs them.

## Texts

Everything a person reads lives in `crates/core/src/i18n.rs` as a key with a Chinese and an
English text, and in `apps/desktop/src/i18n.ts` for the app's own strings. What goes on disk
(status words, urgency, file names) is always Chinese and never changes with the language.
Add new keys to `every_key_has_both_languages` in the same file.

## Releases

- macOS: `scripts/release.sh v0.1.5 "what changed"` signs, notarizes, staples and verifies
  the App and CLI, then signs the final updater archive. Inject credentials with KeyKeeper;
  see [Releasing](docs/releasing.md) for the full command and interrupted-run recovery.
  Only `PUBLISH=1` creates a normal GitHub release marked latest, with five verified assets.
- Linux and Windows: `.github/workflows/release.yml` builds `.deb`, `.rpm`, `.AppImage`,
  `.msi` and an NSIS installer when a `v*` tag is pushed (or by hand, given a tag), and
  attaches them to that release. They are unsigned and carry no updater artifacts.

The version lives in `Cargo.toml` (workspace) and `apps/desktop/src-tauri/tauri.conf.json`;
the release script requires both to match the tag and never rewrites them.

## Plugins

The skill is written once in `plugins/skill/SKILL.md`; `scripts/sync-plugins.sh` copies it into
the Claude Code and Codex plugins. Edit the source, then sync.
