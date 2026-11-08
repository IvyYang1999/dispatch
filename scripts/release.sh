#!/bin/sh
# Build, sign, notarize, staple and verify a macOS release. PUBLISH=1 also publishes --latest.
# Secrets are injected by KeyKeeper; the driver never writes private keys to disk.
# See docs/releasing.md for prerequisites, artifacts and resuming interrupted notarization.
set -eu
cd "$(dirname "$0")/.."
exec python3 scripts/release.py "$@"
