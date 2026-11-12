//! Who a survivor was (the skills expansion, phase 2): a **Background**, **Strengths** and **Flaws**.
//!
//! A Background is what you did before (`assets/data/backgrounds.ron`, a list of
//! [`BackgroundDef`]): starting skill levels, XP multipliers, a few things in the pockets, and
//! points to spend. Strengths and Flaws are who you are (`assets/data/strengths_and_flaws.ron`,
//! [`PicksFile`]): a Strength costs points, a Flaw gives them, and each has effects from a fixed
//! list the sim understands ([`Effect`]). Pairs that can't go together name each other in
//! `excludes`. The clothes a new survivor may start in are in the same file's `wardrobe`.
//! Everything is data: a modder adds a Background or a Strength with no code. Names and numbers
//! are ours.
//!
//! **Picking** is a [`Choice`] (ids). [`OriginData::resolve`] is the one rule, for the new-game
//! screen and the world alike: every id exists, nothing twice, no excluded pair, at most
//! [`MAX_PICKS`] of each, the points add up to 0 or more, and the clothes are from the wardrobe and
//! go on together. What comes out is a [`Start`]: the survivor's [`Origin`] (kept on the player:
//! the ids, the starting skill levels and the [`Perks`], its effects added up) and what it has on
//! and in its pockets on the first morning. [`World::begin_as`] makes a newly added player that
//! survivor.
//!
//! **Where the perks apply** (each one line, where that number is already used): XP in
//! `World::award_xp`; the carry limit in `World::carry`; hunger, thirst and tiredness in
//! `step_needs`; endurance drain and recovery in `refresh_carry` (through `MoveMods`, so snapshots
//! carry them and client prediction stays exact); how far zombies see you and how loud your steps
//! are in `World::step`; healing and bleeding in `step_health`; the infection's chance in
//! `add_wound`.
//!
//! **Kept:** `Player::origin` is `#[serde(skip)]` and saved in an optional section of its own
//! (`ORIG`, only survivors that have one), so the save format didn't change; the perks are saved as
//! numbers, so a save doesn't need the data files it was made with. In `state_hash` only for a
//! survivor that has one (a plain survivor hashes as before). A respawn is the same person again:
//! the Origin stays, its starting skills are given anew.

use serde::{Deserialize, Serialize};

use crate::items::{ItemDef, ItemStack};
use crate::skills::{SKILL_COUNT, Skill, SkillTuning, Skills};
use crate::world::World;

/// At most this many Strengths, and this many Flaws.
pub const MAX_PICKS: usize = 6;
/// A multiplier an Origin's effects add up to is kept within this.
pub const MULT_RANGE: (f32, f32) = (0.25, 4.0);
/// Kilograms an Origin may add to or take off the carry limit.
pub const CARRY_RANGE: (f32, f32) = (-8.0, 12.0);
/// The most starting levels an Origin gives one skill.
pub const MAX_START_LEVELS: u32 = 6;

/// One thing a Strength or a Flaw does. Multipliers are shares: 1.2 is a fifth more.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    /// Starting levels in a skill (on top of the survivor's own and its Background's).
    Skill(Skill, u32),
    /// XP earned in a skill is multiplied by this.
    Xp(Skill, f32),
    /// Kilograms on (or off) the carry limit.
    Carry(f32),
    /// How fast hunger, thirst and tiredness rise.
    Hunger(f32),
    Thirst(f32),
    Tiredness(f32),
    /// The endurance running drains, and how fast it comes back.
    RunDrain(f32),
    Recovery(f32),
    /// The distance zombies see you from.
    SeenFrom(f32),
    /// How far your footsteps carry.
    Footsteps(f32),
    /// How fast wounds heal, and how much they bleed.
    Healing(f32),
    Bleeding(f32),
    /// The chance a zombie's wound carries the infection.
    Infection(f32),
}

impl Effect {
    /// In a player's words: "Fitness +2", "XP in First Aid x1.5", "Carry limit +3 kg".
    pub fn text(&self) -> String {
        let pct = |m: f32, more: &str, less: &str| {
            let p = ((m - 1.0) * 100.0).round();
            if p >= 0.0 { format!("{p:.0}% {more}") } else { format!("{:.0}% {less}", -p) }
        };
        match *self {
            Effect::Skill(s, n) => format!("{} +{n}", s.name()),
            Effect::Xp(s, m) => format!("{} XP x{}", s.name(), short(m)),
            Effect::Carry(kg) => format!("Carry limit {}{} kg", if kg >= 0.0 { "+" } else { "" }, short(kg)),
            Effect::Hunger(m) => format!("Hunger {}", pct(m, "faster", "slower")),
            Effect::Thirst(m) => format!("Thirst {}", pct(m, "faster", "slower")),
            Effect::Tiredness(m) => format!("Tires {}", pct(m, "faster", "slower")),
            Effect::RunDrain(m) => format!("Running tires you {}", pct(m, "more", "less")),
            Effect::Recovery(m) => format!("Catches breath {}", pct(m, "faster", "slower")),
            Effect::SeenFrom(m) => format!("Zombies see you from {}", pct(m, "further", "less far")),
            Effect::Footsteps(m) => format!("Footsteps {}", pct(m, "louder", "quieter")),
            Effect::Healing(m) => format!("Wounds heal {}", pct(m, "faster", "slower")),
            Effect::Bleeding(m) => format!("Bleeds {}", pct(m, "more", "less")),
            Effect::Infection(m) => format!("Infection {}", pct(m, "likelier", "less likely")),
        }
    }
}

/// A number as short as it reads: 2, 0.5, 1.25.
fn short(m: f32) -> String {
    let s = format!("{m:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A survivor's effects added up: what the sim reads. Multipliers are 1 and the kilograms 0 for a
/// survivor with no Origin.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Perks {
    /// XP multiplier per skill (index: `Skill as usize`).
    pub xp: [f32; SKILL_COUNT],
    pub carry_kg: f32,
    pub hunger: f32,
    pub thirst: f32,
    pub tiredness: f32,
    pub run_drain: f32,
    pub recovery: f32,
    pub seen_from: f32,
    pub footsteps: f32,
    pub healing: f32,
    pub bleeding: f32,
    pub infection: f32,
}

impl Default for Perks {
    fn default() -> Self {
        Self { xp: [1.0; SKILL_COUNT], carry_kg: 0.0, hunger: 1.0, thirst: 1.0, tiredness: 1.0, run_drain: 1.0, recovery: 1.0, seen_from: 1.0, footsteps: 1.0, healing: 1.0, bleeding: 1.0, infection: 1.0 }
    }
}

impl Perks {
    fn mults(&self) -> [f32; SKILL_COUNT + 9] {
        let mut out = [1.0; SKILL_COUNT + 9];
        out[..SKILL_COUNT].copy_from_slice(&self.xp);
        out[SKILL_COUNT..].copy_from_slice(&[self.hunger, self.thirst, self.tiredness, self.run_drain, self.recovery, self.seen_from, self.footsteps, self.healing, self.bleeding]);
        out
    }

    /// Add one effect (starting skill levels aren't perks: [`Origin::skills`]).
    fn add(&mut self, e: &Effect) {
        match *e {
            Effect::Skill(..) => {}
            Effect::Xp(s, m) => self.xp[s as usize] *= m,
            Effect::Carry(kg) => self.carry_kg += kg,
            Effect::Hunger(m) => self.hunger *= m,
            Effect::Thirst(m) => self.thirst *= m,
            Effect::Tiredness(m) => self.tiredness *= m,
            Effect::RunDrain(m) => self.run_drain *= m,
            Effect::Recovery(m) => self.recovery *= m,
            Effect::SeenFrom(m) => self.seen_from *= m,
            Effect::Footsteps(m) => self.footsteps *= m,
            Effect::Healing(m) => self.healing *= m,
            Effect::Bleeding(m) => self.bleeding *= m,
            Effect::Infection(m) => self.infection *= m,
        }
    }

    /// Every number held to what an Origin may come to.
    fn clamped(mut self) -> Self {
        let c = |v: &mut f32| *v = v.clamp(MULT_RANGE.0, MULT_RANGE.1);
        self.xp.iter_mut().for_each(c);
        for v in [&mut self.hunger, &mut self.thirst, &mut self.tiredness, &mut self.run_drain, &mut self.recovery, &mut self.seen_from, &mut self.footsteps, &mut self.healing, &mut self.bleeding] {
            c(v);
        }
        // The infection may be taken off altogether (0), never made more than four times as likely.
        self.infection = self.infection.clamp(0.0, MULT_RANGE.1);
        self.carry_kg = self.carry_kg.clamp(CARRY_RANGE.0, CARRY_RANGE.1);
        self
    }

    /// Are these numbers a resolved Origin could hold (a save's are checked with this)?
    pub fn check(&self) -> Result<(), String> {
        if self.mults().iter().any(|m| !m.is_finite() || *m < MULT_RANGE.0 || *m > MULT_RANGE.1) {
            return Err(format!("a multiplier outside {} to {}", MULT_RANGE.0, MULT_RANGE.1));
        }
        if !self.infection.is_finite() || !(0.0..=MULT_RANGE.1).contains(&self.infection) {
            return Err(format!("infection x{}", self.infection));
        }
        if !self.carry_kg.is_finite() || self.carry_kg < CARRY_RANGE.0 || self.carry_kg > CARRY_RANGE.1 {
            return Err(format!("carry limit {} kg", self.carry_kg));
        }
        Ok(())
    }
}

/// Who a survivor was, as the player keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    /// Ids, as the data files name them (empty: a plain survivor).
    pub background: String,
    pub strengths: Vec<String>,
    pub flaws: Vec<String>,
    /// Starting levels on top of every survivor's own, a skill at most once.
    pub skills: Vec<(Skill, u32)>,
    pub perks: Perks,
}

impl Origin {
    /// No Origin: a survivor as they all were before there were any.
    pub fn is_plain(&self) -> bool {
        *self == Origin::default()
    }

    /// A new survivor's XP: every survivor's start, plus this Origin's levels.
    pub fn start_skills(&self, t: &SkillTuning) -> Skills {
        let mut skills = Skills::start(t);
        for &(s, levels) in &self.skills {
            let level = (skills.level(s, t) + levels).min(t.top());
            skills.xp[s as usize] = t.xp_for(level);
        }
        skills
    }

    /// A save's Origin: ids of a sane length, levels and perks in range.
    pub fn check(&self) -> Result<(), String> {
        let ids = std::iter::once(&self.background).chain(&self.strengths).chain(&self.flaws);
        if self.strengths.len() > MAX_PICKS || self.flaws.len() > MAX_PICKS || ids.clone().any(|id| id.len() > 64) {
            return Err("too many or too long ids".into());
        }
        if self.skills.len() > SKILL_COUNT || self.skills.iter().any(|&(_, n)| n > MAX_START_LEVELS) {
            return Err("starting skill levels out of range".into());
        }
        self.perks.check()
    }

    /// Mix what the future depends on into a hash.
    pub(crate) fn hash(&self, mix: &mut impl FnMut(u32)) {
        mix(0x4F52_4947);
        for &(s, n) in &self.skills {
            mix((s as u32) << 8 | n);
        }
        for m in self.perks.mults() {
            mix(m.to_bits());
        }
        mix(self.perks.infection.to_bits());
        mix(self.perks.carry_kg.to_bits());
    }
}

/// An item id and how many.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kit {
    pub item: String,
    #[serde(default = "one")]
    pub count: u16,
}

fn one() -> u16 {
    1
}

/// What a survivor did before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackgroundDef {
    /// Lower-case letters, digits and `_`.
    pub id: String,
    pub name: String,
    /// A line of flavour.
    pub about: String,
    /// Points to spend on Strengths (a plain Background gives the most).
    pub points: i32,
    /// Starting levels, on top of every survivor's own.
    #[serde(default)]
    pub skills: Vec<(Skill, u32)>,
    /// XP multipliers per skill.
    #[serde(default)]
    pub xp: Vec<(Skill, f32)>,
    /// What's in the pockets on the first morning.
    #[serde(default)]
    pub items: Vec<Kit>,
}

/// A Strength (costs points) or a Flaw (gives them).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickDef {
    pub id: String,
    pub name: String,
    pub about: String,
    /// Points a Strength costs, or a Flaw gives: 1 or more either way.
    pub points: i32,
    pub effects: Vec<Effect>,
    /// Ids (of Strengths or Flaws) this can't be taken with. One side naming the other is enough.
    #[serde(default)]
    pub excludes: Vec<String>,
}

/// One place of the starting wardrobe: a label and the item ids to choose from (or nothing).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WardrobeSlot {
    pub name: String,
    pub items: Vec<String>,
    /// Worn when nothing was chosen (the screen's first look); `None`: nothing.
    #[serde(default)]
    pub default: Option<String>,
}

/// `strengths_and_flaws.ron`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PicksFile {
    pub strengths: Vec<PickDef>,
    pub flaws: Vec<PickDef>,
    #[serde(default)]
    pub wardrobe: Vec<WardrobeSlot>,
}

/// Both files, checked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OriginData {
    pub backgrounds: Vec<BackgroundDef>,
    pub strengths: Vec<PickDef>,
    pub flaws: Vec<PickDef>,
    pub wardrobe: Vec<WardrobeSlot>,
}

/// What a player picked, by id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Choice {
    pub background: String,
    pub strengths: Vec<String>,
    pub flaws: Vec<String>,
    /// Clothes from the wardrobe, at most one a slot.
    pub outfit: Vec<String>,
}

/// A resolved [`Choice`]: the survivor, and what it wears and carries on the first morning.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Start {
    pub origin: Origin,
    pub outfit: Vec<String>,
    pub items: Vec<(String, u16)>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn check_mult(what: &str, m: f32) -> Result<(), String> {
    if !m.is_finite() || m < MULT_RANGE.0 || m > MULT_RANGE.1 {
        return Err(format!("{what}: a multiplier must be {} to {}, not {m}", MULT_RANGE.0, MULT_RANGE.1));
    }
    Ok(())
}

fn check_effect(e: &Effect) -> Result<(), String> {
    match *e {
        Effect::Skill(s, n) if n == 0 || n > MAX_START_LEVELS => Err(format!("{} levels must be 1 to {MAX_START_LEVELS}, not {n}", s.name())),
        Effect::Skill(..) => Ok(()),
        Effect::Carry(kg) if !kg.is_finite() || kg == 0.0 || kg < CARRY_RANGE.0 || kg > CARRY_RANGE.1 => Err(format!("carry must be {} to {} kg and not 0, not {kg}", CARRY_RANGE.0, CARRY_RANGE.1)),
        Effect::Carry(_) => Ok(()),
        Effect::Infection(m) if !m.is_finite() || !(0.0..=MULT_RANGE.1).contains(&m) => Err(format!("infection must be 0 to {}, not {m}", MULT_RANGE.1)),
        Effect::Infection(_) => Ok(()),
        Effect::Xp(s, m) => check_mult(&format!("{} XP", s.name()), m),
        Effect::Hunger(m) | Effect::Thirst(m) | Effect::Tiredness(m) | Effect::RunDrain(m) | Effect::Recovery(m) | Effect::SeenFrom(m) | Effect::Footsteps(m) | Effect::Healing(m) | Effect::Bleeding(m) => check_mult("an effect", m),
    }
}

/// Read `backgrounds.ron`.
pub fn parse_backgrounds(text: &str) -> Result<Vec<BackgroundDef>, String> {
    ron::from_str(text).map_err(|e| e.to_string())
}

/// Read `strengths_and_flaws.ron`.
pub fn parse_picks(text: &str) -> Result<PicksFile, String> {
    ron::from_str(text).map_err(|e| e.to_string())
}

impl OriginData {
    /// Put the two files together and check them against the items: ids, names, points, effects,
    /// every `excludes` naming a Strength or Flaw there is, every item an item, every wardrobe
    /// item clothing. Errors name the entry.
    pub fn new(backgrounds: Vec<BackgroundDef>, picks: PicksFile, items: &[ItemDef]) -> Result<Self, String> {
        let data = OriginData { backgrounds, strengths: picks.strengths, flaws: picks.flaws, wardrobe: picks.wardrobe };
        let item = |id: &str| items.iter().find(|d| d.id == id);
        for (k, b) in data.backgrounds.iter().enumerate() {
            let at = format!("background {:?}", b.id);
            if !valid_id(&b.id) {
                return Err(format!("{at}: an id is lower-case letters, digits and _"));
            }
            if data.backgrounds[..k].iter().any(|o| o.id == b.id) {
                return Err(format!("{at}: listed twice"));
            }
            if b.name.trim().is_empty() || b.about.trim().is_empty() {
                return Err(format!("{at}: needs a name and a line about it"));
            }
            if !(0..=40).contains(&b.points) {
                return Err(format!("{at}: points must be 0 to 40, not {}", b.points));
            }
            for (j, &(s, n)) in b.skills.iter().enumerate() {
                if b.skills[..j].iter().any(|o| o.0 == s) {
                    return Err(format!("{at}: {} listed twice", s.name()));
                }
                check_effect(&Effect::Skill(s, n)).map_err(|e| format!("{at}: {e}"))?;
            }
            for (j, &(s, m)) in b.xp.iter().enumerate() {
                if b.xp[..j].iter().any(|o| o.0 == s) {
                    return Err(format!("{at}: {} XP listed twice", s.name()));
                }
                check_effect(&Effect::Xp(s, m)).map_err(|e| format!("{at}: {e}"))?;
            }
            if b.items.len() > 8 {
                return Err(format!("{at}: at most 8 kinds of item"));
            }
            for k in &b.items {
                if item(&k.item).is_none() {
                    return Err(format!("{at}: no item {:?}", k.item));
                }
                if !(1..=20).contains(&k.count) {
                    return Err(format!("{at}: {} x {:?} (1 to 20)", k.count, k.item));
                }
            }
        }
        let all: Vec<&PickDef> = data.strengths.iter().chain(&data.flaws).collect();
        for (k, p) in all.iter().enumerate() {
            let kind = if k < data.strengths.len() { "strength" } else { "flaw" };
            let at = format!("{kind} {:?}", p.id);
            if !valid_id(&p.id) {
                return Err(format!("{at}: an id is lower-case letters, digits and _"));
            }
            if all[..k].iter().any(|o| o.id == p.id) {
                return Err(format!("{at}: listed twice (a Strength and a Flaw can't share an id)"));
            }
            if p.name.trim().is_empty() || p.about.trim().is_empty() {
                return Err(format!("{at}: needs a name and a line about it"));
            }
            if !(1..=20).contains(&p.points) {
                return Err(format!("{at}: points must be 1 to 20, not {}", p.points));
            }
            if p.effects.is_empty() || p.effects.len() > 6 {
                return Err(format!("{at}: 1 to 6 effects"));
            }
            for e in &p.effects {
                check_effect(e).map_err(|e| format!("{at}: {e}"))?;
            }
            for x in &p.excludes {
                if *x == p.id || !all.iter().any(|o| o.id == *x) {
                    return Err(format!("{at}: excludes {x:?}, which isn't another Strength or Flaw"));
                }
            }
        }
        for (k, slot) in data.wardrobe.iter().enumerate() {
            let at = format!("wardrobe {:?}", slot.name);
            if slot.name.trim().is_empty() || slot.items.is_empty() || data.wardrobe[..k].iter().any(|o| o.name == slot.name) {
                return Err(format!("{at}: needs a name of its own and something to choose"));
            }
            for id in &slot.items {
                if item(id).is_none_or(|d| d.clothing.is_none()) {
                    return Err(format!("{at}: {id:?} isn't clothing in items.ron"));
                }
                if data.wardrobe[..k].iter().any(|o| o.items.contains(id)) {
                    return Err(format!("{at}: {id:?} is in another slot too"));
                }
            }
            if slot.default.as_ref().is_some_and(|d| !slot.items.contains(d)) {
                return Err(format!("{at}: its default isn't one of its items"));
            }
        }
        // What's worn when nothing is chosen must go on together.
        let defaults: Vec<String> = data.wardrobe.iter().filter_map(|s| s.default.clone()).collect();
        data.check_outfit(&defaults, items).map_err(|e| format!("wardrobe defaults: {e}"))?;
        Ok(data)
    }

    pub fn background(&self, id: &str) -> Option<&BackgroundDef> {
        self.backgrounds.iter().find(|b| b.id == id)
    }

    pub fn strength(&self, id: &str) -> Option<&PickDef> {
        self.strengths.iter().find(|p| p.id == id)
    }

    pub fn flaw(&self, id: &str) -> Option<&PickDef> {
        self.flaws.iter().find(|p| p.id == id)
    }

    fn pick(&self, id: &str) -> Option<&PickDef> {
        self.strength(id).or_else(|| self.flaw(id))
    }

    /// Can't `a` and `b` (Strengths or Flaws) be taken together?
    pub fn excluded(&self, a: &str, b: &str) -> bool {
        let names = |x: &str, y: &str| self.pick(x).is_some_and(|p| p.excludes.iter().any(|e| e == y));
        names(a, b) || names(b, a)
    }

    /// Points left: the Background's, plus the Flaws', less the Strengths' (ids not in the data
    /// count for nothing; [`Self::resolve`] refuses them).
    pub fn points_left(&self, c: &Choice) -> i32 {
        let of = |ids: &[String], from: &[PickDef]| ids.iter().filter_map(|id| from.iter().find(|p| p.id == *id)).map(|p| p.points).sum::<i32>();
        self.background(&c.background).map_or(0, |b| b.points) + of(&c.flaws, &self.flaws) - of(&c.strengths, &self.strengths)
    }

    /// What a new game starts with before the player changes anything: the first Background, no
    /// Strengths or Flaws, the wardrobe's defaults.
    pub fn default_choice(&self) -> Choice {
        Choice { background: self.backgrounds.first().map(|b| b.id.clone()).unwrap_or_default(), strengths: Vec::new(), flaws: Vec::new(), outfit: self.wardrobe.iter().filter_map(|s| s.default.clone()).collect() }
    }

    /// Clothes from the wardrobe, at most one a slot, that can be worn together.
    fn check_outfit(&self, outfit: &[String], items: &[ItemDef]) -> Result<(), String> {
        let mut slots = vec![false; self.wardrobe.len()];
        let mut worn: Vec<&ItemDef> = Vec::new();
        for id in outfit {
            let slot = self.wardrobe.iter().position(|s| s.items.contains(id)).ok_or_else(|| format!("{id:?} isn't in the starting wardrobe"))?;
            if std::mem::replace(&mut slots[slot], true) {
                return Err(format!("two things for {}", self.wardrobe[slot].name));
            }
            let def = items.iter().find(|d| d.id == *id).filter(|d| d.clothing.is_some()).ok_or_else(|| format!("{id:?} isn't clothing"))?;
            if let Some(o) = worn.iter().find(|o| o.clothing.as_ref().zip(def.clothing.as_ref()).is_some_and(|(a, b)| a.clashes(b))) {
                return Err(format!("{} can't be worn with {}", def.name, o.name));
            }
            worn.push(def);
        }
        Ok(())
    }

    /// The one rule (see the module docs). The error is a sentence for the player.
    pub fn resolve(&self, c: &Choice, items: &[ItemDef]) -> Result<Start, String> {
        let b = self.background(&c.background).ok_or_else(|| format!("no Background {:?}", c.background))?;
        if c.strengths.len() > MAX_PICKS || c.flaws.len() > MAX_PICKS {
            return Err(format!("at most {MAX_PICKS} Strengths and {MAX_PICKS} Flaws"));
        }
        let mut picked: Vec<&PickDef> = Vec::new();
        for (ids, from, kind) in [(&c.strengths, &self.strengths, "Strength"), (&c.flaws, &self.flaws, "Flaw")] {
            for id in ids {
                let p = from.iter().find(|p| p.id == *id).ok_or_else(|| format!("no {kind} {id:?}"))?;
                if picked.iter().any(|o| o.id == p.id) {
                    return Err(format!("{} is picked twice", p.name));
                }
                if let Some(o) = picked.iter().find(|o| self.excluded(&o.id, &p.id)) {
                    return Err(format!("{} can't go with {}", p.name, o.name));
                }
                picked.push(p);
            }
        }
        let left = self.points_left(c);
        if left < 0 {
            return Err(format!("{} point{} over: drop a Strength or take a Flaw", -left, if left == -1 { "" } else { "s" }));
        }
        self.check_outfit(&c.outfit, items)?;
        let mut perks = Perks::default();
        let mut skills: Vec<(Skill, u32)> = Vec::new();
        let mut level = |s: Skill, n: u32| match skills.iter_mut().find(|e| e.0 == s) {
            Some(e) => e.1 = (e.1 + n).min(MAX_START_LEVELS),
            None => skills.push((s, n.min(MAX_START_LEVELS))),
        };
        for &(s, n) in &b.skills {
            level(s, n);
        }
        for &(s, m) in &b.xp {
            perks.add(&Effect::Xp(s, m));
        }
        for p in &picked {
            for e in &p.effects {
                match *e {
                    Effect::Skill(s, n) => level(s, n),
                    _ => perks.add(e),
                }
            }
        }
        skills.sort_by_key(|e| e.0);
        let origin = Origin { background: b.id.clone(), strengths: c.strengths.clone(), flaws: c.flaws.clone(), skills, perks: perks.clamped() };
        Ok(Start { origin, outfit: c.outfit.clone(), items: b.items.iter().map(|k| (k.item.clone(), k.count)).collect() })
    }
}

impl World {
    /// Make player `id`, just added, the survivor `start` describes: its Origin, its starting
    /// skills, its clothes on and its Background's things in its pockets. Clothes and items the
    /// world's content doesn't have are left out. Returns whether there is such a player.
    pub fn begin_as(&mut self, id: u32, start: &Start) -> bool {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return false };
        let skills = start.origin.start_skills(&self.config.skills);
        let worn: Vec<ItemStack> = start.outfit.iter().filter_map(|item| self.item_index(item)).filter(|&i| self.item_def(i).clothing.is_some()).map(ItemStack::one).collect();
        let p = &mut self.players[pi];
        p.origin = start.origin.clone();
        p.skills = skills;
        for s in worn {
            let clash = |a: u16, b: u16, items: &[ItemDef]| items[a as usize].clothing.as_ref().zip(items[b as usize].clothing.as_ref()).is_some_and(|(x, y)| x.clashes(y));
            if !p.inventory.worn.iter().any(|o| o.item == s.item || clash(o.item, s.item, &self.config.items)) {
                p.inventory.worn.push(s);
            }
        }
        for (item, count) in &start.items {
            // The Background's tool of the trade starts in the hand.
            let weapon = self.item_index(item).is_some_and(|i| self.item_def(i).weapon.is_some());
            if weapon && self.players[pi].inventory.hand.is_none() {
                self.hold_item(id, crate::inventory::Slot::Hand, item);
            } else {
                self.give_item(id, item, *count);
            }
        }
        self.refresh_carry(pi);
        true
    }

    /// Player `id`'s Origin (plain if it has none).
    pub fn origin_of(&self, id: u32) -> Option<&Origin> {
        self.player(id).map(|p| &p.origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{parse_items, parse_weapons};
    use crate::world::WorldConfig;

    fn items() -> Vec<ItemDef> {
        let weapons = parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap()
    }

    fn shipped() -> (OriginData, Vec<ItemDef>) {
        let items = items();
        let b = parse_backgrounds(include_str!("../../../assets/data/backgrounds.ron")).expect("backgrounds.ron");
        let p = parse_picks(include_str!("../../../assets/data/strengths_and_flaws.ron")).expect("strengths_and_flaws.ron");
        (OriginData::new(b, p, &items).expect("the shipped files"), items)
    }

    #[test]
    fn the_shipped_files_are_sound_and_every_background_can_start() {
        let (data, items) = shipped();
        assert!(data.backgrounds.len() >= 8 && data.strengths.len() >= 10 && data.flaws.len() >= 10);
        for b in &data.backgrounds {
            let c = Choice { background: b.id.clone(), ..data.default_choice() };
            let start = data.resolve(&c, &items).unwrap_or_else(|e| panic!("{}: {e}", b.id));
            assert_eq!(start.origin.background, b.id);
            assert!(start.origin.check().is_ok());
        }
        // Every Strength and every Flaw alone, on the plainest Background (the most points).
        let plain = data.backgrounds.iter().max_by_key(|b| b.points).unwrap();
        for s in &data.strengths {
            assert!(s.points <= plain.points, "{} can never be afforded alone", s.id);
            data.resolve(&Choice { background: plain.id.clone(), strengths: vec![s.id.clone()], ..Default::default() }, &items).unwrap();
        }
        for f in &data.flaws {
            data.resolve(&Choice { background: plain.id.clone(), flaws: vec![f.id.clone()], ..Default::default() }, &items).unwrap();
        }
    }

    #[test]
    fn points_exclusions_and_clothes_are_checked() {
        let (data, items) = shipped();
        let plain = data.backgrounds.iter().max_by_key(|b| b.points).unwrap().id.clone();
        let base = Choice { background: plain.clone(), ..Default::default() };
        // Too many Strengths for the points.
        let all: Vec<String> = data.strengths.iter().take(MAX_PICKS).map(|s| s.id.clone()).collect();
        let cost: i32 = data.strengths.iter().take(MAX_PICKS).map(|s| s.points).sum();
        let over = Choice { strengths: all, ..base.clone() };
        if cost > data.background(&plain).unwrap().points && !data.strengths.iter().take(MAX_PICKS).any(|a| data.strengths.iter().take(MAX_PICKS).any(|b| data.excluded(&a.id, &b.id))) {
            assert!(data.resolve(&over, &items).unwrap_err().contains("over"));
        }
        // An excluded pair, whichever side names the other.
        let (a, b) = data.strengths.iter().chain(&data.flaws).find_map(|p| p.excludes.first().map(|x| (p.id.clone(), x.clone()))).expect("the shipped set has an excluded pair");
        let side = |id: &str| if data.strength(id).is_some() { (vec![id.to_string()], vec![]) } else { (vec![], vec![id.to_string()]) };
        let (mut s, mut f) = side(&a);
        let (s2, f2) = side(&b);
        s.extend(s2);
        f.extend(f2);
        assert!(data.resolve(&Choice { strengths: s, flaws: f, ..base.clone() }, &items).unwrap_err().contains("can't go with"));
        // Unknown ids, a Strength twice, clothes that aren't in the wardrobe.
        assert!(data.resolve(&Choice { background: "nobody".into(), ..Default::default() }, &items).is_err());
        assert!(data.resolve(&Choice { strengths: vec!["no_such".into()], ..base.clone() }, &items).is_err());
        let cheap = data.strengths.iter().min_by_key(|s| s.points).unwrap().id.clone();
        assert!(data.resolve(&Choice { strengths: vec![cheap.clone(), cheap], ..base.clone() }, &items).unwrap_err().contains("twice"));
        assert!(data.resolve(&Choice { outfit: vec!["leather_jacket".into()], ..base.clone() }, &items).is_err() || data.wardrobe.iter().any(|s| s.items.iter().any(|i| i == "leather_jacket")));
        assert!(data.resolve(&Choice { outfit: vec!["tinned_beans".into()], ..base }, &items).is_err());
    }

    #[test]
    fn bad_files_are_refused_by_name() {
        let items = items();
        let bg = |points: i32| vec![BackgroundDef { id: "a".into(), name: "A".into(), about: "x".into(), points, skills: vec![], xp: vec![], items: vec![] }];
        let pick = |id: &str, effects: Vec<Effect>, excludes: Vec<&str>| PickDef { id: id.into(), name: id.into(), about: "x".into(), points: 2, effects, excludes: excludes.into_iter().map(String::from).collect() };
        let file = |strengths, flaws| PicksFile { strengths, flaws, wardrobe: vec![] };
        assert!(OriginData::new(bg(8), file(vec![pick("s", vec![Effect::Carry(2.0)], vec![])], vec![]), &items).is_ok());
        for (data, says) in [
            (OriginData::new(bg(-1), PicksFile::default(), &items), "points"),
            (OriginData::new(bg(8), file(vec![pick("s", vec![], vec![])], vec![]), &items), "effects"),
            (OriginData::new(bg(8), file(vec![pick("s", vec![Effect::Hunger(9.0)], vec![])], vec![]), &items), "multiplier"),
            (OriginData::new(bg(8), file(vec![pick("s", vec![Effect::Carry(1.0)], vec!["gone"])], vec![]), &items), "excludes"),
            (OriginData::new(bg(8), file(vec![pick("s", vec![Effect::Carry(1.0)], vec![])], vec![pick("s", vec![Effect::Carry(-1.0)], vec![])]), &items), "twice"),
            (OriginData::new(bg(8), PicksFile { wardrobe: vec![WardrobeSlot { name: "Top".into(), items: vec!["tinned_beans".into()], default: None }], ..Default::default() }, &items), "clothing"),
        ] {
            let e = data.unwrap_err();
            assert!(e.contains(says), "{e} should mention {says}");
        }
        assert!(parse_backgrounds("[(id: \"a\", name: \"A\", about: \"x\", points: 1, wealth: 3)]").is_err(), "unknown fields are refused");
    }

    #[test]
    fn a_survivor_starts_as_chosen_and_its_perks_apply() {
        let (data, items) = shipped();
        let plain = data.backgrounds.iter().max_by_key(|b| b.points).unwrap().id.clone();
        let data = OriginData {
            strengths: vec![PickDef { id: "t_strong".into(), name: "T".into(), about: "x".into(), points: 1, effects: vec![Effect::Carry(4.0), Effect::Skill(Skill::Strength, 1), Effect::Xp(Skill::Scavenging, 2.0), Effect::RunDrain(0.5)], excludes: vec![] }],
            flaws: vec![PickDef { id: "t_hungry".into(), name: "H".into(), about: "x".into(), points: 1, effects: vec![Effect::Hunger(2.0), Effect::SeenFrom(1.5)], excludes: vec![] }],
            ..data
        };
        let outfit: Vec<String> = data.wardrobe.iter().filter_map(|s| s.default.clone()).collect();
        let c = Choice { background: plain, strengths: vec!["t_strong".into()], flaws: vec!["t_hungry".into()], outfit: outfit.clone() };
        let start = data.resolve(&c, &items).unwrap();

        let mut config = WorldConfig::new(3, 0, Vec::new());
        config.items = items;
        let mut plain_world = World::new(config.clone());
        let mut world = World::new(config);
        let (a, b) = (plain_world.add_player(glam::Vec2::new(40.0, 40.0)), world.add_player(glam::Vec2::new(40.0, 40.0)));
        assert_eq!(plain_world.state_hash(), world.state_hash());
        assert!(world.begin_as(b, &start));
        assert_ne!(plain_world.state_hash(), world.state_hash(), "an Origin is in the hash");
        let (p, q) = (plain_world.player(a).unwrap().clone(), world.player(b).unwrap().clone());
        // Skills, clothes, carry limit, movement.
        let t = &world.config.skills;
        assert_eq!(q.skills.level(Skill::Strength, t), p.skills.level(Skill::Strength, t) + 1);
        assert_eq!(q.inventory.worn.len(), outfit.len());
        let more = world.config.carry.strength_kg + 4.0;
        assert!((world.carry(&q).limit_kg - plain_world.carry(&p).limit_kg - more).abs() < 1e-3);
        assert!((q.state.mods.run_drain / p.state.mods.run_drain - 0.5).abs() < 1e-3);
        // XP comes twice as fast in the one skill, hunger twice as fast.
        world.award_xp(0, Skill::Scavenging, 10.0);
        plain_world.award_xp(0, Skill::Scavenging, 10.0);
        world.award_xp(0, Skill::Stealth, 10.0);
        assert_eq!(world.players[0].skills.xp(Skill::Scavenging), 2.0 * plain_world.players[0].skills.xp(Skill::Scavenging));
        assert_eq!(world.players[0].skills.xp(Skill::Stealth), 10.0);
        world.skip_time(2.0);
        plain_world.skip_time(2.0);
        let (h, g) = (world.players[0].needs.hunger, plain_world.players[0].needs.hunger);
        assert!((h / g - 2.0).abs() < 0.05, "hunger {h} against {g}");

        // Saved and loaded: the same survivor, the same hash.
        let bytes = world.save();
        let loaded = World::load(&bytes, &world.config).expect("loads");
        assert_eq!(loaded.players[0].origin, world.players[0].origin);
        assert_eq!(loaded.state_hash(), world.state_hash());
        // A plain world's save has no such section and loads plain.
        let again = World::load(&plain_world.save(), &plain_world.config).unwrap();
        assert!(again.players[0].origin.is_plain());

        // A respawn is the same person: the Origin stays and its skills start over.
        world.players[0].hp = 0.0;
        world.players[0].dead = true;
        assert!(world.respawn_player(b));
        let r = &world.players[0];
        assert_eq!(r.origin, start.origin);
        assert_eq!(r.skills.xp(Skill::Scavenging), 0.0);
        assert_eq!(r.skills.level(Skill::Strength, &world.config.skills), world.config.skills.start_strength + 1);
    }
}
