//! Headless protocol bot: logs in to any 26.2 server (offline mode), tracks what a client would
//! know (chunks, own health/air/effects/inventory, entities, chat) and drives a player with
//! vanilla-like physics. The API is blocking; networking and the 20 Hz client tick run on a
//! shared tokio runtime.
use crate::physics::{Body, Input};
use crate::proto::{self, Meta, Stack, Text};
use crate::world::{Column, World};
use anyhow::{anyhow, bail, Context, Result};
use basalt_protocol::{ids, Conn, Reader, Writer, PROTOCOL_VERSION};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use uuid::Uuid;

use ids::play::clientbound as cb;
use ids::play::serverbound as sb;

pub fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().expect("tokio runtime"))
}

pub fn offline_uuid(name: &str) -> Uuid {
    use md5::{Digest, Md5};
    let mut h = Md5::new(); h.update(format!("OfflinePlayer:{name}").as_bytes());
    let mut b: [u8; 16] = h.finalize().into();
    b[6] = b[6] & 0x0f | 0x30; b[8] = b[8] & 0x3f | 0x80;
    Uuid::from_bytes(b)
}

#[derive(Clone, Debug)]
pub struct Line { pub at: Instant, pub text: Text }

#[derive(Clone, Debug)]
pub struct Entity {
    pub uuid: Uuid,
    pub kind: &'static str,
    pub pos: [f64; 3],
    pub spawn_pos: [f64; 3],
    pub spawn_vel: [f64; 3],
    pub vel: [f64; 3],
    pub item: Option<Stack>,
    pub on_ground: bool,
    pub added: Instant,
    /// Shared flags (index 0: 0x01 burning) and living health (index 9) when sent.
    pub flags: u8,
    pub health: Option<f32>,
    pub meta: HashMap<u8, Meta>,
}

/// An explode packet: centre, radius, affected block count, knockback sent to us.
#[derive(Clone, Debug)]
pub struct Explosion { pub at: Instant, pub pos: [f64; 3], pub radius: f32, pub blocks: i32, pub knockback: Option<[f64; 3]> }

/// One logged entity-related packet (for tracking-cadence checks).
#[derive(Clone, Debug)]
pub struct PacketLog { pub at: Instant, pub id: i32, pub eid: i32 }

pub struct State {
    pub name: String,
    pub eid: i32,
    pub gamemode: u8,
    pub may_fly: bool,
    pub body: Body,
    pub input: Input,
    /// Walk target (x, z) and stop tolerance.
    pub goal: Option<([f64; 2], f64)>,
    /// Don't simulate physics (still sends position packets).
    pub frozen: bool,
    pub spawned: bool,
    pub loaded_sent: bool,
    pub dead: bool,
    pub death_msg: Option<Text>,
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
    pub air: i32,
    pub ticks_frozen: i32,
    pub flags: u8,
    /// effect name -> (amplifier, duration ticks at receipt)
    pub effects: HashMap<String, (i32, i32)>,
    /// Player inventory in window-0 slot order (0 result, 1-4 grid, 5-8 armor, 9-35, 36-44 hotbar, 45 offhand).
    pub inv: Vec<Stack>,
    pub cursor: Stack,
    pub held: u8,
    pub state_id: i32,
    /// Open screen: (window id, menu type name) and its slots.
    pub screen: Option<(i32, String)>,
    pub container: Vec<Stack>,
    pub chat: Vec<Line>,
    pub entities: HashMap<i32, Entity>,
    pub players: HashMap<Uuid, String>,
    pub world: World,
    pub teleports: Vec<(Instant, [f64; 3])>,
    pub plog: Vec<PacketLog>,
    pub counts: HashMap<&'static str, u32>,
    pub ticks: u64,
    pub disconnected: Option<String>,
    pub damage: Vec<(Instant, String)>,
    pub health_log: Vec<(Instant, f32)>,
    pub taken: Vec<(i32, i32, i32)>,
    pub explosions: Vec<Explosion>,
    /// Permission level from entity events 24..28.
    pub op_level: u8,
    /// entity events (eid, status) e.g. 3 = death
    pub entity_events: Vec<(Instant, i32, u8)>,
    pub registries: HashMap<String, Vec<String>>,
    pub block_updates: u32,
    out: UnboundedSender<Vec<u8>>,
    last_sent: ([f64; 3], f32, f32, bool),
    ticks_since_pos: u32,
    last_input: u8,
    pub seq: i32,
}

impl State {
    pub fn send(&self, w: Writer) { let _ = self.out.send(w.buf); }
    pub fn pos(&self) -> [f64; 3] { self.body.pos }
    pub fn held_stack(&self) -> &Stack { &self.inv[36 + self.held as usize] }
    /// Total count of an item across the inventory (excluding crafting slots).
    pub fn count(&self, item: &str) -> i32 { self.inv[5..].iter().filter(|s| s.count > 0 && s.name() == item).map(|s| s.count).sum() }
    pub fn find(&self, item: &str) -> Option<usize> { (5..46).find(|&i| self.inv[i].count > 0 && self.inv[i].name() == item) }
    pub fn ench_name(&self, id: i32) -> String {
        self.registries.get("minecraft:enchantment").and_then(|v| v.get(id as usize)).cloned().unwrap_or_else(|| format!("#{id}")).trim_start_matches("minecraft:").to_string()
    }
    pub fn items_near(&self, p: [f64; 3], r: f64) -> Vec<(i32, Entity)> {
        let mut v: Vec<_> = self.entities.iter().filter(|(_, e)| e.kind == "item" && (0..3).map(|i| (e.pos[i] - p[i]).powi(2)).sum::<f64>() <= r * r).map(|(k, e)| (*k, e.clone())).collect();
        v.sort_by_key(|(k, _)| *k);
        v
    }
    fn bump(&mut self, id: i32) { *self.counts.entry(cb::name(id)).or_default() += 1; }

    fn send_move(&mut self, force: bool) {
        let b = &self.body;
        let flags = b.on_ground as u8 | (b.h_collide as u8) << 1;
        let moved = (0..3).any(|i| (b.pos[i] - self.last_sent.0[i]).abs() > 1e-4) || self.ticks_since_pos >= 20;
        let rotated = b.yaw != self.last_sent.1 || b.pitch != self.last_sent.2;
        let w = if force || (moved && rotated) {
            let mut w = Writer::with_id(sb::MOVE_PLAYER_POS_ROT); w.f64(b.pos[0]).f64(b.pos[1]).f64(b.pos[2]).f32(b.yaw).f32(b.pitch).u8(flags); w
        } else if moved {
            let mut w = Writer::with_id(sb::MOVE_PLAYER_POS); w.f64(b.pos[0]).f64(b.pos[1]).f64(b.pos[2]).u8(flags); w
        } else if rotated {
            let mut w = Writer::with_id(sb::MOVE_PLAYER_ROT); w.f32(b.yaw).f32(b.pitch).u8(flags); w
        } else if b.on_ground != self.last_sent.3 {
            let mut w = Writer::with_id(sb::MOVE_PLAYER_STATUS_ONLY); w.u8(flags); w
        } else { self.ticks_since_pos += 1; return };
        if force || moved { self.ticks_since_pos = 0; } else { self.ticks_since_pos += 1; }
        self.last_sent = (b.pos, b.yaw, b.pitch, b.on_ground);
        self.send(w);
    }

    fn client_tick(&mut self) {
        self.ticks += 1;
        if !self.spawned || self.dead || self.disconnected.is_some() { return; }
        if !self.loaded_sent && self.world.loaded(self.body.pos[0].floor() as i32, self.body.pos[2].floor() as i32) {
            self.send(Writer::with_id(sb::PLAYER_LOADED)); self.loaded_sent = true;
        }
        if let Some((g, tol)) = self.goal {
            let (dx, dz) = (g[0] - self.body.pos[0], g[1] - self.body.pos[2]);
            let d = (dx * dx + dz * dz).sqrt();
            // stop early enough that ground friction leaves us near the target
            let speed = (self.body.vel[0].powi(2) + self.body.vel[2].powi(2)).sqrt();
            if d < tol.max(speed * 1.2) { self.goal = None; self.input.forward = 0.0; self.input.jump = false; }
            else { self.body.yaw = (-dx).atan2(dz).to_degrees() as f32; self.input.forward = 1.0; self.input.jump = self.body.h_collide && self.body.on_ground; }
        }
        let i = self.input;
        let flags = (i.forward > 0.0) as u8 | ((i.forward < 0.0) as u8) << 1 | ((i.strafe > 0.0) as u8) << 2 | ((i.strafe < 0.0) as u8) << 3
            | (i.jump as u8) << 4 | (i.sneak as u8) << 5 | (i.sprint as u8) << 6;
        if flags != self.last_input { let mut w = Writer::with_id(sb::PLAYER_INPUT); w.u8(flags); self.send(w); self.last_input = flags; }
        if !self.frozen {
            let spectator = self.gamemode == 3;
            let world = std::mem::take(&mut self.world);
            self.body.tick(&world, i, spectator);
            self.world = world;
        }
        self.send_move(false);
        self.send(Writer::with_id(sb::CLIENT_TICK_END));
    }
}

pub struct Bot {
    pub name: String,
    pub addr: String,
    st: Arc<Mutex<State>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

pub struct Options { pub view: i8 }
impl Default for Options { fn default() -> Self { Options { view: 4 } } }

impl Bot {
    pub fn connect(addr: &str, name: &str) -> Result<Bot> { Self::connect_with(addr, name, Options::default()) }

    pub fn connect_with(addr: &str, name: &str, opts: Options) -> Result<Bot> {
        let (otx, mut orx) = unbounded_channel::<Vec<u8>>();
        let st = Arc::new(Mutex::new(State {
            name: name.to_string(), eid: -1, gamemode: 0, may_fly: false, body: Body::default(), input: Input::default(), goal: None, frozen: false,
            spawned: false, loaded_sent: false, dead: false, death_msg: None, health: 20.0, food: 20, saturation: 5.0, air: 300, ticks_frozen: 0, flags: 0,
            effects: HashMap::new(), inv: vec![Stack::default(); 46], cursor: Stack::default(), held: 0, state_id: 0, screen: None, container: Vec::new(),
            chat: Vec::new(), entities: HashMap::new(), players: HashMap::new(), world: World::default(), teleports: Vec::new(), plog: Vec::new(),
            counts: HashMap::new(), ticks: 0, disconnected: None, damage: Vec::new(), health_log: Vec::new(), taken: Vec::new(), explosions: Vec::new(), op_level: 0, entity_events: Vec::new(), registries: HashMap::new(), block_updates: 0,
            out: otx, last_sent: ([0.0; 3], 0.0, 0.0, false), ticks_since_pos: 0, last_input: 0, seq: 0,
        }));
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (st2, addr2, name2) = (st.clone(), addr.to_string(), name.to_string());
        rt().spawn(async move {
            let r = tokio::select! {
                r = run(&addr2, &name2, opts.view, st2.clone(), &mut orx) => r,
                _ = &mut stop_rx => Ok(()),
            };
            let mut s = st2.lock().unwrap();
            if s.disconnected.is_none() { s.disconnected = Some(match r { Ok(()) => "closed".into(), Err(e) => format!("{e:#}") }); }
        });
        let st3 = st.clone();
        rt().spawn(async move {
            let mut iv = tokio::time::interval(Duration::from_millis(50));
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                iv.tick().await;
                let mut s = st3.lock().unwrap();
                if s.disconnected.is_some() { break; }
                s.client_tick();
            }
        });
        let bot = Bot { name: name.to_string(), addr: addr.to_string(), st, stop: Some(stop_tx) };
        // spawned = first teleport received; then wait for the chunk underfoot
        let ok = bot.wait_until(Duration::from_secs(60), |s| s.disconnected.is_some() || (s.spawned && s.world.loaded(s.body.pos[0].floor() as i32, s.body.pos[2].floor() as i32)));
        if let Some(d) = bot.st().disconnected.clone() { bail!("{name} could not join {addr}: {d}"); }
        if !ok { bail!("{name}: timed out joining {addr}"); }
        Ok(bot)
    }

    pub fn st(&self) -> MutexGuard<'_, State> { self.st.lock().unwrap() }
    pub fn send(&self, w: Writer) { self.st().send(w); }

    pub fn wait_ticks(&self, n: u32) { std::thread::sleep(Duration::from_millis(50 * n as u64)); }

    /// Poll a condition every 10 ms until it holds (true) or the timeout passes (false).
    pub fn wait_until(&self, timeout: Duration, mut f: impl FnMut(&State) -> bool) -> bool {
        let t0 = Instant::now();
        loop {
            if f(&self.st()) { return true; }
            if t0.elapsed() > timeout { return false; }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn alive(&self) -> Result<()> { match &self.st().disconnected { Some(d) => Err(anyhow!("{} disconnected: {d}", self.name)), None => Ok(()) } }

    // ------------------------------------------------------------ chat & commands

    pub fn chat(&self, msg: &str) {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        let mut w = Writer::with_id(sb::CHAT);
        w.str(msg).i64(ts).i64(0).bool(false).varint(0).bytes(&[0, 0, 0]).u8(1);
        self.send(w);
    }

    /// Send a command without waiting for its reply.
    pub fn cmd(&self, cmd: &str) {
        let mut w = Writer::with_id(sb::CHAT_COMMAND); w.str(cmd.trim_start_matches('/')); self.send(w);
    }

    /// Send a command and collect reply lines (system messages that look like command feedback).
    pub fn cmd_reply(&self, cmd: &str) -> Result<Vec<Text>> {
        self.alive()?;
        let mark = self.st().chat.len();
        self.cmd(cmd);
        let is_reply = |t: &Text| match &t.key {
            Some(k) => !(k.starts_with("chat.type") || k.starts_with("multiplayer.") || k.starts_with("death.")),
            None => !t.plain.starts_with('<') && !t.plain.ends_with("joined the game") && !t.plain.ends_with("left the game") && !t.plain.starts_with('['),
        };
        let got = self.wait_until(Duration::from_secs(5), |s| s.chat[mark..].iter().any(|l| is_reply(&l.text)));
        if !got { bail!("no reply to /{cmd}"); }
        std::thread::sleep(Duration::from_millis(60));
        Ok(self.st().chat[mark..].iter().filter(|l| is_reply(&l.text)).map(|l| l.text.clone()).collect())
    }

    /// Run setup commands in order, waiting for each reply.
    pub fn cmds(&self, cmds: &[String]) -> Result<()> { for c in cmds { self.cmd_reply(c)?; } Ok(()) }

    pub fn chat_since(&self, mark: usize) -> Vec<Text> { let s = self.st(); s.chat[mark.min(s.chat.len())..].iter().map(|l| l.text.clone()).collect() }
    pub fn chat_mark(&self) -> usize { self.st().chat.len() }

    // ------------------------------------------------------------ movement

    /// Teleport via /tp and wait until the server's position packet arrives and the chunk is loaded.
    pub fn tp(&self, p: [f64; 3]) -> Result<()> {
        let n = self.st().teleports.len();
        self.cmd(&format!("tp @s {:.4} {:.4} {:.4}", p[0], p[1], p[2])); // explicit decimals: vanilla centres integer x/z
        let ok = self.wait_until(Duration::from_secs(10), |s| s.teleports[n.min(s.teleports.len())..].iter().any(|(_, q)| (0..3).all(|i| (q[i] - p[i]).abs() < 0.01)));
        if !ok { bail!("teleport to {p:?} not confirmed"); }
        self.wait_chunks(1, Duration::from_secs(30))?;
        Ok(())
    }

    /// Wait for all chunks within `r` of the player to arrive.
    pub fn wait_chunks(&self, r: i32, timeout: Duration) -> Result<()> {
        let ok = self.wait_until(timeout, |s| {
            let (cx, cz) = ((s.body.pos[0].floor() as i32) >> 4, (s.body.pos[2].floor() as i32) >> 4);
            (-r..=r).all(|dx| (-r..=r).all(|dz| s.world.chunks.contains_key(&(cx + dx, cz + dz))))
        });
        if !ok { bail!("chunks around {:?} did not arrive", self.st().body.pos); }
        Ok(())
    }

    pub fn set_frozen(&self, f: bool) { let mut s = self.st(); s.frozen = f; if f { s.body.vel = [0.0; 3]; } }
    pub fn fly(&self, on: bool) {
        let mut s = self.st(); s.body.flying = on; s.body.vel = [0.0; 3];
        let mut w = Writer::with_id(sb::PLAYER_ABILITIES); w.u8(if on { 2 } else { 0 }); s.send(w);
    }
    pub fn look(&self, yaw: f32, pitch: f32) { let mut s = self.st(); s.body.yaw = yaw; s.body.pitch = pitch; }
    pub fn look_at(&self, t: [f64; 3]) {
        let mut s = self.st();
        let e = s.body.eye();
        let (dx, dy, dz) = (t[0] - e[0], t[1] - e[1], t[2] - e[2]);
        s.body.yaw = (-dx).atan2(dz).to_degrees() as f32;
        s.body.pitch = (-dy.atan2((dx * dx + dz * dz).sqrt())).to_degrees() as f32;
    }
    pub fn sneak(&self, on: bool) { self.st().input.sneak = on; }
    pub fn sprint(&self, on: bool) {
        let mut s = self.st(); s.input.sprint = on;
        let mut w = Writer::with_id(sb::PLAYER_COMMAND); w.varint(s.eid).varint(if on { 1 } else { 2 }).varint(0); s.send(w);
    }
    pub fn jump(&self) -> Result<()> {
        if !self.wait_until(Duration::from_secs(3), |s| s.body.on_ground) { bail!("not on ground, can't jump"); }
        self.st().input.jump = true;
        self.wait_until(Duration::from_secs(1), |s| !s.body.on_ground);
        self.st().input.jump = false;
        Ok(())
    }

    /// Walk (steering by yaw) to the x/z target; returns when within tolerance and stopped.
    pub fn walk_to(&self, x: f64, z: f64, timeout: Duration) -> Result<()> {
        self.st().goal = Some(([x, z], 0.25));
        let ok = self.wait_until(timeout, |s| s.goal.is_none() || s.dead || s.disconnected.is_some());
        if !ok { self.st().goal = None; self.st().input.forward = 0.0; bail!("walk to {x},{z} timed out at {:?}", self.st().body.pos); }
        self.alive()?;
        // let friction stop us
        self.wait_until(Duration::from_secs(2), |s| s.body.vel[0].abs() + s.body.vel[2].abs() < 1e-3);
        Ok(())
    }
    pub fn walk_by(&self, dx: f64, dz: f64, timeout: Duration) -> Result<()> {
        let p = self.st().body.pos; self.walk_to(p[0] + dx, p[2] + dz, timeout)
    }
    pub fn wait_on_ground(&self, timeout: Duration) -> bool { self.wait_until(timeout, |s| s.body.on_ground && s.body.vel[1].abs() < 0.1) }

    // ------------------------------------------------------------ interaction

    fn next_seq(&self) -> i32 { let mut s = self.st(); s.seq += 1; s.seq }

    pub fn player_action(&self, status: i32, b: [i32; 3], face: u8) {
        let seq = self.next_seq();
        let mut w = Writer::with_id(sb::PLAYER_ACTION); w.varint(status).position(b[0], b[1], b[2]).u8(face).varint(seq); self.send(w);
    }
    /// Attack (left-click) an entity.
    pub fn attack(&self, eid: i32) {
        let mut w = Writer::with_id(sb::ATTACK); w.varint(eid); self.send(w); self.swing();
    }
    pub fn swing(&self) { let mut w = Writer::with_id(sb::SWING); w.varint(0); self.send(w); }

    /// Break a block like a client: start digging, wait the survival dig time for the held
    /// item (from basalt-game), finish. Creative breaks on start. Returns the ticks waited.
    pub fn dig(&self, b: [i32; 3]) -> Result<u32> {
        self.look_at([b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64 + 0.5]);
        self.wait_ticks(2);
        let (state, held, on_ground, creative, in_water) = {
            let s = self.st();
            let h = s.held_stack(); let eye = s.body.eye();
            let w = basalt_world::props::props(s.world.get(eye[0].floor() as i32, eye[1].floor() as i32, eye[2].floor() as i32)).has_fluid();
            (s.world.get(b[0], b[1], b[2]), (h.count > 0).then_some(h.item), s.body.on_ground, s.gamemode == 1, w)
        };
        let face = 1u8;
        self.player_action(0, b, face); self.swing();
        if creative { return Ok(0); }
        let t = crate::game_data().dig_ticks(state, held, on_ground, in_water).ok_or_else(|| anyhow!("{} is unbreakable", basalt_data::BlockState(state).name()))?;
        if t == 0 { return Ok(0); }
        for _ in 0..(t + 1) { self.wait_ticks(1); if t > 5 { self.swing(); } }
        self.player_action(2, b, face);
        Ok(t + 1)
    }

    /// Place the held block against `against` on face `face` (0 down,1 up,2 north,3 south,4 west,5 east).
    pub fn place(&self, against: [i32; 3], face: u8) {
        let off = [[0.5, 0.0, 0.5], [0.5, 1.0, 0.5], [0.5, 0.5, 0.0], [0.5, 0.5, 1.0], [0.0, 0.5, 0.5], [1.0, 0.5, 0.5]][face as usize];
        self.place_at(against, face, [off[0] as f32, off[1] as f32, off[2] as f32]);
    }

    /// use_item_on with an explicit cursor position inside the clicked block.
    pub fn place_at(&self, against: [i32; 3], face: u8, off: [f32; 3]) {
        let seq = self.next_seq();
        let mut w = Writer::with_id(sb::USE_ITEM_ON);
        w.varint(0).position(against[0], against[1], against[2]).varint(face as i32).f32(off[0]).f32(off[1]).f32(off[2]).bool(false).bool(false).varint(seq);
        self.send(w); self.swing();
    }

    pub fn use_item(&self) {
        let seq = self.next_seq();
        let (y, p) = { let s = self.st(); (s.body.yaw, s.body.pitch) };
        let mut w = Writer::with_id(sb::USE_ITEM); w.varint(0).varint(seq).f32(y).f32(p); self.send(w);
    }
    pub fn release_use(&self) { self.player_action(5, [0, 0, 0], 0); }
    /// Q (one item) or ctrl-Q (whole stack).
    pub fn drop_held(&self, whole: bool) {
        // like the real client, predict the removal (vanilla doesn't resend the slot)
        { let mut s = self.st(); let i = 36 + s.held as usize; let n = if whole { s.inv[i].count } else { 1 }; s.inv[i].count -= n; if s.inv[i].count <= 0 { s.inv[i] = Stack::default(); } }
        self.player_action(if whole { 3 } else { 4 }, [0, 0, 0], 0);
    }
    pub fn set_held(&self, slot: u8) { self.st().held = slot; let mut w = Writer::with_id(sb::SET_CARRIED_ITEM); w.i16(slot as i16); self.send(w); }
    pub fn respawn(&self) { let mut w = Writer::with_id(sb::CLIENT_COMMAND); w.varint(0); self.send(w); }

    /// container_click in the open window (inventory if none): slot, button, mode.
    pub fn click(&self, slot: i16, button: i8, mode: i32) {
        let s = self.st();
        let window = s.screen.as_ref().map(|x| x.0).unwrap_or(0);
        let mut w = Writer::with_id(sb::CONTAINER_CLICK);
        w.varint(window).varint(s.state_id).i16(slot).i8(button).varint(mode).varint(0);
        proto::write_hashed(&mut w, &s.cursor);
        s.send(w);
    }
    pub fn close_container(&self) {
        let mut s = self.st();
        let window = s.screen.take().map(|x| x.0).unwrap_or(0);
        let mut w = Writer::with_id(sb::CONTAINER_CLOSE); w.varint(window); s.send(w);
    }

    pub fn quit(mut self) { if let Some(s) = self.stop.take() { let _ = s.send(()); } std::thread::sleep(Duration::from_millis(100)); }
}

impl Drop for Bot { fn drop(&mut self) { if let Some(s) = self.stop.take() { let _ = s.send(()); } } }

// ------------------------------------------------------------ network task

async fn run(addr: &str, name: &str, view: i8, st: Arc<Mutex<State>>, out: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) -> Result<()> {
    let (host, port) = addr.rsplit_once(':').map(|(h, p)| (h.to_string(), p.parse().unwrap_or(25565))).unwrap_or((addr.to_string(), 25565));
    let mut c = Conn::new(TcpStream::connect((host.as_str(), port)).await.with_context(|| format!("connecting to {addr}"))?);
    let mut w = Writer::with_id(0); w.varint(PROTOCOL_VERSION).str(&host).u16(port).varint(2); c.send(&w).await?;
    let mut w = Writer::with_id(ids::login::serverbound::HELLO); w.str(name).uuid(offline_uuid(name)); c.send(&w).await?;
    loop {
        let f = c.read().await?; let mut r = f.reader();
        match f.id {
            ids::login::clientbound::LOGIN_COMPRESSION => { let t = r.varint()?; c.set_threshold(t); }
            ids::login::clientbound::LOGIN_FINISHED => { c.send(&Writer::with_id(ids::login::serverbound::LOGIN_ACKNOWLEDGED)).await?; break; }
            ids::login::clientbound::LOGIN_DISCONNECT => bail!("login rejected: {}", r.str()?),
            ids::login::clientbound::HELLO => bail!("server is in online mode"),
            ids::login::clientbound::CUSTOM_QUERY => { let id = r.varint()?; let mut w = Writer::with_id(ids::login::serverbound::CUSTOM_QUERY_ANSWER); w.varint(id).bool(false); c.send(&w).await?; }
            _ => {}
        }
    }
    'session: loop {
        use ids::configuration::clientbound as cc; use ids::configuration::serverbound as cs;
        let mut w = Writer::with_id(cs::CLIENT_INFORMATION);
        w.str("en_us").i8(view).varint(0).bool(true).u8(0x7f).varint(1).bool(false).bool(true).varint(0); c.send(&w).await?;
        let mut w = Writer::with_id(cs::CUSTOM_PAYLOAD); w.str("minecraft:brand").str("basalt-check"); c.send(&w).await?;
        loop {
            let f = c.read().await?; let mut r = f.reader();
            match f.id {
                cc::SELECT_KNOWN_PACKS => { let mut w = Writer::with_id(cs::SELECT_KNOWN_PACKS); w.bytes(&f.data); c.send(&w).await?; }
                cc::REGISTRY_DATA => {
                    let reg = r.str()?; let n = r.varint()?; let mut names = Vec::new();
                    for _ in 0..n { names.push(r.str()?); if r.bool()? { r.nbt()?; } }
                    st.lock().unwrap().registries.insert(reg, names);
                }
                cc::KEEP_ALIVE => { let v = r.i64()?; let mut w = Writer::with_id(cs::KEEP_ALIVE); w.i64(v); c.send(&w).await?; }
                cc::PING => { let v = r.i32()?; let mut w = Writer::with_id(cs::PONG); w.i32(v); c.send(&w).await?; }
                cc::DISCONNECT => bail!("kicked during configuration: {}", proto::flatten(&r.nbt()?)),
                cc::FINISH_CONFIGURATION => { c.send(&Writer::with_id(cs::FINISH_CONFIGURATION)).await?; break; }
                _ => {}
            }
        }
        loop {
            tokio::select! {
                o = out.recv() => match o { Some(buf) => c.w.send(&Writer { buf }).await?, None => return Ok(()) },
                f = c.r.read() => {
                    let f = f?;
                    match f.id {
                        cb::KEEP_ALIVE => { let v = f.reader().i64()?; let mut w = Writer::with_id(sb::KEEP_ALIVE); w.i64(v); c.w.send(&w).await?; }
                        cb::PING => { let v = f.reader().i32()?; let mut w = Writer::with_id(sb::PONG); w.i32(v); c.w.send(&w).await?; }
                        cb::START_CONFIGURATION => { c.w.send(&Writer::with_id(sb::CONFIGURATION_ACKNOWLEDGED)).await?; continue 'session; }
                        cb::DISCONNECT => bail!("kicked: {}", proto::flatten(&f.reader().nbt()?)),
                        _ => {
                            let mut s = st.lock().unwrap();
                            s.bump(f.id);
                            if let Err(e) = handle(&mut s, f.id, &f.data) { log::debug!("{name}: decode {}: {e:#}", cb::name(f.id)); }
                        }
                    }
                }
            }
        }
    }
}

fn inv_index_from_player_slot(slot: i32) -> Option<usize> {
    match slot { 0..=8 => Some(36 + slot as usize), 9..=35 => Some(slot as usize), 36 => Some(8), 37 => Some(7), 38 => Some(6), 39 => Some(5), 40 => Some(45), _ => None }
}

fn handle(s: &mut State, id: i32, data: &[u8]) -> Result<()> {
    let mut r = Reader::new(data);
    match id {
        cb::LOGIN => {
            s.eid = r.i32()?; r.bool()?;
            let n = r.varint()?; for _ in 0..n { r.str()?; }
            r.varint()?; r.varint()?; r.varint()?; r.bool()?; r.bool()?; r.bool()?;
            r.varint()?; r.str()?; r.i64()?; s.gamemode = r.u8()?;
        }
        cb::RESPAWN => {
            r.varint()?; r.str()?; r.i64()?; s.gamemode = r.u8()?;
            s.world = World::default(); s.entities.clear(); s.dead = false; s.spawned = false; s.loaded_sent = false;
            s.body.vel = [0.0; 3]; s.health = 20.0; s.effects.clear();
        }
        cb::CHUNK_BATCH_FINISHED => { let mut w = Writer::with_id(sb::CHUNK_BATCH_RECEIVED); w.f32(25.0); s.send(w); }
        cb::LEVEL_CHUNK_WITH_LIGHT => { let cx = r.i32()?; let cz = r.i32()?; let c = Column::decode(&mut r)?; s.world.chunks.insert((cx, cz), c); }
        cb::FORGET_LEVEL_CHUNK => { let cz = r.i32()?; let cx = r.i32()?; s.world.chunks.remove(&(cx, cz)); }
        cb::BLOCK_UPDATE => { let (x, y, z) = r.position()?; let st = r.varint()?; s.world.set(x, y, z, st as u32); s.block_updates += 1; }
        cb::SECTION_BLOCKS_UPDATE => {
            let p = r.i64()?;
            let (sx, sz, sy) = ((p >> 42) as i32, (p << 22 >> 42) as i32, (p << 44 >> 44) as i32);
            let n = r.varint()?;
            for _ in 0..n {
                let v = r.varlong()?; let l = (v & 0xFFF) as i32;
                s.world.set(sx * 16 + (l >> 8 & 15), sy * 16 + (l & 15), sz * 16 + (l >> 4 & 15), (v >> 12) as u32);
                s.block_updates += 1;
            }
        }
        cb::PLAYER_POSITION => {
            let tid = r.varint()?;
            let pos = [r.f64()?, r.f64()?, r.f64()?]; let vel = [r.f64()?, r.f64()?, r.f64()?];
            let (yaw, pitch, flags) = (r.f32()?, r.f32()?, r.i32()?);
            let b = &mut s.body;
            for i in 0..3 { b.pos[i] = if flags >> i & 1 == 1 { b.pos[i] + pos[i] } else { pos[i] }; }
            b.yaw = if flags & 8 != 0 { b.yaw + yaw } else { yaw };
            b.pitch = if flags & 16 != 0 { b.pitch + pitch } else { pitch };
            for i in 0..3 { b.vel[i] = if flags >> (5 + i) & 1 == 1 { b.vel[i] + vel[i] } else { vel[i] }; }
            let p = b.pos;
            s.teleports.push((Instant::now(), p));
            let mut w = Writer::with_id(sb::ACCEPT_TELEPORTATION); w.varint(tid); s.send(w);
            s.send_move(true);
            s.spawned = true;
        }
        cb::SYSTEM_CHAT => {
            let t = r.nbt()?; let overlay = r.bool().unwrap_or(false);
            if !overlay { s.chat.push(Line { at: Instant::now(), text: proto::text_of(&t) }); }
        }
        cb::DISGUISED_CHAT => { let t = r.nbt()?; s.chat.push(Line { at: Instant::now(), text: proto::text_of(&t) }); }
        cb::PLAYER_CHAT => {
            let _g = r.varint()?; let sender = r.uuid()?; let _i = r.varint()?;
            if r.bool()? { r.take(256)?; }
            let msg = r.str()?;
            let name = s.players.get(&sender).cloned().unwrap_or_else(|| "?".into());
            s.chat.push(Line { at: Instant::now(), text: Text { plain: format!("<{name}> {msg}"), key: None, args: vec![name, msg] } });
        }
        cb::PLAYER_COMBAT_KILL => { let e = r.varint()?; let m = proto::text_of(&r.nbt()?); if e == s.eid { s.dead = true; s.death_msg = Some(m); } }
        cb::SET_HEALTH => { s.health = r.f32()?; s.food = r.varint()?; s.saturation = r.f32()?; let h = s.health; s.health_log.push((Instant::now(), h)); }
        cb::GAME_EVENT => { let e = r.u8()?; let v = r.f32()?; if e == 3 { s.gamemode = v as u8; } }
        cb::PLAYER_ABILITIES => { let f = r.u8()?; s.body.flying = f & 2 != 0; s.may_fly = f & 4 != 0; }
        cb::SET_HELD_SLOT => { s.held = r.varint()?.clamp(0, 8) as u8; }
        cb::CONTAINER_SET_CONTENT => {
            let window = r.varint()?; s.state_id = r.varint()?; let n = r.varint()?;
            let mut items = Vec::with_capacity(n as usize);
            for _ in 0..n { items.push(proto::read_slot(&mut r)?); }
            s.cursor = proto::read_slot(&mut r)?;
            if window == 0 { for (i, it) in items.into_iter().enumerate().take(46) { s.inv[i] = it; } }
            else {
                let base = items.len().saturating_sub(36);
                for (k, it) in items.iter().enumerate().skip(base) { s.inv[9 + k - base] = it.clone(); }
                s.container = items;
            }
        }
        cb::CONTAINER_SET_SLOT => {
            let window = r.varint()?; s.state_id = r.varint()?; let slot = r.i16()? as i32; let it = proto::read_slot(&mut r)?;
            if window == 0 { if (0..46).contains(&slot) { s.inv[slot as usize] = it; } }
            else if window == -1 { s.cursor = it; }
            else if s.screen.as_ref().map(|x| x.0) == Some(window) {
                let n = s.container.len();
                if (slot as usize) < n {
                    let base = n.saturating_sub(36);
                    if slot as usize >= base { s.inv[9 + slot as usize - base] = it.clone(); }
                    s.container[slot as usize] = it;
                }
            }
        }
        cb::SET_CURSOR_ITEM => { s.cursor = proto::read_slot(&mut r)?; }
        cb::SET_PLAYER_INVENTORY => { let slot = r.varint()?; let it = proto::read_slot(&mut r)?; if let Some(i) = inv_index_from_player_slot(slot) { s.inv[i] = it; } }
        cb::OPEN_SCREEN => {
            let w = r.varint()?; let menu = r.varint()?;
            let name = basalt_data::registries::MENU.get(menu as usize).copied().unwrap_or("?").to_string();
            s.screen = Some((w, name)); s.container.clear();
        }
        cb::CONTAINER_CLOSE => { s.screen = None; }
        cb::UPDATE_MOB_EFFECT => {
            let e = r.varint()?; let eff = r.varint()?; let amp = r.varint()?; let dur = r.varint()?;
            if e == s.eid { let n = basalt_data::registries::MOB_EFFECT.get(eff as usize).copied().unwrap_or("?"); s.effects.insert(n.to_string(), (amp, dur)); }
        }
        cb::REMOVE_MOB_EFFECT => {
            let e = r.varint()?; let eff = r.varint()?;
            if e == s.eid { let n = basalt_data::registries::MOB_EFFECT.get(eff as usize).copied().unwrap_or("?"); s.effects.remove(n); }
        }
        cb::SET_ENTITY_DATA => {
            let e = r.varint()?;
            let meta = proto::read_meta(&mut r);
            if e == s.eid {
                for (i, m) in meta {
                    match (i, m) { (0, Meta::Byte(b)) => s.flags = b, (1, Meta::Int(v)) => s.air = v, (7, Meta::Int(v)) => s.ticks_frozen = v, _ => {} }
                }
            } else if let Some(ent) = s.entities.get_mut(&e) {
                for (i, m) in meta {
                    match (i, &m) {
                        (8, Meta::Item(st)) if ent.kind == "item" => ent.item = Some(st.clone()),
                        (0, Meta::Byte(b)) => ent.flags = *b,
                        (9, Meta::Float(h)) => ent.health = Some(*h),
                        _ => {}
                    }
                    ent.meta.insert(i, m);
                }
            }
            s.plog.push(PacketLog { at: Instant::now(), id, eid: e });
        }
        cb::DAMAGE_EVENT => {
            let e = r.varint()?; let ty = r.varint()?;
            if e == s.eid {
                let n = s.registries.get("minecraft:damage_type").and_then(|v| v.get(ty as usize)).cloned().unwrap_or_else(|| ty.to_string());
                s.damage.push((Instant::now(), n.trim_start_matches("minecraft:").to_string()));
            }
        }
        cb::PLAYER_INFO_UPDATE => {
            let actions = r.u8()?; let n = r.varint()?;
            if actions & 1 != 0 {
                for _ in 0..n {
                    let uuid = r.uuid()?; let name = r.str()?;
                    let np = r.varint()?; for _ in 0..np { r.str()?; r.str()?; if r.bool()? { r.str()?; } }
                    s.players.insert(uuid, name);
                    if actions & 2 != 0 && r.bool()? { r.uuid()?; r.i64()?; r.byte_array()?; r.byte_array()?; }
                    if actions & 4 != 0 { r.varint()?; }
                    if actions & 8 != 0 { r.bool()?; }
                    if actions & 16 != 0 { r.varint()?; }
                    if actions & 32 != 0 && r.bool()? { r.nbt()?; }
                    if actions & 64 != 0 { r.varint()?; }
                    if actions & 128 != 0 { r.bool()?; }
                }
            }
        }
        cb::ADD_ENTITY => {
            let e = r.varint()?; let uuid = r.uuid()?; let kind = r.varint()?;
            let pos = [r.f64()?, r.f64()?, r.f64()?];
            let vel = proto::read_lpvec3(&mut r)?;
            let kind = basalt_data::registries::ENTITY_TYPE.get(kind as usize).copied().unwrap_or("?");
            s.entities.insert(e, Entity { uuid, kind, pos, spawn_pos: pos, spawn_vel: vel, vel, item: None, on_ground: false, added: Instant::now(), flags: 0, health: None, meta: HashMap::new() });
            s.plog.push(PacketLog { at: Instant::now(), id, eid: e });
        }
        cb::MOVE_ENTITY_POS | cb::MOVE_ENTITY_POS_ROT => {
            let e = r.varint()?; let d = [r.i16()? as f64 / 4096.0, r.i16()? as f64 / 4096.0, r.i16()? as f64 / 4096.0];
            if id == cb::MOVE_ENTITY_POS_ROT { r.u8()?; r.u8()?; }
            let g = r.bool()?;
            if let Some(en) = s.entities.get_mut(&e) { for i in 0..3 { en.pos[i] += d[i]; } en.on_ground = g; }
            s.plog.push(PacketLog { at: Instant::now(), id, eid: e });
        }
        cb::ENTITY_POSITION_SYNC => {
            let e = r.varint()?; let pos = [r.f64()?, r.f64()?, r.f64()?]; let vel = [r.f64()?, r.f64()?, r.f64()?];
            r.f32()?; r.f32()?; let g = r.bool()?;
            if let Some(en) = s.entities.get_mut(&e) { en.pos = pos; en.vel = vel; en.on_ground = g; }
            s.plog.push(PacketLog { at: Instant::now(), id, eid: e });
        }
        cb::SET_ENTITY_MOTION => {
            let e = r.varint()?; let v = proto::read_lpvec3(&mut r)?;
            if e == s.eid { s.body.vel = v; } else if let Some(en) = s.entities.get_mut(&e) { en.vel = v; }
            s.plog.push(PacketLog { at: Instant::now(), id, eid: e });
        }
        cb::REMOVE_ENTITIES => {
            let n = r.varint()?;
            for _ in 0..n { let e = r.varint()?; s.entities.remove(&e); s.plog.push(PacketLog { at: Instant::now(), id, eid: e }); }
        }
        cb::EXPLODE => {
            let pos = [r.f64()?, r.f64()?, r.f64()?]; let radius = r.f32()?; let blocks = r.i32()?;
            let knockback = if r.bool()? { Some([r.f64()?, r.f64()?, r.f64()?]) } else { None };
            if let Some(k) = knockback { for i in 0..3 { s.body.vel[i] += k[i]; } }
            s.explosions.push(Explosion { at: Instant::now(), pos, radius, blocks, knockback });
        }
        cb::ENTITY_EVENT => { let e = r.i32()?; let st = r.u8()?; if e == s.eid && (24..=28).contains(&st) { s.op_level = st - 24; } s.entity_events.push((Instant::now(), e, st)); }
        cb::TAKE_ITEM_ENTITY => { let item = r.varint()?; let by = r.varint()?; let n = r.varint()?; s.taken.push((item, by, n)); }
        _ => {}
    }
    Ok(())
}
