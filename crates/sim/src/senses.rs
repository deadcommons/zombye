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
use crate::vision::line_of_sight;
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

const SUNRISE: f32 = 6.0;
const SUNSET: f32 = 20.0;

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

fn stance_mult(t: &ZombieTuning, stance: Stance) -> f32 {
    match stance {
        Stance::Sneak => t.sneak_sight_mult,
        Stance::Walk => 1.0,
        Stance::Run => t.run_sight_mult,
    }
}

/// How far away a zombie facing `facing` can spot `target` lying in unit direction `dir`
/// (before line of sight). Full range ahead, less at the sides, only `rear_range` behind.
pub fn sight_range(t: &ZombieTuning, cone: Cone, daylight: f32, target: &Target, facing: f32, dir: Vec2) -> f32 {
    let stance = stance_mult(t, target.stance) * target.sight_mult;
    let ahead = Vec2::from_angle(facing).dot(dir);
    if ahead < cone.peripheral {
        return t.rear_range * stance;
    }
    let motion = if target.moving { 1.0 } else { t.still_sight_mult };
    let range = (t.sight_night + (t.sight_day - t.sight_night) * daylight) * stance * motion;
    if ahead >= cone.full { range } else { range * t.peripheral_mult }
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
        let reach = sight_range(t, cone, daylight, &best, 0.0, Vec2::X).max(t.rear_range) * t.chase_sight_mult.max(1.0);
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
            let mut range = sight_range(t, cone, daylight, target, z.facing[i], dir);
            if after_this {
                // Once it has you, it keeps you in view a little farther and without luck.
                range *= t.chase_sight_mult;
            }
            if d > range {
                return;
            }
            if !after_this && hash01(seed, tick, z.id[i] as u64, target.id as u64) >= notice_chance(t, d / range) {
                return;
            }
            // Having just given up on something, it only notices someone close up.
            if !after_this && z.bored[i] > 0.0 && d > range * t.sure_sight_frac {
                return;
            }
            if !line_of_sight(map, z.pos[i], target.pos) {
                return;
            }
            if z.state[i].is_after(z.target_player[i]) && !after_this && z.sees[i] > 0 {
                let current = targets.iter().find(|p| p.id == z.target_player[i]);
                if current.is_some_and(|c| c.pos.distance(z.pos[i]) <= d) {
                    return;
                }
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
pub(crate) fn hearing_pass(z: &mut Zombies, hash: &SpatialHash, map: &TileMap, sounds: &[Sound], t: &ZombieTuning) {
    for s in sounds {
        if s.radius <= 0.0 {
            continue;
        }
        hash.for_each_within(s.pos, s.radius, |i| {
            let d = z.pos[i].distance(s.pos);
            if d > s.radius || s.source == SoundSource::Zombie(z.id[i]) {
                return;
            }
            // Only pay for the ray when the sound would be out of earshot muffled.
            if d > s.radius * t.wall_muffle && !line_of_sight(map, z.pos[i], s.pos) {
                return;
            }
            react(z, i, s, d, t);
        });
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
