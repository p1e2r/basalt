#!/usr/bin/env bash
# M7 client scenarios: start a fresh in-memory Basalt server and run `basalt-client --scenario all`
# against it (offscreen rendering: needs a GPU, so local-only). Scenarios that need the vanilla
# server (dig_others) use VANILLA=host:port (default 127.0.0.1:25566) when reachable, else SKIP.
# usage: tools/client-scenarios.sh [SCENARIO[,SCENARIO...]|all]   (output: SCENARIO M7/<name> PASS|FAIL|SKIP ...)
set -uo pipefail
cd "$(dirname "$0")/.."
WHICH=${1:-all}
PORT=${PORT:-25790}
VANILLA=${VANILLA:-127.0.0.1:25566}
OUT=${OUT:-target/client-scenarios}
mkdir -p "$OUT"
cargo build -q --release -p basalt-server -p basalt-client || exit 1
./target/release/basalt-server --memory --flat --port "$PORT" --no-spawning > "$OUT/server.log" 2>&1 &
SRV=$!
trap 'kill $SRV 2>/dev/null' EXIT
for _ in $(seq 1 100); do grep -q "listening" "$OUT/server.log" && break; sleep 0.2; done
BASALT_SCENARIO_SHOTS="$OUT/shots" ./target/release/basalt-client --addr "127.0.0.1:$PORT" --vanilla "$VANILLA" --scenario "$WHICH" 2> "$OUT/client.log" | tee "$OUT/client.out" | grep --line-buffered "^SCENARIO" | tee "$OUT/results.txt"
rc=${PIPESTATUS[0]}
grep -E "panicked" "$OUT/server.log" && { echo "FAIL server panicked"; rc=1; }
exit $rc
