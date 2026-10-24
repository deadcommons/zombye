//! Health beyond the one number (#9): wounds on body parts, bleeding, pain, festering, food
//! poisoning and the zombie infection ("the turn").
//!
//! A player keeps one pool of hit points ([`crate::CombatTuning::player_hp`], 100). Wounds sit on
//! one of ten body parts ([`Part`]); each has a kind ([`WoundKind`]), an age and a time to heal,
//! and drains hit points while it bleeds or festers. Every loss goes through
//! `World::hurt_player`, so god mode (`DevFlags::god`) holds for all of it.
//!
//! - **Zombie strikes** ([`World::roll_zombie_wound`]) land on a part picked by weight (the upper
//!   body likeliest; more head and torso from behind; legs and feet only from a zombie on the
//!   ground), leave a scratch, gash or bite (a bite likelier when two or more are on you or it's
//!   from behind), and the clothing worn on that part ([`World::protection`]) may turn it into a
//!   bruise. Each wound a zombie leaves rolls once for the infection.
//! - **Glass:** smashing a window bare-handed may cut the hand (gloves mostly stop it); climbing
//!   in through a broken window may cut a hand, arm or leg (clothing protects).
//! - **While open** a wound weakens swings (arms and hands), slows walking and running (legs and
//!   feet; a gash or bite on a leg stops running), slows endurance recovery (torso), and adds pain
//!   (double on the head). Leg and torso effects reach movement through [`MoveMods`], which the
//!   server sets and snapshots carry, so client prediction stays exact.
//! - **Festering:** an open wound (not a bruise) rolls every game hour from a seeded hash; a
//!   festering wound hurts more, heals at a third of the speed and costs HP.
//! - **The turn:** hidden for hours (nothing shows, not even to the player's own client or an agent),
//!   then *Feverish*, then *Turning*, then death; the body rises as a zombie. Fatal by default;
//!   `InfectionTuning::fatal` off makes it burn out after the fever, leaving you at 30 HP.
//! - **Food poisoning** is a hook for the needs (#11): [`World::poison_player`] starts it.
//!
//! First aid (#10, [`crate::firstaid`]) sets [`Wound::dressing`], [`Wound::cleaned`] and
//! [`Wound::stitched`]: a dressing stops bleeding (a gash keeps a quarter until stitched), clean
//! dressings turn dirty in time, dirty ones fester more, clean ones much less; painkillers and
//! treatments' stings change [`Health::pain`]; antibiotics ([`Health::aid`]) cure festering.
//!
//! Every roll comes from the combat RNG or a seeded hash, never the clock. Times are game hours
//! (a day is `WorldConfig::day_length_secs`, 3,600 s by default: one game hour is 150 s). All
//! numbers are ours ([`HealthTuning`], [`InfectionTuning`]).

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::combat::{CombatEvent, CombatEventKind};
use crate::firstaid::{Aid, AidView, FirstAidTuning};
use crate::items::BodyPart;
use crate::players::{MoveMods, Player};
use crate::rng::hash01;
use crate::run::DeathCause;
use crate::world::World;

/// Wounds one player can carry at once; strikes past this still hurt but leave no new wound.
pub const MAX_WOUNDS: usize = 64;
/// Longest step [`World::step_health`] takes when time is skipped (game hours).
const SKIP_STEP_H: f32 = 0.1;
const FESTER_SALT: u64 = 0xFE57_E400;

/// Where a wound is. Clothing covers coarser regions ([`BodyPart`]: both arms at once, say);
/// [`Part::region`] maps one to the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Part {
    Head,
    Torso,
    LeftArm,
    RightArm,
    LeftHand,
    RightHand,
    LeftLeg,
    RightLeg,
    LeftFoot,
    RightFoot,
}

impl Part {
    pub const ALL: [Part; 10] = [Self::Head, Self::Torso, Self::LeftArm, Self::RightArm, Self::LeftHand, Self::RightHand, Self::LeftLeg, Self::RightLeg, Self::LeftFoot, Self::RightFoot];

    /// The clothing region covering it.
    pub fn region(self) -> BodyPart {
        match self {
            Self::Head => BodyPart::Head,
            Self::Torso => BodyPart::Torso,
            Self::LeftArm | Self::RightArm => BodyPart::Arms,
            Self::LeftHand | Self::RightHand => BodyPart::Hands,
            Self::LeftLeg | Self::RightLeg => BodyPart::Legs,
            Self::LeftFoot | Self::RightFoot => BodyPart::Feet,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Torso => "torso",
            Self::LeftArm => "left arm",
            Self::RightArm => "right arm",
            Self::LeftHand => "left hand",
            Self::RightHand => "right hand",
            Self::LeftLeg => "left leg",
            Self::RightLeg => "right leg",
            Self::LeftFoot => "left foot",
            Self::RightFoot => "right foot",
        }
    }

    /// The `snake_case` id tools take ("left_arm").
    pub fn id(self) -> String {
        self.name().replace(' ', "_")
    }

    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    /// An arm or a hand: wounds there weaken swings.
    pub fn is_arm(self) -> bool {
        matches!(self.region(), BodyPart::Arms | BodyPart::Hands)
    }

    /// A leg or a foot: wounds there slow you down.
    pub fn is_leg(self) -> bool {
        matches!(self.region(), BodyPart::Legs | BodyPart::Feet)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum WoundKind {
    /// A strike clothing stopped, a blunt knock: hurts a little, never bleeds or festers.
    Bruise,
    Scratch,
    Gash,
    Bite,
    /// From window glass.
    GlassCut,
}

impl WoundKind {
    pub const ALL: [WoundKind; 5] = [Self::Bruise, Self::Scratch, Self::Gash, Self::Bite, Self::GlassCut];

    pub fn name(self) -> &'static str {
        match self {
            Self::Bruise => "bruise",
            Self::Scratch => "scratch",
            Self::Gash => "gash",
            Self::Bite => "bite",
            Self::GlassCut => "glass cut",
        }
    }

    pub fn id(self) -> String {
        self.name().replace(' ', "_")
    }

    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Self::ALL.into_iter().find(|k| k.id() == id)
    }
}

/// What covers a wound (#10 puts these on): none, a clean dressing or a dirty one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Dressing {
    #[default]
    None,
    Clean,
    Dirty,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wound {
    /// Per player, counting up (seeds the festering rolls).
    pub id: u32,
    pub part: Part,
    pub kind: WoundKind,
    /// Game hours since it happened.
    pub age_h: f32,
    /// Game hours of healing it needs (rolled when it happens), and how many it has had.
    pub heal_h: f32,
    pub healed_h: f32,
    /// Still bleeding (a scratch stops by itself; the rest need a dressing, #10).
    pub bleeding: bool,
    pub dressing: Dressing,
    /// Game hours the dressing has been on (#10: a clean one turns dirty).
    pub dressing_age_h: f32,
    /// Cleaned with antiseptic before this dressing (#10): halves the festering chance.
    pub cleaned: bool,
    /// Stitched (#10): stops a gash bleeding and heals twice as fast.
    pub stitched: bool,
    pub festering: bool,
    /// The dressing on it is a plaster (#10): nothing to give back when it comes off.
    pub plaster: bool,
    /// Game hours a clean dressing on it stays clean (set when it goes on, #10).
    pub clean_for_h: f32,
}

impl Wound {
    /// Hit points per game hour it costs while bleeding (not counting festering).
    pub fn bleed_per_h(&self, t: &HealthTuning) -> f32 {
        if !self.bleeding || self.stitched {
            return 0.0;
        }
        let rate = t.wound(self.kind).bleed_per_h;
        match self.dressing {
            Dressing::None => rate,
            // A dressing stops bleeding at once, except a gash's until it's stitched.
            _ if self.kind == WoundKind::Gash => rate * t.dressed_gash_bleed,
            _ => 0.0,
        }
    }

    /// Chance per game hour that it starts festering.
    pub fn fester_chance(&self, t: &HealthTuning) -> f32 {
        if !t.wound(self.kind).can_fester || self.festering {
            return 0.0;
        }
        let base = match self.dressing {
            Dressing::None => t.fester_undressed,
            Dressing::Dirty => t.fester_dirty,
            Dressing::Clean => t.fester_clean,
        };
        if self.cleaned { base * t.fester_cleaned_mult } else { base }
    }

    /// 0 (fresh) to 1 (healed).
    pub fn healed(&self) -> f32 {
        (self.healed_h / self.heal_h.max(1e-3)).clamp(0.0, 1.0)
    }

    /// How fast it heals now (game hours of healing per game hour).
    pub fn heal_speed(&self, t: &HealthTuning) -> f32 {
        (if self.festering { t.fester_heal_mult } else { 1.0 }) * if self.stitched { t.stitched_heal_speed } else { 1.0 }
    }

    /// Pain it adds now: easing as it heals, more while festering, double on the head.
    pub fn pain(&self, t: &HealthTuning) -> f32 {
        let p = t.wound(self.kind).pain * (1.0 - self.healed()) + if self.festering { t.fester_pain } else { 0.0 };
        if self.part == Part::Head { p * t.head_pain_mult } else { p }
    }

    /// As its owner sees it (`stopped`: the fever stops all healing).
    pub fn view(&self, t: &HealthTuning, stopped: bool) -> WoundView {
        let speed = if stopped { 0.0 } else { self.heal_speed(t) };
        WoundView {
            id: self.id,
            part: self.part,
            kind: self.kind,
            bleed_per_h: self.bleed_per_h(t),
            healed: (self.healed() * 20.0).floor() / 20.0,
            heals_in_h: (speed > 0.0).then(|| ((self.heal_h - self.healed_h).max(0.0) / speed).ceil()),
            dressing: self.dressing,
            plaster: self.plaster,
            dirty_in_h: (self.dressing == Dressing::Clean).then(|| (self.clean_for_h - self.dressing_age_h).max(0.0).ceil()),
            cleaned: self.cleaned,
            stitched: self.stitched,
            festering: self.festering,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum InfectionStage {
    /// Nothing shows, not even to the player.
    Hidden,
    /// Sick: losing HP slowly, endurance slow to come back, wounds not healing.
    Feverish,
    /// Losing HP fast; night sight halved. Ends in death (and rising).
    Turning,
}

/// The zombie infection in a player.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Infection {
    pub stage: InfectionStage,
    /// Game hours left in this stage (`Turning` lasts until death).
    pub left_h: f32,
    /// How long the fever will last (rolled when infected).
    pub feverish_h: f32,
    /// The wound it came in through.
    pub wound: u32,
}

/// A player's wounds and illnesses. Saved with the player and in `state_hash`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Health {
    /// In the order they happened.
    pub wounds: Vec<Wound>,
    pub next_wound: u32,
    pub infection: Option<Infection>,
    /// Game hours of food poisoning left (0: none).
    pub food_poisoning_h: f32,
    /// Dead and infected: seconds until the body rises as a zombie.
    pub rise_in: Option<f32>,
    /// First aid in effect: painkillers, treatments' stings, antibiotics, opened medical items (#10).
    pub aid: Aid,
}

/// What wounds and illness do to a player right now (1 = no effect).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effects {
    /// Multiplies swing power (damage, knockback, knockdown) and swing time.
    pub swing_power: f32,
    pub swing_time: f32,
    /// Multiply walking (and sneaking) and running speed.
    pub walk: f32,
    pub run: f32,
    pub can_run: bool,
    /// Multiplies endurance recovery.
    pub regen: f32,
}

/// Coarse health anyone can see of a player (snapshots' `PlayerView`): no detail, never the
/// hidden infection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthTags {
    pub bleeding: bool,
    /// An open wound on a leg or foot.
    pub limping: bool,
    /// An open wound on an arm or hand.
    pub injured_arm: bool,
    /// Feverish, turning or food poisoning.
    pub sick: bool,
}

/// One wound as its owner sees it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WoundView {
    /// The wound's id, for treating it (`Intent::Treat`).
    pub id: u32,
    pub part: Part,
    pub kind: WoundKind,
    /// HP per game hour it is bleeding (0: not bleeding).
    pub bleed_per_h: f32,
    /// 0 fresh to 1 healed, in 5% steps.
    pub healed: f32,
    /// Game hours until it heals at the speed it heals now, rounded up (`None`: not healing).
    pub heals_in_h: Option<f32>,
    pub dressing: Dressing,
    /// The dressing is a plaster.
    pub plaster: bool,
    /// A clean dressing: game hours until it turns dirty, rounded up.
    pub dirty_in_h: Option<f32>,
    pub cleaned: bool,
    pub stitched: bool,
    pub festering: bool,
}

/// A player's health as its own client (and an agent's `health` tool) sees it. The infection
/// only shows once it is feverish.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HealthView {
    pub wounds: Vec<WoundView>,
    /// HP per game hour lost to bleeding and festering together.
    pub losing_per_h: f32,
    pub pain: f32,
    pub infection: Option<InfectionStage>,
    pub food_poisoning: bool,
    pub tags: HealthTags,
    /// Painkillers, antibiotics and opened medical items (#10).
    pub aid: AidView,
}

impl Health {
    pub fn bleeding(&self, t: &HealthTuning) -> usize {
        self.wounds.iter().filter(|w| w.bleed_per_h(t) > 0.0).count()
    }

    /// HP per game hour lost to bleeding and festering (not the infection).
    pub fn losing_per_h(&self, t: &HealthTuning) -> f32 {
        self.wounds.iter().map(|w| w.bleed_per_h(t) + if w.festering { t.fester_hp_per_h } else { 0.0 }).sum()
    }

    /// Pain now: the wounds' and treatments' less what painkillers take away (#10).
    pub fn pain(&self, t: &HealthTuning) -> f32 {
        let wounds: f32 = self.wounds.iter().map(|w| w.pain(t)).sum();
        (wounds + self.aid.sting_pain() - self.aid.relief(&t.first_aid)).max(0.0)
    }

    /// The infection's stage once it shows (never `Hidden`).
    pub fn visible_infection(&self) -> Option<InfectionStage> {
        self.infection.map(|i| i.stage).filter(|s| *s != InfectionStage::Hidden)
    }

    /// Feverish or turning: wounds don't heal and HP doesn't come back.
    pub fn healing_stopped(&self) -> bool {
        self.visible_infection().is_some()
    }

    pub fn food_poisoned(&self) -> bool {
        self.food_poisoning_h > 0.0
    }

    /// Sick (#11's status): feverish, turning or food poisoned.
    pub fn sick(&self) -> bool {
        self.healing_stopped() || self.food_poisoned()
    }

    /// Food poisoning means you can't keep food down (#11 checks this before eating).
    pub fn can_eat(&self) -> bool {
        !self.food_poisoned()
    }

    /// How much faster thirst rises (#11).
    pub fn thirst_mult(&self, t: &HealthTuning) -> f32 {
        if self.food_poisoned() { t.poison_thirst_mult } else { 1.0 }
    }

    pub fn effects(&self, t: &HealthTuning, inf: &InfectionTuning) -> Effects {
        let (mut arm, mut leg, mut torso, mut no_run) = (0.0, 0.0, 0.0, false);
        for w in &self.wounds {
            let e = t.wound(w.kind).effect;
            if w.part.is_arm() {
                arm += e;
            } else if w.part.is_leg() {
                leg += e;
                no_run |= matches!(w.part, Part::LeftLeg | Part::RightLeg) && t.leg_wound_stops_running(w.kind);
            } else if w.part == Part::Torso {
                torso += e;
            }
        }
        let cut = |n: f32, per: f32, max: f32| 1.0 - (n * per).min(max).clamp(0.0, 0.95);
        let mut regen = cut(torso, t.torso_regen_per, t.torso_regen_max);
        if self.healing_stopped() {
            regen *= inf.feverish_regen_mult;
        }
        if self.food_poisoned() {
            regen *= t.poison_regen_mult;
        }
        Effects {
            swing_power: cut(arm, t.arm_power_per, t.arm_power_max),
            swing_time: 1.0 + (arm * t.arm_time_per).min(t.arm_time_max).max(0.0),
            walk: cut(leg, t.leg_walk_per, t.leg_max),
            run: cut(leg, t.leg_run_per, t.leg_max),
            can_run: !no_run,
            regen,
        }
    }

    /// The load's movement modifiers with the wounds' on top.
    pub fn apply_to(&self, mods: MoveMods, t: &HealthTuning, inf: &InfectionTuning) -> MoveMods {
        let e = self.effects(t, inf);
        let walk = e.walk.max(0.05);
        MoveMods { speed: mods.speed * walk, run_speed: mods.run_speed * e.run / walk, can_run: mods.can_run && e.can_run, regen: mods.regen * e.regen, ..mods }
    }

    pub fn tags(&self, t: &HealthTuning) -> HealthTags {
        let open = |f: fn(Part) -> bool| self.wounds.iter().any(|w| f(w.part) && t.wound(w.kind).effect > 0.0);
        HealthTags { bleeding: self.bleeding(t) > 0, limping: open(Part::is_leg), injured_arm: open(Part::is_arm), sick: self.sick() }
    }

    pub fn view(&self, t: &HealthTuning) -> HealthView {
        let stopped = self.healing_stopped();
        let wounds = self.wounds.iter().map(|w| w.view(t, stopped)).collect();
        HealthView {
            wounds,
            losing_per_h: self.losing_per_h(t),
            pain: self.pain(t),
            infection: self.visible_infection(),
            food_poisoning: self.food_poisoned(),
            tags: self.tags(t),
            aid: self.aid.view(&t.first_aid),
        }
    }
}

/// One wound kind's numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WoundTuning {
    /// Hit points it takes at once.
    pub damage: (f32, f32),
    /// HP per game hour while it bleeds, and how long it bleeds by itself (`None`: until dressed).
    pub bleed_per_h: f32,
    pub bleeds_for_h: Option<f32>,
    /// Game hours to heal (each wound rolls ±`HealthTuning::heal_spread` of it).
    pub heal_h: f32,
    pub pain: f32,
    /// How much clothing counts against it (1 = its protection in full).
    pub protection: f32,
    /// How much it counts toward arm, leg and torso effects (an open gash 1, a scratch half).
    pub effect: f32,
    pub can_fester: bool,
}

/// Where zombie strikes land: a weight per part (each arm, hand, leg and foot on its own).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartWeights {
    pub head: f32,
    pub torso: f32,
    pub arm: f32,
    pub hand: f32,
    pub leg: f32,
    pub foot: f32,
}

impl PartWeights {
    pub fn weight(&self, p: Part) -> f32 {
        match p.region() {
            BodyPart::Head => self.head,
            BodyPart::Torso => self.torso,
            BodyPart::Arms => self.arm,
            BodyPart::Hands => self.hand,
            BodyPart::Legs => self.leg,
            BodyPart::Feet => self.foot,
        }
        .max(0.0)
    }

    /// The part a roll in `[0, 1)` lands on; `low` (a zombie on the ground) only legs and feet.
    pub fn pick(&self, roll: f32, low: bool) -> Part {
        let ok = |p: &Part| !low || p.is_leg();
        let total: f32 = Part::ALL.iter().filter(|p| ok(p)).map(|p| self.weight(*p)).sum();
        if total <= 0.0 {
            return if low { Part::LeftLeg } else { Part::Torso };
        }
        let mut r = roll * total;
        let mut last = Part::Torso;
        for p in Part::ALL.into_iter().filter(ok) {
            let w = self.weight(p);
            if r < w {
                return p;
            }
            r -= w;
            if w > 0.0 {
                last = p;
            }
        }
        last
    }
}

/// Which wound a zombie strike leaves (weights), before clothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrikeWounds {
    pub scratch: f32,
    pub gash: f32,
    pub bite: f32,
    /// Added to `bite` when two or more zombies are on you, or the strike comes from behind.
    pub grabbed_bite_bonus: f32,
}

/// Every wound and illness number but the infection's. Serde with defaults, saved with the world's
/// settings. Times in game hours, chances 0..1. Ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HealthTuning {
    pub bruise: WoundTuning,
    pub scratch: WoundTuning,
    pub gash: WoundTuning,
    pub bite: WoundTuning,
    pub glass_cut: WoundTuning,
    /// Each wound's healing time is rolled within ± this share of its kind's.
    pub heal_spread: f32,
    /// A stitched wound heals this many times as fast (#10).
    pub stitched_heal_speed: f32,
    /// A dressed but unstitched gash still bleeds at this share of its rate (#10).
    pub dressed_gash_bleed: f32,
    /// HP back per game hour with nothing bleeding or festering, no fever and not hungry; asleep more (#11).
    pub regen_awake_per_h: f32,
    pub regen_asleep_per_h: f32,

    /// Where strikes land from in front and from behind (zombies reach the upper body best).
    pub front: PartWeights,
    pub behind: PartWeights,
    /// A zombie facing a player's back (the player's facing · the direction of the blow above this).
    pub behind_dot: f32,
    pub strike: StrikeWounds,

    // Effects of open wounds (each counted by its kind's `effect`), with their caps.
    pub arm_power_per: f32,
    pub arm_power_max: f32,
    pub arm_time_per: f32,
    pub arm_time_max: f32,
    pub leg_walk_per: f32,
    pub leg_run_per: f32,
    pub leg_max: f32,
    /// A gash or bite on a leg (not a foot) means no running.
    pub leg_gash_stops_running: bool,
    pub torso_regen_per: f32,
    pub torso_regen_max: f32,
    pub head_pain_mult: f32,

    // Festering: chance per game hour by dressing; what it does.
    pub fester_undressed: f32,
    pub fester_dirty: f32,
    pub fester_clean: f32,
    /// Cleaning a wound before dressing it (#10) multiplies the chance by this.
    pub fester_cleaned_mult: f32,
    pub fester_pain: f32,
    pub fester_heal_mult: f32,
    pub fester_hp_per_h: f32,

    // Glass.
    /// Smashing a window with no weapon cuts the hand that did it this often (gloved: the other).
    pub smash_cut_chance: f32,
    pub smash_cut_gloved: f32,
    /// Climbing in through a broken window: a cut on a hand, arm or leg this often (clothing protects).
    pub climb_cut_chance: f32,

    // Food poisoning (#11 decides when; these are its numbers).
    pub poison_hours: (f32, f32),
    pub poison_regen_mult: f32,
    pub poison_thirst_mult: f32,
    /// Chances for mouldy or rotten food, stale food and unsafe water (#11, #34).
    pub poison_rotten: f32,
    pub poison_stale: f32,
    pub poison_water: f32,

    /// First aid (#10): treatment times, dressings, painkillers, antibiotics.
    pub first_aid: FirstAidTuning,
}

impl Default for HealthTuning {
    fn default() -> Self {
        let wound = |damage, bleed_per_h, bleeds_for_h, heal_h, pain, protection, effect, can_fester| WoundTuning { damage, bleed_per_h, bleeds_for_h, heal_h, pain, protection, effect, can_fester };
        Self {
            bruise: wound((4.0, 6.0), 0.0, None, 4.0, 0.05, 1.0, 0.0, false),
            scratch: wound((6.0, 9.0), 3.0, Some(1.0), 10.0, 0.08, 1.0, 0.5, true),
            gash: wound((9.0, 13.0), 10.0, None, 40.0, 0.15, 0.7, 1.0, true),
            bite: wound((12.0, 18.0), 14.0, None, 60.0, 0.2, 0.5, 1.0, true),
            glass_cut: wound((5.0, 8.0), 8.0, None, 18.0, 0.12, 0.8, 1.0, true),
            heal_spread: 0.25,
            stitched_heal_speed: 2.0,
            dressed_gash_bleed: 0.25,
            regen_awake_per_h: 2.0,
            regen_asleep_per_h: 5.0,

            front: PartWeights { head: 8.0, torso: 30.0, arm: 14.0, hand: 9.0, leg: 6.0, foot: 2.0 },
            behind: PartWeights { head: 12.0, torso: 36.0, arm: 10.0, hand: 4.0, leg: 7.0, foot: 2.0 },
            behind_dot: 0.35,
            strike: StrikeWounds { scratch: 50.0, gash: 30.0, bite: 20.0, grabbed_bite_bonus: 15.0 },

            arm_power_per: 0.12,
            arm_power_max: 0.4,
            arm_time_per: 0.08,
            arm_time_max: 0.25,
            leg_walk_per: 0.08,
            leg_run_per: 0.12,
            leg_max: 0.3,
            leg_gash_stops_running: true,
            torso_regen_per: 0.2,
            torso_regen_max: 0.5,
            head_pain_mult: 2.0,

            fester_undressed: 0.04,
            fester_dirty: 0.06,
            fester_clean: 0.005,
            fester_cleaned_mult: 0.5,
            fester_pain: 0.1,
            fester_heal_mult: 1.0 / 3.0,
            fester_hp_per_h: 1.0,

            smash_cut_chance: 0.6,
            smash_cut_gloved: 0.1,
            climb_cut_chance: 0.15,

            poison_hours: (6.0, 14.0),
            poison_regen_mult: 0.7,
            poison_thirst_mult: 2.0,
            poison_rotten: 0.7,
            poison_stale: 0.1,
            poison_water: 0.3,

            first_aid: FirstAidTuning::default(),
        }
    }
}

impl HealthTuning {
    pub fn wound(&self, kind: WoundKind) -> &WoundTuning {
        match kind {
            WoundKind::Bruise => &self.bruise,
            WoundKind::Scratch => &self.scratch,
            WoundKind::Gash => &self.gash,
            WoundKind::Bite => &self.bite,
            WoundKind::GlassCut => &self.glass_cut,
        }
    }

    fn leg_wound_stops_running(&self, kind: WoundKind) -> bool {
        self.leg_gash_stops_running && matches!(kind, WoundKind::Gash | WoundKind::Bite)
    }

    /// Settings a tick can't work with, by name (checked on load).
    pub fn check(&self) -> Result<(), String> {
        for k in WoundKind::ALL {
            let w = self.wound(k);
            let name = k.id();
            if w.heal_h.is_nan() || w.heal_h <= 0.0 || w.damage.0 < 0.0 || w.damage.1 < w.damage.0 || w.bleed_per_h < 0.0 || w.bleeds_for_h.is_some_and(|b| b < 0.0) {
                return Err(format!("health.{name}: heal_h must be above 0, damage a range from 0 up, bleeding 0 or more"));
            }
        }
        let chances = [
            ("heal_spread", self.heal_spread),
            ("fester_undressed", self.fester_undressed),
            ("fester_dirty", self.fester_dirty),
            ("fester_clean", self.fester_clean),
            ("smash_cut_chance", self.smash_cut_chance),
            ("smash_cut_gloved", self.smash_cut_gloved),
            ("climb_cut_chance", self.climb_cut_chance),
        ];
        if let Some((name, v)) = chances.iter().find(|(_, v)| !(0.0..=1.0).contains(v)) {
            return Err(format!("health.{name} is {v} (must be 0 to 1)"));
        }
        if self.poison_hours.0 < 0.0 || self.poison_hours.1 < self.poison_hours.0 {
            return Err(format!("health.poison_hours {:?} must be a range from 0 up", self.poison_hours));
        }
        self.first_aid.check()
    }
}

/// How the zombie infection is caught.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum InfectionMode {
    /// Nobody catches it.
    Off,
    /// Our chances: a bite 60%, a gash 8%, a scratch never.
    #[default]
    Default,
    /// Your own chances per wound a zombie leaves.
    Custom { bite: f32, gash: f32, scratch: f32 },
}

/// The zombie infection's sandbox settings ("the turn"). Serde with defaults, saved with the
/// world's settings. Ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InfectionTuning {
    pub mode: InfectionMode,
    /// Fatal (the default): the turn ends in death. Off: it burns out after the fever, leaving you
    /// at `survivor_hp`.
    pub fatal: bool,
    /// Multiplies every stage's length, 0.5 to 3.
    pub stage_mult: f32,
    /// Game hours hidden, then feverish (each infection rolls its own).
    pub hidden_h: (f32, f32),
    pub feverish_h: (f32, f32),
    pub feverish_hp_per_h: f32,
    pub turning_hp_per_h: f32,
    /// Endurance recovers this much as fast while feverish or turning.
    pub feverish_regen_mult: f32,
    /// Night sight multiplier while turning (the client's fog of war).
    pub turning_night_sight: f32,
    pub survivor_hp: f32,
    /// Seconds after death until the body rises.
    pub rise_secs: f32,
}

impl Default for InfectionTuning {
    fn default() -> Self {
        Self {
            mode: InfectionMode::Default,
            fatal: true,
            stage_mult: 1.0,
            hidden_h: (6.0, 12.0),
            feverish_h: (12.0, 24.0),
            feverish_hp_per_h: 1.0,
            turning_hp_per_h: 6.0,
            feverish_regen_mult: 0.6,
            turning_night_sight: 0.5,
            survivor_hp: 30.0,
            rise_secs: 30.0,
        }
    }
}

impl InfectionTuning {
    /// Chance a wound of this kind left by a zombie carries the infection.
    pub fn chance(&self, kind: WoundKind) -> f32 {
        let (bite, gash, scratch) = match self.mode {
            InfectionMode::Off => return 0.0,
            InfectionMode::Default => (0.6, 0.08, 0.0),
            InfectionMode::Custom { bite, gash, scratch } => (bite, gash, scratch),
        };
        match kind {
            WoundKind::Bite => bite,
            WoundKind::Gash => gash,
            WoundKind::Scratch => scratch,
            WoundKind::Bruise | WoundKind::GlassCut => 0.0,
        }
    }

    pub fn check(&self) -> Result<(), String> {
        if !(0.5..=3.0).contains(&self.stage_mult) {
            return Err(format!("infection.stage_mult is {} (0.5 to 3)", self.stage_mult));
        }
        if let InfectionMode::Custom { bite, gash, scratch } = self.mode
            && [bite, gash, scratch].iter().any(|c| !(0.0..=1.0).contains(c))
        {
            return Err("infection.mode: Custom chances must be 0 to 1".into());
        }
        for (name, r) in [("hidden_h", self.hidden_h), ("feverish_h", self.feverish_h)] {
            if r.0 < 0.0 || r.1 < r.0 {
                return Err(format!("infection.{name} {r:?} must be a range from 0 up"));
            }
        }
        if self.rise_secs < 0.0 || self.survivor_hp <= 0.0 {
            return Err("infection: rise_secs must be 0 or more, survivor_hp above 0".into());
        }
        Ok(())
    }
}

impl World {
    /// Game hours in one second of simulation.
    pub fn game_hours_per_sec(&self) -> f32 {
        24.0 / self.config.day_length_secs.max(1.0)
    }

    /// Chance (0..1) that clothing a player wears stops a strike on this region: each worn item
    /// covering it lets `1 - protection` through, layers multiplying.
    pub fn protection(&self, p: &Player, region: BodyPart) -> f32 {
        let mut through = 1.0;
        for s in &p.inventory.worn {
            if let Some(c) = self.config.items.get(s.item as usize).and_then(|d| d.clothing.as_ref())
                && c.covers.contains(&region)
            {
                through *= 1.0 - c.protection.clamp(0.0, 1.0);
            }
        }
        1.0 - through
    }

    /// Roll where a zombie strike on player `pi` lands, what it leaves and how much it takes at
    /// once (combat RNG). `behind`: it came at the player's back; `crowd`: two or more are on them;
    /// `low`: the zombie is on the ground (legs and feet only).
    pub fn roll_zombie_wound(&mut self, pi: usize, behind: bool, crowd: bool, low: bool) -> (Part, WoundKind, f32) {
        let t = &self.config.health;
        let part = if behind { &t.behind } else { &t.front }.pick(self.combat_rng.f32(), low);
        let s = &t.strike;
        let bite = s.bite + if crowd || behind { s.grabbed_bite_bonus } else { 0.0 };
        let total = s.scratch.max(0.0) + s.gash.max(0.0) + bite.max(0.0);
        let r = self.combat_rng.f32() * total;
        let mut kind = if r < s.scratch.max(0.0) || total <= 0.0 {
            WoundKind::Scratch
        } else if r < s.scratch.max(0.0) + s.gash.max(0.0) {
            WoundKind::Gash
        } else {
            WoundKind::Bite
        };
        let stop = self.protection(&self.players[pi], part.region()) * t.wound(kind).protection;
        if self.combat_rng.f32() < stop {
            kind = WoundKind::Bruise;
        }
        let (lo, hi) = self.config.health.wound(kind).damage;
        (part, kind, self.combat_rng.range(lo, hi))
    }

    /// A new wound on player `pi` (no HP taken here; see [`World::hurt_player`]). `zombie`: left
    /// by a zombie, so it may carry the infection. Returns its id, or `None` if the player is dead
    /// or carries [`MAX_WOUNDS`] already.
    pub(crate) fn add_wound(&mut self, pi: usize, part: Part, kind: WoundKind, zombie: bool, infect: Option<bool>) -> Option<u32> {
        if self.players[pi].dead || self.players[pi].health.wounds.len() >= MAX_WOUNDS {
            return None;
        }
        let (t, inf) = (&self.config.health, &self.config.infection);
        let spread = t.heal_spread.clamp(0.0, 0.9);
        let heal_h = t.wound(kind).heal_h.max(0.01) * self.combat_rng.range(1.0 - spread, 1.0 + spread);
        let infected = match infect {
            Some(forced) => forced,
            None => zombie && self.combat_rng.f32() < inf.chance(kind),
        };
        let stages = infected.then(|| {
            let m = inf.stage_mult.clamp(0.5, 3.0);
            (self.combat_rng.range(inf.hidden_h.0, inf.hidden_h.1) * m, self.combat_rng.range(inf.feverish_h.0, inf.feverish_h.1) * m)
        });
        let bleeding = t.wound(kind).bleed_per_h > 0.0;
        self.players[pi].run.wounds += 1;
        let h = &mut self.players[pi].health;
        let id = h.next_wound;
        h.next_wound += 1;
        h.wounds.push(Wound {
            id,
            part,
            kind,
            age_h: 0.0,
            heal_h,
            healed_h: 0.0,
            bleeding,
            dressing: Dressing::None,
            dressing_age_h: 0.0,
            cleaned: false,
            stitched: false,
            festering: false,
            plaster: false,
            clean_for_h: 0.0,
        });
        if let Some((hidden, feverish)) = stages
            && h.infection.is_none()
        {
            h.infection = Some(Infection { stage: InfectionStage::Hidden, left_h: hidden, feverish_h: feverish, wound: id });
        }
        self.refresh_carry(pi);
        Some(id)
    }

    /// A wound and the hit points it takes at once, with hit feedback (`PlayerHurt`, from `from`).
    /// God mode: the event happens, but no wound and no HP lost.
    pub(crate) fn wound_player(&mut self, pi: usize, part: Part, kind: WoundKind, damage: f32, from: Vec2, zombie: u32) {
        let p = &self.players[pi];
        let (pos, id, god) = (p.state.pos, p.id, p.dev.god);
        let dir = (pos - from).normalize_or(Vec2::X);
        self.push_combat_event(CombatEvent { kind: CombatEventKind::PlayerHurt, pos, dir, zombie, player: id, damage, crit: false, part: Some(part), wound: Some(kind) });
        if !god {
            self.add_wound(pi, part, kind, zombie != 0, None);
        }
        let cause = if zombie != 0 { DeathCause::Zombie { part, wound: kind } } else { DeathCause::Glass { part } };
        // Being hit stops first aid (#10) and crafting (#12).
        self.interrupt_treatment(pi);
        self.hurt_player_by(pi, damage, cause);
    }

    /// Debug / tests (MCP `wound`): give player `id` a wound of this kind here, taking its damage
    /// (god mode keeps the HP but not the wound off). `infect`: `Some` forces the infection on or
    /// off; `None` rolls as for a zombie's wound when the kind is one a zombie leaves. Returns the
    /// damage, or `None` if there's no such living player (or it has too many wounds).
    pub fn inflict_wound(&mut self, id: u32, part: Part, kind: WoundKind, infect: Option<bool>) -> Option<f32> {
        let pi = self.players.iter().position(|p| p.id == id)?;
        let zombie = matches!(kind, WoundKind::Scratch | WoundKind::Gash | WoundKind::Bite);
        let (lo, hi) = self.config.health.wound(kind).damage;
        let damage = self.combat_rng.range(lo, hi);
        self.add_wound(pi, part, kind, zombie, infect)?;
        let pos = self.players[pi].state.pos;
        self.push_combat_event(CombatEvent { kind: CombatEventKind::PlayerHurt, pos, dir: Vec2::X, zombie: 0, player: id, damage, crit: false, part: Some(part), wound: Some(kind) });
        self.hurt_player_by(pi, damage, DeathCause::Wound { part, wound: kind });
        Some(damage)
    }

    /// Hook for the needs (#11) and water (#34): with this chance (e.g. `HealthTuning::poison_rotten`)
    /// player `id` gets food poisoning (combat RNG). Returns whether it did.
    pub fn poison_player(&mut self, id: u32, chance: f32) -> bool {
        let Some(pi) = self.players.iter().position(|p| p.id == id && !p.dead) else { return false };
        if self.combat_rng.f32() >= chance {
            return false;
        }
        let (lo, hi) = self.config.health.poison_hours;
        let hours = self.combat_rng.range(lo, hi);
        let h = &mut self.players[pi].health;
        h.food_poisoning_h = h.food_poisoning_h.max(hours);
        self.refresh_carry(pi);
        true
    }

    /// Player `pi`'s load modifiers with its wounds and illness on top (`World::refresh_carry`).
    pub(crate) fn health_mods(&self, pi: usize, mods: MoveMods) -> MoveMods {
        let p = &self.players[pi];
        if p.dead { mods } else { p.health.apply_to(mods, &self.config.health, &self.config.infection) }
    }

    /// What player `pi`'s wounds do right now.
    pub fn health_effects(&self, pi: usize) -> Effects {
        self.players[pi].health.effects(&self.config.health, &self.config.infection)
    }

    /// Smashing a window: with no weapon, the right hand may be cut (gloves mostly stop it).
    pub(crate) fn smash_cut(&mut self, id: u32) {
        let Some(pi) = self.players.iter().position(|p| p.id == id && !p.dead) else { return };
        if self.players[pi].weapon.is_some() {
            return;
        }
        let t = &self.config.health;
        let gloved = self.protection(&self.players[pi], BodyPart::Hands) > 0.0;
        let chance = if gloved { t.smash_cut_gloved } else { t.smash_cut_chance };
        if self.combat_rng.f32() >= chance {
            return;
        }
        let (lo, hi) = self.config.health.glass_cut.damage;
        let damage = self.combat_rng.range(lo, hi);
        let from = self.players[pi].state.pos + Vec2::from_angle(self.players[pi].state.facing) * 0.5;
        self.wound_player(pi, Part::RightHand, WoundKind::GlassCut, damage, from, 0);
    }

    /// Climbing in through a broken window: a cut on a hand, arm or leg now and then, unless the
    /// clothing there stops it.
    pub(crate) fn climb_cut(&mut self, pi: usize) {
        if self.players[pi].dead || self.combat_rng.f32() >= self.config.health.climb_cut_chance {
            return;
        }
        const PARTS: [Part; 6] = [Part::LeftHand, Part::RightHand, Part::LeftArm, Part::RightArm, Part::LeftLeg, Part::RightLeg];
        let part = PARTS[self.combat_rng.below(PARTS.len() as u32) as usize];
        let stop = self.protection(&self.players[pi], part.region()) * self.config.health.glass_cut.protection;
        if self.combat_rng.f32() < stop {
            return;
        }
        let (lo, hi) = self.config.health.glass_cut.damage;
        let damage = self.combat_rng.range(lo, hi);
        let from = self.players[pi].state.pos;
        self.wound_player(pi, part, WoundKind::GlassCut, damage, from, 0);
    }

    /// A player died: an infected body rises as a zombie a little later (when the turn is fatal).
    pub(crate) fn health_on_death(&mut self, pi: usize) {
        let inf = &self.config.infection;
        let h = &mut self.players[pi].health;
        if inf.fatal && h.infection.is_some() && h.rise_in.is_none() {
            h.rise_in = Some(inf.rise_secs.max(0.0));
        }
    }

    /// The body of player `pi` gets up as a zombie where it lies.
    pub(crate) fn rise(&mut self, pi: usize) {
        if self.players[pi].health.rise_in.take().is_none() {
            return;
        }
        let (pos, facing) = (self.players[pi].state.pos, self.players[pi].state.facing);
        let speed = self.config.population.speed(1.0, 0.5);
        self.add_zombie(pos, facing, speed);
        self.private_dirty.insert(self.players[pi].id);
    }

    /// Time passes for everyone's wounds and illnesses: `dt_h` game hours (`dt_secs` real seconds
    /// of simulation, for a body about to rise). Bleeding, festering and the infection cost HP
    /// (through `hurt_player`); HP comes back slowly when nothing bleeds or festers and there's no
    /// fever. Movement modifiers and the private view follow when something changed.
    pub(crate) fn step_health(&mut self, dt_h: f32, dt_secs: f32) {
        let seed = self.config.seed() ^ FESTER_SALT;
        let max_hp = self.config.combat.player_hp;
        for pi in 0..self.players.len() {
            if self.players[pi].dead {
                if let Some(left) = &mut self.players[pi].health.rise_in {
                    *left -= dt_secs;
                    if *left <= 0.0 {
                        self.rise(pi);
                    }
                }
                continue;
            }
            self.players[pi].run.hours += dt_h as f64;
            let (t, inf) = (&self.config.health, &self.config.infection);
            let p = &mut self.players[pi];
            if p.health.wounds.is_empty() && p.health.infection.is_none() && p.health.food_poisoning_h <= 0.0 && p.health.aid.is_empty() && p.hp >= max_hp {
                continue;
            }
            let before = signature(&p.health, t);
            let pid = p.id as u64;
            let h = &mut p.health;
            let stopped = h.healing_stopped();
            // An antibiotic dose working: nothing starts festering (#10).
            let covered = h.aid.covered(&t.first_aid);
            h.aid.step(dt_h, &t.first_aid);
            let mut loss = 0.0;
            for w in &mut h.wounds {
                let wt = t.wound(w.kind);
                let (age, hour) = (w.age_h, w.age_h.floor());
                w.age_h += dt_h;
                if w.dressing != Dressing::None {
                    w.dressing_age_h += dt_h;
                    if w.dressing == Dressing::Clean && w.dressing_age_h >= w.clean_for_h {
                        w.dressing = Dressing::Dirty;
                    }
                }
                match wt.bleeds_for_h {
                    // Stops by itself: it bleeds only for the part of this step before then.
                    Some(stop) if w.bleeding && w.age_h >= stop => {
                        loss += w.bleed_per_h(t) * (stop - age).clamp(0.0, dt_h);
                        w.bleeding = false;
                    }
                    _ => loss += w.bleed_per_h(t) * dt_h,
                }
                // Festering: one seeded roll per game hour of the wound's life.
                let mut k = hour + 1.0;
                while k <= w.age_h && !w.festering {
                    if !covered && hash01(seed, pid, w.id as u64, k as u64) < w.fester_chance(t) {
                        w.festering = true;
                    }
                    k += 1.0;
                }
                if w.festering {
                    loss += t.fester_hp_per_h * dt_h;
                }
                if !stopped {
                    w.healed_h += dt_h * w.heal_speed(t);
                }
            }
            h.wounds.retain(|w| w.healed_h < w.heal_h);
            let mut burnt_out = false;
            if let Some(i) = &mut h.infection {
                match i.stage {
                    InfectionStage::Hidden => {
                        i.left_h -= dt_h;
                        if i.left_h <= 0.0 {
                            (i.stage, i.left_h) = (InfectionStage::Feverish, i.feverish_h);
                        }
                    }
                    InfectionStage::Feverish => {
                        loss += inf.feverish_hp_per_h * dt_h;
                        i.left_h -= dt_h;
                        if i.left_h <= 0.0 {
                            if inf.fatal {
                                (i.stage, i.left_h) = (InfectionStage::Turning, 0.0);
                            } else {
                                burnt_out = true;
                            }
                        }
                    }
                    InfectionStage::Turning => loss += inf.turning_hp_per_h * dt_h,
                }
            }
            if burnt_out {
                h.infection = None;
            }
            h.food_poisoning_h = (h.food_poisoning_h - dt_h).max(0.0);
            // Hungry, nothing comes back (#11); asleep, it comes back faster.
            let regen = !h.healing_stopped() && h.losing_per_h(t) <= 0.0 && !p.needs.stops_healing(&self.config.needs);
            if regen && p.hp < max_hp {
                let rate = if p.needs.asleep() { t.regen_asleep_per_h } else { t.regen_awake_per_h };
                p.hp = (p.hp + rate * dt_h).min(max_hp);
            }
            if burnt_out && p.hp > inf.survivor_hp {
                // The fever leaves you weak.
                loss += p.hp - inf.survivor_hp;
            }
            let after = signature(&p.health, t);
            if loss > 0.0 {
                let cause = self.health_cause(pi);
                self.hurt_player_by(pi, loss, cause);
            }
            if before != after && !self.players[pi].dead {
                self.refresh_carry(pi);
            }
        }
    }

    /// Time skipped (`World::skip_time`): wounds and illness live through it in small steps.
    pub(crate) fn skip_health(&mut self, hours: f32) {
        let secs_per_h = 1.0 / self.game_hours_per_sec();
        let mut left = hours.max(0.0);
        while left > 0.0 {
            let dt = left.min(SKIP_STEP_H);
            self.step_health(dt, dt * secs_per_h);
            left -= dt;
        }
    }

    /// Player `id`'s health as its own client sees it (the hidden infection left out).
    pub fn health_view(&self, id: u32) -> Option<HealthView> {
        self.player(id).map(|p| p.health.view(&self.config.health))
    }

    /// Fold player health into the state hash.
    pub(crate) fn hash_health(h: &Health, mix: &mut impl FnMut(u32)) {
        mix(h.wounds.len() as u32 | h.next_wound << 8);
        for w in &h.wounds {
            mix(w.id ^ (w.part as u32) << 20 ^ (w.kind as u32) << 24);
            mix(w.age_h.to_bits());
            mix(w.heal_h.to_bits());
            mix(w.healed_h.to_bits());
            mix(w.dressing_age_h.to_bits());
            mix(w.bleeding as u32 | (w.festering as u32) << 1 | (w.cleaned as u32) << 2 | (w.stitched as u32) << 3 | (w.dressing as u32) << 4 | (w.plaster as u32) << 6);
            mix(w.clean_for_h.to_bits());
        }
        h.aid.hash(mix);
        if let Some(i) = h.infection {
            mix(i.stage as u32 | 0x1F00);
            mix(i.left_h.to_bits());
            mix(i.feverish_h.to_bits());
            mix(i.wound);
        }
        mix(h.food_poisoning_h.to_bits());
        mix(h.rise_in.map_or(u32::MAX, f32::to_bits));
    }
}

/// What the private view and the movement modifiers depend on, coarsely: when it changes, they're
/// sent / worked out again.
fn signature(h: &Health, t: &HealthTuning) -> u64 {
    let mut s: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        s ^= v;
        s = s.wrapping_mul(0x0100_0000_01b3);
    };
    for w in &h.wounds {
        mix(w.id as u64);
        mix((w.healed() * 20.0) as u64 | ((w.bleed_per_h(t) > 0.0) as u64) << 10 | (w.festering as u64) << 12 | (w.dressing as u64) << 13 | (w.cleaned as u64) << 16 | (w.stitched as u64) << 17 | (w.plaster as u64) << 18);
        mix(((w.heal_h - w.healed_h) / w.heal_speed(t).max(1e-3)).ceil() as u64);
        mix((w.clean_for_h - w.dressing_age_h).max(0.0).ceil() as u64);
    }
    mix(h.visible_infection().map_or(9, |s| s as u64));
    mix(h.food_poisoned() as u64);
    // First aid: what the view shows, in whole hours.
    let aid = h.aid.view(&t.first_aid);
    mix(aid.painkillers_h.map_or(u64::MAX, |x| x as u64) ^ (aid.drowsy as u64) << 40);
    if let Some(c) = aid.course {
        mix(c.doses as u64 | (c.next_in_h as u64) << 8 | c.lapses_in_h.map_or(0xFFFF, |x| x as u64) << 24 | (c.covered as u64) << 48);
    }
    mix((h.aid.sting_pain() * 100.0) as u64 | (h.aid.stings.len() as u64) << 32);
    h.aid.opened.iter().for_each(|o| mix(o.item as u64 | (o.used as u64) << 16));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TICK_DT;
    use crate::map::{Edge, Side, TileMap};
    use crate::players::PlayerInput;
    use crate::template::BuildingTemplate;
    use crate::world::WorldConfig;
    use crate::zombies::ZombieState;
    use crate::{PlayerState, items};
    use glam::IVec2;
    use std::f32::consts::PI;

    fn templates() -> Vec<BuildingTemplate> {
        [include_str!("../../../assets/data/buildings/bungalow.ron"), include_str!("../../../assets/data/buildings/family_two_storey.ron")]
            .iter()
            .map(|t| BuildingTemplate::from_ron(t).unwrap())
            .collect()
    }

    fn config(seed: u64) -> WorldConfig {
        let weapons = items::parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        let items = items::parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap();
        let mut c = WorldConfig::new(seed, 0, templates()).with_weapons(weapons).with_items(items);
        c.start_hour = 12.0;
        // Days go by in these tests: no starving meanwhile (#11 has its own).
        c.needs = crate::needs::NeedsTuning::off();
        c
    }

    /// An empty grass field at noon with one player at (100.5, 100.5) facing east.
    fn arena_with(config: WorldConfig) -> (World, u32) {
        let mut w = World::new(config);
        w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
        w.map_changed();
        w.items.clear();
        let id = w.add_player(Vec2::new(100.5, 100.5));
        (w, id)
    }

    fn arena(seed: u64) -> (World, u32) {
        arena_with(config(seed))
    }

    fn zombie(w: &mut World, pos: Vec2, facing: f32) -> usize {
        let i = w.add_zombie(pos, facing, 1.0);
        w.zombies.wander_timer[i] = 1e9;
        i
    }

    fn me(w: &World, id: u32) -> &Player {
        w.player(id).unwrap()
    }

    fn wear(w: &mut World, id: u32, item: &str) {
        let k = w.item_index(item).unwrap();
        let pi = w.players.iter().position(|p| p.id == id).unwrap();
        w.players[pi].inventory.worn.push(crate::ItemStack::one(k));
        w.refresh_carry(pi);
    }

    /// Strikes that land leave a wound on a body part, reported on the event; the upper body
    /// takes most of them, and every wound costs what its kind does.
    #[test]
    fn zombie_strikes_land_on_body_parts_with_wound_types() {
        let (mut w, id) = arena(5);
        w.config.infection.mode = InfectionMode::Off;
        zombie(&mut w, Vec2::new(101.2, 100.5), PI);
        let mut events = Vec::new();
        for _ in 0..20 * 120 {
            w.players[0].hp = 100.0;
            w.step();
            events.extend(w.combat_events.drain(..).filter(|e| e.kind == CombatEventKind::PlayerHurt));
            for a in w.attack_events.iter().filter(|a| a.landed) {
                assert!(a.part.is_some());
                let kind = a.wound.unwrap();
                let d = w.config.health.wound(kind).damage;
                assert!(a.damage >= d.0 && a.damage <= d.1, "{kind:?} for {}", a.damage);
                assert_ne!(kind, WoundKind::GlassCut);
            }
        }
        assert!(events.len() >= 30, "only {} strikes landed", events.len());
        assert!(events.iter().all(|e| e.part.is_some() && e.wound.is_some()));
        let upper = events.iter().filter(|e| matches!(e.part.unwrap().region(), BodyPart::Head | BodyPart::Torso | BodyPart::Arms | BodyPart::Hands)).count();
        assert!(upper as f32 > events.len() as f32 * 0.6, "{upper} of {} on the upper body", events.len());
        // No clothes on: nothing is only a bruise.
        assert!(events.iter().all(|e| e.wound != Some(WoundKind::Bruise)));
        let p = me(&w, id);
        assert!(!p.health.wounds.is_empty() && p.health.wounds.iter().all(|w| w.kind != WoundKind::Bruise));
    }

    /// Many strikes on the torso: a leather jacket turns a good share into bruises (most of the
    /// scratches, fewer of the bites), and with nothing on there are none.
    #[test]
    fn clothing_turns_strikes_into_bruises() {
        let bruises = |jacket: bool| {
            let (mut w, id) = arena(7);
            if jacket {
                wear(&mut w, id, "leather_jacket");
            }
            let (mut torso, mut bruised, mut bites, mut bitten) = (0, 0, 0, 0);
            for _ in 0..6000 {
                let (part, kind, _) = w.roll_zombie_wound(0, false, false, false);
                if part.region() == BodyPart::Torso || part.region() == BodyPart::Arms {
                    torso += 1;
                    bruised += (kind == WoundKind::Bruise) as u32;
                    bitten += (kind == WoundKind::Bite) as u32;
                    bites += 1;
                }
                // Legs: no protection either way.
                if part.is_leg() {
                    assert_ne!(kind, WoundKind::Bruise);
                }
            }
            (bruised as f32 / torso as f32, bitten as f32 / bites as f32)
        };
        let (bare, bare_bites) = bruises(false);
        let (jacket, jacket_bites) = bruises(true);
        assert_eq!(bare, 0.0);
        // 0.45 protection: about 0.45 × (0.5 + 0.3 × 0.7 + 0.2 × 0.5) ≈ 0.36 of hits stopped.
        assert!(jacket > 0.3 && jacket < 0.42, "jacket bruises {jacket}");
        assert!(jacket_bites < bare_bites, "bites {jacket_bites} vs {bare_bites}");
        // Layers combine: a jumper under the jacket stops more.
        let (mut w, id) = arena(7);
        wear(&mut w, id, "leather_jacket");
        let one = w.protection(me(&w, id), BodyPart::Torso);
        wear(&mut w, id, "jumper");
        let two = w.protection(me(&w, id), BodyPart::Torso);
        assert!((one - 0.45).abs() < 1e-5 && (two - (1.0 - 0.55 * 0.85)).abs() < 1e-5, "{one} {two}");
        assert_eq!(w.protection(me(&w, id), BodyPart::Legs), 0.0);
    }

    /// From behind: more on the head and torso, more bites. A zombie on the ground: only legs and feet.
    #[test]
    fn where_strikes_land_depends_on_direction() {
        let (mut w, _) = arena(9);
        let count = |w: &mut World, behind: bool, low: bool| {
            let (mut upper, mut bites, mut legs) = (0, 0, 0);
            for _ in 0..4000 {
                let (part, kind, _) = w.roll_zombie_wound(0, behind, false, low);
                upper += matches!(part, Part::Head | Part::Torso) as u32;
                bites += (kind == WoundKind::Bite) as u32;
                legs += part.is_leg() as u32;
            }
            (upper, bites, legs)
        };
        let (front_upper, front_bites, _) = count(&mut w, false, false);
        let (back_upper, back_bites, _) = count(&mut w, true, false);
        assert!(back_upper > front_upper + 200 && back_bites > front_bites + 200, "{front_upper}/{back_upper} {front_bites}/{back_bites}");
        assert_eq!(count(&mut w, false, true).2, 4000);
    }

    /// A scratch bleeds for an hour and stops; a gash keeps bleeding; HP comes back once nothing bleeds.
    #[test]
    fn bleeding_drains_and_can_stop() {
        let (mut w, id) = arena(11);
        let dmg = w.inflict_wound(id, Part::LeftArm, WoundKind::Scratch, Some(false)).unwrap();
        assert!((6.0..=9.0).contains(&dmg));
        let hp0 = me(&w, id).hp;
        assert!((hp0 - (100.0 - dmg)).abs() < 1e-3);
        w.skip_time(0.5);
        let hp1 = me(&w, id).hp;
        assert!((hp0 - hp1 - 1.5).abs() < 0.05, "half an hour of a scratch: {hp0} -> {hp1}");
        assert!(me(&w, id).health.wounds[0].bleeding);
        w.skip_time(0.6);
        let p = me(&w, id);
        assert!(!p.health.wounds[0].bleeding, "a scratch stops by itself");
        let hp2 = p.hp;
        // Up to half an hour more of bleeding, then a little back.
        assert!(hp1 - hp2 > 1.0 && hp1 - hp2 < 1.6, "{hp1} -> {hp2}");
        // Nothing bleeding: HP creeps back.
        w.skip_time(2.0);
        assert!((me(&w, id).hp - (hp2 + 4.0)).abs() < 0.05, "{} vs {}", me(&w, id).hp, hp2 + 4.0);
        // A gash bleeds 10 an hour and doesn't stop by itself.
        w.inflict_wound(id, Part::Torso, WoundKind::Gash, Some(false)).unwrap();
        let before = me(&w, id).hp;
        w.skip_time(3.0);
        let p = me(&w, id);
        assert!((before - p.hp - 30.0).abs() < 0.2, "{before} -> {}", p.hp);
        assert!(p.health.wounds.iter().any(|w| w.kind == WoundKind::Gash && w.bleeding));
        // And it kills, given time.
        w.skip_time(10.0);
        assert!(me(&w, id).dead);
        // Ticks do it too: one game hour of a bite is 14 HP.
        let (mut w, id) = arena(12);
        w.inflict_wound(id, Part::RightLeg, WoundKind::Bite, Some(false)).unwrap();
        let before = me(&w, id).hp;
        let hour = (1.0 / w.game_hours_per_sec() / TICK_DT).round() as u32;
        for _ in 0..hour {
            w.step();
        }
        assert!((before - me(&w, id).hp - 14.0).abs() < 0.1, "{before} -> {}", me(&w, id).hp);
        // #10's hook: a dressing stops a bite's bleeding, a gash's only to a quarter.
        let pi = 0;
        w.players[pi].health.wounds[0].dressing = Dressing::Clean;
        assert_eq!(w.players[pi].health.wounds[0].bleed_per_h(&w.config.health), 0.0);
        let mut gash = w.players[pi].health.wounds[0];
        gash.kind = WoundKind::Gash;
        assert_eq!(gash.bleed_per_h(&w.config.health), 2.5);
    }

    /// Wounds heal: a bruise in about 4 hours (±25%), and it's gone.
    #[test]
    fn wounds_heal_and_go() {
        let (mut w, id) = arena(13);
        w.inflict_wound(id, Part::Head, WoundKind::Bruise, None).unwrap();
        let wound = me(&w, id).health.wounds[0];
        assert!(wound.heal_h >= 3.0 && wound.heal_h <= 5.0 && !wound.bleeding);
        assert!(me(&w, id).health.pain(&w.config.health) > 0.09, "head pain counts double");
        w.skip_time(2.0);
        assert_eq!(me(&w, id).health.wounds.len(), 1);
        w.skip_time(3.1);
        assert!(me(&w, id).health.wounds.is_empty());
    }

    /// A gash on a leg slows walking and stops running; with it, movement still predicts exactly
    /// from the modifiers the player's state carries.
    #[test]
    fn leg_wounds_slow_movement() {
        let (mut w, id) = arena(15);
        let walk = |w: &World, run: bool| {
            let mut s: PlayerState = me(w, id).state;
            let start = s.pos;
            for _ in 0..20 {
                crate::players::step_movement(&w.map, &mut s, &PlayerInput { move_dir: Vec2::X, facing: 0.0, run, sneak: false }, TICK_DT);
            }
            (s.pos.distance(start), s.stance)
        };
        let (fresh, _) = walk(&w, false);
        let (fresh_run, stance) = walk(&w, true);
        assert_eq!(stance, crate::Stance::Run);
        w.inflict_wound(id, Part::LeftFoot, WoundKind::Scratch, Some(false)).unwrap();
        let (scratched, _) = walk(&w, false);
        let (scratched_run, stance) = walk(&w, true);
        assert!((scratched - fresh * 0.96).abs() < 0.01, "{fresh} -> {scratched}");
        assert!((scratched_run - fresh_run * 0.94).abs() < 0.01 && stance == crate::Stance::Run, "{fresh_run} -> {scratched_run}");
        w.inflict_wound(id, Part::RightLeg, WoundKind::Gash, Some(false)).unwrap();
        let (gashed, _) = walk(&w, false);
        let (_, stance) = walk(&w, true);
        assert!((gashed - fresh * (1.0 - 0.08 * 1.5)).abs() < 0.01, "{gashed}");
        assert_eq!(stance, crate::Stance::Walk, "no running on a gashed leg");
        // Caps at -30%.
        for _ in 0..5 {
            w.inflict_wound(id, Part::LeftLeg, WoundKind::GlassCut, Some(false)).unwrap();
        }
        assert!((walk(&w, false).0 - fresh * 0.7).abs() < 0.01);
        // The server moves the player the same.
        w.players[0].hp = 100.0;
        let mut predicted = me(&w, id).state;
        for t in 0..40u32 {
            let input = PlayerInput { move_dir: Vec2::from_angle(t as f32 * 0.1), facing: 0.0, run: t % 3 == 0, sneak: false };
            w.set_player_input(id, t + 1, input);
            crate::players::step_movement(&w.map, &mut predicted, &input, TICK_DT);
            w.step();
            assert_eq!(me(&w, id).state, predicted, "tick {t}");
        }
    }

    /// Wounded arms swing weaker and slower; a wounded torso slows getting your breath back.
    #[test]
    fn arm_and_torso_wounds_have_effects() {
        let (mut w, id) = arena(17);
        w.inflict_wound(id, Part::RightArm, WoundKind::Gash, Some(false)).unwrap();
        w.inflict_wound(id, Part::LeftHand, WoundKind::Scratch, Some(false)).unwrap();
        let e = w.health_effects(0);
        assert!((e.swing_power - (1.0 - 0.12 * 1.5)).abs() < 1e-4 && (e.swing_time - (1.0 + 0.08 * 1.5)).abs() < 1e-4, "{e:?}");
        assert!(e.walk == 1.0 && e.can_run && e.regen == 1.0);
        w.players[0].state.endurance = 1.0;
        assert!(w.player_attack(id));
        let a = me(&w, id).action.unwrap();
        // (The wounds' pain weakens it a little more, #11.)
        assert!((a.power - e.swing_power * w.config.needs.pain_power(w.pain_of(0))).abs() < 1e-4);
        let bat = w.config.weapons.iter().find(|x| x.category == crate::WeaponCategory::Unarmed).unwrap();
        assert!((me(&w, id).ready_in - bat.swing_seconds * e.swing_time).abs() < 1e-4);
        w.inflict_wound(id, Part::Torso, WoundKind::Bite, Some(false)).unwrap();
        assert!((me(&w, id).state.mods.regen - 0.8).abs() < 1e-4);
    }

    /// Untreated wounds fester now and then (seeded, the same every run), which costs HP and slows healing.
    #[test]
    fn untreated_wounds_fester() {
        let fester = |seed: u64| {
            let (mut w, id) = arena(seed);
            w.config.health.glass_cut.bleed_per_h = 0.0;
            w.players[0].hp = 1e6;
            for part in Part::ALL {
                w.inflict_wound(id, part, WoundKind::GlassCut, Some(false)).unwrap();
            }
            w.skip_time(10.0);
            let h = &me(&w, id).health;
            (h.wounds.iter().filter(|w| w.festering).count(), me(&w, id).hp, w.state_hash())
        };
        let (n, hp, hash) = fester(19);
        assert_eq!(fester(19), (n, hp, hash), "festering is seeded");
        // 10 wounds × 10 hours at 4%: about 3.4 of them.
        let total: usize = (19..29).map(|s| fester(s).0).sum();
        assert!((15..=55).contains(&total), "{total} festered of 100 over 10 hours");
        let (mut w, id) = arena(31);
        w.inflict_wound(id, Part::Torso, WoundKind::Scratch, Some(false)).unwrap();
        w.players[0].health.wounds[0].festering = true;
        let speed = w.players[0].health.wounds[0].heal_speed(&w.config.health);
        assert!((speed - 1.0 / 3.0).abs() < 1e-5);
        w.skip_time(2.0);
        let p = me(&w, id);
        assert!(p.health.pain(&w.config.health) > 0.15, "festering hurts");
        assert!(p.health.losing_per_h(&w.config.health) == 1.0, "a festering wound costs 1 HP an hour");
        assert!(p.hp < 100.0 - 6.0 - 1.9);
    }

    /// Bitten and infected: hidden for hours (the view doesn't show it), then feverish, then
    /// turning, then dead, and the body rises as a zombie. With `fatal` off it burns out and
    /// leaves you at 30 HP.
    #[test]
    fn infection_progresses_and_kills_by_default() {
        let (mut w, id) = arena(23);
        w.inflict_wound(id, Part::LeftArm, WoundKind::Bite, Some(true)).unwrap();
        w.players[0].health.wounds[0].bleeding = false;
        let i = me(&w, id).health.infection.unwrap();
        assert_eq!(i.stage, InfectionStage::Hidden);
        assert!((6.0..=12.0).contains(&i.left_h) && (12.0..=24.0).contains(&i.feverish_h));
        assert_eq!(w.health_view(id).unwrap().infection, None, "hidden from the player");
        assert!(!w.health_view(id).unwrap().tags.sick);
        w.skip_time(i.left_h + 0.05);
        assert_eq!(w.health_view(id).unwrap().infection, Some(InfectionStage::Feverish));
        assert!(me(&w, id).health.sick() && me(&w, id).state.mods.regen < 1.0);
        let healed = me(&w, id).health.wounds[0].healed_h;
        w.skip_time(1.0);
        assert_eq!(me(&w, id).health.wounds[0].healed_h, healed, "no healing while feverish");
        w.skip_time(i.feverish_h);
        assert_eq!(w.health_view(id).unwrap().infection, Some(InfectionStage::Turning));
        let zombies = w.zombies.len();
        let mut hours = 0.0;
        while !me(&w, id).dead {
            w.skip_time(0.01);
            hours += 0.01;
            assert!(hours < 30.0, "never died");
        }
        assert!(me(&w, id).health.rise_in.is_some());
        let pos = me(&w, id).state.pos;
        for _ in 0..(31.0 / TICK_DT) as u32 {
            w.step();
        }
        assert_eq!(w.zombies.len(), zombies + 1, "the body rose");
        assert!(w.zombies.pos[zombies].distance(pos) < 0.5 && w.zombies.state[zombies] != ZombieState::Dead);
        assert!(me(&w, id).health.rise_in.is_none());

        // Not fatal: through the fever and out, at 30 HP.
        let mut c = config(23);
        c.infection.fatal = false;
        let (mut w, id) = arena_with(c);
        w.inflict_wound(id, Part::LeftArm, WoundKind::Bite, Some(true)).unwrap();
        w.players[0].health.wounds[0].bleeding = false;
        w.players[0].hp = 90.0;
        let i = me(&w, id).health.infection.unwrap();
        w.skip_time(40.0);
        let p = me(&w, id);
        assert!(!p.dead && p.health.infection.is_none(), "{:?}", p.health.infection);
        // Back to 30 when it burnt out; since then 2 HP an hour back, or 1 an hour lost if the bite festered.
        let since = 40.0 - i.left_h - i.feverish_h;
        assert!(p.hp <= 30.0 + 2.0 * since + 0.2 && p.hp >= 30.0 - since - 0.2, "{} ({since} h since)", p.hp);
        // Off: a bite never infects.
        let mut c = config(23);
        c.infection.mode = InfectionMode::Off;
        let (mut w, id) = arena_with(c);
        for _ in 0..30 {
            w.players[0].hp = 100.0;
            w.inflict_wound(id, Part::Torso, WoundKind::Bite, None).unwrap();
        }
        assert!(me(&w, id).health.infection.is_none());
        // Default: most bites do (60%).
        let (mut w, _) = arena(23);
        let mut infected = 0;
        for k in 0..200u32 {
            let id = w.add_player(Vec2::new(90.5 + (k % 10) as f32, 90.5));
            w.inflict_wound(id, Part::Torso, WoundKind::Bite, None).unwrap();
            infected += me(&w, id).health.infection.is_some() as u32;
        }
        assert!((95..=145).contains(&infected), "{infected} of 200 bites infected");
    }

    /// God mode: strikes, glass and bleeding take no HP, and nobody turns.
    #[test]
    fn god_mode_prevents_hp_loss_from_wounds() {
        let (mut w, id) = arena(29);
        w.set_dev(id, Some(true), None);
        w.inflict_wound(id, Part::Torso, WoundKind::Bite, Some(true)).unwrap();
        w.inflict_wound(id, Part::LeftLeg, WoundKind::Gash, Some(false)).unwrap();
        assert_eq!(me(&w, id).health.wounds.len(), 2, "debug wounds stay on a god");
        for k in 0..6 {
            zombie(&mut w, Vec2::new(100.5, 100.5) + Vec2::from_angle(k as f32) * 0.8, k as f32 + PI);
        }
        for _ in 0..20 * 30 {
            w.step();
        }
        w.skip_time(60.0);
        let p = me(&w, id);
        assert!(!p.dead && p.hp == 100.0, "{}", p.hp);
        assert_eq!(p.health.wounds.len(), 2, "strikes leave no wounds on a god");
    }

    /// Smashing a window bare-handed cuts the hand more often than not; gloves mostly stop it.
    #[test]
    fn smashing_glass_bare_handed_cuts() {
        let cuts = |gloves: bool| {
            let (mut w, id) = arena(37);
            w.players[0].hp = 1e6;
            if gloves {
                wear(&mut w, id, "work_gloves");
            }
            let window = IVec2::new(101, 100);
            let mut n = 0;
            for _ in 0..200 {
                w.map.set_edge(window, Side::W, Edge::Window);
                assert!(w.player_smash_window(id).is_some());
                let wounds = me(&w, id).health.wounds.len();
                n += (wounds > 0) as u32;
                w.players[0].health.wounds.clear();
                w.refresh_carry(0);
            }
            n
        };
        let (bare, gloved) = (cuts(false), cuts(true));
        assert!((95..=145).contains(&bare), "{bare} of 200 cut bare-handed");
        assert!(gloved < 40, "{gloved} of 200 cut with gloves");
    }

    /// Climbing in through a broken window cuts now and then (a hand, arm or leg); through an
    /// open one never.
    #[test]
    fn climbing_through_broken_glass_cuts() {
        let climbs = |kind: Edge| {
            let (mut w, id) = arena(41);
            for y in 90..110 {
                w.map.set_edge(IVec2::new(103, y), Side::W, Edge::Wall);
            }
            w.map.set_edge(IVec2::new(103, 100), Side::W, kind);
            w.map_changed();
            let mut cut = 0;
            for k in 0..120u32 {
                w.players[0].hp = 100.0;
                w.players[0].health.wounds.clear();
                w.refresh_carry(0);
                w.players[0].state.pos = Vec2::new(101.8, 100.5);
                for t in 0..80u32 {
                    w.set_player_input(id, k * 100 + t + 1, PlayerInput { move_dir: Vec2::X, facing: 0.0, run: false, sneak: false });
                    w.step();
                }
                assert!(me(&w, id).state.pos.x > 103.5, "didn't get through");
                let h = &me(&w, id).health;
                cut += (!h.wounds.is_empty()) as u32;
                assert!(h.wounds.iter().all(|w| w.kind == WoundKind::GlassCut && !matches!(w.part, Part::Head | Part::Torso | Part::LeftFoot | Part::RightFoot)));
            }
            cut
        };
        let broken = climbs(Edge::WindowBroken);
        assert!((6..=34).contains(&broken), "{broken} of 120 climbs cut");
        assert_eq!(climbs(Edge::WindowOpen), 0);
    }

    /// Food poisoning (the hook #11 uses): sick for hours, can't eat, endurance slow to come back.
    #[test]
    fn food_poisoning_is_a_hook() {
        let (mut w, id) = arena(43);
        assert!(!w.poison_player(id, 0.0));
        assert!(w.poison_player(id, 1.0));
        let p = me(&w, id);
        assert!(!p.health.can_eat() && p.health.sick() && p.health.thirst_mult(&w.config.health) == 2.0);
        assert!((p.state.mods.regen - 0.7).abs() < 1e-5);
        assert!(w.health_view(id).unwrap().food_poisoning);
        w.skip_time(14.5);
        let p = me(&w, id);
        assert!(p.health.can_eat() && p.state.mods.regen == 1.0 && p.hp == 100.0);
    }

    /// Wounds, infection and all are saved: a loaded world steps exactly like the original.
    #[test]
    fn wounds_save_and_load_in_lockstep() {
        let mut w = World::new(config(47));
        let id = w.add_player(w.player_spawn);
        let at = w.player(id).unwrap().state.pos;
        w.inflict_wound(id, Part::LeftLeg, WoundKind::Gash, Some(false)).unwrap();
        w.inflict_wound(id, Part::Head, WoundKind::Bite, Some(true)).unwrap();
        w.poison_player(id, 1.0);
        w.players[0].health.wounds[0].dressing = Dressing::Dirty;
        w.players[0].health.wounds[0].festering = true;
        for k in 0..3 {
            zombie(&mut w, at + Vec2::from_angle(k as f32 * 2.0) * 0.8, k as f32 * 2.0 + PI);
        }
        for _ in 0..100 {
            w.step();
        }
        let mut loaded = World::load(&w.save(), &w.config).unwrap();
        assert_eq!(loaded.players, w.players);
        assert_eq!(loaded.state_hash(), w.state_hash());
        for t in 0..600u32 {
            let input = PlayerInput { move_dir: Vec2::from_angle(t as f32 * 0.05), facing: t as f32 * 0.05, run: t % 50 < 20, sneak: false };
            for world in [&mut w, &mut loaded] {
                world.set_player_input(id, t + 1, input);
                world.step();
            }
            assert_eq!(loaded.state_hash(), w.state_hash(), "tick {t}");
        }
        w.skip_time(5.0);
        loaded.skip_time(5.0);
        assert_eq!(loaded.state_hash(), w.state_hash());
    }

    /// The same fight twice is the same; wounds change the hash.
    #[test]
    fn wounds_are_deterministic_and_hashed() {
        let run = || {
            let (mut w, id) = arena(53);
            for k in 0..4 {
                zombie(&mut w, Vec2::new(100.5, 100.5) + Vec2::from_angle(k as f32 * 1.5) * 0.8, k as f32 * 1.5 + PI);
            }
            for _ in 0..20 * 20 {
                w.step();
            }
            let p = me(&w, id);
            (w.state_hash(), p.health.clone(), p.hp.to_bits())
        };
        let a = run();
        assert_eq!(a, run());
        assert!(!a.1.wounds.is_empty());
        let (mut w, id) = arena(53);
        let h = w.state_hash();
        w.inflict_wound(id, Part::Torso, WoundKind::Bruise, None);
        w.players[0].hp = 100.0;
        assert_ne!(w.state_hash(), h);
    }

    #[test]
    fn tunings_check_and_names_parse() {
        assert!(HealthTuning::default().check().is_ok() && InfectionTuning::default().check().is_ok());
        let bad = InfectionTuning { stage_mult: 5.0, ..Default::default() };
        assert!(bad.check().unwrap_err().contains("stage_mult"));
        let mut t = HealthTuning::default();
        t.gash.heal_h = 0.0;
        assert!(t.check().unwrap_err().contains("gash"));
        assert_eq!(Part::from_id("Left Arm"), Some(Part::LeftArm));
        assert_eq!(WoundKind::from_id("glass_cut"), Some(WoundKind::GlassCut));
        for p in Part::ALL {
            assert_eq!(Part::from_id(&p.id()), Some(p));
        }
        let settings = ron::to_string(&InfectionTuning { mode: InfectionMode::Custom { bite: 1.0, gash: 0.5, scratch: 0.1 }, ..Default::default() }).unwrap();
        let back: InfectionTuning = ron::from_str(&settings).unwrap();
        assert_eq!(back.chance(WoundKind::Scratch), 0.1);
    }
}
