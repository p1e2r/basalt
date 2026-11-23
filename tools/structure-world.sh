#!/usr/bin/env bash
# Generate (once) a vanilla reference world for structure parity: chunks -R..R-1 square force-loaded
# with the official server jar, cached in vanilla/structures/<seed> (marker done-26.2, gitignored).
# usage: tools/structure-world.sh SEED [R=32] [CX CZ]   (square centred on chunk CX,CZ; cached in
#   vanilla/structures/<seed>_<cx>_<cz> then)     env: JAVA, PARITY_PORT_BASE (25700..25799),
#   NO_STRUCTURES=1: the same world with generate-structures=false (dir suffix _nostruct; reference for
#   measuring terrain adaptation, see NOTES.md "Terrain adaptation")
set -uo pipefail
cd "$(dirname "$0")/.."
seed=$1; R=${2:-32}; CX=${3:-0}; CZ=${4:-0}
dir=vanilla/structures/$seed
[ "$CX" != 0 ] || [ "$CZ" != 0 ] && dir=vanilla/structures/${seed}_${CX}_${CZ}
gs=true
[ -n "${NO_STRUCTURES:-}" ] && { dir=${dir}_nostruct; gs=false; }
[ -f "$dir/done-26.2" ] && exit 0
[ -f vanilla/server.jar ] || { echo "vanilla/server.jar missing"; exit 1; }
[ -x target/release/examples/structures ] || cargo build --release -q -p basalt-worldgen --example structures
port=$(python3 - "${PARITY_PORT_BASE:-25700}" <<'P'
import socket, sys
b = int(sys.argv[1])
for p in range(b + 20, b + 100):
    s = socket.socket()
    try:
        s.bind(("", p)); s.close(); print(p); break
    except OSError:
        s.close()
P
)
rm -rf "$dir"; mkdir -p "$dir"
echo "eula=true" > "$dir/eula.txt"
cat > "$dir/server.properties" <<P
level-seed=$seed
server-port=$port
online-mode=false
level-type=minecraft\:normal
generate-structures=$gs
spawn-protection=0
view-distance=2
simulation-distance=2
max-tick-time=-1
pause-when-empty-seconds=-1
network-compression-threshold=-1
sync-chunk-writes=false
P
rm -f "$dir/console.fifo"; mkfifo "$dir/console.fifo"
( cd "$dir" && exec "${JAVA:-java}" -Xmx6G -jar ../../server.jar nogui < console.fifo > server.log 2>&1 ) &
pid=$!
echo "vanilla structure world: pid $pid port $port"
exec 7> "$dir/console.fifo"
t=0
until grep -q "Done (" "$dir/server.log" 2>/dev/null; do sleep 1; t=$((t+1)); [ $t -gt 300 ] && { kill $pid; exit 1; }; done
for x in $(seq $((CX-R)) 16 $((CX+R-1))); do for z in $(seq $((CZ-R)) 16 $((CZ+R-1))); do
  x1=$((x+15)); z1=$((z+15)); [ $x1 -gt $((CX+R-1)) ] && x1=$((CX+R-1)); [ $z1 -gt $((CZ+R-1)) ] && z1=$((CZ+R-1))
  echo "forceload add $((x*16)) $((z*16)) $((x1*16)) $((z1*16))" >&7
done; done
n=$(( (2*R)*(2*R) ))
ok=0
for _ in $(seq 1 180); do
  sleep 10
  echo "save-all flush" >&7
  sleep 3
  if CENTER=$CX,$CZ ./target/release/examples/structures count "$dir/world/dimensions/minecraft/overworld/region" $R 2>/dev/null | grep -q "full=$n "; then ok=1; break; fi
done
echo "stop" >&7; exec 7>&-
wait $pid 2>/dev/null
[ $ok = 1 ] && touch "$dir/done-26.2"
[ $ok = 1 ]
