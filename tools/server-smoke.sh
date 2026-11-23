#!/usr/bin/env bash
# End-to-end smoke test: start basalt-server (in-memory world) and drive it with basalt-client.
# usage: tools/server-smoke.sh [OUTDIR]
set -uo pipefail
cd "$(dirname "$0")/.."
OUT=${1:-target/smoke}
PORT=${PORT:-25580}
mkdir -p "$OUT"
cargo build -q -p basalt-server -p basalt-client || exit 1
./target/debug/basalt-server --memory --flat --port "$PORT" --gamemode creative > "$OUT/server.log" 2>&1 &
SRV=$!
trap 'kill $SRV 2>/dev/null' EXIT
for _ in $(seq 1 50); do grep -q "listening" "$OUT/server.log" && break; sleep 0.2; done
fail=0
check() { if grep -q "$2" "$1"; then echo "ok   $3"; else echo "FAIL $3"; fail=1; fi; }

# 1. join, chat, command, place, walk; screenshot
./target/debug/basalt-client 127.0.0.1:$PORT --name Alice --screenshot "$OUT/alice.png" --wait 10 \
  --chat "hello from alice" --cmd "give @s stone 10" --look 0,60 --click r > "$OUT/alice.log" 2>&1 &
A=$!
sleep 2
# 2. a second player joining at the same time (multiplayer visibility)
./target/debug/basalt-client 127.0.0.1:$PORT --name Bob --screenshot "$OUT/bob.png" --wait 14 --walk 4 --look 90,10 > "$OUT/bob.log" 2>&1
wait $A
check "$OUT/alice.log" "screenshot" "alice joined and rendered"
check "$OUT/bob.log" "screenshot" "bob joined and rendered"
check "$OUT/alice.log" "<Alice> hello from alice" "chat echoed"
check "$OUT/server.log" "Bob joined" "server saw second player"
check "$OUT/alice.log" "entities [1-9]" "alice saw another entity"
check "$OUT/server.log" "issued /give" "command received"
grep -E "WARN|ERROR|bad packet|panicked" "$OUT/server.log" && { echo "FAIL server warnings"; fail=1; }
exit $fail
