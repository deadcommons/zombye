//! Zombie behaviour: a small state machine per zombie plus steering, crowd separation and movement.
//!
//! `Wander` → (hears something) `Investigate` → (sees a player) `Chase` → (close, in sight) `Lunge`
//! → (in reach) `Attack`. A chaser that loses sight heads for where it last saw its player and
//! searches there until its memory runs out, then investigates that spot, then wanders. Chasers
//! groan now and then, which pulls nearby zombies in to investigate, so hordes snowball.
//!
//! Zombie flow fields lead through shut doors and windows (see [`crate::flow`]). When the next
//! step of a chaser's or investigator's path goes through one within reach, it switches to `Thump`
//! and batters it on a cooldown ([`ZombieThump`] events; the world applies the damage and breaks
//! the edge). Every thump is a noise on the far side, so more zombies come and join in. Zombies
//! never open doors. Open and broken windows are climbed through slowly (`climb_speed`).
//!
//! [`crate::senses`] decides what a zombie noticed; this module decides what it does about it.
//! Every number is in [`ZombieTuning`].

use std::f32::consts::TAU;

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::TICK_DT;
use crate::combat::MAX_KNOCKBACK_SPEED;
use crate::flow::{FlowCache, FlowField, FlowKey};
use crate::map::{EdgeRef, TileMap};
use crate::players::PLAYER_RADIUS;
use crate::rng::Rng;
use crate::senses::{Sound, SoundSource, Target};
use crate::spatial::SpatialHash;
use crate::vision::{clear_path, line_of_sight};
use crate::zombies::{GET_UP_SECS, Tier, ZombieAnim, ZombieState, Zombies};

/// Zombie body radius in metres.
pub const ZOMBIE_RADIUS: f32 = 0.3;
/// With the debug goal set, zombies within this path distance of it head there.
pub const GOAL_RANGE: f32 = 120.0;
/// Most crowd separation moves a zombie in one tick. With the knockback cap this keeps a step
/// under a body radius plus half a wall, so nothing tunnels through a wall.
const MAX_CROWD_PUSH: f32 = 0.12;
/// Each zombie rechecks whether it can walk straight at its target once every this many ticks.
const DIRECT_CHECK_TICKS: u64 = 8;
/// Targets farther than this always go by flow field (a long straight line is rarely clear and
/// costs a long ray).
const DIRECT_MAX: f32 = 24.0;
/// A player this close (straight line) whose field has no way to the zombie can't be reached
/// (any real path would be well inside the field's range).
const UNREACHABLE_NEAR: f32 = 16.0;

/// Zombie senses and behaviour numbers. Part of `WorldConfig`; serde with defaults for every field,
/// so a data file or sandbox options can override just a few. Distances in metres, times in seconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZombieTuning {
    // Sight.
    /// Range dead ahead at a walking, moving player in full daylight / at night.
    pub sight_day: f32,
    pub sight_night: f32,
    /// Half-angle of the full-range cone, and of the wider peripheral cone (degrees).
    pub sight_cone_deg: f32,
    pub peripheral_deg: f32,
    /// Range multiplier in the peripheral band.
    pub peripheral_mult: f32,
    /// Range behind the zombie (outside the peripheral cone), before the stance multiplier.
    pub rear_range: f32,
    /// Range multipliers for a sneaking / running player, and for one standing still.
    pub sneak_sight_mult: f32,
    pub run_sight_mult: f32,
    pub still_sight_mult: f32,
    /// Within this fraction of the range a check always notices; beyond it the chance per check
    /// falls from `notice_chance` to 0 at full range.
    pub sure_sight_frac: f32,
    pub notice_chance: f32,
    /// Each zombie looks once every this many ticks.
    pub sight_interval_ticks: u8,
    /// A chaser keeps its own player in view out to this multiple of the range, without luck.
    pub chase_sight_mult: f32,

    // Hearing.
    /// Footstep noise radius per tick while moving, by stance.
    pub sneak_noise: f32,
    pub walk_noise: f32,
    pub run_noise: f32,
    /// A door opening or closing.
    pub door_noise: f32,
    /// A hunting zombie's groan, and the seconds between groans.
    pub groan_noise: f32,
    pub groan_min_secs: f32,
    pub groan_max_secs: f32,
    /// About how soon a zombie groans after first noticing someone.
    pub alert_groan_secs: f32,
    /// Fraction of a sound's radius that carries through walls (no line of sight).
    pub wall_muffle: f32,

    // Memory and movement.
    /// How long a chaser keeps after a player it can't see.
    pub memory_secs: f32,
    /// How long a zombie looks around a sound (or a lost player's last spot), on top of the time
    /// it takes to walk to a sound (capped at `max_travel_secs`).
    pub investigate_secs: f32,
    pub max_travel_secs: f32,
    /// Speed multiplier while investigating (of the zombie's own top speed).
    pub investigate_speed_mult: f32,
    /// Idle shuffle speed.
    pub wander_speed: f32,
    /// Milling about around a point it reached: how far it strays.
    pub search_radius: f32,

    // Lunge and attack.
    /// A chaser this close (centre to centre) with line of sight lunges.
    pub lunge_range: f32,
    pub lunge_secs: f32,
    /// Lunge speed: the zombie's speed times this, but at least `lunge_min_speed`.
    pub lunge_speed_mult: f32,
    pub lunge_min_speed: f32,
    pub lunge_cooldown: f32,
    /// Strikes land within this distance (centre to centre).
    pub attack_reach: f32,
    /// Seconds from reaching a player to the first strike, and between strikes.
    pub attack_windup: f32,
    pub attack_cooldown: f32,
    /// It gives up attacking and chases again once the player is this much beyond reach.
    pub attack_leave: f32,

    // Doors and windows.
    /// Hit points of a shut door and of an intact window.
    pub door_hp: f32,
    pub window_hp: f32,
    /// Damage per thump, and seconds between one zombie's thumps (each jittered by ±20%, so a crowd
    /// doesn't thump in step).
    pub thump_damage: f32,
    pub thump_cooldown: f32,
    /// A zombie this close to a door or window (centre to edge) can batter it.
    pub thump_reach: f32,
    /// Noise radius of one thump, of glass breaking (battered or smashed by a player) and of a door
    /// giving way.
    pub thump_noise: f32,
    pub glass_break_noise: f32,
    pub door_break_noise: f32,
    /// A player opening or closing a window (quieter than a door).
    pub window_noise: f32,
    /// Top speed while climbing through an open or broken window.
    pub climb_speed: f32,
    /// A thumper keeps at it this long after it last saw or heard a player (any player's noise
    /// refreshes it, so someone moving about inside keeps the crowd battering), then gives up.
    pub thump_interest_secs: f32,
    /// Crowd pressure: a zombie this close to the door or window (centre to edge) behind one
    /// battering it pushes in too, and each of its thumps does `crowd_thump_frac` of a thump.
    pub crowd_reach: f32,
    pub crowd_thump_frac: f32,

    // Giving up.
    /// A chaser or investigator held back by walls (not by the crowd) that hasn't moved
    /// `stuck_move` metres for `give_up_secs` gives up, as does one whose target has no way to it
    /// at all (not even through a door or window).
    pub give_up_secs: f32,
    pub stuck_move: f32,
    /// After giving up (stuck, or a thumper that lost interest) it mills about where it is, then
    /// wanders. For this long it ignores other zombies' noise and noise that isn't a player's, and
    /// only notices a player close up (within `sure_sight_frac` of its sight range).
    pub bored_secs: f32,

    // Corpses.
    /// Corpses kept at most (the oldest go first), and how long one lies before it's removed.
    pub max_corpses: u32,
    pub corpse_secs: f32,
}

impl Default for ZombieTuning {
    fn default() -> Self {
        Self {
            sight_day: 30.0,
            sight_night: 8.0,
            sight_cone_deg: 55.0,
            peripheral_deg: 110.0,
            peripheral_mult: 0.45,
            rear_range: 2.0,
            sneak_sight_mult: 0.4,
            run_sight_mult: 1.35,
            still_sight_mult: 0.7,
            sure_sight_frac: 0.4,
            notice_chance: 0.45,
            sight_interval_ticks: 4,
            chase_sight_mult: 1.25,

            sneak_noise: 1.2,
            walk_noise: 6.0,
            run_noise: 14.0,
            door_noise: 10.0,
            groan_noise: 9.0,
            groan_min_secs: 5.0,
            groan_max_secs: 12.0,
            alert_groan_secs: 0.8,
            wall_muffle: 0.4,

            memory_secs: 25.0,
            investigate_secs: 15.0,
            max_travel_secs: 120.0,
            investigate_speed_mult: 0.85,
            wander_speed: 0.4,
            search_radius: 3.5,

            lunge_range: 2.4,
            lunge_secs: 0.45,
            lunge_speed_mult: 2.2,
            lunge_min_speed: 2.6,
            lunge_cooldown: 2.0,
            attack_reach: 0.9,
            attack_windup: 0.35,
            attack_cooldown: 1.3,
            attack_leave: 0.5,

            door_hp: 40.0,
            window_hp: 6.0,
            thump_damage: 1.0,
            thump_cooldown: 1.6,
            thump_reach: 0.7,
            thump_noise: 15.0,
            glass_break_noise: 22.0,
            door_break_noise: 26.0,
            window_noise: 2.5,
            climb_speed: 0.35,
            thump_interest_secs: 20.0,
            crowd_reach: 1.6,
            crowd_thump_frac: 0.3,

            give_up_secs: 12.0,
            stuck_move: 1.0,
            bored_secs: 20.0,

            max_corpses: 300,
            corpse_secs: 900.0,
        }
    }
}

/// A zombie struck at a player this tick (by zombie and player id). The world then rolls whether
/// it `landed` and for how much `damage` (see [`crate::combat`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZombieAttack {
    pub zombie: u32,
    pub player: u32,
    pub landed: bool,
    pub damage: f32,
}

/// A zombie battered a door or window this tick. The world applies these in order after the
/// zombie pass (so several zombies on one edge stack their damage).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZombieThump {
    pub zombie: u32,
    pub edge: EdgeRef,
    /// Where the zombie stood (which side it's battering from).
    pub from: Vec2,
    /// Hit points it knocks off: `thump_damage`, or `crowd_thump_frac` of that from the crowd
    /// pushing in behind the front rank.
    pub damage: f32,
}

/// Counts from one tick of [`step_zombies`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Counts {
    pub hunting: u32,
    pub investigating: u32,
    pub thumping: u32,
    /// Staggered or down.
    pub reeling: u32,
}

/// Everything the zombie tick reads or writes besides the zombies themselves.
pub(crate) struct Ctx<'a> {
    pub map: &'a TileMap,
    pub hash: &'a SpatialHash,
    pub targets: &'a [Target],
    pub tuning: &'a ZombieTuning,
    pub flow: &'a mut FlowCache,
    /// The debug goal's field, when `set_goal` is active (senses are off then).
    pub goal: Option<&'a FlowField>,
    pub rng: &'a mut Rng,
    /// Groans go here, heard next tick.
    pub sounds: &'a mut Vec<Sound>,
    pub attacks: &'a mut Vec<ZombieAttack>,
    pub thumps: &'a mut Vec<ZombieThump>,
    pub tick: u64,
}

/// Run every active zombie's state machine and move it (and age every corpse, whatever its tier).
/// New positions go to `next_pos` (same order); zombies on the other tiers are left alone here
/// (see [`crate::tiers`]).
// Indexing by `i` across the struct-of-arrays columns is the point.
#[allow(clippy::needless_range_loop)]
pub(crate) fn step_zombies(z: &mut Zombies, next_pos: &mut [Vec2], cx: &mut Ctx) -> Counts {
    let t = cx.tuning;
    let mut counts = Counts::default();
    for i in 0..z.len() {
        let p = z.pos[i];
        if z.state[i] == ZombieState::Dead {
            // How long it has lain here (old ones are cleared away by the world).
            z.timer[i] += TICK_DT;
            // A corpse only slides to a stop if it was knocked back as it died.
            next_pos[i] = if z.vel[i].length_squared() > 1e-4 {
                z.vel[i] *= 0.75;
                cx.map.resolve_circle(p + z.vel[i] * TICK_DT, ZOMBIE_RADIUS)
            } else {
                p
            };
            continue;
        }
        if z.tier[i] != Tier::Active {
            continue;
        }
        z.sees[i] = z.sees[i].saturating_sub(1);
        z.cooldown[i] = (z.cooldown[i] - TICK_DT).max(0.0);
        z.bored[i] = (z.bored[i] - TICK_DT).max(0.0);
        let mut desired = Vec2::ZERO;
        let mut attacking = false;
        // Heading somewhere with purpose (so walls holding it back count toward giving up).
        let mut pressing = false;
        // Where to look instead of where it walks (an attacker faces its victim).
        let mut look: Option<Vec2> = None;

        if z.state[i].is_reeling() {
            // Staggered or down: nothing but waiting it out (a knockback still slides it).
            z.timer[i] -= TICK_DT;
            if z.timer[i] <= 0.0 {
                recover(z, i, t);
            }
        } else if let Some(goal) = cx.goal {
            // Debug override: everything in range heads for the goal, senses off.
            match goal.distance(p) {
                Some(d) if d <= GOAL_RANGE => {
                    z.state[i] = ZombieState::Investigate;
                    z.target_player[i] = 0;
                    z.target_pos[i] = goal.goals[0].as_vec2() + Vec2::splat(0.5);
                    z.timer[i] = t.investigate_secs;
                    desired = goal.direction(p) * z.speed[i];
                }
                _ => desired = wander(z, i, cx.rng, t, TICK_DT),
            }
        } else {
            match z.state[i] {
                ZombieState::Wander => desired = wander(z, i, cx.rng, t, TICK_DT),
                ZombieState::Investigate => {
                    z.timer[i] -= TICK_DT;
                    if z.timer[i] <= 0.0 {
                        z.state[i] = ZombieState::Wander;
                        z.wander_timer[i] = 0.0;
                    } else {
                        let target = z.target_pos[i];
                        let speed = z.speed[i] * t.investigate_speed_mult;
                        if z.bored[i] > 0.0 {
                            // Given up: only looking about where it stopped.
                            desired = mill(z, i, cx.rng, t, target, TICK_DT);
                        } else {
                        match steer(cx, z, i, FlowKey::point(target), target) {
                            Steer::Go(dir) => (desired, pressing) = (dir * speed, true),
                            Steer::Arrived => desired = mill(z, i, cx.rng, t, target, TICK_DT),
                            Steer::Breach(edge, d) if can_batter(z, i, cx, edge, d) => (attacking, look) = start_thump(z, i, cx, edge),
                            Steer::Breach(edge, _) => (desired, pressing) = (approach(z, i, edge) * speed, true),
                            Steer::Unreachable => lose_interest(z, i, t),
                        }
                        }
                    }
                }
                ZombieState::Chase | ZombieState::Lunge | ZombieState::Attack => {
                    let (d, a, l, pr) = hunt(z, i, cx);
                    (desired, attacking, look, pressing) = (d, a, l, pr);
                }
                ZombieState::Thump => {
                    let (d, a, l) = thump(z, i, cx);
                    (desired, attacking, look) = (d, a, l);
                }
                ZombieState::Staggered | ZombieState::Down | ZombieState::Dead => {}
            }
        }

        let state = z.state[i];
        if state.is_hunting() {
            counts.hunting += 1;
            z.groan_timer[i] -= TICK_DT;
            if z.groan_timer[i] <= 0.0 {
                z.groan_timer[i] = cx.rng.range(t.groan_min_secs, t.groan_max_secs);
                cx.sounds.push(Sound::new(p, t.groan_noise, SoundSource::Zombie(z.id[i])));
            }
        } else if state == ZombieState::Investigate {
            counts.investigating += 1;
        } else if state == ZombieState::Thump {
            counts.thumping += 1;
        } else if state.is_reeling() {
            counts.reeling += 1;
        }

        // Separation: zombies shove each other apart so crowds pile up instead of overlapping. One
        // that's down is underfoot: it neither pushes nor gets pushed (corpses aren't in the hash,
        // except ones killed this tick).
        let mut push = Vec2::ZERO;
        let min_d = ZOMBIE_RADIUS * 2.0;
        let positions = &z.pos;
        let states = &z.state;
        let down = state == ZombieState::Down;
        cx.hash.for_each_near(p, |j| {
            if down || j == i || matches!(states[j], ZombieState::Down | ZombieState::Dead) {
                return;
            }
            let d = p - positions[j];
            let d2 = d.length_squared();
            if d2 < min_d * min_d {
                if d2 > 1e-8 {
                    let len = d2.sqrt();
                    push += d / len * (min_d - len);
                } else {
                    // Exactly overlapping: split deterministically by index.
                    push += if i < j { Vec2::X } else { -Vec2::X } * 0.05;
                }
            }
        });
        // Zombies don't walk through players either (only the zombie gives way, so player
        // movement stays exactly predictable on the client).
        let body = ZOMBIE_RADIUS + PLAYER_RADIUS;
        for target in cx.targets {
            let d = p - target.pos;
            let d2 = d.length_squared();
            if !down && d2 < body * body && d2 > 1e-8 {
                let len = d2.sqrt();
                push += d / len * (body - len) * 2.0;
            }
        }

        // Capped so a knockback can't carry it through a wall in one step.
        let mut vel = z.vel[i].lerp(desired, 0.25).clamp_length_max(MAX_KNOCKBACK_SPEED);
        if cx.map.in_window_opening(p) {
            // Clambering through a window frame.
            vel = vel.clamp_length_max(t.climb_speed);
        }
        // The crowd's shove is capped too: a pile of zombies on one spot (say, near-tier ones
        // that queued up against a wall and were just promoted) spreads out over a few ticks
        // instead of flinging one through the wall.
        let new_p = p + vel * TICK_DT + (push * 0.5).clamp_length_max(MAX_CROWD_PUSH);
        let resolved = cx.map.resolve_circle(new_p, ZOMBIE_RADIUS);
        next_pos[i] = resolved;
        z.vel[i] = vel;
        // Giving up: pushing against a wall (not just stuck in a crowd) without getting anywhere.
        if !pressing || resolved.distance(z.anchor[i]) >= t.stuck_move {
            z.anchor[i] = resolved;
            z.stuck[i] = 0.0;
        } else if resolved.distance_squared(new_p) > 1e-8 {
            z.stuck[i] += TICK_DT;
            if z.stuck[i] >= t.give_up_secs {
                lose_interest(z, i, t);
            }
        }
        z.anim[i] = if state == ZombieState::Staggered {
            ZombieAnim::Stagger
        } else if down {
            if z.timer[i] <= GET_UP_SECS { ZombieAnim::GetUp } else { ZombieAnim::Down }
        } else if attacking {
            if state == ZombieState::Thump { ZombieAnim::Thump } else { ZombieAnim::Attack }
        } else if desired.length_squared() > 0.0025 {
            ZombieAnim::Walk
        } else {
            ZombieAnim::Idle
        };

        if let Some(d) = look.filter(|d| d.length_squared() > 1e-6) {
            z.facing[i] = turn_toward(z.facing[i], d.y.atan2(d.x), 6.0 * TICK_DT);
        } else if vel.length_squared() > 0.01 && !state.is_reeling() {
            z.facing[i] = turn_toward(z.facing[i], vel.y.atan2(vel.x), 6.0 * TICK_DT);
        }
    }
    counts
}

/// Chase, Lunge and Attack. Returns the desired velocity, whether it's attacking, where to look,
/// and whether it's heading somewhere with purpose (see the `pressing` check in [`step_zombies`]).
fn hunt(z: &mut Zombies, i: usize, cx: &mut Ctx) -> (Vec2, bool, Option<Vec2>, bool) {
    let t = cx.tuning;
    let p = z.pos[i];
    let Some(target) = cx.targets.iter().find(|tg| tg.id == z.target_player[i]).copied() else {
        // Its player is gone: go and look where they were.
        start_investigating(z, i, t);
        return (Vec2::ZERO, false, None, false);
    };
    let to = target.pos - p;
    let d = to.length();
    let dir = if d > 1e-4 { to / d } else { Vec2::ZERO };
    match z.state[i] {
        ZombieState::Lunge => {
            z.timer[i] -= TICK_DT;
            z.target_pos[i] = target.pos;
            if d <= t.attack_reach {
                start_attack(z, i, t);
                return (Vec2::ZERO, true, Some(to), false);
            }
            if z.timer[i] <= 0.0 {
                z.state[i] = ZombieState::Chase;
                z.timer[i] = t.memory_secs;
            }
            (dir * (z.speed[i] * t.lunge_speed_mult).max(t.lunge_min_speed), false, None, false)
        }
        ZombieState::Attack => {
            z.target_pos[i] = target.pos;
            if d > t.attack_reach + t.attack_leave {
                z.state[i] = ZombieState::Chase;
                z.timer[i] = t.memory_secs;
                return (dir * z.speed[i], false, None, false);
            }
            if z.cooldown[i] <= 0.0 {
                // Nothing solid between (glass included; an open window is fine).
                if clear_path(cx.map, p, target.pos) {
                    cx.attacks.push(ZombieAttack { zombie: z.id[i], player: target.id, landed: false, damage: 0.0 });
                    z.cooldown[i] = t.attack_cooldown;
                } else {
                    // A door or window shut between us.
                    z.state[i] = ZombieState::Chase;
                    z.timer[i] = t.memory_secs;
                    return (Vec2::ZERO, false, None, false);
                }
            }
            // Shuffle in to stay in contact.
            let close_in = if d > t.attack_reach * 0.8 { dir * z.speed[i] * 0.5 } else { Vec2::ZERO };
            (close_in, true, Some(to), false)
        }
        _ => {
            let sees = z.sees[i] > 0;
            if sees {
                z.target_pos[i] = target.pos;
                z.timer[i] = t.memory_secs;
            } else {
                z.timer[i] -= TICK_DT;
                if z.timer[i] <= 0.0 {
                    start_investigating(z, i, t);
                    return (Vec2::ZERO, false, None, false);
                }
            }
            // Close and nothing in the way (seeing someone through glass doesn't count).
            let open = sees && d < 3.0 && clear_path(cx.map, p, target.pos);
            if open && d <= t.attack_reach {
                start_attack(z, i, t);
                return (Vec2::ZERO, true, Some(to), false);
            }
            if open && d <= t.lunge_range && z.cooldown[i] <= 0.0 {
                z.state[i] = ZombieState::Lunge;
                z.timer[i] = t.lunge_secs;
                z.cooldown[i] = t.lunge_cooldown;
                return (dir * (z.speed[i] * t.lunge_speed_mult).max(t.lunge_min_speed), false, None, false);
            }
            if open {
                return (dir * z.speed[i], false, None, true);
            }
            let goal = z.target_pos[i];
            // While it sees them it follows the player's own field; otherwise only the spot it remembers.
            let key = if sees { FlowKey::Player(target.id) } else { FlowKey::point(goal) };
            match steer(cx, z, i, key, goal) {
                Steer::Go(dir) => (dir * z.speed[i], false, None, true),
                // At the last known spot and nobody there: search around it.
                Steer::Arrived => (mill(z, i, cx.rng, t, goal, TICK_DT), false, None, false),
                Steer::Breach(edge, d) if can_batter(z, i, cx, edge, d) => {
                    let (a, l) = start_thump(z, i, cx, edge);
                    (Vec2::ZERO, a, l, false)
                }
                Steer::Breach(edge, _) => (approach(z, i, edge) * z.speed[i], false, None, true),
                Steer::Unreachable => {
                    lose_interest(z, i, t);
                    (Vec2::ZERO, false, None, false)
                }
            }
        }
    }
}

/// Back on its feet after a stagger or knockdown: after whoever hit it (it can't see them yet,
/// so it heads for where they were), or back to wandering.
pub(crate) fn recover(z: &mut Zombies, i: usize, t: &ZombieTuning) {
    if z.target_player[i] != 0 {
        z.state[i] = ZombieState::Chase;
        z.timer[i] = t.memory_secs;
    } else {
        z.state[i] = ZombieState::Wander;
        z.wander_timer[i] = 0.0;
    }
    z.cooldown[i] = z.cooldown[i].max(t.attack_windup);
}

fn start_attack(z: &mut Zombies, i: usize, t: &ZombieTuning) {
    z.state[i] = ZombieState::Attack;
    z.cooldown[i] = z.cooldown[i].max(t.attack_windup);
}

/// Lost (or lost interest in) its player: investigate the last known spot for a while.
pub(crate) fn start_investigating(z: &mut Zombies, i: usize, t: &ZombieTuning) {
    z.state[i] = ZombieState::Investigate;
    z.target_player[i] = 0;
    z.timer[i] = t.investigate_secs;
    z.wander_timer[i] = 0.0;
}

/// Gives up on getting where it was going (stuck against walls, no way there at all, or a
/// thumper nobody has given a reason to keep at it): mills about where it stands, then wanders,
/// and stays bored for a while (see [`ZombieTuning::bored_secs`]).
pub(crate) fn lose_interest(z: &mut Zombies, i: usize, t: &ZombieTuning) {
    start_investigating(z, i, t);
    z.target_pos[i] = z.pos[i];
    z.bored[i] = t.bored_secs;
    z.stuck[i] = 0.0;
    z.anchor[i] = z.pos[i];
}

/// What [`steer`] says to do.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Steer {
    /// Walk this way (unit vector).
    Go(Vec2),
    /// There, or as close as it can get (at the field's goal with the target out of sight).
    Arrived,
    /// The way on is through this shut door or window, this far away (to the edge): batter it
    /// once within reach (or close behind someone battering it), else walk up to it.
    Breach(EdgeRef, f32),
    /// No way there at all, not even through a door or window (only said of players' fields,
    /// which always lead to an open tile).
    Unreachable,
}

/// Unit direction from zombie `i` to the middle of `edge`.
fn approach(z: &Zombies, i: usize, edge: EdgeRef) -> Vec2 {
    (edge.mid() - z.pos[i]).normalize_or_zero()
}

/// Can zombie `i`, `d` metres from `edge`, batter it: in reach, or close behind the front rank
/// (crowd pressure)?
fn can_batter(z: &Zombies, i: usize, cx: &Ctx, edge: EdgeRef, d: f32) -> bool {
    let t = cx.tuning;
    d <= t.thump_reach || (d <= t.crowd_reach && front_rank(z, i, cx, edge))
}

/// Is some other zombie battering `edge` from within reach of it?
fn front_rank(z: &Zombies, i: usize, cx: &Ctx, edge: EdgeRef) -> bool {
    let reach = cx.tuning.thump_reach;
    let mut found = false;
    cx.hash.for_each_within(edge.mid(), reach + 0.5, |j| {
        found |= j != i && z.state[j] == ZombieState::Thump && z.thump_edge[j] == edge && edge.distance(z.pos[j]) <= reach;
    });
    found
}

/// Thump `edge`: start battering it. Returns (attacking, look) for the zombie tick.
fn start_thump(z: &mut Zombies, i: usize, cx: &mut Ctx, edge: EdgeRef) -> (bool, Option<Vec2>) {
    z.state[i] = ZombieState::Thump;
    z.thump_edge[i] = edge;
    // Interest in getting through runs down from here unless a player is seen or heard.
    z.timer[i] = z.timer[i].min(cx.tuning.thump_interest_secs);
    // A short, varied wind-up, so a crowd arriving together doesn't thump in step.
    z.cooldown[i] = z.cooldown[i].max(cx.rng.range(0.2, cx.tuning.thump_cooldown * 0.6));
    (true, Some(edge.mid() - z.pos[i]))
}

/// Thump: batter the door or window on a cooldown while the way to the target still leads through
/// it (rechecked every tick against the same flow field the chase or investigation uses). Close
/// behind the front rank it pushes in and adds crowd pressure (a fraction of a thump). Its
/// interest (`timer`) runs down unless it sees or hears a player (the senses top it up to
/// `thump_interest_secs`); when it runs out the zombie gives up. Once the way is open, or leads
/// elsewhere, it goes back to chasing or investigating.
fn thump(z: &mut Zombies, i: usize, cx: &mut Ctx) -> (Vec2, bool, Option<Vec2>) {
    let t = cx.tuning;
    let p = z.pos[i];
    z.timer[i] -= TICK_DT;
    if z.timer[i] <= 0.0 {
        lose_interest(z, i, t);
        return (Vec2::ZERO, false, None);
    }
    let player = z.target_player[i];
    let mut key = FlowKey::point(z.target_pos[i]);
    if player != 0 {
        match cx.targets.iter().find(|tg| tg.id == player) {
            Some(tg) if z.sees[i] > 0 => {
                z.target_pos[i] = tg.pos;
                key = FlowKey::Player(player);
            }
            Some(_) => {}
            None => {
                start_investigating(z, i, t);
                return (Vec2::ZERO, false, None);
            }
        }
    }
    let goal = z.target_pos[i];
    match steer(cx, z, i, key, goal) {
        Steer::Breach(edge, d) if can_batter(z, i, cx, edge, d) => {
            z.thump_edge[i] = edge;
            let front = d <= t.thump_reach;
            if z.cooldown[i] <= 0.0 {
                let damage = if front { t.thump_damage } else { t.thump_damage * t.crowd_thump_frac };
                cx.thumps.push(ZombieThump { zombie: z.id[i], edge, from: p, damage });
                z.cooldown[i] = t.thump_cooldown * cx.rng.range(0.8, 1.2);
            }
            // Lean in, so the crowd behind presses it against the door; behind the front rank,
            // push.
            let to = edge.mid() - p;
            let lean = if front { 0.2 } else { z.speed[i] * 0.5 };
            (to.normalize_or_zero() * lean, true, Some(to))
        }
        Steer::Breach(edge, _) => {
            // Shoved back from it by the crowd: push in again.
            z.thump_edge[i] = edge;
            (approach(z, i, edge) * z.speed[i], false, None)
        }
        Steer::Unreachable => {
            lose_interest(z, i, t);
            (Vec2::ZERO, false, None)
        }
        other => {
            let speed = if player != 0 {
                z.state[i] = ZombieState::Chase;
                z.speed[i]
            } else {
                z.state[i] = ZombieState::Investigate;
                z.speed[i] * t.investigate_speed_mult
            };
            let desired = if let Steer::Go(dir) = other { dir * speed } else { Vec2::ZERO };
            (desired, false, None)
        }
    }
}

/// How zombie `i` gets toward `target`: straight there when the line is clear, else along the
/// cached flow field for `key` when there is one covering it, else straight anyway (sliding along
/// walls). When the field's next step goes through a shut door or window, it says
/// [`Steer::Breach`] (walk up to it, and batter it once in reach). [`Steer::Arrived`] once it's there (with nothing solid
/// between), or can't get closer (at the field's goal with the target out of sight behind a wall).
/// [`Steer::Unreachable`] when a player's field (which always leads to the open tile the player
/// stands on) has no way to them from here, not even through a door or window.
fn steer(cx: &mut Ctx, z: &mut Zombies, i: usize, key: FlowKey, target: Vec2) -> Steer {
    let p = z.pos[i];
    let to = target - p;
    let d = to.length();
    // Within arm's length counts as there, unless a door or window is in the way.
    if d <= 1.2 && (d < 0.05 || clear_path(cx.map, p, target)) {
        return Steer::Arrived;
    }
    if (i as u64 + cx.tick).is_multiple_of(DIRECT_CHECK_TICKS) {
        z.direct[i] = d <= DIRECT_MAX && clear_path(cx.map, p, target);
    }
    if z.direct[i] {
        return Steer::Go(to / d);
    }
    let goal_tile = target.floor().as_ivec2();
    if let Some(field) = cx.flow.get(key, goal_tile, cx.tick) {
        match field.distance(p) {
            Some(fd) if fd > 1.5 => {
                // The way on through a shut door or window: from this tile, or (crowding in at
                // the side or behind) from one of the next two along the path, if that close.
                let mut tile = p.floor().as_ivec2();
                for ahead in 0..3 {
                    if let Some(side) = field.straight_step_side_of(tile)
                        && cx.map.edge(tile, side).is_breakable()
                    {
                        let edge = EdgeRef::of(tile, side);
                        let d = edge.distance(p);
                        if ahead == 0 || d <= cx.tuning.crowd_reach {
                            return Steer::Breach(edge, d);
                        }
                        break;
                    }
                    match field.next_tile(tile) {
                        Some(next) => tile = next,
                        None => break,
                    }
                }
                let dir = field.direction(p);
                if dir != Vec2::ZERO {
                    return Steer::Go(dir);
                }
            }
            // At the field's goal: the target is in this cell; walk the last bit if it's in sight.
            Some(_) if d > 2.0 && !line_of_sight(cx.map, p, target) => return Steer::Arrived,
            // Well inside the field's range and still no way there.
            None if matches!(key, FlowKey::Player(_)) && d <= UNREACHABLE_NEAR => return Steer::Unreachable,
            _ => {}
        }
    }
    Steer::Go(to / d)
}

/// Idle shuffling: pick a direction (or stand) every few seconds. `dt` is the time since its last
/// update (one tick when active, more on the near tier).
pub(crate) fn wander(z: &mut Zombies, i: usize, rng: &mut Rng, t: &ZombieTuning, dt: f32) -> Vec2 {
    z.wander_timer[i] -= dt;
    if z.wander_timer[i] <= 0.0 {
        z.wander_timer[i] = rng.range(3.0, 9.0);
        z.wander[i] = if rng.f32() < 0.5 { Vec2::ZERO } else { Vec2::from_angle(rng.range(0.0, TAU)) * t.wander_speed };
    }
    z.wander[i]
}

/// Milling about near `centre` (searching a spot): short hops, turning back when it strays.
pub(crate) fn mill(z: &mut Zombies, i: usize, rng: &mut Rng, t: &ZombieTuning, centre: Vec2, dt: f32) -> Vec2 {
    z.wander_timer[i] -= dt;
    if z.wander_timer[i] <= 0.0 {
        z.wander_timer[i] = rng.range(1.0, 3.0);
        let back = centre - z.pos[i];
        z.wander[i] = if back.length() > t.search_radius {
            back.normalize_or_zero() * t.wander_speed * 1.5
        } else if rng.f32() < 0.3 {
            Vec2::ZERO
        } else {
            Vec2::from_angle(rng.range(0.0, TAU)) * t.wander_speed * 1.5
        };
    }
    z.wander[i]
}

/// Rotate angle `from` toward `to` by at most `max_step` radians, taking the short way round.
pub(crate) fn turn_toward(from: f32, to: f32, max_step: f32) -> f32 {
    let mut d = (to - from) % TAU;
    if d > std::f32::consts::PI {
        d -= TAU;
    } else if d < -std::f32::consts::PI {
        d += TAU;
    }
    from + d.clamp(-max_step, max_step)
}
