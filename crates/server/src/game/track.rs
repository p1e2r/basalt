//! Visibility tracking for non-player entities (mobs, projectiles): each player is sent
//! add_entity (+ data/attributes/equipment) when an entity is in a chunk the player has loaded
//! and within the tracking range, and remove_entities when it leaves.
//! Item entities keep their own tracker in items.rs.
use super::Game;
use basalt_protocol::{ids::play::clientbound as cb, Writer};
use std::collections::HashSet;

/// Tracking range in blocks (vanilla: 80 for monsters, 64 for most others; we use one value).
const RANGE: f64 = 80.0;

impl Game {
    /// (eid, position) of every tracked entity.
    fn tracked(&self) -> impl Iterator<Item = (i32, [f64; 3])> + '_ {
        self.mobs.values().map(|m| (m.eid, m.pos)).chain(self.arrows.values().map(|a| (a.eid, a.pos))).chain(self.fallers.values().map(|f| (f.eid, f.pos))).chain(self.orbs.values().map(|o| (o.eid, o.pos)))
    }

    fn spawn_packets_for(&self, eid: i32) -> Vec<Writer> {
        if let Some(m) = self.mobs.get(&eid) { return m.spawn_packets(); }
        if let Some(a) = self.arrows.get(&eid) { return a.spawn_packets(); }
        if let Some(f) = self.fallers.get(&eid) { return f.spawn_packets(); }
        if let Some(o) = self.orbs.get(&eid) { return o.spawn_packets(); }
        Vec::new()
    }

    fn visible_to(p: &super::Player, pos: [f64; 3]) -> bool {
        let c = ((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4);
        let (dx, dz) = (pos[0] - p.pos[0], pos[2] - p.pos[2]);
        p.loaded.contains(&c) && dx * dx + dz * dz <= RANGE * RANGE && !p.dead
    }

    /// Make every player's set of tracked entities match what they should see.
    pub(super) fn sync_viewers(&mut self) {
        let ents: Vec<(i32, [f64; 3])> = self.tracked().collect();
        let mut adds: Vec<(i32, Vec<i32>)> = Vec::new();
        let mut rems: Vec<(i32, Vec<i32>)> = Vec::new();
        for p in self.players.values() {
            let vis: HashSet<i32> = ents.iter().filter(|(_, pos)| Self::visible_to(p, *pos)).map(|e| e.0).collect();
            let add: Vec<i32> = vis.difference(&p.seen).copied().collect();
            let rem: Vec<i32> = p.seen.difference(&vis).copied().collect();
            if !add.is_empty() { adds.push((p.eid, add)); }
            if !rem.is_empty() { rems.push((p.eid, rem)); }
        }
        for (pid, ids) in adds {
            for id in &ids { for w in self.spawn_packets_for(*id) { self.players[&pid].send(w); } }
            self.players.get_mut(&pid).unwrap().seen.extend(ids);
        }
        for (pid, ids) in rems {
            let p = self.players.get_mut(&pid).unwrap();
            for id in &ids { p.seen.remove(id); }
            let mut w = Writer::with_id(cb::REMOVE_ENTITIES); w.varint(ids.len() as i32); for id in &ids { w.varint(*id); } p.send(w);
        }
    }

    /// Start tracking a newly spawned entity for everyone who should see it.
    pub(super) fn sync_viewers_for(&mut self, eid: i32) {
        let Some(pos) = self.tracked().find(|e| e.0 == eid).map(|e| e.1) else { return };
        let viewers: Vec<i32> = self.players.values().filter(|p| !p.seen.contains(&eid) && Self::visible_to(p, pos)).map(|p| p.eid).collect();
        if viewers.is_empty() { return; }
        let pk = self.spawn_packets_for(eid);
        for v in viewers { let p = self.players.get_mut(&v).unwrap(); for w in &pk { p.send(Writer { buf: w.buf.clone() }); } p.seen.insert(eid); }
    }

    pub(super) fn send_to_viewers(&self, eid: i32, w: &Writer) {
        for p in self.players.values() { if p.seen.contains(&eid) { p.send(Writer { buf: w.buf.clone() }); } }
    }

    /// Tell viewers an entity is gone (the entity must already be removed from its map).
    pub(super) fn untrack(&mut self, eid: i32) {
        for p in self.players.values_mut() {
            if p.seen.remove(&eid) { let mut w = Writer::with_id(cb::REMOVE_ENTITIES); w.varint(1).varint(eid); p.send(w); }
        }
    }
}
