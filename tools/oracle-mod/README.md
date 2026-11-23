# Basalt oracle mod (TEST-ONLY, never shipped)

A tiny Fabric client mod that runs inside the **real, locally installed 26.2 client** and dumps
the vanilla client's *own outputs* for inputs Basalt chooses, as JSON. Basalt never uses the mod's
internals: it only **compares** its own results against these dumps (`basalt-check client-mesh`).

- Not built by cargo or CI; build it by hand with `tools/oracle-mod/build.sh`.
- The mod only *calls* the client's public API (discovered from `javap -p` signature listings and
  runtime behaviour). No Mojang method bodies were read, copied or ported; nothing in Basalt is
  derived from decompiled code.
- Everything it produces contains Mojang-derived geometry/texture names and stays under
  `vanilla/oracle/` (gitignored). Commit only this source, `SCHEMA.md` and the comparer code.

## Pinned components (all from maven.fabricmc.net)

26.2 is shipped **unobfuscated** and Fabric's intermediary for 26.2 is `0.0.0`, so no mappings and
no Loom/Gradle are needed: the mod is compiled with plain `javac` against the installed client jar.
Fabric API is not used (the mod needs only the loader's `ClientModInitializer` entrypoint).

| artifact | version | sha256 |
|---|---|---|
| net.fabricmc:fabric-loader | 0.20.1 | `d0c5a082e07410aaff9de7ab196aa6b4236002870f9ba3ad5c1260fcce5110ad` |
| net.fabricmc:sponge-mixin | 0.17.5+mixin.0.8.7 | `03f0065023bfc3045d71bfb99872451d9ad5a0800a5a0dac34ab8c7d5c134f5d` |
| org.ow2.asm:asm | 9.10.1 | `ed825d10ab1399c8c0cb669e688cf0c8c82629b4c8399b58352b68e92ca10fcb` |
| org.ow2.asm:asm-analysis | 9.10.1 | `dede75a21306b65974ecd8f87114ff6970f09fb794157a4ca09ab25c888c2bfc` |
| org.ow2.asm:asm-commons | 9.10.1 | `6d0abefb7cbf972ea16edb37ec14835372505063a45f976ab7ea889ed9497895` |
| org.ow2.asm:asm-tree | 9.10.1 | `3dfb0d5b6a106cd40b5b250e39935fbf2f927f4477546a5369a3ac609cf0506b` |
| org.ow2.asm:asm-util | 9.10.1 | `1bb99d091fba2597dc6d51193e9bbcf0d8447e7ed96bd8f0198b18152f09655c` |

The library list and checksums are the ones in Fabric's official launcher profile
(`https://meta.fabricmc.net/v2/versions/loader/26.2/0.20.1/profile/json`); the loader checksum is
the `.sha256` published next to the jar on maven.fabricmc.net. `build.sh` verifies every download.

## Use

```
tools/oracle-mod/build.sh                      # -> vanilla/oracle/basalt-oracle.jar (+ vanilla/oracle/fabric/*.jar)
tools/oracle-mod/launch.py &                   # vanilla client at the title screen, oracle watching vanilla/oracle/in
cargo build --release -p basalt-client -p basalt-check
target/release/basalt-check client-mesh        # launches the oracle itself if it is not running
```

`launch.py` uses the launcher's installed 26.2 (`~/Library/Application Support/minecraft`),
a separate game dir `vanilla/oracle/game`, no server join. The client must reach the title screen
(the mod writes `vanilla/oracle/out/_ready.json`). Vanilla responses are cached per request hash in
`vanilla/oracle/cache/`, so re-running the comparer after a Basalt fix needs no client.

## Adding a subsystem

One `Handler` class (`void handle(JsonObject request, JsonWriter out)`), one line in
`OracleMod.HANDLERS`, one section in `SCHEMA.md`, and a comparer under `crates/check/src/client/`
using `client::oracle::Oracle`.
