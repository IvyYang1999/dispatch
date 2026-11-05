#!/bin/sh
# Build a release of Dispatch for this Mac and, with PUBLISH=1, publish it as a GitHub prerelease.
#
#   scripts/release.sh v0.1.1 "what changed"            # build only: dist/
#   PUBLISH=1 scripts/release.sh v0.1.1 "what changed"  # build and publish
#
# Produces, under dist/:
#   Dispatch-<version>-macos-<arch>.zip          Dispatch.app + dispatch CLI + 安装.txt (for a first install)
#   Dispatch-<version>-<platform>.app.tar.gz     the updater package (signed with the Tauri updater key)
#   latest.json                                  what installed apps poll to find this release
#
# The updater key: TAURI_SIGNING_PRIVATE_KEY (a path or the key text), default ~/.tauri/dispatch.key.
# Code signing / notarization with a Developer ID is a separate step on the machine that has it.
set -e
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
TAG="${1:?usage: release.sh <tag> [notes]}"
NOTES="${2:-Dispatch $TAG}"
VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
ARCH=$(uname -m)
case "$ARCH" in
  arm64) TRIPLE=aarch64-apple-darwin; PLATFORM=darwin-aarch64 ;;
  x86_64) TRIPLE=x86_64-apple-darwin; PLATFORM=darwin-x86_64 ;;
  *) echo "unsupported arch $ARCH" >&2; exit 1 ;;
esac
: "${TAURI_SIGNING_PRIVATE_KEY:=$HOME/.tauri/dispatch.key}"
export TAURI_SIGNING_PRIVATE_KEY
[ -f "$TAURI_SIGNING_PRIVATE_KEY" ] || [ "${#TAURI_SIGNING_PRIVATE_KEY}" -gt 200 ] || { echo "no updater key at $TAURI_SIGNING_PRIVATE_KEY" >&2; exit 1; }

# keep the app's version in step with the workspace
sed -i '' -E "s/\"version\": \"[^\"]+\"/\"version\": \"$VERSION\"/" apps/desktop/src-tauri/tauri.conf.json

cargo build --release -p dispatch
mkdir -p apps/desktop/src-tauri/binaries
cp target/release/dispatch "apps/desktop/src-tauri/binaries/dispatch-$TRIPLE"
# EXTRA_TAURI_CONFIG merges a JSON fragment into tauri.conf.json for this build only
# (for example allowing an http update endpoint while testing the updater locally).
if [ -n "$EXTRA_TAURI_CONFIG" ]; then
  (cd apps/desktop && npm run tauri build -- --bundles app --config "$EXTRA_TAURI_CONFIG")
else
  (cd apps/desktop && npm run tauri build -- --bundles app)
fi

OUT="dist/Dispatch-$VERSION-macos-$ARCH"
rm -rf "$OUT" "$OUT.zip"
mkdir -p "$OUT"
cp -R target/release/bundle/macos/Dispatch.app "$OUT/"
cp target/release/dispatch "$OUT/dispatch"
cat > "$OUT/安装.txt" <<'EOF'
Dispatch

1. 把 Dispatch.app 拖到「应用程序」。
2. 如果这个包没有经过公证，第一次打开前先去掉隔离属性：
     xattr -dr com.apple.quarantine /Applications/Dispatch.app
3. 打开 Dispatch.app → 设置 → 一键安装（装命令行、Claude Code 的 hook，并注册到 Codex）。
   或者在终端里：./dispatch install
4. 给 Claude Code 装插件：在 Claude Code 里输入
     /plugin install dispatch --marketplace IvyYang1999/dispatch
   给 Codex 装插件：
     codex plugin marketplace add IvyYang1999/dispatch && codex plugin add dispatch
EOF
(cd dist && zip -qry "$(basename "$OUT").zip" "$(basename "$OUT")")

TGZ=target/release/bundle/macos/Dispatch.app.tar.gz
[ -f "$TGZ" ] && [ -f "$TGZ.sig" ] || { echo "updater artifacts missing: is createUpdaterArtifacts on and the key set?" >&2; exit 1; }
cp "$TGZ" "dist/Dispatch-$VERSION-$PLATFORM.app.tar.gz"
SIG=$(cat "$TGZ.sig")
cat > dist/latest.json <<EOF
{
  "version": "$VERSION",
  "notes": "$NOTES",
  "pub_date": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "platforms": {
    "$PLATFORM": {
      "signature": "$SIG",
      "url": "https://github.com/IvyYang1999/dispatch/releases/download/$TAG/Dispatch-$VERSION-$PLATFORM.app.tar.gz"
    }
  }
}
EOF
echo "built:"
ls -la "$OUT.zip" "dist/Dispatch-$VERSION-$PLATFORM.app.tar.gz" dist/latest.json

if [ "$PUBLISH" = "1" ]; then
  gh release create "$TAG" "$OUT.zip" "dist/Dispatch-$VERSION-$PLATFORM.app.tar.gz" dist/latest.json --prerelease --title "Dispatch $VERSION" --notes "$NOTES"
  echo "published $TAG"
fi
