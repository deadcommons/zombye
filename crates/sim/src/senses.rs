//! Zombie senses: sight (a view cone whose range depends on light, the player's stance and whether
//! they move, and that needs line of sight) and hearing (sound events with a radius, muffled
//! through walls).
//!
//! Senses only change what a zombie is after (its [`ZombieState`] and target); [`crate::ai`] turns
//! that into movement. Sight is checked for a quarter of the zombies each tick, and only for those
//! near a player (spatial hash radius query), so thousands of distant zombies cost nothing here.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::ai::ZombieTuning;
use crate::map::TileMap;
use crate::players::Stance;
use crate::rng::hash01;
use crate::spatial::SpatialHash;
use crate::vision::{line_of_sight, sight_through};
use crate::zombies::{Tier, ZombieState, Zombies};

/// Who or what made a sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoundSource {
    /// Not a player or zombie (debug noise, later alarms, gunshots on their own...).
    World,
    Player(u32),
    Zombie(u32),
}

/// A noise: zombies within `radius` of `pos` (less through walls) hear it and come to look at
/// `lure`, which is usually `pos` itself. A zombie battering a door sounds from the door but lures
/// the zombies it draws to the far side, so they try to get in too.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sound {
    pub pos: Vec2,
    pub radius: f32,
    pub source: SoundSource,
    pub lure: Vec2,
}

impl Sound {
    pub fn new(pos: Vec2, radius: f32, source: SoundSource) -> Self {
        Self { pos, radius, source, lure: pos }
    }
}

/// What the senses need to know about a player this tick.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub id: u32,
    pub pos: Vec2,
    pub stance: Stance,
    /// Moved this tick. Standing still is harder to spot.
    pub moving: bool,
    /// Dev toggle (`DevFlags::ignored`): never seen, and whoever is after them loses them. Still a
    /// body zombies walk round.
    pub ignored: bool,
    /// Multiplies every sight range against this player (Stealth, #46: 1 at level 0).
    pub sight_mult: f32,
}

pub const SUNRISE: f32 = 6.0;
pub const SUNSET: f32 = 20.0;

/// Is it dark at this hour of day (before sunrise or from sunset on)?
pub fn is_night(hour: f32) -> bool {
    !(SUNRISE..SUNSET).contains(&hour.rem_euclid(24.0))
}

/// 0 at night, 1 in full daylight, smooth through dawn and dusk (the same curve the client lights with).
pub fn daylight(hour: f32) -> f32 {
    if !(SUNRISE..SUNSET).contains(&hour) {
        return 0.0;
    }
    let elevation = ((hour - SUNRISE) / (SUNSET - SUNRISE) * std::f32::consts::PI).sin();
    let x = (elevation / 0.25).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The view cone, as cosines of its half-angles (precomputed once per pass).
#[derive(Clone, Copy, Debug)]
pub struct Cone {
    full: f32,
    peripheral: f32,
}

impl Cone {
    pub fn new(t: &ZombieTuning) -> Self {
        Self { full: t.sight_cone_deg.to_radians().cos(), peripheral: t.peripheral_deg.to_radians().cos() }
    }
}

impl Cone {
    /// From the half-angles in degrees: full range inside the first, less out to the second.
    pub fn from_degrees(full: f32, peripheral: f32) -> Self {
        Self { full: full.to_radians().cos(), peripheral: peripheral.to_radians().cos() }
    }
}

/// A pair of eyes: how far they see by day and by night, in which cone, and what the stance and
/// stillness of whoever they look at do to that. Zombies' come from [`ZombieTuning`]; survivors'
/// from [`crate::awareness::PlayerSightTuning`] (#66), so both see by one rule ([`eyes_range`]).
#[derive(Clone, Copy, Debug)]
pub struct Eyes {
    pub day: f32,
    pub night: f32,
    pub cone: Cone,
    pub peripheral_mult: f32,
    pub rear_range: f32,
    pub sneak_mult: f32,
    pub run_mult: f32,
    pub still_mult: f32,
}

impl Eyes {
    pub fn of_zombie(t: &ZombieTuning, cone: Cone) -> Self {
        Self { day: t.sight_day, night: t.sight_night, cone, peripheral_mult: t.peripheral_mult, rear_range: t.rear_range, sneak_mult: t.sneak_sight_mult, run_mult: t.run_sight_mult, still_mult: t.still_sight_mult }
    }
}

/// How far away eyes facing `facing` can spot `target` lying in unit direction `dir` (before line
/// of sight). Full range ahead, less at the sides, only `rear_range` behind.
pub fn eyes_range(e: &Eyes, daylight: f32, target: &Target, facing: f32, dir: Vec2) -> f32 {
    let stance = match target.stance {
        Stance::Sneak => e.sneak_mult,
        Stance::Walk => 1.0,
        Stance::Run => e.run_mult,
    } * target.sight_mult;
    let ahead = Vec2::from_angle(facing).dot(dir);
    if ahead < e.cone.peripheral {
        return e.rear_range * stance;
    }
    let motion = if target.moving { 1.0 } else { e.still_mult };
    let range = (e.night + (e.day - e.night) * daylight) * stance * motion;
    if ahead >= e.cone.full { range } else { range * e.peripheral_mult }
}

/// How far away a zombie facing `facing` can spot `target` lying in unit direction `dir`
/// (before line of sight). Full range ahead, less at the sides, only `rear_range` behind.
pub fn sight_range(t: &ZombieTuning, cone: Cone, daylight: f32, target: &Target, facing: f32, dir: Vec2) -> f32 {
    eyes_range(&Eyes::of_zombie(t, cone), daylight, target, facing, dir)
}

/// Chance per sight check of noticing someone at `frac` of the sight range: certain up close, then
/// falling off, so sneaking past at the edge of sight usually works.
pub fn notice_chance(t: &ZombieTuning, frac: f32) -> f32 {
    if frac <= t.sure_sight_frac {
        return 1.0;
    }
    let x = ((1.0 - frac) / (1.0 - t.sure_sight_frac)).clamp(0.0, 1.0);
    t.notice_chance * x * x
}

/// Sight checks for this tick. Zombie `i` looks when `(i + tick) % sight_interval_ticks == 0`.
/// A zombie that spots a player starts (or keeps) chasing them, and switches to another player
/// only if that one is nearer than the one it can still see. Returns the number of checks made.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sight_pass(z: &mut Zombies, hash: &SpatialHash, map: &TileMap, targets: &[Target], t: &ZombieTuning, daylight: f32, tick: u64, seed: u64) -> u32 {
    let interval = t.sight_interval_ticks.max(1);
    let cone = Cone::new(t);
    let mut checks = 0;
    for target in targets.iter().filter(|t| !t.ignored) {
        // The farthest this player could be seen from: dead ahead by a zombie already after them.
        let best = Target { moving: true, ..*target };
        let reach = sight_range(t, cone, daylight, &best, 0.0, Vec2::X).max(t.rear_range) * t.chase_sight_mult.max(1.0) * t.max_alertness().max(1.0);
        hash.for_each_within(target.pos, reach, |i| {
            // Staggered, down or dead (killed this tick, so still in the hash): not looking. Only
            // active zombies look at all.
            if !(i as u64 + tick).is_multiple_of(interval as u64) || z.state[i].is_reeling() || z.state[i] == ZombieState::Dead || z.tier[i] != Tier::Active {
                return;
            }
            let to = target.pos - z.pos[i];
            let d = to.length();
            if d > reach {
                return;
            }
            checks += 1;
            let thumping = z.state[i] == ZombieState::Thump;
            let after_this = z.state[i].is_after(z.target_player[i]) && z.target_player[i] == target.id;
            let dir = if d > 1e-4 { to / d } else { Vec2::from_angle(z.facing[i]) };
            let alert = t.alertness_of(seed, z.id[i]);
            let mut range = sight_range(t, cone, daylight, target, z.facing[i], dir) * alert;
            if after_this {
                // Once it has you, it keeps you in view a little farther and without luck.
                range *= t.chase_sight_mult;
            }
            if d > range {
                return;
            }
            let roll = hash01(seed, tick, z.id[i] as u64, target.id as u64);
            if !after_this && roll >= (notice_chance(t, d / range) * alert).min(1.0) {
                return;
            }
            // Having just given up on something, it only notices someone close up.
            if !after_this && z.bored[i] > 0.0 && d > range * t.sure_sight_frac {
                return;
            }
            let Some(through_window) = sight_through(map, z.pos[i], target.pos) else {
                return;
            };
            if through_window {
                // Through a window pane or frame it only makes out someone close by.
                range *= t.window_sight_mult;
                if d > range || (!after_this && roll >= (notice_chance(t, d / range) * alert).min(1.0)) {
                    return;
                }
            }
            if z.state[i].is_after(z.target_player[i]) && !after_this && z.sees[i] > 0 {
                let current = targets.iter().find(|p| p.id == z.target_player[i]);
                if current.is_some_and(|c| c.pos.distance(z.pos[i]) <= d) {
                    return;
                }
            }
            if z.state[i] == ZombieState::Wander && d > range * t.sure_sight_frac {
                // A wanderer that catches sight of someone some way off only glimpses them: it
                // turns and shuffles over for a look, and gives chase if it notices them again.
                glimpse(z, i, target.pos, d, t);
                return;
            }
            if thumping {
                // Battering its way in: now it's after this player (the field decides whether
                // that's still through this door).
            } else if !z.state[i].is_hunting() {
                // Just noticed: groan almost at once, which draws the neighbours.
                z.groan_timer[i] = t.alert_groan_secs * (0.5 + hash01(seed, tick, z.id[i] as u64, 0x67));
                z.state[i] = ZombieState::Chase;
            } else if !after_this {
                z.state[i] = ZombieState::Chase;
            }
            z.target_player[i] = target.id;
            z.target_pos[i] = target.pos;
            if z.state[i] == ZombieState::Chase {
                z.timer[i] = t.memory_secs;
            } else if thumping {
                z.timer[i] = z.timer[i].max(t.thump_interest_secs);
            }
            z.bored[i] = 0.0;
            z.sees[i] = interval.saturating_add(1);
        });
    }
    checks
}

/// Every zombie in the spatial hash (active and near) in earshot of each sound reacts: one that isn't hunting goes to investigate the
/// sound's position; one hunting a player it can't see updates where it thinks they are if the
/// sound came from them. Sounds through walls only carry `wall_muffle` of their radius.
///
/// The sounds are heard in order, and what a zombie does about one it may undo for the next. Most
/// listeners are idle (wandering or investigating, not bored), and for those each sound heard
/// replaces all the last one did, so only the last one they hear counts: they are done first,
/// going through the sounds backward and stopping at the first each hears, which is one ray
/// instead of one per sound in earshot when many loud noises fall on one tick (#70). The rest
/// (hunting, battering, bored) then hear every sound in order as before. The outcome is exactly
/// that of every zombie hearing every sound in order. `marks` is scratch, kept between calls.
/// Each zombie hears a sound out to its radius times the zombie's alertness.
#[allow(clippy::too_many_arguments)]
pub(crate) fn hearing_pass(z: &mut Zombies, hash: &SpatialHash, map: &TileMap, sounds: &[Sound], t: &ZombieTuning, seed: u64, marks: &mut Vec<u8>) {
    /// Not met yet this pass; idle, nothing heard yet; idle, done; one of the rest.
    const NEW: u8 = 0;
    const IDLE: u8 = 1;
    const DONE: u8 = 2;
    const REST: u8 = 3;
    let reach_mult = t.max_alertness();
    let heard = |z: &Zombies, i: usize, s: &Sound| {
        let d = z.pos[i].distance(s.pos);
        if d > s.radius * reach_mult || s.source == SoundSource::Zombie(z.id[i]) || !would_react(z, i, s) {
            return None;
        }
        let radius = s.radius * t.alertness_of(seed, z.id[i]);
        // Only pay for the ray when the sound would be out of earshot muffled.
        (d <= radius && (d <= radius * t.wall_muffle || line_of_sight(map, z.pos[i], s.pos))).then_some(d)
    };
    if sounds.iter().filter(|s| s.radius > 0.0).count() < 2 {
        // One sound: nothing to save.
        for s in sounds.iter().filter(|s| s.radius > 0.0) {
            hash.for_each_within(s.pos, s.radius * reach_mult, |i| {
                if let Some(d) = heard(z, i, s) {
                    react(z, i, s, d, t);
                }
            });
        }
        return;
    }
    marks.clear();
    marks.resize(z.len(), NEW);
    let mut rest = false;
    for s in sounds.iter().rev().filter(|s| s.radius > 0.0) {
        hash.for_each_within(s.pos, s.radius * reach_mult, |i| {
            if marks[i] == NEW {
                let idle = matches!(z.state[i], ZombieState::Wander | ZombieState::Investigate) && z.bored[i] <= 0.0;
                marks[i] = if idle { IDLE } else { REST };
                rest |= !idle;
            }
            if marks[i] == IDLE
                && let Some(d) = heard(z, i, s)
            {
                react(z, i, s, d, t);
                marks[i] = DONE;
            }
        });
    }
    if !rest {
        return;
    }
    for s in sounds.iter().filter(|s| s.radius > 0.0) {
        hash.for_each_within(s.pos, s.radius * reach_mult, |i| {
            if marks[i] == REST
                && let Some(d) = heard(z, i, s)
            {
                react(z, i, s, d, t);
            }
        });
    }
}

/// Zombie `i`, wandering, glimpsed someone at `pos`, `d` metres off: it goes to look, as if it had
/// heard something there (no groan, so the neighbours aren't drawn in).
fn glimpse(z: &mut Zombies, i: usize, pos: Vec2, d: f32, t: &ZombieTuning) {
    z.state[i] = ZombieState::Investigate;
    z.target_player[i] = 0;
    z.target_pos[i] = pos;
    let travel = d / (z.speed[i] * t.investigate_speed_mult).max(0.1);
    z.timer[i] = t.investigate_secs + (travel * 1.5).min(t.max_travel_secs);
}

/// Would [`react`] change anything about zombie `i` if it heard `s`? Asked before the ray to the
/// sound is cast: a loud noise reaches thousands of zombies, and most of a horde already hunting
/// someone has no use for it (#70). Must say yes whenever `react` would do anything.
pub(crate) fn would_react(z: &Zombies, i: usize, s: &Sound) -> bool {
    let by_player = matches!(s.source, SoundSource::Player(_));
    match z.state[i] {
        ZombieState::Chase | ZombieState::Lunge | ZombieState::Attack => z.sees[i] == 0 && s.source == SoundSource::Player(z.target_player[i]),
        ZombieState::Thump => by_player,
        ZombieState::Wander | ZombieState::Investigate => by_player || z.bored[i] <= 0.0,
        ZombieState::Staggered | ZombieState::Down | ZombieState::Dead => false,
    }
}

/// Zombie `i`, `d` metres from sound `s`, heard it: what it does about it (the same on every tier).
pub(crate) fn react(z: &mut Zombies, i: usize, s: &Sound, d: f32, t: &ZombieTuning) {
    match z.state[i] {
        ZombieState::Chase | ZombieState::Lunge | ZombieState::Attack => {
            if z.sees[i] == 0 && s.source == SoundSource::Player(z.target_player[i]) {
                z.target_pos[i] = s.lure;
                z.timer[i] = z.timer[i].max(t.memory_secs);
            }
        }
        ZombieState::Thump => {
            // Busy battering: only a player's noise changes where it thinks they are, and
            // any player's noise keeps it at it.
            if let SoundSource::Player(id) = s.source {
                if z.target_player[i] == 0 || (z.target_player[i] == id && z.sees[i] == 0) {
                    z.target_pos[i] = s.lure;
                }
                z.timer[i] = z.timer[i].max(t.thump_interest_secs);
            }
        }
        ZombieState::Wander | ZombieState::Investigate if z.bored[i] > 0.0 && !matches!(s.source, SoundSource::Player(_)) => {
            // Lost interest lately: other zombies' racket doesn't draw it back yet.
        }
        ZombieState::Wander | ZombieState::Investigate => {
            // A player's noise always draws it, bored or not.
            z.bored[i] = 0.0;
            z.state[i] = ZombieState::Investigate;
            z.target_player[i] = 0;
            z.target_pos[i] = s.lure;
            // Time to walk there (roughly) plus time to look around.
            let travel = d / (z.speed[i] * t.investigate_speed_mult).max(0.1);
            z.timer[i] = t.investigate_secs + (travel * 1.5).min(t.max_travel_secs);
        }
        ZombieState::Staggered | ZombieState::Down | ZombieState::Dead => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daylight_curve() {
        assert_eq!(daylight(2.0), 0.0);
        assert_eq!(daylight(23.0), 0.0);
        assert_eq!(daylight(13.0), 1.0);
        let dawn = daylight(6.5);
        assert!(dawn > 0.0 && dawn < 1.0, "{dawn}");
    }

    /// The hearing pass (#70) against the plain rule it stands for: every zombie hears every
    /// sound in earshot in order, each reaction on top of the last. A crowd in every state
    /// among walls, many overlapping sounds of every source: the columns come out the same.
    #[test]
    fn the_hearing_pass_is_every_zombie_hearing_every_sound_in_order() {
        use crate::map::{Edge, Side};
        use crate::rng::Rng;
        use glam::IVec2;
        let mut map = TileMap::new(6, 6);
        for k in 0..96 {
            if k % 9 != 4 {
                map.set_edge(IVec2::new(30, k), Side::W, Edge::Wall);
                map.set_edge(IVec2::new(k, 50), Side::N, Edge::Wall);
                map.set_edge(IVec2::new(k, 70), Side::N, if k % 2 == 0 { Edge::DoorClosed } else { Edge::Window });
            }
        }
        let t = ZombieTuning::default();
        for round in 0..6u64 {
            let mut rng = Rng::new(0x70 + round);
            let mut z = Zombies::default();
            let states = [
                ZombieState::Wander,
                ZombieState::Wander,
                ZombieState::Investigate,
                ZombieState::Chase,
                ZombieState::Lunge,
                ZombieState::Attack,
                ZombieState::Thump,
                ZombieState::Staggered,
                ZombieState::Down,
                ZombieState::Dead,
            ];
            for id in 1..=3000u32 {
                z.push(id, Vec2::new(rng.range(1.0, 95.0), rng.range(1.0, 95.0)), 0.0, rng.range(0.6, 1.6), 10.0);
                let i = z.len() - 1;
                z.state[i] = states[rng.below(states.len() as u32) as usize];
                z.target_player[i] = if z.state[i].is_hunting() || rng.f32() < 0.3 { 1 + rng.below(3) } else { 0 };
                z.sees[i] = if rng.f32() < 0.4 { 3 } else { 0 };
                z.bored[i] = if rng.f32() < 0.35 { rng.range(0.1, 20.0) } else { 0.0 };
                z.timer[i] = rng.range(0.0, 12.0);
            }
            let mut hash = SpatialHash::new(96.0, 96.0, 1.0);
            let states = &z.state;
            hash.rebuild_where(&z.pos, |i| states[i] != ZombieState::Dead);
            // None, one, and many sounds; some silent ones among them.
            let count = [0, 1, 2, 12, 60, 200][round as usize];
            let sounds: Vec<Sound> = (0..count)
                .map(|k| {
                    let pos = Vec2::new(rng.range(1.0, 95.0), rng.range(1.0, 95.0));
                    let source = match rng.below(4) {
                        0 => SoundSource::Player(1 + rng.below(3)),
                        1 | 2 => SoundSource::Zombie(1 + rng.below(3000)),
                        _ => SoundSource::World,
                    };
                    let radius = if k % 17 == 5 { 0.0 } else { rng.range(3.0, 45.0) };
                    let mut s = Sound::new(pos, radius, source);
                    s.lure = pos + Vec2::new(rng.range(-2.0, 2.0), rng.range(-2.0, 2.0));
                    s
                })
                .collect();
            let mut plain = z.clone();
            for s in sounds.iter().filter(|s| s.radius > 0.0) {
                for i in 0..plain.len() {
                    let d = plain.pos[i].distance(s.pos);
                    let radius = s.radius * t.alertness_of(7, plain.id[i]);
                    if plain.state[i] == ZombieState::Dead || d > radius || s.source == SoundSource::Zombie(plain.id[i]) {
                        continue;
                    }
                    if d > radius * t.wall_muffle && !line_of_sight(&map, plain.pos[i], s.pos) {
                        continue;
                    }
                    react(&mut plain, i, s, d, &t);
                }
            }
            hearing_pass(&mut z, &hash, &map, &sounds, &t, 7, &mut Vec::new());
            let changed = (0..z.len()).filter(|&i| z.state[i] == ZombieState::Investigate && z.target_pos[i] != z.pos[i]).count();
            assert!(count < 12 || changed > 500, "round {round}: the sounds reach the crowd ({changed} investigating)");
            assert_eq!(z.state, plain.state, "round {round}");
            assert_eq!(z.target_pos, plain.target_pos, "round {round}");
            assert_eq!(z.target_player, plain.target_player, "round {round}");
            assert_eq!(z.timer, plain.timer, "round {round}");
            assert_eq!(z.bored, plain.bored, "round {round}");
        }
    }

    #[test]
    fn sight_range_depends_on_cone_light_and_stance() {
        let t = ZombieTuning::default();
        let cone = Cone::new(&t);
        let walker = Target { id: 1, pos: Vec2::ZERO, stance: Stance::Walk, moving: true, ignored: false, sight_mult: 1.0 };
        let ahead = sight_range(&t, cone, 1.0, &walker, 0.0, Vec2::X);
        let side = sight_range(&t, cone, 1.0, &walker, 0.0, Vec2::Y);
        let behind = sight_range(&t, cone, 1.0, &walker, 0.0, -Vec2::X);
        assert!(ahead > side && side > behind, "{ahead} {side} {behind}");
        assert!(behind <= 2.5);
        let night = sight_range(&t, cone, 0.0, &walker, 0.0, Vec2::X);
        assert!(night < ahead * 0.5, "night {night} vs day {ahead}");
        let sneak = sight_range(&t, cone, 1.0, &Target { stance: Stance::Sneak, ..walker }, 0.0, Vec2::X);
        let run = sight_range(&t, cone, 1.0, &Target { stance: Stance::Run, ..walker }, 0.0, Vec2::X);
        let still = sight_range(&t, cone, 1.0, &Target { moving: false, ..walker }, 0.0, Vec2::X);
        assert!(sneak < still && still < ahead && ahead < run, "{sneak} {still} {ahead} {run}");
        // Stealth (#46) shortens every range by its multiplier.
        let stealthy = sight_range(&t, cone, 1.0, &Target { sight_mult: 0.88, ..walker }, 0.0, Vec2::X);
        assert!((stealthy - ahead * 0.88).abs() < 1e-4, "{stealthy} {ahead}");
        assert_eq!(notice_chance(&t, 0.1), 1.0);
        assert!(notice_chance(&t, 0.9) < 0.05);
    }
}
