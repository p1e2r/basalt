//! Sounds from the launcher's asset store (indexes/<id>.json → objects/<xx>/<hash>):
//! sounds.json events, Ogg Vorbis decoded with lewton, positional attenuation + stereo pan,
//! played through rodio when an output device exists. Every play is also recorded in `log`
//! so headless tests can assert on it without audio hardware.
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[cfg(any(target_os = "macos", target_os = "windows", feature = "audio"))]
const HAVE_AUDIO: bool = true;
#[cfg(not(any(target_os = "macos", target_os = "windows", feature = "audio")))]
const HAVE_AUDIO: bool = false;

#[derive(Clone, Debug)]
struct Variant { name: String, volume: f32, pitch: f32, weight: u32, is_event: bool }

pub struct Pcm { pub rate: u32, pub channels: u16, pub samples: Vec<i16> }

#[derive(Clone, Debug)]
pub struct Played { pub event: String, pub file: String, pub gain: f32, pub pitch: f32, pub pan: f32, pub at: Instant }

pub struct Sounds {
    events: HashMap<String, Vec<Variant>>,
    objects: HashMap<String, String>,
    dir: PathBuf,
    cache: HashMap<String, Option<Arc<Pcm>>>,
    pub master: f32,
    /// per-category volume (options "Music & Sounds"), by category name (block, hostile, ui, ...)
    pub categories: HashMap<String, f32>,
    pub log: Vec<Played>,
    pub listener: ([f64; 3], f32),
    rng: u64,
    #[cfg(any(target_os = "macos", target_os = "windows", feature = "audio"))]
    out: Option<(rodio::OutputStream, rodio::OutputStreamHandle)>,
}

/// Default launcher asset directory and index id for 26.2 (macOS layout; override with flags/env).
pub fn default_assets() -> (PathBuf, Option<String>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let mc = if cfg!(target_os = "macos") { PathBuf::from(&home).join("Library/Application Support/minecraft") }
        else if cfg!(target_os = "windows") { PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join(".minecraft") }
        else { PathBuf::from(&home).join(".minecraft") };
    let dir = std::env::var("BASALT_ASSETS").map(PathBuf::from).unwrap_or_else(|_| mc.join("assets"));
    let id = std::env::var("BASALT_ASSET_INDEX").ok().or_else(|| {
        let v: Value = serde_json::from_slice(&std::fs::read(mc.join("versions/26.2/26.2.json")).ok()?).ok()?;
        v["assetIndex"]["id"].as_str().map(String::from)
    });
    (dir, id)
}

impl Sounds {
    /// Load the sound index. `audio_out`: open an output device (false in tests/headless).
    pub fn load(dir: PathBuf, index: Option<String>, audio_out: bool) -> Sounds {
        let mut s = Sounds { events: HashMap::new(), objects: HashMap::new(), dir: dir.clone(), cache: HashMap::new(), master: 1.0, categories: HashMap::new(), log: Vec::new(), listener: ([0.0; 3], 0.0), rng: 0x2545F4914F6CDD1D,
            #[cfg(any(target_os = "macos", target_os = "windows", feature = "audio"))] out: None };
        let index = index.or_else(|| {
            // newest index file as a fallback
            let mut v: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(dir.join("indexes")).ok()?.flatten()
                .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.file_name().to_string_lossy().trim_end_matches(".json").to_string()))).collect();
            v.sort(); v.pop().map(|x| x.1)
        });
        let Some(index) = index else { log::warn!("no asset index found in {}; sounds disabled", dir.display()); return s };
        let Ok(raw) = std::fs::read(dir.join("indexes").join(format!("{index}.json"))) else { log::warn!("asset index {index} missing; sounds disabled"); return s };
        let Ok(v) = serde_json::from_slice::<Value>(&raw) else { return s };
        if let Some(o) = v["objects"].as_object() { for (k, e) in o { if let Some(h) = e["hash"].as_str() { s.objects.insert(k.clone(), h.to_string()); } } }
        if let Some(sj) = s.read_object("minecraft/sounds.json").and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            for (ev, def) in sj.as_object().into_iter().flatten() {
                let mut vs = Vec::new();
                for e in def["sounds"].as_array().into_iter().flatten() {
                    let v = match e {
                        Value::String(n) => Variant { name: n.clone(), volume: 1.0, pitch: 1.0, weight: 1, is_event: false },
                        Value::Object(o) => Variant { name: o.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(), volume: o.get("volume").and_then(|x| x.as_f64()).unwrap_or(1.0) as f32,
                            pitch: o.get("pitch").and_then(|x| x.as_f64()).unwrap_or(1.0) as f32, weight: o.get("weight").and_then(|x| x.as_u64()).unwrap_or(1) as u32,
                            is_event: o.get("type").and_then(|x| x.as_str()) == Some("event") },
                        _ => continue,
                    };
                    vs.push(v);
                }
                s.events.insert(ev.clone(), vs);
            }
        }
        log::info!("sounds: {} events from asset index {index}", s.events.len());
        #[cfg(any(target_os = "macos", target_os = "windows", feature = "audio"))]
        if audio_out { s.out = rodio::OutputStream::try_default().map_err(|e| log::warn!("no audio output: {e}")).ok(); }
        let _ = audio_out;
        s
    }
    pub fn null() -> Sounds { Sounds::load(PathBuf::from("/nonexistent"), Some("none".into()), false) }
    pub fn event_count(&self) -> usize { self.events.len() }
    pub fn has_output(&self) -> bool {
        #[cfg(any(target_os = "macos", target_os = "windows", feature = "audio"))] { return self.out.is_some(); }
        #[allow(unreachable_code)] { let _ = HAVE_AUDIO; false }
    }

    pub fn set_category(&mut self, cat: &str, v: f32) { if cat != "master" { self.categories.insert(cat.to_string(), v); } }

    /// (code, "Name (Region)") of the languages in the asset store, sorted like the vanilla list.
    pub fn languages(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self.objects.keys().filter_map(|k| k.strip_prefix("minecraft/lang/")?.strip_suffix(".json").map(String::from)).filter_map(|code| {
            let j: Value = serde_json::from_slice(&self.read_object(&format!("minecraft/lang/{code}.json"))?).ok()?;
            Some((code.clone(), format!("{} ({})", j["language.name"].as_str()?, j["language.region"].as_str()?)))
        }).collect();
        v.push(("en_us".into(), "English (US)".into()));
        v.sort_by(|a, b| a.1.cmp(&b.1)); v.dedup_by(|a, b| a.0 == b.0);
        v
    }

    /// Raw bytes of a launcher asset object by its index key (e.g. "minecraft/textures/gui/title/background/panorama_0.png").
    pub fn read_object(&self, key: &str) -> Option<Vec<u8>> {
        let h = self.objects.get(key)?;
        std::fs::read(self.dir.join("objects").join(&h[..2]).join(h)).ok()
    }

    /// Decode (cached) "minecraft/sounds/<name>.ogg".
    pub fn pcm(&mut self, name: &str) -> Option<Arc<Pcm>> {
        if let Some(c) = self.cache.get(name) { return c.clone(); }
        let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
        let data = self.read_object(&format!("{ns}/sounds/{path}.ogg"));
        let pcm = data.and_then(|d| decode_ogg(&d).map_err(|e| log::warn!("decode {name}: {e}")).ok()).map(Arc::new);
        self.cache.insert(name.to_string(), pcm.clone());
        pcm
    }

    fn rand(&mut self) -> u64 { self.rng ^= self.rng << 13; self.rng ^= self.rng >> 7; self.rng ^= self.rng << 17; self.rng }

    fn pick(&mut self, event: &str, depth: u32) -> Option<Variant> {
        let vs = self.events.get(event)?.clone();
        let total: u32 = vs.iter().map(|v| v.weight.max(1)).sum();
        if total == 0 { return None; }
        let mut r = (self.rand() % total as u64) as u32;
        for v in vs {
            let w = v.weight.max(1);
            if r < w { return if v.is_event && depth < 4 { self.pick(&v.name, depth + 1).map(|mut x| { x.volume *= v.volume; x.pitch *= v.pitch; x }) } else { Some(v) }; }
            r -= w;
        }
        None
    }

    /// Play a sound event at a world position (None = non-positional, e.g. UI).
    pub fn play(&mut self, event: &str, pos: Option<[f64; 3]>, volume: f32, pitch: f32) {
        let event = event.trim_start_matches("minecraft:");
        let Some(v) = self.pick(event, 0) else { return };
        let vol = volume * v.volume;
        let (mut gain, mut pan) = (vol.min(1.0), 0.0f32);
        if let Some(p) = pos {
            let (l, yaw) = self.listener;
            let d = [p[0] - l[0], p[1] - l[1], p[2] - l[2]];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32;
            let range = 16.0 * vol.max(1.0);
            gain *= (1.0 - dist / range).max(0.0);
            if dist > 0.01 {
                // listener's right: yaw 0 faces +Z, right = -X
                let (s, c) = yaw.to_radians().sin_cos();
                let right = [-c as f64, 0.0, -s as f64];
                pan = ((d[0] * right[0] + d[2] * right[2]) / dist as f64) as f32;
            }
        }
        gain *= self.master * self.categories.get(category_of(event)).copied().unwrap_or(1.0);
        let pitch = (pitch * v.pitch).clamp(0.5, 2.0);
        self.log.push(Played { event: event.to_string(), file: v.name.clone(), gain, pitch, pan, at: Instant::now() });
        if self.log.len() > 256 { self.log.remove(0); }
        if gain <= 0.001 { return; }
        #[cfg(any(target_os = "macos", target_os = "windows", feature = "audio"))]
        {
            if self.out.is_none() { return; }
            let Some(pcm) = self.pcm(&v.name) else { return };
            use rodio::Source;
            let buf = rodio::buffer::SamplesBuffer::new(pcm.channels, pcm.rate, pcm.samples.clone());
            let l = (gain * (1.0 - pan).min(1.0)).max(0.0); let r = (gain * (1.0 + pan).min(1.0)).max(0.0);
            let src = rodio::source::ChannelVolume::new(buf.speed(pitch), vec![l, r]);
            if let Some((_, h)) = &self.out { let _ = h.play_raw(src.convert_samples()); }
        }
    }
    pub fn play_ui(&mut self, event: &str) { self.play(event, None, 0.25, 1.0); }
}

pub fn decode_ogg(data: &[u8]) -> anyhow::Result<Pcm> {
    let mut r = lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(data))?;
    let (rate, channels) = (r.ident_hdr.audio_sample_rate, r.ident_hdr.audio_channels as u16);
    let mut samples = Vec::new();
    while let Some(p) = r.read_dec_packet_itl()? { samples.extend_from_slice(&p); }
    Ok(Pcm { rate, channels, samples })
}

// ---------------------------------------------------------------- block sound groups

/// Block sound group (public-knowledge table by block name; the data generator doesn't export it).
pub fn block_group(name: &str) -> &'static str {
    let has = |s: &str| name.contains(s);
    if name.ends_with("_wool") || name.ends_with("_carpet") { "wool" }
    else if has("glass") || has("ice") && !has("packed_ice") || name == "glowstone" || name == "sea_lantern" || name == "beacon" { "glass" }
    else if name == "sand" || name == "red_sand" || name.ends_with("concrete_powder") || name == "soul_sand" { "sand" }
    else if name == "gravel" || name == "clay" || name == "farmland" || name == "dirt_path" || name == "coarse_dirt" || name == "dirt" || name == "rooted_dirt" || name == "mud" { "gravel" }
    else if name == "grass_block" || name == "mycelium" || name == "podzol" || has("leaves") || has("sapling") || name == "short_grass" || name == "tall_grass" || name == "fern" || has("flower") || name == "hay_block" || has("tulip") || name == "dandelion" || name == "poppy" || name == "vine" || name == "sugar_cane" || name == "sponge" || name == "kelp" { "grass" }
    else if name == "snow" || name == "snow_block" || name == "powder_snow" { "snow" }
    else if has("planks") || has("_log") || has("_wood") || has("stripped") || name.starts_with("oak") || name.starts_with("spruce") || name.starts_with("birch") || name.starts_with("jungle") || name.starts_with("acacia") || name.starts_with("dark_oak") || name.starts_with("mangrove") || name.starts_with("pale_oak")
        || name == "crafting_table" || name == "bookshelf" || name == "chest" || name == "barrel" || name == "note_block" || name == "jukebox" || has("fence") && !has("nether_brick") || name == "ladder" || name == "composter" || name == "lectern" || name == "cartography_table" || name == "fletching_table" || name == "smithing_table" || name == "loom" { "wood" }
    else if name.ends_with("_block") && (has("iron") || has("gold") || has("diamond") || has("emerald") || has("netherite") || has("lapis") || has("redstone")) || name == "anvil" || name == "hopper" || name == "cauldron" || name == "iron_bars" || name == "iron_door" || name == "iron_trapdoor" { "metal" }
    else { "stone" }
}

/// Sound category of an event, for the per-category volume sliders (by event name prefix; the
/// server's sound packets carry their own source but client-side sounds don't).
fn category_of(event: &str) -> &'static str {
    const HOSTILE: &[&str] = &["zombie", "skeleton", "creeper", "spider", "enderman", "witch", "slime", "phantom", "drowned", "husk", "stray", "blaze", "ghast", "piglin_brute", "warden", "pillager", "vindicator", "evoker", "ravager", "guardian", "silverfish", "endermite", "shulker", "vex", "hoglin", "zoglin", "magma_cube", "wither", "ender_dragon", "breeze", "bogged", "creaking"];
    let mut parts = event.split('.');
    match (parts.next(), parts.next()) {
        (Some("music"), _) | (Some("music_disc"), _) if event.starts_with("music.") => "music",
        (Some("music_disc"), _) | (Some("block"), Some("note_block")) | (Some("block"), Some("jukebox")) => "record",
        (Some("weather"), _) | (Some("entity"), Some("lightning_bolt")) => "weather",
        (Some("block"), _) => "block",
        (Some("ui"), _) => "ui",
        (Some("ambient"), _) => "ambient",
        (Some("entity"), Some("player")) | (Some("entity"), Some("item")) | (Some("entity"), Some("experience_orb")) | (Some("item"), _) => "player",
        (Some("entity"), Some(m)) if HOSTILE.contains(&m) => "hostile",
        _ => "neutral",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn groups() {
        assert_eq!(super::block_group("oak_planks"), "wood");
        assert_eq!(super::block_group("stone"), "stone");
        assert_eq!(super::block_group("grass_block"), "grass");
        assert_eq!(super::block_group("white_wool"), "wool");
        assert_eq!(super::block_group("sand"), "sand");
        assert_eq!(super::block_group("glass"), "glass");
    }
}
