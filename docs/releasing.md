# macOS releases

Run from a clean, committed checkout on the signing Mac. The tag, workspace version in
`Cargo.toml` and app version in `apps/desktop/src-tauri/tauri.conf.json` must match. The script
does not bump versions or modify source files. Linux/Windows bundles are still built by
`release.yml` when the published tag appears.

Prerequisites: Rust, Node/npm, Python 3.9+, Xcode command-line tools, `minisign`, frontend
dependencies (`npm ci` in `apps/desktop`), and a Developer ID Application identity in the
macOS Keychain. Publishing additionally needs `gh` with an existing login. If multiple
Developer ID Application identities are available, select one with `APPLE_SIGNING_IDENTITY`.

## Build and verify, without publishing

```sh
keykeeper run -c dispatch-updater-signing -c apple-notary \
  --reason "Build, sign and notarize the Dispatch release candidate; do not publish" -- \
  scripts/release.sh v0.1.5 "What changed"
```

`TAURI_SIGNING_PRIVATE_KEY` and its optional `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` are passed
only to the final Tauri signer process. `APPLE_ID`, `APPLE_TEAM_ID` and
`APPLE_APP_SPECIFIC_PASSWORD` authenticate `notarytool`. An existing `NOTARYTOOL_PROFILE`
can replace those three fields; in that case inject only the updater credential. No private
key is read from a default file or written to disk. The Developer ID private key stays in
the macOS Keychain. Do not run the script under shell tracing or print the environment.

The sequence is mandatory:

1. Build the CLI and App with updater artifacts disabled.
2. Sign the embedded CLI and then the App with hardened runtime and a timestamp.
3. Submit the App and standalone CLI separately; require both to be `Accepted`.
4. Staple and validate the App, verify its signature and Gatekeeper assessment. Verify the
   standalone CLI with `codesign -R=notarized --check-notarization` (`spctl` expects an App).
5. Package the stapled App into the updater archive, sign it with `--app-version`, and verify
   against the committed public key. Compare extracted ZIP/TAR contents with the signed payload.
6. Write `latest.json` with JSON escaping and `SHA256SUMS.txt` with exactly four entries.

The run directory is `dist/release-<tag>-<commit>/`. It contains notarization receipts and
exactly these five **publishable** files (the script selects them explicitly):

- `Dispatch-<version>-macos-<arch>-signed.zip` (App, CLI and installation instructions)
- `Dispatch-<version>-<platform>.app.tar.gz`
- `Dispatch-<version>-<platform>.app.tar.gz.sig`
- `latest.json`
- `SHA256SUMS.txt`

App and CLI inside the first-install ZIP match the updater App. This directory is ignored
by Git; do not upload internal submission archives or receipts as release assets.

## Publish the verified candidate

```sh
keykeeper run -c dispatch-updater-signing -c apple-notary \
  --reason "Verify and publish the approved Dispatch release" -- \
  env PUBLISH=1 scripts/release.sh v0.1.5 "What changed" --notes-file /absolute/path/release-notes.md
```

Use `--notes-file` for a full release body; the positional notes are the updater summary.
If a build was already made, repeat the **same** source, tag and notes with `--resume`.
The script reuses accepted submissions, exports and verifies the final artifacts again,
then publishes a normal release with `--latest` and `--target <verified commit>`. It refuses
an existing tag instead of silently attaching a different build to it. There is no
`--prerelease`: the installed updater reads `releases/latest/download/latest.json`.

## Interrupted or rejected notarization

- Keep the run directory. Use the same command with `--resume` after a timeout; a recorded
  submission ID is polled again, never submitted again. Concurrent runs are refused.
- An `Invalid` submission stops the pipeline and saves Apple's issues in `*.log.json`.
  Fix the reported cause on a new commit before starting a new candidate.
- If the submit connection died before returning an ID, the receipt says `Submitting`.
  The script refuses to upload again. Inspect Apple's `notarytool history` and `log` to
  reconcile that request before proceeding; elapsed time alone is not a failed submission.
- Changing an archive, source commit, version or notes invalidates resume/publication.
  No quarantine removal, Gatekeeper bypass, unsigned fallback or forced submission is used.

## Installed CLI after an App update

On a release App's startup, an already-installed `~/.dispatch/bin/dispatch` is refreshed
from the bundled CLI using a same-directory atomic replacement. This runs on the blocking
pool and works on the first upgrade from 0.1.4, whose updater only replaces the App.
Identical copies are a no-op; a newer CLI is retained. First launch still requires the
explicit Install button. Development builds do not replace a production CLI.

This synchronization does not run `dispatch install`, rewrite Claude hooks or register MCP
again. Existing hook/MCP paths keep pointing at the refreshed executable. Running MCP
processes keep their old in-memory code until the client restarts them. If replacement
fails (for example a locked executable on Windows), the old file remains and Settings
shows an error with the Reinstall action.

Run `python3 -m unittest discover -s scripts -p test_release.py -v` for the release failure
gates, and `cargo test -p dispatch-desktop cli_sync` for CLI replacement tests. Real signing
and notarization require a build-only run on the signing Mac.
