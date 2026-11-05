#!/bin/sh
# The skill and the launcher are written once (plugins/skill, plugins/bin) and copied
# into each plugin, because marketplaces clone the plugin folder as it is.
set -e
cd "$(dirname "$0")/.."
for p in claude codex; do
  mkdir -p "plugins/$p/skills/dispatch" "plugins/$p/bin"
  cp plugins/skill/SKILL.md "plugins/$p/skills/dispatch/SKILL.md"
  cp plugins/bin/dispatch "plugins/$p/bin/dispatch"
  chmod +x "plugins/$p/bin/dispatch"
done
echo "plugins synced"
