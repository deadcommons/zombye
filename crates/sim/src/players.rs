//! Players: survivors driven by client input.
//!
//! Movement is one pure function, [`step_movement`], shared by the server tick and the client's
//! prediction of its own player. Both sides run it with the same map, input and [`TICK_DT`], so a
//! predicted position only diverges from the server's when the world changed under it (a door, later
//! a shove), and reconciliation fixes that. Climbing through an open or broken window is part of
//! movement (slow while inside the window opening), and depends only on the map, so it predicts
//! exactly too.
//!
//! [`TICK_DT`]: crate::TICK_DT

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::map::TileMap;

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

/// The part of a player that movement changes, and that clients predict.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    pub pos: Vec2,
    pub facing: f32,
    pub stance: Stance,
    /// 0..1. Running drains it.
    pub endurance: f32,
    /// Ran out of breath; can't run until endurance recovers to [`RUN_RECOVER_ENDURANCE`].
    pub winded: bool,
}

impl PlayerState {
    pub fn new(pos: Vec2) -> Self {
        Self { pos, facing: 0.0, stance: Stance::Walk, endurance: 1.0, winded: false }
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
    /// Dead players ignore input and aren't seen or heard. Single-player can respawn.
    pub dead: bool,
    /// Equipped weapon (index into `WorldConfig::weapons`); `None` = bare hands.
    pub weapon: Option<u16>,
    /// The swing, shove or stomp under way, until it lands.
    pub action: Option<Action>,
    /// Seconds until the next action can start.
    pub ready_in: f32,
    /// An action asked for just before the last one finished; starts as soon as it can.
    pub queued: Option<ActionKind>,
}

impl Player {
    pub fn new(id: u32, pos: Vec2, hp: f32) -> Self {
        Self {
            id,
            state: PlayerState::new(pos),
            input: PlayerInput::default(),
            last_input_seq: 0,
            hp,
            dead: false,
            weapon: None,
            action: None,
            ready_in: 0.0,
            queued: None,
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
    let can_run = !state.winded && state.endurance > RUN_MIN_ENDURANCE;
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
            state.endurance = (state.endurance - RUN_DRAIN * dt).max(0.0);
            if state.endurance <= RUN_MIN_ENDURANCE {
                state.winded = true;
            }
        }
        Stance::Sneak => state.endurance = (state.endurance + SNEAK_REGEN * dt).min(1.0),
        Stance::Walk => state.endurance = (state.endurance + REGEN * dt).min(1.0),
    }

    if !moving {
        return;
    }
    let mut speed = match state.stance {
        Stance::Run => RUN_SPEED,
        Stance::Walk => WALK_SPEED,
        Stance::Sneak => SNEAK_SPEED,
    };
    if map.in_window_opening(state.pos) {
        speed = speed.min(CLIMB_SPEED);
    }
    let step = input.move_dir * speed * dt;
    // Sub-step so fast movement (or a long dt) can't tunnel through thin walls.
    let n = (step.length() / MAX_SUBSTEP).ceil().max(1.0) as u32;
    let part = step / n as f32;
    let mut p = state.pos;
    for _ in 0..n {
        p = map.resolve_circle(p + part, PLAYER_RADIUS);
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

    #[test]
    fn bad_input_is_clamped() {
        let i = PlayerInput { move_dir: Vec2::new(f32::NAN, 1.0), facing: f32::INFINITY, ..Default::default() }.sanitized();
        assert_eq!(i.move_dir, Vec2::ZERO);
        assert_eq!(i.facing, 0.0);
    }
}
