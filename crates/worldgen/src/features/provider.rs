//! Int and block-state providers.

use super::{Features, Pred, Tags, R};
use crate::noise::{NoiseParams, NormalNoise};
use crate::random::Rng;
use crate::region::Region;
use crate::state_from_json;
use anyhow::{anyhow, bail, Result};
use basalt_data::BlockState;
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum IntProvider {
    Constant(i32),
    Uniform(i32, i32),
    BiasedToBottom(i32, i32),
    Clamped(Box<IntProvider>, i32, i32),
    Weighted(Vec<(IntProvider, i32)>, i32),
    Trapezoid(i32, i32, i32),
}

impl IntProvider {
    pub fn parse(v: &Value) -> Result<Self> {
        if let Some(i) = v.as_i64() { return Ok(IntProvider::Constant(i as i32)); }
        let g = |k: &str| v[k].as_i64().map(|x| x as i32).ok_or_else(|| anyhow!("int provider {k}"));
        match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "constant" => Ok(IntProvider::Constant(g("value")?)),
            "uniform" => Ok(IntProvider::Uniform(g("min_inclusive")?, g("max_inclusive")?)),
            "biased_to_bottom" => Ok(IntProvider::BiasedToBottom(g("min_inclusive")?, g("max_inclusive")?)),
            "clamped" => Ok(IntProvider::Clamped(Box::new(IntProvider::parse(&v["source"])?), g("min_inclusive")?, g("max_inclusive")?)),
            "weighted_list" => {
                let mut e = Vec::new();
                let mut total = 0;
                for d in v["distribution"].as_array().ok_or_else(|| anyhow!("distribution"))? {
                    let w = d["weight"].as_i64().unwrap_or(1) as i32;
                    total += w;
                    e.push((IntProvider::parse(&d["data"])?, w));
                }
                Ok(IntProvider::Weighted(e, total))
            }
            "trapezoid" => Ok(IntProvider::Trapezoid(g("min")?, g("max")?, v.get("plateau").and_then(|p| p.as_i64()).unwrap_or(0) as i32)),
            t => bail!("unsupported int provider {t}"),
        }
    }

    pub fn sample(&self, r: &mut R) -> i32 {
        match self {
            IntProvider::Constant(c) => *c,
            IntProvider::Uniform(a, b) => r.next_int_bounded(b - a + 1) + a,
            IntProvider::BiasedToBottom(a, b) => { let n = r.next_int_bounded(b - a + 1) + 1; a + r.next_int_bounded(n) }
            IntProvider::Clamped(s, a, b) => s.sample(r).clamp(*a, *b),
            IntProvider::Weighted(e, total) => {
                let mut i = r.next_int_bounded(*total);
                for (p, w) in e {
                    i -= w;
                    if i < 0 { return p.sample(r); }
                }
                0
            }
            IntProvider::Trapezoid(min, max, plateau) => {
                // Measured against vanilla: the plateau-free case is the classic triangular
                // `nextInt(h+1) - nextInt(h+1)` around the centre.
                if *plateau == 0 && (max - min) % 2 == 0 {
                    let h = (max - min) / 2;
                    return (min + max) / 2 + r.next_int_bounded(h + 1) - r.next_int_bounded(h + 1);
                }
                let k = max - min;
                if *plateau >= k { return r.next_int_bounded(k + 1) + min; }
                let l = (k - plateau) / 2;
                let m = k - l;
                min + r.next_int_bounded(m + 1) + r.next_int_bounded(l + 1)
            }
        }
    }

    pub fn max(&self) -> i32 {
        match self {
            IntProvider::Constant(c) => *c,
            IntProvider::Uniform(_, b) | IntProvider::BiasedToBottom(_, b) | IntProvider::Trapezoid(_, b, _) => *b,
            IntProvider::Clamped(s, _, b) => s.max().min(*b),
            IntProvider::Weighted(e, _) => e.iter().map(|(p, _)| p.max()).max().unwrap_or(0),
        }
    }
}

pub enum StateProvider {
    Simple(u16),
    Weighted(Vec<(u16, i32)>, i32),
    RuleBased { rules: Vec<(Pred, StateProvider)>, fallback: Option<Box<StateProvider>> },
    NoiseThreshold { noise: NormalNoise, scale: f64, threshold: f32, high_chance: f32, default: u16, low: Vec<u16>, high: Vec<u16> },
    Noise { noise: NormalNoise, scale: f64, states: Vec<u16> },
    RandomizedInt { source: Box<StateProvider>, property: String, values: IntProvider },
    Rotated(u16),
}

fn noise_of(v: &Value) -> Result<(NormalNoise, f64)> {
    let n = &v["noise"];
    let p = NoiseParams {
        first_octave: n["firstOctave"].as_i64().unwrap_or(0) as i32,
        amplitudes: n["amplitudes"].as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect()).unwrap_or_default(),
    };
    Ok((NormalNoise::new_seeded_legacy(v["seed"].as_i64().unwrap_or(0), &p), v["scale"].as_f64().unwrap_or(1.0)))
}

fn states(v: &Value) -> Result<Vec<u16>> {
    v.as_array().ok_or_else(|| anyhow!("states"))?.iter().map(state_from_json).collect()
}

impl StateProvider {
    pub fn parse(tags: &Tags, v: &Value) -> Result<Self> {
        Ok(match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "simple_state_provider" => StateProvider::Simple(state_from_json(&v["state"])?),
            "weighted_state_provider" => {
                let mut e = Vec::new();
                let mut total = 0;
                for d in v["entries"].as_array().ok_or_else(|| anyhow!("entries"))? {
                    let w = d["weight"].as_i64().unwrap_or(1) as i32;
                    total += w;
                    e.push((state_from_json(&d["data"])?, w));
                }
                StateProvider::Weighted(e, total)
            }
            "rule_based_state_provider" => {
                let mut rules = Vec::new();
                for r in v["rules"].as_array().ok_or_else(|| anyhow!("rules"))? {
                    rules.push((Pred::parse(tags, &r["if_true"])?, StateProvider::parse(tags, &r["then"])?));
                }
                let fallback = if v.get("fallback").map(|f| !f.is_null()).unwrap_or(false) { Some(Box::new(StateProvider::parse(tags, &v["fallback"])?)) } else { None };
                StateProvider::RuleBased { rules, fallback }
            }
            "noise_threshold_provider" => {
                let (noise, scale) = noise_of(v)?;
                StateProvider::NoiseThreshold {
                    noise, scale, threshold: v["threshold"].as_f64().unwrap_or(0.0) as f32, high_chance: v["high_chance"].as_f64().unwrap_or(0.0) as f32,
                    default: state_from_json(&v["default_state"])?, low: states(&v["low_states"])?, high: states(&v["high_states"])?,
                }
            }
            "noise_provider" => {
                let (noise, scale) = noise_of(v)?;
                StateProvider::Noise { noise, scale, states: states(&v["states"])? }
            }
            "randomized_int_state_provider" => StateProvider::RandomizedInt {
                source: Box::new(StateProvider::parse(tags, &v["source"])?),
                property: v["property"].as_str().unwrap_or("").to_string(),
                values: IntProvider::parse(&v["values"])?,
            },
            "rotated_block_provider" => StateProvider::Rotated(state_from_json(&v["state"])?),
            t => bail!("unsupported state provider {t}"),
        })
    }

    /// State for a position (None: a rule-based provider without fallback matched nothing).
    pub fn get(&self, f: &Features, reg: &mut Region, r: &mut R, x: i32, y: i32, z: i32) -> Option<u16> {
        Some(match self {
            StateProvider::Simple(s) => *s,
            StateProvider::Weighted(e, total) => {
                let mut i = r.next_int_bounded(*total);
                let mut out = e[0].0;
                for (s, w) in e {
                    i -= w;
                    if i < 0 { out = *s; break; }
                }
                out
            }
            StateProvider::RuleBased { rules, fallback } => {
                for (p, sp) in rules {
                    if p.test(f, reg, x, y, z) { return sp.get(f, reg, r, x, y, z); }
                }
                return fallback.as_ref().and_then(|fb| fb.get(f, reg, r, x, y, z));
            }
            StateProvider::NoiseThreshold { noise, scale, threshold, high_chance, default, low, high } => {
                let d = noise.get(x as f64 * scale, y as f64 * scale, z as f64 * scale);
                if d < *threshold as f64 { low[r.next_int_bounded(low.len() as i32) as usize] }
                else if r.next_float() < *high_chance { high[r.next_int_bounded(high.len() as i32) as usize] }
                else { *default }
            }
            StateProvider::Noise { noise, scale, states } => {
                let d = noise.get(x as f64 * scale, y as f64 * scale, z as f64 * scale);
                let i = (((1.0 + d) / 2.0 * states.len() as f64) as i32).clamp(0, states.len() as i32 - 1);
                states[i as usize]
            }
            StateProvider::RandomizedInt { source, property, values } => {
                let s = source.get(f, reg, r, x, y, z)?;
                let v = values.sample(r);
                BlockState(s as u32).with(property, &v.to_string()).map(|b| b.0 as u16).unwrap_or(s)
            }
            StateProvider::Rotated(s) => {
                let a = ["x", "y", "z"][r.next_int_bounded(3) as usize];
                BlockState(*s as u32).with("axis", a).map(|b| b.0 as u16).unwrap_or(*s)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_providers() {
        let mut r = R::new(1);
        let t = IntProvider::parse(&serde_json::json!({"type": "minecraft:trapezoid", "min": -7, "max": 7, "plateau": 0})).unwrap();
        let w = IntProvider::parse(&serde_json::json!({"type": "minecraft:weighted_list", "distribution": [{"data": 10, "weight": 9}, {"data": 11, "weight": 1}]})).unwrap();
        let b = IntProvider::parse(&serde_json::json!({"type": "minecraft:biased_to_bottom", "min_inclusive": 2, "max_inclusive": 5})).unwrap();
        for _ in 0..500 {
            assert!((-7..=7).contains(&t.sample(&mut r)));
            assert!((10..=11).contains(&w.sample(&mut r)));
            assert!((2..=5).contains(&b.sample(&mut r)));
        }
        assert_eq!(IntProvider::parse(&serde_json::json!(4)).unwrap().sample(&mut r), 4);
    }
}
