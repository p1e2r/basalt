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
- Fluids (water/lava, also waterlogged blocks), measured with the oracle:
  - Heights: source and falling (level >= 8) 8/9, flowing level l (1..7) (8-l)/9. A corner height is the
    average over the 4 cells touching the corner (own, the two side cells, the diagonal): a fluid cell of
    height >= 0.8 weighs 10, a lower one weighs 1, an open non-fluid cell (air, plants) weighs 1 at height 0,
    a solid cell does not count; the diagonal counts only if one of the two side cells holds the fluid. Any
    of the four cells having the same fluid above it makes the corner 1.0. An isolated source is therefore
    (8/9*10)/12 = 0.7407, drawn at 0.7397: every corner < 1 is drawn 0.001 lower, full corners stay at 1.0.
  - Top: always drawn (also under stone) unless the cell above has the same fluid; double-sided = two quads
    with opposite winding; still sprite uv (u = x, v = z), or, when the flow vector (pushes away from higher
    neighbours / towards fluid below an open neighbour) is non-zero, the flow sprite using only its central
    half (0.25..0.75) rotated by atan2(flow) - 90 degrees (flow +z = unrotated).
  - Bottom: one quad (still sprite, u = x, v = z) at y = 0.001, shade 0.5; hidden by the same fluid below, an
    occluding block below, or the block's own shape covering the whole bottom (stairs, slabs, sculk sensors; NOT
    trapdoors, mangrove roots, leaves, glass, grates which never occlude). When hidden, side faces start at y = 0.
  - Sides: inset 0.001 (north z=0.001, south 0.999, west x=0.001, east 0.999), shade 0.8 NS / 0.6 WE, flow sprite
    at half scale (u 0..0.5, v = (1 - drawn height) * 0.5 at the top, 0.5 at the bottom), drawn double-sided.
    Culled by the same fluid, by an occluding neighbour, by a neighbour face (or the own block's shape) covering
    0..h. Water against a full non-opaque cube (glass, leaves) draws one single-sided water_overlay quad instead;
    lava has no overlay (plain flow quads).
  - Light is uniform per quad: top and sides = per-channel max(own cell, cell above), bottom = max(own cell, cell
    below); the block's own emission counts as block light of its cell, full-bright blocks (active sculk sensor,
    magma) give 15/15, lava block light 15. Tint = biome water colour (water) / white (lava) times the shade.
- Smooth light on inset faces (fitted with controlled light probes: random 5x5x5 light cells around one
  block, all else air; vertex light is truncated to 1/16): the *centre* cell is always the cell in front of the
  face, but the side and diagonal cells come from that same layer only for a face on the block boundary - for an
  inset face (slab top, cactus side, wall post, daylight detector, trapdoor) they are the block's own layer. If
  the centre cell is an opaque block (an inset face covered by a neighbour) the block's own cell stands in for
  it (also as the "dark cell" fallback value). Corner values combine bilinearly by vertex position over the whole
  block face as before. Ambient occlusion still samples one layer for all four cells (own layer when inset);
  that matches ~90 % of inset-face vertices, the rest is unexplained (a diagonal cell sometimes counts
  differently), see "what remains".
- Flat-lit faces (models with "ambientocclusion": false: levers, bars, brewing stands): the cell in front of the
  face when the face lies on the block boundary (whether or not the model declares a cullface), otherwise the
  block's own cell.
- Element rotation (all three axes, corrected): the standard right-handed rotation by "angle" degrees about the
  axis through "origin" (y: x' = x cos a + z sin a, z' = -x sin a + z cos a; x: y' = y cos a - z sin a,
  z' = y sin a + z cos a). Faces of zero-thickness elements (zero area) are not emitted.
- Model offset (position hash, measured at 100 random positions per block, all exact): h = (x*3129871 ^ z*116129781)
  as a 64-bit value, h = h*h*42317861 + h*11, h >>= 16 (arithmetic; y is not used). dx = clamp((bits0..3 / 15 - 0.5) * 0.5,
  +-max), dz = same with bits 8..11, dy = (bits 4..7 / 15 - 1) * v. Blocks: +-0.25 flowers, tall grass/ferns (dy range
  0.2: short_grass, fern, short/tall_dry_grass; large_fern and tall_grass have none), bamboo, propagule, roots, nether
  sprouts, pitcher plant, eyeblossoms, torchflower, wither rose; small dripleaf +-0.25 with dy range 0.1;
  pointed_dripstone and sulfur_spike +-0.125. (crates/client/src/assets.rs `model_offset_params`.)
- Weighted variants: one block seed s = (x*3129871 ^ z*116129781 ^ y) with the same mixing/shift as above seeds a
  java.util.Random-style LCG (scramble ^ 0x5DEECE66D, a = a*0x5DEECE66D + 0xB, 48 bit); every part (variants entry or
  multipart apply) that lists several models draws nextInt(total weight) and takes the first model whose cumulative
  weight exceeds it (single-model parts draw nothing). Stone and lily pad (4 variants): 300/300 random positions
  identical to vanilla. Bamboo (offset + 4 variants) is NOT reproduced yet (its mesh also differs in a way not
  explained by the four models).

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
  `cargo xtask window-screenshot out.png` captures only the Minecraft window (any OS; macOS uses tools/mc-window-shot.swift).
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
- Window-only video of the real client: `cargo xtask window-bounds` → crop the avfoundation
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
- Command parsing (measured by `basalt-check --fuzz commands`): Brigadier semantics. A literal that matches the next
  word wins, otherwise every argument child is tried; the candidate that consumed all input (then the one without
  errors) wins. Leftover input after a complete parse → `command.unknown.argument`; an incomplete command (node not
  executable) or an unknown first word → `command.unknown.command`; exactly one argument error → that error.
  Entity names read like unquoted strings (`~` is not a name character, so `tp ~ ~ ~ 90` is "unknown argument", while
  `tp 1 2` is two player names → `argument.entity.notfound.entity`). Block positions need integers when absolute
  (`parsing.int.invalid(0.5)`), vec3 centres absolute integer x/z (+0.5). `@s[limit=1]` → `argument.entity.options.inapplicable`.
  Time arguments: float + optional d/s/t (`368t`, `1.5` rounds to 2, `1x` → invalid unit, then `time set` tries the
  marker branch → `commands.time.no_time_marker_found`). `/gamemode` to the current mode, `/time set noon`
  (`commands.time.set.time_marker`), `/save-all` (`commands.save.saving` then `.success`), `/kill` (works on creative),
  `/help <unknown>` (`commands.help.failed`), `/setblock` of the identical block (`commands.setblock.failed`),
  `/fill` counts changed blocks (`commands.fill.failed` when 0), `/effect clear` with nothing to clear, `/enchant`
  with level > max (`commands.enchant.failed.level(level, max)`) all behave as in crates/server/src/game/cmd.rs.
- Digging (measured with fresh dig state, `basalt-check --fuzz dig` / `DIG scans` in the replay format): the server
  accepts a stop when `progress_per_tick * (ticks_since_start + 1) >= 0.7` (stone by hand 104, planks 41, wool 16),
  where progress = speed / hardness / 30 (100 without the right tool), speed /5 in air and /5 with the head in water.
  The client's on_ground flag is ignored: a bot standing on the floor but claiming airborne digs at the ground rate,
  one really above the floor digs 5x slower. A rejected stop is NOT forgotten (delayed destroy): the server keeps
  tracking from the start tick and breaks whatever block is at that position once progress reaches 1.0 — also a
  block placed there later; the tracking ends when the cell is air at a tick. (That is why probes next to the
  threshold used to alternate accept/reject, why early stops on fast blocks still "break" ~8 ticks later, and why
  vanilla looked 4-5 ticks faster than 0.7*T for blocks under ~23 ticks.) Infested blocks are not tool-gated (/30).
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

## Randomized differential harnesses (crates/check/src/fuzz, tools/pr_ready.sh)
- Run: `basalt-check --fuzz clicks|commands|dig|place|worldgen [--cases N] [--seed S] [--long] [--replay FILE] [--jobs N]`.
  Cases are generated from (seed, n) with a per-case splitmix64 stream, recorded as JSONL in
  `target/check/cases/<harness>/<seed>-<n>.jsonl` (per-server results next to them as `.vanilla/.basalt.results.jsonl`,
  failing cases only as `.fail.jsonl`), and replay exactly with `--replay`. Output: mismatch classes with the 3 smallest
  inputs each, and `RESULT fuzz=<id> cases= mismatches= new= info= errors= skipped=`. `new` counts mismatch classes not listed
  in `crates/check/known-divergences.txt` (`<harness> <class>`); `info` classes (packet-level diffs, key-name diffs) never fail.
- Result cache + selective runs: `basalt-check --affected [--base REF]` / `--list-affected` / `--no-cache`; every harness declares
  the paths it can be affected by in `crates/check/src/harness.rs` (globs). Cache key = harness + seed + n + content hash of the
  declared paths + data version + vanilla on/off; only passes are cached (`target/check/cache.json`).
- Vanilla behaviours learned while building these (all observed, not read from code):
  - Chat commands run on an async chain: a command sent right after a container packet can overtake it or lag behind it. Use
    a `list` barrier (`fuzz::barrier`) between "set up with commands" and "act with packets".
  - Container clicks: vanilla answers with *incremental* container_set_slot/set_cursor_item for exactly the changed slots (a click
    that changes nothing sends nothing); Basalt resends set_content for the whole window on every click (info class
    `packets/full-content-vs-incremental`). A client that lost track resyncs by sending a click with a stale state id: vanilla then
    answers with set_content. The harness uses that to start every case from a known view.
  - Vanilla's /clear also clears the carried item and the 2x2 grid; /give fills hotbar 36..44 first, then 9..35.
  - Shift-click in a crafting table moves inventory stacks into the 3x3 grid first (slots 1-9), then between hotbar and main.
  - Dig acceptance: after `start_destroy`, vanilla accepts `stop_destroy` after about `ceil(0.7*T)-1` server ticks, where T is the
    break time computed from the *server's* view of the player: the client's on_ground flag is ignored (a frozen bot claiming
    on_ground=false breaks at the on-ground speed; Basalt takes the flag and applies the /5 penalty). Basalt's rule is
    `0.7*T - 6` ticks. In water (eye in water) the /5 penalty applies in both.
  - Command errors arrive as two system chat lines: the error (translation key) and a `command.context.here` line echoing the input.
    Incomplete commands (`/gamemode`) answer `command.unknown.command`; wrong trailing/bad-typed args `command.unknown.argument`
    or the specific `argument.*`/`parsing.*` key. `/setblock` that changes nothing fails (`commands.setblock.failed`); `/kill` with
    no match fails (`argument.entity.notfound.entity`); `/time set` takes time units (`368t`, `1d`) and marker names.
  - Placement: a click on a replaceable block (grass, snow layers < full, water) places into it; waterlogging is applied for
    slabs/stairs/fences into water; neighbours re-derive connection state (fence/wall/pane/bars) on placement.
- **Divergences found by the first fuzz run (seed 1, default sizes; all listed in known-divergences.txt):**
  - clicks: only `state/table/mode1` — shift-click in a crafting table from the inventory part must go to the grid first
    (crates/server/src/game/inv.rs). Everything else (modes 0-6, drags, number keys, double-click, drops, 2x2 grid) matches in
    final state; packet streams differ everywhere (info): Basalt resends set_content per click, vanilla sends only changed slots.
  - commands (172/500 outcome classes differ, crates/server/src/game.rs `command`): no argument-count/type validation (extra args,
    `weather rain ` with empty duration, `give @s stone 1 2`, `tp ~ ~ ~ 90`, `save-all foo`, `list foo` all succeed); no selector
    parsing (`@s[`, `@x`, `@e[type=foo]`, `kill @e[...]` with no match must fail with argument.entity.notfound.entity); aliases and
    shorthands vanilla rejects (`gamemode 1`, `gamemode s`, bare `enchant sharpness`); missing: `tp @s`/`tp @s @s` (entity
    targets), time units (`368t`, `1d`), `/summon armor_stand|villager`, `setblock` of the same block must fail
    (commands.setblock.failed), `effect clear` with no effects must fail, `help foo` must fail, `/kill` on creative players works in
    vanilla. All replies are English (key table: fuzz/commands.rs `basalt_key`; unmapped ones are `key-unmapped/*` info classes).
    FIXED here: `/tp` accepted NaN/inf/huge coordinates and the next move packet got the player kicked ("invalid position").
  - dig (crates/server/src/game.rs `dig`, crates/game/src/{hardness,lib}.rs): acceptance is `0.7*T-6` ticks in Basalt vs
    `ceil(0.7*T)-1` in vanilla (too early by ~5 ticks for T>20); on_ground comes from the client flag (vanilla: server-side), so
    airborne spoofing changes Basalt's break time; wrong break times for specific block/tool pairs (gravel/netherrack by hand,
    axe on stripped logs, quartz pillar with iron pickaxe: hardness or tool-speed tables differ from vanilla 26.2).
  - place (crates/server/src/game.rs placement): no second halves (doors, beds), no neighbour shape updates (fence/wall/pane/bars
    connections, stairs shape), no support/survival checks (torches, carpets, signs, pressure plates, plants, candles, lanterns pop
    or are refused in vanilla), no replaceable-target rule (grass/snow), no waterlogging, facing for dispensers/droppers/barrels/
    anvils/buttons/skulls/banners/signs uses the wrong axis, interactive targets (chest, slab? no: chest/hopper/shulker/door) must
    open a screen / toggle instead of placing when not sneaking.
  - worldgen parity (tools/parity.sh, 6 seeds x 5 sites): seed 42 site 2,-3,2 features 98.22 and seed 2026 site 0,0,3 shape 98.99
    are under the thresholds (crates/worldgen).
  - repo hygiene found along the way: `cargo clippy` has a deny-level error in crates/client/src/scenario.rs (`erasing_op`), so it
    cannot finish that crate; `cargo fmt --check` differs in ~109 files (dense one-line style).
  - scenarios: `m5_undead_sun` fails on the shared vanilla server right now (reference run: zombie never ignites at noon) with both
    the old and new check binaries; not touched by this work, probably server state (retest on a clean vanilla).

## Containers (M6: chests, barrels, furnaces) — measured on vanilla 26.2 with `basalt-probe proxy` + scenarios m6_*
- Right-click on a container block (not sneaking with an item in hand; reach: eye to block box <= 4.5 + 1.0) answers with
  `sound` (block.chest.open, id 349), `open_screen`, `container_set_content`, and for chests `block_event`; nothing else.
- `open_screen`: varint window id (1..100, counts up per open), varint menu type (MENU registry: generic_9x3 = 2,
  generic_9x6 = 5, furnace = 14, blast_furnace = 10, smoker = 22), title as a network-NBT compound `{translate: "..."}`:
  `container.chest`, `container.chestDouble`, `container.barrel`, `container.furnace`, `container.blast_furnace`, `container.smoker`.
  `container_set_content`: container slots first, then the 27 main + 9 hotbar slots of the player, then the cursor.
- Chest lid (`block_event`): position, u8 type = 1, u8 value = viewer count, varint block registry id (chest = 201). Sent
  at open, re-sent about every 5 ticks while someone has it open, and with the new count when a viewer closes (count 0 at the end).
  Double chests send it for *both* halves. Sounds: `sound` packet source 4 (blocks), volume 0.5, pitch 0.9..1.0, at the block centre
  (for a double chest the midpoint of the halves): block.chest.open (349) when the count goes 0 -> 1, block.chest.close (347) at 1 -> 0.
  Barrels have no block_event: the `open` blockstate flips with the first/last viewer, sounds block.barrel.open/close (141/140)
  at the middle of the face the barrel is facing.
- Chest pairing (measured, scenario m6_double_chest): chest facing south, west neighbour placed first: west = `type=right`,
  east = `type=left`; so a half pairs with the block clockwise of its facing when it is `left` and counter-clockwise when `right`.
  A new chest whose clockwise neighbour is a single chest with the same facing becomes `left`, counter-clockwise `right`; the
  neighbour flips to the opposite type. Different facing never pairs. Double-chest window slots 0..26 = the RIGHT half, 27..53 = the
  LEFT half. Breaking one half turns the other `single`. A solid block above a half blocks opening.
- Barrel facing = opposite of the nearest looking direction (looking down at the floor -> `up`, horizontal -> opposite of the look).
- Double-click (mode 6) collects only when the *clicked slot is empty* (or untakeable); on a non-empty slot it is a no-op.
  Otherwise every click mode (0-6, drags) behaves exactly like the player inventory: scenario m6_chest_clicks compares the container
  and window after 30 consecutive steps and they match. Shift-click: container -> inventory fills from the last slot backwards
  (hotbar end first), inventory -> container from the first slot; furnace: result -> inventory (reverse), smeltable -> input slot,
  fuel -> fuel slot, else main <-> hotbar.
- Closing returns the cursor stack to the inventory. Breaking a container closes its viewers' windows (clientbound container_close)
  and spills the contents (also in creative); the block itself drops as usual.
- Furnaces: `container_set_data` = varint window, i16 property, i16 value; ids 0 = burn time left, 1 = burn duration (fuel's burn
  time), 2 = cook progress, 3 = cook total. All four are sent right after the content when the window opens; afterwards only the
  ones that changed, each tick. Tick order: the tick that lights the fuel already counts one cook tick (burn 1600, progress 1; burn
  time + progress stays 1601), progress returns to 0 in the tick it reaches the total (199 -> 0, ingot appears), without fuel
  progress decays 2 per tick and `lit` goes false when the burn time hits 0. Defaults: cook total 200 (furnace), 100 (blast
  furnace, smoker, per recipe `cookingtime`). Block entity NBT (all numbers shorts): `lit_time_remaining`, `lit_total_time`,
  `cooking_time_spent`, `cooking_total_time`, `RecipesUsed {recipe id: n}` (e.g. `minecraft:iron_ingot_from_smelting_iron_ore`),
  `Items [{Slot: byte, id, count: int, components?}]`, `components: {}`. Taking anything from the result slot pays out the stored
  experience (floor(sum xp) plus a random extra point for the fraction) and clears RecipesUsed.
- Chunk packets carry container block entities with no NBT at all (the NBT field is the single TAG_End byte 0x00), type ids
  chest 1, furnace 0, trapped_chest 2, barrel 26, smoker 27, blast_furnace 28. Placing a container sends no block_entity_data.

## Scripted vanilla-client input (oracle control channel; `basalt-check client-input`, `client-clicks`)
Measured 2026-11-23 by driving the real 26.2 client through tools/oracle-mod's control channel against a fresh Basalt server:
- 26.2 sends **no** `player_command` for sneaking: sneak is only `player_input` flags (0x20 = shift, 0x01 forward, 0x10 jump, 0x40 sprint; sent on change). Sprinting sends the `player_input` sprint flag **and** `player_command` action 1 (start sprint) / 2 (stop sprint, sent with the key release).
- Digging a block under the feet with the held key: `player_action` status 0 (start) at the first tick, one `swing` per tick while the key is held (41 swings over a 40-tick hold, vs 14 from basalt-client), status 2 (stop) when the block breaks; the client then keeps digging the next block while the key is still down (status 0 on the block below). `use_item_on` carries hand, pos, face varint, cursor xyz f32, inside, world-border-hit, sequence.
- Walking 10 ticks + sprinting 10 ticks sends one `move_player_pos` per tick (`pos` only, no rotation unless the view changed); a still player re-sends position every 20 ticks. basalt-client reproduces the same net displacement and packet count (dz 4.86 / 51 packets).
- Container clicks go out as `container_click(window, state_id, slot i16, button i8, mode varint, changed slots [slot, hashed stack], hashed carried)`; hashed stack = present bool, item varint, count varint, added components (type varint + int hash), removed component types. Drags are mode 5 packets: start (-999, button `type<<2`), one per slot (`type<<2|1`), end (-999, `type<<2|2`).
- Client parity gaps found by client-input (sprint `player_command`, shift-click prediction, a swing packet per dig tick) are fixed: 0 divergences (2026-11-23).
- Harness notes: an unfocused window never grabs the mouse, and attack/dig (`continueAttack`) needs the grab flag, so the mod sets it while in a world. The 25700-25799 port range is shared with other agents: `free_port` can race, `client-input` retries the server start.

## Client digging and sprinting vs the real 26.2 client (oracle client-dig / client-input, 2026-11-23)
- Empty hand, survival: stone 150 ticks (start → finish player_action), dirt 15; creative: start only (instant). Same on Basalt and vanilla servers.
- While attack is held on blocks the client sends a swing every tick, including the 5-tick delay between blocks.
- Abort (player_action 1) is sent with face 0 (DOWN), not the mined face.
- Sprint: double-tap forward within 7 ticks or the sprint key starts it; it outlives the key; sneaking does NOT end it (the client keeps the sprint until forward is released). player_input's 0x40 bit is the sprint key state, not the sprint state.
- Known gap (client_dig vanilla/count, vanilla/sequence): falling onto the next block right after a break, basalt-client starts that dig 5 ticks after the break and aborts it a tick later; the real client waits until it has landed.

## Block placement rules (crates/server/src/game/place.rs) — measured with `basalt-check --fuzz place` against vanilla 26.2
Placement went from 178/300 differing cases (seed 1) to 25 (all but 2 are flowing water, see "Not modelled").
- Clicked block: a plain click toggles trapdoors, wooden/copper doors and fence gates (and runs neighbour updates);
  sneaking with an item in hand places instead. Replaceable = tag `replaceable` (snow only at layers=1).
- `setblock` (not `fill`) gives the new block the connection shape its neighbours imply (fence/wall/pane/bars/stairs) and
  runs neighbour updates; it does not self-check survival (floating torches/snow/lanterns stay until a neighbour changes).
- Neighbour updates after a placement: bush-like blocks (flowers, saplings, snow layer, carpets, crops, vines, ...) re-check
  survival on any neighbour change; attached blocks (torches, lanterns, signs, buttons, plates, rails, doors, candles, ...)
  only break when the update comes from their support side. Farmland under a solid block becomes dirt. Beds are placed
  WITHOUT neighbour updates (a floating snow layer beside a new bed survives).
- Support: `sturdy(face)` = full square face (full cubes, glass, slabs by type, stairs: bottom/top half + the face the stair
  FACES; not farmland/soul sand/mud/leaves). `center_support` additionally accepts fence/wall/bars tops and chest undersides:
  torches, candles, sea pickles, lanterns, pressure plates, hanging-sign ceilings use it. Plants use the `supports_*` block tags
  (vegetation, crops, cactus, sugar cane, bamboo, ...). Doors/rails/repeaters/pressure plates/carpets/cake etc. as in vanilla.
  Wall signs need a sturdy face (a fence is refused); wall banners only need a solid block (farmland ok); wall hanging signs also
  accept the side of stairs (not slabs).
- Facing: stairs = look direction; trapdoor = clicked side face (cursor.y>0.5 -> top) unless the clicked block was replaced, then
  the opposite of the look direction (half = top unless clicking the top face); anvil = clockwise of look; decorated pot / fence
  gate / door / bed = look; dispenser/dropper/piston = opposite of the NEAREST look direction (f32 ordering, exact ties keep
  down, up, north, south, west, east); observer = the nearest look direction itself; shulker box / end rod / lightning rod /
  amethyst = clicked face; hopper = opposite of a side face else down.
- StandingAndWall items (torch, sign, banner, skull, coral fan): a side click attaches to the clicked block (wall variant) when it
  survives (skulls: always); otherwise the look directions are walked: DOWN tries the standing variant, others the wall variant.
  Skulls never use the walk for a non-side click. Rotation: signs/banners `floor((yaw+180)*16/360+0.5)&15`, skulls `floor(yaw*16/360+0.5)&15`.
- Buttons/levers/grindstone/ladder/tripwire hook: the clicked face's candidate is tried first, then the look-direction walk.
  Grindstone accepts any non-replaceable neighbour; buttons need a sturdy face.
- Hanging sign: walk the look directions (DOWN skipped): UP = ceiling variant (rotation as signs, attached=true), horizontals =
  wall variant facing away from the look direction (never along the clicked face's axis).
- Doors: both halves, hinge from the two side columns (solid blocks / neighbouring door) else from the cursor; lower half needs a
  sturdy top below. Beds: head one block ahead, both cells replaceable, no support needed. Tall plants need the upper cell free.
- Connections: fences connect to same-kind fences, perpendicular gates and sturdy faces (not leaves/pumpkins/shulkers); panes and
  bars to sturdy faces, walls, panes/bars; walls to sturdy faces, walls, panes/bars (low/tall by the block above; `up` false only for a
  straight line without a post-override block above); stairs shape (outer/inner corners) from front/back stair neighbours.
- Waterlogged only when the cell is a water SOURCE (the coral/pickle defaults are waterlogged=true and must be reset). Leaves
  placed by a player are `persistent=true`. Concrete powder placed in/next to water becomes concrete. Gravity blocks over open
  space fall (resolved instantly, no falling-block entity yet). Farmland item under a solid block becomes dirt.
- Slabs: bottom/top by face and cursor.y>0.5; clicking the matching face of a single slab of the same block makes a double slab.
- Not modelled (known divergences): flowing water spreading from placed/`setblock`ed sources (M6 fluids), random-tick farmland
  drying, bell attachment, hanging sign placed on top of a block (rotation=0/attached=false), random vine/propagule ages.
- Crafting table window, shift-click from the player inventory (slots 10..45): the 3x3 grid is tried first (merge, then empty
  cells); if anything fits there the rest stays in the source slot (no fallback to the other inventory section). Shield (offhand
  equippable) shift-clicks into the offhand slot first.
