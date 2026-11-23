//! Vanilla 26.2 level files, read and written back losslessly (unknown tags are kept):
//! `level.dat` (spawn, difficulty, game type, time), `data/minecraft/world_gen_settings.dat`
//! (seed), `game_rules.dat` (random_tick_speed), `weather.dat`, `world_clocks.dat` (day time).
//! Layouts as found in worlds written by the 26.2 server (vanilla/srv/world, 2026-11-24).
use basalt_nbt::{Compound, Tag};
use std::io::{Read, Write};
use std::path::Path;

pub fn read_gz(path: &Path) -> Option<Tag> {
    let raw = std::fs::read(path).ok()?;
    let mut buf = Vec::new();
    flate2::read::GzDecoder::new(&raw[..])
        .read_to_end(&mut buf)
        .ok()?;
    basalt_nbt::Reader::new(&buf).named().ok().map(|(_, t)| t)
}

pub fn write_gz(path: &Path, t: &Tag) -> std::io::Result<()> {
    let mut raw = Vec::new();
    basalt_nbt::write_named(&mut raw, "", t);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("dat_new");
    let mut e =
        flate2::write::GzEncoder::new(std::fs::File::create(&tmp)?, flate2::Compression::default());
    e.write_all(&raw)?;
    e.finish()?;
    std::fs::rename(tmp, path)
}

/// What Basalt uses from the level files.
#[derive(Clone, Debug, Default)]
pub struct Level {
    pub seed: Option<i64>,
    pub spawn: Option<[i32; 3]>,
    pub difficulty: Option<u8>,
    pub day_time: Option<i64>,
    pub game_time: Option<i64>,
    pub random_tick_speed: Option<i32>,
    pub raining: Option<bool>,
}

fn data(path: &Path) -> Option<Compound> {
    read_gz(path)?.get("data")?.as_compound().cloned()
}

const DIFFS: [&str; 4] = ["peaceful", "easy", "normal", "hard"];

pub fn load(world: &Path) -> Level {
    let mut l = Level::default();
    if let Some(d) = read_gz(&world.join("level.dat"))
        .and_then(|t| t.get("Data").and_then(|d| d.as_compound()).cloned())
    {
        if let Some(Tag::IntArray(p)) = d.get("spawn").and_then(|s| s.get("pos")) {
            if p.len() == 3 {
                l.spawn = Some([p[0], p[1], p[2]]);
            }
        }
        l.difficulty = d
            .get("difficulty_settings")
            .and_then(|s| s.get("difficulty"))
            .and_then(|v| v.as_str())
            .and_then(|n| DIFFS.iter().position(|x| *x == n))
            .map(|i| i as u8);
        l.game_time = d.get("Time").and_then(|t| t.as_i64());
    }
    let dm = world.join("data/minecraft");
    l.seed = data(&dm.join("world_gen_settings.dat"))
        .and_then(|d| d.get("seed").and_then(|s| s.as_i64()));
    l.random_tick_speed = data(&dm.join("game_rules.dat"))
        .and_then(|d| {
            d.get("minecraft:random_tick_speed")
                .and_then(|v| v.as_i64())
        })
        .map(|v| v as i32);
    l.raining = data(&dm.join("weather.dat"))
        .and_then(|d| d.get("raining").and_then(|v| v.as_i64()))
        .map(|v| v != 0);
    l.day_time = data(&dm.join("world_clocks.dat")).and_then(|d| {
        d.get("minecraft:overworld")
            .and_then(|o| o.get("total_ticks"))
            .and_then(|v| v.as_i64())
    });
    l
}

fn root_of(path: &Path) -> Compound {
    read_gz(path)
        .and_then(|t| t.as_compound().cloned())
        .unwrap_or_default()
}

fn put_data(path: &Path, f: impl FnOnce(&mut Compound)) {
    let mut root = root_of(path);
    let mut d = root
        .get("data")
        .and_then(|d| d.as_compound())
        .cloned()
        .unwrap_or_default();
    f(&mut d);
    root.insert("data".into(), Tag::Compound(d));
    root.insert("DataVersion".into(), Tag::Int(basalt_data::WORLD_VERSION));
    if let Err(e) = write_gz(path, &Tag::Compound(root)) {
        log::warn!("writing {}: {e}", path.display());
    }
}

/// Write Basalt's values into the level files, keeping everything else they contain.
pub fn save(world: &Path, l: &Level, flat: bool) {
    let path = world.join("level.dat");
    let mut root = root_of(&path);
    let mut d = root
        .get("Data")
        .and_then(|d| d.as_compound())
        .cloned()
        .unwrap_or_default();
    let fresh = d.is_empty();
    d.insert("DataVersion".into(), Tag::Int(basalt_data::WORLD_VERSION));
    if let Some(t) = l.game_time {
        d.insert("Time".into(), Tag::Long(t));
    }
    if let Some(s) = l.spawn {
        let mut sp = d
            .get("spawn")
            .and_then(|s| s.as_compound())
            .cloned()
            .unwrap_or_default();
        sp.insert("pos".into(), Tag::IntArray(s.to_vec()));
        sp.entry("dimension".into())
            .or_insert(Tag::String("minecraft:overworld".into()));
        sp.entry("yaw".into()).or_insert(Tag::Float(0.0));
        sp.entry("pitch".into()).or_insert(Tag::Float(0.0));
        d.insert("spawn".into(), Tag::Compound(sp));
    }
    if let Some(df) = l.difficulty {
        let mut ds = d
            .get("difficulty_settings")
            .and_then(|s| s.as_compound())
            .cloned()
            .unwrap_or_default();
        ds.insert(
            "difficulty".into(),
            Tag::String(DIFFS[df.min(3) as usize].into()),
        );
        ds.entry("hardcore".into()).or_insert(Tag::Byte(0));
        ds.entry("locked".into()).or_insert(Tag::Byte(0));
        d.insert("difficulty_settings".into(), Tag::Compound(ds));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    d.insert("LastPlayed".into(), Tag::Long(now));
    if fresh {
        // a new world: what a 26.2 level.dat holds besides the fields above (values as in the 26.2
        // server's own level.dat: version 19133, Version Id = DataVersion, Name "26.2", Series "main")
        d.insert(
            "LevelName".into(),
            Tag::String(
                world
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "world".into()),
            ),
        );
        d.insert("GameType".into(), Tag::Int(0));
        d.insert("allowCommands".into(), Tag::Byte(1));
        d.insert("initialized".into(), Tag::Byte(1));
        d.insert("WasModded".into(), Tag::Byte(1));
        d.insert(
            "ServerBrands".into(),
            Tag::List(vec![Tag::String("basalt".into())]),
        );
        d.insert(
            "DataPacks".into(),
            basalt_nbt::compound([
                ("Enabled", Tag::List(vec![Tag::String("vanilla".into())])),
                (
                    "Disabled",
                    Tag::List(
                        [
                            "minecart_improvements",
                            "redstone_experiments",
                            "trade_rebalance",
                        ]
                        .iter()
                        .map(|p| Tag::String(p.to_string()))
                        .collect(),
                    ),
                ),
            ]),
        );
        d.insert("version".into(), Tag::Int(19133));
        d.insert(
            "Version".into(),
            basalt_nbt::compound([
                ("Id", Tag::Int(basalt_data::WORLD_VERSION)),
                ("Name", Tag::String("26.2".into())),
                ("Series", Tag::String("main".into())),
                ("Snapshot", Tag::Byte(0)),
            ]),
        );
    }
    root.insert("Data".into(), Tag::Compound(d));
    if let Err(e) = write_gz(&path, &Tag::Compound(root)) {
        log::warn!("writing level.dat: {e}");
    }
    let dm = world.join("data/minecraft");
    if let Some(seed) = l.seed {
        put_data(&dm.join("world_gen_settings.dat"), |d| {
            d.insert("seed".into(), Tag::Long(seed));
            d.entry("bonus_chest".into()).or_insert(Tag::Byte(0));
            d.entry("generate_structures".into())
                .or_insert(Tag::Byte(1));
            d.entry("dimensions".into())
                .or_insert_with(|| default_dimensions(flat));
        });
    }
    if let Some(r) = l.random_tick_speed {
        put_data(&dm.join("game_rules.dat"), |d| {
            d.insert("minecraft:random_tick_speed".into(), Tag::Int(r));
        });
    }
    if let Some(r) = l.raining {
        put_data(&dm.join("weather.dat"), |d| {
            d.insert("raining".into(), Tag::Byte(r as i8));
        });
    }
    if let Some(t) = l.day_time {
        put_data(&dm.join("world_clocks.dat"), |d| {
            let mut o = d
                .get("minecraft:overworld")
                .and_then(|o| o.as_compound())
                .cloned()
                .unwrap_or_default();
            o.insert("total_ticks".into(), Tag::Long(t));
            d.insert("minecraft:overworld".into(), Tag::Compound(o));
        });
    }
}

/// The dimensions compound of world_gen_settings for a new world (vanilla's presets).
fn default_dimensions(flat: bool) -> Tag {
    let s = |v: &str| Tag::String(v.into());
    let noise = |settings: &str, preset: &str| {
        basalt_nbt::compound([
            ("type", s("minecraft:noise")),
            ("settings", s(settings)),
            (
                "biome_source",
                basalt_nbt::compound([("type", s("minecraft:multi_noise")), ("preset", s(preset))]),
            ),
        ])
    };
    let overworld = if flat {
        let layer =
            |b: &str, h: i32| basalt_nbt::compound([("block", s(b)), ("height", Tag::Int(h))]);
        basalt_nbt::compound([
            ("type", s("minecraft:flat")),
            (
                "settings",
                basalt_nbt::compound([
                    ("biome", s("minecraft:plains")),
                    ("lakes", Tag::Byte(0)),
                    ("features", Tag::Byte(0)),
                    (
                        "layers",
                        Tag::List(vec![
                            layer("minecraft:bedrock", 1),
                            layer("minecraft:dirt", 2),
                            layer("minecraft:grass_block", 1),
                        ]),
                    ),
                    ("structure_overrides", Tag::List(vec![])),
                ]),
            ),
        ])
    } else {
        noise("minecraft:overworld", "minecraft:overworld")
    };
    let dim = |ty: &str, gen: Tag| basalt_nbt::compound([("type", s(ty)), ("generator", gen)]);
    let end = basalt_nbt::compound([
        ("type", s("minecraft:noise")),
        ("settings", s("minecraft:end")),
        (
            "biome_source",
            basalt_nbt::compound([("type", s("minecraft:the_end"))]),
        ),
    ]);
    basalt_nbt::compound([
        ("minecraft:overworld", dim("minecraft:overworld", overworld)),
        (
            "minecraft:the_nether",
            dim(
                "minecraft:the_nether",
                noise("minecraft:nether", "minecraft:nether"),
            ),
        ),
        ("minecraft:the_end", dim("minecraft:the_end", end)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_keeps_unknown_tags() {
        let dir = std::env::temp_dir().join(format!("basalt-level-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // a level.dat with a tag Basalt does not know
        let extra = basalt_nbt::compound([(
            "Data",
            basalt_nbt::compound([("SomeFutureTag", Tag::Int(7))]),
        )]);
        std::fs::create_dir_all(&dir).unwrap();
        write_gz(&dir.join("level.dat"), &extra).unwrap();
        let l = Level {
            seed: Some(42),
            spawn: Some([1, 70, 2]),
            difficulty: Some(2),
            day_time: Some(1234),
            game_time: Some(99),
            random_tick_speed: Some(3),
            raining: Some(false),
        };
        save(&dir, &l, false);
        let back = load(&dir);
        assert_eq!(back.seed, Some(42));
        assert_eq!(back.spawn, Some([1, 70, 2]));
        assert_eq!(back.difficulty, Some(2));
        assert_eq!(back.day_time, Some(1234));
        assert_eq!(
            read_gz(&dir.join("level.dat"))
                .unwrap()
                .get("Data")
                .unwrap()
                .get("SomeFutureTag"),
            Some(&Tag::Int(7))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
