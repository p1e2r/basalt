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

## Entity + first-person animation vs the real 26.2 client (oracle `bones`/`fp`, `basalt-check client-anim`, 2026-11-23)
Measured from the vanilla client's own model parts / PoseStacks (exact fits; formulas live in crates/client/src/entity.rs):
- Limb swing `cos(pos*0.6662 + phase)`; the phase (pi) is added after the scale. Walk anim: speed += (min(dist*4,1)-speed)*0.4, pos += speed.
- Humanoid idle sway: arms zRot ±(0.05cos(0.09age)+0.05), xRot ±0.05sin(0.067age); legs yRot/zRot ±0.005; standing leg z 0, sneak (y 12, z 4).
- Attack swing (6 ticks): body yaw sin(sqrt(t)2pi)*0.2 carries the shoulders; right arm xRot -= sin((1-(1-t)^4)pi)*1.2 + sin(t pi)*-(headPitch-0.7)*0.75, yRot += 3*bodyYaw, zRot -= 0.4 sin(t pi).
- Holding an item: right arm xRot = swing*0.5 - pi/10. Zombie arms -80° (-120° aggressive), yRot ∓(0.1 - 0.6 sin(t pi)), xRot += 1.2 sin(t pi) - 0.4 sin((1-(1-t)^2)pi). Aggressive skeleton without bow: -90°, yRot ∓0.1.
- Creeper: right hind/left front legs use the +pi phase. Spider leg phases hind 0, middle-hind pi, middle-front pi/2, front 3pi/2. Cow front legs at z -5.
- Death roll: sqrt((deathTime-1)/20*1.6) (clamped 1) x 90° (spider 180°), toward the model's right side. Red overlay while hurtTime>0 or dying.
- Chicken wings zRot ±(sin(flap)+1)*flapSpeed; in the air flap +1.8/tick at speed 1; on the ground speed -0.3/tick, flap rate x0.9/tick.
- Mob head yaw from rotate_head: linear over 3 ticks.
- First person: arm/item hand matrices (fp_arm_matrix / fp_item_matrix) match to < 1e-4; equip height eases ±0.4/tick to (attack strength)^3 (cooldown 20/attack speed ticks), to 0 while another item is drawn, which swaps once < 0.1; attacking air swings and resets the strength.
- View bob (view space): T(sin(w pi)b/2, -|cos(w pi)b|, 0) Rz(3° sin(w pi)b) Rx(5° |cos(w pi - 0.2)|b), w = -(walkDist + step*pt), walkDist += 0.6*horizontal distance, b eases x0.4 to min(0.1, horizontal speed) on ground. Hurt roll -14° sin(((hurtTime-pt)/10)^4 pi).
- Not done: swimming pose, hand sway (xBob/yBob lag), bow/crossbow/eat arm poses, baby head scaling, burning-entity fire overlay, red-overlay colour (vanilla white fur -> ~(202..225, 106..125, 106..125)).

## Nether (M8, 2026-11-23) — derived from ./data, vanilla-generated worlds and the m8_* scenarios
- Worldgen: noise_settings/nether.json has `legacy_random_source: true`. Root positional random = `JavaRandom(seed).nextLong()`;
  `fromHashOf(s)` = `JavaRandom(String.hashCode(s) ^ that)`, octaves seeded `hashCode("octave_N") ^ nextLong()`.
  old_blended_noise (nether/base_3d_noise) is seeded with `JavaRandom(worldSeed)` directly. The biome noises
  nether/temperature / nether/vegetation use the 1.16 construction (legacy octave order) seeded `seed` / `seed + 1`:
  with that biomes match vanilla's saved nether 100% (seeds 12345, 42). Biome list: reports/biome_parameters/minecraft/nether.json.
- Generation range = noise.min_y/height (0..128) inside the 256-high dimension: anchors (surface rules, carvers,
  placements) resolve against 0..128 (ceiling bedrock 123..127), y 128..255 stays air. Aquifers off: lava (default fluid) below sea level 32.
- nether_cave carver = cave carver with cave bound 10, thickness `(f*2+f)*2`, tunnel y scale 5, and every replaceable
  carved block becomes lava at y <= min_y+31, else cave_air.
- Nether chunks on the wire: 16 sections, light arrays for sections -1..16 (18 bits), no sky light (sky masks 0);
  heightmaps relative to y 0. Respawn into the nether (measured over the wire): dim type id + "minecraft:the_nether",
  sea level 32, data-kept flags 3 for a dimension change; order respawn, change_difficulty, commands, player_position,
  player_abilities, initialize_border, set_default_spawn_position (overworld), game_event 13, ticking_state/step,
  container_set_content, set_held_slot, set_health, set_experience.
- Portals (m8_portal_frames diff = vanilla): frames light from fire set anywhere inside (setblock too), corners optional,
  inner 2..21 wide x 3..21 high (22 wide does not light, incomplete frames don't), axis x tried first; breaking a frame block
  removes all portal blocks; flint and steel lights it (tool damage 1 in survival).
- Travel (m8_portal_travel diff = vanilla): survival enters after ~80 ticks inside (vanilla compares the counter before
  incrementing: 81st tick), creative at once; staying in the arrival portal never bounces back (the cooldown is renewed
  while inside); stepping out and in returns to the original portal. Exit at x/8, z/8 (x*8 back), search radius 16 into
  the nether / 128 into the overworld; a new portal uses vanilla's PortalForcer spiral search. First run against vanilla:
  arrival x and y identical; the exit is centred across the portal (vanilla drops the across-axis offset).
- Known gap (2026-11-24): exit-portal placement search. For the m8_portal_travel arena, vanilla built the new Nether portal at y 89 / z 38.5, Basalt at y 113 / z 43.5 Vanilla's own arrival position varies between runs on the shared world; arrival x/y/z are notes, travel time and dimension are compared.

## Enchanting / anvil / grindstone / smithing / brewing (M10) — measured on vanilla 26.2 with scenarios m10_*
- Menus (open_screen): enchantment (13, title `container.enchant`, 2 + 36 slots: item, lapis), anvil (8, `container.repair`, 3: two inputs, result), grindstone (15,
  `container.grindstone_title`, 3), smithing (21, `container.upgrade`, 4: template, base, addition, result), brewing_stand (11, `container.brewing`, 5: bottles 0-2,
  ingredient 3, blaze fuel 4). container_set_data right after the content: enchanting 10 props (0-2 level cost of the three buttons, 3 = seed & ~0xF (0 until the player's first
  enchant), 4-6 clue enchantment registry id (-1 none), 7-9 clue level (-1)); anvil 1 (0 = cost, stays >= 40 with an empty result = "Too Expensive"); smithing 1 (0 = recipe
  error); brewing 2 (0 = brew time left, 1 = fuel). Grindstone none.
- Enchanting table: button costs are three independent rolls (base = rand(1..8) + power/2 + rand(0..power); slot0 = max(base/3,1), slot1 = base*2/3+1, slot2 = max(base, power*2); below
  slot+1 -> 0). 15 bookshelves always offer 30 on the top button; corner positions of the 5x5 ring count; a shelf needs the block towards the table (block tags
  enchantment_power_provider / enchantment_power_transmitter) and only two layers (table level and +1) count. Applying button i costs i+1 lapis and i+1 levels (the point total does
  not change), requires levels >= the button cost, sets a new seed, plays block.enchantment_table.use.
- Anvil (checked on 17 cases): cost = sum of both inputs' repair_cost + operations: material repair 1 per unit used (each unit restores max_damage/4), combining two damaged items 2 (+12% bonus),
  each transferred enchantment `anvil_cost * level` (halved, min 1, when the right slot is an enchanted book), +1 per incompatible enchantment, rename +1. Result repair_cost =
  max(left, right) * 2 + 1 except when the only operation was a rename (unchanged). Rename-only cost is capped at 39; cost >= 40 -> empty result, data prop still shows the cost.
  Renaming to the empty string removes a custom name. Taking: levels -= cost, left slot removed, right slot reduced by the units used (or removed when combining).
- Grindstone: one enchanted input -> same item without non-curse enchantments (enchanted_book -> book unless curses remain), repair_cost rebuilt as 2^curses - 1 (absent when none);
  two same items -> combined durability, different items or plain undamaged single -> no result. Taking pays experience orbs: ceil(sum(min_cost(level))/2) + rand(that).
- Smithing: recipes from ./data (smithing_transform / smithing_trim); trim material comes from the addition item's provides_trim_material, pattern from the recipe; same trim again -> no result.
  Taking consumes one of each input.
- Brewing: fuel blaze powder = 20 charges, a brew takes 400 ticks and uses one charge, converts every bottle and uses one ingredient. Recipe table (water, awkward, effect, long_, strong_,
  corrupted, splash via gunpowder, lingering via dragon_breath) in game/station.rs, pinned by the unit test `brew_table_matches_vanilla_measurement` (9 bases x 20 ingredients measured by
  m10_brew_table); notable: water + stone/slime_block/cobweb/breeze_rod = mundane, strong_swiftness + fermented spider eye = unchanged.
- Stack wire components added: repair_cost (varint), custom_name (NBT text), stored_enchantments, potion_contents (bool+potion id, bool, varint 0, bool), trim (material id+1, pattern id+1).

## Overworld structures (M3c, 2026-11-24) — from ./data, templates read from the server jar, vanilla worlds + /locate
Measured with `examples/structures` (dump / rand / raw / heights / compare) against worlds generated by the official
server jar (tools/structure-world.sh, cached in vanilla/structures/) and `/locate` on the vanilla servers.
- random_spread: region = floorDiv(chunk, spacing); `JavaRandom(rx*341873128712 + rz*132897987541 + seed + salt)`,
  offset `nextInt(spacing-separation)` x then z (triangular: mean of two draws per axis). Frequency reduction:
  legacy_type_1 `JavaRandom(((cx>>4) ^ (cz>>4)<<4) ^ seed)`, skip one int, `nextInt(1/f) == 0`; legacy_type_2 the region
  formula with the chunk coords and salt 10387320, `nextFloat < f`; legacy_type_3 large-feature seed, `nextDouble < f`.
  Exclusion zone (outposts vs villages, 10 chunks) = placement geometry + frequency only (no biome check).
- Several structures in a set: weighted tries with a large-feature-seeded JavaRandom (`nextInt(total)`), the failed entry
  removed. Each try: the structure's generation point, then the biome tag at that point (quart cell).
- Start random: `JavaRandom` with setLargeFeatureSeed(seed, cx, cz). Biome check points: jigsaw = (centre of the start
  piece box, start_height + projected surface, centre); onTopOfChunkCenter types (shipwreck, igloo, swamp hut, temples,
  ocean ruins) = chunk (8, 8) at the first occupied height of the heightmap; mineshaft = (8, 50, 8) after one nextDouble;
  buried treasure (9, 90, 9); ruined portal = template origin (chunk corner) at the piece y.
- Jigsaw (all piece lists = vanilla: 23 villages/outposts/trail ruins/trial chambers/ancient cities, 10 000+ pieces): start
  height first, rotation `nextInt(4)`, element `list[nextInt(n)]` (elements repeated by weight). start_jigsaw_name: the
  anchor is looked up in the *shuffled* jigsaw list (consumes the random); piece floor (min y + ground delta 1) moves to
  the start y (+ WORLD_SURFACE_WG at the box centre when projected). Children breadth-first by placement priority (FIFO
  inside a priority); per jigsaw (shuffled, stable-sorted by selection priority): pool list shuffled (duplicates kept) +
  fallback shuffled, rotations shuffled, candidate jigsaws shuffled; free space = parent box (target inside the parent)
  or the shared max_distance box minus placed boxes; expansion hack for villages/outposts. Template jigsaws in (y, x, z)
  order. Pool aliases (trial chambers) resolve with a *legacy* positional random of the world seed at the start origin.
- Shipwrecks: rotation then template from a 20 (ocean) / 11 (beached) entry list (order in place.rs, measured), pivot
  (4, 0, 15), y = mean (ocean) of OCEAN_FLOOR_WG over the template's unrotated footprint from the chunk corner, or the
  minimum of WORLD_SURFACE_WG minus half the height minus nextInt(3) (beached).
- Ruined portals: setup by cumulative weight (only with >1 setups), air pocket drawn only when 0 < p < 1, giant if
  nextFloat < 0.05 (giant_portal_1..3 else portal_1..10), rotation, mirror (nextFloat >= 0.5 = FRONT_BACK), pivot
  (size/2); y on land/ocean floor = highest y with >= 3 of the 4 box corner columns solid.
- Ocean ruins (cold): rotation, large if nextFloat <= 0.3, index (brick_1..8 / big_brick_{1,2,3,8}); three overlaid templates
  brick/cracked/mossy at integrity 0.8/0.7/0.5 (large 0.9/0.7/0.5), origin chunk corner, y = OCEAN_FLOOR_WG at the origin.
- `/locate structure`: rings of spacing regions around the source; the first hit inside a ring (x outer, z inner) wins,
  not the nearest. Reply `commands.locate.structure.success` [id | `#tag (id)`, [x, ~, z], floor(distance)]; locate
  position = chunk corner + locate_offset (buried treasure 9, 0, 9). m3_locate_structure = vanilla for 13 ids + a tag.
- Igloos (9 vanilla igloos, seed 12345): rotation `nextInt(4)`, laboratory if `nextDouble() < 0.5`, then `nextInt(8) + 3`
  ladder pieces. Template origins from the chunk corner: top (0, 0, 0) pivot (3, 0, 5), ladder i (2, -3 - 3i, 4) pivot (1, 0, 1),
  bottom (0, -3n - 6, -2) pivot (3, 0, 7) (one world pivot). Top floor y = WORLD_SURFACE_WG - 1 at the top's local (3, 0) after
  rotation (9/9). Saved order: bottom, ladders top-down, top (vanilla keeps TPY = 90-based and moves only BB). Without a
  laboratory the top's trapdoor (at the pivot) is snow_block. 9/9 piece-exact, blocks 95.7-100 % (rest: snow layers, trees).
- Swamp huts (6 vanilla huts, seed 42; procedural, layout measured with `structures blocks` and encoded in place.rs):
  orientation `nextInt(4)` over (north, east, south, west) = saved `O` (2, 3, 0, 1); box at the chunk corner, 9x7 (west/east)
  or 7x9 (north/south), 7 high; floor y = floor(mean WORLD_SURFACE_WG over the box footprint) (6/6). East = the west layout
  mirrored in x, north = transposed (states: transform_state mirror / mirror + ccw90); south assumed = transpose + z flip (no
  vanilla sample: 0 of 12 located huts of seed 42 face south). Four oak-log legs (1,1) (6,1) (1,5) (6,5) go down from y-1
  while the block is air or water. Hut interior air only inside the walls; the rest of the box keeps terrain/vegetation.
  Blocks 97.7-100 % (rest: mushrooms/grass decoration drift). Witch and cat entities not spawned.
- Not modelled: procedural structures (strongholds, monuments, mansions: placement only, no pieces), the encapsulate
  (trial chambers) and bury (trail ruins, strongholds) adaptations, ocean-ruin clusters and warm ruins' integrity,
  ruined-portal processors (netherrack, lava, moss), the capped processor, block entities (chest loot) and template
  entities (villagers, golems), ancient-city sculk. Unexplained: 4 of 29 seed-12345 mineshaft starts that vanilla rejects
  (lush caves / frozen peaks, biome passes), 1 of 31 desert pyramid candidates (seed 12345 chunk -234,-281: corners above
  sea level, but the middle of one edge is under water; only "a corner at or below sea level rejects" is modelled, 30/31).

### Desert pyramids and jungle temples (M3c, 2026-11-24) — crates/worldgen/src/structure/temple.rs
Measured on 9 desert pyramids (seeds 12345 / 42) and 12 jungle temples (seeds 12345 / 42 / 1) at Basalt's placement
positions, generated with `tools/structure-sites.sh` (one server, several small force-loaded squares; cached in
vanilla/structures/<seed>_temples), plus 22 more desert candidates (vanilla/structures/<seed>_deserts) for the rejection rule.
- Start: orientation `nextInt(4)` over (north, east, south, west) = saved `O` (2, 3, 0, 1) like the swamp hut (21/21); box at
  the chunk corner, desert 21 x 15 x 21, jungle 12 x 10 x 15 (x and z swapped for `O` 1 / 3).
- Desert `HPos` = minimum WORLD_SURFACE_WG over the footprint (9/9). The saved box usually starts at `HPos`, but 3 of 9 sit
  1-2 lower (BB y = HPos - 1 / - 2: -86,-110 and 196,148 of seed 42 by 1, 243,147 by 2); no rule found (start random,
  placement random draws 0..11 per chunk, footprint/halo minima all tried), so Basalt uses `HPos`. The layout reaches 14
  below the box (the hidden TNT room) and is placed relative to the box floor.
- Jungle floor = floor(mean MOTION_BLOCKING_NO_LEAVES over the box) at placement time (11/12; the start-time WG mean gives
  10/12, WORLD_SURFACE 4/12): the temple lies in one chunk, the height is taken when that chunk is decorated.
- Layouts: per cell the majority block of all instances (normalised into one frame) where at least two instances differ from
  Basalt's no-structure terrain (stone/dirt/leaves/vegetation dropped as terrain), states from the `O` = 3 instances; local
  frame = the world frame of `O` 0; `O` 2 = z flipped, 3 = transposed, 1 = transposed + x flipped (the same as the floors'
  draw order, below). Desert suspicious sand (random positions, ~1 in 3 of the sand floor cells) is placed as sand; chests
  without loot.
- Jungle cobblestone / mossy cobblestone: drawn from the placement random (features::R seeded for the structure's
  surface_structures slot) with `nextFloat() < 0.4` = cobblestone. Draw order fitted on all 12 temples (each random cell's
  draw index is consistent with all of them): 13 boxes in local coordinates filled y, x, z with one draw per cell (the
  first is the whole 12 x 5 x 15 base, later air cells draw too), then 127 single cells with their own draw index (tables
  JUNGLE_BOXES / JUNGLE_DRAWS). Cells that are always plain cobblestone or always mossy are fixed blocks.
- Block parity inside the vanilla boxes (examples/structures compare, all instances in-sample): desert 98.4-99.9 % (the 3
  sunk boxes 72-82 %), jungle 86-98 % (one temple whose floor is 1 off: 57 %). Vanilla's per-temple loot, the jungle
  temple's vines and the desert's suspicious sand positions are not reproduced.

### Mineshafts (M3c, 2026-11-24) — crates/worldgen/src/structure/mineshaft.rs
Fitted on the 53 mineshafts saved in the 64 x 64-chunk vanilla worlds of seeds 12345 / 42 / 1 and 10 mesa mineshafts
(vanilla/structures/<seed>_mesa, seeds 12345 / 42 / 2, + 2 normal ones there): all 65 generate piece-for-piece identical (8274 pieces: type, box,
`O`/`D`, `Num`, rails `hr`, spider `sc`, two floors `tf`, depth `GD`, saved order = creation order, depth first). Rules (start
random = large-feature random after the frequency `nextDouble`):
- Room: w = 8 + nextInt(6), h = 5 + nextInt(6), d = 8 + nextInt(6) at (chunk*16 + 2, 50, chunk*16 + 2). Sides N, S, W, E:
  k = 0; loop { k += nextInt(len); if k + 3 > len break; y = y0 + nextInt(h - 4) + 1; child at offset k (N: (x0+k, y, z0-1)
  ...); entrance = the child's x/y (or z/y) extent through the 2-block wall; k += 4 }. (nextInt of a power of two uses the
  high bits: matters for w = 8, h - 4 = 2 / 4.)
- Child (depth d): none if d > 9 or more than 80 blocks (x or z) from the room corner; t = nextInt(100): >= 80 crossing
  (two floors = nextInt(4) == 0, box 5 x 3|7 x 5 around the entry), >= 70 stairs (3 x 8 x 9, y - 5..y + 2), else corridor
  (sections = nextInt(3) + 2, shortened while the box intersects an existing piece, none at 0; then rails = nextInt(3) == 0,
  spider = !rails && nextInt(23) == 0). Pieces may not intersect any earlier piece.
- Corridor children: j = nextInt(4), y = y0 + nextInt(3) - 1; j <= 1 straight on, 2 / 3 = turn left / right (N: west /
  east at z0; S: at z1 - 3; W: north / south at x0; E: at x1 - 3). Then, if depth < 8, side slots at far-end offsets 3, 8,
  13 (up to len - 4): a = nextInt(5): 0 = west/north side, 1 = east/south side, child depth + 2 at y0.
- Crossing children: N, S, W, E minus the side it was entered from, at y0; if two floors: N, W, E, S (all four) with
  `nextBool()` each at y0 + 4. Stairs: one child straight on at the bottom.
- Vertical: normal = top j = (ymax - ymin + 2) + minY(-64); if j < sea - 10: j += nextInt(sea - 10 - j); move by j - ymax
  (53/53). Mesa: vanilla bottoms 55..87 / tops 79..113 (10 samples); no rule on the start random, the room or the surface
  height fitted, Basalt draws the bottom uniformly from 55..87 (statistical).
- Placement (measured per cell in the piece frames, 1015 placed corridors): a piece is left out of a chunk when its box holds
  water or lava inside that chunk (a corridor carved in one chunk and missing in the next), or when the biome at the centre
  of its box part in the chunk is in `#mineshaft_blocking` (deep dark: whole areas of vanilla mineshafts are missing,
  12345 -32,10). Interiors become cave_air (natural air too). Corridors: supports at offsets 2 + 5i (fences, connected to
  the solid wall beside them, + planks; only where the ceiling above both sides is solid; 16 % of vanilla's such spots have
  none, the middle plank ~75 %), planks under the floor where it is air (bridges), rails along the middle (70 % in rail
  corridors), cobwebs under the ceiling at the sides (4-9 %), spider corridors ~55 % cobwebs + a spawner. Crossings: the
  "+" shape per floor, the middle 3 x 3 open between two floors, plank pillars at the four inner corners. Stairs: landings
  (5..7 / 0..2) with a 3-high diagonal. Rooms: 3 layers above the floor fully open, an ellipsoid dome above (87 % of the dome
  cells), entrances cut. Mesa: dark oak. Not reproduced: the per-block random elements' draw order (cobwebs, rails, the
  ~13 % of ceiling cells vanilla leaves solid), torches, hanging chains, chest minecarts (entities), spawner block entity.
- Block parity in the vanilla piece boxes (examples/structures compare, ONLY=mineshaft, first 120 chunks): seed 12345
  90.1 %, 42 88.5 %, 1 89.2 % (terrain alone 27-31 %); mesa 15.8 / 49.7 % (the y is not reproduced).

### Terrain adaptation (M3c, 2026-11-24) — fitted empirically, crates/worldgen/src/structure/adapt.rs
Method: the same seed/site generated twice by the official server jar (tools/structure-world.sh, `NO_STRUCTURES=1` =
generate-structures=false, cached in vanilla/structures/<site>_nostruct), plus Basalt's own pre-adaptation density per
block (`Generator::density_dump`; Basalt's terrain agrees with the no-structure world 99.9 % there). `structures adapt`
dumps (x, y, z, vanilla with, vanilla without, Basalt density) near the pieces of one structure type + the vanilla piece
boxes / ground deltas / projections / junctions. The contribution B is added to the final density per block (before the
aquifer); "vanilla solid <=> density + B > 0" is fitted first with free lookup tables (sum over pieces, by outside offset
and dy), whose shape gave the closed forms below, then their 4 parameters by logistic loss + error count.
- beard_thin (villages, pillager outposts), per rigid piece: `B = -0.53 * dyc/r * exp(-r^2 / 16.4)` with
  `dyc = y + 0.5 - (box min y + ground_level_delta)`, `r^2 = dx^2 + dz^2 + dyc^2`, dx/dz = distance outside the box (0
  within). Odd in dyc: fills below the floor, clears above it; strongest ~1.5 blocks from the floor. Terrain-matching pieces
  (streets) add nothing themselves, but every junction entry of every piece (both ends of a connection are saved, rigid and
  terrain-matching alike) adds `-0.26 * dyc/r * exp(-r^2/16)` around (source_x, source_ground_y, source_z). Reach 12 blocks.
  Explains 98 % of the 22 867 solid<->air flips at 4 fitted sites (residual 458); dropping the junctions leaves 50 %
  unexplained, junctions from rigid pieces only 47 %.
- beard_box (ancient cities): at or above the box floor `B = -0.36 * exp(-r^2/16)`, r = 3D distance outside the box (0
  inside); nothing measurable below the floor (all solid there). Explains 96 % of 278 752 flips; the free table does not do
  better (residual: flips next to aquifer fluid), vertical falloff above the top is slightly slower than horizontal.
- Block parity inside the vanilla piece boxes (before -> after; "halo" = blocks within 8 of a box, outside all boxes):
  village_plains 90.82 -> 99.69 (halo 94.75 -> 99.67), village_desert 94.15 -> 99.75 (96.52 -> 99.76), village_savanna
  90.19 -> 98.95 (95.33 -> 99.53), village_snowy 97.51 -> 99.52 (97.34 -> 99.91), village_taiga 90.62 -> 99.46 (91.54 ->
  99.58), pillager_outpost 90.53 -> 99.72 (93.50 -> 99.85), ancient_city 13.5 -> 96.6 (halo 58.9 -> 94.4; the rest is
  sculk). Snowy/taiga sites were not used for fitting. Encapsulate (trial chambers, already 99.7 %) and bury not fitted.
- Turn off with env BASALT_NO_BEARD (measuring); `HALO=n` in `structures compare` reports the halo rows.
