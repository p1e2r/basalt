# Basalt — clean-room Rust reimplementation of Minecraft Java 26.2 (protocol 776)
No Mojang code is translated or committed. Game data comes from the official data
generator (tools/extract-data.sh → ./data). Behaviour is matched by observing a
real server/client over the wire.

## Test harness (this machine)
- Vanilla 26.2 server: /home/claude/vanilla/srv, port 25566, offline, compression off.
- Real 26.2 client headless: /home/claude/mcclient/run.sh host:port [name] (Xvfb :99).
  Screenshot: DISPLAY=:99 import -window root shot.png
- Probe: target/debug/basalt-probe host:port secs dumpdir (joins, dumps clientbound).
- Proxy: basalt-probe proxy 127.0.0.1:25570 127.0.0.1:25566 /tmp/px (logs both directions).

## Verified wire formats (26.2)
- play/login: i32 eid, bool hardcore, varint n + n dim names, varint maxPlayers,
  varint viewDist, varint simDist, bool reducedDebug, bool respawnScreen,
  bool limitedCrafting, varint dimType, str dimName, i64 hashedSeed, u8 gamemode,
  i8 prevGamemode, bool debug, bool flat, bool hasDeathLoc, varint portalCooldown,
  varint seaLevel, bool enforcesSecureChat, + one trailing 0x00 byte (unknown, send 0).
- player_position: varint tpId, f64 x y z, f64 vx vy vz, f32 yaw pitch, i32 flags.
- game_event: u8 event, f32 value (13 = start waiting for chunks).
- set_chunk_cache_center: varint cx, varint cz.
- set_default_spawn_position: str dim, position, f32 yaw, f32 pitch.
- set_time: i64 gameTime, varint n clocks, each {varint clockId, varlong ticks, f32 partial, f32 rate}.
- player_abilities: u8 flags, f32 flySpeed(0.05), f32 fovMod(0.1).
- set_health: f32 hp, varint food, f32 saturation. ticking_state: f32 tickrate, bool frozen.
- player_info_update: u8 action bitset (0xff = all 8), varint n, each: uuid,
  [add] str name + varint props, [chat] bool, [gm] varint, [listed] bool,
  [latency] varint, [display] bool, [order] varint, [hat] bool.
- level_chunk_with_light: i32 x, i32 z, varint nHeightmaps {varint type, varint nLongs, longs}
  (vanilla sends types 5,4,1 with 37 longs), varint dataSize, 24 sections each
  {i16 nonAirBlocks, i16 fluidCount, blocks container, biomes container},
  varint nBlockEntities {u8 packedXZ, i16 y, varint type, nbt},
  light: 4 bitsets (sky, block, emptySky, emptyBlock; 26 bits), varint n sky arrays
  {varint 2048 + bytes}, varint n block arrays.
- Paletted container: u8 bpe; bpe 0 → varint single value; indirect → varint n + ids;
  then longs with NO length prefix (count computed). Blocks: indirect 4..8 (min 4), direct 15.
  Biomes: indirect 1..3, direct otherwise.
- Config: server sends custom_payload brand, update_enabled_features [minecraft:vanilla],
  select_known_packs [minecraft:core 26.2]; then registry_data per registry with entries
  (name, hasData=false) — entry order = sorted file names in data/data/minecraft/<reg>/.
  Registry send order: see crates/server (copied from vanilla capture). Then update_tags
  for: instrument timeline potion damage_type item worldgen/biome fluid banner_pattern dialog
  game_event painting_variant enchantment entity_type point_of_interest_type block.
- Vanilla waits for chunk_batch_received before sending more chunks.
