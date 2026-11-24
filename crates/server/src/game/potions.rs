//! Potion table: colour and effects of every vanilla potion, measured on vanilla 26.2 on 2026-11-24 by throwing
//! each splash potion at a player over the wire (`basalt-check --probe`): level_event 2002 / 2007 colour data and
//! update_mob_effect (effect, amplifier, duration ticks) at full strength. Instant potions (2007) are healing
//! (heals 4 << amplifier) and harming (6 << amplifier, measured 6 and 12 damage).

/// (potion name, ARGB colour of the splash particles, [(effect name, amplifier, ticks)], instant)
pub const POTIONS: &[(&str, u32, &[(&str, u8, i32)], bool)] = &[
    ("water", 0xff385dc6, &[], false),
    ("mundane", 0xff385dc6, &[], false),
    ("thick", 0xff385dc6, &[], false),
    ("awkward", 0xff385dc6, &[], false),
    (
        "night_vision",
        0xffc2ff66,
        &[("night_vision", 0, 3600)],
        false,
    ),
    (
        "long_night_vision",
        0xffc2ff66,
        &[("night_vision", 0, 9600)],
        false,
    ),
    (
        "invisibility",
        0xfff6f6f6,
        &[("invisibility", 0, 3600)],
        false,
    ),
    (
        "long_invisibility",
        0xfff6f6f6,
        &[("invisibility", 0, 9600)],
        false,
    ),
    ("leaping", 0xfffdff84, &[("jump_boost", 0, 3600)], false),
    (
        "long_leaping",
        0xfffdff84,
        &[("jump_boost", 0, 9600)],
        false,
    ),
    (
        "strong_leaping",
        0xfffdff84,
        &[("jump_boost", 1, 1800)],
        false,
    ),
    (
        "fire_resistance",
        0xffff9900,
        &[("fire_resistance", 0, 3600)],
        false,
    ),
    (
        "long_fire_resistance",
        0xffff9900,
        &[("fire_resistance", 0, 9600)],
        false,
    ),
    ("swiftness", 0xff33ebff, &[("speed", 0, 3600)], false),
    ("long_swiftness", 0xff33ebff, &[("speed", 0, 9600)], false),
    ("strong_swiftness", 0xff33ebff, &[("speed", 1, 1800)], false),
    ("slowness", 0xff8bafe0, &[("slowness", 0, 1800)], false),
    ("long_slowness", 0xff8bafe0, &[("slowness", 0, 4800)], false),
    (
        "strong_slowness",
        0xff8bafe0,
        &[("slowness", 3, 400)],
        false,
    ),
    (
        "turtle_master",
        0xff8d82e6,
        &[("slowness", 3, 400), ("resistance", 2, 400)],
        false,
    ),
    (
        "long_turtle_master",
        0xff8d82e6,
        &[("slowness", 3, 800), ("resistance", 2, 800)],
        false,
    ),
    (
        "strong_turtle_master",
        0xff8d85e6,
        &[("slowness", 5, 400), ("resistance", 3, 400)],
        false,
    ),
    (
        "water_breathing",
        0xff98dac0,
        &[("water_breathing", 0, 3600)],
        false,
    ),
    (
        "long_water_breathing",
        0xff98dac0,
        &[("water_breathing", 0, 9600)],
        false,
    ),
    ("healing", 0xfff82423, &[("instant_health", 0, 1)], true),
    (
        "strong_healing",
        0xfff82423,
        &[("instant_health", 1, 1)],
        true,
    ),
    ("harming", 0xffa9656a, &[("instant_damage", 0, 1)], true),
    (
        "strong_harming",
        0xffa9656a,
        &[("instant_damage", 1, 1)],
        true,
    ),
    ("poison", 0xff87a363, &[("poison", 0, 900)], false),
    ("long_poison", 0xff87a363, &[("poison", 0, 1800)], false),
    ("strong_poison", 0xff87a363, &[("poison", 1, 432)], false),
    (
        "regeneration",
        0xffcd5cab,
        &[("regeneration", 0, 900)],
        false,
    ),
    (
        "long_regeneration",
        0xffcd5cab,
        &[("regeneration", 0, 1800)],
        false,
    ),
    (
        "strong_regeneration",
        0xffcd5cab,
        &[("regeneration", 1, 450)],
        false,
    ),
    ("strength", 0xffffc700, &[("strength", 0, 3600)], false),
    ("long_strength", 0xffffc700, &[("strength", 0, 9600)], false),
    (
        "strong_strength",
        0xffffc700,
        &[("strength", 1, 1800)],
        false,
    ),
    ("weakness", 0xff484d48, &[("weakness", 0, 1800)], false),
    ("long_weakness", 0xff484d48, &[("weakness", 0, 4800)], false),
    ("luck", 0xff59c106, &[("luck", 0, 6000)], false),
    (
        "slow_falling",
        0xfff3cfb9,
        &[("slow_falling", 0, 1800)],
        false,
    ),
    (
        "long_slow_falling",
        0xfff3cfb9,
        &[("slow_falling", 0, 4800)],
        false,
    ),
    (
        "wind_charged",
        0xffbdc9ff,
        &[("wind_charged", 0, 3600)],
        false,
    ),
    ("weaving", 0xff78695a, &[("weaving", 0, 3600)], false),
    ("oozing", 0xff99ffa3, &[("oozing", 0, 3600)], false),
    ("infested", 0xff8c9b8c, &[("infested", 0, 3600)], false),
];

pub fn potion(name: &str) -> Option<(u32, &'static [(&'static str, u8, i32)], bool)> {
    POTIONS
        .iter()
        .find(|p| p.0 == name)
        .map(|p| (p.1, p.2, p.3))
}
