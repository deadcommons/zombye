//! Needs (#11): hunger, thirst and tiredness, food going off, and sleep.
//!
//! Each need runs from 0 (fine) to 1 (worst) and rises per **game hour** ([`NeedsTuning`]; one game
//! hour is 150 s with the default 3,600 s day), so it follows the day length and `World::skip_time`.
//! Past three thresholds (mild, strong, severe) a need shows as a status tag and starts to bite:
//!
//! | Need | Mild | Strong | Severe |
//! |---|---|---|---|
//! | Hunger | *Peckish* | *Hungry*: no HP back, endurance recovers slower | *Starving*: loses HP, weaker swings |
//! | Thirst | *Dry* | *Thirsty*: endurance recovers slower | *Parched*: loses HP, no running |
//! | Tiredness | *Drowsy* | *Tired*: slower swings, narrower sight | *Exhausted*: slower still, walks slower |
//!
//! Movement effects go into [`MoveMods`] (`World::refresh_carry` calls [`World::needs_mods`]), so
//! the snapshot carries them and client prediction stays exact; HP lost to hunger and thirst goes
//! through `World::hurt_player`, so god mode holds.
//!
//! **Eating and drinking** is the `Use` item job (`crate::inventory`): it takes 2-5 s by how much the
//! item does, takes one go off the top item of the stack (`ItemStack::used`), and lowers hunger and
//! thirst by the item's amounts (a negative amount raises it: crackers make you thirsty). Food eaten
//! while hunger is under `waste_below` does nothing for hunger. Food poisoning means no eating.
//!
//! **Spoiling:** an item with `spoils_hours` goes [`Freshness::Fresh`] → `Stale` (at that age) →
//! `Mouldy` (at twice it), by its stack's `born` minute and the world's game clock
//! ([`World::game_minute`]): no per-tick work. Stale food gives less and may poison you, mouldy food
//! gives much less and usually does (`HealthTuning::poison_stale` / `poison_rotten`, through
//! `World::poison_player`). Food found in furniture was made a seeded while before the world began.
//!
//! **Sleep** (`World::player_sleep`): on a bed or a sofa within reach, else the floor (you wake up
//! sore). Tiredness falls instead of rising, hunger and thirst rise at half speed, HP comes back
//! faster (`HealthTuning::regen_asleep_per_h`), and a sleeper doesn't move. You wake when rested,
//! when hit, when a sound reaches you (within `wake_noise_m`), when a zombie sees you, in agony, or
//! after an hour asleep starving or parched; or when you get up (`World::wake_player`, or any action
//! through `net::apply_intent`). While every living player is asleep, game time runs
//! `sleep_time_scale` (30) times as fast: the clock, needs, wounds and illness (not the zombies, who
//! keep to real time). Single-player that's whenever you sleep; with others, only once everyone is.
//!
//! **Status tags** ([`status_tags`]): the eight groups of the design (hunger, thirst, tiredness,
//! endurance, load, pain, bleeding, sick) plus "Asleep", each with a level and a tooltip, from the
//! needs and what the player's own client already knows, so the HUD and the MCP tools say the same.
//!
//! Every number is ours. Needs, sleep and soreness are saved with the player and in `state_hash`.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::TICK_DT;
use crate::health::InfectionStage;
use crate::inventory::{Burden, ItemNote, MAX_ITEM_NOTES};
use crate::items::{ItemDef, ItemStack};
use crate::players::{MoveMods, Stance};
use crate::rng::hash01;
use crate::senses::SoundSource;
use crate::world::World;
use crate::zombies::Tier;

/// Longest step needs take when time is skipped (game hours).
const SKIP_STEP_H: f32 = 0.1;
const FOOD_AGE_SALT: u64 = 0xF00D_A6E0;
/// A bed or sofa this close (to the nearest edge of its footprint, m), in your room, is slept on.
pub const SLEEP_REACH: f32 = 1.0;
/// Most the time scale may be (a saved setting past it is clamped).
pub const MAX_TIME_SCALE: f32 = 100.0;

/// How bad a need is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Level {
    Fine,
    Mild,
    Strong,
    Severe,
}

/// What a player sleeps on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum SleepSpot {
    Bed,
    Sofa,
    /// Anywhere else: slow to rest, and you wake up sore.
    Floor,
}

impl SleepSpot {
    pub fn name(self) -> &'static str {
        match self {
            Self::Bed => "bed",
            Self::Sofa => "sofa",
            Self::Floor => "floor",
        }
    }
}

/// A player asleep.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sleep {
    pub spot: SleepSpot,
    /// Game hours asleep so far.
    pub hours: f32,
}

/// Why a sleeper woke up (`World::woke`, per tick).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeReason {
    /// Tiredness fell below `rested_below`.
    Rested,
    /// A zombie's strike landed (in god mode too).
    Hit,
    /// A sound reached the sleeper (a thump, a break, another player, a groan close by...).
    Noise,
    /// A zombie saw the sleeper.
    Seen,
    /// Agony.
    Pain,
    /// An hour or more asleep while starving, or parched.
    Hunger,
    Thirst,
    /// Got up (asked to, or did something).
    GotUp,
    Died,
}

impl WakeReason {
    pub fn text(self) -> &'static str {
        match self {
            Self::Rested => "rested",
            Self::Hit => "something hit you",
            Self::Noise => "a noise woke you",
            Self::Seen => "a zombie saw you",
            Self::Pain => "the pain woke you",
            Self::Hunger => "hunger woke you",
            Self::Thirst => "thirst woke you",
            Self::GotUp => "you got up",
            Self::Died => "you died",
        }
    }
}

/// One player's needs. Saved with the player.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Needs {
    /// 0 (fed) to 1 (starving).
    pub hunger: f32,
    /// 0 to 1 (parched).
    pub thirst: f32,
    /// 0 (rested) to 1 (exhausted).
    pub tiredness: f32,
    /// Asleep, and on what.
    pub sleep: Option<Sleep>,
    /// Game hours of soreness left from a night on the floor (adds `floor_sore_pain`).
    pub sore_h: f32,
}

impl Needs {
    pub fn asleep(&self) -> bool {
        self.sleep.is_some()
    }

    pub fn hunger_level(&self, t: &NeedsTuning) -> Level {
        t.level(self.hunger)
    }

    pub fn thirst_level(&self, t: &NeedsTuning) -> Level {
        t.level(self.thirst)
    }

    pub fn tiredness_level(&self, t: &NeedsTuning) -> Level {
        t.level(self.tiredness)
    }

    /// Hungry or worse: HP doesn't come back.
    pub fn stops_healing(&self, t: &NeedsTuning) -> bool {
        self.hunger_level(t) >= Level::Strong
    }

    /// Pain from soreness.
    pub fn sore_pain(&self, t: &NeedsTuning) -> f32 {
        if self.sore_h > 0.0 { t.floor_sore_pain } else { 0.0 }
    }

    pub fn view(&self, t: &NeedsTuning) -> NeedsView {
        let r = |v: f32| (v * 100.0).round() / 100.0;
        NeedsView { hunger: r(self.hunger), thirst: r(self.thirst), tiredness: r(self.tiredness), asleep: self.sleep.map(|s| s.spot), sore: self.sore_pain(t) }
    }
}

/// A player's needs as its own client (and an agent's tools) see them: rounded to 1%.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NeedsView {
    pub hunger: f32,
    pub thirst: f32,
    pub tiredness: f32,
    pub asleep: Option<SleepSpot>,
    /// Pain from soreness, on top of the wounds' (`HealthView::pain`).
    pub sore: f32,
}

/// How far gone food is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Freshness {
    Fresh,
    Stale,
    Mouldy,
}

impl Freshness {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Mouldy => "mouldy",
        }
    }
}

/// How fresh a stack of `def` is at game minute `now` (`None`: it doesn't spoil).
pub fn freshness(def: &ItemDef, s: &ItemStack, now: i32) -> Option<Freshness> {
    let h = def.consumable()?.spoils_hours?;
    let age_h = (now as f64 - s.born as f64) / 60.0;
    Some(if age_h < h as f64 {
        Freshness::Fresh
    } else if age_h < 2.0 * h as f64 {
        Freshness::Stale
    } else {
        Freshness::Mouldy
    })
}

/// Every needs and sleep number. Serde with defaults, saved with the world's settings. Rates per
/// game hour, needs 0..1. Ours.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NeedsTuning {
    pub hunger_per_h: f32,
    pub hunger_asleep_per_h: f32,
    pub thirst_per_h: f32,
    pub thirst_asleep_per_h: f32,
    /// Extra thirst per game hour spent running, and per swing.
    pub run_thirst_per_h: f32,
    pub swing_thirst: f32,
    pub tired_per_h: f32,
    /// Tiredness lost per game hour asleep on a bed, a sofa, the floor.
    pub rest_bed_per_h: f32,
    pub rest_sofa_per_h: f32,
    pub rest_floor_per_h: f32,

    /// Where a need turns mild, strong and severe.
    pub mild: f32,
    pub strong: f32,
    pub severe: f32,

    /// Endurance recovery while hungry, thirsty (multipliers; both can apply).
    pub hungry_regen: f32,
    pub thirsty_regen: f32,
    /// HP per game hour starving, parched.
    pub starving_hp_per_h: f32,
    pub parched_hp_per_h: f32,
    /// Swing power while starving; swing time tired and exhausted; walking speed exhausted.
    pub starving_swing_power: f32,
    pub tired_swing_time: f32,
    pub exhausted_swing_time: f32,
    pub exhausted_walk: f32,
    /// The client's view cone while tired or worse (a multiplier on its angle).
    pub tired_sight: f32,

    /// Pain levels (sore, hurting, agony) and swing power at each.
    pub pain_sore: f32,
    pub pain_hurting: f32,
    pub pain_agony: f32,
    pub pain_swing_power: (f32, f32, f32),

    /// Eating or drinking takes `eat_secs.0` plus this per unit of hunger and thirst it does, at most `eat_secs.1`.
    pub eat_secs: (f32, f32),
    pub eat_secs_per: f32,
    /// Food eaten with hunger under this does nothing for hunger.
    pub waste_below: f32,
    /// Share of the hunger and thirst stale and mouldy food gives.
    pub stale_food: f32,
    pub mouldy_food: f32,
    /// Food found in furniture was made this share of its `spoils_hours` before the world began
    /// (each stack rolls within the range).
    pub found_food_age: (f32, f32),

    /// No sleeping with tiredness below this; asleep, you wake once it's below `rested_below`.
    pub min_tiredness_to_sleep: f32,
    pub rested_below: f32,
    /// A sound this close (and loud enough to reach you) wakes you.
    pub wake_noise_m: f32,
    /// Starving or parched, you wake after this many game hours asleep.
    pub hunger_wakes_after_h: f32,
    /// A night on the floor: pain for this many game hours.
    pub floor_sore_pain: f32,
    pub floor_sore_h: f32,
    /// Game time runs this many times as fast while every living player is asleep (1: never).
    pub sleep_time_scale: f32,
}

impl Default for NeedsTuning {
    fn default() -> Self {
        Self {
            hunger_per_h: 0.04,
            hunger_asleep_per_h: 0.02,
            thirst_per_h: 0.06,
            thirst_asleep_per_h: 0.03,
            run_thirst_per_h: 0.02,
            swing_thirst: 0.002,
            tired_per_h: 0.05,
            rest_bed_per_h: 0.12,
            rest_sofa_per_h: 0.09,
            rest_floor_per_h: 0.06,

            mild: 0.35,
            strong: 0.6,
            severe: 0.85,

            hungry_regen: 0.85,
            thirsty_regen: 0.75,
            starving_hp_per_h: 2.0,
            parched_hp_per_h: 3.0,
            starving_swing_power: 0.85,
            tired_swing_time: 1.1,
            exhausted_swing_time: 1.25,
            exhausted_walk: 0.9,
            tired_sight: 0.85,

            pain_sore: 0.2,
            pain_hurting: 0.45,
            pain_agony: 0.75,
            pain_swing_power: (0.95, 0.85, 0.7),

            eat_secs: (2.0, 5.0),
            eat_secs_per: 8.0,
            waste_below: 0.1,
            stale_food: 0.7,
            mouldy_food: 0.4,
            found_food_age: (0.0, 0.6),

            min_tiredness_to_sleep: 0.3,
            rested_below: 0.05,
            wake_noise_m: 20.0,
            hunger_wakes_after_h: 1.0,
            floor_sore_pain: 0.1,
            floor_sore_h: 2.0,
            sleep_time_scale: 30.0,
        }
    }
}

impl NeedsTuning {
    /// Needs that never rise (a sandbox without them, and tests of other things over long skips).
    pub fn off() -> Self {
        Self { hunger_per_h: 0.0, hunger_asleep_per_h: 0.0, thirst_per_h: 0.0, thirst_asleep_per_h: 0.0, run_thirst_per_h: 0.0, swing_thirst: 0.0, tired_per_h: 0.0, ..Self::default() }
    }

    pub fn level(&self, v: f32) -> Level {
        if v >= self.severe {
            Level::Severe
        } else if v >= self.strong {
            Level::Strong
        } else if v >= self.mild {
            Level::Mild
        } else {
            Level::Fine
        }
    }

    /// Tiredness lost per game hour asleep here.
    pub fn rest_per_h(&self, spot: SleepSpot) -> f32 {
        match spot {
            SleepSpot::Bed => self.rest_bed_per_h,
            SleepSpot::Sofa => self.rest_sofa_per_h,
            SleepSpot::Floor => self.rest_floor_per_h,
        }
    }

    /// 0 none, 1 sore, 2 hurting, 3 agony.
    pub fn pain_level(&self, pain: f32) -> u8 {
        [self.pain_sore, self.pain_hurting, self.pain_agony].iter().filter(|&&p| pain >= p).count() as u8
    }

    /// Swing power at this pain.
    pub fn pain_power(&self, pain: f32) -> f32 {
        match self.pain_level(pain) {
            0 => 1.0,
            1 => self.pain_swing_power.0,
            2 => self.pain_swing_power.1,
            _ => self.pain_swing_power.2,
        }
    }

    /// Settings a tick can't work with, by name (checked on load).
    pub fn check(&self) -> Result<(), String> {
        if !(0.0 < self.mild && self.mild < self.strong && self.strong < self.severe && self.severe <= 1.0) {
            return Err(format!("needs: thresholds must rise within 0..1 (mild {}, strong {}, severe {})", self.mild, self.strong, self.severe));
        }
        if !(0.0 < self.pain_sore && self.pain_sore < self.pain_hurting && self.pain_hurting < self.pain_agony) {
            return Err("needs: pain levels must rise from above 0".into());
        }
        let rates = [
            ("hunger_per_h", self.hunger_per_h),
            ("hunger_asleep_per_h", self.hunger_asleep_per_h),
            ("thirst_per_h", self.thirst_per_h),
            ("thirst_asleep_per_h", self.thirst_asleep_per_h),
            ("run_thirst_per_h", self.run_thirst_per_h),
            ("swing_thirst", self.swing_thirst),
            ("tired_per_h", self.tired_per_h),
            ("rest_bed_per_h", self.rest_bed_per_h),
            ("rest_sofa_per_h", self.rest_sofa_per_h),
            ("rest_floor_per_h", self.rest_floor_per_h),
            ("starving_hp_per_h", self.starving_hp_per_h),
            ("parched_hp_per_h", self.parched_hp_per_h),
            ("eat_secs_per", self.eat_secs_per),
            ("floor_sore_h", self.floor_sore_h),
            ("hunger_wakes_after_h", self.hunger_wakes_after_h),
        ];
        if let Some((name, v)) = rates.iter().find(|(_, v)| !(v.is_finite() && *v >= 0.0)) {
            return Err(format!("needs.{name} is {v} (must be 0 or more)"));
        }
        if !(self.eat_secs.0 >= 0.0 && self.eat_secs.1 >= self.eat_secs.0 && self.eat_secs.1 < 600.0) {
            return Err(format!("needs.eat_secs {:?} must be a range from 0 up (under 600 s)", self.eat_secs));
        }
        let (lo, hi) = self.found_food_age;
        if !(0.0 <= lo && lo <= hi && hi <= 10.0) {
            return Err(format!("needs.found_food_age {:?} must be a range within 0..10", self.found_food_age));
        }
        if !(1.0..=MAX_TIME_SCALE).contains(&self.sleep_time_scale) {
            return Err(format!("needs.sleep_time_scale is {} (1 to {MAX_TIME_SCALE})", self.sleep_time_scale));
        }
        Ok(())
    }
}

/// What kind of status a tag is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TagKind {
    Hunger,
    Thirst,
    Tiredness,
    Endurance,
    Load,
    Pain,
    Bleeding,
    Sick,
    Asleep,
}

/// One status tag: a short label, how bad (1 mild to 3 severe) and what it does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusTag {
    pub kind: TagKind,
    pub label: String,
    pub severity: u8,
    pub tip: String,
}

/// What the status tags are worked out from: all of it is in the player's own client's views.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatusInput {
    pub needs: NeedsView,
    /// The wounds' pain (`HealthView::pain`); soreness is added from `needs`.
    pub pain: f32,
    pub bleeding: usize,
    pub food_poisoned: bool,
    pub festering: bool,
    pub infection: Option<InfectionStage>,
    pub endurance: f32,
    pub winded: bool,
    /// `CombatTuning::tired_below`.
    pub tired_below: f32,
    pub burden: Burden,
    /// The game time scale now (`World::time_scale`).
    pub time_scale: f32,
}

fn pct(m: f32) -> String {
    format!("{:.0}%", ((1.0 - m) * 100.0).abs())
}

/// The status tags that apply, in the HUD's order.
pub fn status_tags(t: &NeedsTuning, s: &StatusInput) -> Vec<StatusTag> {
    let mut out = Vec::new();
    let mut tag = |kind, label: &str, severity: u8, tip: String| out.push(StatusTag { kind, label: label.to_string(), severity, tip });
    let n = &s.needs;
    match t.level(n.hunger) {
        Level::Fine => {}
        Level::Mild => tag(TagKind::Hunger, "Peckish", 1, "Peckish: getting hungry. Nothing yet.".into()),
        Level::Strong => tag(TagKind::Hunger, "Hungry", 2, format!("Hungry: health doesn't come back; endurance recovers {} slower. Eat something.", pct(t.hungry_regen))),
        Level::Severe => tag(TagKind::Hunger, "Starving", 3, format!("Starving: losing {:.0} HP a game hour; swings {} weaker. Eat now.", t.starving_hp_per_h, pct(t.starving_swing_power))),
    }
    match t.level(n.thirst) {
        Level::Fine => {}
        Level::Mild => tag(TagKind::Thirst, "Dry", 1, "Dry: getting thirsty. Nothing yet.".into()),
        Level::Strong => tag(TagKind::Thirst, "Thirsty", 2, format!("Thirsty: endurance recovers {} slower. Drink something.", pct(t.thirsty_regen))),
        Level::Severe => tag(TagKind::Thirst, "Parched", 3, format!("Parched: losing {:.0} HP a game hour; no running. Drink now.", t.parched_hp_per_h)),
    }
    match t.level(n.tiredness) {
        Level::Fine => {}
        Level::Mild => tag(TagKind::Tiredness, "Drowsy", 1, "Drowsy: you could sleep. Nothing yet.".into()),
        Level::Strong => tag(TagKind::Tiredness, "Tired", 2, format!("Tired: swings {} slower; you see a narrower cone. Sleep (B).", pct(t.tired_swing_time))),
        Level::Severe => tag(TagKind::Tiredness, "Exhausted", 3, format!("Exhausted: swings {} slower; walking {} slower. Sleep (B).", pct(t.exhausted_swing_time), pct(t.exhausted_walk))),
    }
    if s.winded {
        tag(TagKind::Endurance, "Winded", 3, "Winded: out of breath, no running until you get it back.".into());
    } else if s.endurance < s.tired_below {
        tag(TagKind::Endurance, "Short of breath", 1, "Short of breath: swings are weaker and slower until your endurance recovers.".into());
    }
    match s.burden {
        Burden::None => {}
        Burden::Burdened => tag(TagKind::Load, "Burdened", 1, "Burdened: a little over your limit: slower, running tires you faster.".into()),
        Burden::Heavy => tag(TagKind::Load, "Heavy", 2, "Heavy: well over your limit: slower still, no running, actions cost more.".into()),
        Burden::Overloaded => tag(TagKind::Load, "Overloaded", 3, "Overloaded: far over your limit: very slow, no running or climbing.".into()),
    }
    let pain = s.pain + n.sore;
    match t.pain_level(pain) {
        0 => {}
        1 => tag(TagKind::Pain, "Sore", 1, format!("Sore: swings {} weaker.", pct(t.pain_swing_power.0))),
        2 => tag(TagKind::Pain, "Hurting", 2, format!("Hurting: swings {} weaker.", pct(t.pain_swing_power.1))),
        _ => tag(TagKind::Pain, "Agony", 3, format!("Agony: swings {} weaker; you can't sleep.", pct(t.pain_swing_power.2))),
    }
    match s.bleeding {
        0 => {}
        1 => tag(TagKind::Bleeding, "Bleeding", 3, "Bleeding: losing HP until it's dressed (H for your wounds).".into()),
        k => tag(TagKind::Bleeding, &format!("Bleeding x{k}"), 3, format!("Bleeding from {k} wounds: losing HP fast until they're dressed (H for your wounds).")),
    }
    match s.infection {
        Some(InfectionStage::Turning) => tag(TagKind::Sick, "Turning", 3, "Turning: losing HP fast; your night sight is going.".into()),
        Some(InfectionStage::Feverish) => tag(TagKind::Sick, "Feverish", 3, "Feverish: losing HP, no healing, slow to get your breath back.".into()),
        _ if s.festering => tag(TagKind::Sick, "Sick", 2, "Sick: a wound is festering: it hurts, heals slowly and costs HP.".into()),
        _ if s.food_poisoned => tag(TagKind::Sick, "Queasy", 2, "Queasy: food poisoning: you can't keep food down, you get thirsty fast.".into()),
        _ => {}
    }
    if let Some(spot) = n.asleep {
        let label = if s.time_scale > 1.0 { format!("Asleep x{:.0}", s.time_scale) } else { "Asleep".to_string() };
        let fast = if s.time_scale > 1.0 { format!(" Time runs {:.0}x as fast.", s.time_scale) } else { String::new() };
        tag(TagKind::Asleep, &label, 1, format!("Asleep on the {}: resting, healing faster.{fast} Move or press B to get up.", spot.name()));
    }
    out
}

/// The status tags as one line ("Hungry, Tired, Bleeding").
pub fn tags_line(tags: &[StatusTag]) -> String {
    tags.iter().map(|t| t.label.as_str()).collect::<Vec<_>>().join(", ")
}

impl World {
    /// Game hours since tick 0 (time skips and sped-up sleep included). Never wraps.
    pub fn game_hours(&self) -> f64 {
        self.tick as f64 * TICK_DT as f64 / self.config.day_length_secs.max(1.0) as f64 * 24.0 + self.clock_offset_hours as f64
    }

    /// The game clock in whole minutes since tick 0 (what `ItemStack::born` counts in).
    pub fn game_minute(&self) -> i32 {
        (self.game_hours() * 60.0).floor() as i32
    }

    /// How many times as fast game time runs now: `sleep_time_scale` while every living player is
    /// asleep (and there is one), else 1.
    pub fn time_scale(&self) -> f32 {
        let scale = self.config.needs.sleep_time_scale.clamp(1.0, MAX_TIME_SCALE);
        let mut living = self.players.iter().filter(|p| !p.dead).peekable();
        if scale > 1.0 && living.peek().is_some() && living.all(|p| p.needs.asleep()) { scale } else { 1.0 }
    }

    /// `count` of `item` made now: fresh if it spoils.
    pub(crate) fn fresh_stack(&self, item: u16, count: u16) -> ItemStack {
        let spoils = self.config.items.get(item as usize).and_then(|d| d.consumable()).is_some_and(|c| c.spoils_hours.is_some());
        ItemStack { born: if spoils { self.game_minute() } else { 0 }, ..ItemStack::new(item, count) }
    }

    /// How fresh a stack is now (`None`: it doesn't spoil).
    pub fn freshness_of(&self, s: &ItemStack) -> Option<Freshness> {
        freshness(self.config.items.get(s.item as usize)?, s, self.game_minute())
    }

    /// Food just found in container `c` (filled from the seed): made a seeded while before the world
    /// began, so it goes off in its own time whenever it's found.
    pub(crate) fn age_found_food(&self, c: u32, stacks: &mut [ItemStack]) {
        let (lo, hi) = self.config.needs.found_food_age;
        let seed = self.config.seed() ^ FOOD_AGE_SALT;
        for (k, s) in stacks.iter_mut().enumerate() {
            let Some(h) = self.config.items.get(s.item as usize).and_then(|d| d.consumable()).and_then(|c| c.spoils_hours) else { continue };
            let share = lo + (hi - lo) * hash01(seed, c as u64, k as u64, s.item as u64);
            s.born = -((h * share * 60.0).round() as i32);
        }
    }

    /// Seconds eating or drinking `item` takes (other uses: `CarryTuning::use_secs`).
    pub(crate) fn use_secs(&self, item: u16) -> f32 {
        let t = &self.config.needs;
        match self.item_def(item).consumable() {
            Some(c) => (t.eat_secs.0 + t.eat_secs_per * (c.hunger.abs() + c.thirst.abs())).clamp(t.eat_secs.0, t.eat_secs.1),
            None => self.config.carry.use_secs,
        }
    }

    /// Can player `pi` eat or drink `item` now?
    pub(crate) fn can_consume(&self, pi: usize, item: u16) -> Result<(), String> {
        if self.item_def(item).food.is_some() && !self.players[pi].health.can_eat() {
            return Err("you can't keep food down (food poisoning)".into());
        }
        Ok(())
    }

    /// Player `pi` had a go of `had` (the stack as it was): its hunger and thirst, less if it's gone
    /// off, and maybe food poisoning. Says what it did.
    pub(crate) fn consume(&mut self, pi: usize, had: ItemStack) -> String {
        let Some(c) = self.item_def(had.item).consumable().cloned() else { return "nothing happened".into() };
        let t = self.config.needs;
        let fresh = self.freshness_of(&had);
        let (mult, poison) = match fresh {
            Some(Freshness::Stale) => (t.stale_food, self.config.health.poison_stale),
            Some(Freshness::Mouldy) => (t.mouldy_food, self.config.health.poison_rotten),
            _ => (1.0, 0.0),
        };
        let n = &mut self.players[pi].needs;
        let mut bits = Vec::new();
        if let Some(f) = fresh.filter(|f| *f != Freshness::Fresh) {
            bits.push(format!("it was {}", f.name()));
        }
        let mut change = |need: &mut f32, amount: f32, what: &str, waste: bool| {
            if amount == 0.0 {
                return;
            }
            if amount > 0.0 && waste {
                bits.push(format!("you weren't {what}, so it did nothing for your {}", if what == "hungry" { "hunger" } else { "thirst" }));
                return;
            }
            let before = *need;
            // Going off takes from what it gives, not from what it costs.
            *need = (*need - if amount > 0.0 { amount * mult } else { amount }).clamp(0.0, 1.0);
            bits.push(format!("{} {:.0}% -> {:.0}%", if what == "hungry" { "hunger" } else { "thirst" }, before * 100.0, *need * 100.0));
        };
        let waste = n.hunger < t.waste_below && self.config.items[had.item as usize].food.is_some();
        change(&mut n.hunger, c.hunger, "hungry", waste);
        change(&mut n.thirst, c.thirst, "thirsty", false);
        let id = self.players[pi].id;
        if poison > 0.0 && self.poison_player(id, poison) {
            bits.push("FOOD POISONING".into());
        }
        self.private_dirty.insert(id);
        bits.join("; ")
    }

    /// Player `pi`'s movement with its needs on top of its load and wounds: asleep, it doesn't
    /// move; hungry and thirsty, endurance recovers slower; parched, no running; exhausted, slower.
    pub(crate) fn needs_mods(&self, pi: usize, mods: MoveMods) -> MoveMods {
        let p = &self.players[pi];
        if p.dead {
            return mods;
        }
        let (t, n) = (&self.config.needs, &p.needs);
        if n.asleep() {
            return MoveMods { speed: 0.0, can_run: false, can_climb: false, ..mods };
        }
        let mut m = mods;
        if n.hunger_level(t) >= Level::Strong {
            m.regen *= t.hungry_regen;
        }
        match n.thirst_level(t) {
            Level::Severe => {
                m.regen *= t.thirsty_regen;
                m.can_run = false;
            }
            Level::Strong => m.regen *= t.thirsty_regen,
            _ => {}
        }
        if n.tiredness_level(t) == Level::Severe {
            m.speed *= t.exhausted_walk;
        }
        m
    }

    /// Player `pi`'s pain: its wounds' and soreness.
    pub fn pain_of(&self, pi: usize) -> f32 {
        let p = &self.players[pi];
        p.health.pain(&self.config.health) + p.needs.sore_pain(&self.config.needs)
    }

    /// A swing starts: it makes you a little thirstier, and hunger, tiredness and pain change its
    /// power and time (multipliers).
    pub(crate) fn needs_swing(&mut self, pi: usize) -> (f32, f32) {
        let t = self.config.needs;
        let pain = self.pain_of(pi);
        let n = &mut self.players[pi].needs;
        n.thirst = (n.thirst + t.swing_thirst).min(1.0);
        let power = if n.hunger_level(&t) == Level::Severe { t.starving_swing_power } else { 1.0 } * t.pain_power(pain);
        let time = match n.tiredness_level(&t) {
            Level::Severe => t.exhausted_swing_time,
            Level::Strong => t.tired_swing_time,
            _ => 1.0,
        };
        (power, time)
    }

    /// What player `pi` would sleep on where it stands: a bed, else a sofa, within
    /// [`SLEEP_REACH`] in its room, else the floor.
    pub fn sleep_spot(&self, pos: Vec2) -> SleepSpot {
        let room = self.map.room_at(pos);
        if room == 0 {
            return SleepSpot::Floor;
        }
        let mut best = SleepSpot::Floor;
        for f in self.map.buildings.iter().flat_map(|b| &b.furniture).filter(|f| f.level == 0) {
            let spot = match f.kind.as_str() {
                "bed" => SleepSpot::Bed,
                "sofa" => SleepSpot::Sofa,
                _ => continue,
            };
            let lo = f.min.as_vec2();
            let hi = lo + f.size.as_vec2();
            if pos.clamp(lo, hi).distance(pos) <= SLEEP_REACH && self.map.room_at((lo + hi) * 0.5) == room && (spot as u8) < (best as u8) {
                best = spot;
            }
        }
        best
    }

    fn needs_note(&mut self, player: u32, ok: bool, text: String) {
        if self.item_notes.len() >= MAX_ITEM_NOTES {
            self.item_notes.remove(0);
        }
        self.item_notes.push(ItemNote { player, ok, text });
    }

    /// Player `id` lies down to sleep: on a bed or sofa within reach, else the floor. Refused (and
    /// noted for its client) if it isn't tired enough, is in agony, or is busy swinging. Drops its
    /// item jobs.
    pub fn player_sleep(&mut self, id: u32) -> Result<String, String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        let t = self.config.needs;
        let p = &self.players[pi];
        let why = if p.dead {
            Some("dead".to_string())
        } else if p.needs.asleep() {
            return Ok("already asleep".into());
        } else if p.needs.tiredness < t.min_tiredness_to_sleep {
            Some(format!("not tired enough to sleep (tiredness {:.0}%, sleep from {:.0}%)", p.needs.tiredness * 100.0, t.min_tiredness_to_sleep * 100.0))
        } else if t.pain_level(self.pain_of(pi)) >= 3 {
            Some("in too much pain to sleep".into())
        } else if p.action.is_some() {
            Some("busy fighting".into())
        } else {
            None
        };
        if let Some(why) = why {
            self.needs_note(id, false, why.clone());
            return Err(why);
        }
        let spot = self.sleep_spot(self.players[pi].state.pos);
        self.cancel_jobs(pi);
        let p = &mut self.players[pi];
        p.queued = None;
        p.needs.sleep = Some(Sleep { spot, hours: 0.0 });
        self.refresh_carry(pi);
        let text = format!("you lie down on the {} and fall asleep", spot.name());
        self.needs_note(id, true, text.clone());
        Ok(text)
    }

    /// Player `pi` wakes up (if asleep), for `reason`. A night on the floor leaves it sore.
    pub(crate) fn wake(&mut self, pi: usize, reason: WakeReason) {
        let t = self.config.needs;
        let p = &mut self.players[pi];
        let Some(s) = p.needs.sleep.take() else { return };
        if s.spot == SleepSpot::Floor && reason != WakeReason::Died {
            p.needs.sore_h = t.floor_sore_h;
        }
        let id = p.id;
        self.woke.push((id, reason));
        if reason != WakeReason::Died {
            self.needs_note(id, true, format!("awake: {} (slept {:.1} game hours on the {})", reason.text(), s.hours, s.spot.name()));
        }
        self.refresh_carry(pi);
    }

    /// Player `id` gets up. Returns whether it was asleep.
    pub fn wake_player(&mut self, id: u32) -> bool {
        let Some(pi) = self.players.iter().position(|p| p.id == id && p.needs.asleep()) else { return false };
        self.wake(pi, WakeReason::GotUp);
        true
    }

    /// Debug / tests (MCP `set_needs`): set player `id`'s needs (0..1; `None` leaves one as it is).
    /// Returns them, or `None` if there's no such player.
    pub fn set_needs(&mut self, id: u32, hunger: Option<f32>, thirst: Option<f32>, tiredness: Option<f32>) -> Option<Needs> {
        let pi = self.players.iter().position(|p| p.id == id)?;
        let n = &mut self.players[pi].needs;
        let set = |v: &mut f32, to: Option<f32>| {
            if let Some(x) = to.filter(|x| x.is_finite()) {
                *v = x.clamp(0.0, 1.0);
            }
        };
        set(&mut n.hunger, hunger);
        set(&mut n.thirst, thirst);
        set(&mut n.tiredness, tiredness);
        let n = *n;
        self.refresh_carry(pi);
        Some(n)
    }

    /// Player `id`'s needs as its own client sees them.
    pub fn needs_view(&self, id: u32) -> Option<NeedsView> {
        self.player(id).map(|p| p.needs.view(&self.config.needs))
    }

    /// Player `id`'s status tags (the HUD's, and the MCP tools').
    pub fn status_tags(&self, id: u32) -> Vec<StatusTag> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Vec::new() };
        let p = &self.players[pi];
        if p.dead {
            return Vec::new();
        }
        let h = &self.config.health;
        let input = StatusInput {
            needs: p.needs.view(&self.config.needs),
            pain: p.health.pain(h),
            bleeding: p.health.bleeding(h),
            food_poisoned: p.health.food_poisoned(),
            festering: p.health.wounds.iter().any(|w| w.festering),
            infection: p.health.visible_infection(),
            endurance: p.state.endurance,
            winded: p.state.winded,
            tired_below: self.config.combat.tired_below,
            burden: self.carry(p).burden,
            time_scale: self.time_scale(),
        };
        status_tags(&self.config.needs, &input)
    }

    /// Did something wake sleeper `pi` this tick: a strike landing, a sound reaching it, a zombie
    /// seeing it?
    fn disturbed(&self, pi: usize) -> Option<WakeReason> {
        let p = &self.players[pi];
        let (id, pos) = (p.id, p.state.pos);
        if self.attack_events.iter().any(|a| a.player == id && a.landed) {
            return Some(WakeReason::Hit);
        }
        let reach = self.config.needs.wake_noise_m;
        if self.heard.iter().any(|s| !matches!(s.source, SoundSource::Player(o) if o == id) && s.pos.distance(pos) <= s.radius.min(reach)) {
            return Some(WakeReason::Noise);
        }
        let z = &self.zombies;
        if (0..z.len()).any(|i| z.tier[i] == Tier::Active && z.sees[i] > 0 && z.target_player[i] == id) {
            return Some(WakeReason::Seen);
        }
        None
    }

    /// `dt_h` game hours pass for everyone's needs. `live`: a real tick (sleepers can be woken by
    /// what happened in it), not a time skip. Hunger and thirst past severe cost HP (through
    /// `hurt_player`); sleepers rest and wake. Movement modifiers and the private view follow when
    /// something changed.
    pub(crate) fn step_needs(&mut self, dt_h: f32, live: bool) {
        let t = self.config.needs;
        for pi in 0..self.players.len() {
            if self.players[pi].dead {
                if self.players[pi].needs.asleep() {
                    self.wake(pi, WakeReason::Died);
                }
                continue;
            }
            let pain = self.pain_of(pi);
            let thirst_mult = self.players[pi].health.thirst_mult(&self.config.health);
            let p = &mut self.players[pi];
            let (before, view_before) = (signature(&p.needs, &t), view_key(&p.needs));
            let running = p.state.stance == Stance::Run;
            let n = &mut p.needs;
            let asleep = n.asleep();
            n.hunger += if asleep { t.hunger_asleep_per_h } else { t.hunger_per_h } * dt_h;
            n.thirst += (if asleep { t.thirst_asleep_per_h } else { t.thirst_per_h } * thirst_mult + if running { t.run_thirst_per_h } else { 0.0 }) * dt_h;
            match &mut n.sleep {
                Some(s) => {
                    n.tiredness -= t.rest_per_h(s.spot) * dt_h;
                    s.hours += dt_h;
                }
                None => n.tiredness += t.tired_per_h * dt_h,
            }
            n.hunger = n.hunger.clamp(0.0, 1.0);
            n.thirst = n.thirst.clamp(0.0, 1.0);
            n.tiredness = n.tiredness.clamp(0.0, 1.0);
            n.sore_h = (n.sore_h - dt_h).max(0.0);
            let mut loss = 0.0;
            if n.hunger_level(&t) == Level::Severe {
                loss += t.starving_hp_per_h * dt_h;
            }
            if n.thirst_level(&t) == Level::Severe {
                loss += t.parched_hp_per_h * dt_h;
            }
            let (sleep, tiredness, starving, parched) = (n.sleep, n.tiredness, n.hunger_level(&t) == Level::Severe, n.thirst_level(&t) == Level::Severe);
            let wake = match sleep {
                Some(_) if tiredness < t.rested_below => Some(WakeReason::Rested),
                Some(s) if s.hours >= t.hunger_wakes_after_h && starving => Some(WakeReason::Hunger),
                Some(s) if s.hours >= t.hunger_wakes_after_h && parched => Some(WakeReason::Thirst),
                Some(_) if t.pain_level(pain) >= 3 => Some(WakeReason::Pain),
                Some(_) if live => self.disturbed(pi),
                _ => None,
            };
            if let Some(r) = wake {
                self.wake(pi, r);
            }
            if loss > 0.0 && self.hurt_player(pi, loss) {
                continue;
            }
            let n = &self.players[pi].needs;
            if signature(n, &t) != before {
                self.refresh_carry(pi);
            } else if view_key(n) != view_before {
                let id = self.players[pi].id;
                self.private_dirty.insert(id);
            }
        }
    }

    /// Time skipped (`World::skip_time`): needs live through it in small steps (sleepers rest, and
    /// wake only by themselves).
    pub(crate) fn skip_needs(&mut self, hours: f32) {
        let mut left = hours.max(0.0);
        while left > 0.0 {
            let dt = left.min(SKIP_STEP_H);
            self.step_needs(dt, false);
            left -= dt;
        }
    }

    /// Fold player needs into the state hash.
    pub(crate) fn hash_needs(n: &Needs, mix: &mut impl FnMut(u32)) {
        mix(n.hunger.to_bits());
        mix(n.thirst.to_bits());
        mix(n.tiredness.to_bits());
        mix(n.sore_h.to_bits());
        if let Some(s) = n.sleep {
            mix(s.spot as u32 | 0x51EE_0000);
            mix(s.hours.to_bits());
        }
    }
}

/// What the movement modifiers depend on: the need levels and sleep.
fn signature(n: &Needs, t: &NeedsTuning) -> u32 {
    n.hunger_level(t) as u32 | (n.thirst_level(t) as u32) << 4 | (n.tiredness_level(t) as u32) << 8 | n.sleep.map_or(0xF, |s| s.spot as u32) << 12 | ((n.sore_h > 0.0) as u32) << 16
}

/// What the private view shows: the needs to 1%.
fn view_key(n: &Needs) -> u32 {
    let pc = |v: f32| (v * 100.0).round() as u32;
    pc(n.hunger) | pc(n.thirst) << 8 | pc(n.tiredness) << 16 | (n.asleep() as u32) << 24 | ((n.sore_h > 0.0) as u32) << 25
}

#[cfg(test)]
mod tests;
