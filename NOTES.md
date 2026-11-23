# Basalt — clean-room Rust reimplementation of Minecraft Java 26.2 (protocol 776)
No Mojang code is translated or committed. Game data comes from the official data
generator (tools/extract-data.sh → ./data). Behaviour is matched by observing a
real server/client over the wire.

## Test harness (this machine)
- Vanilla 26.2 server: vanilla/srv (run.sh), port 25566, offline, compression off.
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
- login_finished: uuid, str name, varint nProps (0), then 16 more bytes (per-connection id, UUID-shaped; the real
  client hangs without them; random v4 accepted).
- Serverbound play (accepted by vanilla 26.2, verified with basalt-client):
  accept_teleportation varint id; move_player_pos f64 xyz + u8 flags (1 onGround, 2 hCollision);
  move_player_pos_rot f64 xyz f32 yaw pitch u8 flags; move_player_rot f32 f32 u8; status_only u8;
  player_loaded (empty, after first teleport); client_tick_end (empty, every tick);
  player_input u8 (1 fwd 2 back 4 left 8 right 16 jump 32 sneak 64 sprint);
  chat_command str; chat: str msg, i64 ts, i64 salt, bool hasSig(false), varint offset,
  3-byte ack bitset, u8 checksum (1 accepted when unsigned);
  player_action varint status, position, u8 face, varint seq (creative: status 0 alone breaks);
  use_item_on varint hand, position, varint face, f32 cursor xyz, bool inside, bool worldBorderHit, varint seq;
  swing varint hand; set_carried_item i16.
- Vanilla console: vanilla/srv/run.sh pipes stdin from vanilla/srv/console.fifo
  (`echo "op Basalt" > vanilla/srv/console.fifo`).
- Real client harness (macOS, uses the launcher's installed 26.2): `tools/run-vanilla-client.py HOST:PORT [NAME]`
  (offline, own game dir vanilla/mcclient, quick-play join; log in vanilla/mcclient/logs/latest.log).
  `swift tools/mc-window-shot.swift out.png` captures only the Minecraft window.
  Proxy it through `basalt-probe proxy 127.0.0.1:25570 127.0.0.1:<server> DIR` to record both directions.
- Entity data (set_entity_data: varint eid, then {u8 index, varint serializer, value}..., 0xff):
  0 shared flags (byte, ser 0: 0x01 on fire, 0x02 crouch, 0x08 sprint), 1 air supply (varint, ser 1;
  vanilla lets it reach -20 then deals drown damage and resets to 0; refills +4/tick), 7 ticks frozen
  (varint), 8 item entity stack (ser 7, slot format), 9 living health (float, ser 3).
- update_mob_effect: varint eid, varint effect (MOB_EFFECT registry), varint amplifier, varint duration
  ticks, u8 flags (0x0e = particles+icon+blend). remove_mob_effect: varint eid, varint effect.
- Item stack components on the wire: damage (type 3) = varint; enchantments (type 13) = varint n,
  {varint enchantment id (synced enchantment registry order), varint level}.
- basalt-probe proxy does not handle compression: run the Basalt side with --compression -1 when proxying.
- Window-only video of the real client: `swift tools/mc-window-bounds.swift` → crop the avfoundation
  screen capture to those bounds (×2 on Retina) inside the ffmpeg filter so only the window is saved.
- LpVec3 (velocity in add_entity / set_entity_motion): single 0 byte if all |v| < 2^-15; else
  scale = ceil(max |v|), bits = (scale&3) | (scale>3)<<2 | x15<<3 | y15<<18 | z15<<33 where
  c15 = round((c/scale*0.5+0.5)*32766); written as u8, u8, u32 BE (low 48 bits), + varint(scale>>2)
  if scale>3. (Round-trips vanilla bytes; see protocol buf.rs test.)
- Vanilla item tracking (measured): add_entity carries the initial velocity (block drop: (±0.1, 0.2,
  ±0.1)); then move_entity_pos (i16 deltas of the last sent position ×4096, + bool onGround) and
  set_entity_motion every 20 ticks; on landing / lift-off an absolute entity_position_sync (+ motion
  first) — a delta there makes the real client snap/flicker; the first tick after spawn re-sends a
  zero move_entity_pos + the stack. Resting items only run their move step every 4th tick (so their
  velocity reads ~(0,-0.08,0) at the 20-tick update, then 0).
  Items: gravity 0.04 before moving, drag 0.98, ground friction slipperiness×0.98, bounce vy×-0.5.
- Mobs (measured on vanilla 26.2 via basalt-probe proxy, M5):
  - Spawn is a bundle: bundle_delimiter, add_entity, set_entity_data, update_attributes,
    [set_equipment], bundle_delimiter. add_entity = varint eid, uuid, varint type, f64 xyz, LpVec3 vel,
    u8 pitch, u8 yaw, u8 headYaw, varint data (0 for mobs; arrows: the SHOOTER'S entity id, not id+1).
  - Mob metadata: 9 health (float); 6 pose (ser 20; 7 = DYING, sent with health 0 on death);
    8 living flags (byte; 1 = using item, skeleton drawing its bow); 15 mob flags (byte: 1 NoAI,
    2 left-handed, 4 aggressive — zombie arms up / skeleton aiming); 16 baby (boolean ser 8) for
    ageable mobs and zombies; creeper 16 swell state (varint ser 1, -1 idle / 1 swelling),
    17 powered (bool), 18 ignited (bool); sheep 18 wool byte (low 4 bits colour, 0x10 sheared);
    spider 16 climbing (byte 1); chicken 19 variant (ser 31, only sent when not default);
    arrow 8 flags byte (1 crit), 10 in ground (bool). Vanilla sends only non-default values.
  - update_attributes: varint eid, varint n, {varint attribute id, f64 base, varint nmods,
    {str id, f64 amount, u8 op}}; vanilla syncs only movement_speed for these mobs (zombie 0.23,
    skeleton/creeper/pig/chicken 0.25, spider 0.3, cow 0.2, sheep 0.23; baby zombie carries
    "minecraft:baby" +0.5 op 1).
  - set_equipment: varint eid, then {u8 slot (|0x80 if more follow), slot item}; skeleton: slot 0 bow.
  - Movement: move_entity_pos(_rot) about every tick while moving (i16 deltas ×4096 + bool onGround),
    rotate_head (varint eid, u8), set_entity_motion (varint eid, LpVec3), entity_position_sync
    (varint eid, f64 xyz, f64 vel xyz, f32 yaw pitch, bool onGround) occasionally.
  - Damage: damage_event = varint eid, varint damage_type id, varint cause eid+1 (0 none), varint
    direct eid+1, bool hasPos (+ f64 xyz). The hurt player alone also gets hurt_animation (varint eid,
    f32 yaw). Hurt/death/ambient/step sounds are server-sent `sound` packets: varint sound id+1,
    varint category (5 hostile, 6 neutral, 7 player), i32 x*8 y*8 z*8, f32 vol, f32 pitch, i64 seed.
  - Death: set_entity_data health 0 + pose 7, entity_event 3, death sound, loot dropped at once;
    ~20 ticks later entity_event 60 (poof) then remove_entities.
  - Knockback of a player by a mob: set_entity_motion with the player's own id.
  - explode: f64 x y z, f32 radius, i32 block count (all positions the rays touched, incl. air:
    282-598 for r 3-4), Optional<f64×3> player knockback (bool + 3 doubles, only to the knocked
    player), varint particle (explosion_emitter), sound holder varint (id+1 of
    entity.generic.explode), varint n block-particle entries {varint particle, f32 scaling, f32 speed,
    varint weight} (vanilla: poof 0.5/1.0/1, smoke 1.0/1.0/1). Destroyed blocks go out separately as
    section_blocks_update / block_update.
  - Serverbound (minecraft.wiki protocol page, 26.x): attack = varint entity id; interact = varint
    entity id, varint hand, LpVec3 target offset, bool sneaking.
  - Reference values (vanilla, difficulty=easy, unarmored player): zombie hit 2.5; skeleton arrow from
    10 blocks 3.0; creeper (r 3, Fuse:0) 4.36 blocks from the player's feet → 5.157 damage and
    knockback magnitude 0.274 (impact uses distance to feet, push direction aims at the eyes).
