#!/usr/bin/env bash
# Build the TEST-ONLY oracle mod (never shipped; not part of cargo or CI).
# Downloads pinned Fabric Loader components from maven.fabricmc.net only (sha256-verified),
# compiles against the locally installed 26.2 client jar with plain javac (26.2 is unobfuscated,
# Fabric intermediary for 26.2 is 0.0.0 = no remapping, so no Loom/Gradle is needed), and writes
# vanilla/oracle/basalt-oracle.jar. Everything under vanilla/ is gitignored.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
oracle="${ORACLE_DIR:-$repo/vanilla/oracle}"; mkdir -p "$oracle"; oracle="$(cd "$oracle" && pwd)"
fab="$oracle/fabric"
mkdir -p "$fab" "$oracle/build"

M=https://maven.fabricmc.net
# path  sha256   (pinned; see README.md)
libs=(
  "net/fabricmc/fabric-loader/0.20.1/fabric-loader-0.20.1.jar d0c5a082e07410aaff9de7ab196aa6b4236002870f9ba3ad5c1260fcce5110ad"
  "net/fabricmc/sponge-mixin/0.17.5+mixin.0.8.7/sponge-mixin-0.17.5+mixin.0.8.7.jar 03f0065023bfc3045d71bfb99872451d9ad5a0800a5a0dac34ab8c7d5c134f5d"
  "org/ow2/asm/asm/9.10.1/asm-9.10.1.jar ed825d10ab1399c8c0cb669e688cf0c8c82629b4c8399b58352b68e92ca10fcb"
  "org/ow2/asm/asm-analysis/9.10.1/asm-analysis-9.10.1.jar dede75a21306b65974ecd8f87114ff6970f09fb794157a4ca09ab25c888c2bfc"
  "org/ow2/asm/asm-commons/9.10.1/asm-commons-9.10.1.jar 6d0abefb7cbf972ea16edb37ec14835372505063a45f976ab7ea889ed9497895"
  "org/ow2/asm/asm-tree/9.10.1/asm-tree-9.10.1.jar 3dfb0d5b6a106cd40b5b250e39935fbf2f927f4477546a5369a3ac609cf0506b"
  "org/ow2/asm/asm-util/9.10.1/asm-util-9.10.1.jar 1bb99d091fba2597dc6d51193e9bbcf0d8447e7ed96bd8f0198b18152f09655c"
)
for l in "${libs[@]}"; do
  path="${l% *}"; sum="${l#* }"; f="$fab/$(basename "$path")"
  if [ ! -f "$f" ] || [ "$(shasum -a 256 "$f" | cut -d' ' -f1)" != "$sum" ]; then
    echo "download $M/$path"
    curl -fsSL -o "$f.part" "$M/$path"
    got="$(shasum -a 256 "$f.part" | cut -d' ' -f1)"
    if [ "$got" != "$sum" ]; then echo "sha256 mismatch for $path: $got != $sum" >&2; rm -f "$f.part"; exit 1; fi
    mv "$f.part" "$f"
  fi
done

cp_vanilla="$(python3 "$here/launch.py" --classpath)"
cp_all="$cp_vanilla:$fab/fabric-loader-0.20.1.jar:$fab/sponge-mixin-0.17.5+mixin.0.8.7.jar"
rm -rf "$oracle/build/classes"; mkdir -p "$oracle/build/classes"
javac --release 25 -nowarn -d "$oracle/build/classes" -cp "$cp_all" $(find "$here/src" -name '*.java')
cp "$here/fabric.mod.json" "$here/basalt-oracle.mixins.json" "$oracle/build/classes/"
(cd "$oracle/build/classes" && jar cf "$oracle/basalt-oracle.jar" .)
echo "built $oracle/basalt-oracle.jar"
