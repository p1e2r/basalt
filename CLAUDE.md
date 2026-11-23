# Basalt — handoff notes for Claude Code

Clean-room Rust reimplementation of Minecraft Java 26.2 (protocol 776). MIT licensed.
Never translate decompiled Mojang code; match behaviour by observing a real server/client
over the wire. Never commit Mojang data/assets (data/, vanilla/, *.jar are gitignored).

Read NOTES.md for verified packet formats.

## Status (updated 2026-11-23)
- Done & working: crates/nbt, crates/data (tables generated from ./data at build time),
  crates/protocol (framing, compression, varints, NBT, text), crates/probe (joins a server and
  dumps packets; `basalt-probe proxy LISTEN TARGET DUMPDIR` logs both directions).
- crates/client: **milestone 1 done.** Joins vanilla 26.2 on 25566, renders chunks (meshing
  workers, frustum culling, fog, sky, HUD), walks with vanilla-accepted physics, chat,
  creative/survival break, place, hotbar. Textures come from vanilla/client.jar (`--jar`).
  Headless test mode: `basalt-client --screenshot out.png --wait 10 [--cmd 'tp @s 0 100 0']
  [--chat msg] [--look yaw,pitch] [--walk secs] [--click l|r]`; env BASALT_NOFOG / BASALT_NOCULL
  / BASALT_DIAG for debugging.
- crates/world: **done.** Paletted containers, chunk network encode, heightmaps, sky/block light
  (BFS, incremental, matches vanilla's stored light exactly on real chunks), Anvil region
  read/write (vanilla 26.2 worlds keep regions in world/dimensions/minecraft/overworld/region).
  Block light/opacity props are an approximate table (props.rs).
- crates/server: **milestone 2 done.** `basalt-server [--port 25565] [--world DIR | --memory]
  [--gamemode creative] [--view N] [--compression N]`. Status ping, offline login, configuration
  (29 synced registries + 15 tag registries built from ./data), join sequence, chunk streaming
  with batch acks, movement, break/place (orientation for logs/stairs/slabs/doors/torches…),
  chat, commands (/gamemode /tp /give /time /say /list /spawn /save), tab list, player
  entities, keepalive, world + player persistence. Flat generator until worldgen is wired in.
  Verified: real 26.2 client joins, renders, sees a second (basalt-client) player, chat,
  placed blocks; restart keeps blocks/inventory/position. `tools/server-smoke.sh` runs the
  two-client smoke test.
- Not started: crates/worldgen (subagent working on it in a worktree).

### Task list
- [x] M1 client builds, joins vanilla, renders chunks
- [x] M2 crates/world: sections/paletted containers, light, Anvil region read/write
- [x] M2 crates/server: login/config/play, chunk streaming, movement, break/place, chat, multiplayer
- [x] M2 verify with real 26.2 client (tools/run-vanilla-client.py) + basalt-client
- [ ] M3 crates/worldgen vs vanilla seed 12345, then plug into server (replace gen::Flat)
- Server gaps: survival drops go straight to inventory (no item entities), no container_click /
  crafting, no mobs, no fluids/redstone ticking, no online-mode auth. Real-client walking/
  breaking driven by a human not yet verified (only bot-driven).
- Known client gaps: no entity models (boxes only), survival dig time not hardness-based,
  no inventory screen, no sounds, edge chunks without loaded neighbours are not meshed.

## Local setup (macOS)
1. Java 25 + Rust. `./tools/extract-data.sh` (downloads the official server jar, runs its data
   generator into ./data). Required before any build.
2. Test server: vanilla/srv/run.sh (online-mode=false, network-compression-threshold=-1,
   level-seed=12345, server-port=25566). Console via `echo cmd > vanilla/srv/console.fifo`.
   Client jar for textures: vanilla/client.jar (from Mojang's version manifest).
3. Test client: the user's normal launcher with 26.2, joining localhost.
   (NOTES.md's /home/claude paths refer to the old cloud machine.)

## Plan
1. Fix client build; get it joining vanilla on 25566 and rendering chunks.
2. crates/world + crates/server: real 26.2 client can join, walk, break/place, chat, multiplayer.
3. crates/worldgen: data-driven noise router, multi-noise biomes, surface rules, carvers,
   features; compare to vanilla seed 12345 chunk-by-chunk. API:
   `Generator::new(seed).generate_chunk(cx, cz) -> GeneratedChunk { blocks: Vec<[u16;4096]>
   (24 sections, y<<8|z<<4|x), biomes: Vec<[u8;64]> (y<<4|z<<2|x) }`.
