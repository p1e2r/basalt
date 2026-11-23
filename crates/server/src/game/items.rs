//! Item entities: spawning, vanilla-matching physics, merging, pickup, despawn, and tracking.
//!
//! Tracking follows what a vanilla 26.2 server sends for items (measured with the proxy): the
//! initial velocity rides in add_entity and the client simulates; the server then sends a
//! position delta (move_entity_pos, quantized to 1/4096 of the last *sent* position) plus
//! set_entity_motion when the velocity changed, every 20 ticks or when the item lands/lifts off,
//! and a full entity_position_sync only when the delta doesn't fit in an i16.
use super::{NO_ENCH, Game, Stack};
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use basalt_world::props;
use rand::Rng;
use std::collections::HashSet;
use uuid::Uuid;

pub struct ItemEntity {
    pub eid: i32,
    pub uuid: Uuid,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub stack: Stack,
    pub pickup_delay: u32,
    pub age: u32,
    pub on_ground: bool,
    track: Track,
}

/// What viewers were last told about an entity (shared by every viewer, like vanilla's ServerEntity).
pub struct Track { pos_q: [i64; 3], vel: [f64; 3], on_ground: bool, ticks: u32 }

/// Vanilla's tracker update interval for item entities.
const ITEM_UPDATE_INTERVAL: u32 = 20;

fn quantize(v: f64) -> i64 { (v * 4096.0).round() as i64 }

const DESPAWN_TICKS: u32 = 6000;

impl Game {
    pub(super) fn item_type(&self) -> i32 { basalt_data::registry_id(basalt_data::registries::ENTITY_TYPE, "item").unwrap_or(0) }

    pub(super) fn spawn_item(&mut self, pos: [f64; 3], vel: [f64; 3], stack: Stack, pickup_delay: u32) {
        if stack.count <= 0 { return; }
        let eid = self.sh.next_eid.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let track = Track { pos_q: [quantize(pos[0]), quantize(pos[1]), quantize(pos[2])], vel, on_ground: false, ticks: 0 };
        let e = ItemEntity { eid, uuid: Uuid::new_v4(), pos, vel, stack, pickup_delay, age: 0, on_ground: false, track };
        self.items.insert(eid, e);
        self.sync_item_viewers();
    }

    /// Drops from a broken block, scattered like vanilla (random offset inside the block, small upward pop).
    pub(super) fn drop_at_block(&mut self, b: [i32; 3], drops: Vec<(i32, i32)>) {
        for (item, count) in drops {
            let (ox, oy, oz) = (self.rng.gen_range(0.25..0.75), self.rng.gen_range(0.25..0.75), self.rng.gen_range(0.25..0.75));
            let vel = [self.rng.gen_range(-0.1..0.1), 0.2, self.rng.gen_range(-0.1..0.1)];
            self.spawn_item([b[0] as f64 + ox, b[1] as f64 + oy - 0.125, b[2] as f64 + oz], vel, Stack { item, count, damage: 0, ench: NO_ENCH }, 10);
        }
    }

    /// Item thrown by a player (Q / dragging out of the inventory / death).
    pub(super) fn throw_item(&mut self, eid: i32, stack: Stack, scatter: bool) {
        let Some(p) = self.players.get(&eid) else { return };
        let pos = [p.pos[0], p.pos[1] + 1.62 - 0.3, p.pos[2]];
        let vel = if scatter {
            let (a, s) = (self.rng.gen_range(0.0..std::f64::consts::TAU), self.rng.gen_range(0.0..0.5));
            [a.cos() * s, 0.2, a.sin() * s]
        } else {
            let (y, pi) = ((p.yaw as f64).to_radians(), (p.pitch as f64).to_radians());
            [-y.sin() * pi.cos() * 0.3, -pi.sin() * 0.3 + 0.1, y.cos() * pi.cos() * 0.3]
        };
        self.spawn_item(pos, vel, stack, 40);
    }

    fn item_add_packets(&self, e: &ItemEntity) -> (Writer, Writer) {
        let mut a = Writer::with_id(cb::ADD_ENTITY);
        // initial velocity goes with the spawn so the client can simulate the pop from the start
        // spawn at the shared tracking base so later deltas line up for late viewers too
        let b = e.track.pos_q.map(|c| c as f64 / 4096.0);
        a.varint(e.eid).uuid(e.uuid).varint(self.item_type()).f64(b[0]).f64(b[1]).f64(b[2]).lpvec3(e.track.vel).u8(0).u8(0).u8(0).varint(0);
        let mut m = Writer::with_id(cb::SET_ENTITY_DATA);
        // index 8 = item stack (serializer 7), then terminator
        m.varint(e.eid).u8(8).varint(7);
        super::write_slot(&mut m, e.stack);
        m.u8(0xff);
        (a, m)
    }

    /// Make each player's set of visible item entities match the chunks they have loaded.
    pub(super) fn sync_item_viewers(&mut self) {
        let mut adds: Vec<(i32, Vec<i32>)> = Vec::new();
        let mut removes: Vec<(i32, Vec<i32>)> = Vec::new();
        for p in self.players.values() {
            let visible: HashSet<i32> = self.items.values().filter(|e| p.loaded.contains(&(((e.pos[0].floor() as i32) >> 4), ((e.pos[2].floor() as i32) >> 4)))).map(|e| e.eid).collect();
            let add: Vec<i32> = visible.difference(&p.seen_items).copied().collect();
            let rem: Vec<i32> = p.seen_items.difference(&visible).copied().collect();
            if !add.is_empty() { adds.push((p.eid, add)); }
            if !rem.is_empty() { removes.push((p.eid, rem)); }
        }
        for (pid, ids) in adds {
            for id in &ids { let (a, m) = self.item_add_packets(&self.items[id]); let p = &self.players[&pid]; p.send(a); p.send(m); }
            self.players.get_mut(&pid).unwrap().seen_items.extend(ids);
        }
        for (pid, ids) in removes {
            let p = self.players.get_mut(&pid).unwrap();
            for id in &ids { p.seen_items.remove(id); }
            let mut w = Writer::with_id(cb::REMOVE_ENTITIES); w.varint(ids.len() as i32); for id in &ids { w.varint(*id); } p.send(w);
        }
    }

    fn remove_item(&mut self, eid: i32) {
        self.items.remove(&eid);
        for p in self.players.values_mut() {
            if p.seen_items.remove(&eid) { let mut w = Writer::with_id(cb::REMOVE_ENTITIES); w.varint(1).varint(eid); p.send(w); }
        }
    }

    fn solid(&self, x: f64, y: f64, z: f64) -> bool { props::props(self.world.get_block(x.floor() as i32, y.floor() as i32, z.floor() as i32)).blocks_motion() }

    pub(super) fn tick_items(&mut self) {
        let ids: Vec<i32> = self.items.keys().copied().collect();
        let mut dead = Vec::new();
        for id in &ids {
            let (pos, vel) = { let e = &self.items[id]; (e.pos, e.vel) };
            // unloaded chunk: freeze
            if self.world.chunk((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4).is_none() { continue; }
            // vanilla item order: buoyancy or gravity, move with collision, then drag/friction
            let mut v = vel; let mut p = pos;
            let in_water = props::props(self.world.get_block(p[0].floor() as i32, (p[1] + 0.1).floor() as i32, p[2].floor() as i32)).has_fluid();
            if in_water { if v[1] < 0.06 { v[1] += 5.0e-4; } v[0] *= 0.99; v[2] *= 0.99; } else { v[1] -= 0.04; }
            // stuck inside a block: push up
            if self.solid(p[0], p[1] + 0.1, p[2]) { p[1] = p[1].floor() + 1.0; v[1] = 0.0; }
            let ny = p[1] + v[1];
            let mut on_ground = false;
            if v[1] < 0.0 && self.solid(p[0], ny, p[2]) { p[1] = ny.floor() + 1.0; v[1] = 0.0; on_ground = true; }
            else if v[1] > 0.0 && self.solid(p[0], ny + 0.25, p[2]) { v[1] = 0.0; }
            else { p[1] = ny; }
            for ax in [0usize, 2] {
                let mut q = p; q[ax] += v[ax];
                let edge = q[ax] + 0.125 * v[ax].signum();
                let mut probe = q; probe[ax] = edge;
                if self.solid(probe[0], probe[1] + 0.05, probe[2]) { v[ax] = 0.0; } else { p[ax] = q[ax]; }
            }
            let slip = if on_ground {
                match basalt_data::BlockState(self.world.get_block(p[0].floor() as i32, (p[1] - 0.99).floor() as i32, p[2].floor() as i32)).name() {
                    "ice" | "packed_ice" | "frosted_ice" => 0.98, "blue_ice" => 0.989, "slime_block" => 0.8, _ => 0.6 }
            } else { 1.0 };
            let f = slip * 0.98;
            v[0] *= f; v[2] *= f; v[1] *= 0.98;
            if on_ground && v[1] < 0.0 { v[1] *= -0.5; }
            if p[1] < basalt_world::MIN_Y as f64 - 64.0 { dead.push(*id); continue; }
            let e = self.items.get_mut(id).unwrap();
            e.pos = p; e.vel = v; e.on_ground = on_ground; e.age += 1;
            if e.pickup_delay > 0 { e.pickup_delay -= 1; }
            if e.age >= DESPAWN_TICKS { dead.push(*id); }
        }
        for id in dead { self.remove_item(id); }
        // merge identical stacks lying close together
        if self.tick % 10 == 0 {
            let ids: Vec<i32> = self.items.keys().copied().collect();
            for (i, a) in ids.iter().enumerate() {
                for b in &ids[i + 1..] {
                    let (Some(ea), Some(eb)) = (self.items.get(a), self.items.get(b)) else { continue };
                    if ea.stack.item != eb.stack.item { continue; }
                    let max = self.max_stack(ea.stack.item);
                    if ea.stack.count + eb.stack.count > max { continue; }
                    let d2: f64 = (0..3).map(|k| (ea.pos[k] - eb.pos[k]).powi(2)).sum();
                    if d2 > 0.5 * 0.5 + 0.25 { continue; }
                    let add = eb.stack.count;
                    self.items.get_mut(a).unwrap().stack.count += add;
                    self.remove_item(*b);
                    let e = &self.items[a];
                    let mut m = Writer::with_id(cb::SET_ENTITY_DATA); m.varint(e.eid).u8(8).varint(7); super::write_slot(&mut m, e.stack); m.u8(0xff);
                    for p in self.players.values() { if p.seen_items.contains(a) { p.send(Writer { buf: m.buf.clone() }); } }
                }
            }
        }
        // pickup
        let mut picks: Vec<(i32, i32)> = Vec::new();
        for e in self.items.values() {
            if e.pickup_delay > 0 { continue; }
            for p in self.players.values() {
                if p.dead || p.gamemode == 3 { continue; }
                let dx = (e.pos[0] - p.pos[0]).abs(); let dz = (e.pos[2] - p.pos[2]).abs(); let dy = e.pos[1] - p.pos[1];
                if dx < 1.3 && dz < 1.3 && dy > -0.5 && dy < 2.3 { picks.push((e.eid, p.eid)); break; }
            }
        }
        for (item_id, pid) in picks {
            let Some(e) = self.items.get(&item_id) else { continue };
            let stack = e.stack;
            let left = self.give_stack(pid, stack);
            let taken = stack.count - left;
            if taken <= 0 { continue; }
            for p in self.players.values() { if p.seen_items.contains(&item_id) { let mut w = Writer::with_id(cb::TAKE_ITEM_ENTITY); w.varint(item_id).varint(pid).varint(taken); p.send(w); } }
            if left == 0 { self.remove_item(item_id); } else {
                let e = self.items.get_mut(&item_id).unwrap(); e.stack.count = left;
            }
        }
        // tracking: deltas + motion every 20 ticks or on landing/lift-off, like vanilla
        let mut out: Vec<(i32, Vec<Writer>)> = Vec::new();
        for e in self.items.values_mut() {
            let t = &mut e.track;
            t.ticks += 1;
            let ground_changed = e.on_ground != t.on_ground;
            if t.ticks % ITEM_UPDATE_INTERVAL != 0 && !ground_changed { continue; }
            let mut ws = Vec::new();
            let q = [quantize(e.pos[0]), quantize(e.pos[1]), quantize(e.pos[2])];
            let d = [q[0] - t.pos_q[0], q[1] - t.pos_q[1], q[2] - t.pos_q[2]];
            if d.iter().any(|c| c.abs() > i16::MAX as i64) {
                let mut w = Writer::with_id(cb::ENTITY_POSITION_SYNC);
                w.varint(e.eid).f64(e.pos[0]).f64(e.pos[1]).f64(e.pos[2]).f64(e.vel[0]).f64(e.vel[1]).f64(e.vel[2]).f32(0.0).f32(0.0).bool(e.on_ground);
                ws.push(w); t.pos_q = q;
            } else if d != [0; 3] || ground_changed {
                let mut w = Writer::with_id(cb::MOVE_ENTITY_POS); w.varint(e.eid).i16(d[0] as i16).i16(d[1] as i16).i16(d[2] as i16).bool(e.on_ground);
                ws.push(w); t.pos_q = q;
            }
            let dv: f64 = (0..3).map(|k| (e.vel[k] - t.vel[k]).powi(2)).sum();
            let was_still = t.vel == [0.0; 3];
            if dv > 1.0e-7 || (was_still && e.vel != [0.0; 3]) {
                let mut w = Writer::with_id(cb::SET_ENTITY_MOTION); w.varint(e.eid).lpvec3(e.vel); ws.push(w); t.vel = e.vel;
            }
            t.on_ground = e.on_ground;
            if !ws.is_empty() { out.push((e.eid, ws)); }
        }
        for (id, ws) in out { for p in self.players.values() { if p.seen_items.contains(&id) { for w in &ws { p.send(Writer { buf: w.buf.clone() }); } } } }
        if self.tick % 10 == 0 { self.sync_item_viewers(); }
    }
}
