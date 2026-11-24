# Basalt — handoff notes for Claude Code

Clean-room Rust reimplementation of Minecraft Java 26.2 (protocol 776). MIT licensed.
Never translate decompiled Mojang code; match behaviour by observing a real server/client
(over the wire, or through the test-only oracle mod's data dumps).

**What may be committed:** never commit Mojang code, assets (textures, models, sounds, lang), jars, or
bulk generated data (./data, ./vanilla incl. vanilla/oracle dumps and caches are gitignored). Small
measured constants — values observed from vanilla behaviour, e.g. a 16-entry colour table or a handful
of light levels — ARE allowed, each with a comment saying where/how it was measured (tool, date).

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
- crates/worldgen structures (M3c, crates/worldgen/src/structure/, rules in NOTES.md "Overworld structures"): all
  overworld random_spread sets (spacing/salt/spread, frequency reduction, exclusion zones, weighted tries, biome check
  points) -> start positions = vanilla for every placed type in 9 sites (64x64 chunks at seeds 12345/42/1 + one site per
  village biome/outpost); jigsaw assembly (villages x5, outposts, trail ruins, trial chambers incl. pool aliases, ancient
  cities) piece-for-piece = vanilla (10k+ pieces); templates read at runtime from vanilla/server.jar ($BASALT_STRUCTURES),
  processors (rule/block_rot/protected_blocks; capped skipped), gravity for terrain_matching, waterlogging, feature pool
  elements; shipwrecks, ruined portals (all 7), cold ocean ruins, igloos (9/9 piece-exact), swamp huts (measured layout)
  placed. Terrain adaptation (structure/adapt.rs, 2026-11-24): beard_thin / beard_box fitted empirically from vanilla worlds
  generated with and without structures (NOTES.md "Terrain adaptation"). `/locate structure <id|#tag>` in the server =
  vanilla (m3_locate_structure, 13 ids + tag). Block-exact inside vanilla piece boxes: trial chambers 99.5-99.8 %,
  villages 98.95-99.75 % (was 90-97.5), outposts 99.7 % (was 90.5), ancient cities 96.6 % (was 13.5; rest is sculk),
  igloos 95.7-100 %, swamp huts 97.7-100 %, desert pyramids 98.4-99.9 % (3/9 vanilla boxes sit 1-2 lower: 72-82 %),
  jungle temples 86-98 % (mossy draw order fitted), mineshafts 88.5-90.1 % (piece generator fitted: 65/65 vanilla
  mineshafts, 8274 pieces exact; per-block randoms statistical; mesa y not fitted), ruined portals 67-94 %, shipwrecks 74-90 %, cold ocean ruins ~89.5 %,
  strongholds (concentric rings 123/128 + 122/128 exact vs /locate, rest 1 chunk off; pieces fitted: 48/48 strongholds,
  7750 pieces exact; measured layouts + fitted shell brick draws: 77-82 %), ocean monuments (start 40/40, measured building
  layout 92.5 % in-sample / 88.9 % elsewhere; rooms not fitted). Woodland mansions not generated. Parity: `tools/structure-parity.sh` (harness
  worldgen_structures; vanilla worlds via tools/structure-world.sh in vanilla/structures/), `examples/structures`.
- crates/worldgen: **terrain + most decoration done** (M3b: exact 5x5 neighbour decoration,
  trees/huge mushrooms/vegetation, lakes, springs, geodes, monster rooms; seed 12345 r3: biomes
  100%, exact 99.0-99.6%, all non-structure blocks 99.2-99.6%; ~75-105 ms/chunk). Remaining:
  lush caves, dark-oak drift, jungle/mangrove/cherry trees, corals, dripstone, sculk, glow lichen.
  Earlier summary: Noise router/density functions, aquifers,
  ore veins, multi-noise biomes, surface rules, cave/canyon carvers, ores + disks + freeze.
  vs vanilla seed 12345 (15x15 chunks at two sites): biomes 100.000%, shape 99.6-99.7%, exact
  terrain blocks 99.3-99.45%. Missing: trees/vegetation, lakes, springs, geodes, monster rooms,
  structures, nether/end. Compare: `cargo run -p basalt-worldgen --release --example compare --
  vanilla/srv/world/dimensions/minecraft/overworld/region <cx> <cz> <radius>`.
  Build the server with --release (worldgen ~40 ms/chunk in release).

- crates/check + tools/check-all.sh: **test harness (M18a).** `basalt-check` is a GPU-free protocol
  bot (login/config/play, chunk + entity + inventory + chat tracking, vanilla-like physics, dig/place/
  click/eat/drop/attack) running ~50 scenarios (M1 M2 M3 M4 M5 Env Items) against fresh Basalt servers
  (`--debug` enables `/basalt:state|block|items` JSON queries) and/or the vanilla server on 25566
  (queries via `data get entity` / `execute if`), with differential comparison (`--diff`).
  `tools/check-all.sh [--quick] [--no-vanilla] [--real-client] [--ci]` = build, `cargo test --workspace`,
  worldgen parity vs vanilla worlds generated by the server jar under vanilla/parity/<seed> (cached),
  scenarios + differential, optional real-client smoke; prints a table, writes target/check/results.json.
  Single scenarios: `target/release/basalt-check --filter m4_fall,Env [--vanilla 127.0.0.1:25566 | --diff]`;
  `--list`; `--probe ADDR name cmd...` for poking a server by hand. CI: .github/workflows/check.yml.
  Vanilla-measured behaviour learned by the harness is in NOTES.md ("Test harness findings").

- Client oracle (M7/M14 parity, stage 1 = block meshing): tools/oracle-mod is a TEST-ONLY Fabric
  client mod (never shipped, not built by cargo/CI; `tools/oracle-mod/build.sh`, `launch.py`) that
  runs the installed vanilla 26.2 client and dumps its own outputs (schema: tools/oracle-mod/SCHEMA.md;
  dumps stay in vanilla/oracle, gitignored). `basalt-check client-mesh [--seed S --n N]` generates
  scenes, runs the oracle + `basalt-client --mesh-dump`, diffs quads by geometry. Seed 1 / n 20000:
  84,726 scenes, 1,231,508 vanilla quads; mismatching quads 1,402,649 -> 623,922 -> 32,849 (2.7 %) after the
  fluid renderer (heights, flow uv, overlay, culling, light), inset-face smooth light, model offsets,
  position-seeded weighted variants and the element rotation sign fixes (rules in NOTES.md). Remaining, each
  class < 1 % of quads: light 10.6k (sculk sensor tendrils, dried ghast, chorus flower, vault), ambient
  occlusion on inset faces 7.9k (~10 % of vertices: a diagonal cell sometimes counts differently), uv 6.8k
  (sculk vein, resin clump, glow lichen, fences/uvlock with x rotation), culling ~3.3k (sculk sensors,
  trapdoors/doors, dried ghast), shade 0.9k, lever/tripwire hook/hanging sign geometry 0.5k, bamboo variants.

### Task list
- [x] M1 client builds, joins vanilla, renders chunks
- [x] M2 crates/world: sections/paletted containers, light, Anvil region read/write
- [x] M2 crates/server: login/config/play, chunk streaming, movement, break/place, chat, multiplayer
- [x] M2 verify with real 26.2 client (tools/run-vanilla-client.py) + basalt-client
- [x] M3a worldgen terrain/biomes/surface/carvers/ores vs vanilla seed 12345, wired into server
- [x] M3b features: trees + vegetation, lakes, springs, geodes, monster rooms; exact neighbour mode
      — done: tools/check-all.sh green 2026-11-23 (M3 4/4, parity seeds 12345/42/1)
- [~] M3c structures (overworld) — placement + jigsaw + template structures done, see crates/worldgen status;
      done 2026-11-24: terrain adaptation (beard_thin/beard_box), igloos, swamp huts (south orientation unverified),
      desert pyramids + jungle temples (measured layouts, structure/temple.rs), mineshafts (structure/mineshaft.rs,
      pieces exact incl. mesa; NOTES.md "Mineshafts"; biome check at the moved room fixed the 4 extra seed-12345 starts);
      strongholds (structure/stronghold.rs: rings, pieces exact, measured layouts; /locate stronghold in
      m3_locate_structure), ocean monuments (structure/monument.rs: start exact, measured layout); multi-site vanilla
      worlds: tools/structure-sites.sh;
      todo: woodland mansions (start rule + pieces; NOTES.md "Woodland mansions"), monument rooms, stronghold inner
      brick order + library/portal draws, encapsulate/bury adaptation, desert sinking rule, mesa mineshaft y, mineshaft per-block random order,
      ancient-city sculk, ocean-ruin clusters, portal processors, chest loot + template entities
- [x] M4 survival gameplay — done, check-all M4 22/22 + Env 26/26 + Items 2/2 (2026-11-23) (crates/game rules; server game/{items,inv,health}.rs):
      hardness/tool dig times (server validates), loot-table drops as item entities + pickup/merge/
      despawn, Q-drop, all container_click modes, 2x2 + crafting table crafting, armor slots,
      health/hunger/saturation/regen/starve, fall damage, eating, death (drops inventory) + respawn.
      Tool durability (damage component, persisted). Environment (game/env.rs): air supply 300 +
      drowning (Respiration, Water Breathing, conduit power w/ frame detection, turtle helmet),
      powder-snow freezing (ticks_frozen 140, thaw 2/tick, leather immunity, entity-data synced),
      suffocation, lava, fire/burning, campfire, magma, cactus, berry bush, void. Status effects
      (/effect), enchantments on stacks (/enchant), /setblock /fill /kill. Armor reduction done in M5. Double blocks + support pops done (2026-11-24; breaking either half of a tall plant/door removes the other, dependants of a removed block pop with tool-less drops via dig / creative / /setblock / /fill / falling sand; /setblock + /fill destroy drop; scenarios m4_double_block_break + m4_support_break match vanilla; rules in NOTES.md "Double blocks and support pops"; pops from explosions/piston/fluids not wired yet). Missing: non-food use_item (buckets etc.), death
      screen in basalt-client (auto-respawns), human-driven real-client inventory test.
- [x] M5 mobs — done, check-all M5 12/12 + Diff 34/34 vs vanilla (2026-11-23) (server game/{mob,ai,path,combat,arrow,explosion,spawn,track}.rs):
      zombie/skeleton/creeper/spider/cow/pig/sheep/chicken with vanilla-measured spawn packets +
      metadata (baby, sheep colour/sheared, creeper fuse, aggressive, burning, death pose); generic
      visibility tracking for mobs+arrows; AABB block collision, step-up, gravity, water buoyancy;
      A* pathfinding (unit-tested); goals: wander, look, panic, tempt, target (spiders only in the
      dark), melee, skeleton archery (arrows stick/hit), creeper swell + explosion (ray-march blocks,
      exposure damage, explosion-decay loot, measured explode packet); undead burn in daylight;
      natural spawning (dark monsters 24..128 blocks away, per-player cap 40; animals on chunk
      generation and rarely afterwards), despawn >128 / random >32; attack/interact packets
      (crit, cooldown, knockback, sword/tool wear), armor reduction + wear for all armor-affected
      damage (health.rs hurt_by), loot via entity tables (on-fire smelting, sheep colour), shearing,
      milking, flint-and-steel creepers; /summon <type> [x y z] [baby NoAI Color:N sheared ignited],
      /kill @e[type=..], /mobs [r] (debug). Mobs persist in <world>/entities.json. --no-spawning.
      XP orbs + /xp done (game/xporb.rs: mob kills by players and ores drop orbs that drift and are collected; vanilla XP model — level, float progress, gained-points counter; m5_xp_orbs + m5_xp_command match vanilla; client XP bar M7/xp_bar). Breeding done (game/breed.rs; m5_breeding matches vanilla: baby 3.3 s after feeding, 1-7 xp). Sheep eat grass (regrow wool; m5_sheep_eat_grass). Land animals avoid water (land-only wander targets, water path cost 8) and swim back to shore (float goal); m5_animals_avoid_water 0 % of samples in a lake vs vanilla 2.4 %, m5_animal_float, m5_chicken_fall match vanilla (2026-11-24). Missing: zombie reinforcements/drowned conversion,
      mob-vs-mob pushing, swimming AI beyond buoyancy, peaceful natural regen. /difficulty done (saved per world; mob/arrow/explosion damage scaling, starvation floor, peaceful removes + refuses monsters; m5_difficulty matches vanilla).
- [~] M6 world logic (weather done: /weather + cycle + rain/thunder levels, scenario m6_weather matches vanilla; containers done: chests (single/double/trapped), barrels, furnace/blast furnace/smoker as block entities (server game/container.rs; Anvil-persisted, windows + click modes + lid events + smelting + XP; scenarios m6_chest_*, m6_double_chest, m6_barrel, m6_break_contents, m6_furnace_*, m6_blast_smoker, m6_container_persistence all match vanilla; client screens M7/chest + M7/furnace; falling blocks done (server game/falling.rs: sand/gravel/powder/anvils as falling_block entities, columns, torch → item, powder hardens; m6_falling_blocks matches vanilla incl. 24-tick 10-block drop; client draws them, M7/falling_block); random ticks done (server game/randomtick.rs + /gamerule random_tick_speed: crops, grass spread/decay, leaf decay, sugar cane, cactus, farmland, oak saplings; m6_random_ticks matches vanilla outcomes); ender chests done (per-player EnderItems, scenario m6_ender_chest); todo: XP orbs, chest/furnace models in the client, fluids, block ticks, other sapling/tree kinds, stems, berries, ice/snow melt)
- [~] M7 client parity (19/19 tools/client-scenarios.sh, 2026-11-23; asset grid 0 problems; input parity vs the real client:
      `basalt-check client-input` 0 divergences (oracle scripted-input channel, tools/oracle-mod Control.java, port 25789);
      done: held item + equip anim, item name above hotbar, break/dig particles, menus like vanilla (blurred world /
      panorama backdrops, title with splash, pause icons, options + all sub-pages, language switching, server list pings
      with MOTD/players/ping bars/favicons); todo: singleplayer (integrated basalt-server), check-all --client group,
      clouds, below-horizon void sky, sky tint, glint, unifont fallback (CJK etc.), toasts (recipe unlocks), advancements/stats screens)
- [~] M8 Nether/End, portals, structures — **Nether started (2026-11-23)**:
      worldgen `Generator::new_dim(seed, Dimension::Nether)` (legacy random source, nether biome preset, nether_cave
      carver, gen range 0..128 inside the 0..256 dimension, nether features in features/nether.rs). Parity
      (`tools/parity.sh --nether 12345 42`, harness `worldgen_nether`, worlds cached in vanilla/parity-nether/<seed>):
      biomes 100% at all 10 sites, shape 99.83-99.99, exact 99.81-99.98, features 99.31-99.97 (fungus forests drift,
      basalt deltas / soul sand valley features unverified). Server: one world/generator/players/entities per
      dimension (game/dim.rs, swapped in as "current"), login/respawn per dimension, nether saved in
      world/dimensions/minecraft/the_nether/region, `execute in <dim> run <cmd>`, portals (game/portal.rs: frames 2x3..21x21,
      fire/flint and steel lighting, breaking, 80-tick travel 8:1, exit search 16/128 + vanilla's PortalForcer build).
      Scenarios m8_execute_in / m8_portal_frames / m8_portal_travel (diff vs vanilla, pass) + m8_nether_persistence;
      client: sky-less dimensions (fog colour background, ambient light, dimension fog), client scenario `nether`.
      Todo: End, structures (fortress, bastion), nether mobs, portal POI files (Basalt keeps basalt-portals.json),
      portal sounds/nausea in the client, entity travel through portals, the overworld region folder is still `region/`.
- [~] M13 partial: /scoreboard (objectives, players, operation) + /bossbar (add/remove/list/get/set; game/bossbar.rs, custom_boss_events.dat) match vanilla (m13_scoreboard, m13_bossbar); basalt-client draws the sidebar and boss bars (crates/client/src/board.rs) with geometry identical to the real client in client_visual scenes scoreboard_sidebar / scoreboard_full / bossbars (also fixed the UI atlas overflow that dropped the ascii font page); todo: tab list (+ list-slot score), below_name score, teams
- [ ] M9 redstone
- [~] M10 enchant/anvil/grindstone/smithing/brewing (server game/{station,stackx}.rs + container.rs Kind::{Enchant,Anvil,Grind,Smith,Brewing}, crates/game ench.rs data-driven
      enchantment rules; basalt-client screens). Differential vs vanilla (2026-11-24): m10_menus, m10_enchant_table, m10_anvil_cases (17 cases, results+cost identical), m10_anvil_take,
      m10_grindstone, m10_smithing, m10_brewing (400-tick brew, fuel 20 -> 19) all match; potion table measured by m10_brew_table (vanilla only) and pinned by a unit test.
      Stack now carries repair cost / custom name / stored enchantments / potion / trim (stackx.rs); `/give` accepts item components (damage, repair_cost, custom_name,
      enchantments, stored_enchantments, potion_contents). Todo: enchant result distribution statistics, anvil item-name default (rename to the item's own name), real-client
      live test of the new screens, brewing stand persistence scenario, /setblock block-entity NBT.
- [ ] M11 villagers/raids/AI
- [ ] M12 bosses/endgame
- [~] M13 commands/datapacks/advancements/scoreboards — commands so far (2026-11-24, scenarios m13_world_commands,
      m13_misc_commands, m13_scoreboard all match vanilla): seed, me, msg/tell/w, setworldspawn, spawnpoint, defaultgamemode,
      item replace entity, clone, particle (simple particles), stopsound, tag, rotate, data get entity|block, scoreboard
      objectives (add/remove/list/setdisplay) + players (set/add/remove/get/list/reset), synced packets, scoreboard.dat.
      selector tag= / scores= / level= filters. Todo: scoreboard criteria other than dummy, enable/modify, data merge/modify,
      teams, bossbar, function/datapacks, advancements, client sidebar rendering.
- [ ] M14 client polish
- [~] M16 lossless vanilla worlds + admin files (admin done: server.properties / ops.json / whitelist.json / banned-*.json in vanilla formats, op levels from commands.json, op/deop/whitelist/ban/ban-ip/pardon/pardon-ip/banlist/kick, login refusals; no ops.json = everyone op; scenario m2_admin; level files done: vanilla overworld region path, level.dat + data/minecraft/*.dat and players/data/<uuid>.dat read and written keeping unknown tags — the official 26.2 server opens Basalt's saves (blocks, seed, player position/inventory/enchantments/XP checked), scenario m16_vanilla_files; todo: entities/ and poi/ folders, block/fluid ticks, structures in chunks)  - [ ] M17 performance  - [~] M18 tests/CI/docs/plugins
      (done: crates/check harness, check-all.sh, CI workflow; todo: --client group (visual/asset/input/UI/perf), docs, plugins)
- Server gaps: survival drops go straight to inventory (no item entities), no container_click /
  crafting, no mobs, no fluids/redstone ticking (Basalt is offline-mode only by design). Real-client walking/
  breaking driven by a human not yet verified (only bot-driven).
- Known client gaps: no entity models (boxes only), survival dig time not hardness-based,
  no inventory screen, no sounds, edge chunks without loaded neighbours are not meshed.

## Testing rules (from the user, 2026-11-23)
- Every new feature ships with a scenario test in the check harness (crates/check).
- Run `tools/check-all.sh` and get a clean pass/fail table before marking anything done.
- Prefer differential checks against vanilla (25566) over hand-written expectations.
- **Run `tools/pr_ready.sh` before merging** (fmt gate for new files, clippy: only NEW warnings vs
  tools/baselines/clippy.txt, `cargo test --workspace`, then only the harnesses affected by your diff;
  `--all` runs everything, `--no-vanilla` skips the differential/fuzz ones, `--client` adds the client group).
  `basalt-check --list-affected` shows what your change touches; results are cached in
  target/check/cache.json (`--no-cache` to force). New harnesses must declare their source paths in
  crates/check/src/harness.rs.
- Randomized differential harnesses (`basalt-check --fuzz clicks|commands|dig|place|worldgen`, each with
  `--cases N --seed S`, `--long`, `--replay FILE`; cases are recorded in target/check/cases/) diff Basalt
  against vanilla per case. Known gaps live in crates/check/known-divergences.txt (`<harness> <class>`);
  only NEW mismatch classes fail. Fix a divergence, then delete its line. `tools/check-all.sh --fuzz` runs them.
- **Client harness group (built 2026-11-23, `basalt-check --group client`, run by `tools/check-all.sh --client`
  and `cargo xtask pr-ready --client`; rows SKIP, never fail, without GPU / real client / display / CI):**
  `client_visual` (8 scenes: flat noon/sunset/night, underwater, cave room, seed-12345 terrain, inventory, title;
  real vanilla window captured with `cargo xtask window-screenshot` (MC_WINDOW_PID picks our client) vs
  basalt-client `--script` ops `config`/`gui`/`screenshot` at the same size; per-tile SSIM + histogram; limits in
  tools/baselines/client-visual.json; images in target/check/client-visual/<scene>/), `client_assets`
  (`--asset-grid` blocks+items), `client_ui_<scenario>` (one row per `basalt-client --scenario`), `client_input`,
  `client_dig`, `client_clicks`, `client_mesh` (oracle), `client_perf` (rd 8/16/32, tools/baselines/client-perf.json,
  fail on >20 % regression). `--update-baseline` re-records visual limits / perf numbers (they describe current
  parity, not perfection: tighten them as the client improves). Several oracle clients may run side by side:
  `ORACLE_CTL_PORT` (the driver picks a free one if 25789 is taken), visual uses vanilla/oracle-visual and `ORACLE_WIN`.
- Client work (M7, M14) is done only when `tools/check-all.sh --client` passes: visual parity vs the
  vanilla client (same seed/pos/rotation/time, per-region SSIM/histogram, diff images on failure),
  asset coverage (every block state + item renders, no missing model/texture), input parity
  (serverbound packets diffed vs the vanilla client through the proxy), UI tests (inventory/
  crafting clicks asserted against server state), perf benchmarks (FPS, mesh time, memory at render
  distance 8/16/32 with regression thresholds).

## Local setup (macOS, Windows, Linux)
Everything runs through `cargo xtask <task>` (alias in .cargo/config.toml; xtask/ is a workspace crate).
The old `tools/*.sh` scripts are thin wrappers around the same tasks, so existing commands still work on macOS.
1. Java 25 + Rust (+ `curl`, which ships with macOS, Linux and Windows 10+). `cargo xtask extract-data`
   (downloads the official server jar via Mojang's version manifest, runs its data generator into ./data).
   Required before any build (crates/data and crates/world generate tables from ./data in build.rs).
2. Minecraft folder detection lives in ONE place, `crates/paths` (basalt-paths): macOS
   `~/Library/Application Support/minecraft`, Windows `%APPDATA%\.minecraft`, Linux `~/.minecraft`
   (override: `BASALT_MINECRAFT_DIR`). Client jar: `--jar` / `BASALT_JAR`, else `vanilla/client.jar`, else
   `<mc>/versions/26.2/26.2.jar`. Sounds/panorama/languages: `--assets/--asset-index` / `BASALT_ASSETS` /
   `BASALT_ASSET_INDEX`, else `<mc>/assets` + the index named in `<mc>/versions/26.2/26.2.json`.
3. Test server (macOS/Linux): vanilla/srv/run.sh (online-mode=false, network-compression-threshold=-1,
   level-seed=12345, server-port=25566). Console via `echo cmd > vanilla/srv/console.fifo`.
   Windows has no FIFOs: the differential harnesses that drive the vanilla console (`--diff`, `--fuzz`,
   `dig_others`) need macOS/Linux or WSL; use `--no-vanilla` on Windows. (check-all's parity world
   generation uses a stdin pipe, so it is portable.)
4. Test client: the user's normal launcher with 26.2, joining localhost. Real-client helpers
   (tools/run-vanilla-client.py, tools/oracle-mod/launch.py) are optional Python and detect the OS.
5. Tasks (ports in docs/headers of xtask/src/*.rs):
   - `cargo xtask pr-ready [--client] [--all] [--no-vanilla] [--update-baselines] [--base REF]`
   - `cargo xtask check-all [--quick] [--no-vanilla] [--real-client] [--ci] [--fuzz] [--fuzz-long]`
   - `cargo xtask smoke [OUTDIR]` (env PORT), `cargo xtask client-scenarios [NAMES|all]` (env PORT, VANILLA, OUT)
   - `cargo xtask window-screenshot out.png` / `window-bounds`: capture ONLY the Minecraft window
     (macOS: swiftc-compiled tools/mc-window-*.swift; Windows: PowerShell + PrintWindow; Linux/X11: xdotool +
     ImageMagick `import` or `xwd`; title prefix `Minecraft`, override `MC_WINDOW_TITLE`).
6. Disk (keep ~/Downloads/rustcraft under 20 GB; user's rule 2026-11-24): `cargo xtask clean-disk [--aggressive] [--dry-run]`
   removes finished agent worktrees (no unmerged commits, nothing changed in 30 min, no process inside; branches stay),
   sweeps build folders (target/, target-play/) with cargo-sweep (2 days; aggressive: 0 days + incremental caches), keeps
   the newest 3 runs per harness in target/check (also pruned at the end of every pr-ready) and drops motion frame dumps.
   It never touches vanilla/ or data/ (jars, oracle dumps, parity worlds: expensive to regenerate) or a running agent.
   Agent worktrees share one build folder: .claude/worktrees/.cargo/config.toml (untracked) sets build.target-dir to the
   main checkout's target/. In a new worktree, `ln -s ../../../target target` so the harness finds target/release.
   Caveat: worktrees building different code at the same time overwrite each other's binaries in the shared folder —
   build and test one at a time, or point BASALT_SERVER (and --client) at copies of your own binaries.
7. CI: .github/workflows/build.yml builds release binaries (server, client, probe, check) on windows-latest
   (x86_64-pc-windows-msvc), macos-14 (aarch64-apple-darwin) and ubuntu-latest, runs `cargo test --workspace`
   and uploads the binaries; check.yml runs the harness on Linux. Both generate ./data with `cargo xtask extract-data`.
   Linux needs only pkg-config + libx11/libxkbcommon dev headers (winit/wgpu dlopen at runtime) and
   libasound2-dev for the opt-in audio feature (`--features basalt-client/audio`; macOS/Windows have audio always on).
   wgpu backends are the defaults (Metal / DX12 / Vulkan). `basalt-server` stops via SIGINT on unix; on Windows the
   check harness kills it (no graceful save).

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
16. Lossless vanilla world compatibility + server admin files (server.properties, ops,
    whitelist, bans).
17. Performance: multithreading, lighting engine, LOD.
18. Parity test suite, CI builds for macOS/Windows/Linux, docs, plugin API.


## Daily log
(One entry appended each day at 8 am Jakarta by the long-run loop: what changed, check numbers, agents, open issues.)

### 2026-11-24 (Tue)
- Merged: M8 Nether (b45d78e: worldgen biomes 100 %, shape/exact 99.8-99.99 %, per-dimension server, portals); client
  harness group (c00436d); M10 enchanting/anvil/grindstone/smithing/brewing (699b0fa, all m10_* = vanilla); M3c structures
  stages 1-4 (8f43c5f, 8082f64, 9115b2c, 57bb118: 165/165 starts, jigsaw 3986/3986 pieces, terrain adaptation fitted
  (villages 98.9-99.8 %, ancient cities 96.6 %), igloos, swamp huts, temples, mineshafts, strongholds 77-82 %, monuments).
- M16: admin files (6b0d3b6), vanilla level files (8d8c533), players/data .dat (a04c6e3): the official server opens Basalt saves.
- M13: /seed /me /msg /setworldspawn /spawnpoint /defaultgamemode /item /clone (7044793), /particle /stopsound /tag
  /rotate (0b566d3), /data get (f5886d9), /scoreboard + operation + selector tag=/scores=/level= (82a8d6e, 6999ffa,
  f9898af), /bossbar + client sidebar/boss bars (75989f4). Ender chests (59f40f3).
- Play-test fixes: font advances (11b527b, UI atlas overflow), grass shimmer = missing mipmaps (ffc14f8, diagnosed from
  2 s motion recordings of both clients; flat_noon SSIM 0.888 -> 0.955), item drops hanging/teleporting (bfe958a), pickup
  animation + hotbar pop + vanilla insertion order (dc5b9de; fuzz_pickup 750 cases 0 mismatches, 13f7446), chat-box keys
  (81a4dc2), player water physics fitted to the real client (c4a3f9a, client-water 4/4 cases).
- Checks: pr_ready green on main after every merge; client_visual 12/12 scenes; client scenarios 35/35; fuzz: clicks 0,
  commands 0 new (8 known), pickup 0, place/dig known only; client_mesh <= 32849 baseline.
- Agents: running — double-block breaking + support checks, land animals avoiding water (both due 8:30 pm on their
  branches). Stopped: the portal-placement agent (no progress; WIP scenario on worktree-agent-a8aab1358eb6aafd2, unmerged).
- Open: water splash/bubbles/sounds, Depth Strider/Dolphin's Grace/lava/currents; landing sounds + particles; throwables,
  tool actions, buckets (requested, continue Wed 25 Nov 8 pm); woodland mansions; m8 exit-portal build rule.
- Schedule: the user takes the laptop away; the loop ends tonight 9 pm WIB with a hand-off in this file.
