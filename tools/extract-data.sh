#!/usr/bin/env bash
# Downloads the official Minecraft 26.2 server jar from Mojang and runs its
# built-in data generator. Output goes to ./data (not committed — Mojang data
# stays on your machine). Requires Java 25+.
set -euo pipefail
cd "$(dirname "$0")/.."
VER=26.2
mkdir -p vanilla
if [ ! -f vanilla/server.jar ]; then
  URL=$(curl -s https://piston-meta.mojang.com/mc/game/version_manifest_v2.json | python3 -c "import sys,json;print([v['url'] for v in json.load(sys.stdin)['versions'] if v['id']=='$VER'][0])")
  SRV=$(curl -s "$URL" | python3 -c "import sys,json;print(json.load(sys.stdin)['downloads']['server']['url'])")
  curl -L -o vanilla/server.jar "$SRV"
fi
JAVA=${JAVA:-java}
rm -rf vanilla/gen
(cd vanilla && "$JAVA" -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports --server --output gen)
rm -rf data && mkdir -p data && cp -r vanilla/gen/reports vanilla/gen/data data/
echo "data extracted to ./data"
