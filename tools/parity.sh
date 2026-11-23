#!/usr/bin/env bash
# Worldgen parity of Basalt vs real vanilla worlds: N seeds x several sites per seed.
#
# usage: tools/parity.sh [--out DIR] [--radius-scale 1] SEED...
#   Vanilla reference worlds are generated headless with the official server jar (vanilla/server.jar)
#   and cached in vanilla/parity/<seed> (marker done-26.2; shared with tools/check-all.sh), so only the
#   first run of a seed is slow. Each seed is compared at the sites below (centre chunk + radius, all
#   inside the force-loaded chunk square -5..4) with the check-all thresholds:
#   biomes >= 99.9, shape >= 99.0, exact >= 98.5, features >= 98.5.
# Output: one `PARITY seed=.. site=cx,cz,r chunks=.. biomes=.. shape=.. exact=.. features=.. ok=0|1` per
#   site and a final `RESULT fuzz=worldgen cases=.. mismatches=.. new=.. errors=.. skipped=.. seeds=..`.
# env: JAVA (java), PARITY_PORT_BASE (25700; vanilla generation servers use 25700-25799)
set -uo pipefail
cd "$(dirname "$0")/.."
OUT=target/check
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT=$2; shift 2 ;;
    *) break ;;
  esac
done
SEEDS=("$@")
[ ${#SEEDS[@]} -gt 0 ] || SEEDS=(12345 42 1 7 2026 99999)
JAVA=${JAVA:-java}
SITES=("0 0 3" "-3 -3 2" "2 2 2" "-3 2 2" "2 -3 2")
mkdir -p "$OUT"

echo "note: building the worldgen compare example"
cargo build --release -p basalt-worldgen --example compare -q 2>&1 | tail -5
[ -x target/release/examples/compare ] || { echo "RESULT fuzz=worldgen cases=0 mismatches=0 new=0 info=0 errors=1 skipped=1 secs=0"; exit 1; }

free_port() { python3 - "${PARITY_PORT_BASE:-25700}" <<'EOF'
import socket, sys
base = int(sys.argv[1])
for p in range(base, base + 100):
    s = socket.socket()
    try:
        s.bind(("", p)); s.close(); print(p); break
    except OSError:
        s.close()
EOF
}

# Generate a vanilla world for $1 (same procedure and marker as tools/check-all.sh).
gen_vanilla_world() {
  local seed=$1 dir=vanilla/parity/$1
  [ -f "$dir/done-26.2" ] && return 0
  [ -f vanilla/server.jar ] || { echo "vanilla/server.jar missing (run tools/extract-data.sh)"; return 1; }
  rm -rf "$dir"; mkdir -p "$dir"
  local port; port=$(free_port)
  echo "eula=true" > "$dir/eula.txt"
  cat > "$dir/server.properties" <<EOF
level-seed=$seed
server-port=$port
online-mode=false
level-type=minecraft\:normal
generate-structures=true
spawn-protection=0
view-distance=2
simulation-distance=2
max-tick-time=-1
pause-when-empty-seconds=-1
network-compression-threshold=-1
sync-chunk-writes=false
EOF
  rm -f "$dir/console.fifo"; mkfifo "$dir/console.fifo"
  ( cd "$dir" && exec "$JAVA" -Xmx2G -jar ../../server.jar nogui < console.fifo > server.log 2>&1 ) &
  local pid=$!
  exec 7> "$dir/console.fifo"
  local t=0
  until grep -q "Done (" "$dir/server.log" 2>/dev/null; do sleep 1; t=$((t+1)); [ $t -gt 300 ] && { echo "vanilla server for seed $seed didn't start"; kill $pid; exec 7>&-; return 1; }; done
  echo "forceload add -80 -80 79 79" >&7
  local region=$dir/world/dimensions/minecraft/overworld/region ok=0
  for _ in $(seq 1 40); do
    sleep 5
    echo "save-all flush" >&7; sleep 2
    if [ -d "$region" ] && ./target/release/examples/compare "$region" 0 0 3 "$seed" 2>/dev/null | grep -q "chunks=49 "; then ok=1; break; fi
  done
  echo "stop" >&7; exec 7>&-
  wait $pid 2>/dev/null
  [ $ok = 1 ] && touch "$dir/done-26.2"
  [ $ok = 1 ]
}

T0=$(date +%s)
cases=0; bad=0; errors=0; evaluated=0
for seed in "${SEEDS[@]}"; do
  if ! gen_vanilla_world "$seed" > "$OUT/parity-gen-$seed.log" 2>&1; then
    echo "note: seed $seed: vanilla world generation failed: $(tail -1 "$OUT/parity-gen-$seed.log")"
    errors=$((errors+1)); continue
  fi
  region=vanilla/parity/$seed/world/dimensions/minecraft/overworld/region
  for site in "${SITES[@]}"; do
    set -- $site
    cases=$((cases+1))
    res=$(./target/release/examples/compare "$region" "$1" "$2" "$3" "$seed" 2> "$OUT/parity-$seed.err" | grep '^RESULT' | tail -1)
    verdict=$(python3 - "$res" <<'EOF'
import sys
parts = sys.argv[1].split()
kv = dict(p.split("=", 1) for p in parts[1:]) if len(parts) > 1 else {}
th = {"biomes": 99.9, "shape": 99.0, "exact": 98.5, "features": 98.5}
bad = [f"{k} {kv.get(k)} < {v}" for k, v in th.items() if float(kv.get(k, 0)) < v]
want = None
print(("1" if not bad and int(kv.get("chunks", 0)) > 0 else "0") + " chunks=" + kv.get("chunks", "0") + " " + " ".join(f"{k}={kv.get(k)}" for k in th) + (" | " + "; ".join(bad) if bad else ""))
EOF
)
    ok=${verdict%% *}
    evaluated=$((evaluated+1))
    [ "$ok" = 1 ] || bad=$((bad+1))
    echo "PARITY seed=$seed site=$1,$2,$3 ${verdict#* } ok=$ok"
  done
done
echo "RESULT fuzz=worldgen cases=$cases mismatches=$bad new=$bad info=0 errors=$errors skipped=$([ $evaluated -eq 0 ] && echo 1 || echo 0) seeds=${#SEEDS[@]} secs=$(( $(date +%s) - T0 ))"
[ $bad -eq 0 ] && [ $errors -eq 0 ]
