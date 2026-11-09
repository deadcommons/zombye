//! What goes over the network between a client and a multiplayer server (#21): the messages, how
//! they are framed, and snapshots cut down to what one client needs.
//!
//! **Transport:** one TCP connection per client (`TCP_NODELAY`), carrying frames: a `u32`
//! little-endian length, then that many bytes of postcard (the save files' encoding). TCP because
//! it needs nothing but the standard library, crosses home routers outward without help, and its
//! ordered, reliable delivery lets every snapshot be a *delta* against the one before with no
//! acknowledgements. Its cost is head-of-line blocking on a lossy link; the messages
//! ([`ClientMsg`], [`ServerMsg`]) don't care what carries them, so UDP or QUIC can replace it
//! behind [`crate::remote::Remote`] and [`crate::host::Host`] (docs/PLAN.md §2).
//!
//! **Joining:** the client sends [`Hello`]; the server answers [`ServerMsg::Welcome`] or
//! [`ServerMsg::Refused`]. The map isn't sent (a 1 km map is a million tiles): the client builds
//! it from the seed with its own copy of the game data, as loading a save does, checks its hash
//! against the server's, and applies the doors and windows changed since ([`Welcome::map_edits`]).
//!
//! **Snapshots:** each tick the server captures one [`Snapshot`], cuts it down per client
//! ([`Snapshot::for_client`]: what's within the interest radius of that client's survivor, and
//! only its own private parts) and sends what changed since the last one it sent that client
//! ([`DeltaEncoder`] / [`DeltaDecoder`]). Zombies, the bulk of it, are packed to about 11 bytes
//! ([`ZombiePacked`]). The proper interest management and visibility rules are #22 and #66.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zombye_sim::map::EdgeChange;
use zombye_sim::save::ContentPrint;
use zombye_sim::worldgen::GenConfig;
use zombye_sim::{CarryTuning, CombatEvent, CombatTuning, GroundItem, HealthTuning, InfectionTuning, ItemNote, LevelUp, NeedsTuning, RunStats, SkillTuning, ZombieAnim};

use crate::{EdgeDamage, FloorView, Intent, OutfitView, PlayerView, PrivateView, RunView, Snapshot, ZombieView};

/// Bumped whenever a message changes shape; a server refuses any other.
pub const PROTOCOL: u32 = 1;
/// The port a server listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 7777;
/// The most players a server takes (the design size, docs/PLAN.md §4.8), and the default.
pub const MAX_PLAYERS: usize = 64;
pub const DEFAULT_PLAYERS: usize = 32;

/// The largest frame each side accepts from the other. A client's messages are tiny.
pub const MAX_CLIENT_FRAME: usize = 4 * 1024;
pub const MAX_SERVER_FRAME: usize = 8 * 1024 * 1024;

/// Client to server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// First, once.
    Hello(Hello),
    /// Something the client's player wants to do. The server makes it that player's whatever
    /// `player` it names.
    Intent(Intent),
    /// Answered with [`ServerMsg::Pong`] carrying the same number (the client's clock, for ping).
    Ping(u32),
    /// Leaving.
    Bye,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    /// Who this is. A name whose survivor is already in the world and not being played takes that
    /// survivor up again. Empty: the server makes one up.
    pub name: String,
    /// The server's join password, if it has one.
    pub password: String,
}

/// Server to client.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome(Box<Welcome>),
    /// Not let in, and why (wrong version or password, full, name in use).
    Refused(String),
    Snapshot(Box<SnapshotDelta>),
    Pong(u32),
    /// The server is closing this connection, and why.
    Bye(String),
}

/// A server's rules (docs/PLAN.md §4.8; the rest, and the server browser, are #69).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    /// The server's name.
    pub name: String,
    /// Players can hurt each other. Off: co-op.
    pub pvp: bool,
    pub max_players: u32,
    /// Dev toggles and debug intents are taken.
    pub allow_dev: bool,
}

/// The numbers a client needs to predict and to label things as the server does.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tunings {
    pub combat: CombatTuning,
    pub health: HealthTuning,
    pub infection: InfectionTuning,
    pub needs: NeedsTuning,
    pub skills: SkillTuning,
    pub carry: CarryTuning,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Welcome {
    pub protocol: u32,
    /// The client's player and the name it goes by.
    pub player: u32,
    pub name: String,
    pub rules: Rules,
    /// The map: generated from this with the client's own game data...
    pub generation: GenConfig,
    /// ...which must be the server's (the fingerprint saves use) and give this map hash...
    pub content: ContentPrint,
    pub map_hash: u64,
    /// ...then the doors and windows opened, shut or broken since, and the cracks.
    pub map_edits: Vec<EdgeChange>,
    pub edge_damage: Vec<EdgeDamage>,
    pub tunings: Tunings,
}

/// A zombie as sent: position in 1/64 m, facing in 1/256 turns, speed in 1/48 m/s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZombiePacked {
    pub id: u32,
    pub x: u32,
    pub y: u32,
    pub facing: u8,
    pub speed: u8,
    pub anim: ZombieAnim,
}

/// A zombie the client already has that only moved: about 4 bytes. `gap` is its id less the id of
/// the move before it in the list (the first: its id), ids rising; `dx`, `dy` are in
/// [`ZombiePacked`]'s position steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZombieMove {
    pub gap: u32,
    pub dx: i8,
    pub dy: i8,
    pub facing: u8,
}

const POS_STEPS: f32 = 64.0;
const SPEED_STEPS: f32 = 48.0;

impl ZombiePacked {
    pub fn pack(z: &ZombieView) -> Self {
        let turn = z.facing.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        Self {
            id: z.id,
            x: (z.pos[0].max(0.0) * POS_STEPS).round() as u32,
            y: (z.pos[1].max(0.0) * POS_STEPS).round() as u32,
            facing: ((turn * 256.0).round() as u32 & 255) as u8,
            speed: (z.speed.max(0.0) * SPEED_STEPS).round().min(255.0) as u8,
            anim: z.anim,
        }
    }

    pub fn unpack(&self) -> ZombieView {
        ZombieView {
            id: self.id,
            pos: [self.x as f32 / POS_STEPS, self.y as f32 / POS_STEPS],
            facing: self.facing as f32 / 256.0 * std::f32::consts::TAU,
            speed: self.speed as f32 / SPEED_STEPS,
            anim: self.anim,
        }
    }
}

/// A [`Snapshot`] for one client, as the changes since the last one sent to it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SnapshotDelta {
    pub tick: u64,
    pub time_of_day: f32,
    pub time_scale: f32,
    pub game_minute: i32,
    /// Every player in view, whole (few, and ours must be exact for prediction).
    pub players: Vec<PlayerView>,
    /// Zombies new to this client or changed, the ids of those no longer in view, and those that
    /// only moved (applied after the others).
    pub zombies: Vec<ZombiePacked>,
    pub zombies_gone: Vec<u32>,
    pub zombie_moves: Vec<ZombieMove>,
    /// Ground items new or changed, and the ids of those gone.
    pub items: Vec<GroundItem>,
    pub items_gone: Vec<u32>,
    // Things that happened (each once).
    pub edge_changes: Vec<EdgeChange>,
    pub edge_damage: Vec<EdgeDamage>,
    pub hits: Vec<CombatEvent>,
    pub private: Vec<PrivateView>,
    pub item_notes: Vec<ItemNote>,
    pub level_ups: Vec<LevelUp>,
    /// Sent when they change (the run about once a second while only its clocks move).
    pub floor: Option<Vec<u32>>,
    pub run: Option<RunStats>,
    pub outfits: Option<Vec<OutfitView>>,
    pub risen: Option<Vec<OutfitView>>,
}

impl Snapshot {
    /// This snapshot as the client playing `player` gets it over the network: its own private
    /// parts ([`Snapshot::for_player`]), and of the world only what is within `radius` metres of
    /// its survivor: zombies, other players, items on the ground, hits. Map changes all go: the
    /// client's map copy must stay right wherever it walks next. With no such player, nothing is
    /// cut by distance.
    pub fn for_client(&self, player: u32, radius: f32) -> Snapshot {
        let me: Option<glam::Vec2> = self.players.iter().find(|p| p.id == player).map(|p| p.pos.into());
        let near = |pos: glam::Vec2| me.is_none_or(|m| m.distance_squared(pos) <= radius * radius);
        let players: Vec<PlayerView> = self.players.iter().filter(|p| p.id == player || near(p.pos.into())).copied().collect();
        let zombies: Vec<ZombieView> = self.zombies.iter().filter(|z| near(z.pos.into())).copied().collect();
        let seen = |id: u32| players.iter().any(|p| p.id == id);
        Snapshot {
            tick: self.tick,
            time_of_day: self.time_of_day,
            time_scale: self.time_scale,
            game_minute: self.game_minute,
            outfits: self.outfits.iter().filter(|o| seen(o.id)).cloned().collect(),
            risen: self.risen.iter().filter(|o| zombies.binary_search_by_key(&o.id, |z| z.id).is_ok()).cloned().collect(),
            players,
            zombies,
            edge_changes: self.edge_changes.clone(),
            edge_damage: self.edge_damage.clone(),
            items: self.items.iter().filter(|i| near(i.pos)).copied().collect(),
            hits: self.hits.iter().filter(|h| near(h.pos)).copied().collect(),
            private: self.private.iter().filter(|v| v.player == player).cloned().collect(),
            item_notes: self.item_notes.iter().filter(|n| n.player == player).cloned().collect(),
            floor: self.floor.iter().filter(|f| f.player == player).cloned().collect(),
            runs: self.runs.iter().filter(|r| r.player == player).cloned().collect(),
            level_ups: self.level_ups.iter().filter(|l| l.player == player).copied().collect(),
        }
    }
}

/// How often a run whose clocks alone moved is sent again (ticks).
const RUN_EVERY: u64 = 20;

/// The server's memory of what one client has: turns each snapshot for it into a delta.
#[derive(Default)]
pub struct DeltaEncoder {
    started: bool,
    zombies: Vec<ZombiePacked>,
    items: Vec<GroundItem>,
    floor: Vec<u32>,
    run: Option<RunStats>,
    outfits: Vec<OutfitView>,
    risen: Vec<OutfitView>,
}

/// The entries of `next` that are new or differ from `last`'s (both sorted by `key`), and the keys
/// of `last` missing from `next`.
fn diff<T: Copy + PartialEq>(last: &[T], next: &[T], key: impl Fn(&T) -> u32) -> (Vec<T>, Vec<u32>) {
    let (mut changed, mut gone) = (Vec::new(), Vec::new());
    let (mut i, mut j) = (0, 0);
    while i < last.len() || j < next.len() {
        match (last.get(i), next.get(j)) {
            (Some(a), Some(b)) if key(a) == key(b) => {
                if a != b {
                    changed.push(*b);
                }
                i += 1;
                j += 1;
            }
            (Some(a), Some(b)) if key(a) < key(b) => {
                gone.push(key(a));
                i += 1;
            }
            (Some(a), None) => {
                gone.push(key(a));
                i += 1;
            }
            (_, Some(b)) => {
                changed.push(*b);
                j += 1;
            }
            (None, None) => break,
        }
    }
    (changed, gone)
}

/// `last` with `changed` put in (replacing by key, or added in order) and `gone` taken out.
fn patch<T: Copy>(last: &mut Vec<T>, changed: &[T], gone: &[u32], key: impl Fn(&T) -> u32) {
    if !gone.is_empty() {
        last.retain(|t| gone.binary_search(&key(t)).is_err());
    }
    for c in changed {
        match last.binary_search_by_key(&key(c), &key) {
            Ok(k) => last[k] = *c,
            Err(k) => last.insert(k, *c),
        }
    }
}

impl DeltaEncoder {
    /// The delta that takes this client from the last snapshot encoded to `view` (already cut
    /// down for it by [`Snapshot::for_client`]). The first is everything.
    pub fn encode(&mut self, view: Snapshot) -> SnapshotDelta {
        let first = !std::mem::replace(&mut self.started, true);
        let mut zombies: Vec<ZombiePacked> = view.zombies.iter().map(ZombiePacked::pack).collect();
        zombies.sort_by_key(|z| z.id);
        let (z_changed, z_gone) = diff(&self.zombies, &zombies, |z| z.id);
        // Of the changed, those that only moved a little go as moves: most of a walking crowd.
        let (mut z_whole, mut z_moves) = (Vec::new(), Vec::<ZombieMove>::new());
        let mut last_id = 0;
        for z in z_changed {
            let step = self.zombies.binary_search_by_key(&z.id, |o| o.id).ok().and_then(|k| {
                let o = &self.zombies[k];
                let (dx, dy) = (z.x as i64 - o.x as i64, z.y as i64 - o.y as i64);
                (o.anim == z.anim && o.speed == z.speed && dx.abs() <= 127 && dy.abs() <= 127).then_some((dx as i8, dy as i8))
            });
            match step {
                Some((dx, dy)) => {
                    z_moves.push(ZombieMove { gap: z.id - last_id, dx, dy, facing: z.facing });
                    last_id = z.id;
                }
                None => z_whole.push(z),
            }
        }
        let z_changed = z_whole;
        self.zombies = zombies;
        let mut items = view.items;
        items.sort_by_key(|i| i.id);
        let (i_changed, i_gone) = diff(&self.items, &items, |i| i.id);
        self.items = items;

        let floor = view.floor.into_iter().next().map(|f| f.items).unwrap_or_default();
        let floor = (first || floor != self.floor).then(|| {
            self.floor = floor.clone();
            floor
        });
        let run = view.runs.into_iter().next().map(|r| r.run);
        let counts = |r: &RunStats| (r.kills, r.items_taken, r.searched, r.wounds, r.death.is_some());
        let run_due = match (&self.run, &run) {
            (Some(a), Some(b)) => counts(a) != counts(b) || (a != b && view.tick % RUN_EVERY == 0),
            (None, None) => false,
            _ => true,
        };
        let run = if first || run_due {
            self.run = run.clone();
            run
        } else {
            None
        };
        let outfits = (first || view.outfits != self.outfits).then(|| {
            self.outfits = view.outfits.clone();
            view.outfits
        });
        let risen = (first || view.risen != self.risen).then(|| {
            self.risen = view.risen.clone();
            view.risen
        });
        SnapshotDelta {
            tick: view.tick,
            time_of_day: view.time_of_day,
            time_scale: view.time_scale,
            game_minute: view.game_minute,
            players: view.players,
            zombies: z_changed,
            zombie_moves: z_moves,
            zombies_gone: z_gone,
            items: i_changed,
            items_gone: i_gone,
            edge_changes: view.edge_changes,
            edge_damage: view.edge_damage,
            hits: view.hits,
            private: view.private,
            item_notes: view.item_notes,
            level_ups: view.level_ups,
            floor,
            run,
            outfits,
            risen,
        }
    }
}

/// The client's side of [`DeltaEncoder`]: keeps the last whole snapshot and rebuilds the next.
#[derive(Default)]
pub struct DeltaDecoder {
    zombies: Vec<ZombiePacked>,
    last: Snapshot,
}

impl DeltaDecoder {
    /// The whole snapshot `delta` stands for, for the client playing `player`. Things that
    /// happened (edge changes, hits, notes) are in the snapshot they came with only.
    pub fn apply(&mut self, delta: SnapshotDelta, player: u32) -> Snapshot {
        let s = &mut self.last;
        (s.tick, s.time_of_day, s.time_scale, s.game_minute) = (delta.tick, delta.time_of_day, delta.time_scale, delta.game_minute);
        s.players = delta.players;
        patch(&mut self.zombies, &delta.zombies, &delta.zombies_gone, |z| z.id);
        let mut id = 0u32;
        for m in &delta.zombie_moves {
            id = id.wrapping_add(m.gap);
            if let Ok(k) = self.zombies.binary_search_by_key(&id, |z| z.id) {
                let z = &mut self.zombies[k];
                z.x = z.x.saturating_add_signed(m.dx as i32);
                z.y = z.y.saturating_add_signed(m.dy as i32);
                z.facing = m.facing;
            }
        }
        s.zombies = self.zombies.iter().map(ZombiePacked::unpack).collect();
        patch(&mut s.items, &delta.items, &delta.items_gone, |i| i.id);
        s.edge_changes = delta.edge_changes;
        s.edge_damage = delta.edge_damage;
        s.hits = delta.hits;
        s.private = delta.private;
        s.item_notes = delta.item_notes;
        s.level_ups = delta.level_ups;
        if let Some(items) = delta.floor {
            s.floor = vec![FloorView { player, items }];
        }
        if let Some(run) = delta.run {
            s.runs = vec![RunView { player, run }];
        }
        if let Some(outfits) = delta.outfits {
            s.outfits = outfits;
        }
        if let Some(risen) = delta.risen {
            s.risen = risen;
        }
        s.clone()
    }
}

/// `msg` as a frame: its length, then its postcard bytes.
pub fn encode_frame<T: Serialize>(msg: &T) -> Vec<u8> {
    let mut frame = vec![0u8; 4];
    frame = postcard::to_extend(msg, frame).expect("messages serialize");
    let len = (frame.len() - 4) as u32;
    frame[..4].copy_from_slice(&len.to_le_bytes());
    frame
}

/// Read one frame of at most `max` bytes and decode it. Returns the message and the bytes it took
/// on the wire. A longer frame, or one that doesn't decode, is `InvalidData`.
pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read, max: usize) -> io::Result<(T, usize)> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a frame of {len} bytes (at most {max})")));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let msg = postcard::from_bytes(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("a frame that doesn't decode: {e}")))?;
    Ok((msg, len + 4))
}

/// Write `msg` as one frame. Returns the bytes written.
pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<usize> {
    let frame = encode_frame(msg);
    w.write_all(&frame)?;
    Ok(frame.len())
}

impl Intent {
    /// The player this intent says it is from; `None` for the debug and admin intents that name
    /// none. A network server overwrites it with the sender's own player.
    pub fn player_mut(&mut self) -> Option<&mut u32> {
        match self {
            Intent::PlayerInput { player, .. }
            | Intent::Interact { player }
            | Intent::SmashWindow { player }
            | Intent::Attack { player }
            | Intent::Shove { player }
            | Intent::Stomp { player }
            | Intent::PickUp { player }
            | Intent::MoveItem { player, .. }
            | Intent::Equip { player, .. }
            | Intent::Wear { player, .. }
            | Intent::Unequip { player, .. }
            | Intent::Drop { player, .. }
            | Intent::Use { player, .. }
            | Intent::Search { player, .. }
            | Intent::Treat { player, .. }
            | Intent::Craft { player, .. }
            | Intent::Sleep { player, .. }
            | Intent::CancelJobs { player, .. }
            | Intent::Respawn { player }
            | Intent::Dev { player, .. } => Some(player),
            Intent::SpawnItem { .. } | Intent::SetGoal { .. } | Intent::SpawnZombies { .. } | Intent::SkipTime { .. } | Intent::MakeNoise { .. } => None,
        }
    }

    /// Only for developers: the dev toggles and the debug and admin intents. A multiplayer server
    /// takes them only with `--allow-dev`.
    pub fn dev_only(&self) -> bool {
        matches!(self, Intent::Dev { .. } | Intent::SpawnItem { .. } | Intent::SetGoal { .. } | Intent::SpawnZombies { .. } | Intent::SkipTime { .. } | Intent::MakeNoise { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LocalServer, content};

    /// A client that applies every delta has, each tick, the snapshot the server cut for it:
    /// players and everything of its own exactly, zombies to the wire's precision; and the deltas
    /// are a fraction of the size of whole snapshots.
    #[test]
    fn deltas_rebuild_each_clients_snapshot() {
        let mut config = content::load_config(&content::default_data_dir(), 3).unwrap();
        config.zombies = Some(1500);
        let mut server = LocalServer::new(config);
        let a = server.join(None);
        let b = server.join(Some((server.world.player_spawn + glam::Vec2::new(30.0, 0.0)).to_array()));
        let radius = server.world.config.tiers.interest_radius;
        let (mut encoder, mut decoder) = (DeltaEncoder::default(), DeltaDecoder::default());
        let (mut whole, mut sent, mut moves) = (0usize, 0usize, 0usize);
        let bytes = |s: &Snapshot| postcard::to_allocvec(s).unwrap().len();
        for t in 0..300u32 {
            let dir = glam::Vec2::from_angle(t as f32 * 0.02);
            server.send(Intent::PlayerInput { player: a, seq: t + 1, move_dir: dir.to_array(), facing: 0.0, run: true, sneak: false, aim: false });
            if t == 40 {
                // Something for the floor, the items and a note.
                let at = server.world.player(a).unwrap().state.pos;
                server.send(Intent::SpawnItem { item: "tinned_beans".into(), count: 2, pos: at.to_array() });
            }
            if t == 60 {
                server.send(Intent::PickUp { player: a });
                server.send(Intent::MakeNoise { pos: server.world.player(a).unwrap().state.pos.to_array(), radius: 60.0, lasting: false });
            }
            let full = server.tick();
            let view = full.for_client(a, radius);
            assert!(view.players.iter().any(|p| p.id == a));
            assert!(view.private.iter().all(|v| v.player == a) && view.runs.len() == 1 && view.floor.len() == 1);
            assert!(view.zombies.len() <= full.zombies.len());
            let frame = encode_frame(&ServerMsg::Snapshot(Box::new(encoder.encode(view.clone()))));
            let (msg, n) = read_frame::<ServerMsg>(&mut frame.as_slice(), MAX_SERVER_FRAME).unwrap();
            assert_eq!(n, frame.len());
            let ServerMsg::Snapshot(delta) = msg else { panic!("a snapshot") };
            moves += delta.zombie_moves.len();
            let got = decoder.apply(*delta, a);
            whole += bytes(&view);
            sent += frame.len();

            let exact = |s: &Snapshot| postcard::to_allocvec(&(s.tick, s.time_of_day, s.game_minute, &s.players, &s.items_sorted(), &s.floor, &s.outfits, &s.risen, &s.private, &s.item_notes, &s.hits, &s.level_ups)).unwrap();
            assert_eq!(exact(&got), exact(&view), "tick {t}");
            assert_eq!(got.zombies.len(), view.zombies.len(), "tick {t}");
            for (g, v) in got.zombies.iter().zip(&view.zombies) {
                assert_eq!((g.id, g.anim), (v.id, v.anim), "tick {t}");
                assert!(glam::Vec2::from(g.pos).distance(v.pos.into()) < 0.012, "zombie {} at tick {t}: {:?} for {:?}", g.id, g.pos, v.pos);
                let turn = (g.facing - v.facing).rem_euclid(std::f32::consts::TAU);
                assert!(turn.min(std::f32::consts::TAU - turn) < 0.02, "zombie {} faces {} for {}", g.id, g.facing, v.facing);
                assert!((g.speed - v.speed).abs() < 0.011);
            }
            let (g, v) = (got.run_of(a).unwrap(), view.run_of(a).unwrap());
            assert_eq!((g.kills, g.items_taken, g.death.is_some()), (v.kills, v.items_taken, v.death.is_some()));
            assert!((g.secs - v.secs).abs() <= 1.0, "the run's clock is at most a second old");
        }
        let _ = b;
        assert!(moves > 1000, "walking zombies go as moves ({moves})");
        assert!(sent * 3 < whole, "deltas took {sent} bytes, whole snapshots {whole}");
    }

    impl Snapshot {
        fn items_sorted(&self) -> Vec<GroundItem> {
            let mut items = self.items.clone();
            items.sort_by_key(|i| i.id);
            items
        }
    }

    #[test]
    fn frames_are_checked() {
        let frame = encode_frame(&ClientMsg::Ping(7));
        let (msg, n) = read_frame::<ClientMsg>(&mut frame.as_slice(), MAX_CLIENT_FRAME).unwrap();
        assert!(matches!(msg, ClientMsg::Ping(7)) && n == frame.len());
        // Too long, cut short, and not a message.
        let long = [&(MAX_CLIENT_FRAME as u32 + 1).to_le_bytes()[..], &[0; 8]].concat();
        assert_eq!(read_frame::<ClientMsg>(&mut long.as_slice(), MAX_CLIENT_FRAME).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(read_frame::<ClientMsg>(&mut &frame[..frame.len() - 1], MAX_CLIENT_FRAME).is_err());
        let junk = [&3u32.to_le_bytes()[..], &[250, 250, 250]].concat();
        assert_eq!(read_frame::<ClientMsg>(&mut junk.as_slice(), MAX_CLIENT_FRAME).unwrap_err().kind(), io::ErrorKind::InvalidData);
        // Every intent says whose it is, or that it is for developers.
        let mut i = Intent::Attack { player: 3 };
        *i.player_mut().unwrap() = 9;
        assert!(matches!(i, Intent::Attack { player: 9 }) && !i.dev_only());
        assert!(Intent::SkipTime { hours: 1.0 }.dev_only() && Intent::SkipTime { hours: 1.0 }.player_mut().is_none());
    }
}
