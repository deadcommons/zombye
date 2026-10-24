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
use crate::map::TileMap;
use crate::needs::Needs;
use crate::run::RunStats;
use crate::skills::Skills;

/// Player body radius in metres.
pub const PLAYER_RADIUS: f32 = 0.3;
/// Movement speeds in m/s.
pub const WALK_SPEED: f32 = 3.0;
pub const RUN_SPEED: f32 = 5.5;
pub const SNEAK_SPEED: f32 = 1.4;
/// Top speed while climbing through an open or broken window (whatever the stance). Crossing the
/// opening takes about a second.
pub const CLIMB_SPEED: f32 = 0.7;
/// Running needs more endurance than this.
pub const RUN_MIN_ENDURANCE: f32 = 0.05;
/// Once winded (endurance fell to [`RUN_MIN_ENDURANCE`]), you can't run again until it is back above this.
pub const RUN_RECOVER_ENDURANCE: f32 = 0.2;
/// Endurance per second spent running / regained otherwise.
pub const RUN_DRAIN: f32 = 0.06;
pub const REGEN: f32 = 0.05;
pub const SNEAK_REGEN: f32 = 0.04;
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
}

impl Default for MoveMods {
    fn default() -> Self {
        Self { speed: 1.0, run_speed: 1.0, run_drain: 1.0, regen: 1.0, can_run: true, can_climb: true }
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
}

impl PlayerState {
    pub fn new(pos: Vec2) -> Self {
        Self { pos, facing: 0.0, stance: Stance::Walk, endurance: 1.0, winded: false, mods: MoveMods::default() }
    }
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
    /// Developer toggles. Not saved: a loaded world has them off.
    #[serde(skip)]
    pub dev: DevFlags,
    /// This life's stats and how it ended (`crate::run`). Saved in its own section of the save,
    /// not here, so the player's layout (and the format) stays as it was.
    #[serde(skip)]
    pub run: RunStats,
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
            dev: DevFlags::default(),
            run: RunStats::default(),
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
    let input = input.sanitized();
    state.facing = input.facing;
    let moving = input.move_dir.length_squared() > 1e-4;

    if state.winded && state.endurance > RUN_RECOVER_ENDURANCE {
        state.winded = false;
    }
    let mods = state.mods;
    let can_run = mods.can_run && !state.winded && state.endurance > RUN_MIN_ENDURANCE;
    // Running beats sneaking: holding run while sneak is toggled on means "get out of here".
    state.stance = if input.run && moving && can_run {
        Stance::Run
    } else if input.sneak {
        Stance::Sneak
    } else {
        Stance::Walk
    };

    match state.stance {
        Stance::Run => {
            state.endurance = (state.endurance - RUN_DRAIN * mods.run_drain * dt).max(0.0);
            if state.endurance <= RUN_MIN_ENDURANCE {
                state.winded = true;
            }
        }
        Stance::Sneak => state.endurance = (state.endurance + SNEAK_REGEN * mods.regen * dt).min(1.0),
        Stance::Walk => state.endurance = (state.endurance + REGEN * mods.regen * dt).min(1.0),
    }

    if !moving {
        return;
    }
    let mut speed = mods.speed
        * match state.stance {
            Stance::Run => RUN_SPEED * mods.run_speed,
            Stance::Walk => WALK_SPEED,
            Stance::Sneak => SNEAK_SPEED,
        };
    let climbing = map.in_window_opening(state.pos);
    if climbing {
        speed = speed.min(CLIMB_SPEED);
    }
    let step = input.move_dir * speed * dt;
    // Sub-step so fast movement (or a long dt) can't tunnel through thin walls.
    let n = (step.length() / MAX_SUBSTEP).ceil().max(1.0) as u32;
    let part = step / n as f32;
    let mut p = state.pos;
    for _ in 0..n {
        let next = map.resolve_circle(p + part, PLAYER_RADIUS);
        // Too loaded to climb: a window opening is a wall (unless already in it).
        if !mods.can_climb && !climbing && map.in_window_opening(next) {
            break;
        }
        p = next;
    }
    state.pos = p;
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
        PlayerInput { move_dir: Vec2::X, facing: 0.0, run: true, sneak: false }
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

    /// An open window is a way through, but climbing it is slow; a shut one is a wall.
    #[test]
    fn climbing_through_an_open_window_is_slow() {
        let mut map = walled_map();
        let window = IVec2::new(16, 8);
        let cross = |map: &TileMap| {
            let mut s = PlayerState::new(Vec2::new(14.5, 8.5));
            for t in 1..=200 {
                step_movement(map, &mut s, &PlayerInput { run: false, ..run_east() }, TICK_DT);
                if s.pos.x > 18.5 {
                    return Some(t);
                }
            }
            None
        };
        map.set_edge(window, Side::W, Edge::Window);
        assert_eq!(cross(&map), None, "walked through a shut window");
        let plain = TileMap::new(2, 1);
        let walk = cross(&plain).unwrap();
        for kind in [Edge::WindowOpen, Edge::WindowBroken] {
            map.set_edge(window, Side::W, kind);
            let climb = cross(&map).expect("climbs through");
            // About 0.7 m of opening at climb speed instead of walking pace.
            assert!(climb as f32 * TICK_DT > walk as f32 * TICK_DT + 0.6, "{kind:?}: climbed in {climb} ticks, walked in {walk}");
        }
        assert!(map.in_window_opening(Vec2::new(16.1, 8.5)) && !map.in_window_opening(Vec2::new(15.5, 8.5)));
        // Walking along the wall past the window isn't climbing.
        assert!(!map.in_window_opening(Vec2::new(16.0 - PLAYER_RADIUS - 0.08, 8.5)));
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
