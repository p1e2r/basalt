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

## Vanilla client block meshing (measured with tools/oracle-mod + basalt-check client-mesh)
- Smooth lighting: face plane = neighbour layer if the face lies on the block boundary, else the
  block's own layer. Each corner of the unit face square averages 4 cells (centre, 2 sides,
  diagonal): AO factor = mean of 0.2 (full-cube collidable non-glass blocks incl. leaves) / 1.0;
  the centre cell counts too, no "both sides block the corner" rule. Sky = mean with sky-0 cells
  taking the centre's sky; block = mean with fully dark cells (0/0) taking the centre's value.
  Vertices interpolate the 4 corners bilinearly by position. Emitting blocks and models with
  "ambientocclusion": false are flat-lit (light of the cull-face neighbour, else own cell), block
  light = max(light, emission). Magma and active sculk sensors are drawn at block light 15.
- Directional shade 0.5/1.0/0.8/0.8/0.6/0.6 (down/up/N/S/W/E) baked into the vertex colour.
- Render layer is per quad from the alpha of the sprite *region* the face samples: any partial alpha
  -> translucent, any zero alpha -> cutout, else solid; model textures declared
  {"sprite", "force_translucent": true} are translucent; glass / glass_pane_top are translucent.
- Face UVs are exact (no inset). uvlock under a pure y rotation keeps authored side-face UVs.
- A face is culled when this block's whole solid boundary face on that side is covered by the
  neighbour's solid face (slabs/stairs cull; beds/signs/banners/heads never occlude).
- Redstone dust tint by power 0..15: 4C0000 700000 7A0000 840000 8E0000 990000 A30000 AD0000
  B70000 C10000 CC0000 D60000 E00000 EA0600 F41B00 FF3200. Stems: age a -> (32a, 255-8a, 4a).
- Emission: glow lichen 7 only with a face; trial spawner 4 (waiting) / 8 (active, ejecting) / 0;
  vault 6 inactive / 12 otherwise.

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
- basalt-client test hooks (M7; stable CLI + output, see crates/client/src/{scenario,assetgrid,bench}.rs):
  - Scenarios: `basalt-client --addr H:P [--vanilla H:P] --scenario all|name[,name]` → one line each
    `SCENARIO M7/<name> PASS|FAIL|SKIP <details>`, exit 1 on any FAIL. Names: edge_chunks inventory
    crafting_table death dig dig_others(vanilla) models interp sounds menus. Shots in $BASALT_SCENARIO_SHOTS
    (default target/scenario-shots). Wrapper: tools/client-scenarios.sh (fresh in-memory Basalt server).
  - Visual parity: `--screenshot out.png --size WxH [--options vanilla/mcclient/options.txt] [--fov F]
    [--render-distance N] [--gui-scale G] [--time T] [--no-gui] [--pos x,y,z] [--look yaw,pitch] [--cmd ...]`;
    waits until no chunk arrived for 1.5 s and all sections are meshed (max 30 s). options.txt fov is
    normalised (fov = 70 + v*40). --no-gui = F1 (no HUD, no hand).
  - Asset coverage: `--asset-grid out.png [--blocks|--items] [--allowlist tools/asset-allowlist.txt]` →
    out.png (+ out_N.png pages), out.json {kind, cells, pages, total, problems[], allowlisted[]}; exit 1 if
    problems is non-empty. No server needed.
  - Benchmark: `--bench out.json --addr H:P --render-distance N --seconds S [--bench-mode rotate|fly]
    [--size WxH]` → JSON: fps_avg, fps_1pct_low, frame_ms{avg,p50,p90,p99,max}, mesh_ms_per_section{avg,p95},
    sections_meshed, sections_resident, chunks_loaded, chunk_visible_latency_ms{count,avg,p95,max},
    memory{rss_kb, gpu_section_buffers_kb}. The server must allow the view distance (Basalt: --view 32).
  - Real client handshake order (captured through the proxy): brand custom_payload, then client_information
    ("en_us", view, chat 0, colors 1, skin parts 0x7f, main hand 1, filtering 0, listing 1, particles 0).
    The launcher-run real client sends an all-zero UUID in hello in offline mode.

## Test harness findings (crates/check, measured against vanilla 26.2 with the bot)
- Command feedback is system_chat with a translatable component: `data get entity @s Health` →
  `commands.data.entity.query` with [displayName, value] where value is a styled text tree whose
  flattened text is SNBT (`20.0f`, `300s`, `[0.5d, 201.0d, 0.5d]`,
  `[{count: 4, Slot: 0b, id: "minecraft:dirt"}]`, components as
  `components: {"minecraft:damage": 2, "minecraft:enchantments": {"minecraft:respiration": 3}}`).
  Missing paths reply `arguments.nbtpath.nothing_found(<path>)` wrapped as `{"text":"","extra":[{translate..}],"color":"red"}`
  (TicksFrozen and active_effects are absent when zero/empty). `execute if block x y z <state>` →
  `commands.execute.conditional.pass` / `.fail`; `execute if entity @e[...]` →
  `commands.execute.conditional.pass_count(N)`. Other ops see `chat.type.admin` echoes of your commands.
  `/gamemode X` when already X prints nothing. `/clear` with an empty inventory → `clear.failed.single`.
- Player NBT paths: Health (f), foodLevel, foodSaturationLevel (f), foodExhaustionLevel (f), Air (s),
  TicksFrozen, Fire (s; -20 when not burning), playerGameType, Pos, Inventory (Slot 0-8 hotbar,
  9-35 main), equipment {head, chest, legs, feet, offhand}, active_effects [{id, amplifier, duration}].
- `/tp @s X Y Z` with integer X/Z centres the player on the block (+0.5); send explicit decimals.
- Console `op <name>` in offline mode lower-cases names it can't resolve (the op is then for a
  different offline UUID): use lower-case bot names. The console is shared; match `Made <name> a server operator`.
- Q / ctrl-Q (player_action 4 / 3): vanilla does NOT resend the slot — the client predicts the removal.
- set_health is sent only when health/food change: saturation drops from exhaustion are invisible to
  the client until then (query the server).
- A block at head height only (feet free) makes vanilla switch the player to the crawling pose: no
  suffocation. Fully enclosed (feet + head) → in_wall 1.0 every 10 ticks.
- Players start with Fire -20: standing in a fire block ignites after ~20 ticks (in_fire 1.0 per hit
  meanwhile); burning lasts ~8 s after leaving (on_fire 1.0 per 20 ticks). Rain on the player puts fires
  out each tick — fire/lava tests need a roof (the vanilla test server may be raining).
- Sprinting: 0.1 exhaustion per metre (47.8 blocks → saturation 5 → 4, exhaustion 0.74 left).
- Drowning: air 300 → -20 at 1/tick, 2.0 drown damage at tick ~320 then every 20 ticks; refill +4/tick.
- Summon NBT: any compound (`{PersistenceRequired:1b}`) stops vanilla's random equipment/baby rolls.
- Entity rotation bytes: yaw is an unsigned-wrapping byte (256 = 360°, interpolate via shortest angle);
  **pitch is a signed byte** (-128..127) clamped to ±90° — read unsigned, a mob looking slightly up
  arrives as ~337° and its head lerps through a backflip (client: net::pitch_angle).
- `commands` (clientbound): varint node count; node = u8 flags (bits 0-1 type: 0 root/1 literal/2
  argument; 0x04 executable; 0x08 redirect; 0x10 custom suggestions; 0x20 restricted), varint child
  count + indices, [varint redirect], [string name (literal/argument)], [argument: varint parser id
  (COMMAND_ARGUMENT_TYPE), properties, (0x10) string suggestion type]; then varint root index.
  Parser properties: integer/long/float/double = u8 flags (1 min, 2 max) + values; string = varint
  (0 word, 1 phrase, 2 greedy); entity = u8 (1 single, 2 players only); score_holder = u8 (1 multiple);
  time = i32 min; resource/resource_key/resource_or_tag(_key)/resource_selector = string registry.
  Vanilla suggestion types seen: minecraft:ask_server, summonable_entities, available_sounds. The
  restricted flag equals the presence of `permissions` in reports/commands.json. Basalt builds its
  tree from that file (crates/server/src/commands.rs); tools/decode-commands.py decodes a captured
  packet and tools/compare-commands.py diffs two (0 differences over the 15 shared commands).
- command_suggestion (serverbound): varint id, string text (with the leading slash).
  command_suggestions (clientbound): varint id, varint start, varint length, varint count, then
  {string text, bool hasTooltip (+ component)}.
- First-person arm (empty hand), fitted to the real client: BASALT_ARM="tx,ty,tz,ry,rx,rz,scale"
  overrides it (default -0.45,-0.8,0.4,-0.3,-2.3,0,1); the arm points up/forward from the lower
  right. Reference silhouette: crates/client/refs/first_person_arm.txt.
- Observed oddity (not fixed): the real client renders a night sky on Basalt even at day_time 6000.
- **Resolved (was "dim daytime sky"):** login and respawn carry an `isFlat` boolean (after the debug flag). Vanilla clients
  use the world's min build height as the horizon for flat worlds and sea level (63) otherwise; a player below the horizon gets
  a darkened sky with a black horizon band. Basalt always sent isFlat=false, so a `--flat` world (players at y=-59) looked
  dusky/black in the real client. Now `--flat` sends isFlat=true. Generated worlds were never affected (noon sky matched
  vanilla). Ruled out along the way: registry order, update_tags, set_time layout, weather events, login fields.
