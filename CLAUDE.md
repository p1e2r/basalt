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
  two-client smoke test. Generation runs on a worker pool (gen.rs); `--flat` for superflat.
- crates/worldgen: **terrain done, features partial.** Noise router/density functions, aquifers,
  ore veins, multi-noise biomes, surface rules, cave/canyon carvers, ores + disks + freeze.
  vs vanilla seed 12345 (15x15 chunks at two sites): biomes 100.000%, shape 99.6-99.7%, exact
  terrain blocks 99.3-99.45%. Missing: trees/vegetation, lakes, springs, geodes, monster rooms,
  structures, nether/end. Compare: `cargo run -p basalt-worldgen --release --example compare --
  vanilla/srv/world/dimensions/minecraft/overworld/region <cx> <cz> <radius>`.
  Build the server with --release (worldgen ~40 ms/chunk in release).

### Task list
- [x] M1 client builds, joins vanilla, renders chunks
- [x] M2 crates/world: sections/paletted containers, light, Anvil region read/write
- [x] M2 crates/server: login/config/play, chunk streaming, movement, break/place, chat, multiplayer
- [x] M2 verify with real 26.2 client (tools/run-vanilla-client.py) + basalt-client
- [x] M3a worldgen terrain/biomes/surface/carvers/ores vs vanilla seed 12345, wired into server
- [ ] M3b features: trees + vegetation, lakes, springs, geodes, monster rooms; exact neighbour mode
- [ ] M3c structures (overworld; may move into M8)
- [~] M4 survival gameplay — implemented & bot-tested (crates/game rules; server game/{items,inv,health}.rs):
      hardness/tool dig times (server validates), loot-table drops as item entities + pickup/merge/
      despawn, Q-drop, all container_click modes, 2x2 + crafting table crafting, armor slots,
      health/hunger/saturation/regen/starve, fall damage, eating, death (drops inventory) + respawn.
      Tool durability (damage component, persisted). Missing: armor damage reduction (needs M5 combat), non-food use_item (buckets etc.), death
      screen in basalt-client (auto-respawns), human-driven real-client inventory test.
- [ ] M5 mobs   - [ ] M6 world logic   - [ ] M7 client parity
- [ ] M8 Nether/End, portals, structures
- [ ] M9 redstone  - [ ] M10 enchant/anvil/brewing/smithing/XP  - [ ] M11 villagers/raids/AI
- [ ] M12 bosses/endgame  - [ ] M13 commands/datapacks/advancements/scoreboards
- [ ] M14 client polish  - [ ] M15 online-mode/encryption/chat signing
- [ ] M16 lossless vanilla worlds + admin files  - [ ] M17 performance  - [ ] M18 tests/CI/docs/plugins
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
4. Survival gameplay: block hardness + tool tiers/speeds, drops (loot tables), item entities +
   pickup, inventory (container_click), crafting from vanilla recipe JSON (2x2 + table),
   health/hunger/saturation/fall damage, death + respawn.
5. Mobs: spawning rules, AI (pathfinding, goals), combat (melee/ranged, knockback, armor),
   loot tables.
6. World logic: fluids, falling blocks, day/night + weather, random ticks/growth,
   furnaces/chests and other block entities.
7. Client parity: inventory/crafting UI, sounds, entity models, menus (title, server list,
   pause, settings).
8. Nether/End: dimensions, portals, structures.
9. Redstone: vanilla update order, pistons, hoppers, observers, rails.
10. Enchanting, anvil, brewing, smithing, XP.
11. Villagers + trading, raids, full mob AI.
12. Bosses/endgame: ender dragon, wither, warden, elytra, end cities, 26.x features.
13. Full commands + datapacks/functions, advancements, scoreboards.
14. Client polish: all screens, particles, animations, lighting/AO, sky, sounds/music,
    resource packs, keybinds.
15. Online-mode auth, encryption, chat signing.
16. Lossless vanilla world compatibility + server admin files (server.properties, ops,
    whitelist, bans).
17. Performance: multithreading, lighting engine, LOD.
18. Parity test suite, CI builds for macOS/Windows/Linux, docs, plugin API.
