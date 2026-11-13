//! Players: survivors driven by client input.
//!
//! Movement is one pure function, [`step_movement`], shared by the server tick and the client's
//! prediction of its own player. Both sides run it with the same map, input and [`TICK_DT`], so a
//! predicted position only diverges from the server's when the world changed under it (a door, later
//! a shove), and reconciliation fixes that. Climbing through an open or broken window is part of
//! movement (slow while inside the window opening), and depends only on the map, so it predicts
//! exactly too. What a player carries changes how it moves ([`MoveMods`], worked out by the server
//! from the load and part of [`PlayerState`]), so a client predicting from its snapshot state moves
//! exactly as the server does.
//!
//! [`TICK_DT`]: crate::TICK_DT

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::health::Health;
use crate::inventory::{Inventory, ItemJob};
use crate::map::{CLIMB_ZONE, TileMap};
use crate::needs::Needs;
use crate::run::RunStats;
use crate::skills::Skills;

/// Player body radius in metres.
pub const PLAYER_RADIUS: f32 = 0.3;
/// Movement speeds in m/s.
pub const WALK_SPEED: f32 = 3.0;
pub const RUN_SPEED: f32 = 5.5;
pub const SNEAK_SPEED: f32 = 1.4;
/// Walking while aiming (right mouse held): slower and steadier, facing the aim; no running.
pub const AIM_SPEED: f32 = 2.2;
/// Top speed while climbing through an open or broken window (whatever the stance). Crossing the
/// opening takes about a second.
pub const CLIMB_SPEED: f32 = 0.7;
/// Running needs more endurance than this.
pub const RUN_MIN_ENDURANCE: f32 = 0.05;
/// Once winded (endurance fell to [`RUN_MIN_ENDURANCE`]), you can't run again until it is back above this.
pub const RUN_RECOVER_ENDURANCE: f32 = 0.2;
/// How far past a climb zone a climb ends (and starts from), so the body is clear of it.
const CLIMB_CLEAR: f32 = 0.03;
/// Endurance per second spent running / regained standing still / walking / sneaking (the
/// defaults of [`MoveTuning`]).
pub const RUN_DRAIN: f32 = 0.06;
pub const REGEN: f32 = 0.05;
pub const WALK_REGEN: f32 = 0.02;
pub const SNEAK_REGEN: f32 = 0.03;
/// Longest single collision step, well under the body radius so walls can't be tunnelled through.
const MAX_SUBSTEP: f32 = 0.1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Stance {
    #[default]
    Walk = 0,
    Run = 1,
    Sneak = 2,
}

/// What a client asks its player to do this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerInput {
    /// Ground-plane direction, length ≤ 1 (analog sticks can ask for less than full speed).
    pub move_dir: Vec2,
    /// Facing angle in radians, 0 = +x, counter-clockwise toward +y.
    pub facing: f32,
    pub run: bool,
    pub sneak: bool,
    /// Aiming (the right mouse held): walks at [`AIM_SPEED`] (sneaking stays as slow), can't run, and
    /// only an aiming player attacks (the server's intents check it). Not saved (a loaded world's
    /// players aren't aiming until their next input).
    #[serde(skip)]
    pub aim: bool,
}

impl PlayerInput {
    /// Clamp untrusted input: finite values, `move_dir` no longer than 1.
    pub fn sanitized(self) -> Self {
        let move_dir = if self.move_dir.is_finite() { self.move_dir.clamp_length_max(1.0) } else { Vec2::ZERO };
        let facing = if self.facing.is_finite() { self.facing.rem_euclid(std::f32::consts::TAU) } else { 0.0 };
        Self { move_dir, facing, ..self }
    }
}

/// How a player's load, Fitness (#46), wounds (#9), needs and sleep (#11) change its movement.
/// Worked out by the server whenever what it carries, its skills, its wounds or its needs change
/// (`CarryTuning::mods`, then `SkillTuning::move_mods`, then `Health::apply_to`, then
/// `World::needs_mods`; asleep it doesn't move at all) and sent with the player's state, so
/// prediction uses the same numbers.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MoveMods {
    /// Multiplies every speed.
    pub speed: f32,
    /// Multiplies running speed on top of `speed` (leg wounds slow running more than walking, #9).
    pub run_speed: f32,
    /// Multiplies the endurance running drains.
    pub run_drain: f32,
    /// Multiplies the endurance regained walking or sneaking.
    pub regen: f32,
    pub can_run: bool,
    /// Can start climbing through an open or broken window.
    pub can_climb: bool,
    /// The most endurance there can be (tiredness lowers it: `NeedsTuning::tired_endurance`).
    /// Not saved with the player (the mods are worked out again on load) and sent on its own
    /// (`PlayerView::endurance_cap`), so the save and the player's layout stay as they were.
    #[serde(skip, default = "one")]
    pub max_endurance: f32,
}

fn one() -> f32 {
    1.0
}

impl Default for MoveMods {
    fn default() -> Self {
        Self { speed: 1.0, run_speed: 1.0, run_drain: 1.0, regen: 1.0, can_run: true, can_climb: true, max_endurance: 1.0 }
    }
}

/// The part of a player that movement changes, and that clients predict (with what it depends on
/// besides the map and the input: `mods`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    pub pos: Vec2,
    pub facing: f32,
    pub stance: Stance,
    /// 0..1. Running drains it.
    pub endurance: f32,
    /// Ran out of breath; can't run until endurance recovers to [`RUN_RECOVER_ENDURANCE`].
    pub winded: bool,
    /// From the load: set by the server, read by movement.
    pub mods: MoveMods,
    /// Over a fence or through an open or broken window, started with E (`World::player_climb`):
    /// while set, movement takes the body across by itself and the input's direction is ignored.
    /// Without one a climb zone is a wall. Saved in a section of its own (`crate::save`, `CLMB`),
    /// not here, so the player's layout (and the format) stays as it was.
    #[serde(skip)]
    pub climb: Option<Climb>,
    /// The floor it walks on: 0 the ground (and a flight of stairs, all the way up it), 1 the
    /// first upper floor. Movement changes it at the top end of a flight
    /// ([`TileMap::resolve_walker`]). Saved in a section of its own (`LEVL`), like the climb.
    #[serde(skip)]
    pub level: u8,
}

impl PlayerState {
    pub fn new(pos: Vec2) -> Self {
        Self { pos, facing: 0.0, stance: Stance::Walk, endurance: 1.0, winded: false, mods: MoveMods::default(), climb: None, level: 0 }
    }

    /// How high above the ground it stands (on the stairs, upstairs).
    pub fn height(&self, map: &TileMap) -> f32 {
        map.height_at(self.level, self.pos)
    }

    /// Off the ground floor: upstairs, or on a flight of stairs.
    pub fn off_ground(&self, map: &TileMap) -> bool {
        self.level > 0 || map.stair_at(self.pos).is_some()
    }
}

/// How fast a survivor moves and how its endurance (stamina) drains and comes back
/// (`WorldConfig::movement`; ours). Saved with the world's settings, and reachable from a
/// difficulty preset's `tuning`. The world hands it to its map (`TileMap::movement`), which is
/// what [`step_movement`] reads, so a client's prediction has the server's numbers too (sent in
/// the join's tunings). The `*_SPEED`, `RUN_DRAIN` and `*_ENDURANCE` constants are its defaults.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MoveTuning {
    /// Speeds in m/s, before load, wounds and needs (`MoveMods`).
    pub walk_speed: f32,
    pub run_speed: f32,
    pub sneak_speed: f32,
    /// Walking while aiming.
    pub aim_speed: f32,
    /// Top speed while climbing through a window or over a fence.
    pub climb_speed: f32,
    /// Endurance a second spent running.
    pub run_drain: f32,
    /// Endurance a second regained standing still (or sitting: `SitTuning::endurance_regen` on top).
    pub stand_regen: f32,
    /// ...walking (or aiming, or climbing): catching your breath on the move is slow.
    pub walk_regen: f32,
    /// ...sneaking.
    pub sneak_regen: f32,
    /// Running needs more endurance than this; running down to it winds you.
    pub run_min_endurance: f32,
    /// Winded, you can't run again until endurance is back above this.
    pub run_recover_endurance: f32,
}

impl Default for MoveTuning {
    fn default() -> Self {
        Self {
            walk_speed: WALK_SPEED,
            run_speed: RUN_SPEED,
            sneak_speed: SNEAK_SPEED,
            aim_speed: AIM_SPEED,
            climb_speed: CLIMB_SPEED,
            run_drain: RUN_DRAIN,
            stand_regen: REGEN,
            walk_regen: WALK_REGEN,
            sneak_regen: SNEAK_REGEN,
            run_min_endurance: RUN_MIN_ENDURANCE,
            run_recover_endurance: RUN_RECOVER_ENDURANCE,
        }
    }
}

impl MoveTuning {
    pub fn check(&self) -> Result<(), String> {
        for (name, v, lo, hi) in [
            ("walk_speed", self.walk_speed, 0.1, 20.0),
            ("run_speed", self.run_speed, 0.1, 30.0),
            ("sneak_speed", self.sneak_speed, 0.1, 20.0),
            ("aim_speed", self.aim_speed, 0.1, 20.0),
            ("climb_speed", self.climb_speed, 0.05, 10.0),
            ("run_drain", self.run_drain, 0.0, 10.0),
            ("stand_regen", self.stand_regen, 0.0, 10.0),
            ("walk_regen", self.walk_regen, 0.0, 10.0),
            ("sneak_regen", self.sneak_regen, 0.0, 10.0),
            ("run_min_endurance", self.run_min_endurance, 0.0, 0.9),
            ("run_recover_endurance", self.run_recover_endurance, 0.0, 1.0),
        ] {
            if !(v.is_finite() && (lo..=hi).contains(&v)) {
                return Err(format!("movement: {name} {v} ({lo} to {hi})"));
            }
        }
        if self.run_recover_endurance < self.run_min_endurance {
            return Err(format!("movement: run_recover_endurance {} below run_min_endurance {}", self.run_recover_endurance, self.run_min_endurance));
        }
        Ok(())
    }
}

/// Walking between floors (`WorldConfig::stairs`; ours). The world hands `speed` to its map
/// (`TileMap::stair_speed`), which is what movement reads, so a client's prediction has it too.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StairTuning {
    /// Speed on a flight of stairs, as a share of the speed on the flat (walking, running or
    /// sneaking alike).
    pub speed: f32,
    /// Zombies neither see nor strike a player standing higher than this above the ground, in
    /// metres (part-way up a flight; upstairs is always out of their sight and reach).
    pub out_of_reach: f32,
}

impl Default for StairTuning {
    fn default() -> Self {
        Self { speed: 0.4, out_of_reach: 1.6 }
    }
}

impl StairTuning {
    pub fn check(&self) -> Result<(), String> {
        if !(self.speed.is_finite() && (0.05..=1.0).contains(&self.speed)) {
            return Err(format!("stairs: speed {} (0.05 to 1)", self.speed));
        }
        if !(self.out_of_reach.is_finite() && (0.0..=10.0).contains(&self.out_of_reach)) {
            return Err(format!("stairs: out_of_reach {} (0 to 10)", self.out_of_reach));
        }
        Ok(())
    }
}

/// A climb under way over a fence or through an open or broken window: the body walks to the
/// near side of the edge's climb zone (if it isn't there yet), then crosses it at [`CLIMB_SPEED`]
/// to just past the far side.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Climb {
    /// The point on the edge's line it crosses at.
    pub at: Vec2,
    /// Unit, square across the edge, the way it goes.
    pub dir: Vec2,
    /// Seconds before it gives up (something stands in the way).
    pub left: f32,
}

impl Climb {
    /// How far along `dir` the body at `p` is from the edge's line (negative: still on the near side).
    pub fn across(&self, p: Vec2) -> f32 {
        (p - self.at).dot(self.dir)
    }
}

/// When pressing E climbs (`WorldConfig::climb`; ours). The client gets it on joining, so it
/// sends a climb only where the server takes one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClimbTuning {
    /// Furthest from the body's middle to the fence or window, in metres.
    pub reach: f32,
    /// How far off square on to the edge the player may face, in degrees either side.
    pub facing_degrees: f32,
    /// A climb that hasn't got across in this many seconds gives up.
    pub give_up_secs: f32,
}

impl Default for ClimbTuning {
    fn default() -> Self {
        Self { reach: 1.1, facing_degrees: 70.0, give_up_secs: 5.0 }
    }
}

impl ClimbTuning {
    pub fn check(&self) -> Result<(), String> {
        let ok = |v: f32, lo: f32, hi: f32| v.is_finite() && (lo..=hi).contains(&v);
        if !ok(self.reach, CLIMB_ZONE + CLIMB_CLEAR, 5.0) {
            return Err(format!("climb.reach {} must be {} to 5 m", self.reach, CLIMB_ZONE + CLIMB_CLEAR));
        }
        if !ok(self.facing_degrees, 1.0, 180.0) {
            return Err(format!("climb.facing_degrees {} must be 1 to 180", self.facing_degrees));
        }
        if !ok(self.give_up_secs, 0.5, 60.0) {
            return Err(format!("climb.give_up_secs {} must be 0.5 to 60", self.give_up_secs));
        }
        Ok(())
    }
}

/// The fence or open or broken window a player at `pos` facing `facing` would climb with E, if
/// any: within `reach` of its middle, faced within `facing_degrees` of square on, nothing that
/// blocks walking between, and room on the far side. The nearest and most squarely faced wins.
/// Pure: the client asks it before sending a climb, the server again when it takes one.
pub fn climb_from(map: &TileMap, pos: Vec2, facing: f32, t: &ClimbTuning) -> Option<Climb> {
    use crate::map::Side;
    use glam::IVec2;
    let face = Vec2::from_angle(facing);
    let min_dot = t.facing_degrees.to_radians().cos();
    let r = t.reach.ceil() as i32 + 1;
    let base = pos.floor().as_ivec2();
    let mut best: Option<(f32, Climb)> = None;
    for dy in -r..=r {
        for dx in -r..=r {
            let tile = base + IVec2::new(dx, dy);
            if !map.in_bounds(tile) {
                continue;
            }
            // The north edge runs along x at y = tile.y, the west edge along y at x = tile.x.
            for (side, along, normal) in [(Side::N, Vec2::X, Vec2::Y), (Side::W, Vec2::Y, Vec2::X)] {
                if !map.edge(tile, side).is_climbable() {
                    continue;
                }
                let start = tile.as_vec2();
                let a = (pos - start).dot(along).clamp(0.35, 0.65);
                let at = start + along * a;
                let s = (pos - at).dot(normal);
                let dir = if s.abs() > 1e-3 {
                    -normal * s.signum()
                } else if face.dot(normal) >= 0.0 {
                    normal
                } else {
                    -normal
                };
                // Past it already (out of its far side): nothing to climb.
                if (pos - at).dot(dir) > CLIMB_ZONE {
                    continue;
                }
                let d = pos.distance(at);
                if d > t.reach || face.dot(dir) < min_dot {
                    continue;
                }
                let near = at - dir * (CLIMB_ZONE + CLIMB_CLEAR);
                let far = at + dir * (CLIMB_ZONE + CLIMB_CLEAR + 0.3);
                if map.tile_blocked(far.floor().as_ivec2()) || !crate::vision::clear_reach(map, pos, near) {
                    continue;
                }
                let score = d - face.dot(dir) * 0.5;
                if best.is_none_or(|(b, _)| score < b) {
                    best = Some((score, Climb { at, dir, left: t.give_up_secs }));
                }
            }
        }
    }
    best.map(|(_, c)| c)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Player {
    /// Stable network id. Players have their own id space, starting at 1.
    pub id: u32,
    pub state: PlayerState,
    /// Latest input; applied once per tick until a newer one arrives.
    pub input: PlayerInput,
    /// Sequence number of `input`, echoed in snapshots so the client knows what the server has applied.
    pub last_input_seq: u32,
    /// Hit points (see `CombatTuning::player_hp`); dead at 0.
    pub hp: f32,
    /// Wounds on body parts, the infection, food poisoning (see `crate::health`).
    pub health: Health,
    /// Hunger, thirst, tiredness and sleep (see `crate::needs`).
    pub needs: Needs,
    /// XP in each skill (see `crate::skills`).
    pub skills: Skills,
    /// Dead players ignore input and aren't seen or heard. Single-player can respawn.
    pub dead: bool,
    /// The weapon in hand (index into `WorldConfig::weapons`); `None` = bare hands. Derived from
    /// `inventory.hand` whenever it changes (`World::refresh_carry`).
    pub weapon: Option<u16>,
    /// What it carries, holds and wears.
    pub inventory: Inventory,
    /// The container it has open (searched), if any: its contents go to this player's client.
    pub open: Option<u32>,
    /// Item moves, searches and uses queued, the first under way (see `crate::inventory`).
    pub jobs: Vec<ItemJob>,
    /// The swing, shove or stomp under way, until it lands.
    pub action: Option<Action>,
    /// Seconds until the next action can start.
    pub ready_in: f32,
    /// An action asked for just before the last one finished; starts as soon as it can.
    pub queued: Option<ActionKind>,
    /// Reeling or on the ground after another player's blow (PvP, `crate::combat`): no moving, no
    /// actions until it runs out. Saved in a section of its own (`crate::save`), not here, so the
    /// player's layout (and the format) stays as it was.
    #[serde(skip)]
    pub knock: Option<Knock>,
    /// Sitting down, and on what ([`crate::sit`]): no moving, breath back faster. Saved in a
    /// section of its own (`SEAT`), like `knock`.
    #[serde(skip)]
    pub seated: Option<crate::sit::SeatKind>,
    /// Logged out (#68, `crate::logout`): nobody plays it, and it lies where it was left, asleep and
    /// defenceless, for this many more seconds before it leaves the world. Saved in a section of
    /// its own, like `knock`.
    #[serde(skip)]
    pub away: Option<f32>,
    /// Spawn protection (#68): seconds left in which other players' blows pass it by. Starting a
    /// swing, shove or stomp ends it. Saved with `away`.
    #[serde(skip)]
    pub protected: f32,
    /// Developer toggles. Not saved: a loaded world has them off.
    #[serde(skip)]
    pub dev: DevFlags,
    /// This life's stats and how it ended (`crate::run`). Saved in its own section of the save,
    /// not here, so the player's layout (and the format) stays as it was.
    #[serde(skip)]
    pub run: RunStats,
    /// This life's first day and night: objectives and the outcome at dawn (`crate::firstnight`).
    /// Saved in its own optional section, like `run`; `None` in a save from before it.
    #[serde(skip)]
    pub first_night: Option<crate::firstnight::FirstNight>,
    /// What this survivor has seen of the map (`crate::explore`). Saved in its own optional
    /// section, like `run`; blank in a save from before it.
    #[serde(skip)]
    pub explored: crate::explore::Explored,
    /// The tick of the snapshot this player's client was showing when it sent its latest input
    /// (`World::player_saw`; lag compensation, `crate::lagcomp`). 0: never said (a local player),
    /// so nothing is rewound for it. Not saved: a client says again every tick.
    #[serde(skip)]
    pub seen_tick: u64,
    /// The gesture it is making (#67, `crate::speech`): a pose others see, nothing more. Not saved,
    /// not in the state hash.
    #[serde(skip)]
    pub gesture: Option<crate::speech::Gesturing>,
    /// Its hands are tied (#73, `crate::bonds`). Saved in a section of its own, like `knock`, and
    /// in the state hash only while set.
    #[serde(skip)]
    pub restraint: Option<crate::bonds::Restraint>,
    /// What its hands are doing to another survivor: tying, freeing, searching, taking. Not
    /// saved (a load calls it off, as it lowers raised hands).
    #[serde(skip)]
    pub hands: Option<crate::bonds::HandsJob>,
    /// The tied survivor whose things it is going through (a player id). Not saved.
    #[serde(skip)]
    pub frisking: Option<u32>,
    /// How far up (+) or down (-) of level it aims, in radians (#72, `World::set_pitch`): with the
    /// facing, the way a shot goes. The isometric client works it out from what the cursor is on;
    /// a first-person one would send its camera's. Like the aim itself, the client's latest word:
    /// not saved (level after a load, until the next), not in the state hash.
    #[serde(skip)]
    pub pitch: f32,
    /// Who this survivor was: its Background, Strengths and Flaws and what they do
    /// (`crate::origin`). Saved in a section of its own (`ORIG`), so not here.
    #[serde(skip)]
    pub origin: crate::origin::Origin,
    /// The vehicle it is at the wheel of (`crate::vehicles`, by id): its body sits in the seat
    /// and doesn't walk. The vehicle names its driver in the save (`VEHC`), so not here.
    #[serde(skip)]
    pub vehicle: Option<u32>,
}

/// A player staggered or knocked down by another player's swing or shove (see `combat`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Knock {
    /// On the ground (a stomp can hit it; swings and shoves pass over), else reeling on its feet.
    pub down: bool,
    /// Seconds until it is over.
    pub timer: f32,
}

/// Developer toggles for testing without dying (`World::set_dev`; `net::Intent::Dev`, F1 / F2 in
/// the client, MCP `dev`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevFlags {
    /// Takes no damage and can't die: zombie strikes still land (events, hit feedback), HP just
    /// doesn't drop (`World::hurt_player`).
    pub god: bool,
    /// Zombies don't see or hear this player, and those after it lose it.
    pub ignored: bool,
}

impl Player {
    pub fn new(id: u32, pos: Vec2, hp: f32) -> Self {
        Self {
            id,
            state: PlayerState::new(pos),
            input: PlayerInput::default(),
            last_input_seq: 0,
            hp,
            health: Health::default(),
            needs: Needs::default(),
            skills: Skills::default(),
            dead: false,
            weapon: None,
            inventory: Inventory::default(),
            open: None,
            jobs: Vec::new(),
            action: None,
            ready_in: 0.0,
            queued: None,
            knock: None,
            seated: None,
            away: None,
            protected: 0.0,
            dev: DevFlags::default(),
            run: RunStats::default(),
            first_night: None,
            explored: crate::explore::Explored::default(),
            seen_tick: 0,
            gesture: None,
            restraint: None,
            hands: None,
            frisking: None,
            pitch: 0.0,
            origin: crate::origin::Origin::default(),
            vehicle: None,
        }
    }
}

/// What a player can do with their hands (see `combat`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ActionKind {
    /// Swing the equipped weapon (or punch).
    Swing = 0,
    /// Push zombies in front back; always staggers them.
    Shove = 1,
    /// Stamp on a downed zombie at your feet.
    Stomp = 2,
}

/// An action between starting and landing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub kind: ActionKind,
    /// Seconds until it lands.
    pub timer: f32,
    /// Strength multiplier from endurance when it started (1 = rested).
    pub power: f32,
    /// The weapon swung (index into `WorldConfig::weapons`).
    pub weapon: u16,
}

/// Advance one player by `dt` seconds of `input`. Pure: the same arguments always give the same result.
pub fn step_movement(map: &TileMap, state: &mut PlayerState, input: &PlayerInput, dt: f32) {
    let mut input = input.sanitized();
    // A climb under way walks the body across by itself: to the near side of the climb zone,
    // then over to just past the far side, facing the way it goes; it stops there, or gives up.
    let mut max_step = f32::INFINITY;
    if let Some(mut c) = state.climb {
        c.left -= dt;
        let s = c.across(state.pos);
        if s >= CLIMB_ZONE + CLIMB_CLEAR || c.left <= 0.0 || !state.mods.can_climb {
            state.climb = None;
        } else {
            let near = c.at - c.dir * (CLIMB_ZONE + CLIMB_CLEAR);
            let target = if s < -CLIMB_ZONE && state.pos.distance(near) > 0.02 { near } else { c.at + c.dir * (CLIMB_ZONE + CLIMB_CLEAR + 0.01) };
            let to = target - state.pos;
            max_step = to.length();
            input = PlayerInput { move_dir: to.normalize_or_zero(), facing: c.dir.to_angle().rem_euclid(std::f32::consts::TAU), run: false, aim: false, ..input };
            state.climb = Some(c);
        }
    }
    state.facing = input.facing;
    let moving = input.move_dir.length_squared() > 1e-4;

    let t = map.movement;
    if state.winded && state.endurance > t.run_recover_endurance {
        state.winded = false;
    }
    let mods = state.mods;
    let can_run = mods.can_run && !state.winded && state.endurance > t.run_min_endurance;
    // Running beats sneaking: holding run while sneak is toggled on means "get out of here".
    state.stance = if input.run && moving && can_run && !input.aim {
        Stance::Run
    } else if input.sneak {
        Stance::Sneak
    } else {
        Stance::Walk
    };

    let cap = mods.max_endurance.clamp(0.0, 1.0);
    match state.stance {
        Stance::Run => {
            state.endurance = (state.endurance - t.run_drain * mods.run_drain * dt).max(0.0);
            if state.endurance <= t.run_min_endurance {
                state.winded = true;
            }
        }
        // Breath comes back fastest standing still, slowly on the move.
        _ if !moving => state.endurance += t.stand_regen * mods.regen * dt,
        Stance::Sneak => state.endurance += t.sneak_regen * mods.regen * dt,
        Stance::Walk => state.endurance += t.walk_regen * mods.regen * dt,
    }
    // Tired, the bar doesn't fill all the way (and what's over the cap is gone).
    state.endurance = state.endurance.min(cap);

    if !moving {
        return;
    }
    let mut speed = mods.speed
        * match state.stance {
            Stance::Run => t.run_speed * mods.run_speed,
            Stance::Walk if input.aim => t.aim_speed,
            Stance::Walk => t.walk_speed,
            Stance::Sneak => t.sneak_speed,
        };
    // Upstairs and on a flight of stairs the ground floor's windows and fences are out of it.
    let mut level = state.level;
    let on_stairs = level == 0 && map.stair_at(state.pos).is_some();
    let climbing = level == 0 && !on_stairs && map.in_window_opening(state.pos);
    if climbing {
        speed = speed.min(t.climb_speed);
    }
    if on_stairs && map.stair_speed > 0.0 {
        speed *= map.stair_speed;
    }
    let step = (input.move_dir * speed * dt).clamp_length_max(max_step);
    // Sub-step so fast movement (or a long dt) can't tunnel through thin walls.
    let n = (step.length() / MAX_SUBSTEP).ceil().max(1.0) as u32;
    let part = step / n as f32;
    // Fences and open windows are climbed only with E (a climb under way); otherwise their climb
    // zone is a wall, which the body slides along. One already in a zone (a climb given up, a
    // load) can walk out of it.
    let may_enter = climbing || (mods.can_climb && state.climb.is_some());
    let mut p = state.pos;
    for _ in 0..n {
        let mut next = map.resolve_walker(p + part, PLAYER_RADIUS, level);
        let stair = if level == 0 { map.stair_at(p) } else { None };
        if level > 0 {
            // Upstairs: stepping into the stairwell (only its top end is open) is onto the flight.
            if map.stair_at(next).is_some() {
                level = 0;
            }
        } else if let Some(s) = stair {
            // On a flight: past its top end is the upper floor.
            if s.progress(next) >= 1.0 {
                level = 1;
                next = map.resolve_walker(next, PLAYER_RADIUS, level);
            }
        } else if !may_enter && map.stair_at(next).is_none() && map.in_window_opening(next) {
            let slide = [Vec2::new(part.x, 0.0), Vec2::new(0.0, part.y)]
                .map(|d| map.resolve_walker(p + d, PLAYER_RADIUS, level))
                .into_iter()
                .filter(|q| !map.in_window_opening(*q))
                .max_by(|a, b| a.distance_squared(p).total_cmp(&b.distance_squared(p)));
            match slide {
                Some(q) => next = q,
                None => break,
            }
        }
        p = next;
    }
    state.pos = p;
    state.level = level;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TICK_DT;
    use crate::map::{Edge, Side};
    use glam::IVec2;

    /// A 32 × 16 m field with a wall running north-south along x = 16.
    fn walled_map() -> TileMap {
        let mut map = TileMap::new(2, 1);
        for y in 0..map.height() {
            map.set_edge(IVec2::new(16, y), Side::W, Edge::Wall);
        }
        map
    }

    fn run_east() -> PlayerInput {
        PlayerInput { move_dir: Vec2::X, facing: 0.0, run: true, sneak: false, aim: false }
    }

    #[test]
    fn cannot_walk_or_run_through_walls() {
        let map = walled_map();
        for (run, dt) in [(false, TICK_DT), (true, TICK_DT), (true, 0.5), (true, 3.0)] {
            let mut s = PlayerState::new(Vec2::new(10.5, 8.5));
            let input = PlayerInput { run, ..run_east() };
            for _ in 0..200 {
                step_movement(&map, &mut s, &input, dt);
                assert!(s.pos.x < 16.0 - PLAYER_RADIUS + 0.01, "crossed the wall (run {run}, dt {dt}): {}", s.pos);
            }
            // Diagonally into the wall too.
            let input = PlayerInput { move_dir: Vec2::new(1.0, 1.0).normalize(), ..input };
            for _ in 0..200 {
                step_movement(&map, &mut s, &input, dt);
                assert!(s.pos.x < 16.0, "crossed the wall diagonally (run {run}, dt {dt}): {}", s.pos);
            }
        }
    }

    /// Aiming walks slower, can't run (the run key is ignored), and sneaking stays as slow.
    #[test]
    fn aiming_is_slower_and_never_runs() {
        let map = TileMap::new(4, 1);
        let start = Vec2::new(5.5, 8.5);
        let one_second = |input: PlayerInput| {
            let mut s = PlayerState::new(start);
            for _ in 0..20 {
                step_movement(&map, &mut s, &input, TICK_DT);
            }
            (s.pos.distance(start), s.stance)
        };
        let (d, stance) = one_second(PlayerInput { aim: true, ..run_east() });
        assert!((d - AIM_SPEED).abs() < 0.01 && stance == Stance::Walk, "aiming with run held: {d} {stance:?}");
        let (d, stance) = one_second(PlayerInput { aim: true, run: false, sneak: true, ..run_east() });
        assert!((d - SNEAK_SPEED).abs() < 0.01 && stance == Stance::Sneak, "aiming sneak: {d}");
    }

    #[test]
    fn speeds_match_stance() {
        let map = TileMap::new(4, 1);
        let start = Vec2::new(5.5, 8.5);
        let distance = |input: PlayerInput| {
            let mut s = PlayerState::new(start);
            for _ in 0..20 {
                step_movement(&map, &mut s, &input, TICK_DT);
            }
            (s.pos.distance(start), s.stance)
        };
        let walk = PlayerInput { run: false, ..run_east() };
        let (d, stance) = distance(walk);
        assert!((d - WALK_SPEED).abs() < 0.01 && stance == Stance::Walk, "walk {d} {stance:?}");
        let (d, stance) = distance(run_east());
        assert!((d - RUN_SPEED).abs() < 0.01 && stance == Stance::Run, "run {d} {stance:?}");
        let (d, stance) = distance(PlayerInput { sneak: true, ..walk });
        assert!((d - SNEAK_SPEED).abs() < 0.01 && stance == Stance::Sneak, "sneak {d} {stance:?}");
        // Over-long input is clamped to full speed.
        let (d, _) = distance(PlayerInput { move_dir: Vec2::new(50.0, 0.0), ..walk });
        assert!((d - WALK_SPEED).abs() < 0.01, "clamped {d}");
        // Standing still with run held doesn't count as running.
        let (d, stance) = distance(PlayerInput { move_dir: Vec2::ZERO, ..run_east() });
        assert!(d == 0.0 && stance == Stance::Walk);
    }

    #[test]
    fn endurance_drains_while_running_and_regenerates() {
        // Run back and forth in an open field so walls don't stop us.
        let map = TileMap::new(8, 8);
        let mut s = PlayerState::new(Vec2::new(64.0, 64.0));
        let ticks = |secs: f32| (secs / TICK_DT).round() as u32;
        for _ in 0..ticks(5.0) {
            step_movement(&map, &mut s, &run_east(), TICK_DT);
        }
        assert!((s.endurance - (1.0 - 5.0 * RUN_DRAIN)).abs() < 0.01, "after 5 s running: {}", s.endurance);

        // Run until winded: then you walk, even with run held.
        let run_west = PlayerInput { move_dir: -Vec2::X, ..run_east() };
        for _ in 0..ticks(20.0) {
            step_movement(&map, &mut s, &run_west, TICK_DT);
            if s.winded {
                break;
            }
        }
        assert!(s.winded && s.endurance <= RUN_MIN_ENDURANCE + 0.01, "winded: {s:?}");
        step_movement(&map, &mut s, &run_west, TICK_DT);
        assert_eq!(s.stance, Stance::Walk);

        // Rest: endurance comes back and running is possible again.
        let before = s.endurance;
        for _ in 0..ticks(4.0) {
            step_movement(&map, &mut s, &PlayerInput::default(), TICK_DT);
        }
        assert!((s.endurance - (before + 4.0 * REGEN)).abs() < 0.01, "after 4 s rest: {}", s.endurance);
        step_movement(&map, &mut s, &run_east(), TICK_DT);
        assert_eq!(s.stance, Stance::Run);

        // Regeneration caps at 1.
        for _ in 0..ticks(60.0) {
            step_movement(&map, &mut s, &PlayerInput::default(), TICK_DT);
        }
        assert_eq!(s.endurance, 1.0);
    }

    /// Breath comes back fastest standing still, slowly walking; the map's tuning is what moves
    /// a body, and a cap (tiredness) holds the bar down.
    #[test]
    fn endurance_comes_back_slower_on_the_move() {
        let map = TileMap::new(8, 8);
        let ticks = |secs: f32| (secs / TICK_DT).round() as u32;
        let walk = PlayerInput { run: false, ..run_east() };
        let mut s = PlayerState::new(Vec2::new(64.0, 64.0));
        s.endurance = 0.2;
        for _ in 0..ticks(5.0) {
            step_movement(&map, &mut s, &walk, TICK_DT);
        }
        assert!((s.endurance - (0.2 + 5.0 * WALK_REGEN)).abs() < 0.01, "walking: {}", s.endurance);
        let mut s = PlayerState::new(Vec2::new(64.0, 64.0));
        s.endurance = 0.2;
        for _ in 0..ticks(5.0) {
            step_movement(&map, &mut s, &PlayerInput::default(), TICK_DT);
        }
        assert!((s.endurance - (0.2 + 5.0 * REGEN)).abs() < 0.01, "standing: {}", s.endurance);
        // Tired: the bar stops at the cap.
        s.mods.max_endurance = 0.4;
        for _ in 0..ticks(20.0) {
            step_movement(&map, &mut s, &PlayerInput::default(), TICK_DT);
        }
        assert!((s.endurance - 0.4).abs() < 1e-5, "capped: {}", s.endurance);
        // Speeds and drain come from the map's tuning.
        let mut map = TileMap::new(8, 8);
        map.movement = MoveTuning { run_speed: 8.0, run_drain: 0.5, ..MoveTuning::default() };
        let mut s = PlayerState::new(Vec2::new(64.0, 64.0));
        step_movement(&map, &mut s, &run_east(), 1.0);
        assert!((s.pos.x - 72.0).abs() < 0.01 && (s.endurance - 0.5).abs() < 0.01, "{s:?}");
        assert!(MoveTuning::default().check().is_ok());
        assert!(MoveTuning { run_recover_endurance: 0.01, ..MoveTuning::default() }.check().is_err());
    }

    /// An open window is a way through, but only with E (a climb), and climbing it is slow; a shut
    /// one is a wall, and so is an open one walked into without E.
    #[test]
    fn climbing_through_an_open_window_is_slow() {
        let mut map = walled_map();
        let window = IVec2::new(16, 8);
        let tuning = ClimbTuning::default();
        let cross = |map: &TileMap, press_e: bool| {
            let mut s = PlayerState::new(Vec2::new(14.5, 8.5));
            for t in 1..=200 {
                if press_e && s.climb.is_none() {
                    s.climb = climb_from(map, s.pos, s.facing, &tuning);
                }
                step_movement(map, &mut s, &PlayerInput { run: false, ..run_east() }, TICK_DT);
                if s.pos.x > 18.5 {
                    return Some(t);
                }
            }
            None
        };
        map.set_edge(window, Side::W, Edge::Window);
        assert_eq!(cross(&map, true), None, "walked through a shut window");
        let plain = TileMap::new(2, 1);
        let walk = cross(&plain, false).unwrap();
        for kind in [Edge::WindowOpen, Edge::WindowBroken, Edge::Fence] {
            map.set_edge(window, Side::W, kind);
            assert_eq!(cross(&map, false), None, "{kind:?}: climbed without E");
            let climb = cross(&map, true).expect("climbs through");
            // About 0.7 m of opening at climb speed instead of walking pace.
            assert!(climb as f32 * TICK_DT > walk as f32 * TICK_DT + 0.6, "{kind:?}: climbed in {climb} ticks, walked in {walk}");
        }
        assert!(map.in_window_opening(Vec2::new(16.1, 8.5)) && !map.in_window_opening(Vec2::new(15.5, 8.5)));
        // Walking along the wall past the window isn't climbing.
        assert!(!map.in_window_opening(Vec2::new(16.0 - PLAYER_RADIUS - 0.08, 8.5)));
    }

    /// E climbs only what is in reach and faced, with room beyond; the climb takes the body across
    /// whatever the input says, and ends just past the far side.
    #[test]
    fn e_climbs_what_is_faced_and_in_reach() {
        let mut map = TileMap::new(2, 1);
        for y in 0..16 {
            map.set_edge(IVec2::new(16, y), Side::W, Edge::Fence);
        }
        let t = ClimbTuning::default();
        let at = |x: f32, y: f32| Vec2::new(x, y);
        assert!(climb_from(&map, at(15.2, 8.3), 0.0, &t).is_some(), "facing it, close");
        assert!(climb_from(&map, at(15.2, 8.3), std::f32::consts::PI, &t).is_none(), "back to it");
        assert!(climb_from(&map, at(14.0, 8.3), 0.0, &t).is_none(), "out of reach");
        // Off square within the arc still climbs, crossing square on at the tile it is beside.
        let c = climb_from(&map, at(15.4, 8.9), 0.6, &t).expect("a little off square");
        assert_eq!(c.dir, Vec2::X);
        assert!((c.at - at(16.0, 8.65)).length() < 1e-4, "{}", c.at);
        // A tree right behind it: no room to land.
        let mut treed = map.clone();
        treed.tile_mut(IVec2::new(16, 8)).unwrap().feature = crate::map::Feature::Tree;
        assert!(climb_from(&treed, at(15.2, 8.5), 0.0, &t).is_none());
        // Walking backwards the whole time: the climb still goes over, then lets go.
        let mut s = PlayerState::new(at(15.2, 8.5));
        s.climb = climb_from(&map, s.pos, 0.0, &t);
        let back = PlayerInput { move_dir: -Vec2::X, facing: 0.0, ..Default::default() };
        for _ in 0..60 {
            step_movement(&map, &mut s, &back, TICK_DT);
        }
        assert!(s.climb.is_none() && s.pos.x > 16.0 + CLIMB_ZONE, "{} {:?}", s.pos, s.climb);
        // Then walking back is a wall again.
        for _ in 0..40 {
            step_movement(&map, &mut s, &back, TICK_DT);
        }
        assert!(s.pos.x > 16.0 + CLIMB_ZONE - 1e-3, "walked back over without E: {}", s.pos);
        // Walking along the fence diagonally slides along it instead of sticking.
        let mut s = PlayerState::new(at(15.6, 4.5));
        let diag = PlayerInput { move_dir: Vec2::new(1.0, 1.0).normalize(), facing: 0.8, ..Default::default() };
        for _ in 0..30 {
            step_movement(&map, &mut s, &diag, TICK_DT);
        }
        assert!(s.pos.y > 6.0 && s.pos.x < 16.0, "{}", s.pos);
    }

    /// A heavy load slows movement, drains running faster and can forbid running and climbing.
    #[test]
    fn load_modifiers_change_movement() {
        let map = TileMap::new(4, 1);
        let start = Vec2::new(5.5, 8.5);
        let go = |mods: MoveMods, input: PlayerInput| {
            let mut s = PlayerState { mods, ..PlayerState::new(start) };
            for _ in 0..20 {
                step_movement(&map, &mut s, &input, TICK_DT);
            }
            s
        };
        let slow = MoveMods { speed: 0.5, run_drain: 2.0, ..MoveMods::default() };
        let s = go(slow, PlayerInput { run: false, ..run_east() });
        assert!((s.pos.distance(start) - WALK_SPEED * 0.5).abs() < 0.01);
        let s = go(slow, run_east());
        assert_eq!(s.stance, Stance::Run);
        assert!((s.endurance - (1.0 - 2.0 * RUN_DRAIN)).abs() < 0.01, "{}", s.endurance);
        let s = go(MoveMods { can_run: false, ..MoveMods::default() }, run_east());
        assert_eq!(s.stance, Stance::Walk);
        // Can't climb: an open window stops you like a wall.
        let mut map = walled_map();
        map.set_edge(IVec2::new(16, 8), Side::W, Edge::WindowOpen);
        let mut s = PlayerState { mods: MoveMods { can_climb: false, ..MoveMods::default() }, ..PlayerState::new(Vec2::new(14.5, 8.5)) };
        for _ in 0..100 {
            step_movement(&map, &mut s, &PlayerInput { run: false, ..run_east() }, TICK_DT);
        }
        assert!(s.pos.x < 16.0 && !map.in_window_opening(s.pos), "climbed out overloaded: {}", s.pos);
    }

    #[test]
    fn bad_input_is_clamped() {
        let i = PlayerInput { move_dir: Vec2::new(f32::NAN, 1.0), facing: f32::INFINITY, ..Default::default() }.sanitized();
        assert_eq!(i.move_dir, Vec2::ZERO);
        assert_eq!(i.facing, 0.0);
    }
}
