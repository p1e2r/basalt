//! Animation comparer hook (`--anim-eval IN.jsonl --out OUT.jsonl`), the basalt-client side of
//! `basalt-check client-anim`: reads the vanilla oracle's per-tick bone dumps (tools/oracle-mod
//! SCHEMA.md, control op "bones"), feeds each frame's render-state inputs (walk animation, head
//! rotation, age, attack time, crouching, death time, ...) into Basalt's own model animation and
//! writes the resulting part poses in the same layout, so the harness can diff them per part.
use crate::entity::{self, EntityAtlas, Pose, REC};
use crate::mesh::MeshData;
use crate::Assets;
use anyhow::{Context, Result};
use glam::Mat4;
use serde_json::{json, Map, Value};
use std::f32::consts::PI;
use std::io::Write;

fn kind_of(t: &str) -> Option<i32> {
    let t = t.trim_start_matches("minecraft:");
    basalt_data::registries::ENTITY_TYPE.iter().position(|n| *n == t).map(|i| i as i32)
}

/// Basalt's pose for one vanilla render state.
pub fn pose_from_state(st: &Value, kind: i32) -> Pose {
    let f = |k: &str| st[k].as_f64().unwrap_or(0.0) as f32;
    let b = |k: &str| st[k].as_bool().unwrap_or(false);
    Pose {
        pos: [0.0; 3],
        body: f("bodyRot"),
        limb: f("walkAnimationPos"),
        amt: f("walkAnimationSpeed"),
        head_yaw: f("yRot").to_radians(),
        pitch: f("xRot").to_radians(),
        age: f("ageInTicks"),
        swing: f("attackTime"),
        sneak: b("isCrouching"),
        death: f("deathTime"),
        hurt: b("hasRedOverlay"),
        flip: entity::flip_degrees(kind),
        holding: st["rightArmPose"].as_str() == Some("ITEM"),
        wing: entity::wing_angle(f("flap"), f("flapSpeed")),
    }
}

/// Part poses of `kind` in `pose`: name -> [x, y, z, xRot, yRot, zRot].
pub fn eval(atlas: &EntityAtlas, kind: i32, st: &Value, pose: &Pose) -> Map<String, Value> {
    let mut e = entity::Entity::new(1, uuid::Uuid::nil(), kind, [0.0; 3], [0.0; 3], 0.0, 0.0, 0.0);
    e.baby = st["isBaby"].as_bool().unwrap_or(false);
    if st["isAggressive"].as_bool().unwrap_or(false) { e.mob_flags |= 4; }
    REC.with(|r| *r.borrow_mut() = Some(Vec::new()));
    let mut m = MeshData::default();
    entity::emit_model(atlas, &e, pose, [255, 255], &mut m);
    let rec = REC.with(|r| r.borrow_mut().take()).unwrap_or_default();
    let mut out = Map::new();
    for (n, p, r) in rec { out.insert(n.to_string(), json!([p[0], p[1], p[2], r[0], r[1], r[2]])); }
    out
}

pub fn run(assets: &Assets, input: &str, out: &str) -> Result<()> {
    let atlas = EntityAtlas::build(assets);
    let text = std::fs::read_to_string(input).with_context(|| format!("reading {input}"))?;
    let mut w = std::io::BufWriter::new(std::fs::File::create(out).with_context(|| format!("creating {out}"))?);
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(line)?;
        // first-person records ("fp" recorder): Basalt's hand transform for the same inputs
        if let Some(k @ ("arm" | "item")) = v["kind"].as_str() {
            let f = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
            let (a, h) = (f("attack"), f("mainHO") + (f("mainH") - f("mainHO")) * f("pt"));
            let m = if k == "arm" { entity::fp_arm_matrix(a, h) } else { entity::fp_item_matrix(a, h) };
            writeln!(w, "{}", json!({"tick": v["tick"], "frame": v["frame"], "kind": k, "m": m.to_cols_array().to_vec()}))?;
            continue;
        }
        let Some(frames) = v["frames"].as_array() else { continue };
        let Some(kind) = v["type"].as_str().and_then(kind_of) else { continue };
        let mut fo = Vec::new();
        for fr in frames {
            let st = &fr["state"];
            let pose = pose_from_state(st, kind);
            let parts = eval(&atlas, kind, st, &pose);
            // the body transform in the oracle's convention (setupRotations on an identity stack,
            // before the model flip): Basalt's body matrix x rotY(180)
            let m = entity::body_matrix(&pose) * Mat4::from_rotation_y(PI);
            fo.push(json!({"pt": fr["pt"], "parts": parts, "rot_matrix": m.to_cols_array().to_vec(), "hurt_tint": pose.hurt || pose.death > 0.0}));
        }
        writeln!(w, "{}", json!({"tick": v["tick"], "type": v["type"], "frames": fo}))?;
    }
    Ok(())
}
