#!/bin/sh
# Build the CLI and the desktop app in release mode and zip them together for another Mac.
# Output: dist/Dispatch-<version>-macos-<arch>.zip holding Dispatch.app and the dispatch CLI.
set -e
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')
ARCH=$(uname -m)
OUT="dist/Dispatch-$VERSION-macos-$ARCH"

cargo build --release -p dispatch
(cd apps/desktop && npm run tauri build -- --bundles app)

rm -rf "$OUT" "$OUT.zip"
mkdir -p "$OUT"
cp -R target/release/bundle/macos/Dispatch.app "$OUT/"
cp target/release/dispatch "$OUT/dispatch"
cat > "$OUT/安装.txt" <<'EOF'
Dispatch（第一版，ad-hoc 签名，没有公证）

1. 把 Dispatch.app 拖到「应用程序」。
2. 第一次打开前，在终端里去掉隔离属性（不然 macOS 会说「已损坏」）：
     xattr -dr com.apple.quarantine /Applications/Dispatch.app
     xattr -d  com.apple.quarantine ./dispatch
3. 装命令行、hook，并注册到 Codex：
     ./dispatch install
4. 给 Claude Code 装插件（MCP 工具 + skill + hook）：在 Claude Code 里输入
     /plugin install dispatch --marketplace IvyYang1999/dispatch
   给 Codex 装插件：
     codex plugin marketplace add IvyYang1999/dispatch && codex plugin add dispatch
5. 打开 Dispatch.app。
EOF
(cd dist && zip -qry "$(basename "$OUT").zip" "$(basename "$OUT")")
echo "built: $OUT.zip"
ls -la "$OUT.zip"
