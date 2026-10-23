//! Messages between clients (human or agent) and the authoritative server.
//!
//! Clients never touch the simulation directly: they send [`Intent`]s and receive [`Snapshot`]s.
//! Single-player uses the same types through an in-process [`LocalServer`], so multiplayer is a
//! transport change, not a rewrite.

pub mod content;
pub mod save_file;
pub mod shutdown;

use serde::{Deserialize, Serialize};
use zombye_sim::map::{EdgeChange, OwnedEdge};
use zombye_sim::{ActionKind, Burden, Carry, CombatEvent, DevFlags, GroundItem, HealthTags, HealthView, Inventory, ItemNote, ItemStack, MoveMods, Place, PlayerInput, Stance, Treatment, World, WorldConfig, ZombieAnim};

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
    /// Pick up the item nearest to where the player faces (within reach of its feet), at once: a
    /// weapon into the hand (what was held into pockets and bags, or left on the floor), anything
    /// else into pockets and bags, as many as fit.
    PickUp { player: u32 },
    /// Move `count` (0 = all) of the stack at `from`/`index` to `to`: between pockets and bags
    /// (`Carried`), hands and bag slots, worn clothing, the open container and the floor (`Ground`,
    /// where `index` is the ground item's id). Takes a moment (queued on the server; checked again
    /// when it happens: still there, in reach, fits, not too heavy).
    MoveItem { player: u32, from: Place, index: u32, to: Place, count: u16 },
    /// Put the carried stack `index` where it belongs: a bag on, clothing on, anything else in hand.
    Equip { player: u32, index: u32 },
    /// Put on the carried clothing `index`.
    Wear { player: u32, index: u32 },
    /// Take off or let go of what's at `from` (a slot, or `Worn` and an index) into pockets and
    /// bags, or onto the floor if there's no room.
    Unequip { player: u32, from: Place, index: u32 },
    /// Drop `count` (0 = all) of the stack at `from`/`index` at the player's feet.
    Drop { player: u32, from: Place, index: u32, count: u16 },
    /// Eat or drink one of the stack at `from`/`index` (carried or held).
    Use { player: u32, from: Place, index: u32 },
    /// Search (open) a container: this one, or with `None` the one the player faces within reach.
    /// The first search takes the container's search time and fills it.
    Search { player: u32, container: Option<u32> },
    /// First aid (#10): `action` on the player's wound `wound` (its id in the health view; `None`
    /// for painkillers and antibiotics) with one use of `item` (an index into the items; `None`:
    /// the best the player carries, or nothing to take a dressing off). Takes a few seconds,
    /// standing still; a strike landing or a swing interrupts it. Checked when asked and when done.
    Treat { player: u32, wound: Option<u32>, action: Treatment, item: Option<u16> },
    /// Single-player / debug: a dead player gets up at the spawn point.
    Respawn { player: u32 },
    /// Dev toggles for testing (`None` leaves one as it is): `god` takes no damage and can't die,
    /// `ignored` isn't seen or heard by zombies (those after the player lose them). A hosted server
    /// refuses it unless started with `--allow-dev` ([`LocalServer::hosted`]).
    Dev { player: u32, god: Option<bool>, ignored: Option<bool> },
    /// Debug: put `count` of an item (by data id, e.g. `baseball_bat` or `tinned_beans`) on the ground.
    SpawnItem { item: String, count: u16, pos: [f32; 2] },
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
    /// What its load does to its movement. Needed to predict movement exactly.
    pub mods: MoveMods,
    /// Busy with its things: seconds left on the item job under way (0 if none), and jobs queued.
    pub busy: f32,
    pub jobs: u8,
    /// Dev toggles on for this player (god mode, ignored by zombies), for the HUD.
    pub dev: DevFlags,
    /// What anyone can see of its health: bleeding, limping, a hurt arm, sick. The detail is only
    /// in its own [`PrivateView::health`].
    pub health: HealthTags,
}

/// What a container holds, for whoever has it open.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContainerView {
    pub id: u32,
    /// Its kind (an id in `containers.ron`) and name.
    pub kind: String,
    pub name: String,
    pub room_l: f32,
    pub stacks: Vec<ItemStack>,
}

/// The part of a player only its own client gets: everything it has, its load, the container it
/// has open, and its wounds and illnesses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrivateView {
    pub player: u32,
    pub inventory: Inventory,
    pub carry: Carry,
    /// The weight past which nothing more can be picked up (kg).
    pub max_kg: f32,
    pub open: Option<ContainerView>,
    /// Wounds by body part, pain, the infection once it shows (#9).
    pub health: HealthView,
}

impl PrivateView {
    pub fn of(world: &World, id: u32) -> Option<Self> {
        let p = world.player(id)?;
        let open = p.open.and_then(|c| {
            let site = world.container(c)?;
            let def = world.container_def(site);
            Some(ContainerView { id: c, kind: def.id.clone(), name: def.name.clone(), room_l: def.room_l, stacks: world.container_contents(c).unwrap_or(&[]).to_vec() })
        });
        let carry = world.carry(p);
        let health = world.health_view(id).unwrap_or_default();
        Some(Self { player: id, inventory: p.inventory.clone(), carry, max_kg: carry.limit_kg * world.config.carry.max_load, open, health })
    }

    /// Has a load in this stage or worse.
    pub fn at_least(&self, b: Burden) -> bool {
        self.carry.burden >= b
    }
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
    /// Every item lying about (a short list for now; interest management later).
    pub items: Vec<GroundItem>,
    /// Hits, kills, shoves, stomps and wounds since the previous snapshot, for hit feedback.
    pub hits: Vec<CombatEvent>,
    /// Players' private views (inventory, load, open container) that changed since the previous
    /// snapshot, each once; a client keeps the latest of its own. A networked server sends each
    /// client only its own ([`Snapshot::for_player`]).
    pub private: Vec<PrivateView>,
    /// What came of players' item requests since the previous snapshot (done, or why not).
    pub item_notes: Vec<ItemNote>,
    /// Each player's floor: the ground items it can take right now, by the server's own reach
    /// rule (`World::floor_of`), nearest first. Every snapshot; a client gets only its own.
    pub floor: Vec<FloorView>,
}

/// The ground items one player can reach (ids into [`Snapshot::items`]), nearest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FloorView {
    pub player: u32,
    pub items: Vec<u32>,
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
                mods: p.state.mods,
                busy: p.jobs.first().map_or(0.0, |j| j.left.max(0.0)),
                jobs: p.jobs.len().min(255) as u8,
                dev: p.dev,
                health: if p.dead { HealthTags::default() } else { p.health.tags(&world.config.health) },
            })
            .collect();
        let private = world.take_private_dirty().into_iter().filter_map(|id| PrivateView::of(world, id)).collect();
        Self {
            tick: world.tick,
            time_of_day: world.time_of_day(),
            players,
            zombies,
            edge_changes: std::mem::take(&mut world.edge_changes),
            edge_damage: world.take_edge_damage().into_iter().map(|(e, hp)| EdgeDamage { tile: e.tile, edge: e.edge, hp }).collect(),
            items: world.items().to_vec(),
            hits: std::mem::take(&mut world.combat_events),
            private,
            item_notes: std::mem::take(&mut world.item_notes),
            floor: world.players.iter().map(|p| FloorView { player: p.id, items: world.floor_ids(p.id) }).collect(),
        }
    }

    /// This snapshot as client `player` should get it: only its own private view and notes.
    pub fn for_player(mut self, player: u32) -> Self {
        self.private.retain(|v| v.player == player);
        self.item_notes.retain(|n| n.player == player);
        self.floor.retain(|f| f.player == player);
        self
    }

    /// The ground items `player` can reach, nearest first (empty if it isn't in this snapshot).
    pub fn floor_of(&self, player: u32) -> &[u32] {
        self.floor.iter().find(|f| f.player == player).map_or(&[], |f| f.items.as_slice())
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
            mods: self.mods,
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
        // Refusals are noted for the player's client (`Snapshot::item_notes`); nothing else to do.
        Intent::MoveItem { player, from, index, to, count } => {
            let _ = world.player_move_item(*player, *from, *index, *to, *count);
        }
        Intent::Equip { player, index } => {
            let _ = world.player_equip(*player, *index);
        }
        Intent::Wear { player, index } => {
            let _ = world.player_wear(*player, *index);
        }
        Intent::Unequip { player, from, index } => {
            let _ = world.player_unequip(*player, *from, *index);
        }
        Intent::Drop { player, from, index, count } => {
            let _ = world.player_drop(*player, *from, *index, *count);
        }
        Intent::Use { player, from, index } => {
            let _ = world.player_use(*player, *from, *index);
        }
        Intent::Search { player, container } => {
            let _ = world.player_search(*player, *container);
        }
        Intent::Treat { player, wound, action, item } => {
            let _ = world.player_treat(*player, *wound, *action, *item);
        }
        Intent::Respawn { player } => {
            world.respawn_player(*player);
        }
        Intent::Dev { player, god, ignored } => {
            world.set_dev(*player, *god, *ignored);
        }
        Intent::SpawnItem { item, count, pos } => {
            world.spawn_item(item, (*count).max(1), (*pos).into());
        }
        Intent::SetGoal { pos } => world.set_goal(pos.map(Into::into)),
        Intent::SpawnZombies { count, center, radius } => {
            world.spawn_zombies(*count, (*center).into(), *radius);
        }
        Intent::SkipTime { hours } => world.skip_time(hours.clamp(0.0, 24.0)),
        Intent::MakeNoise { pos, radius, lasting } => world.make_noise((*pos).into(), radius.clamp(0.0, 500.0), *lasting),
    }
}

impl Intent {
    /// A developer toggle ([`Intent::Dev`]), which only single-player and servers started with
    /// `--allow-dev` take.
    pub fn is_dev(&self) -> bool {
        matches!(self, Intent::Dev { .. })
    }
}

/// In-process server for single-player: owns the world, takes intents, produces snapshots.
pub struct LocalServer {
    pub world: World,
    inbox: Vec<Intent>,
    /// Takes [`Intent::Dev`] (single-player: yes; a hosted server: only with `--allow-dev`).
    pub allow_dev: bool,
    /// Dev intents refused so far.
    pub refused_dev: u32,
}

impl LocalServer {
    pub fn new(config: WorldConfig) -> Self {
        Self::from_world(World::new(config))
    }

    /// Serve an existing world (one loaded from a save), single-player: dev toggles allowed.
    pub fn from_world(world: World) -> Self {
        Self { world, inbox: Vec::new(), allow_dev: true, refused_dev: 0 }
    }

    /// A server other people join (multiplayer, M6): dev toggles are refused unless `allow_dev`
    /// (its `--allow-dev`), so nobody makes themselves unkillable or invisible to zombies.
    pub fn hosted(world: World, allow_dev: bool) -> Self {
        Self { allow_dev, ..Self::from_world(world) }
    }

    /// Swap in another world (a loaded save), dropping intents queued for the old one. The next
    /// snapshot carries every player's private view again.
    pub fn replace_world(&mut self, world: World) {
        self.world = world;
        self.world.mark_all_private_dirty();
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
            if intent.is_dev() && !self.allow_dev {
                self.refused_dev += 1;
                continue;
            }
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
        let items = zombye_sim::items::parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap();
        let containers = zombye_sim::containers::parse_containers(include_str!("../../../assets/data/containers.ron")).unwrap();
        LocalServer::new(WorldConfig::new(4, 50, templates).with_weapons(weapons).with_items(items).with_containers(containers))
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
        server.send(Intent::SpawnItem { item: "baseball_bat".into(), count: 1, pos: ahead.to_array() });
        server.tick();
        server.send(Intent::PickUp { player: id });
        let snap = server.tick();
        let view = *snap.players.iter().find(|p| p.id == id).unwrap();
        let bat = server.world.weapon_index("baseball_bat").unwrap();
        assert_eq!(view.weapon, Some(bat));
        let bat_item = server.world.item_index("baseball_bat").unwrap();
        assert!(snap.items.iter().all(|it| it.stack.item != bat_item || it.pos.distance(ahead) > 0.01));
        // Our private view says so too (it changed, so it came with this snapshot).
        let mine = snap.clone().for_player(id);
        assert_eq!(mine.private.len(), 1);
        assert_eq!(mine.private[0].inventory.hand, Some(ItemStack::one(bat_item)));
        assert!(mine.item_notes.iter().any(|n| n.ok && n.text.contains("baseball bat")), "{:?}", mine.item_notes);
        assert!(server.tick().private.is_empty(), "sent again with nothing changed");
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

    /// Single-player takes dev toggles and snapshots show them; a hosted server refuses them
    /// unless started with `--allow-dev`.
    #[test]
    fn dev_toggles_single_player_yes_hosted_only_with_allow_dev() {
        let mut solo = server();
        let id = solo.join(None);
        solo.send(Intent::Dev { player: id, god: Some(true), ignored: None });
        let snap = solo.tick();
        assert_eq!(snap.players[0].dev, DevFlags { god: true, ignored: false });
        solo.send(Intent::Dev { player: id, god: None, ignored: Some(true) });
        assert_eq!(solo.tick().players[0].dev, DevFlags { god: true, ignored: true });

        for allow in [false, true] {
            let mut hosted = LocalServer::hosted(server().world, allow);
            let id = hosted.join(None);
            hosted.send(Intent::Dev { player: id, god: Some(true), ignored: Some(true) });
            let view = hosted.tick().players[0];
            assert_eq!(view.dev, if allow { DevFlags { god: true, ignored: true } } else { DevFlags::default() }, "allow_dev {allow}");
            assert_eq!(hosted.refused_dev, !allow as u32);
            assert_eq!(hosted.world.player(id).unwrap().dev, view.dev);
        }
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

    /// First aid through intents (#10): dressing a bite from what the player carries, standing
    /// still meanwhile, with the private view showing it; and each tick's prediction from the last
    /// snapshot (as the client reconciles) matches the server through the start and the end.
    #[test]
    fn treating_goes_through_intents_and_predicts() {
        use zombye_sim::{Dressing, Part, WoundKind};
        let mut server = server();
        let id = server.join(None);
        server.send(Intent::Dev { player: id, god: None, ignored: Some(true) });
        server.tick();
        assert!(server.world.give_item(id, "gauze", 1));
        server.world.inflict_wound(id, Part::RightLeg, WoundKind::Bite, Some(false)).unwrap();
        let snap = server.tick();
        let wound = snap.private.iter().find(|v| v.player == id).unwrap().health.wounds[0].id;
        let mut last = snap.players.iter().find(|p| p.id == id).unwrap().state();
        for t in 0..140u32 {
            if t == 10 {
                server.send(Intent::Treat { player: id, wound: Some(wound), action: Treatment::Dress, item: None });
            }
            let seq = t + 1;
            let a = t as f32 * 0.05;
            let (move_dir, facing) = ([a.cos(), a.sin()], a);
            server.send(Intent::PlayerInput { player: id, seq, move_dir, facing, run: false, sneak: false });
            let mut predicted = last;
            step_movement(&server.world.map, &mut predicted, &PlayerInput { move_dir: move_dir.into(), facing, run: false, sneak: false }, TICK_DT);
            let snap = server.tick();
            let view = snap.players.iter().find(|p| p.id == id).unwrap();
            if t > 10 && t < 85 {
                assert_eq!(view.mods.speed, 0.0, "standing still while dressing (tick {t})");
            }
            // Standing still starts and ends after a tick's movement, so the snapshot always has
            // the modifiers the next tick moves with.
            assert_eq!(view.state().pos, predicted.pos, "tick {t}");
            last = view.state();
        }
        let w = &server.world.player(id).unwrap().health.wounds[0];
        assert_eq!(w.dressing, Dressing::Clean);
        assert!(!w.bleeding);
        let view = PrivateView::of(&server.world, id).unwrap();
        assert_eq!(view.health.wounds[0].dressing, Dressing::Clean);
        assert_eq!(view.health.aid.opened.len(), 1, "two uses left in the roll");
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

    /// Items going in and out of an inventory change the load, and with it movement; the client
    /// predicts with the modifiers from its latest snapshot and stays exact.
    #[test]
    fn prediction_matches_server_with_a_changing_load() {
        let mut server = server();
        let id = server.join(None);
        let at = server.world.player(id).unwrap().state.pos;
        // A rucksack on (50 L), and a heap of planks on the floor at our feet, 2 kg each: taking them one by one goes past
        // the carry limit (14 kg) and into heavy (18.2) while we walk and run about.
        for _ in 0..10 {
            server.send(Intent::SpawnItem { item: "wooden_plank".into(), count: 1, pos: at.to_array() });
        }
        assert!(server.world.hold_item(id, zombye_sim::Slot::Back, "hiking_pack"));
        let snap = server.tick();
        let mut predicted = snap.players.iter().find(|p| p.id == id).unwrap().state();
        let mut mods_seen = vec![predicted.mods];
        let mut seq = 0;
        for t in 0..300u32 {
            seq += 1;
            let a = t as f32 * 0.05;
            // Small circles, so the planks stay within reach.
            let (move_dir, facing, run) = ([a.cos() * 0.06, a.sin() * 0.06], a, t % 40 < 25);
            server.send(Intent::PlayerInput { player: id, seq, move_dir, facing, run, sneak: false });
            if t % 15 == 0 {
                let floor = server.world.floor_of(id);
                if let Some(g) = floor.first() {
                    server.send(Intent::MoveItem { player: id, from: Place::Ground, index: g.id, to: Place::Carried, count: 0 });
                }
            }
            step_movement(&server.world.map, &mut predicted, &PlayerInput { move_dir: move_dir.into(), facing, run, sneak: false }, TICK_DT);
            let snap = server.tick();
            let view = snap.players.iter().find(|p| p.id == id).unwrap();
            // (A job done at the end of the tick may have changed the modifiers for the next one.)
            assert_eq!(zombye_sim::PlayerState { mods: predicted.mods, ..view.state() }, predicted, "diverged at seq {seq}");
            // What a reconcile would take from the snapshot: the load's modifiers.
            predicted.mods = view.mods;
            if *mods_seen.last().unwrap() != view.mods {
                mods_seen.push(view.mods);
            }
        }
        let carried: u16 = server.world.player(id).unwrap().inventory.carried.iter().map(|s| s.count).sum();
        assert_eq!(carried, 10, "{:?}", server.world.item_notes);
        assert!(mods_seen.len() >= 3 && !mods_seen.last().unwrap().can_run, "burdened, then heavy: {mods_seen:?}");
    }

    /// Wounds on the legs come and go while we walk and run (a gash stops running): the client
    /// predicts with the modifiers from its latest snapshot and stays exact. Our own snapshot
    /// carries our wounds; everyone sees only the coarse tags.
    #[test]
    fn prediction_matches_server_with_leg_wounds() {
        use zombye_sim::{Part, WoundKind};
        let mut server = server();
        let id = server.join(None);
        let other = server.join(None);
        let snap = server.tick();
        let mut predicted = snap.players.iter().find(|p| p.id == id).unwrap().state();
        let mut mods_seen = vec![predicted.mods];
        for seq in 1..=300u32 {
            let a = seq as f32 * 0.05;
            let (move_dir, facing, run) = ([a.cos(), a.sin()], a, seq % 40 < 25);
            server.send(Intent::PlayerInput { player: id, seq, move_dir, facing, run, sneak: false });
            step_movement(&server.world.map, &mut predicted, &PlayerInput { move_dir: move_dir.into(), facing, run, sneak: false }, TICK_DT);
            let snap = server.tick();
            let view = snap.players.iter().find(|p| p.id == id).unwrap();
            assert_eq!(zombye_sim::PlayerState { mods: predicted.mods, ..view.state() }, predicted, "diverged at seq {seq}");
            predicted.mods = view.mods;
            if *mods_seen.last().unwrap() != view.mods {
                mods_seen.push(view.mods);
            }
            // Hurt at the end of the tick (as a zombie's strike is), so the snapshot carries it.
            let wound = match seq {
                40 => (Part::LeftFoot, WoundKind::Scratch),
                120 => (Part::RightLeg, WoundKind::Gash),
                _ => continue,
            };
            server.world.inflict_wound(id, wound.0, wound.1, Some(false)).unwrap();
            let snap = Snapshot::capture(&mut server.world);
            let view = snap.players.iter().find(|p| p.id == id).unwrap();
            predicted.mods = view.mods;
            mods_seen.push(view.mods);
            if seq == 120 {
                let mine = snap.clone().for_player(id);
                let h = &mine.private.iter().find(|v| v.player == id).expect("our health changed").health;
                assert_eq!(h.wounds.len(), 2);
                assert!(h.wounds.iter().any(|w| w.part == Part::RightLeg && w.kind == WoundKind::Gash && w.bleed_per_h > 0.0));
                assert!(h.tags.bleeding && h.tags.limping);
                assert!(snap.clone().for_player(other).private.iter().all(|v| v.player != id), "only our own client gets our wounds");
                assert!(view.health.bleeding && view.health.limping && !view.health.sick);
            }
        }
        assert_eq!(mods_seen.len(), 3, "{mods_seen:?}");
        assert!(!mods_seen[2].can_run && mods_seen[2].speed < mods_seen[1].speed);
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

    /// The floor a snapshot lists is exactly what the server takes: every listed item moves
    /// without a refusal, nothing within a metre and a bit but behind a wall is listed, and a
    /// taken item is gone from the next snapshot's list.
    #[test]
    fn the_floor_in_snapshots_is_what_the_server_accepts() {
        let mut server = server();
        let id = server.join(None);
        server.world.hold_item(id, zombye_sim::Slot::Back, "hiking_pack");
        let me = server.world.player(id).unwrap().state.pos;
        // A ring of towels round the player in the bedroom: some through its walls.
        let mut ring = Vec::new();
        for k in 0..12 {
            let at = me + glam::Vec2::from_angle(k as f32 * std::f32::consts::TAU / 12.0) * 1.2;
            ring.extend(server.world.spawn_item("towel", 1, at));
        }
        let snap = server.tick().for_player(id);
        let listed = snap.floor_of(id).to_vec();
        assert!(!listed.is_empty() && listed.len() < ring.len(), "some in reach, some behind walls: {} of {}", listed.len(), ring.len());
        assert_eq!(snap.floor.len(), 1, "only our own floor");
        for &g in &ring {
            server.send(Intent::MoveItem { player: id, from: Place::Ground, index: g, to: Place::Carried, count: 0 });
            let notes = server.tick().item_notes;
            let refused = notes.iter().any(|n| !n.ok && n.text.contains("towel"));
            assert_eq!(refused, !listed.contains(&g), "towel {g}: listed {}, notes {notes:?}", listed.contains(&g));
        }
        // Each listed one goes, and the snapshots stop listing it once it has.
        let mut snap = server.tick();
        for _ in 0..200 {
            if server.world.player(id).unwrap().jobs.is_empty() {
                break;
            }
            snap = server.tick();
            assert!(snap.item_notes.iter().all(|n| n.ok), "{:?}", snap.item_notes);
            for g in snap.floor_of(id) {
                assert!(server.world.items().iter().any(|it| it.id == *g), "lists {g}, which is gone");
            }
        }
        assert!(snap.floor_of(id).iter().all(|g| !listed.contains(g)), "{:?}", snap.floor_of(id));
    }
}
