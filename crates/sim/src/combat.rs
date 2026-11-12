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
//! **Player against player** (`CombatTuning::pvp`, off in single-player): with it on, other players
//! are targets in the same list as zombies, by the same reach, arc and clear line. A swing or stomp
//! leaves a wound on a body part and takes HP (`World::strike_player`); a blow may stagger a
//! player or knock it down ([`crate::players::Knock`]: no moving or acting until it is over; only
//! a stomp hits one on the ground). Off, players are never targets.
//!
//! Zombie strikes ([`crate::ai::ZombieAttack`]) land with `zombie_hit_chance`, more likely the more
//! zombies are on the same player, and leave a wound on a body part ([`crate::health`]: where it
//! lands, what it leaves and what it takes at once). A player at 0 HP is dead: input is ignored,
//! zombies lose interest, and the weapon falls to the floor. Every number is in [`CombatTuning`],
//! [`crate::health::HealthTuning`] or the weapon data; all ours.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::TICK_DT;
use crate::ai::{self, ZOMBIE_RADIUS};
use crate::health::{Part, WoundKind};
use crate::items::{GroundItem, WeaponCategory, WeaponDef};
use crate::run::DeathCause;
use crate::players::{Action, ActionKind, DevFlags, Knock, PLAYER_RADIUS, Player, PlayerInput, RUN_MIN_ENDURANCE};
use crate::senses::{Sound, SoundSource};
use crate::skills::Skill;
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
    /// How long a stagger and a knockdown last. A knockdown's last `zombies::GET_UP_SECS` (2 s) are
    /// getting up; before that the client plays a 1.3 s fall, so the shortest leaves ~0.7 s lying still.
    pub stagger_secs: (f32, f32),
    pub down_secs: (f32, f32),

    // Shove: hands, or the weapon held across the body, to push them off and make room to swing.
    pub shove_range: f32,
    pub shove_arc_degrees: f32,
    pub shove_targets: u32,
    /// How far a shove pushes a zombie back (metres, before Strength and how tired you are; at
    /// most about 1.5 m, `ZombieTuning::knockback_slide`).
    pub shove_knockback: f32,
    /// Knockdown chance of a shove (it always staggers), before the staggered / behind multipliers.
    pub shove_knockdown_chance: f32,
    /// A shove with a melee weapon in hand (pushed with its shaft): this much more reach, and
    /// knockback times `shove_weapon_knockback`.
    pub shove_weapon_range: f32,
    pub shove_weapon_knockback: f32,
    /// Shoves on the same zombie this close together add up: each earlier one (at most
    /// `shove_repeat_max`) adds `shove_repeat_knockdown` to the knockdown chance. A knockdown
    /// starts it over.
    pub shove_repeat_secs: f32,
    pub shove_repeat_knockdown: f32,
    pub shove_repeat_max: u32,
    pub shove_windup: f32,
    pub shove_seconds: f32,
    pub shove_endurance: f32,
    pub shove_noise: f32,

    // Stomp.
    /// Reach from the player's centre to a downed zombie's centre.
    pub stomp_range: f32,
    pub stomp_arc_degrees: f32,
    pub stomp_damage: (f32, f32),
    /// A zombie on the ground lies on its back with its head this far behind its feet (its
    /// position): the line between is the body a stomp can land on.
    pub down_head_offset: f32,
    /// A stomp from within this of the head (and in the arc) is a stomp to the head: its damage
    /// times `stomp_head_mult`.
    pub stomp_head_reach: f32,
    pub stomp_head_mult: f32,
    /// A stomp that doesn't kill keeps the zombie on the ground at least this long before it
    /// starts to get up (one getting up is stamped back down).
    pub stomp_pin_secs: f32,
    pub stomp_crit_chance: f32,
    pub stomp_crit_multiplier: f32,
    pub stomp_windup: f32,
    pub stomp_seconds: f32,
    pub stomp_endurance: f32,
    pub stomp_noise: f32,

    // Zombie strikes.
    /// Chance a strike lands, plus this for every other zombie attacking the same player, at most
    /// `zombie_hit_max`. What a landed strike does is up to the wound it leaves (`HealthTuning`).
    pub zombie_hit_chance: f32,
    pub zombie_crowd_bonus: f32,
    pub zombie_hit_max: f32,

    // Players and each other (PLAN §4.8).
    /// Swings, shoves and stomps hit other players as they hit zombies. Off in single-player and
    /// co-op (players are never targets); a multiplayer server turns it on unless started with
    /// `--pvp off`.
    pub pvp: bool,
    /// A swing's or stomp's damage to a player is its damage to a zombie times this (the player
    /// also gets the wound).
    pub pvp_damage_mult: f32,
    /// A bullet's damage to a player is its damage to a zombie times this (#72).
    pub pvp_shot_mult: f32,
    /// Chance a blunt weapon's blow splits the skin (a gash, not a bruise); a crit always does.
    pub pvp_blunt_gash: f32,
    /// A cut that clothing stops is a bruise, and takes this much of its damage.
    pub pvp_stopped_damage: f32,
    /// How long a player reels from a blow, and how long one stays on the ground.
    pub player_stagger_secs: (f32, f32),
    pub player_down_secs: (f32, f32),

    // Lag compensation (#23, `crate::lagcomp`).
    /// A blow of a player whose client reports what it shows (one over the network) is tested
    /// against its targets where they stood at most this long ago; 0 turns it off. It is also the
    /// most by which a blow about to land survives its player being floored (a trade of blows).
    /// 0.35 s covers a 150 ms ping with the client's own tick and snapshot cushion on top.
    pub lag_comp_max_secs: f32,

    // Weapons in the hand (wear: `crate::wear`).
    /// A blow at a target nearer than the weapon's `close_range` does this much of its damage.
    pub cramped_damage: f32,
    /// A two-handed weapon swung in one hand (its item's `one_hand`, the other hand full) hits
    /// this much as hard, and takes `one_hand_slow` times as long.
    pub one_hand_power: f32,
    pub one_hand_slow: f32,
    /// With this share of its life or less left a weapon is battered, and does `battered_damage`
    /// of its damage.
    pub battered_below: f32,
    pub battered_damage: f32,
    /// A broken weapon (every knock taken: `crate::wear`) stays in the hand as an item, but its
    /// swing does this much of its damage and knockback, and never crits or knocks down.
    pub broken_damage: f32,
    /// Each level of the weapon's skill makes a knock this much less likely.
    pub wear_skill_less: f32,
    /// A weapon found in furniture has had at most this share of its life (0: every one is sound).
    pub found_wear_max: f32,
}

/// A blow that lands though its player was just staggered, floored or killed by another player's:
/// on its own client it had already landed (see `World::strike_player`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Trade {
    pi: usize,
    action: Action,
    /// Where it stood and faced before the other's blow moved it.
    origin: Vec2,
    facing: f32,
    /// Ticks until it would have landed.
    ahead: u64,
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
            down_secs: (4.0, 6.0),

            shove_range: 0.75,
            shove_arc_degrees: 150.0,
            shove_targets: 3,
            shove_knockback: 1.2,
            shove_knockdown_chance: 0.25,
            shove_weapon_range: 0.2,
            shove_weapon_knockback: 1.2,
            shove_repeat_secs: 3.0,
            shove_repeat_knockdown: 0.15,
            shove_repeat_max: 3,
            shove_windup: 0.1,
            shove_seconds: 0.6,
            shove_endurance: 0.035,
            shove_noise: 2.5,

            stomp_range: 1.1,
            stomp_arc_degrees: 140.0,
            // Two or three to the body, one to the head.
            stomp_damage: (36.0, 48.0),
            down_head_offset: 1.45,
            stomp_head_reach: 0.9,
            stomp_head_mult: 4.0,
            stomp_pin_secs: 1.5,
            stomp_crit_chance: 0.15,
            // Short of a fresh zombie's HP even then: only the head is one stamp.
            stomp_crit_multiplier: 1.6,
            stomp_windup: 0.15,
            stomp_seconds: 0.55,
            stomp_endurance: 0.04,
            stomp_noise: 4.0,

            zombie_hit_chance: 0.5,
            zombie_crowd_bonus: 0.12,
            zombie_hit_max: 0.9,

            // A bat takes about eight blows to kill, an axe four: long enough to run or give up.
            pvp: false,
            pvp_damage_mult: 0.4,
            // A pistol takes three or four shots to the body, one or two to the head.
            pvp_shot_mult: 0.7,
            pvp_blunt_gash: 0.3,
            pvp_stopped_damage: 0.5,
            player_stagger_secs: (0.5, 0.8),
            // Short next to a zombie's 4-6 s: time for two stomps, not five.
            player_down_secs: (1.4, 2.0),

            lag_comp_max_secs: 0.35,

            cramped_damage: 0.6,
            one_hand_power: 0.75,
            one_hand_slow: 1.2,
            battered_below: 0.25,
            battered_damage: 0.85,
            broken_damage: 0.15,
            wear_skill_less: 0.04,
            found_wear_max: 0.7,
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

/// A player's action struck another player this tick (PvP).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerHitEvent {
    /// Who struck, and who was hit.
    pub player: u32,
    pub victim: u32,
    pub kind: ActionKind,
    pub weapon: u16,
    /// What it would take off (a god loses nothing).
    pub damage: f32,
    pub crit: bool,
    pub behind: bool,
    /// Where it landed and the wound it left (none for a shove).
    pub part: Option<Part>,
    pub wound: Option<WoundKind>,
    pub effect: HitEffect,
}

/// What a [`CombatEvent`] was, for hit feedback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CombatEventKind {
    /// A swing hurt a zombie (it may also have staggered or floored it).
    ZombieHit,
    /// A swing or stomp killed a zombie.
    ZombieKilled,
    /// A zombie's strike landed on a player, or (`by` set) another player's swing or stomp.
    PlayerHurt,
    /// A shove pushed a zombie back (no damage).
    Shove,
    /// A stomp hurt a downed zombie without killing it.
    Stomp,
    /// A player's shove pushed another player back (no damage): `player` the one shoved, `by` who did it.
    PlayerShoved,
    /// A bullet or pellet flew (#72, `crate::ranged`), whether or not it hit anybody (what it hit
    /// has its own event): `player` shot it, `pos` is where it stopped, `dir` the way it flew
    /// over the ground and `damage` how far (metres), so the line starts at `pos - dir * damage`.
    Shot,
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
    /// `PlayerHurt`: the body part hit and the wound it left.
    pub part: Option<Part>,
    pub wound: Option<WoundKind>,
    /// `PlayerHurt`, `PlayerShoved`: the player who did it (PvP); 0 when it wasn't a player.
    pub by: u32,
}

/// Bit set on a candidate's index in `World::scratch_hits` when it is a player, not a zombie.
pub(crate) const PLAYER_TARGET: usize = 1 << (usize::BITS - 1);

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
#[derive(Clone, Copy)]
pub(crate) struct Strike {
    /// Reach to the zombie's body edge (centre for stomps); a shot's range.
    pub(crate) range: f32,
    /// Half the arc, in radians.
    pub(crate) half_arc: f32,
    pub(crate) max_targets: u32,
    pub(crate) damage: (f32, f32),
    pub(crate) crit_chance: f32,
    pub(crate) crit_multiplier: f32,
    pub(crate) knockback: f32,
    pub(crate) stagger_chance: f32,
    pub(crate) knockdown_chance: f32,
    pub(crate) noise: f32,
    /// A bullet's hit (#72, `crate::ranged`), and how high up the body it struck: a player is
    /// wounded on the part there, not one from the tables.
    pub(crate) shot: Option<f32>,
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
        if self.players[pi].dead || self.players[pi].needs.asleep() || self.players[pi].knock.is_some() || self.players[pi].away.is_some() || self.players[pi].restraint.is_some() {
            return false;
        }
        // The trigger pulled on an empty gun (#72): it is reloaded instead, if there is anything
        // to reload it with (the refusal says what's missing), and a reload under way carries on.
        // Not a gun from upstairs yet (`crate::stairs`): a shot's flight is over the ground floor.
        if kind == ActionKind::Swing && self.players[pi].state.height(&self.map) > self.config.stairs.out_of_reach && self.players[pi].weapon.is_some_and(|w| self.config.weapons[w as usize].ranged.is_some()) {
            self.note(id, false, "no clear shot from up here".to_string());
            return false;
        }
        if kind == ActionKind::Swing && self.gun_is_empty(pi) {
            let _ = self.player_reload(id);
            return false;
        }
        // Fighting drops whatever you were doing with your things, and with anyone else's (#73).
        self.cancel_jobs(pi);
        self.players[pi].hands = None;
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
        // Wounded arms and hands swing weaker and slower.
        // So do hunger, tiredness and pain (#11), and swinging makes you thirsty.
        let (arm_power, arm_time) = if kind == ActionKind::Swing {
            let e = self.health_effects(pi);
            let (power, time) = self.needs_swing(pi);
            (e.swing_power * power, e.swing_time * time)
        } else {
            (1.0, 1.0)
        };
        // A practised hand swings cheaper and quicker (Bludgeon, Edged; #46), and swinging a heavy
        // weapon builds Strength.
        // (No skill makes shooting better yet, #72.)
        let (skill_cost, skill_time) = if kind == ActionKind::Swing && self.config.weapons[weapon as usize].ranged.is_none() {
            let skill = self.config.weapons[weapon as usize].skill();
            let level = self.skill_level(pi, skill);
            let heavy = self.players[pi].inventory.hand.is_some_and(|s| self.item_def(s.item).weight_kg > self.config.skills.heavy_weapon_kg);
            if heavy {
                self.award_xp(pi, Skill::Strength, self.config.skills.heavy_swing_xp);
            }
            (self.config.skills.cost_mult(skill, level), self.config.skills.time_mult(skill, level))
        } else {
            (1.0, 1.0)
        };
        let cost = cost * skill_cost;
        let c = &self.config.combat;
        // A two-handed weapon swung in one hand, the other being full: weaker and slower.
        let inv = &self.players[pi].inventory;
        let one_handed = kind == ActionKind::Swing && inv.off_hand.is_some() && inv.hand.is_some_and(|s| self.item_def(s.item).two_handed);
        let (grip_power, grip_time) = if one_handed { (c.one_hand_power, c.one_hand_slow) } else { (1.0, 1.0) };
        let p = &mut self.players[pi];
        let (power, slow) = c.fatigue(p.state.endurance);
        let (power, slow) = (power * arm_power * grip_power, slow * arm_time * skill_time * grip_time);
        p.state.endurance = (p.state.endurance - cost).max(0.0);
        if p.state.endurance <= RUN_MIN_ENDURANCE {
            p.state.winded = true;
        }
        // Attacking ends spawn protection (#68).
        p.protected = 0.0;
        p.action = Some(Action { kind, timer: windup * slow, power, weapon });
        p.ready_in = total * slow;
        p.queued = None;
    }

    /// Single-player / debug: a dead player gets up again at the spawn point with full health and
    /// empty hands, a new survivor (its skills start over). Returns whether it respawned.
    pub fn respawn_player(&mut self, id: u32) -> bool {
        let spawn = self.map.resolve_circle(self.player_spawn, PLAYER_RADIUS);
        let hp = self.config.combat.player_hp;
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return false };
        if !self.players[pi].dead {
            return false;
        }
        // An infected body left behind gets up now.
        self.rise(pi);
        // A new survivor: its skills start over (#46).
        // The same person again (`crate::origin`): who it was stays, with the skills it began with.
        let origin = self.players[pi].origin.clone();
        let skills = origin.start_skills(&self.config.skills);
        let p = &mut self.players[pi];
        // (A new `Player` stands on the ground floor.)
        *p = Player { last_input_seq: p.last_input_seq, input: PlayerInput::default(), dev: p.dev, skills, origin, seen_tick: p.seen_tick, ..Player::new(id, spawn, hp) };
        self.refresh_carry(pi);
        self.begin_first_night(pi);
        self.script_event(crate::script::ScriptEvent::Spawned { player: id });
        true
    }

    /// Count down action timers and land the actions due this tick. Needs the spatial hash
    /// current (rebuilt after players move).
    pub(crate) fn step_player_actions(&mut self) {
        let mut due = std::mem::take(&mut self.scratch_actions);
        due.clear();
        // Players reeling or down from another's blow (PvP) recover. This runs after movement, so
        // the modifiers a snapshot carries are the ones the next tick moves with.
        for pi in 0..self.players.len() {
            let p = &mut self.players[pi];
            if let Some(k) = &mut p.knock {
                k.timer -= TICK_DT;
                if k.timer <= 0.0 || p.dead {
                    p.knock = None;
                    self.refresh_carry(pi);
                }
            }
        }
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
                && p.knock.is_none()
                && let Some(kind) = p.queued.take()
            {
                self.begin_action(pi, kind);
            }
        }
        for &(pi, a) in &due {
            let p = &self.players[pi].state;
            self.land(pi, a, p.pos, p.facing, 0);
        }
        self.scratch_actions = due;
        // Trades of blows (see `strike_player`): blows that had landed on their own client before
        // the one that floored their player showed there. One may floor someone else in turn.
        let mut k = 0;
        while k < self.scratch_trades.len() {
            let t = self.scratch_trades[k];
            self.land(t.pi, t.action, t.origin, t.facing, t.ahead);
            k += 1;
        }
        self.scratch_trades.clear();
    }

    /// An action lands, swung from `origin` facing `facing`: find its targets and hit them.
    ///
    /// **Lag compensation (#23):** for a player whose client reports what it shows, the targets
    /// are tested (reach, arc, the clear line, which is nearest, the way the blow goes) where they
    /// stood in the snapshot it was showing as it sent the input this tick applied, `ahead` ticks
    /// on, at most `lag_comp_max_secs` back (`World::rewind_tick`). Its own place is the server's:
    /// its client predicts that. Who can be hit goes by now (a zombie dead or down since isn't
    /// swung at; the map is as it is, so a door shut since stops the blow), and what the blow
    /// does happens to the target where it is now.
    fn land(&mut self, pi: usize, a: Action, origin: Vec2, facing: f32, ahead: u64) {
        // A gun shoots (#72).
        if a.kind == ActionKind::Swing && self.config.weapons[a.weapon as usize].ranged.is_some() {
            return self.fire(pi, a, origin, facing, ahead);
        }
        let c = &self.config.combat;
        // Skills (#46): Strength knocks back farther and shoves down more often, so does Brawl;
        // the weapon's skill (Brawl for stomps) hits harder.
        let sk = &self.config.skills;
        let level = |s: Skill| self.players[pi].skills.level(s, sk);
        let knock = sk.knockback_mult(level(Skill::Strength));
        // Shoving with a melee weapon in hand: pushed with the shaft, it reaches and pushes further.
        let armed = a.weapon != self.unarmed && self.config.weapons[a.weapon as usize].ranged.is_none();
        let s = match a.kind {
            ActionKind::Swing => {
                let w = &self.config.weapons[a.weapon as usize];
                // A battered weapon hits weaker, a broken one hardly at all (`crate::wear`).
                let dm = sk.damage_mult(w.skill(), level(w.skill())) * self.wear_damage_mult(pi);
                let broken = self.hand_condition(pi) == Some(crate::wear::Condition::Broken);
                let bk = if broken { c.broken_damage } else { 1.0 };
                Strike {
                    range: w.range,
                    half_arc: (w.arc_degrees * 0.5).to_radians(),
                    max_targets: w.max_targets,
                    damage: (w.damage.0 * dm, w.damage.1 * dm),
                    crit_chance: if broken { 0.0 } else { w.crit_chance },
                    crit_multiplier: w.crit_multiplier,
                    knockback: w.knockback * knock * bk,
                    stagger_chance: w.stagger_chance * bk,
                    knockdown_chance: if broken { 0.0 } else { w.knockdown_chance },
                    noise: w.noise_radius,
                    shot: None,
                }
            }
            ActionKind::Shove => Strike {
                range: c.shove_range + if armed { c.shove_weapon_range } else { 0.0 },
                half_arc: (c.shove_arc_degrees * 0.5).to_radians(),
                max_targets: c.shove_targets,
                damage: (0.0, 0.0),
                crit_chance: 0.0,
                crit_multiplier: 1.0,
                knockback: c.shove_knockback * knock * if armed { c.shove_weapon_knockback } else { 1.0 },
                stagger_chance: 1.0,
                knockdown_chance: (c.shove_knockdown_chance + sk.shove_knockdown_bonus(level(Skill::Strength), level(Skill::Brawl))).min(1.0),
                noise: c.shove_noise,
                shot: None,
            },
            ActionKind::Stomp => Strike {
                range: c.stomp_range,
                half_arc: (c.stomp_arc_degrees * 0.5).to_radians(),
                max_targets: 1,
                damage: {
                    let dm = sk.damage_mult(Skill::Brawl, level(Skill::Brawl));
                    (c.stomp_damage.0 * dm, c.stomp_damage.1 * dm)
                },
                crit_chance: c.stomp_crit_chance,
                crit_multiplier: c.stomp_crit_multiplier,
                knockback: 0.0,
                stagger_chance: 0.0,
                knockdown_chance: 0.0,
                noise: c.stomp_noise,
                shot: None,
            },
        };
        let player = self.players[pi].id;
        let forward = Vec2::from_angle(facing);
        let stomp = a.kind == ActionKind::Stomp;

        // Where the attacker's client showed everybody (none: where they are).
        let history = std::mem::take(&mut self.lag_history);
        let frame = self.rewind_tick(pi, ahead).and_then(|t| history.frame(t));
        // They may have moved this far since.
        let moved = frame.map_or(0.0, |f| self.tick.saturating_sub(f.tick) as f32 * TICK_DT * crate::lagcomp::MAX_BODY_SPEED);

        // Candidates in reach and arc, nearest first (ties by index, so the order is stable).
        let mut found = std::mem::take(&mut self.scratch_hits);
        found.clear();
        let z = &self.zombies;
        let reach = s.range + if stomp { 0.0 } else { ZOMBIE_RADIUS };
        // A body on the ground reaches back to its head.
        let lying = if stomp { c.down_head_offset.max(0.0) } else { 0.0 };
        // From upstairs, or high on a flight of stairs, a blow reaches nothing on the ground
        // (`crate::stairs`): zombies are only ever there.
        let (my_level, my_height) = (self.players[pi].state.level, self.players[pi].state.height(&self.map));
        let out_of_reach = self.config.stairs.out_of_reach;
        let above = my_height > out_of_reach;
        self.hash.for_each_within(origin, reach + lying + moved, |j| {
            if above {
                return;
            }
            // Swings and shoves pass over downed zombies; stomps only hit them.
            let state = z.state[j];
            if state == ZombieState::Dead || (state == ZombieState::Down) != stomp {
                return;
            }
            let feet = frame.and_then(|f| f.zombie(z.id[j])).unwrap_or(z.pos[j]);
            if stomp {
                // Anywhere along the body, the head first (`stomp_spot`).
                if let Some(spot) = stomp_spot(c, origin, facing, feet, z.facing[j]) {
                    found.push((spot.dist, j));
                }
                return;
            }
            let to = feet - origin;
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
        // With PvP on, other players are targets in the same list, by the same rule: swings and
        // shoves pass over one on the ground, stomps only hit those.
        if self.config.combat.pvp {
            let reach = s.range + if stomp { 0.0 } else { PLAYER_RADIUS };
            for (k, o) in self.players.iter().enumerate() {
                // A fresh survivor's spawn protection (#68): players' blows pass it by.
                if k == pi || o.dead || o.protected > 0.0 || o.knock.is_some_and(|k| k.down) != stomp {
                    continue;
                }
                // Only someone on the same floor, at about the same height.
                if o.state.level != my_level || (o.state.height(&self.map) - my_height).abs() > out_of_reach {
                    continue;
                }
                let to = frame.and_then(|f| f.player(o.id)).unwrap_or(o.state.pos) - origin;
                let d = to.length();
                if d > reach {
                    continue;
                }
                if d > 1e-4 {
                    let cos = forward.dot(to / d);
                    let half_width = if stomp { 0.0 } else { (PLAYER_RADIUS / d).min(1.0).asin() };
                    if cos.clamp(-1.0, 1.0).acos() > s.half_arc + half_width {
                        continue;
                    }
                }
                found.push((d, PLAYER_TARGET | k));
            }
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

        // Nearer than this to the target's body there's no room for a full swing.
        let close = if a.kind == ActionKind::Swing { self.config.weapons[a.weapon as usize].close_range } else { 0.0 };
        let mut hits = 0;
        for &(d, j) in &found {
            if hits >= s.max_targets {
                break;
            }
            let victim = (j & PLAYER_TARGET != 0).then_some(j & !PLAYER_TARGET);
            let mut at = match victim {
                Some(k) => frame.and_then(|f| f.player(self.players[k].id)).unwrap_or(self.players[k].state.pos),
                None => frame.and_then(|f| f.zombie(self.zombies.id[j])).unwrap_or(self.zombies.pos[j]),
            };
            // A stomp lands where on the body it reaches; on the head, it's decisive.
            let mut struck = s;
            if stomp && victim.is_none() {
                let c = &self.config.combat;
                if let Some(spot) = stomp_spot(c, origin, facing, at, self.zombies.facing[j]) {
                    at = spot.at;
                    if spot.head {
                        // Shown as a crit (the heavy hit feedback).
                        struck.crit_chance = 1.0;
                        struck.crit_multiplier = c.stomp_head_mult;
                    }
                }
            }
            if if my_level > 0 { !self.map.upper_clear(origin, at) } else { !clear_path(&self.map, origin, at) } {
                continue;
            }
            let cramped = d - if victim.is_some() { PLAYER_RADIUS } else { ZOMBIE_RADIUS } < close;
            let falloff = self.config.combat.multi_hit_falloff.powi(hits as i32) * if cramped { self.config.combat.cramped_damage } else { 1.0 };
            let effect = match victim {
                Some(k) => self.strike_player(pi, a, &s, origin, forward, k, at, falloff),
                None => self.strike(player, a, &struck, origin, forward, j, at, falloff),
            };
            hits += 1;
            // Practice (#46): a hit with a weapon or fist, a kill instead, a zombie shoved, a stomp that kills.
            let sk = &self.config.skills;
            let killed = effect == HitEffect::Killed;
            let (skill, xp) = match a.kind {
                ActionKind::Swing => (self.config.weapons[a.weapon as usize].skill(), if killed { sk.kill_xp } else { sk.hit_xp }),
                ActionKind::Shove => (Skill::Brawl, sk.shove_xp),
                ActionKind::Stomp => (Skill::Brawl, if killed { sk.stomp_kill_xp } else { 0.0 }),
            };
            self.award_xp(pi, skill, xp);
        }
        self.scratch_hits = found;
        self.lag_history = history;
        self.wear_weapon(pi, a, hits);
        self.action_events.push(ActionEvent { player, kind: a.kind, weapon: a.weapon, hits });
        if s.noise > 0.0 && self.heard.len() < crate::world::MAX_SOUNDS {
            // Heard this tick, like footsteps.
            self.heard.push(Sound::new(origin, s.noise, SoundSource::Player(player)));
        }
    }

    /// Hit zombie `j` from `origin`, the zombie taken to stand at `at` (where the attacker saw it).
    /// Returns what it did.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn strike(&mut self, player: u32, a: Action, s: &Strike, origin: Vec2, forward: Vec2, j: usize, at: Vec2, falloff: f32) -> HitEffect {
        let c = &self.config.combat;
        let zt = &self.config.zombie;
        let rng = &mut self.combat_rng;
        let z = &mut self.zombies;
        let to = at - origin;
        let dir = to.normalize_or(forward);
        let behind = Vec2::from_angle(z.facing[j]).dot(dir) > c.behind_dot;
        let crit_chance = s.crit_chance + if behind && s.crit_chance > 0.0 { c.behind_crit_bonus } else { 0.0 };
        let crit = rng.f32() < crit_chance;
        let mut damage = if s.damage.1 > 0.0 { rng.range(s.damage.0, s.damage.1) * a.power * falloff } else { 0.0 };
        if crit {
            damage *= s.crit_multiplier;
        }
        z.hp[j] -= damage;
        // Shoves on it in the last few seconds (none for anything else): each makes this one likelier to floor it.
        let earlier = if a.kind == ActionKind::Shove { shoves_before(&self.shoved, z.id[j], self.tick, c) } else { 0 };
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
            // Stomped but not dead: stays down, and one getting up is stamped back down.
            if a.kind == ActionKind::Stomp {
                z.timer[j] = z.timer[j].max(crate::zombies::GET_UP_SECS + c.stomp_pin_secs);
                z.anim[j] = ZombieAnim::Down;
            }
            HitEffect::Hit
        } else {
            let mut knockdown = (s.knockdown_chance + earlier as f32 * c.shove_repeat_knockdown) * a.power;
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
        // Knocked back: reeling or down it slides on further (`ZombieTuning::knockback_slide`),
        // else it is soon walking again.
        if s.knockback > 0.0 {
            let slide = if z.state[j].is_reeling() { zt.knockback_slide } else { 0.75 };
            z.vel[j] = dir * slide_speed(s.knockback * a.power, slide);
        }
        if a.kind == ActionKind::Shove {
            let (id, tick) = (z.id[j], self.tick);
            remember_shove(&mut self.shoved, id, tick, c, effect == HitEffect::KnockedDown);
        }
        let (zid, zpos) = (z.id[j], z.pos[j]);
        self.hit_events.push(HitEvent { player, zombie: zid, kind: a.kind, damage, crit, behind, effect });
        if effect == HitEffect::Killed {
            self.count_kill(player);
        }
        let kind = match (effect, a.kind) {
            (HitEffect::Killed, _) => CombatEventKind::ZombieKilled,
            (_, ActionKind::Swing) => CombatEventKind::ZombieHit,
            (_, ActionKind::Shove) => CombatEventKind::Shove,
            (_, ActionKind::Stomp) => CombatEventKind::Stomp,
        };
        self.push_combat_event(CombatEvent { kind, pos: zpos, dir, zombie: zid, player, damage, crit, part: None, wound: None, by: 0 });
        effect
    }

    /// PvP: player `pi`'s action hits player `vi` from `origin`. A swing or stomp leaves a wound on
    /// a body part (a blade gashes, a blunt weapon bruises or now and then gashes, fists and boots
    /// bruise; clothing may stop a cut to a bruise) and takes HP through `hurt_player_by`, so god
    /// mode holds. Then, as for a zombie: knockback, a knockdown roll, else a stagger roll.
    /// Returns what it did. `at` is where the attacker saw the victim (lag compensation).
    ///
    /// **A trade of blows (#23):** a blow that staggers, floors or kills a player calls off the
    /// swing that player had under way, unless that swing was about to land within the player's
    /// own view lag (`World::view_lag_ticks`, so at most `lag_comp_max_secs`): on its own screen it
    /// had landed before this blow showed there, so it lands too, now, from where the player stood.
    /// Both sides get the same rule and the same cap; a local player (no lag) keeps none.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn strike_player(&mut self, pi: usize, a: Action, s: &Strike, origin: Vec2, forward: Vec2, vi: usize, at: Vec2, falloff: f32) -> HitEffect {
        let c = self.config.combat.clone();
        let attacker = self.players[pi].id;
        let v = &self.players[vi];
        let (victim, vpos, god) = (v.id, v.state.pos, v.dev.god);
        let (pending, vfacing) = (v.action, v.state.facing);
        let was = v.knock;
        let dir = (at - origin).normalize_or(forward);
        let behind = Vec2::from_angle(v.state.facing).dot(dir) > c.behind_dot;
        let crit_chance = s.crit_chance + if behind && s.crit_chance > 0.0 { c.behind_crit_bonus } else { 0.0 };
        let crit = self.combat_rng.f32() < crit_chance;
        let pvp_mult = if s.shot.is_some() { c.pvp_shot_mult } else { c.pvp_damage_mult };
        let mut damage = if s.damage.1 > 0.0 { self.combat_rng.range(s.damage.0, s.damage.1) * a.power * falloff * pvp_mult } else { 0.0 };
        if crit {
            damage *= s.crit_multiplier;
        }
        let stomp = a.kind == ActionKind::Stomp;
        let mut hurt = None;
        let mut died = false;
        if s.damage.1 > 0.0 {
            // Where it lands (a blow at the back finds more head and torso) and what it leaves.
            let t = &self.config.health;
            // A bullet wounds the part at the height it struck (#72).
            let part = match s.shot {
                Some(height) => self.shot_part(height),
                None => if behind && !stomp { &t.behind } else { &t.front }.pick(self.combat_rng.f32(), false),
            };
            let mut kind = match self.config.weapons[a.weapon as usize].category {
                _ if stomp => WoundKind::Bruise,
                WeaponCategory::Unarmed => WoundKind::Bruise,
                // A bullet wound is a gash for now (it bleeds until dressed, and wants stitching).
                WeaponCategory::ShortBlade | WeaponCategory::Axe | WeaponCategory::Firearm => WoundKind::Gash,
                WeaponCategory::ShortBlunt | WeaponCategory::LongBlunt => {
                    if self.combat_rng.f32() < c.pvp_blunt_gash || crit {
                        WoundKind::Gash
                    } else {
                        WoundKind::Bruise
                    }
                }
            };
            // Clothing may stop a blade, never a bullet.
            if kind != WoundKind::Bruise && s.shot.is_none() {
                let stop = self.protection(&self.players[vi], part.region()) * self.config.health.wound(kind).protection;
                if self.combat_rng.f32() < stop {
                    kind = WoundKind::Bruise;
                    damage *= c.pvp_stopped_damage;
                }
            }
            hurt = Some((part, kind));
            self.push_combat_event(CombatEvent { kind: CombatEventKind::PlayerHurt, pos: vpos, dir, zombie: 0, player: victim, damage, crit, part: Some(part), wound: Some(kind), by: attacker });
            if !god {
                self.add_wound(vi, part, kind, false, None);
            }
            self.interrupt_treatment(vi);
            died = self.hurt_player_by(vi, damage, DeathCause::Survivor { by: attacker, weapon: a.weapon, stomp, part, wound: kind });
        } else {
            self.push_combat_event(CombatEvent { kind: CombatEventKind::PlayerShoved, pos: vpos, dir, zombie: 0, player: victim, damage: 0.0, crit: false, part: None, wound: None, by: attacker });
        }
        let effect = if died {
            self.count_player_kill(attacker);
            HitEffect::Killed
        } else {
            // A blow wakes a sleeper.
            self.wake(vi, crate::needs::WakeReason::Hit);
            if was.is_some_and(|k| k.down) {
                // Stomped but not dead: stays down.
                HitEffect::Hit
            } else {
                if s.knockback > 0.0 {
                    self.push_player(vi, dir, (s.knockback * a.power).min(1.0));
                }
                let mut knockdown = s.knockdown_chance * a.power;
                if behind {
                    knockdown *= c.behind_knockdown_mult;
                }
                if was.is_some() {
                    knockdown *= c.staggered_knockdown_mult;
                }
                let rng = &mut self.combat_rng;
                let knock = if rng.f32() < knockdown {
                    Some(Knock { down: true, timer: rng.range(c.player_down_secs.0, c.player_down_secs.1) })
                } else if rng.f32() < s.stagger_chance {
                    // Hit again while reeling: whichever lasts longer.
                    Some(Knock { down: false, timer: rng.range(c.player_stagger_secs.0, c.player_stagger_secs.1).max(was.map_or(0.0, |k| k.timer)) })
                } else {
                    None
                };
                match knock {
                    Some(k) => {
                        // Whatever it was doing is over, and it stands (or lies) still: this is
                        // after movement, so the snapshot carries what the next tick moves with.
                        let p = &mut self.players[vi];
                        p.knock = Some(k);
                        p.action = None;
                        p.queued = None;
                        self.cancel_jobs(vi);
                        self.refresh_carry(vi);
                        if k.down { HitEffect::KnockedDown } else { HitEffect::Staggered }
                    }
                    None => HitEffect::Hit,
                }
            }
        };
        if matches!(effect, HitEffect::Killed | HitEffect::Staggered | HitEffect::KnockedDown)
            && let Some(b) = pending
            && b.timer <= self.view_lag_ticks(vi) as f32 * TICK_DT + 1e-4
        {
            self.scratch_trades.push(Trade { pi: vi, action: b, origin: vpos, facing: vfacing, ahead: (b.timer / TICK_DT).ceil() as u64 });
        }
        self.player_hit_events.push(PlayerHitEvent {
            player: attacker,
            victim,
            kind: a.kind,
            weapon: a.weapon,
            damage,
            crit,
            behind,
            part: hurt.map(|h| h.0),
            wound: hurt.map(|h| h.1),
            effect,
        });
        effect
    }

    /// Knock player `pi` back `metres` along `dir`, in steps short enough that a wall stops it;
    /// never into a window opening (that is a climb).
    fn push_player(&mut self, pi: usize, dir: Vec2, metres: f32) {
        const STEP: f32 = 0.1;
        let mut p = self.players[pi].state.pos;
        let level = self.players[pi].state.level;
        // Never onto or off a flight of stairs either: a floor is changed on one's feet.
        let stair = self.map.stair_at(p).is_some();
        let in_window = level == 0 && !stair && self.map.in_window_opening(p);
        let mut left = metres;
        while left > 1e-4 {
            let next = self.map.resolve_walker(p + dir * left.min(STEP), PLAYER_RADIUS, level);
            if (level == 0 && !stair && !in_window && self.map.in_window_opening(next)) || self.map.stair_at(next).is_some() != stair || (stair && self.map.stair_at(next).is_some_and(|s| s.progress(next) >= 1.0)) {
                break;
            }
            p = next;
            left -= STEP;
        }
        self.players[pi].state.pos = p;
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
            // Where it lands and what it leaves: from behind (the player faces the way the blow
            // goes), with others on them too, or from a zombie on the ground.
            let (from, low) = self.zombies.index_of(a.zombie).map_or((pos, false), |j| (self.zombies.pos[j], self.zombies.state[j] == ZombieState::Down));
            let dir = (pos - from).normalize_or(Vec2::X);
            let behind = Vec2::from_angle(self.players[pi].state.facing).dot(dir) > self.config.health.behind_dot;
            let (part, wound, damage) = self.roll_zombie_wound(pi, behind, on >= 2, low);
            let e = &mut self.attack_events[k];
            (e.landed, e.damage, e.part, e.wound) = (true, damage, Some(part), Some(wound));
            self.wound_player(pi, part, wound, damage, from, a.zombie);
        }
    }

    /// Take `damage` off player `pi`'s health, killing it at 0: the one place a player loses HP,
    /// so god mode (`DevFlags::god`: nothing comes off) holds for anything that hurts. Returns
    /// whether it died. The death is recorded as [`DeathCause::Other`]; say what hurt with
    /// [`World::hurt_player_by`] (`crate::run`).
    #[allow(dead_code)] // every caller in the tree names its cause now; kept for those that can't
    pub(crate) fn hurt_player(&mut self, pi: usize, damage: f32) -> bool {
        self.hurt_player_by(pi, damage, DeathCause::Other)
    }

    /// Dev toggles for player `id` (`None` leaves one as it is); returns the flags now, or `None`
    /// if there's no such player. Turning `ignored` on makes every zombie after that player (chasing,
    /// lunging, striking, battering its way to them) lose them at once: it goes to look where it
    /// last saw them, then wanders as usual. Off, they can be seen and heard again from the next tick.
    pub fn set_dev(&mut self, id: u32, god: Option<bool>, ignored: Option<bool>) -> Option<DevFlags> {
        let p = self.players.iter_mut().find(|p| p.id == id)?;
        p.dev.god = god.unwrap_or(p.dev.god);
        p.dev.ignored = ignored.unwrap_or(p.dev.ignored);
        let dev = p.dev;
        if dev.ignored {
            let (z, t) = (&mut self.zombies, &self.config.zombie);
            for i in 0..z.len() {
                if z.target_player[i] == id && z.state[i].is_after(id) {
                    ai::start_investigating(z, i, t);
                    z.sees[i] = 0;
                }
            }
        }
        Some(dev)
    }

    /// Dead at once (see [`World::hurt_player`] for damage, which god mode stops).
    pub(crate) fn kill_player(&mut self, pi: usize) {
        let p = &mut self.players[pi];
        p.hp = 0.0;
        p.dead = true;
        p.action = None;
        p.queued = None;
        p.knock = None;
        p.input.move_dir = Vec2::ZERO;
        let id = p.id;
        self.script_event(crate::script::ScriptEvent::Died { player: id });
        // An infected body rises later.
        self.health_on_death(pi);
        // Everything it had falls where it lay.
        self.drop_everything(pi);
    }
}

/// Where a stomp from `origin`, facing `facing`, lands on a zombie lying on the ground with its
/// feet at `feet` (its position) facing `zombie_facing`: on its back, its head
/// `down_head_offset` behind its feet. The head when it is within `stomp_head_reach` and in the
/// stomp's arc; else the nearest point of the body if that is within `stomp_range` and in the
/// arc; else nowhere. The server and the client's Space (stomp or shove?) both ask this.
pub fn stomp_spot(c: &CombatTuning, origin: Vec2, facing: f32, feet: Vec2, zombie_facing: f32) -> Option<StompSpot> {
    let forward = Vec2::from_angle(facing);
    let half_arc = (c.stomp_arc_degrees * 0.5).to_radians();
    let in_arc = |p: Vec2| {
        let to = p - origin;
        let d = to.length();
        d <= 1e-4 || forward.dot(to / d).clamp(-1.0, 1.0).acos() <= half_arc
    };
    let head = feet - Vec2::from_angle(zombie_facing) * c.down_head_offset.max(0.0);
    let dh = head.distance(origin);
    if dh <= c.stomp_head_reach && in_arc(head) {
        return Some(StompSpot { dist: dh, at: head, head: true });
    }
    // The nearest point of the body from feet to head.
    let body = head - feet;
    let t = if body.length_squared() > 1e-8 { ((origin - feet).dot(body) / body.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    let near = feet + body * t;
    let d = near.distance(origin);
    (d <= c.stomp_range && in_arc(near)).then_some(StompSpot { dist: d, at: near, head: false })
}

/// Where a stomp lands on a body ([`stomp_spot`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StompSpot {
    /// From the stomping player's centre.
    pub dist: f32,
    pub at: Vec2,
    /// On the head: `stomp_head_mult` times the damage.
    pub head: bool,
}

/// How many shoves zombie `id` has had within `shove_repeat_secs` before `tick` (at most
/// `shove_repeat_max`).
pub(crate) fn shoves_before(shoved: &[Shoved], id: u32, tick: u64, c: &CombatTuning) -> u32 {
    let window = (c.shove_repeat_secs / TICK_DT).round() as u64;
    match shoved.binary_search_by_key(&id, |s| s.zombie) {
        Ok(k) if tick.saturating_sub(shoved[k].tick) <= window => (shoved[k].count as u32).min(c.shove_repeat_max),
        _ => 0,
    }
}

/// Note a shove on zombie `id` (or, `floored`, forget it: it starts over), and forget the ones
/// too old to count.
pub(crate) fn remember_shove(shoved: &mut Vec<Shoved>, id: u32, tick: u64, c: &CombatTuning, floored: bool) {
    let before = shoves_before(shoved, id, tick, c);
    let window = (c.shove_repeat_secs / TICK_DT).round() as u64;
    shoved.retain(|s| s.zombie != id && tick.saturating_sub(s.tick) <= window);
    if !floored {
        let count = (before + 1).min(c.shove_repeat_max).min(u8::MAX as u32) as u8;
        let at = shoved.partition_point(|s| s.zombie < id);
        shoved.insert(at, Shoved { zombie: id, count, tick });
    }
}

/// A zombie shoved lately ([`World::shoved`]): how many times running and when last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shoved {
    pub zombie: u32,
    pub count: u8,
    pub tick: u64,
}

/// Initial speed that knocks a body back about `metres` (velocity decays by a quarter a tick, so
/// the slide covers about 3 ticks' worth), capped at [`MAX_KNOCKBACK_SPEED`].
pub fn knockback_speed(metres: f32) -> f32 {
    slide_speed(metres, 0.75)
}

/// The speed that slides a body `metres` before it stops, keeping `slide` of its speed each
/// tick (a walker's 0.75, a reeling zombie's `ZombieTuning::knockback_slide`), at most
/// [`MAX_KNOCKBACK_SPEED`].
pub fn slide_speed(metres: f32, slide: f32) -> f32 {
    let slide = slide.clamp(0.05, 0.98);
    (metres * (1.0 - slide) / (slide * TICK_DT)).min(MAX_KNOCKBACK_SPEED)
}

#[cfg(test)]
mod pvp_tests;

#[cfg(test)]
mod lag_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{Edge, Side, TileMap};
    use crate::template::BuildingTemplate;
    use crate::world::WorldConfig;
    use crate::{PlayerInput, items};
    use glam::IVec2;
    use std::f32::consts::{FRAC_PI_2, PI, TAU};

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
        let mut c = WorldConfig::new(seed, zombies, templates()).with_weapons(weapons()).with_items(items());
        // These tests count on 3 L of pockets.
        c.carry.pockets_l = 3.0;
        World::new(c)
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
        // On the ground it doesn't attack or chase, and swings pass over it (once the shove's
        // slide has run out).
        w.zombies.timer[z] = 10.0;
        for _ in 0..20 {
            w.step();
        }
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
        // It strikes back the whole time: this is about endurance, so don't die of it.
        w.players[0].hp = 1e6;
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
                    let d = w.config.health.wound(a.wound.unwrap()).damage;
                    assert!(a.damage >= d.0 && a.damage <= d.1);
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
        assert!(p.inventory.is_empty() && p.state.mods == w.config.skills.move_mods(w.config.skills.start_fitness, crate::players::MoveMods::default()));
        // Dead: input is ignored and the zombies lose interest.
        let at = p.state.pos;
        w.set_player_input(id, 1, PlayerInput { move_dir: Vec2::X, facing: 0.0, run: true, sneak: false, aim: false });
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
            // A new survivor every time (Strength 1, Brawl 0): shoving practises Brawl, which floors more.
            w.players[0].skills = crate::skills::Skills::start(&w.config.skills);
            (w.zombies.pos[z], w.zombies.vel[z], w.zombies.facing[z]) = (Vec2::new(101.3, 100.5), Vec2::ZERO, facing);
            (w.zombies.state[z], w.zombies.timer[z], w.zombies.anim[z]) = (ZombieState::Wander, 0.0, ZombieAnim::Idle);
            // A first shove each time (earlier ones add up, tested below).
            w.shoved.clear();
            rest(&mut w, id);
            assert!(w.player_shove(id));
            let hits = until_ready(&mut w, id);
            assert_eq!(hits.len(), 1);
            downs += (hits[0].effect == HitEffect::KnockedDown) as u32;
        }
        downs as f32 / trials as f32
    }

    /// A shove knocks a lone zombie down now and then (a little more than one in four from the
    /// front, with a new survivor's Strength), more often from behind.
    #[test]
    fn shove_sometimes_knocks_down_a_lone_zombie() {
        let mut c = CombatTuning::default();
        c.shove_knockdown_chance += crate::skills::SkillTuning::default().shove_knockdown_bonus(2, 0);
        let front = shove_knockdown_rate(PI);
        let behind = shove_knockdown_rate(0.0);
        eprintln!("shove knockdowns: {front:.2} from the front, {behind:.2} from behind (chance {}, ×{} behind)", c.shove_knockdown_chance, c.behind_knockdown_mult);
        assert!((front - c.shove_knockdown_chance).abs() < 0.07, "front {front}");
        assert!((behind - c.shove_knockdown_chance * c.behind_knockdown_mult).abs() < 0.08, "behind {behind}");
        assert!(front > 0.15 && behind > front);
    }

    /// A shove pushes a lone zombie well back (room for a swing), further with a weapon in hand.
    #[test]
    fn a_shove_makes_room_to_swing() {
        let mut pushed = Vec::new();
        for weapon in [None, Some("baseball_bat")] {
            let (mut w, id) = arena(31);
            if let Some(k) = weapon {
                equip(&mut w, id, k);
            }
            w.config.combat.shove_knockdown_chance = 0.0;
            let z = zombie(&mut w, Vec2::new(101.3, 100.5), PI);
            let before = w.zombies.pos[z].x;
            assert!(w.player_shove(id));
            // How far back it slides before it comes on again.
            let mut d: f32 = 0.0;
            for _ in 0..30 {
                w.step();
                d = d.max(w.zombies.pos[z].x - before);
            }
            eprintln!("shove ({weapon:?}): pushed back {d:.2} m");
            pushed.push(d);
        }
        assert!(pushed[0] > 1.0, "bare hands pushed only {:.2} m", pushed[0]);
        assert!(pushed[1] > pushed[0] + 0.1, "a bat shoved no further: {pushed:?}");
        // A bat's reach from the player's centre is its range plus a body: the zombie is now
        // out at the end of it, not crowding it.
        let bat = &weapons()[weapons().iter().position(|w| w.id == "baseball_bat").unwrap()];
        assert!(0.8 + pushed[1] > bat.close_range + ZOMBIE_RADIUS + PLAYER_RADIUS);
    }

    /// Shoves on one zombie in quick succession add up: the third is far likelier to floor it than
    /// the first. A knockdown starts the count over.
    #[test]
    fn repeated_shoves_add_up_to_a_knockdown() {
        let (mut w, id) = arena(32);
        let z = zombie(&mut w, Vec2::new(101.3, 100.5), PI);
        let zid = w.zombies.id[z];
        let mut downs = [0u32; 3];
        let trials = 300;
        for _ in 0..trials {
            w.shoved.clear();
            for k in 0..3 {
                w.players[0].skills = crate::skills::Skills::start(&w.config.skills);
                (w.zombies.pos[z], w.zombies.vel[z], w.zombies.facing[z]) = (Vec2::new(101.3, 100.5), Vec2::ZERO, PI);
                // Recovered from the last one: only the count carries over, not the stagger.
                (w.zombies.state[z], w.zombies.timer[z], w.zombies.anim[z]) = (ZombieState::Wander, 0.0, ZombieAnim::Idle);
                rest(&mut w, id);
                assert!(w.player_shove(id));
                let hits = until_ready(&mut w, id);
                if hits[0].effect == HitEffect::KnockedDown {
                    downs[k] += 1;
                    assert!(w.shoved.iter().all(|s| s.zombie != zid), "a knockdown starts over");
                    break;
                }
                assert_eq!(super::shoves_before(&w.shoved, zid, w.tick, &w.config.combat), k as u32 + 1);
            }
        }
        // Rates of the k-th shove flooring it, of those that got that far.
        let first = downs[0] as f32 / trials as f32;
        let third = downs[2] as f32 / (trials - downs[0] - downs[1]).max(1) as f32;
        eprintln!("shove knockdowns: first {first:.2}, third {third:.2} ({downs:?})");
        assert!(third > first + 0.2, "first {first}, third {third}");
        // Shoves too far apart don't add up.
        w.shoved.clear();
        super::remember_shove(&mut w.shoved, zid, 100, &w.config.combat, false);
        let late = 100 + (w.config.combat.shove_repeat_secs / TICK_DT) as u64 + 5;
        assert_eq!(super::shoves_before(&w.shoved, zid, late, &w.config.combat), 0);
        // And they are saved (in a generated town: the arena's map isn't one a save can rebuild).
        let mut w = World::new(WorldConfig::new(53, 0, templates()).with_weapons(weapons()).with_items(items()));
        w.add_player(w.player_spawn);
        let at = w.player_spawn + Vec2::new(5.0, 0.0);
        let z = zombie(&mut w, at, PI);
        w.step();
        let hash = w.state_hash();
        super::remember_shove(&mut w.shoved, w.zombies.id[z], w.tick, &w.config.combat, false);
        assert_ne!(w.state_hash(), hash, "shoves adding up are in the hash");
        let loaded = World::load(&w.save(), &w.config).unwrap();
        assert_eq!(loaded.shoved, w.shoved);
        assert_eq!(loaded.state_hash(), w.state_hash());
    }

    #[test]
    fn a_stomp_finds_the_body_and_the_head() {
        let c = CombatTuning::default();
        // A zombie that faced west, knocked onto its back: feet at (0, 0), head 1.45 m east.
        let (feet, facing) = (Vec2::ZERO, PI);
        let head = Vec2::new(c.down_head_offset, 0.0);
        // Standing by the head, facing it: the head.
        let s = super::stomp_spot(&c, head + Vec2::new(0.0, -0.6), FRAC_PI_2, feet, facing).unwrap();
        assert!(s.head && s.at.distance(head) < 1e-4, "{s:?}");
        // Beside the middle of the body: the body, where it's nearest.
        let s = super::stomp_spot(&c, Vec2::new(0.6, -0.8), FRAC_PI_2, feet, facing).unwrap();
        assert!(!s.head && (s.at - Vec2::new(0.6, 0.0)).length() < 1e-4, "{s:?}");
        // At the feet, facing them: the body.
        assert!(!super::stomp_spot(&c, Vec2::new(-0.7, 0.0), 0.0, feet, facing).unwrap().head);
        // Facing away, or too far off: nothing.
        assert!(super::stomp_spot(&c, Vec2::new(-0.7, 0.0), PI, feet, facing).is_none());
        assert!(super::stomp_spot(&c, Vec2::new(0.6, -2.0), FRAC_PI_2, feet, facing).is_none());
    }

    /// One stamp on the head finishes a downed zombie; a stamp keeps one getting up on the ground.
    #[test]
    fn a_stomp_to_the_head_is_decisive() {
        for seed in 1..=8 {
            let (mut w, id) = arena(40 + seed);
            let z = zombie(&mut w, Vec2::new(101.2, 100.5), PI);
            (w.zombies.state[z], w.zombies.anim[z], w.zombies.timer[z]) = (ZombieState::Down, ZombieAnim::Down, 5.0);
            // Stand south of its head, facing north.
            let head = w.zombies.pos[z] + Vec2::new(w.config.combat.down_head_offset, 0.0);
            w.players[0].state.pos = head + Vec2::new(0.0, 0.6);
            w.players[0].state.facing = -FRAC_PI_2;
            w.players[0].input.facing = -FRAC_PI_2;
            assert!(w.player_stomp(id));
            let hits = until_ready(&mut w, id);
            assert_eq!(hits.len(), 1, "seed {seed}");
            assert!(hits[0].crit, "seed {seed}: a head stomp is a crit");
            assert_eq!(w.zombies.state[z], ZombieState::Dead, "seed {seed}: survived a stamp on the head ({:?})", hits[0]);
        }
        // Getting up: a stamp on the body puts it back down for a while.
        let (mut w, id) = arena(50);
        let z = zombie(&mut w, Vec2::new(101.2, 100.5), PI);
        (w.zombies.state[z], w.zombies.anim[z], w.zombies.timer[z]) = (ZombieState::Down, ZombieAnim::GetUp, 1.0);
        w.zombies.hp[z] = 1000.0;
        assert!(w.player_stomp(id));
        let hits = until_ready(&mut w, id);
        assert_eq!(hits.len(), 1);
        assert_eq!(w.zombies.state[z], ZombieState::Down);
        assert!(w.zombies.timer[z] > crate::zombies::GET_UP_SECS, "still getting up: {} s", w.zombies.timer[z]);
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
        // The old 3 L pockets, so the mug below fills them.
        w.config.carry.pockets_l = 3.0;
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

    /// Two players 0.8 m apart, the first swinging a weapon at the second until `swings` have
    /// landed (or the second is dead). Returns the world and both ids.
    fn duel(pvp: bool, weapon: &str, swings: u32) -> (World, u32, u32) {
        let (mut w, a) = arena(5);
        w.config.combat.pvp = pvp;
        let b = w.add_player(Vec2::new(101.3, 100.5));
        equip(&mut w, a, weapon);
        w.set_player_input(a, 1, PlayerInput { facing: 0.0, aim: true, ..Default::default() });
        let mut landed = 0;
        for _ in 0..20 * 60 {
            if landed >= swings || w.player(b).unwrap().dead {
                break;
            }
            w.player_attack(a);
            w.step();
            landed += w.action_events.iter().filter(|e| e.player == a).count() as u32;
            // A blow knocks it back (#65): back in front of the next swing, and rested.
            let pi = w.players.iter().position(|p| p.id == b).unwrap();
            w.players[pi].state.pos = Vec2::new(101.3, 100.5);
            w.players[0].state.endurance = 1.0;
        }
        (w, a, b)
    }

    /// PvP (PLAN §4.8): with `pvp` on a swing wounds another player in its arc and can kill, and
    /// the death names a survivor; off (single-player, co-op), players can't hurt each other.
    /// (The whole of it, shoves, stomps and knockdowns included, is in `pvp_tests`.)
    #[test]
    fn swings_hit_other_players_only_with_pvp_on() {
        let (w, _, b) = duel(false, "baseball_bat", 3);
        let p = w.player(b).unwrap();
        assert_eq!((p.hp, p.health.wounds.len()), (100.0, 0), "co-op: untouched");
        assert!(w.combat_events.iter().all(|e| e.kind != CombatEventKind::PlayerHurt));

        let (w, a, b) = duel(true, "baseball_bat", 1);
        let p = w.player(b).unwrap();
        assert!(p.hp < 100.0 && !p.dead, "one swing of a bat hurts: hp {}", p.hp);
        assert_eq!(p.health.wounds.len(), 1);
        assert!(matches!(p.health.wounds[0].kind, WoundKind::Bruise | WoundKind::Gash), "a bat bruises, or splits the skin");
        assert_eq!(w.player(a).unwrap().hp, 100.0, "the one swinging isn't hit");
        let e = w.combat_events.iter().find(|e| e.kind == CombatEventKind::PlayerHurt).expect("a hurt event for the client");
        assert_eq!((e.player, e.zombie, e.by), (b, 0, a));

        let (w, a, b) = duel(true, "kitchen_knife", 99);
        let p = w.player(b).unwrap();
        assert!(p.dead, "enough swings kill");
        assert!(p.health.wounds.is_empty() || p.health.wounds.iter().all(|x| x.kind == WoundKind::Gash || x.kind == WoundKind::Bruise));
        let death = p.run.death.expect("a cause of death");
        assert!(matches!(death.cause, DeathCause::Survivor { by, .. } if by == a), "{:?}", death.cause);
        assert!(death.cause.describe(false).starts_with("Killed by another survivor"));

        // A player behind the one swinging, or beyond a wall of reach, isn't hit; god mode holds.
        let (mut w, a) = arena(6);
        w.config.combat.pvp = true;
        let behind = w.add_player(Vec2::new(99.5, 100.5));
        let god = w.add_player(Vec2::new(101.5, 100.5));
        w.set_dev(god, Some(true), None);
        w.set_player_input(a, 1, PlayerInput { facing: 0.0, aim: true, ..Default::default() });
        for _ in 0..60 {
            w.player_attack(a);
            w.step();
        }
        assert_eq!(w.player(behind).unwrap().hp, 100.0);
        assert_eq!(w.player(god).unwrap().hp, 100.0);
        assert!(w.player(god).unwrap().health.wounds.is_empty());
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
                w.set_player_input(id, t + 1, PlayerInput { move_dir: Vec2::ZERO, facing: a, run: false, sneak: false, aim: false });
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
        for _ in 0..20 * 9 {
            w.step();
            if anims.last() != Some(&w.zombies.anim[z]) {
                anims.push(w.zombies.anim[z]);
            }
        }
        let up = anims.iter().position(|a| *a == ZombieAnim::GetUp).expect("never got up");
        assert_eq!(anims[up - 1], ZombieAnim::Down, "{anims:?}");
        assert!(!matches!(anims[up + 1], ZombieAnim::Down | ZombieAnim::GetUp), "{anims:?}");
    }

    /// Zombies all round the player, every one facing them, close enough to strike at once.
    fn horde(w: &mut World, n: usize) -> Vec<usize> {
        (0..n)
            .map(|k| {
                let a = k as f32 * TAU / n as f32;
                zombie(w, Vec2::new(100.5, 100.5) + Vec2::from_angle(a) * (0.8 + 0.6 * (k % 2) as f32), a + PI)
            })
            .collect()
    }

    /// How many zombies are after player `id` (chasing, lunging, striking or battering toward them).
    fn after(w: &World, id: u32) -> usize {
        (0..w.zombies.len()).filter(|&i| w.zombies.target_player[i] == id && w.zombies.state[i].is_after(id)).count()
    }

    /// God mode: a horde strikes (the strikes land and are reported) but the player keeps 100 HP;
    /// with it off again, the same horde kills them.
    #[test]
    fn god_mode_survives_a_horde() {
        let (mut w, id) = arena(41);
        assert_eq!(w.player(id).unwrap().hp, 100.0);
        assert_eq!(w.set_dev(id, Some(true), None), Some(DevFlags { god: true, ignored: false }));
        horde(&mut w, 12);
        let (mut landed, mut hurt) = (0, 0);
        for _ in 0..20 * 60 {
            w.step();
            landed += w.attack_events.iter().filter(|a| a.landed && a.player == id).count();
            hurt += w.combat_events.drain(..).filter(|e| e.kind == CombatEventKind::PlayerHurt && e.player == id).count();
            let p = w.player(id).unwrap();
            assert!(!p.dead && p.hp == 100.0, "god mode lost HP: {}", p.hp);
        }
        assert!(landed > 50 && hurt == landed, "{landed} strikes landed, {hurt} hurt events");
        assert_eq!(w.set_dev(id, Some(false), None), Some(DevFlags::default()));
        let mut ticks = 0;
        while !w.player(id).unwrap().dead {
            w.step();
            ticks += 1;
            assert!(ticks < 20 * 60, "survived a minute without god mode");
        }
        assert_eq!(w.set_dev(99, Some(true), None), None, "no such player");
    }

    /// Ignored: a zombie looking straight at the player doesn't see them, one behind a running
    /// (noisy) player doesn't hear them, and one it shoves doesn't come for it. Not ignored, each
    /// does (the control).
    #[test]
    fn ignored_players_are_neither_seen_nor_heard() {
        let watch = |ignored: bool, setup: fn(&mut World, u32) -> usize, run: bool| {
            let (mut w, id) = arena(43);
            w.set_dev(id, None, Some(ignored));
            let z = setup(&mut w, id);
            let mut noticed = false;
            for t in 0..20 * 10u32 {
                if run {
                    let a = t as f32 * 0.3;
                    w.set_player_input(id, t + 1, PlayerInput { move_dir: Vec2::from_angle(a), facing: PI, run: true, sneak: false, aim: false });
                }
                w.step();
                assert!(!ignored || w.attack_events.is_empty(), "struck an ignored player");
                noticed |= w.zombies.state[z] != ZombieState::Wander && !w.zombies.state[z].is_reeling();
            }
            noticed
        };
        // Sight: 6 m ahead, facing the player, who stands still in daylight (no noise).
        let ahead: fn(&mut World, u32) -> usize = |w, _| zombie(w, Vec2::new(106.5, 100.5), PI);
        // Hearing: 7 m off with its back to the player, who runs about (in earshot of the dullest).
        let behind: fn(&mut World, u32) -> usize = |w, _| zombie(w, Vec2::new(107.5, 100.5), 0.0);
        assert!(watch(false, ahead, false) && !watch(true, ahead, false), "sight");
        assert!(watch(false, behind, true) && !watch(true, behind, true), "hearing");
        // Being hit: shoved, it turns on the player when it recovers, finds nobody and goes to look.
        let (mut w, id) = arena(43);
        w.set_dev(id, None, Some(true));
        let z = zombie(&mut w, Vec2::new(101.3, 100.5), PI);
        assert!(w.player_shove(id));
        for _ in 0..20 * 10 {
            w.step();
            assert!(w.attack_events.is_empty(), "struck an ignored player");
        }
        assert_eq!(after(&w, id), 0, "{:?}", w.zombies.state[z]);
    }

    /// Turning ignored on makes the zombies after the player lose them at once (they look where
    /// they last saw them, then wander); off again, they notice the player as usual.
    #[test]
    fn ignoring_drops_chasers_and_turning_it_off_brings_them_back() {
        let (mut w, id) = arena(47);
        w.set_dev(id, Some(true), None);
        horde(&mut w, 6);
        for _ in 0..20 * 5 {
            w.step();
        }
        assert!(after(&w, id) >= 4, "only {} after the player", after(&w, id));
        w.set_dev(id, None, Some(true));
        assert_eq!(after(&w, id), 0);
        assert!(w.zombies.state.iter().all(|s| matches!(s, ZombieState::Investigate | ZombieState::Staggered | ZombieState::Down)), "{:?}", w.zombies.state);
        let secs = w.config.zombie.investigate_secs + 5.0;
        for _ in 0..(secs / TICK_DT) as u32 {
            w.step();
            assert_eq!(after(&w, id), 0);
            assert!(w.attack_events.is_empty());
        }
        assert!(w.zombies.state.iter().all(|s| *s == ZombieState::Wander), "{:?}", w.zombies.state);
        w.set_dev(id, None, Some(false));
        let mut t = 0u32;
        while after(&w, id) == 0 {
            t += 1;
            w.set_player_input(id, t, PlayerInput { move_dir: Vec2::from_angle(t as f32 * 0.2), facing: 0.0, run: false, sneak: false, aim: false });
            w.step();
            assert!(t < 20 * 20, "nobody noticed the player again in 20 s");
        }
    }

    /// Dev toggles leave the state hash alone while off, change it while on, and aren't saved.
    #[test]
    fn dev_flags_hash_only_when_on_and_are_not_saved() {
        let mut w = World::new(WorldConfig::new(53, 0, templates()).with_weapons(weapons()).with_items(items()));
        let id = w.add_player(w.player_spawn);
        w.step();
        let (hash, bytes) = (w.state_hash(), w.save());
        w.set_dev(id, Some(true), Some(true));
        assert_ne!(w.state_hash(), hash);
        assert_eq!(w.save(), bytes, "dev toggles are not in the save");
        let loaded = World::load(&w.save(), &w.config).unwrap();
        assert_eq!(loaded.player(id).unwrap().dev, DevFlags::default());
        assert_eq!(loaded.state_hash(), hash);
        w.set_dev(id, Some(false), Some(false));
        assert_eq!(w.state_hash(), hash);
    }
}
