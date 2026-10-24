//! Skills (#46): players get better at what they do, by doing it.
//!
//! Nine skills, each with XP that only practice raises and a level from 0 to [`MAX_LEVEL`] at
//! cumulative XP thresholds ([`SkillTuning::levels`]: 100, 250, 500, 900, 1,500). Everyone starts at 0,
//! except Fitness and Strength at 1 (100 XP). Nothing is lost when a skill isn't used; there are no
//! skill books. A respawn is a new survivor and starts over.
//!
//! | Skill | Raised by (XP) | Each level gives |
//! |---|---|---|
//! | Fitness | running: 1 per 10 s | endurance drains 6% slower and recovers 6% faster |
//! | Strength | swinging a weapon over 1.5 kg: 1 per swing; moving over your carry limit: 1 per game minute | +1.5 kg carry limit (`CarryTuning::strength_kg`); shove knockdown chance +3%; knockback +5% |
//! | Bludgeon | hits with blunt weapons: 2 per hit, 5 per kill | damage +6%, endurance cost -5%, swing time -3% |
//! | Edged | hits with knives and axes: 2 per hit, 5 per kill | as Bludgeon |
//! | Brawl | punches 2 (a punch that kills 5), each zombie shoved 2, stomp kills 5 | punch and stomp damage +10%; shove knockdown chance +4% |
//! | Stealth | sneaking (moving) within 15 m of a zombie that isn't after you: 1 per 5 s | sneaking footsteps 8% quieter; zombies see you from 6% less far |
//! | First Aid | each treatment done (#10): 10 | treatment 10% faster (`FirstAidTuning::skill_faster_per_level`); from level 3 stitching hurts half as much |
//! | Scavenging | each container searched for the first time: 2 | first searches 10% faster and 10% quieter |
//! | Handiwork | each craft (#12): 5 | crafting 10% faster ([`SkillTuning::craft_time_mult`]) |
//!
//! Weapon skills follow how a weapon hits, not each weapon: every weapon in `weapons.ron` names its
//! skill (`skill: Some(Bludgeon)`), defaulting to its category's ([`crate::items::WeaponCategory::skill`]:
//! `ShortBlunt`/`LongBlunt` Bludgeon, `ShortBlade`/`Axe` Edged, `Unarmed` Brawl). A blow that kills
//! gives the kill's XP instead of the hit's.
//!
//! XP is earned in the sim (deterministic), saved with the player (format 7) and in `state_hash`.
//! Effects are applied where those numbers are already used: Fitness through [`MoveMods`] (so the
//! snapshot carries it and client prediction stays exact; a Fitness or Strength level-up refreshes the
//! modifiers at the end of the tick, after movement, like every other change to them), Strength in the
//! carry limit and the combat formulas, the weapon skills in `combat`, Stealth in the senses and
//! footsteps, First Aid in `firstaid`, Scavenging in searching. Each level-up is a [`LevelUp`] in
//! `World::level_ups`, drained into snapshots for the client's HUD line ("Edged 2").
//!
//! **For crafting (#12):** [`World::skill_level`] and [`World::award_xp`] (by player index), with
//! [`SkillTuning::craft_xp`] per craft and [`SkillTuning::craft_time_mult`] for its time.
//!
//! Every number is ours ([`SkillTuning`], serde defaults, saved with the world's settings).

use serde::{Deserialize, Serialize};

use crate::TICK_DT;
use crate::firstaid::FirstAidTuning;
use crate::inventory::{Burden, CarryTuning};
use crate::players::{MoveMods, Stance};
use crate::world::World;
use crate::zombies::ZombieState;

/// The highest level.
pub const MAX_LEVEL: u32 = 5;
/// How many skills there are.
pub const SKILL_COUNT: usize = 9;
/// Level-ups kept for replication if nobody drains them (headless servers, tests).
pub const MAX_LEVEL_UPS: usize = 64;
/// No skill makes a time, cost or noise smaller than this share of its usual.
const MIN_MULT: f32 = 0.2;

/// One of the skills.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Skill {
    Fitness,
    Strength,
    Bludgeon,
    Edged,
    Brawl,
    Stealth,
    FirstAid,
    Scavenging,
    /// Crafting (#12; this module only keeps its XP and level).
    Handiwork,
}

impl Skill {
    pub const ALL: [Skill; SKILL_COUNT] =
        [Self::Fitness, Self::Strength, Self::Bludgeon, Self::Edged, Self::Brawl, Self::Stealth, Self::FirstAid, Self::Scavenging, Self::Handiwork];

    /// Lowercase id (MCP arguments).
    pub fn id(self) -> &'static str {
        match self {
            Self::Fitness => "fitness",
            Self::Strength => "strength",
            Self::Bludgeon => "bludgeon",
            Self::Edged => "edged",
            Self::Brawl => "brawl",
            Self::Stealth => "stealth",
            Self::FirstAid => "first_aid",
            Self::Scavenging => "scavenging",
            Self::Handiwork => "handiwork",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Fitness => "Fitness",
            Self::Strength => "Strength",
            Self::Bludgeon => "Bludgeon",
            Self::Edged => "Edged",
            Self::Brawl => "Brawl",
            Self::Stealth => "Stealth",
            Self::FirstAid => "First Aid",
            Self::Scavenging => "Scavenging",
            Self::Handiwork => "Handiwork",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }

    /// The skills a weapon can name: how it hits.
    pub fn is_weapon_skill(self) -> bool {
        matches!(self, Self::Bludgeon | Self::Edged | Self::Brawl)
    }
}

/// A player's XP in each skill (index: `Skill as usize`). Saved with the player.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Skills {
    pub xp: [f32; SKILL_COUNT],
}

impl Default for Skills {
    /// A new survivor with the built-in numbers (`World::add_player` uses the world's own).
    fn default() -> Self {
        Self::start(&SkillTuning::default())
    }
}

impl Skills {
    /// A new survivor: Fitness and Strength at their starting levels, the rest at 0.
    pub fn start(t: &SkillTuning) -> Self {
        let mut xp = [0.0; SKILL_COUNT];
        xp[Skill::Fitness as usize] = t.xp_for(t.start_fitness);
        xp[Skill::Strength as usize] = t.xp_for(t.start_strength);
        Self { xp }
    }

    pub fn xp(&self, s: Skill) -> f32 {
        self.xp[s as usize]
    }

    pub fn level(&self, s: Skill, t: &SkillTuning) -> u32 {
        t.level(self.xp(s))
    }

    /// Every skill as a client and an agent see it.
    pub fn view(&self, t: &SkillTuning) -> Vec<SkillView> {
        Skill::ALL
            .into_iter()
            .map(|s| {
                let (xp, level) = (self.xp(s), self.level(s, t));
                let next_at = (level < MAX_LEVEL).then(|| t.xp_for(level + 1));
                let progress = next_at.map_or(1.0, |n| {
                    let from = t.xp_for(level);
                    ((xp - from) / (n - from).max(1e-3)).clamp(0.0, 1.0)
                });
                SkillView { skill: s, level, xp: xp.floor(), next_at, progress }
            })
            .collect()
    }
}

/// One skill as a client (the skills panel) and an agent see it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillView {
    pub skill: Skill,
    pub level: u32,
    /// Whole XP so far.
    pub xp: f32,
    /// XP the next level needs (`None` at the top).
    pub next_at: Option<f32>,
    /// 0..1 of the way from this level to the next.
    pub progress: f32,
}

/// A player reached a new level (`World::level_ups`, drained into snapshots).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelUp {
    pub player: u32,
    pub skill: Skill,
    pub level: u32,
}

impl LevelUp {
    /// The HUD's line: "Edged 2".
    pub fn text(&self) -> String {
        format!("{} {}", self.skill.name(), self.level)
    }
}

/// Every skill number: XP per level, what earns it and what each level gives. Serde with defaults,
/// saved with the world's settings. Ours. (Strength's carry limit is `CarryTuning::strength_kg` and
/// First Aid's speed and stitching `FirstAidTuning::skill_*`, where those numbers are used.)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillTuning {
    /// Cumulative XP to reach levels 1 to 5.
    pub levels: [f32; MAX_LEVEL as usize],
    /// Levels a new survivor starts with.
    pub start_fitness: u32,
    pub start_strength: u32,

    // XP.
    /// Fitness per second running.
    pub run_xp_per_sec: f32,
    /// Strength per swing of a weapon heavier than `heavy_weapon_kg`.
    pub heavy_swing_xp: f32,
    pub heavy_weapon_kg: f32,
    /// Strength per game minute moving while over the carry limit.
    pub overload_xp_per_min: f32,
    /// A weapon skill (Bludgeon, Edged, Brawl for punches) per zombie hit, and per kill instead.
    pub hit_xp: f32,
    pub kill_xp: f32,
    /// Brawl per zombie shoved, and per stomp that kills.
    pub shove_xp: f32,
    pub stomp_kill_xp: f32,
    /// Stealth per second sneaking (and moving) within `stealth_range` of a zombie that isn't after you.
    pub stealth_xp_per_sec: f32,
    pub stealth_range: f32,
    /// First Aid per treatment done.
    pub treat_xp: f32,
    /// Scavenging per container searched for the first time.
    pub search_xp: f32,
    /// Handiwork per craft (#12).
    pub craft_xp: f32,

    // What each level gives.
    /// Fitness: running drains endurance this much slower, and it recovers this much faster.
    pub fitness_drain: f32,
    pub fitness_regen: f32,
    /// Strength: shove knockdown chance added (a chance, 0..1), and knockback (a share).
    pub strength_shove_knockdown: f32,
    pub strength_knockback: f32,
    /// Bludgeon and Edged: damage up, endurance cost and swing time down (shares).
    pub weapon_damage: f32,
    pub weapon_cost: f32,
    pub weapon_time: f32,
    /// Brawl: punch and stomp damage up (a share), shove knockdown chance added.
    pub brawl_damage: f32,
    pub brawl_shove_knockdown: f32,
    /// Stealth: sneaking footsteps quieter, and the distance zombies see you from shorter (shares).
    pub stealth_noise: f32,
    pub stealth_sight: f32,
    /// Scavenging: first searches faster and quieter (shares).
    pub search_faster: f32,
    pub search_quieter: f32,
    /// Handiwork: crafting faster (a share).
    pub craft_faster: f32,
}

impl Default for SkillTuning {
    fn default() -> Self {
        Self {
            levels: [100.0, 250.0, 500.0, 900.0, 1500.0],
            start_fitness: 1,
            start_strength: 1,

            run_xp_per_sec: 0.1,
            heavy_swing_xp: 1.0,
            heavy_weapon_kg: 1.5,
            overload_xp_per_min: 1.0,
            hit_xp: 2.0,
            kill_xp: 5.0,
            shove_xp: 2.0,
            stomp_kill_xp: 5.0,
            stealth_xp_per_sec: 0.2,
            stealth_range: 15.0,
            treat_xp: 10.0,
            search_xp: 2.0,
            craft_xp: 5.0,

            fitness_drain: 0.06,
            fitness_regen: 0.06,
            strength_shove_knockdown: 0.03,
            strength_knockback: 0.05,
            weapon_damage: 0.06,
            weapon_cost: 0.05,
            weapon_time: 0.03,
            brawl_damage: 0.1,
            brawl_shove_knockdown: 0.04,
            stealth_noise: 0.08,
            stealth_sight: 0.06,
            search_faster: 0.1,
            search_quieter: 0.1,
            craft_faster: 0.1,
        }
    }
}

/// `per` a level more, from 1.
fn up(per: f32, level: u32) -> f32 {
    1.0 + per * level as f32
}

/// `per` a level less, from 1 (never below [`MIN_MULT`]).
fn down(per: f32, level: u32) -> f32 {
    (1.0 - per * level as f32).max(MIN_MULT)
}

fn pct(m: f32) -> String {
    format!("{:.0}%", ((m - 1.0) * 100.0).abs())
}

impl SkillTuning {
    /// The level `xp` reaches.
    pub fn level(&self, xp: f32) -> u32 {
        self.levels.iter().filter(|&&n| xp >= n).count() as u32
    }

    /// XP at the start of `level` (0 for level 0).
    pub fn xp_for(&self, level: u32) -> f32 {
        match level.min(MAX_LEVEL) {
            0 => 0.0,
            l => self.levels[l as usize - 1],
        }
    }

    /// XP stops here (the top level).
    pub fn max_xp(&self) -> f32 {
        self.levels[MAX_LEVEL as usize - 1]
    }

    /// A swing's damage at this level of the skill it uses (stomps: Brawl's).
    pub fn damage_mult(&self, skill: Skill, level: u32) -> f32 {
        match skill {
            Skill::Bludgeon | Skill::Edged => up(self.weapon_damage, level),
            Skill::Brawl => up(self.brawl_damage, level),
            _ => 1.0,
        }
    }

    /// A swing's endurance cost at this level of its skill.
    pub fn cost_mult(&self, skill: Skill, level: u32) -> f32 {
        if matches!(skill, Skill::Bludgeon | Skill::Edged) { down(self.weapon_cost, level) } else { 1.0 }
    }

    /// A swing's windup and recovery at this level of its skill.
    pub fn time_mult(&self, skill: Skill, level: u32) -> f32 {
        if matches!(skill, Skill::Bludgeon | Skill::Edged) { down(self.weapon_time, level) } else { 1.0 }
    }

    /// Knockback of swings and shoves at this Strength.
    pub fn knockback_mult(&self, strength: u32) -> f32 {
        up(self.strength_knockback, strength)
    }

    /// Added to a shove's knockdown chance at this Strength and Brawl.
    pub fn shove_knockdown_bonus(&self, strength: u32, brawl: u32) -> f32 {
        self.strength_shove_knockdown * strength as f32 + self.brawl_shove_knockdown * brawl as f32
    }

    /// Sneaking footsteps' radius at this Stealth.
    pub fn footstep_mult(&self, stealth: u32) -> f32 {
        down(self.stealth_noise, stealth)
    }

    /// How far zombies can see a player with this Stealth (a multiplier on their sight range).
    pub fn sight_mult(&self, stealth: u32) -> f32 {
        down(self.stealth_sight, stealth)
    }

    /// A first search's time, and its noise, at this Scavenging.
    pub fn search_time_mult(&self, scavenging: u32) -> f32 {
        down(self.search_faster, scavenging)
    }

    pub fn search_noise_mult(&self, scavenging: u32) -> f32 {
        down(self.search_quieter, scavenging)
    }

    /// For crafting (#12): a craft's time at this Handiwork is its usual time times this.
    pub fn craft_time_mult(&self, handiwork: u32) -> f32 {
        down(self.craft_faster, handiwork)
    }

    /// Movement with Fitness on top: running drains endurance slower, it comes back faster.
    pub fn move_mods(&self, fitness: u32, mods: MoveMods) -> MoveMods {
        MoveMods { run_drain: mods.run_drain * down(self.fitness_drain, fitness), regen: mods.regen * up(self.fitness_regen, fitness), ..mods }
    }

    /// What `level` in `skill` gives, in all (the skills panel and the MCP `skills` tool).
    pub fn gives(&self, skill: Skill, level: u32, carry: &CarryTuning, aid: &FirstAidTuning) -> String {
        if level == 0 {
            return "nothing yet".into();
        }
        let l = level;
        match skill {
            Skill::Fitness => format!("endurance drains {} slower and recovers {} faster", pct(down(self.fitness_drain, l)), pct(up(self.fitness_regen, l))),
            Skill::Strength => format!(
                "carry limit {:.1} kg (+{:.1}), shove knockdown chance +{:.0}%, knockback +{}",
                carry.limit(l),
                carry.strength_kg * l as f32,
                self.strength_shove_knockdown * l as f32 * 100.0,
                pct(self.knockback_mult(l))
            ),
            Skill::Bludgeon | Skill::Edged => format!(
                "{} damage +{}, endurance cost -{}, swing time -{}",
                if skill == Skill::Bludgeon { "blunt weapons:" } else { "knives and axes:" },
                pct(self.damage_mult(skill, l)),
                pct(self.cost_mult(skill, l)),
                pct(self.time_mult(skill, l))
            ),
            Skill::Brawl => format!("punch and stomp damage +{}, shove knockdown chance +{:.0}%", pct(self.damage_mult(skill, l)), self.brawl_shove_knockdown * l as f32 * 100.0),
            Skill::Stealth => format!("sneaking footsteps {} quieter, zombies see you from {} less far", pct(self.footstep_mult(l)), pct(self.sight_mult(l))),
            Skill::FirstAid => format!(
                "treatment {} faster{}",
                pct((1.0 - aid.skill_faster_per_level * l as f32).clamp(0.3, 1.0)),
                if l >= aid.skill_stitch_level { ", stitching hurts half as much" } else { "" }
            ),
            Skill::Scavenging => format!("searching {} faster and {} quieter", pct(self.search_time_mult(l)), pct(self.search_noise_mult(l))),
            Skill::Handiwork => format!("crafting {} faster", pct(self.craft_time_mult(l))),
        }
    }

    /// How XP in `skill` is earned.
    pub fn earned_by(&self, skill: Skill) -> String {
        match skill {
            Skill::Fitness => format!("running ({} XP a second)", self.run_xp_per_sec),
            Skill::Strength => format!(
                "swinging a weapon over {} kg ({} XP a swing), moving over your carry limit ({} XP a game minute)",
                self.heavy_weapon_kg, self.heavy_swing_xp, self.overload_xp_per_min
            ),
            Skill::Bludgeon => format!("hits with blunt weapons ({} XP a hit, {} a kill)", self.hit_xp, self.kill_xp),
            Skill::Edged => format!("hits with knives and axes ({} XP a hit, {} a kill)", self.hit_xp, self.kill_xp),
            Skill::Brawl => format!("punches ({} XP, {} a kill), shoves ({} XP a zombie), stomp kills ({} XP)", self.hit_xp, self.kill_xp, self.shove_xp, self.stomp_kill_xp),
            Skill::Stealth => format!("sneaking within {:.0} m of a zombie that isn't after you ({} XP a second)", self.stealth_range, self.stealth_xp_per_sec),
            Skill::FirstAid => format!("each treatment ({} XP)", self.treat_xp),
            Skill::Scavenging => format!("each container searched for the first time ({} XP)", self.search_xp),
            Skill::Handiwork => format!("each thing crafted ({} XP)", self.craft_xp),
        }
    }

    /// Settings a tick can't work with, by name (checked on load).
    pub fn check(&self) -> Result<(), String> {
        let mut last = 0.0;
        for (k, &n) in self.levels.iter().enumerate() {
            if !(n.is_finite() && n > last && n <= 1e6) {
                return Err(format!("skills: levels must rise from above 0 (level {} at {n})", k + 1));
            }
            last = n;
        }
        if self.start_fitness > MAX_LEVEL || self.start_strength > MAX_LEVEL {
            return Err(format!("skills: start levels {} and {} must be 0 to {MAX_LEVEL}", self.start_fitness, self.start_strength));
        }
        let numbers = [
            ("run_xp_per_sec", self.run_xp_per_sec),
            ("heavy_swing_xp", self.heavy_swing_xp),
            ("heavy_weapon_kg", self.heavy_weapon_kg),
            ("overload_xp_per_min", self.overload_xp_per_min),
            ("hit_xp", self.hit_xp),
            ("kill_xp", self.kill_xp),
            ("shove_xp", self.shove_xp),
            ("stomp_kill_xp", self.stomp_kill_xp),
            ("stealth_xp_per_sec", self.stealth_xp_per_sec),
            ("stealth_range", self.stealth_range),
            ("treat_xp", self.treat_xp),
            ("search_xp", self.search_xp),
            ("craft_xp", self.craft_xp),
        ];
        if let Some((name, v)) = numbers.iter().find(|(_, v)| !(v.is_finite() && (0.0..=1e4).contains(v))) {
            return Err(format!("skills.{name} is {v} (must be 0 to 10000)"));
        }
        let shares = [
            ("fitness_drain", self.fitness_drain),
            ("fitness_regen", self.fitness_regen),
            ("strength_shove_knockdown", self.strength_shove_knockdown),
            ("strength_knockback", self.strength_knockback),
            ("weapon_damage", self.weapon_damage),
            ("weapon_cost", self.weapon_cost),
            ("weapon_time", self.weapon_time),
            ("brawl_damage", self.brawl_damage),
            ("brawl_shove_knockdown", self.brawl_shove_knockdown),
            ("stealth_noise", self.stealth_noise),
            ("stealth_sight", self.stealth_sight),
            ("search_faster", self.search_faster),
            ("search_quieter", self.search_quieter),
            ("craft_faster", self.craft_faster),
        ];
        if let Some((name, v)) = shares.iter().find(|(_, v)| !(v.is_finite() && (0.0..=0.5).contains(v))) {
            return Err(format!("skills.{name} is {v} (a share a level, 0 to 0.5)"));
        }
        Ok(())
    }
}

impl World {
    /// Player `pi`'s (an index into `players`) level in `skill`. For crafting (#12): Handiwork.
    pub fn skill_level(&self, pi: usize, skill: Skill) -> u32 {
        self.players.get(pi).map_or(0, |p| p.skills.level(skill, &self.config.skills))
    }

    /// How far zombies can see player `id`, as a multiplier on their sight range (Stealth; 1 if
    /// there's no such player).
    pub fn player_sight_mult(&self, id: u32) -> f32 {
        self.player(id).map_or(1.0, |p| self.config.skills.sight_mult(p.skills.level(Skill::Stealth, &self.config.skills)))
    }

    /// Player `id`'s skills as its own client and an agent see them.
    pub fn skills_view(&self, id: u32) -> Option<Vec<SkillView>> {
        self.player(id).map(|p| p.skills.view(&self.config.skills))
    }

    /// Player `pi` (an index into `players`) earns `xp` in `skill` (nothing if it's dead; XP stops
    /// at the top level). For crafting (#12): `award_xp(pi, Skill::Handiwork, config.skills.craft_xp)`
    /// once per thing crafted. Returns the new level if it went up: that is a [`LevelUp`] for its
    /// client, and for Fitness and Strength its movement modifiers are refreshed at the end of the
    /// tick.
    pub fn award_xp(&mut self, pi: usize, skill: Skill, xp: f32) -> Option<u32> {
        let t = &self.config.skills;
        let p = self.players.get_mut(pi)?;
        if p.dead || xp.is_nan() || xp <= 0.0 {
            return None;
        }
        let s = &mut p.skills.xp[skill as usize];
        let (before, lvl) = (*s, t.level(*s));
        *s = (*s + xp).min(t.max_xp());
        let now = t.level(*s);
        let id = p.id;
        if s.floor() != before.floor() {
            self.private_dirty.insert(id);
        }
        if now == lvl {
            return None;
        }
        if self.level_ups.len() >= MAX_LEVEL_UPS {
            self.level_ups.remove(0);
        }
        self.level_ups.push(LevelUp { player: id, skill, level: now });
        if matches!(skill, Skill::Fitness | Skill::Strength) && !self.skill_mods_due.contains(&id) {
            self.skill_mods_due.push(id);
        }
        Some(now)
    }

    /// Level-ups since the last call (replication: a snapshot drains them).
    pub fn take_level_ups(&mut self) -> Vec<LevelUp> {
        std::mem::take(&mut self.level_ups)
    }

    /// The XP that comes from what players did this tick: running (Fitness), moving over the carry
    /// limit (Strength) and sneaking near zombies that aren't after them (Stealth). After movement
    /// and the senses (the spatial hash is current).
    pub(crate) fn step_skills(&mut self) {
        let t = self.config.skills.clone();
        let minutes = TICK_DT * self.game_hours_per_sec() * 60.0;
        for k in 0..self.targets.len() {
            let target = self.targets[k];
            if !target.moving {
                continue;
            }
            let Some(pi) = self.players.iter().position(|p| p.id == target.id) else { continue };
            match target.stance {
                Stance::Run => {
                    self.award_xp(pi, Skill::Fitness, t.run_xp_per_sec * TICK_DT);
                }
                Stance::Sneak if !target.ignored => {
                    let (z, id) = (&self.zombies, target.id);
                    let mut near = false;
                    self.hash.for_each_within(target.pos, t.stealth_range, |i| {
                        near |= z.state[i] != ZombieState::Dead && z.pos[i].distance(target.pos) <= t.stealth_range && !(z.target_player[i] == id && z.state[i].is_after(id));
                    });
                    if near {
                        self.award_xp(pi, Skill::Stealth, t.stealth_xp_per_sec * TICK_DT);
                    }
                }
                _ => {}
            }
            let p = &self.players[pi];
            if !p.needs.asleep() && self.carry(p).burden > Burden::None {
                self.award_xp(pi, Skill::Strength, t.overload_xp_per_min * minutes);
            }
        }
    }

    /// Movement modifiers for players whose Fitness or Strength went up this tick (end of the
    /// tick, after movement, so the snapshot carries what the next tick moves with).
    pub(crate) fn refresh_skill_mods(&mut self) {
        for id in std::mem::take(&mut self.skill_mods_due) {
            if let Some(pi) = self.players.iter().position(|p| p.id == id) {
                self.refresh_carry(pi);
            }
        }
    }

    /// Fold player skills into the state hash.
    pub(crate) fn hash_skills(s: &Skills, mix: &mut impl FnMut(u32)) {
        for xp in s.xp {
            mix(xp.to_bits());
        }
    }
}

#[cfg(test)]
mod tests;
