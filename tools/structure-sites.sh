#!/usr/bin/env bash
# Generate (once) one vanilla reference world with several small force-loaded sites (official server
# jar), cached in vanilla/structures/<seed>_<name> (marker done-26.2, gitignored). Used for structures
# that are far apart (temples, mineshaft variants): one server run instead of one per site.
# usage: tools/structure-sites.sh SEED NAME R CX,CZ [CX,CZ ...]
#   each site = chunks CX-R..CX+R-1 x CZ-R..CZ+R-1 (like tools/structure-world.sh); env JAVA,
#   PARITY_PORT_BASE (25700..25799), XMX (default 3G)
set -uo pipefail
cd "$(dirname "$0")/.."
seed=$1; name=$2; R=$3; shift 3
sites=("$@")
dir=vanilla/structures/${seed}_$name
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
generate-structures=true
spawn-protection=0
view-distance=2
simulation-distance=2
max-tick-time=-1
pause-when-empty-seconds=-1
network-compression-threshold=-1
sync-chunk-writes=false
P
rm -f "$dir/console.fifo"; mkfifo "$dir/console.fifo"
( cd "$dir" && exec "${JAVA:-java}" -Xmx${XMX:-3G} -jar ../../server.jar nogui < console.fifo > server.log 2>&1 ) &
pid=$!
echo "vanilla structure sites: pid $pid port $port"
exec 7> "$dir/console.fifo"
t=0
until grep -q "Done (" "$dir/server.log" 2>/dev/null; do sleep 1; t=$((t+1)); [ $t -gt 300 ] && { kill $pid; exit 1; }; done
for s in "${sites[@]}"; do
  cx=${s%,*}; cz=${s#*,}
  echo "forceload add $(((cx-R)*16)) $(((cz-R)*16)) $(((cx+R-1)*16)) $(((cz+R-1)*16))" >&7
done
n=$(( (2*R)*(2*R) ))
ok=0
for _ in $(seq 1 120); do
  sleep 8
  echo "save-all flush" >&7
  sleep 3
  all=1
  for s in "${sites[@]}"; do
    if ! CENTER=$s ./target/release/examples/structures count "$dir/world/dimensions/minecraft/overworld/region" $R 2>/dev/null | grep -q "full=$n "; then all=0; break; fi
  done
  [ $all = 1 ] && { ok=1; break; }
done
echo "stop" >&7; exec 7>&-
wait $pid 2>/dev/null
[ $ok = 1 ] && touch "$dir/done-26.2"
[ $ok = 1 ]
