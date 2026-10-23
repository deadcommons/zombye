//! Fight or flee: players swing weapons, shove and stomp; zombies take damage, stagger, fall down
//! and die; zombie strikes hurt players.
//!
//! A player action ([`ActionKind`]) starts when its intent arrives (or is queued if it comes just
//! before the last one finishes), costs endurance at once, and lands after a windup. At impact the
//! server finds zombies within reach and arc in the player's facing, with a clear line (glass and
//! fences block), nearest first up to the weapon's `max_targets`, and applies damage (less for each
//! extra target), knockback and seeded stagger / knockdown rolls. Hits from behind crit more and
//! knock down more easily. Below `tired_below` endurance, actions get slower and weaker. Every
//! action makes a noise.
//!
//! Zombie strikes ([`crate::ai::ZombieAttack`]) land with `zombie_hit_chance`, more likely the more
//! zombies are on the same player, and do `zombie_damage`. A player at 0 HP is dead: input is
//! ignored, zombies lose interest, and the weapon falls to the floor. Every number is in
//! [`CombatTuning`] or the weapon data; all ours.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::TICK_DT;
use crate::ai::ZOMBIE_RADIUS;
use crate::items::{GroundItem, WeaponDef};
use crate::players::{Action, ActionKind, PLAYER_RADIUS, Player, PlayerInput, RUN_MIN_ENDURANCE};
use crate::senses::{Sound, SoundSource};
use crate::vision::clear_path;
use crate::world::World;
use crate::zombies::{ZombieAnim, ZombieState};

/// How far from its position a player can pick something up, and how far ahead it aims.
pub const PICKUP_REACH: f32 = 1.3;
/// A knocked-back zombie moves at most this fast (m/s), so one tick's step (0.25 m) stays under its
/// body radius and it can't be knocked through a wall.
pub const MAX_KNOCKBACK_SPEED: f32 = 5.0;

/// Player health and every combat number that isn't a weapon's. Serde with defaults, like
/// [`crate::ZombieTuning`]. Distances in metres, times in seconds, chances 0..1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CombatTuning {
    /// A player's full hit points.
    pub player_hp: f32,
    /// Zombie hit points are rolled per zombie in this range.
    pub zombie_hp: (f32, f32),

    // Endurance.
    /// Below this endurance actions weaken and slow down, down to `tired_power` (damage, knockback
    /// and knockdown chance) and `tired_slow` (time multiplier) at 0.
    pub tired_below: f32,
    pub tired_power: f32,
    pub tired_slow: f32,
    /// An action asked for this close to the end of the last one is queued instead of dropped.
    pub action_buffer_secs: f32,

    // Hitting.
    /// Each target after the first in one swing takes this much more of the damage (0.8 → 80%, 64%...).
    pub multi_hit_falloff: f32,
    /// A zombie facing away from the attacker (its forward · attacker→zombie above this) is hit from behind.
    pub behind_dot: f32,
    /// From behind: added crit chance, and knockdown chance multiplier.
    pub behind_crit_bonus: f32,
    pub behind_knockdown_mult: f32,
    /// Knockdown chance multiplier against a zombie already staggered.
    pub staggered_knockdown_mult: f32,
    /// How long a stagger and a knockdown last.
    pub stagger_secs: (f32, f32),
    pub down_secs: (f32, f32),

    // Shove.
    pub shove_range: f32,
    pub shove_arc_degrees: f32,
    pub shove_targets: u32,
    pub shove_knockback: f32,
    /// Knockdown chance of a shove (it always staggers), before the staggered / behind multipliers.
    pub shove_knockdown_chance: f32,
    pub shove_windup: f32,
    pub shove_seconds: f32,
    pub shove_endurance: f32,
    pub shove_noise: f32,

    // Stomp.
    /// Reach from the player's centre to a downed zombie's centre.
    pub stomp_range: f32,
    pub stomp_arc_degrees: f32,
    pub stomp_damage: (f32, f32),
    pub stomp_crit_chance: f32,
    pub stomp_crit_multiplier: f32,
    pub stomp_windup: f32,
    pub stomp_seconds: f32,
    pub stomp_endurance: f32,
    pub stomp_noise: f32,

    // Zombie strikes.
    /// Chance a strike lands, plus this for every other zombie attacking the same player, at most
    /// `zombie_hit_max`.
    pub zombie_hit_chance: f32,
    pub zombie_crowd_bonus: f32,
    pub zombie_hit_max: f32,
    pub zombie_damage: (f32, f32),
}

impl Default for CombatTuning {
    fn default() -> Self {
        Self {
            player_hp: 100.0,
            zombie_hp: (85.0, 115.0),

            tired_below: 0.5,
            tired_power: 0.55,
            tired_slow: 1.6,
            action_buffer_secs: 0.2,

            multi_hit_falloff: 0.8,
            behind_dot: 0.35,
            behind_crit_bonus: 0.3,
            behind_knockdown_mult: 1.6,
            staggered_knockdown_mult: 1.5,
            stagger_secs: (0.6, 0.9),
            down_secs: (3.0, 5.0),

            shove_range: 0.75,
            shove_arc_degrees: 150.0,
            shove_targets: 3,
            shove_knockback: 0.75,
            shove_knockdown_chance: 0.25,
            shove_windup: 0.1,
            shove_seconds: 0.6,
            shove_endurance: 0.035,
            shove_noise: 2.5,

            stomp_range: 1.1,
            stomp_arc_degrees: 140.0,
            stomp_damage: (30.0, 42.0),
            stomp_crit_chance: 0.15,
            stomp_crit_multiplier: 2.0,
            stomp_windup: 0.15,
            stomp_seconds: 0.55,
            stomp_endurance: 0.04,
            stomp_noise: 4.0,

            zombie_hit_chance: 0.5,
            zombie_crowd_bonus: 0.12,
            zombie_hit_max: 0.9,
            zombie_damage: (7.0, 12.0),
        }
    }
}

impl CombatTuning {
    /// (power, slowness) of an action started at this endurance: (1, 1) when rested.
    pub fn fatigue(&self, endurance: f32) -> (f32, f32) {
        if endurance >= self.tired_below || self.tired_below <= 0.0 {
            return (1.0, 1.0);
        }
        let x = (endurance / self.tired_below).clamp(0.0, 1.0);
        (self.tired_power + (1.0 - self.tired_power) * x, self.tired_slow + (1.0 - self.tired_slow) * x)
    }
}

/// What a hit did to a zombie.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HitEffect {
    /// Hurt, still coming.
    Hit,
    Staggered,
    KnockedDown,
    Killed,
}

/// A player's action struck a zombie this tick.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HitEvent {
    pub player: u32,
    pub zombie: u32,
    pub kind: ActionKind,
    pub damage: f32,
    pub crit: bool,
    pub behind: bool,
    pub effect: HitEffect,
}

/// What a [`CombatEvent`] was, for hit feedback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CombatEventKind {
    /// A swing hurt a zombie (it may also have staggered or floored it).
    ZombieHit,
    /// A swing or stomp killed a zombie.
    ZombieKilled,
    /// A zombie's strike landed on a player.
    PlayerHurt,
    /// A shove pushed a zombie back (no damage).
    Shove,
    /// A stomp hurt a downed zombie without killing it.
    Stomp,
}

/// Something worth showing: blood, a flash, a flinch. Collected every tick into
/// `World::combat_events` and drained into snapshots, so a client sees each one once.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CombatEvent {
    pub kind: CombatEventKind,
    /// Where the body that was hit stood.
    pub pos: Vec2,
    /// Unit direction of the blow (attacker toward victim).
    pub dir: Vec2,
    /// The zombie hit, or the one that struck.
    pub zombie: u32,
    /// The player who struck, or the one who was hurt.
    pub player: u32,
    pub damage: f32,
    pub crit: bool,
}

/// Combat events kept for replication if nobody drains them (headless servers, tests).
pub const MAX_COMBAT_EVENTS: usize = 1024;

/// A player's action landed this tick (whether or not it hit anything).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionEvent {
    pub player: u32,
    pub kind: ActionKind,
    pub weapon: u16,
    pub hits: u32,
}

/// The parameters of one landing action, from the weapon or the shove/stomp tuning.
struct Strike {
    /// Reach to the zombie's body edge (centre for stomps).
    range: f32,
    /// Half the arc, in radians.
    half_arc: f32,
    max_targets: u32,
    damage: (f32, f32),
    crit_chance: f32,
    crit_multiplier: f32,
    knockback: f32,
    stagger_chance: f32,
    knockdown_chance: f32,
    noise: f32,
}

impl World {
    /// Bare hands' index in `config.weapons`.
    pub fn unarmed(&self) -> u16 {
        self.unarmed
    }

    /// The weapon a player swings (bare hands if none).
    pub fn wielded(&self, weapon: Option<u16>) -> &WeaponDef {
        &self.config.weapons[weapon.unwrap_or(self.unarmed) as usize]
    }

    pub fn weapon_index(&self, id: &str) -> Option<u16> {
        self.config.weapons.iter().position(|w| w.id == id).map(|i| i as u16)
    }

    /// Items lying about, by id.
    pub fn items(&self) -> &[GroundItem] {
        &self.items
    }

    /// Player `id` swings its weapon (or punches). Returns whether it started or was queued.
    pub fn player_attack(&mut self, id: u32) -> bool {
        self.request_action(id, ActionKind::Swing)
    }

    /// Player `id` shoves whatever is in front of it.
    pub fn player_shove(&mut self, id: u32) -> bool {
        self.request_action(id, ActionKind::Shove)
    }

    /// Player `id` stamps on a downed zombie at its feet (a miss if there's none at impact).
    pub fn player_stomp(&mut self, id: u32) -> bool {
        self.request_action(id, ActionKind::Stomp)
    }

    fn request_action(&mut self, id: u32, kind: ActionKind) -> bool {
        let buffer = self.config.combat.action_buffer_secs;
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return false };
        if self.players[pi].dead {
            return false;
        }
        // Fighting drops whatever you were doing with your things.
        self.cancel_jobs(pi);
        let p = &mut self.players[pi];
        if p.action.is_none() && p.ready_in <= 0.0 {
            self.begin_action(pi, kind);
            true
        } else if p.ready_in <= buffer {
            p.queued = Some(kind);
            true
        } else {
            false
        }
    }

    fn begin_action(&mut self, pi: usize, kind: ActionKind) {
        let weapon = self.players[pi].weapon.unwrap_or(self.unarmed);
        let c = &self.config.combat;
        let (windup, total, cost) = match kind {
            ActionKind::Swing => {
                let w = &self.config.weapons[weapon as usize];
                (w.windup_seconds, w.swing_seconds, w.endurance_cost)
            }
            ActionKind::Shove => (c.shove_windup, c.shove_seconds, c.shove_endurance),
            ActionKind::Stomp => (c.stomp_windup, c.stomp_seconds, c.stomp_endurance),
        };
        // A heavy load makes every action more tiring.
        let cost = if self.carry(&self.players[pi]).burden >= crate::inventory::Burden::Heavy { cost * self.config.carry.heavy_action_cost } else { cost };
        let c = &self.config.combat;
        let p = &mut self.players[pi];
        let (power, slow) = c.fatigue(p.state.endurance);
        p.state.endurance = (p.state.endurance - cost).max(0.0);
        if p.state.endurance <= RUN_MIN_ENDURANCE {
            p.state.winded = true;
        }
        p.action = Some(Action { kind, timer: windup * slow, power, weapon });
        p.ready_in = total * slow;
        p.queued = None;
    }

    /// Single-player / debug: a dead player gets up again at the spawn point with full health and
    /// empty hands. Returns whether it respawned.
    pub fn respawn_player(&mut self, id: u32) -> bool {
        let spawn = self.map.resolve_circle(self.player_spawn, PLAYER_RADIUS);
        let hp = self.config.combat.player_hp;
        let Some(p) = self.players.iter_mut().find(|p| p.id == id) else { return false };
        if !p.dead {
            return false;
        }
        *p = Player { last_input_seq: p.last_input_seq, input: PlayerInput::default(), ..Player::new(id, spawn, hp) };
        let pi = self.players.iter().position(|p| p.id == id).expect("just found");
        self.refresh_carry(pi);
        true
    }

    /// Count down action timers and land the actions due this tick. Needs the spatial hash
    /// current (rebuilt after players move).
    pub(crate) fn step_player_actions(&mut self) {
        let mut due = std::mem::take(&mut self.scratch_actions);
        due.clear();
        for pi in 0..self.players.len() {
            let p = &mut self.players[pi];
            if p.dead {
                continue;
            }
            p.ready_in = (p.ready_in - TICK_DT).max(0.0);
            if let Some(mut a) = p.action {
                a.timer -= TICK_DT;
                if a.timer <= 0.0 {
                    p.action = None;
                    due.push((pi, a));
                } else {
                    p.action = Some(a);
                }
            }
            if p.action.is_none()
                && p.ready_in <= 0.0
                && let Some(kind) = p.queued.take()
            {
                self.begin_action(pi, kind);
            }
        }
        for &(pi, a) in &due {
            self.land(pi, a);
        }
        self.scratch_actions = due;
    }

    /// An action lands: find its targets and hit them.
    fn land(&mut self, pi: usize, a: Action) {
        let c = &self.config.combat;
        let s = match a.kind {
            ActionKind::Swing => {
                let w = &self.config.weapons[a.weapon as usize];
                Strike {
                    range: w.range,
                    half_arc: (w.arc_degrees * 0.5).to_radians(),
                    max_targets: w.max_targets,
                    damage: w.damage,
                    crit_chance: w.crit_chance,
                    crit_multiplier: w.crit_multiplier,
                    knockback: w.knockback,
                    stagger_chance: w.stagger_chance,
                    knockdown_chance: w.knockdown_chance,
                    noise: w.noise_radius,
                }
            }
            ActionKind::Shove => Strike {
                range: c.shove_range,
                half_arc: (c.shove_arc_degrees * 0.5).to_radians(),
                max_targets: c.shove_targets,
                damage: (0.0, 0.0),
                crit_chance: 0.0,
                crit_multiplier: 1.0,
                knockback: c.shove_knockback,
                stagger_chance: 1.0,
                knockdown_chance: c.shove_knockdown_chance,
                noise: c.shove_noise,
            },
            ActionKind::Stomp => Strike {
                range: c.stomp_range,
                half_arc: (c.stomp_arc_degrees * 0.5).to_radians(),
                max_targets: 1,
                damage: c.stomp_damage,
                crit_chance: c.stomp_crit_chance,
                crit_multiplier: c.stomp_crit_multiplier,
                knockback: 0.0,
                stagger_chance: 0.0,
                knockdown_chance: 0.0,
                noise: c.stomp_noise,
            },
        };
        let player = self.players[pi].id;
        let origin = self.players[pi].state.pos;
        let forward = Vec2::from_angle(self.players[pi].state.facing);
        let stomp = a.kind == ActionKind::Stomp;

        // Candidates in reach and arc, nearest first (ties by index, so the order is stable).
        let mut found = std::mem::take(&mut self.scratch_hits);
        found.clear();
        let z = &self.zombies;
        let reach = s.range + if stomp { 0.0 } else { ZOMBIE_RADIUS };
        self.hash.for_each_within(origin, reach, |j| {
            // Swings and shoves pass over downed zombies; stomps only hit them.
            let state = z.state[j];
            if state == ZombieState::Dead || (state == ZombieState::Down) != stomp {
                return;
            }
            let to = z.pos[j] - origin;
            let d = to.length();
            if d > reach {
                return;
            }
            if d > 1e-4 {
                // In the arc if any of its body is: widen by the body's angular half-width.
                let cos = forward.dot(to / d);
                let half_width = if stomp { 0.0 } else { (ZOMBIE_RADIUS / d).min(1.0).asin() };
                if cos.clamp(-1.0, 1.0).acos() > s.half_arc + half_width {
                    return;
                }
            }
            found.push((d, j));
        });
        found.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

        let mut hits = 0;
        for &(_, j) in &found {
            if hits >= s.max_targets {
                break;
            }
            if !clear_path(&self.map, origin, self.zombies.pos[j]) {
                continue;
            }
            let falloff = self.config.combat.multi_hit_falloff.powi(hits as i32);
            self.strike(player, a, &s, origin, forward, j, falloff);
            hits += 1;
        }
        self.scratch_hits = found;
        self.action_events.push(ActionEvent { player, kind: a.kind, weapon: a.weapon, hits });
        if s.noise > 0.0 && self.heard.len() < crate::world::MAX_SOUNDS {
            // Heard this tick, like footsteps.
            self.heard.push(Sound::new(origin, s.noise, SoundSource::Player(player)));
        }
    }

    /// Hit zombie `j` from `origin`.
    #[allow(clippy::too_many_arguments)]
    fn strike(&mut self, player: u32, a: Action, s: &Strike, origin: Vec2, forward: Vec2, j: usize, falloff: f32) {
        let c = &self.config.combat;
        let zt = &self.config.zombie;
        let rng = &mut self.combat_rng;
        let z = &mut self.zombies;
        let to = z.pos[j] - origin;
        let dir = to.normalize_or(forward);
        let behind = Vec2::from_angle(z.facing[j]).dot(dir) > c.behind_dot;
        let crit_chance = s.crit_chance + if behind && s.crit_chance > 0.0 { c.behind_crit_bonus } else { 0.0 };
        let crit = rng.f32() < crit_chance;
        let mut damage = if s.damage.1 > 0.0 { rng.range(s.damage.0, s.damage.1) * a.power * falloff } else { 0.0 };
        if crit {
            damage *= s.crit_multiplier;
        }
        z.hp[j] -= damage;
        if s.knockback > 0.0 {
            z.vel[j] = dir * knockback_speed(s.knockback * a.power);
        }
        // Whatever happens next, it knows who did it.
        z.target_player[j] = player;
        z.target_pos[j] = origin;
        z.bored[j] = 0.0;
        let was = z.state[j];
        let effect = if z.hp[j] <= 0.0 {
            z.hp[j] = 0.0;
            z.state[j] = ZombieState::Dead;
            z.anim[j] = ZombieAnim::Dead;
            z.target_player[j] = 0;
            z.timer[j] = 0.0;
            HitEffect::Killed
        } else if was == ZombieState::Down {
            // Stomped but not dead: stays down.
            HitEffect::Hit
        } else {
            let mut knockdown = s.knockdown_chance * a.power;
            if behind {
                knockdown *= c.behind_knockdown_mult;
            }
            if was == ZombieState::Staggered {
                knockdown *= c.staggered_knockdown_mult;
            }
            let roll = rng.f32();
            if roll < knockdown {
                z.state[j] = ZombieState::Down;
                z.anim[j] = ZombieAnim::Down;
                z.timer[j] = rng.range(c.down_secs.0, c.down_secs.1);
                HitEffect::KnockedDown
            } else if rng.f32() < s.stagger_chance {
                z.state[j] = ZombieState::Staggered;
                z.anim[j] = ZombieAnim::Stagger;
                z.timer[j] = rng.range(c.stagger_secs.0, c.stagger_secs.1);
                HitEffect::Staggered
            } else {
                // Hurt but not stopped: it's after you now.
                if !matches!(was, ZombieState::Staggered) {
                    z.state[j] = if was == ZombieState::Attack { ZombieState::Attack } else { ZombieState::Chase };
                    z.timer[j] = zt.memory_secs;
                }
                HitEffect::Hit
            }
        };
        if matches!(effect, HitEffect::Staggered | HitEffect::KnockedDown) {
            // No instant strike when it recovers.
            z.cooldown[j] = z.cooldown[j].max(zt.attack_windup);
        }
        let (zid, zpos) = (z.id[j], z.pos[j]);
        self.hit_events.push(HitEvent { player, zombie: zid, kind: a.kind, damage, crit, behind, effect });
        let kind = match (effect, a.kind) {
            (HitEffect::Killed, _) => CombatEventKind::ZombieKilled,
            (_, ActionKind::Swing) => CombatEventKind::ZombieHit,
            (_, ActionKind::Shove) => CombatEventKind::Shove,
            (_, ActionKind::Stomp) => CombatEventKind::Stomp,
        };
        self.push_combat_event(CombatEvent { kind, pos: zpos, dir, zombie: zid, player, damage, crit });
    }

    pub(crate) fn push_combat_event(&mut self, e: CombatEvent) {
        if self.combat_events.len() < MAX_COMBAT_EVENTS {
            self.combat_events.push(e);
        }
    }

    /// Turn this tick's zombie strikes into damage. Run after the zombie pass.
    pub(crate) fn apply_zombie_attacks(&mut self) {
        let c = self.config.combat.clone();
        let reach = self.config.zombie.attack_reach + self.config.zombie.attack_leave + 0.5;
        for k in 0..self.attack_events.len() {
            let a = self.attack_events[k];
            let Some(pi) = self.players.iter().position(|p| p.id == a.player) else { continue };
            if self.players[pi].dead {
                continue;
            }
            // How many are on this player right now.
            let pos = self.players[pi].state.pos;
            let z = &self.zombies;
            let mut on = 0u32;
            self.hash.for_each_within(pos, reach + 1.0, |j| {
                if z.state[j] == ZombieState::Attack && z.target_player[j] == a.player && z.pos[j].distance(pos) <= reach {
                    on += 1;
                }
            });
            let chance = (c.zombie_hit_chance + c.zombie_crowd_bonus * on.saturating_sub(1) as f32).min(c.zombie_hit_max);
            if self.combat_rng.f32() >= chance {
                continue;
            }
            let damage = self.combat_rng.range(c.zombie_damage.0, c.zombie_damage.1);
            self.attack_events[k].landed = true;
            self.attack_events[k].damage = damage;
            let from = self.zombies.index_of(a.zombie).map_or(pos, |j| self.zombies.pos[j]);
            let dir = (pos - from).normalize_or(Vec2::X);
            self.push_combat_event(CombatEvent { kind: CombatEventKind::PlayerHurt, pos, dir, zombie: a.zombie, player: a.player, damage, crit: false });
            let p = &mut self.players[pi];
            p.hp -= damage;
            if p.hp <= 0.0 {
                self.kill_player(pi);
            }
        }
    }

    pub(crate) fn kill_player(&mut self, pi: usize) {
        let p = &mut self.players[pi];
        p.hp = 0.0;
        p.dead = true;
        p.action = None;
        p.queued = None;
        p.input.move_dir = Vec2::ZERO;
        // Everything it had falls where it lay.
        self.drop_everything(pi);
    }
}

/// Initial speed that knocks a body back about `metres` (velocity decays by a quarter a tick, so
/// the slide covers about 3 ticks' worth), capped at [`MAX_KNOCKBACK_SPEED`].
pub fn knockback_speed(metres: f32) -> f32 {
    (metres / (3.0 * TICK_DT)).min(MAX_KNOCKBACK_SPEED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{Edge, Side, TileMap};
    use crate::template::BuildingTemplate;
    use crate::world::WorldConfig;
    use crate::{PlayerInput, items};
    use glam::IVec2;
    use std::f32::consts::PI;

    fn templates() -> Vec<BuildingTemplate> {
        [include_str!("../../../assets/data/buildings/bungalow.ron"), include_str!("../../../assets/data/buildings/family_two_storey.ron")]
            .iter()
            .map(|t| BuildingTemplate::from_ron(t).unwrap())
            .collect()
    }

    fn weapons() -> Vec<WeaponDef> {
        items::parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap()
    }

    fn items() -> Vec<crate::items::ItemDef> {
        items::parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons()).unwrap()
    }

    fn world(seed: u64, zombies: u32) -> World {
        World::new(WorldConfig::new(seed, zombies, templates()).with_weapons(weapons()).with_items(items()))
    }

    /// An empty grass field at noon with one player at (100.5, 100.5) facing east and nothing else.
    fn arena(seed: u64) -> (World, u32) {
        let mut config = WorldConfig::new(seed, 0, templates()).with_weapons(weapons()).with_items(items());
        config.start_hour = 12.0;
        let mut w = World::new(config);
        w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
        w.map_changed();
        w.items.clear();
        let id = w.add_player(Vec2::new(100.5, 100.5));
        (w, id)
    }

    /// Put a weapon (its item) in player `id`'s hand. Returns the weapon's index.
    fn equip(w: &mut World, id: u32, weapon: &str) -> u16 {
        assert!(w.hold_item(id, crate::inventory::Slot::Hand, weapon));
        let k = w.weapon_index(weapon).unwrap();
        assert_eq!(w.player(id).unwrap().weapon, Some(k));
        k
    }

    /// A zombie that stands still (it can't wander off by chance), facing `facing`.
    fn zombie(w: &mut World, pos: Vec2, facing: f32) -> usize {
        let i = w.add_zombie(pos, facing, 1.0);
        w.zombies.wander_timer[i] = 1e9;
        i
    }

    /// Step until the player's current action has landed (and it can act again). Returns the hits.
    fn until_ready(w: &mut World, id: u32) -> Vec<HitEvent> {
        let mut hits = Vec::new();
        for _ in 0..200 {
            w.step();
            hits.extend_from_slice(&w.hit_events);
            let p = w.player(id).unwrap();
            if p.action.is_none() && p.ready_in <= 0.0 && p.queued.is_none() {
                return hits;
            }
        }
        panic!("action never finished");
    }

    fn rest(w: &mut World, id: u32) {
        w.players.iter_mut().find(|p| p.id == id).unwrap().state.endurance = 1.0;
    }

    #[test]
    fn a_bat_kills_a_zombie_in_a_few_hits() {
        for seed in 1..=6 {
            let (mut w, id) = arena(seed);
            let bat = equip(&mut w, id, "baseball_bat");
            // Knockdowns are tested with stomps below; here we count swings at a standing zombie.
            w.config.weapons[bat as usize].knockdown_chance = 0.0;
            let z = zombie(&mut w, Vec2::new(101.6, 100.5), PI);
            let mut hits = 0;
            for _ in 0..12 {
                // Keep it in front of the bat.
                w.zombies.pos[z] = Vec2::new(101.6, 100.5);
                rest(&mut w, id);
                assert!(w.player_attack(id));
                hits += until_ready(&mut w, id).iter().filter(|h| h.zombie == w.zombies.id[z]).count();
                if w.zombies.state[z] == ZombieState::Dead {
                    break;
                }
            }
            assert_eq!(w.zombies.state[z], ZombieState::Dead, "seed {seed}: still standing after {hits} hits");
            assert!((2..=5).contains(&hits), "seed {seed}: took {hits} hits");
            assert_eq!(w.zombies.anim[z], ZombieAnim::Dead);
        }
    }

    #[test]
    fn swings_only_hit_in_front_and_in_reach() {
        let (mut w, id) = arena(2);
        equip(&mut w, id, "baseball_bat");
        let front = zombie(&mut w, Vec2::new(101.5, 100.5), PI);
        let behind = zombie(&mut w, Vec2::new(99.5, 100.5), 0.0);
        let beside = zombie(&mut w, Vec2::new(100.5, 99.2), 0.0);
        let far = zombie(&mut w, Vec2::new(103.5, 100.5), PI);
        w.player_attack(id);
        let hits = until_ready(&mut w, id);
        let ids: Vec<u32> = hits.iter().map(|h| h.zombie).collect();
        assert_eq!(ids, vec![w.zombies.id[front]], "hit {ids:?}");
        for i in [behind, beside, far] {
            assert!(w.zombies.hp[i] > 80.0 && w.zombies.state[i] != ZombieState::Dead);
        }
        // A wall between: no hit.
        let (mut w, id) = arena(2);
        equip(&mut w, id, "baseball_bat");
        for y in 90..110 {
            w.map.set_edge(IVec2::new(101, y), Side::W, Edge::Wall);
        }
        w.map_changed();
        zombie(&mut w, Vec2::new(101.5, 100.5), PI);
        w.player_attack(id);
        assert!(until_ready(&mut w, id).is_empty(), "hit through a wall");
    }

    #[test]
    fn max_targets_is_respected_nearest_first() {
        for (weapon, want) in [("baseball_bat", 3), ("kitchen_knife", 1), ("fire_axe", 2)] {
            let (mut w, id) = arena(3);
            let k = equip(&mut w, id, weapon);
            // Land at once, before the crowd shuffles apart.
            w.config.weapons[k as usize].windup_seconds = 0.0;
            // A fan of six zombies in front, at slightly different distances.
            let mut placed = Vec::new();
            for k in 0..6 {
                let a = (k as f32 - 2.5) * 0.12;
                let d = 0.75 + k as f32 * 0.03;
                placed.push(zombie(&mut w, Vec2::new(100.5, 100.5) + Vec2::from_angle(a) * d, PI));
            }
            w.player_attack(id);
            let hits = until_ready(&mut w, id);
            assert_eq!(hits.len(), want, "{weapon}: {} hits", hits.len());
            let nearest: Vec<u32> = placed.iter().take(want).map(|&i| w.zombies.id[i]).collect();
            assert_eq!(hits.iter().map(|h| h.zombie).collect::<Vec<_>>(), nearest, "{weapon}");
            // Later targets take less.
            if want > 1 && !hits.iter().any(|h| h.crit) {
                assert!(hits[1].damage < w.config.weapons[w.weapon_index(weapon).unwrap() as usize].damage.1);
            }
        }
    }

    #[test]
    fn shove_pushes_back_and_staggers_without_damage() {
        let (mut w, id) = arena(4);
        let a = zombie(&mut w, Vec2::new(101.3, 100.3), PI);
        let b = zombie(&mut w, Vec2::new(101.2, 100.9), PI);
        let behind = zombie(&mut w, Vec2::new(99.6, 100.5), 0.0);
        let hp: Vec<f32> = w.zombies.hp.clone();
        let before = [w.zombies.pos[a], w.zombies.pos[b]];
        assert!(w.player_shove(id));
        let hits = until_ready(&mut w, id);
        assert_eq!(hits.len(), 2, "{hits:?}");
        for h in &hits {
            assert_eq!(h.damage, 0.0);
            assert!(matches!(h.effect, HitEffect::Staggered | HitEffect::KnockedDown), "{h:?}");
        }
        for (k, &i) in [a, b].iter().enumerate() {
            let pushed = w.zombies.pos[i].distance(Vec2::new(100.5, 100.5)) - before[k].distance(Vec2::new(100.5, 100.5));
            assert!(pushed > 0.4, "zombie {i} pushed only {pushed} m");
            assert_eq!(w.zombies.hp[i], hp[i]);
        }
        assert!(!w.zombies.state[behind].is_reeling());
        // A staggered zombie can't strike: walk it into reach and check for a while.
        let p = w.player(id).unwrap().state.pos;
        w.zombies.pos[a] = p + Vec2::new(0.6, 0.0);
        w.zombies.state[a] = ZombieState::Staggered;
        w.zombies.timer[a] = 0.5;
        for _ in 0..8 {
            w.step();
            assert!(w.attack_events.iter().all(|e| e.zombie != w.zombies.id[a]), "a staggered zombie attacked");
        }
        // Shoves have a short cooldown.
        assert!(w.player_shove(id));
        assert!(!w.player_shove(id) || w.player(id).unwrap().queued.is_some());
    }

    #[test]
    fn knock_down_then_stomp_kills() {
        let (mut w, id) = arena(5);
        let z = zombie(&mut w, Vec2::new(101.2, 100.5), PI);
        // Stomping a standing zombie does nothing.
        w.player_stomp(id);
        assert!(until_ready(&mut w, id).is_empty());
        let mut shoves = 0;
        while w.zombies.state[z] != ZombieState::Down {
            shoves += 1;
            assert!(shoves < 40, "never knocked down");
            w.zombies.pos[z] = Vec2::new(101.1, 100.5);
            rest(&mut w, id);
            w.player_shove(id);
            until_ready(&mut w, id);
        }
        assert_eq!(w.zombies.anim[z], ZombieAnim::Down);
        // On the ground it doesn't attack or chase, and swings pass over it.
        w.zombies.timer[z] = 10.0;
        let at = w.zombies.pos[z];
        for _ in 0..20 {
            w.step();
            assert!(w.attack_events.is_empty());
        }
        assert!(w.zombies.pos[z].distance(at) < 0.05, "a downed zombie crawled");
        w.player_attack(id);
        assert!(until_ready(&mut w, id).is_empty(), "a swing hit a downed zombie");
        // Step up and stamp on it.
        w.players[0].state.pos = w.zombies.pos[z] - Vec2::new(0.7, 0.0);
        let mut stomps = 0;
        while w.zombies.state[z] != ZombieState::Dead {
            stomps += 1;
            assert!(stomps <= 5, "still alive after 5 stomps (hp {})", w.zombies.hp[z]);
            rest(&mut w, id);
            assert!(w.player_stomp(id));
            let hits = until_ready(&mut w, id);
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].kind, ActionKind::Stomp);
            if w.zombies.state[z] != ZombieState::Dead {
                assert_eq!(w.zombies.state[z], ZombieState::Down, "a stomp shouldn't stand it up");
            }
        }
        assert!(stomps >= 2, "one stomp killed a fresh zombie");
    }

    #[test]
    fn downed_zombies_get_up_and_come_back() {
        let (mut w, id) = arena(6);
        let z = zombie(&mut w, Vec2::new(101.2, 100.5), PI);
        w.zombies.state[z] = ZombieState::Down;
        w.zombies.timer[z] = 1.0;
        w.zombies.target_player[z] = id;
        for _ in 0..30 {
            w.step();
        }
        assert!(w.zombies.state[z].is_hunting(), "{:?}", w.zombies.state[z]);
    }

    #[test]
    fn endurance_drains_with_swings_and_weakens_them() {
        let (mut w, id) = arena(7);
        let bat = equip(&mut w, id, "baseball_bat");
        let def = w.config.weapons[bat as usize].clone();
        {
            let d = &mut w.config.weapons[bat as usize];
            (d.crit_chance, d.knockdown_chance, d.stagger_chance) = (0.0, 0.0, 0.0);
        }
        let z = zombie(&mut w, Vec2::new(101.5, 100.5), PI);
        w.zombies.hp[z] = 1e9;
        // Each swing costs its endurance.
        let e0 = w.player(id).unwrap().state.endurance;
        w.player_attack(id);
        let e1 = w.player(id).unwrap().state.endurance;
        assert!((e0 - e1 - def.endurance_cost).abs() < 1e-5, "{e0} -> {e1}");
        until_ready(&mut w, id);
        // Swinging and swinging wears you out.
        for _ in 0..20 {
            w.zombies.pos[z] = Vec2::new(101.5, 100.5);
            w.player_attack(id);
            until_ready(&mut w, id);
        }
        assert!(w.player(id).unwrap().state.endurance < 0.5, "{}", w.player(id).unwrap().state.endurance);

        // Rested vs exhausted: damage and swing time.
        let measure = |w: &mut World, endurance: f32| {
            let (mut dmg, mut ticks) = (0.0, 0);
            for _ in 0..8 {
                w.zombies.pos[z] = Vec2::new(101.5, 100.5);
                w.zombies.state[z] = ZombieState::Wander;
                w.players.iter_mut().find(|p| p.id == id).unwrap().state.endurance = endurance;
                w.player_attack(id);
                for _ in 0..200 {
                    w.step();
                    ticks += 1;
                    dmg += w.hit_events.iter().map(|h| h.damage).sum::<f32>();
                    let p = w.player(id).unwrap();
                    if p.action.is_none() && p.ready_in <= 0.0 {
                        break;
                    }
                }
            }
            (dmg / 8.0, ticks)
        };
        let (fresh_dmg, fresh_ticks) = measure(&mut w, 1.0);
        let (tired_dmg, tired_ticks) = measure(&mut w, 0.02);
        assert!(tired_dmg < fresh_dmg * 0.7, "tired {tired_dmg} vs fresh {fresh_dmg}");
        assert!(tired_ticks as f32 > fresh_ticks as f32 * 1.3, "tired {tired_ticks} ticks vs fresh {fresh_ticks}");
        // Swinging to exhaustion winds you: no running until you recover.
        assert!(w.player(id).unwrap().state.winded);
    }

    #[test]
    fn zombie_attacks_hurt_and_can_kill() {
        // Hit rate: one zombie alone, then four at once (more likely to land).
        let rate = |n: usize| {
            let (mut w, _) = arena(8);
            for k in 0..n {
                let a = k as f32 * PI * 2.0 / n as f32;
                zombie(&mut w, Vec2::new(100.5, 100.5) + Vec2::from_angle(a) * 0.75, a + PI);
            }
            w.players[0].hp = 1e6;
            let (mut attacks, mut landed) = (0, 0);
            for _ in 0..20 * 40 {
                w.step();
                attacks += w.attack_events.len();
                landed += w.attack_events.iter().filter(|a| a.landed).count();
                for a in w.attack_events.iter().filter(|a| a.landed) {
                    assert!(a.damage >= w.config.combat.zombie_damage.0 && a.damage <= w.config.combat.zombie_damage.1);
                }
            }
            assert!(attacks > 20, "{n} zombies: only {attacks} attacks");
            landed as f32 / attacks as f32
        };
        let (one, four) = (rate(1), rate(4));
        assert!(one > 0.3 && one < 0.7, "one zombie lands {one}");
        assert!(four > one + 0.1, "four zombies land {four}, one {one}");

        // Three on you with no fight back: dead in well under a minute.
        let (mut w, id) = arena(9);
        equip(&mut w, id, "baseball_bat");
        let bat = w.item_index("baseball_bat").unwrap();
        for k in 0..3 {
            let a = k as f32 * 2.1;
            zombie(&mut w, Vec2::new(100.5, 100.5) + Vec2::from_angle(a) * 0.75, a + PI);
        }
        let mut hp = w.player(id).unwrap().hp;
        let mut ticks = 0;
        while !w.player(id).unwrap().dead {
            w.step();
            ticks += 1;
            assert!(ticks < 20 * 60, "survived a minute");
            let now = w.player(id).unwrap().hp;
            assert!(now <= hp);
            hp = now;
        }
        assert!(ticks > 20 * 3, "died in {ticks} ticks");
        let p = w.player(id).unwrap();
        assert_eq!(p.hp, 0.0);
        assert!(p.weapon.is_none());
        assert!(w.items().iter().any(|it| it.stack.item == bat && it.pos.distance(p.state.pos) < 0.01), "the bat should drop");
        assert!(p.inventory.is_empty() && p.state.mods == crate::players::MoveMods::default());
        // Dead: input is ignored and the zombies lose interest.
        let at = p.state.pos;
        w.set_player_input(id, 1, PlayerInput { move_dir: Vec2::X, facing: 0.0, run: true, sneak: false });
        assert!(!w.player_attack(id));
        for _ in 0..40 {
            w.step();
        }
        assert_eq!(w.player(id).unwrap().state.pos, at);
        assert!(w.attack_events.is_empty());
        // Respawn: back at the start with full health.
        assert!(w.respawn_player(id));
        let p = w.player(id).unwrap();
        assert!(!p.dead && p.hp == w.config.combat.player_hp);
        assert!(!w.respawn_player(id), "only the dead respawn");
    }

    /// How often one shove floors a lone zombie (rested player, zombie not staggered), facing the
    /// player or facing away.
    fn shove_knockdown_rate(facing: f32) -> f32 {
        let (mut w, id) = arena(21);
        let z = zombie(&mut w, Vec2::new(101.3, 100.5), facing);
        let trials = 400;
        let mut downs = 0;
        for _ in 0..trials {
            (w.zombies.pos[z], w.zombies.vel[z], w.zombies.facing[z]) = (Vec2::new(101.3, 100.5), Vec2::ZERO, facing);
            (w.zombies.state[z], w.zombies.timer[z], w.zombies.anim[z]) = (ZombieState::Wander, 0.0, ZombieAnim::Idle);
            rest(&mut w, id);
            assert!(w.player_shove(id));
            let hits = until_ready(&mut w, id);
            assert_eq!(hits.len(), 1);
            downs += (hits[0].effect == HitEffect::KnockedDown) as u32;
        }
        downs as f32 / trials as f32
    }

    /// A shove knocks a lone zombie down now and then (about one in four from the front), more
    /// often from behind.
    #[test]
    fn shove_sometimes_knocks_down_a_lone_zombie() {
        let c = CombatTuning::default();
        let front = shove_knockdown_rate(PI);
        let behind = shove_knockdown_rate(0.0);
        eprintln!("shove knockdowns: {front:.2} from the front, {behind:.2} from behind (chance {}, ×{} behind)", c.shove_knockdown_chance, c.behind_knockdown_mult);
        assert!((front - c.shove_knockdown_chance).abs() < 0.07, "front {front}");
        assert!((behind - c.shove_knockdown_chance * c.behind_knockdown_mult).abs() < 0.08, "behind {behind}");
        assert!(front > 0.15 && behind > front);
    }

    #[test]
    fn corpses_dont_collide_or_act() {
        let (mut w, id) = arena(10);
        equip(&mut w, id, "fire_axe");
        let z = zombie(&mut w, Vec2::new(101.4, 100.5), PI);
        w.zombies.hp[z] = 1.0;
        w.player_attack(id);
        let hits = until_ready(&mut w, id);
        assert_eq!(hits[0].effect, HitEffect::Killed);
        for _ in 0..10 {
            w.step();
        }
        let corpse = w.zombies.pos[z];
        // A zombie standing right on it isn't pushed off, and the corpse doesn't budge (the player
        // steps away so the other one doesn't go for them).
        w.players[0].state.pos = Vec2::new(200.5, 200.5);
        let other = zombie(&mut w, corpse + Vec2::new(0.05, 0.0), 0.0);
        w.zombies.speed[other] = 0.0;
        let other_at = w.zombies.pos[other];
        for _ in 0..20 {
            w.emit_sound(Vec2::new(100.5, 100.5), 20.0);
            w.step();
            assert_eq!(w.zombies.state[z], ZombieState::Dead);
            assert_eq!(w.zombies.pos[z], corpse);
            assert!(w.attack_events.iter().all(|a| a.zombie != w.zombies.id[z]));
        }
        assert!(w.zombies.pos[other].distance(other_at) < 0.15, "pushed off a corpse: {} -> {}", other_at, w.zombies.pos[other]);
        // Swings don't hit corpses.
        w.zombies.pos[other] = Vec2::new(150.0, 150.0);
        w.players[0].state.pos = Vec2::new(100.5, 100.5);
        w.players[0].state.facing = (corpse - Vec2::new(100.5, 100.5)).to_angle();
        w.player_attack(id);
        assert!(until_ready(&mut w, id).is_empty());
    }

    #[test]
    fn weapons_lie_in_houses_and_can_be_picked_up() {
        let w = world(1, 0);
        let b = world(1, 0);
        assert_eq!(w.items(), b.items(), "placement is deterministic");
        assert!(w.items().len() >= w.map.buildings.len() / 2, "only {} weapons in {} houses", w.items().len(), w.map.buildings.len());
        assert!(w.items().iter().any(|it| w.map.building_at(it.pos) == Some(w.spawn_building)), "nothing in the spawn house");
        for it in w.items() {
            let t = it.pos.floor().as_ivec2();
            assert!(w.map.building_at(it.pos).is_some() && !w.map.tile_blocked(t), "item {} at {}", it.id, it.pos);
            assert!(w.item_weapon(it.stack.item).is_some_and(|k| k != w.unarmed()));
        }

        let (mut w, id) = arena(11);
        assert!(w.player_pick_up(id).is_none(), "nothing to pick up");
        let bat = w.spawn_item("baseball_bat", 1, Vec2::new(101.3, 100.5)).unwrap();
        let knife = w.spawn_item("kitchen_knife", 1, Vec2::new(103.5, 100.5)).unwrap();
        let axe = w.spawn_item("fire_axe", 1, Vec2::new(105.5, 100.5)).unwrap();
        assert!(w.spawn_item("fists", 1, Vec2::new(101.0, 100.5)).is_none(), "bare hands aren't an item");
        assert_eq!(w.player_pick_up(id), Some(bat));
        assert_eq!(w.wielded(w.player(id).unwrap().weapon).id, "baseball_bat");
        assert_eq!(w.items().len(), 2);
        // Out of reach.
        assert!(w.player_pick_up(id).is_none());
        // Picking up another weapon: the bat goes into your pockets (2.5 of 3 litres)...
        w.players[0].state.pos = Vec2::new(102.8, 100.5);
        assert_eq!(w.player_pick_up(id), Some(knife));
        assert_eq!(w.wielded(w.player(id).unwrap().weapon).id, "kitchen_knife");
        let bat_item = w.item_index("baseball_bat").unwrap();
        assert_eq!(w.player(id).unwrap().inventory.carried, vec![crate::items::ItemStack::one(bat_item)]);
        // ...and with no room for the knife (a mug fills the pockets up), it stays on the floor where
        // you stood.
        assert!(w.give_item(id, "mug", 1));
        w.players[0].state.pos = Vec2::new(104.8, 100.5);
        assert_eq!(w.player_pick_up(id), Some(axe));
        assert_eq!(w.wielded(w.player(id).unwrap().weapon).id, "fire_axe");
        let knife_item = w.item_index("kitchen_knife").unwrap();
        let left = w.items().iter().find(|it| it.stack.item == knife_item).unwrap();
        assert_eq!(left.pos, Vec2::new(104.8, 100.5));
    }

    #[test]
    fn fights_are_deterministic() {
        let run = |seed: u64, offset: u32| {
            let mut w = world(seed, 400);
            let p = w.player_spawn;
            let id = w.add_player(p);
            equip(&mut w, id, "baseball_bat");
            w.spawn_zombies(12, p, 3.0);
            let mut kills = 0;
            for t in 0..20 * 30u32 {
                let a = t as f32 * 0.07;
                w.set_player_input(id, t + 1, PlayerInput { move_dir: Vec2::ZERO, facing: a, run: false, sneak: false });
                match (t + offset) % 23 {
                    0 => _ = w.player_attack(id),
                    9 => _ = w.player_shove(id),
                    15 => _ = w.player_stomp(id),
                    _ => {}
                }
                w.step();
                kills += w.hit_events.iter().filter(|h| h.effect == HitEffect::Killed).count();
            }
            (w.state_hash(), kills, w.player(id).unwrap().hp.to_bits())
        };
        let a = run(3, 0);
        assert_eq!(a, run(3, 0));
        assert_ne!(a.0, run(3, 5).0);
    }

    /// Hits, kills and wounds are collected as combat events (for hit feedback) until drained,
    /// with where they happened and which way the blow went.
    #[test]
    fn combat_events_report_hits_kills_and_wounds() {
        let (mut w, id) = arena(3);
        let bat = equip(&mut w, id, "baseball_bat");
        w.config.weapons[bat as usize].knockdown_chance = 0.0;
        let z = zombie(&mut w, Vec2::new(101.6, 100.5), PI);
        let zid = w.zombies.id[z];
        let mut kinds = Vec::new();
        for _ in 0..12 {
            w.zombies.pos[z] = Vec2::new(101.6, 100.5);
            rest(&mut w, id);
            w.player_attack(id);
            until_ready(&mut w, id);
            if w.zombies.state[z] == ZombieState::Dead {
                break;
            }
        }
        for e in w.combat_events.drain(..) {
            assert_eq!((e.zombie, e.player), (zid, id));
            assert!(e.pos.distance(Vec2::new(101.6, 100.5)) < 0.5 && e.dir.dot(Vec2::X) > 0.9, "{e:?}");
            kinds.push(e.kind);
        }
        assert_eq!(kinds.last(), Some(&CombatEventKind::ZombieKilled), "{kinds:?}");
        assert!(kinds[..kinds.len() - 1].iter().all(|k| *k == CombatEventKind::ZombieHit), "{kinds:?}");

        // A zombie's strike that lands is a wound on the player, pointing away from the zombie.
        let (mut w, id) = arena(4);
        zombie(&mut w, Vec2::new(101.1, 100.5), PI);
        let mut hurt = None;
        for _ in 0..20 * 20 {
            w.step();
            if let Some(e) = w.combat_events.iter().find(|e| e.kind == CombatEventKind::PlayerHurt) {
                hurt = Some(*e);
                break;
            }
        }
        let e = hurt.expect("never hurt");
        assert_eq!(e.player, id);
        assert!(e.damage > 0.0 && e.dir.dot(-Vec2::X) > 0.9, "{e:?}");
        // Nobody drains them here: they pile up only to the cap.
        for _ in 0..20 * 60 {
            w.step();
        }
        assert!(w.combat_events.len() <= MAX_COMBAT_EVENTS);
    }

    /// A knocked-down zombie shows the get-up anim for the last moments on the ground.
    #[test]
    fn downed_zombies_get_up() {
        let (mut w, id) = arena(5);
        let z = zombie(&mut w, Vec2::new(101.2, 100.5), PI);
        let mut shoves = 0;
        while w.zombies.state[z] != ZombieState::Down {
            shoves += 1;
            assert!(shoves < 40, "never knocked down");
            w.zombies.pos[z] = Vec2::new(101.1, 100.5);
            rest(&mut w, id);
            w.player_shove(id);
            until_ready(&mut w, id);
        }
        let mut anims = vec![w.zombies.anim[z]];
        for _ in 0..20 * 6 {
            w.step();
            if anims.last() != Some(&w.zombies.anim[z]) {
                anims.push(w.zombies.anim[z]);
            }
        }
        let up = anims.iter().position(|a| *a == ZombieAnim::GetUp).expect("never got up");
        assert_eq!(anims[up - 1], ZombieAnim::Down, "{anims:?}");
        assert!(!matches!(anims[up + 1], ZombieAnim::Down | ZombieAnim::GetUp), "{anims:?}");
    }
}
