//! All scenarios, grouped by milestone. Each sets up its own area, asserts with `ctx.check` /
//! `ctx.approx`, and records `ctx.metric`s that differential mode compares against vanilla.
use crate::runner::{Servers, Spec, WorldKind};

pub mod env;
pub mod m1;
pub mod m3;
pub mod m4;
pub mod m5;
pub mod m6;
pub mod m8;
pub mod util;

macro_rules! spec {
    ($group:literal, $id:literal, $f:path, $world:ident, $servers:ident, $diff:literal, $desc:literal) => {
        Spec { id: $id, group: $group, desc: $desc, world: WorldKind::$world, servers: Servers::$servers, diff: $diff, run: $f }
    };
}

pub static ALL: &[Spec] = &[
    spec!("M6", "m6_chest_open_close", m6::chest_open_close, Flat, Both, true, "chest window: menu/title/contents, lid block_event(1, viewers), open/close sounds"),
    spec!("M6", "m6_chest_clicks", m6::chest_clicks, Flat, Both, true, "every container_click mode in a chest: state after each step identical to vanilla"),
    spec!("M6", "m6_chest_close_cursor", m6::chest_close_cursor, Flat, Both, true, "closing a chest with an item on the cursor returns it to the inventory"),
    spec!("M6", "m6_double_chest", m6::double_chest, Flat, Both, true, "chests pair by type=left/right, 54-slot window, both halves animate, breaking un-pairs"),
    spec!("M6", "m6_barrel", m6::barrel, Flat, Both, true, "barrel facing, open state + sounds, window"),
    spec!("M6", "m6_break_contents", m6::break_contents, Flat, Both, true, "breaking a container spills contents (also creative), closes viewers' windows"),
    spec!("M6", "m6_container_persistence", m6::container_persistence, FlatOnDisk, BasaltOnly, false, "chest + furnace contents/progress survive a restart"),
    spec!("M6", "m6_furnace_smelt", m6::furnace_smelt, Flat, Both, true, "furnace: lit, container_set_data, 200 ticks per ingot, RecipesUsed, taking the result"),
    spec!("M6", "m6_furnace_clicks", m6::furnace_clicks, Flat, Both, true, "furnace window shift-click rules and slot restrictions"),
    spec!("M6", "m6_furnace_fuel", m6::furnace_fuel, Flat, Both, true, "fuel runs out: lit off, progress decays 2/tick"),
    spec!("M6", "m6_blast_smoker", m6::blast_smoker, Flat, Both, true, "blast furnace 100 ticks + xp, smoker food only"),
    spec!("M6", "m6_weather", m6::weather, Flat, Both, true, "/weather rain|thunder|clear: begin/end events and 0.01/tick level changes"),
    spec!("M6", "m6_random_ticks", m6::random_ticks, Flat, Both, true, "random ticks (random_tick_speed raised briefly): crops, grass spread/decay, leaf decay, sugar cane, cactus, saplings"),
    spec!("M6", "m6_falling_blocks", m6::falling_blocks, Flat, Both, true, "sand/gravel fall as entities: spawn delay, fall time, chains, torch break, concrete powder in water"),
    spec!("M1", "m1_join_spawn", m1::join_spawn, Flat, Both, false, "login/configuration/play, spawn position, health"),
    spec!("M1", "m1_status_ping", m1::status_ping, Flat, Both, false, "server list ping: version 26.2/776 + pong"),
    spec!("M1", "m1_chunk_streaming", m1::chunk_streaming, Flat, Both, true, "chunks around the player arrive; more stream in after moving"),
    spec!("M2", "m2_chat_echo", m1::chat_echo, Flat, Both, false, "chat message is broadcast back"),
    spec!("M2", "m2_commands", m1::commands, Flat, Both, false, "gamemode/give/tp/time replies and effects"),
    spec!("M2", "m2_movement_accepted", m1::movement, Flat, Both, true, "walk 12 blocks, jump, fall: no correction"),
    spec!("M2", "m2_multiplayer", m1::multiplayer, Flat, Both, false, "two bots see each other's add_entity, movement and chat"),
    spec!("M2", "m2_creative_break_place", m1::creative_break_place, Flat, Both, false, "creative break + place, seen by bot and server"),
    spec!("M2", "m2_block_orientation", m1::orientation, Flat, Both, true, "log axis from clicked face, stairs facing from yaw, slab half"),
    spec!("M2", "m2_place_shapes", m1::place_shapes, Flat, Both, true, "doors/beds/tall plants halves, fence/wall/pane connections, support refusal, trapdoor toggling (vs vanilla)"),
    spec!("M2", "m2_persistence", m1::persistence, FlatOnDisk, BasaltOnly, false, "restart with same --world: block, inventory, position kept"),
    spec!("M3", "m3_worldgen_spawn", m3::worldgen_columns, Generated, BasaltOnly, false, "bot-observed terrain at spawn columns = vanilla region files (seed 12345)"),
    spec!("M4", "m4_dig_timing", m4::dig_timing, Flat, Both, true, "survival dig with tool timing; too-fast break rejected"),
    spec!("M4", "m4_drops_pickup", m4::drops_pickup, Flat, Both, true, "broken block drops an item entity which is picked up"),
    spec!("M4", "m4_tool_durability", m4::durability, Flat, Both, true, "pickaxe loses 1 per block, sword 2"),
    spec!("M4", "m4_q_drop", m4::q_drop, Flat, Both, true, "Q drops one, ctrl-Q the stack; item entities carry the stack"),
    spec!("M4", "m4_craft_2x2", m4::craft_2x2, Flat, Both, true, "log -> 4 planks in the inventory grid, shift-click result"),
    spec!("M4", "m4_craft_table", m4::craft_table, Flat, Both, true, "crafting table 3x3 wooden pickaxe"),
    spec!("M4", "m4_armor_slots", m4::armor_slots, Flat, Both, true, "armor slots accept only matching armor; shift-click equips"),
    spec!("M4", "m4_fall_damage", m4::fall_damage, Flat, Both, true, "10-block fall = 7 damage; 3-block fall = 0"),
    spec!("M4", "m4_exhaustion", m4::exhaustion, Flat, Both, true, "sprinting ~40 blocks costs ~1 saturation"),
    spec!("M4", "m4_eating", m4::eating, Flat, Both, true, "hunger effect drains food; eating bread restores 5 food"),
    spec!("M4", "m4_death_respawn", m4::death_respawn, Flat, Both, true, "/kill drops inventory as items; respawn restores health"),
    spec!("M5", "m5_zombie_hit", m5::zombie_hit, Flat, Both, true, "zombie melee 2.5 on easy; 2.025 with an iron chestplate"),
    spec!("M5", "m5_skeleton_arrow", m5::skeleton_arrow, Flat, Both, true, "skeleton arrow from 10 blocks ~3 damage"),
    spec!("M5", "m5_creeper", m5::creeper, Flat, Both, true, "creeper Fuse:0 at 4.36 blocks: 5.157 damage, knockback 0.274"),
    spec!("M5", "m5_cow_loot", m5::cow_loot, Flat, Both, true, "cow killed by player: 1-3 beef, 0-2 leather"),
    spec!("M5", "m5_difficulty", m5::difficulty, Flat, Both, true, "/difficulty replies, change_difficulty, zombie damage per difficulty, peaceful refuses monsters"),
    spec!("M5", "m5_xp_command", m5::xp_command, Flat, Both, true, "/xp add|set|query points and levels: replies and set_experience totals"),
    spec!("M5", "m5_sheep_eat_grass", m5::sheep_eat_grass, Flat, Both, true, "baby sheep eat grass: eating animation, grass turns to dirt"),
    spec!("M5", "m5_breeding", m5::breeding, Flat, Both, true, "feeding two cows wheat: love mode, baby cow, 1-7 xp"),
    spec!("M5", "m5_xp_orbs", m5::xp_orbs, Flat, Both, true, "player kills drop experience orbs (cow 1-3); collected by walking onto them"),
    spec!("M5", "m5_sheep_loot", m5::sheep_loot, Flat, Both, true, "red sheep drops 1 red wool; sheared sheep none"),
    spec!("M5", "m5_undead_sun", m5::undead_sun, Flat, Both, true, "zombie under noon sky catches fire and takes damage"),
    spec!("Env", "env_drowning", env::drowning, Flat, Both, true, "air drains 1/tick, 2 damage at -20, then every 20 ticks; refill +4/tick"),
    spec!("Env", "env_respiration", env::respiration, Flat, Both, true, "Respiration III slows air loss"),
    spec!("Env", "env_water_breathing", env::water_breathing, Flat, Both, true, "Water Breathing keeps air at 300"),
    spec!("Env", "env_freezing", env::freezing, Flat, Both, true, "powder snow: ticks_frozen +1/tick to 140, damage every 40 ticks, thaw 2/tick"),
    spec!("Env", "env_freeze_leather", env::freeze_leather, Flat, Both, true, "leather boots: no freezing"),
    spec!("Env", "env_freeze_fire_thaw", env::freeze_fire_thaw, Flat, Both, true, "frozen player entering lava/fire thaws"),
    spec!("Env", "env_suffocation", env::suffocation, Flat, Both, true, "head in stone: 1 damage per 10 ticks"),
    spec!("Env", "env_lava", env::lava, Flat, Both, true, "lava: 4 damage per 10 ticks and burning"),
    spec!("Env", "env_fire", env::fire, Flat, Both, true, "fire block sets burning; burn damage continues after leaving"),
    spec!("Env", "env_cactus", env::cactus, Flat, Both, true, "touching cactus hurts"),
    spec!("Env", "env_berry_bush", env::berry_bush, Flat, Both, true, "moving through a grown berry bush hurts"),
    spec!("Env", "env_magma", env::magma, Flat, Both, true, "magma hurts unless sneaking"),
    spec!("Env", "env_void", env::void, Flat, Both, true, "below the world: 4 damage per 10 ticks until death"),
    spec!("Items", "items_tracking", m4::item_tracking, Flat, Both, true, "item add_entity carries velocity; update cadence; position matches server"),
    // appended last: a scenario's arena is placed by its index, so inserting earlier would move the others
    spec!("M2", "m2_command_forms", m1::command_forms, Flat, Both, false, "Brigadier parsing: tp/time/gamemode/enchant/help/effect/setblock forms accepted or rejected with vanilla's keys; /kill kills creative players"),
    spec!("M2", "m2_command_effects", m1::command_effects, Flat, Both, true, "/tellraw /title /damage /playsound: replies and effects vs vanilla"),
    spec!("M4", "m4_dig_accept", m4::dig_accept, Flat, Both, false, "stop accepted at progress*(ticks+1) >= 0.7 (stone/hand 104, wool/hand 16), on_ground flag ignored, 5x slower above the floor"),
    spec!("M8", "m8_execute_in", m8::execute_in, Flat, Both, true, "execute in the_nether run tp: respawn into the nether (layout 0..256, bedrock floor/roof), seed-12345 nether columns over the wire, and back"),
    spec!("M8", "m8_portal_frames", m8::portal_frames, Flat, Both, true, "obsidian frames: fire lights 2x3..21-wide frames (x/z axis, corners optional), not 22-wide or incomplete ones; flint and steel; breaking the frame removes the portal"),
    spec!("M8", "m8_portal_travel", m8::portal_travel, Flat, Both, true, "standing in a portal: survival ~80 ticks to x/8 z/8 with a new exit portal, no bounce-back while standing, return to the same portal; creative at once"),
    spec!("M8", "m8_nether_persistence", m8::nether_persistence, FlatOnDisk, BasaltOnly, false, "player + block in the nether survive a restart; nether saved in world/dimensions/minecraft/the_nether/region"),
];
