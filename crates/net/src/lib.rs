//! Messages between clients (human or agent) and the authoritative server.
//!
//! Clients never touch the simulation directly: they send [`Intent`]s and receive [`Snapshot`]s.
//! Single-player uses the same types through an in-process [`LocalServer`], so multiplayer is a
//! transport change, not a rewrite.

pub mod content;
pub mod save_file;

use serde::{Deserialize, Serialize};
use zombye_sim::map::{EdgeChange, OwnedEdge};
use zombye_sim::{ActionKind, CombatEvent, GroundItem, PlayerInput, Stance, World, WorldConfig, ZombieAnim};

/// Something a client wants to happen. The server validates and applies it.
///
/// `player` fields name the client's own player (from [`LocalServer::join`]). A network transport
/// will check the sender owns that player (M6).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Intent {
    /// Movement input, sent every tick. The server keeps the newest (by `seq`, starting at 1) and
    /// applies it once per tick. `move_dir` is a ground-plane vector of length ≤ 1 (clamped).
    PlayerInput { player: u32, seq: u32, move_dir: [f32; 2], facing: f32, run: bool, sneak: bool },
    /// Open or close the door or window the player faces, if it's within reach of the player's
    /// server position. Broken ones can't be shut.
    Interact { player: u32 },
    /// Smash the window the player faces (shut or open), if it's within reach. Loud.
    SmashWindow { player: u32 },
    /// Swing the equipped weapon (bare hands: a punch). Lands after the weapon's windup; asked for
    /// during the last moments of the previous action, it's queued.
    Attack { player: u32 },
    /// Shove the zombies in front back: no damage, always staggers, may knock down.
    Shove { player: u32 },
    /// Stamp on a downed zombie at the player's feet, in front.
    Stomp { player: u32 },
    /// Pick up the weapon nearest to where the player faces (within reach) and equip it, dropping
    /// the one held.
    PickUp { player: u32 },
    /// Single-player / debug: a dead player gets up at the spawn point.
    Respawn { player: u32 },
    /// Debug: put a weapon (by data id, e.g. `baseball_bat`) on the ground.
    SpawnItem { weapon: String, pos: [f32; 2] },
    /// Debug: make zombies chase this point instead of the players. `None` goes back to the players.
    SetGoal { pos: Option<[f32; 2]> },
    SpawnZombies { count: u32, center: [f32; 2], radius: f32 },
    /// Debug/admin: move the clock forward.
    SkipTime { hours: f32 },
    /// Debug: a noise at `pos` that zombies within `radius` metres (less through walls) come to
    /// investigate. Loud ones draw the far horde in over the following minutes; `lasting` makes
    /// that pull last much longer (like an alarm left ringing).
    MakeNoise {
        pos: [f32; 2],
        radius: f32,
        #[serde(default)]
        lasting: bool,
    },
}

/// What one zombie looks like to a client. Server-only state (AI memory, paths) is not included.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ZombieView {
    pub id: u32,
    pub pos: [f32; 2],
    pub facing: f32,
    /// Top speed in m/s; lets the renderer match walk-cycle rate to movement.
    pub speed: f32,
    pub anim: ZombieAnim,
}

/// What one player looks like to a client.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PlayerView {
    pub id: u32,
    pub pos: [f32; 2],
    pub facing: f32,
    pub stance: Stance,
    pub endurance: f32,
    /// Ran out of breath (can't run until endurance recovers). Needed to predict movement exactly.
    pub winded: bool,
    /// The newest input the server has applied; the owning client replays anything after it.
    pub last_input_seq: u32,
    pub hp: f32,
    pub dead: bool,
    /// Equipped weapon, as an index into the weapons the client got on joining; `None` = bare hands.
    pub weapon: Option<u16>,
    /// The swing, shove or stomp under way (before it lands), for the renderer.
    pub action: Option<ActionKind>,
    /// Seconds until that action lands (0 if none), and until the player can act again. The
    /// renderer times swings with these.
    pub action_left: f32,
    pub ready_in: f32,
}

/// A door or window whose hit points changed: how much of its full hit points it has left (0..1;
/// 1 = undamaged, or broken, which `edge_changes` says). Clients show cracks from it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeDamage {
    pub tile: glam::IVec2,
    pub edge: OwnedEdge,
    pub hp: f32,
}

impl EdgeDamage {
    /// Every damaged door and window right now, for a client joining (it gets the map then, and
    /// from then on only changes).
    pub fn all(world: &World) -> Vec<Self> {
        world
            .damaged_edges()
            .filter_map(|(e, _)| world.edge_hp(e).map(|(hp, max)| Self { tile: e.tile, edge: e.edge, hp: (hp / max).clamp(0.0, 1.0) }))
            .collect()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub tick: u64,
    /// Hour of day in [0, 24).
    pub time_of_day: f32,
    pub players: Vec<PlayerView>,
    pub zombies: Vec<ZombieView>,
    /// Map edits (doors and windows opening, closing, breaking) since the previous snapshot. Clients apply these to their
    /// copy of the map, which they received when joining.
    pub edge_changes: Vec<EdgeChange>,
    /// Doors and windows whose hit points changed since the previous snapshot (each once).
    pub edge_damage: Vec<EdgeDamage>,
    /// Every weapon lying about (a short list for now; interest management later).
    pub items: Vec<GroundItem>,
    /// Hits, kills, shoves, stomps and wounds since the previous snapshot, for hit feedback.
    pub hits: Vec<CombatEvent>,
}

impl Snapshot {
    /// Capture the active zombies within `TierTuning::interest_radius` of a player (in id order;
    /// zombies that leave it, are demoted to a far tier, or are corpses the server cleared away
    /// are simply no longer listed, and clients despawn them) and take pending map edits and edge
    /// damage. Proper per-client interest management comes with #22.
    pub fn capture(world: &mut World) -> Self {
        let z = &world.zombies;
        let zombies = (0..z.len())
            .filter(|&i| world.in_interest(i))
            .map(|i| ZombieView { id: z.id[i], pos: z.pos[i].to_array(), facing: z.facing[i], speed: z.speed[i], anim: z.anim[i] })
            .collect();
        let players = world
            .players
            .iter()
            .map(|p| PlayerView {
                id: p.id,
                pos: p.state.pos.to_array(),
                facing: p.state.facing,
                stance: p.state.stance,
                endurance: p.state.endurance,
                winded: p.state.winded,
                last_input_seq: p.last_input_seq,
                hp: p.hp,
                dead: p.dead,
                weapon: p.weapon,
                action: p.action.map(|a| a.kind),
                action_left: p.action.map_or(0.0, |a| a.timer.max(0.0)),
                ready_in: p.ready_in,
            })
            .collect();
        Self {
            tick: world.tick,
            time_of_day: world.time_of_day(),
            players,
            zombies,
            edge_changes: std::mem::take(&mut world.edge_changes),
            edge_damage: world.take_edge_damage().into_iter().map(|(e, hp)| EdgeDamage { tile: e.tile, edge: e.edge, hp }).collect(),
            items: world.items().to_vec(),
            hits: std::mem::take(&mut world.combat_events),
        }
    }
}

impl PlayerView {
    /// The predictable part of the player, for client-side reconciliation.
    pub fn state(&self) -> zombye_sim::PlayerState {
        zombye_sim::PlayerState {
            pos: self.pos.into(),
            facing: self.facing,
            stance: self.stance,
            endurance: self.endurance,
            winded: self.winded,
        }
    }
}

/// Apply an intent to the world. The one place client requests turn into sim changes.
pub fn apply_intent(world: &mut World, intent: &Intent) {
    match intent {
        Intent::PlayerInput { player, seq, move_dir, facing, run, sneak } => {
            let input = PlayerInput { move_dir: (*move_dir).into(), facing: *facing, run: *run, sneak: *sneak };
            world.set_player_input(*player, *seq, input);
        }
        Intent::Interact { player } => {
            world.player_interact(*player);
        }
        Intent::SmashWindow { player } => {
            world.player_smash_window(*player);
        }
        Intent::Attack { player } => {
            world.player_attack(*player);
        }
        Intent::Shove { player } => {
            world.player_shove(*player);
        }
        Intent::Stomp { player } => {
            world.player_stomp(*player);
        }
        Intent::PickUp { player } => {
            world.player_pick_up(*player);
        }
        Intent::Respawn { player } => {
            world.respawn_player(*player);
        }
        Intent::SpawnItem { weapon, pos } => {
            world.spawn_item(weapon, (*pos).into());
        }
        Intent::SetGoal { pos } => world.set_goal(pos.map(Into::into)),
        Intent::SpawnZombies { count, center, radius } => {
            world.spawn_zombies(*count, (*center).into(), *radius);
        }
        Intent::SkipTime { hours } => world.skip_time(hours.clamp(0.0, 24.0)),
        Intent::MakeNoise { pos, radius, lasting } => world.make_noise((*pos).into(), radius.clamp(0.0, 500.0), *lasting),
    }
}

/// In-process server for single-player: owns the world, takes intents, produces snapshots.
pub struct LocalServer {
    pub world: World,
    inbox: Vec<Intent>,
}

impl LocalServer {
    pub fn new(config: WorldConfig) -> Self {
        Self::from_world(World::new(config))
    }

    /// Serve an existing world (one loaded from a save).
    pub fn from_world(world: World) -> Self {
        Self { world, inbox: Vec::new() }
    }

    /// Swap in another world (a loaded save), dropping intents queued for the old one.
    pub fn replace_world(&mut self, world: World) {
        self.world = world;
        self.inbox.clear();
    }

    /// Join the game: spawns a player in the bedroom (or at `at`, a single-player debug option)
    /// and returns its id.
    pub fn join(&mut self, at: Option<[f32; 2]>) -> u32 {
        let pos = at.map(Into::into).unwrap_or(self.world.player_spawn);
        self.world.add_player(pos)
    }

    pub fn send(&mut self, intent: Intent) {
        self.inbox.push(intent);
    }

    /// Apply queued intents, advance one tick, and return the new snapshot.
    pub fn tick(&mut self) -> Snapshot {
        for intent in self.inbox.drain(..) {
            apply_intent(&mut self.world, &intent);
        }
        self.world.step();
        Snapshot::capture(&mut self.world)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zombye_sim::players::step_movement;
    use zombye_sim::{BuildingTemplate, TICK_DT};

    fn server() -> LocalServer {
        let templates = [include_str!("../../../assets/data/buildings/bungalow.ron")].iter().map(|t| BuildingTemplate::from_ron(t).unwrap()).collect();
        let weapons = zombye_sim::items::parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        LocalServer::new(WorldConfig::new(4, 50, templates).with_weapons(weapons))
    }

    /// Picking up, swinging and getting hurt all go through intents and show up in snapshots.
    #[test]
    fn combat_goes_through_intents_and_snapshots() {
        let mut server = server();
        let id = server.join(None);
        let snap = server.tick();
        assert!(!snap.items.is_empty(), "weapons should lie about");
        let me = server.world.player(id).unwrap().state;
        let ahead = me.pos + glam::Vec2::from_angle(me.facing) * 0.4;
        server.send(Intent::SpawnItem { weapon: "baseball_bat".into(), pos: ahead.to_array() });
        server.tick();
        server.send(Intent::PickUp { player: id });
        let snap = server.tick();
        let view = *snap.players.iter().find(|p| p.id == id).unwrap();
        let bat = server.world.weapon_index("baseball_bat").unwrap();
        assert_eq!(view.weapon, Some(bat));
        assert!(snap.items.iter().all(|it| it.weapon != bat || it.pos.distance(ahead) > 0.01));
        server.send(Intent::Attack { player: id });
        let snap = server.tick();
        assert_eq!(snap.players[0].action, Some(ActionKind::Swing));
        assert!(snap.players[0].endurance < view.endurance);
        // Kill the player the hard way and respawn.
        server.world.players[0].hp = 1.0;
        let z = server.world.add_zombie(server.world.player(id).unwrap().state.pos + glam::Vec2::new(0.6, 0.0), std::f32::consts::PI, 1.0);
        server.world.zombies.wander_timer[z] = 1e9;
        let mut dead = false;
        for _ in 0..20 * 30 {
            if server.tick().players[0].dead {
                dead = true;
                break;
            }
        }
        assert!(dead, "a zombie next to a 1 HP player should kill them");
        server.send(Intent::Respawn { player: id });
        let snap = server.tick();
        assert!(!snap.players[0].dead && snap.players[0].hp == 100.0);
    }

    /// The client predicts with the same function the server runs, so with no world changes the
    /// prediction matches the server exactly.
    #[test]
    fn prediction_matches_server() {
        let mut server = server();
        let id = server.join(None);
        let snap = Snapshot::capture(&mut server.world);
        let mut predicted = snap.players[0].state();
        for seq in 1..=200u32 {
            let a = seq as f32 * 0.07;
            let (move_dir, facing, run, sneak) = ([a.cos(), a.sin()], a, seq % 60 < 30, seq % 60 > 50);
            server.send(Intent::PlayerInput { player: id, seq, move_dir, facing, run, sneak });
            step_movement(&server.world.map, &mut predicted, &PlayerInput { move_dir: move_dir.into(), facing, run, sneak }, TICK_DT);
            let snap = server.tick();
            let view = snap.players.iter().find(|p| p.id == id).unwrap();
            assert_eq!(view.last_input_seq, seq);
            assert_eq!(view.state(), predicted, "diverged at seq {seq}");
        }
    }

    /// Climbing through a window predicts exactly too: the client keeps its own copy of the map,
    /// updated only from snapshot edge changes, and predicts with that.
    #[test]
    fn prediction_matches_server_through_a_window() {
        use zombye_sim::map::{Edge, Side};
        let mut server = server();
        let house = server.world.spawn_building;
        let map = &server.world.map;
        let b = &map.buildings[house];
        // An outer window with open floor on both sides.
        let mut found = None;
        for y in b.min.y..b.min.y + b.size.y {
            for x in b.min.x..b.min.x + b.size.x {
                let t = glam::IVec2::new(x, y);
                for side in Side::ALL {
                    let n = t + side.offset();
                    if found.is_none() && map.tile(t).room != 0 && map.tile(n).room == 0 && map.edge(t, side) == Edge::Window && !map.tile_blocked(t) && !map.tile_blocked(n) {
                        found = Some((t, n));
                    }
                }
            }
        }
        let (inside, outside) = found.expect("a window");
        let from = inside.as_vec2() + glam::Vec2::splat(0.5);
        let dir = (outside - inside).as_vec2();
        let facing = dir.y.atan2(dir.x);
        let id = server.join(Some(from.to_array()));
        let mut client_map = server.world.map.clone();
        let mut predicted = Snapshot::capture(&mut server.world).players.iter().find(|p| p.id == id).unwrap().state();
        let mut seq = 0;
        let mut drive = |server: &mut LocalServer, client_map: &mut zombye_sim::TileMap, predicted: &mut zombye_sim::PlayerState, move_dir: [f32; 2], ticks: u32| {
            for _ in 0..ticks {
                seq += 1;
                server.send(Intent::PlayerInput { player: id, seq, move_dir, facing, run: false, sneak: false });
                step_movement(client_map, predicted, &PlayerInput { move_dir: move_dir.into(), facing, run: false, sneak: false }, TICK_DT);
                let snap = server.tick();
                for c in &snap.edge_changes {
                    client_map.set_owned_edge(c.tile, c.edge, c.kind);
                }
                let view = snap.players.iter().find(|p| p.id == id).unwrap();
                assert_eq!(view.state(), *predicted, "diverged at seq {seq}");
            }
        };
        // Face the window and open it, then climb out and keep going.
        drive(&mut server, &mut client_map, &mut predicted, [0.0, 0.0], 2);
        server.send(Intent::Interact { player: id });
        drive(&mut server, &mut client_map, &mut predicted, [0.0, 0.0], 1);
        assert_eq!(client_map.edge(inside, side_toward(inside, outside)), Edge::WindowOpen, "the client heard about the window");
        let start = predicted.pos;
        let mut climbed = 0;
        for _ in 0..60 {
            drive(&mut server, &mut client_map, &mut predicted, dir.to_array(), 1);
            climbed += client_map.in_window_opening(predicted.pos) as u32;
        }
        assert!(climbed >= 15, "only {climbed} ticks climbing");
        assert_eq!(server.world.map.room_at(predicted.pos), 0, "got out");
        assert!(predicted.pos.distance(start) > 2.0);
    }

    fn side_toward(a: glam::IVec2, b: glam::IVec2) -> zombye_sim::map::Side {
        zombye_sim::map::Side::ALL.into_iter().find(|s| a + s.offset() == b).unwrap()
    }

    /// Battered doors and windows report their hit points as they change (once each), and
    /// corpses the server clears away drop out of snapshots.
    #[test]
    fn snapshots_carry_edge_damage_and_drop_cleared_corpses() {
        use zombye_sim::ZombieState;
        use zombye_sim::map::{Edge, EdgeRef, Side};
        let mut server = server();
        server.world.config.zombie.max_corpses = 2;
        let house = server.world.spawn_building;
        let b = server.world.map.buildings[house].clone();
        // Any outer window of the spawn house, and a zombie just outside it after the player inside.
        let map = &server.world.map;
        let mut found = None;
        for y in b.min.y..b.min.y + b.size.y {
            for x in b.min.x..b.min.x + b.size.x {
                let t = glam::IVec2::new(x, y);
                for side in Side::ALL {
                    let n = t + side.offset();
                    if found.is_none() && map.tile(t).room != 0 && map.tile(n).room == 0 && map.edge(t, side) == Edge::Window && !map.tile_blocked(n) && !map.tile_blocked(t) {
                        found = Some((EdgeRef::of(t, side), t, n));
                    }
                }
            }
        }
        let (window, inside, outside) = found.expect("a window");
        let z = server.world.add_zombie(outside.as_vec2() + glam::Vec2::splat(0.5), 0.0, 1.0);
        server.world.zombies.state[z] = ZombieState::Investigate;
        server.world.zombies.target_pos[z] = inside.as_vec2() + glam::Vec2::splat(0.5);
        server.world.zombies.timer[z] = 60.0;
        let mut reported = Vec::new();
        for _ in 0..20 * 30 {
            let snap = server.tick();
            reported.extend(snap.edge_damage.iter().filter(|d| d.tile == window.tile && d.edge == window.edge).map(|d| d.hp));
            if snap.edge_changes.iter().any(|c| c.kind == Edge::WindowBroken) {
                break;
            }
        }
        assert!(reported.len() >= 3, "{reported:?}");
        assert!(reported.windows(2).take(reported.len() - 2).all(|w| w[1] < w[0]), "hit points only go down: {reported:?}");
        assert_eq!(reported.last(), Some(&1.0), "broken: no damage left to show");
        // Three corpses, two kept: the oldest is gone from the next snapshot.
        let ids: Vec<u32> = (0..3)
            .map(|k| {
                let i = server.world.add_zombie(glam::Vec2::new(5.5 + k as f32, 5.5), 0.0, 1.0);
                (server.world.zombies.state[i], server.world.zombies.hp[i]) = (ZombieState::Dead, 0.0);
                let id = server.world.zombies.id[i];
                server.tick();
                id
            })
            .collect();
        let snap = server.tick();
        let listed: Vec<u32> = snap.zombies.iter().map(|z| z.id).filter(|id| ids.contains(id)).collect();
        assert_eq!(listed, ids[1..]);
        assert!(snap.zombies.windows(2).all(|w| w[0].id < w[1].id), "zombies in id order");
    }

    /// Only active zombies near a player go out in snapshots: far ones (near or virtual tier) don't,
    /// until the player comes close again.
    #[test]
    fn snapshots_only_carry_active_zombies_near_players() {
        let mut server = server();
        let id = server.join(None);
        let me = server.world.player(id).unwrap().state.pos;
        let near = server.world.add_zombie(me + glam::Vec2::new(5.0, 0.0), 0.0, 1.0);
        let near_id = server.world.zombies.id[near];
        let far_spot = glam::Vec2::new(if me.x < 160.0 { 300.0 } else { 20.0 }, me.y);
        let far = server.world.add_zombie(far_spot, 0.0, 1.0);
        let far_id = server.world.zombies.id[far];
        let snap = server.tick();
        let listed = |snap: &Snapshot, id: u32| snap.zombies.iter().any(|z| z.id == id);
        assert!(listed(&snap, near_id));
        assert!(!listed(&snap, far_id), "a zombie {:.0} m away was sent", far_spot.distance(me));
        let i = server.world.zombies.index_of(far_id).unwrap();
        assert_ne!(server.world.zombies.tier[i], zombye_sim::Tier::Active);
        assert!(snap.zombies.iter().all(|z| glam::Vec2::from(z.pos).distance(me) <= server.world.config.tiers.interest_radius));
        // Walk over (teleport): it's promoted and sent again.
        let at = server.world.zombies.pos[i] - glam::Vec2::new(10.0, 0.0);
        server.world.players[0].state.pos = server.world.map.resolve_circle(at, zombye_sim::players::PLAYER_RADIUS);
        let snap = server.tick();
        assert!(listed(&snap, far_id), "promoted zombie not sent");
    }

    /// A server loaded from a save carries on exactly like the one saved: the same snapshots for
    /// the same intents, and a client resuming from the loaded player's view predicts it exactly.
    #[test]
    fn a_loaded_server_carries_on_like_the_saved_one() {
        let mut a = server();
        let id = a.join(None);
        let mut seq = 0;
        // Swings cost endurance on the server only (prediction catches up from the next snapshot),
        // so only the part before the save swings.
        let intents = |seq: &mut u32, swing: bool| {
            *seq += 1;
            let t = *seq as f32 * 0.05;
            let mut out = vec![Intent::PlayerInput { player: id, seq: *seq, move_dir: [t.cos(), t.sin()], facing: t, run: *seq % 50 < 20, sneak: false }];
            if swing && seq.is_multiple_of(30) {
                out.push(Intent::Attack { player: id });
            }
            out
        };
        for _ in 0..150 {
            for i in intents(&mut seq, true) {
                a.send(i);
            }
            a.tick();
        }
        let loaded = World::load(&a.world.save(), &a.world.config).expect("loads");
        let mut b = LocalServer::from_world(loaded);
        let mut predicted = Snapshot::capture(&mut b.world).players.iter().find(|p| p.id == id).expect("our player").state();
        assert_eq!(predicted, a.world.player(id).unwrap().state);
        for _ in 0..200 {
            let batch = intents(&mut seq, false);
            for i in &batch {
                a.send(i.clone());
                b.send(i.clone());
                if let Intent::PlayerInput { move_dir, facing, run, sneak, .. } = i {
                    step_movement(&b.world.map, &mut predicted, &PlayerInput { move_dir: (*move_dir).into(), facing: *facing, run: *run, sneak: *sneak }, TICK_DT);
                }
            }
            let (sa, sb) = (a.tick(), b.tick());
            assert_eq!(format!("{sa:?}"), format!("{sb:?}"), "snapshots differ at tick {}", sa.tick);
            assert_eq!(sb.players.iter().find(|p| p.id == id).unwrap().state(), predicted, "prediction diverged at tick {}", sb.tick);
        }
        assert_eq!(a.world.state_hash(), b.world.state_hash());
    }

    #[test]
    fn move_dir_is_clamped_server_side() {
        let mut server = server();
        let id = server.join(None);
        let start = server.world.player(id).unwrap().state.pos;
        server.send(Intent::PlayerInput { player: id, seq: 1, move_dir: [0.0, 100.0], facing: 0.0, run: false, sneak: false });
        server.tick();
        let moved = server.world.player(id).unwrap().state.pos.distance(start);
        assert!(moved <= zombye_sim::players::WALK_SPEED * TICK_DT + 1e-4, "moved {moved} m in one tick");
    }
}
