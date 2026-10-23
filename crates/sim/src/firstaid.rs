//! First aid (#10): dressing wounds, cleaning them, stitching gashes, painkillers and antibiotics.
//!
//! A treatment ([`Treatment`]) is an item job (`JobKind::Treat`) like moving or eating: it's
//! checked when asked for ([`World::player_treat`]) and again when it's done, takes a few seconds
//! ([`FirstAidTuning::secs`]) during which the player stands still (its `MoveMods` say so, so
//! prediction stays exact), and is interrupted by a strike landing or by swinging, shoving or
//! stomping (which drop the whole queue).
//!
//! - **Dressings** (items tagged `plaster` or `bandage`; `clean` ones, and plasters, go on clean):
//!   stop a scratch, glass cut or bite bleeding at once; a dressed gash still bleeds at
//!   `HealthTuning::dressed_gash_bleed` until it's stitched. A plaster only covers a bruise or a
//!   scratch. A clean dressing turns dirty after [`FirstAidTuning::dressing_clean_h`] (a plaster
//!   [`FirstAidTuning::plaster_clean_h`]; a wound bleeding when it went on soils it in
//!   [`FirstAidTuning::bleeding_clean_h`]); dirty ones fester more (`HealthTuning::fester_dirty`).
//!   Dressing a dressed wound changes the dressing; taking one off ([`Treatment::Undress`]) gives a
//!   dirty rag back (not a plaster's).
//! - **Antiseptic** (`disinfects`) cleans an undressed wound: half the festering chance until its
//!   next dressing comes off. It stings a little for an hour.
//! - **Stitches** (`stitches`: the sewing kit) close a gash: no more bleeding, twice the healing.
//!   They hurt for two hours. A hand's dressing has to come off first.
//! - **Painkillers** (`painkiller`): less pain for a few hours; a second dose while one works makes
//!   you drowsy ([`Health::drowsy`], for #11).
//! - **Antibiotics** (`antibiotic`): a course of one pill a day; the last pill of the course cures
//!   every festering wound, and while a dose works no wound starts festering. They don't touch the
//!   zombie infection.
//!
//! Medical items have uses (`MedicalDef::uses`: a box of eight plasters). A player keeps count of
//! the uses gone from the opened one of each kind it carries ([`Aid::opened`]); the item is used up
//! with its last use. (Per-stack uses would follow the stack when dropped; that can come with the
//! food's uses, #11.)
//!
//! The rules on what can be done to which wound with which item are one pure function over the
//! view a client has ([`check`], [`options`], [`best_item`]), so the health panel offers only what
//! the server will do. Numbers are ours ([`FirstAidTuning`], in `HealthTuning::first_aid`).

use serde::{Deserialize, Serialize};

use crate::health::{Dressing, Health, Part, WoundKind, WoundView};
use crate::inventory::{Inventory, JobKind};
use crate::inventory::merge_into;
use crate::items::{ItemDef, ItemStack};
use crate::world::World;

/// Painkiller doses and treatment stings remembered at once (older ones are dropped).
pub const MAX_DOSES: usize = 8;
/// Opened medical items remembered at once.
pub const MAX_OPENED: usize = 32;
/// The item a dressing taken off becomes (if the content has it).
pub const RAG_ITEM: &str = "rag";

/// Something done for a wound, or taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Treatment {
    /// Put a dressing on (or change the one that's on).
    Dress,
    /// Antiseptic on an undressed wound.
    Clean,
    /// Stitch a gash.
    Stitch,
    Painkiller,
    Antibiotic,
    /// Take a dressing off.
    Undress,
}

impl Treatment {
    pub const ALL: [Treatment; 6] = [Self::Dress, Self::Clean, Self::Stitch, Self::Painkiller, Self::Antibiotic, Self::Undress];

    /// The id tools take.
    pub fn id(self) -> &'static str {
        match self {
            Self::Dress => "dress",
            Self::Clean => "clean",
            Self::Stitch => "stitch",
            Self::Painkiller => "painkillers",
            Self::Antibiotic => "antibiotics",
            Self::Undress => "remove",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        match id.as_str() {
            "painkiller" => Some(Self::Painkiller),
            "antibiotic" => Some(Self::Antibiotic),
            "undress" | "remove_dressing" | "take_off" => Some(Self::Undress),
            "bandage" => Some(Self::Dress),
            "disinfect" => Some(Self::Clean),
            _ => Self::ALL.into_iter().find(|t| t.id() == id),
        }
    }

    /// Done to a wound (else taken: pills).
    pub fn on_wound(self) -> bool {
        !matches!(self, Self::Painkiller | Self::Antibiotic)
    }

    /// Needs an item (taking a dressing off doesn't).
    pub fn needs_item(self) -> bool {
        self != Self::Undress
    }

    /// In words, for buttons and notes ("dress", "take painkillers").
    pub fn verb(self) -> &'static str {
        match self {
            Self::Dress => "dress",
            Self::Clean => "clean",
            Self::Stitch => "stitch",
            Self::Painkiller => "take painkillers",
            Self::Antibiotic => "take antibiotics",
            Self::Undress => "take the dressing off",
        }
    }

    /// What an item is for, by its tags.
    pub fn of_item(def: &ItemDef) -> Option<Self> {
        if is_dressing(def) {
            Some(Self::Dress)
        } else if def.has_tag("disinfects") {
            Some(Self::Clean)
        } else if def.has_tag("stitches") {
            Some(Self::Stitch)
        } else if def.has_tag("painkiller") {
            Some(Self::Painkiller)
        } else if def.has_tag("antibiotic") {
            Some(Self::Antibiotic)
        } else {
            None
        }
    }
}

pub fn is_dressing(def: &ItemDef) -> bool {
    def.has_tag("plaster") || def.has_tag("bandage")
}

pub fn is_plaster(def: &ItemDef) -> bool {
    def.has_tag("plaster")
}

/// A dressing that goes on clean (a plaster, gauze, a clean rag); a plain rag goes on dirty.
pub fn dresses_clean(def: &ItemDef) -> bool {
    is_plaster(def) || def.has_tag("clean")
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// Can `t` be done to wound `w` (`None` for pills) with item `def` (`None` to take a dressing
/// off)? If not, why, in words. The health panel offers only what passes; the server checks again
/// when the job is asked for and when it's done.
pub fn check(t: Treatment, w: Option<&WoundView>, def: Option<&ItemDef>) -> Result<(), String> {
    if t.needs_item() {
        let Some(d) = def else { return Err(format!("you have nothing to {} with", t.verb())) };
        if Treatment::of_item(d) != Some(t) {
            return Err(format!("you can't {} with the {}", t.verb(), lower(&d.name)));
        }
    }
    if !t.on_wound() {
        return Ok(());
    }
    let Some(w) = w else { return Err("which wound?".into()) };
    let kind = w.kind.name();
    match t {
        Treatment::Dress if def.is_some_and(is_plaster) && !matches!(w.kind, WoundKind::Bruise | WoundKind::Scratch) => Err(format!("a plaster won't cover a {kind}")),
        Treatment::Clean if w.kind == WoundKind::Bruise => Err("a bruise has nothing to clean".into()),
        Treatment::Clean if w.dressing != Dressing::None => Err("take the dressing off first".into()),
        Treatment::Clean if w.cleaned => Err(format!("the {kind} is clean already")),
        Treatment::Stitch if w.kind != WoundKind::Gash => Err(format!("only a gash takes stitches, not a {kind}")),
        Treatment::Stitch if w.stitched => Err("it's stitched already".into()),
        Treatment::Stitch if matches!(w.part, Part::LeftHand | Part::RightHand) && w.dressing != Dressing::None => Err("take the dressing off your hand first".into()),
        Treatment::Undress if w.dressing == Dressing::None => Err("there's no dressing on it".into()),
        _ => Ok(()),
    }
}

/// What a player can treat with: its carried stacks, then what's in its hands.
pub fn usable(inv: &Inventory) -> impl Iterator<Item = &ItemStack> {
    inv.carried.iter().chain(inv.hand.iter()).chain(inv.off_hand.iter())
}

/// Does `inv` have one of `item` to treat with?
pub fn carries(inv: &Inventory, item: u16) -> bool {
    usable(inv).any(|s| s.item == item)
}

/// The item kinds in `inv` that will do `t` to `w`, best first: for a dressing a plaster where
/// one will do (quick, and it stays clean longest), then clean dressings, then a rag.
pub fn items_for(t: Treatment, w: Option<&WoundView>, inv: &Inventory, items: &[ItemDef]) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for s in usable(inv) {
        if !out.contains(&s.item) && items.get(s.item as usize).is_some_and(|d| check(t, w, Some(d)).is_ok()) {
            out.push(s.item);
        }
    }
    if t == Treatment::Dress {
        let rank = |i: &u16| {
            let d = &items[*i as usize];
            if is_plaster(d) { 0 } else if dresses_clean(d) { 1 } else { 2 }
        };
        out.sort_by_key(rank);
    }
    out
}

/// The item a player would use for `t` on `w` from what it carries (see [`items_for`]).
pub fn best_item(t: Treatment, w: Option<&WoundView>, inv: &Inventory, items: &[ItemDef]) -> Option<u16> {
    items_for(t, w, inv, items).into_iter().next()
}

/// The item to try `t` on `w` with when none is named: the best that will do it, else any that
/// does `t` at all (so the refusal says what's wrong with the wound, not that there's no item).
pub fn pick_item(t: Treatment, w: Option<&WoundView>, inv: &Inventory, items: &[ItemDef]) -> Option<u16> {
    best_item(t, w, inv, items).or_else(|| usable(inv).map(|s| s.item).find(|i| items.get(*i as usize).and_then(Treatment::of_item) == Some(t)))
}

/// Everything a player can do for wound `w` (or take, with `None`) with what it carries: each
/// treatment with each item kind that will do it (`None`: no item, taking a dressing off).
pub fn options(w: Option<&WoundView>, inv: &Inventory, items: &[ItemDef]) -> Vec<(Treatment, Option<u16>)> {
    let mut out = Vec::new();
    for t in Treatment::ALL.into_iter().filter(|t| t.on_wound() == w.is_some()) {
        if !t.needs_item() {
            if check(t, w, None).is_ok() {
                out.push((t, None));
            }
            continue;
        }
        out.extend(items_for(t, w, inv, items).into_iter().map(|i| (t, Some(i))));
    }
    out
}

/// What a wound needs next, in words (the health panel and the MCP `health` tool).
pub fn needs(w: &WoundView) -> &'static str {
    match w {
        w if w.festering => "antibiotics (a course: one a day), and clean dressings changed when dirty",
        w if w.bleed_per_h > 0.0 && w.dressing == Dressing::None => "a dressing to stop the bleeding",
        w if w.bleed_per_h > 0.0 => "stitches (a sewing kit; a dressed gash still bleeds a little)",
        w if w.dressing == Dressing::Dirty => "a clean dressing (take the dirty one off; dirty ones fester)",
        w if w.kind != WoundKind::Bruise && w.dressing == Dressing::None && !w.cleaned => "antiseptic and a clean dressing, to keep it from festering",
        w if w.kind != WoundKind::Bruise && w.dressing == Dressing::None => "a clean dressing to keep it from festering",
        _ => "time",
    }
}

/// A treatment's pain: how much, and game hours left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sting {
    pub pain: f32,
    pub left_h: f32,
}

/// An antibiotics course under way (or just finished, while its last dose still works).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Course {
    /// Doses taken in it.
    pub doses: u8,
    /// Game hours since the last.
    pub since_h: f32,
}

/// Uses gone from the opened one of a medical item kind a player carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opened {
    pub item: u16,
    pub used: u8,
}

/// First aid in effect on a player (`Health::aid`). Saved with the player, in `state_hash`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Aid {
    /// Game hours left on each painkiller dose that's still working.
    pub painkillers: Vec<f32>,
    /// Pain treatments left (antiseptic, stitches), for a while.
    pub stings: Vec<Sting>,
    pub course: Option<Course>,
    pub opened: Vec<Opened>,
}

impl Aid {
    /// Pain taken away now.
    pub fn relief(&self, t: &FirstAidTuning) -> f32 {
        if self.painkillers.is_empty() { 0.0 } else { t.painkiller_relief }
    }

    pub fn sting_pain(&self) -> f32 {
        self.stings.iter().map(|s| s.pain).sum()
    }

    /// Two painkiller doses (or more) working at once.
    pub fn drowsy(&self, t: &FirstAidTuning) -> bool {
        self.painkillers.len() >= t.drowsy_doses.max(1) as usize
    }

    /// An antibiotic dose works: no wound starts festering.
    pub fn covered(&self, t: &FirstAidTuning) -> bool {
        self.course.is_some_and(|c| c.since_h < t.antibiotic_cover_h)
    }

    /// Uses left in the opened one of `item` (`None`: none opened).
    pub fn left(&self, item: u16, uses: u8) -> Option<u8> {
        self.opened.iter().find(|o| o.item == item).map(|o| uses.saturating_sub(o.used))
    }

    /// Why an antibiotic can't be taken now, if it can't.
    pub fn antibiotic_refusal(&self, t: &FirstAidTuning) -> Option<String> {
        let c = self.course?;
        (c.since_h < t.antibiotic_min_gap_h).then(|| format!("too soon for another antibiotic (the next in ~{:.0} h)", (t.antibiotic_min_gap_h - c.since_h).ceil().max(1.0)))
    }

    /// Time passes: doses and stings wear off, a course lapses.
    pub(crate) fn step(&mut self, dt_h: f32, t: &FirstAidTuning) {
        for d in &mut self.painkillers {
            *d -= dt_h;
        }
        self.painkillers.retain(|d| *d > 0.0);
        for s in &mut self.stings {
            s.left_h -= dt_h;
        }
        self.stings.retain(|s| s.left_h > 0.0);
        if let Some(c) = &mut self.course {
            c.since_h += dt_h;
            if c.since_h > t.antibiotic_max_gap_h.max(t.antibiotic_cover_h) {
                self.course = None;
            }
        }
    }

    /// Nothing in effect and nothing opened.
    pub fn is_empty(&self) -> bool {
        self.painkillers.is_empty() && self.stings.is_empty() && self.course.is_none() && self.opened.is_empty()
    }

    fn push_dose(list: &mut Vec<f32>, h: f32) {
        if list.len() >= MAX_DOSES {
            list.remove(0);
        }
        list.push(h);
    }

    fn sting(&mut self, pain: f32, h: f32) {
        if pain <= 0.0 || h <= 0.0 {
            return;
        }
        if self.stings.len() >= MAX_DOSES {
            self.stings.remove(0);
        }
        self.stings.push(Sting { pain, left_h: h });
    }

    pub(crate) fn hash(&self, mix: &mut impl FnMut(u32)) {
        mix(self.painkillers.len() as u32 | (self.stings.len() as u32) << 8 | (self.opened.len() as u32) << 16 | 0xA1 << 24);
        self.painkillers.iter().for_each(|d| mix(d.to_bits()));
        for s in &self.stings {
            mix(s.pain.to_bits());
            mix(s.left_h.to_bits());
        }
        if let Some(c) = self.course {
            mix(c.doses as u32 | 0xC0_0000);
            mix(c.since_h.to_bits());
        }
        self.opened.iter().for_each(|o| mix(o.item as u32 | (o.used as u32) << 16));
    }

    /// What the private view shows (hours rounded up; it is resent when they change).
    pub fn view(&self, t: &FirstAidTuning) -> AidView {
        let course = self.course.map(|c| CourseView {
            doses: c.doses,
            of: t.antibiotic_course.max(1),
            next_in_h: (t.antibiotic_min_gap_h - c.since_h).max(0.0).ceil(),
            lapses_in_h: (c.doses < t.antibiotic_course.max(1)).then(|| (t.antibiotic_max_gap_h - c.since_h).max(0.0).ceil()),
            covered: c.since_h < t.antibiotic_cover_h,
        });
        AidView {
            painkillers_h: self.painkillers.iter().copied().reduce(f32::max).map(f32::ceil),
            drowsy: self.drowsy(t),
            course,
            opened: self.opened.clone(),
        }
    }
}

/// The antibiotics course as its player sees it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CourseView {
    pub doses: u8,
    /// Doses the course takes.
    pub of: u8,
    /// Game hours until the next dose counts (0: now).
    pub next_in_h: f32,
    /// Game hours until the course lapses if no dose comes (`None`: finished).
    pub lapses_in_h: Option<f32>,
    /// A dose is working: no wound starts festering.
    pub covered: bool,
}

/// A player's first aid as its own client sees it (`HealthView::aid`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AidView {
    /// Game hours of painkiller relief left, rounded up (`None`: none working).
    pub painkillers_h: Option<f32>,
    pub drowsy: bool,
    pub course: Option<CourseView>,
    /// Uses gone from the opened one of each medical item kind.
    pub opened: Vec<Opened>,
}

/// First aid's numbers (`HealthTuning::first_aid`). Seconds of play for the treatments, game hours
/// for the rest. Ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FirstAidTuning {
    pub dress_secs: f32,
    pub plaster_secs: f32,
    pub clean_secs: f32,
    pub stitch_secs: f32,
    pub pill_secs: f32,
    pub remove_secs: f32,
    /// Dressing a hand (with the other one) takes this much longer.
    pub own_hand_mult: f32,
    /// Each First Aid level (#46) makes treatment this much faster; from `skill_stitch_level` up,
    /// stitching hurts half as much.
    pub skill_faster_per_level: f32,
    pub skill_stitch_level: u32,
    /// Hours a clean dressing stays clean: a plaster, any other, and any other put on a bleeding wound.
    pub plaster_clean_h: f32,
    pub dressing_clean_h: f32,
    pub bleeding_clean_h: f32,
    /// Pain cleaning and stitching cause, and for how many game hours.
    pub clean_pain: f32,
    pub clean_pain_h: f32,
    pub stitch_pain: f32,
    pub stitch_pain_h: f32,
    /// Pain a painkiller dose takes away, for how long; this many doses at once make you drowsy.
    pub painkiller_relief: f32,
    pub painkiller_h: f32,
    pub drowsy_doses: u32,
    /// Antibiotics: doses in a course; a dose counts from `min_gap` hours after the last, the
    /// course lapses after `max_gap` hours without one; each dose keeps festering off for `cover` hours.
    pub antibiotic_course: u8,
    pub antibiotic_min_gap_h: f32,
    pub antibiotic_max_gap_h: f32,
    pub antibiotic_cover_h: f32,
}

impl Default for FirstAidTuning {
    fn default() -> Self {
        Self {
            dress_secs: 4.0,
            plaster_secs: 2.0,
            clean_secs: 3.0,
            stitch_secs: 8.0,
            pill_secs: 1.0,
            remove_secs: 1.0,
            own_hand_mult: 1.5,
            skill_faster_per_level: 0.1,
            skill_stitch_level: 3,
            plaster_clean_h: 24.0,
            dressing_clean_h: 12.0,
            bleeding_clean_h: 6.0,
            clean_pain: 0.1,
            clean_pain_h: 1.0,
            stitch_pain: 0.2,
            stitch_pain_h: 2.0,
            painkiller_relief: 0.4,
            painkiller_h: 4.0,
            drowsy_doses: 2,
            antibiotic_course: 2,
            antibiotic_min_gap_h: 12.0,
            antibiotic_max_gap_h: 36.0,
            antibiotic_cover_h: 24.0,
        }
    }
}

impl FirstAidTuning {
    /// Seconds `t` on `w` with `def` takes at First Aid `level`.
    pub fn secs(&self, t: Treatment, w: Option<&WoundView>, def: Option<&ItemDef>, level: u32) -> f32 {
        let mut s = match t {
            Treatment::Dress if def.is_some_and(is_plaster) => self.plaster_secs,
            Treatment::Dress => self.dress_secs,
            Treatment::Clean => self.clean_secs,
            Treatment::Stitch => self.stitch_secs,
            Treatment::Painkiller | Treatment::Antibiotic => self.pill_secs,
            Treatment::Undress => self.remove_secs,
        };
        if let Some(w) = w
            && t == Treatment::Dress
        {
            if w.dressing != Dressing::None {
                s += self.remove_secs;
            }
            if matches!(w.part, Part::LeftHand | Part::RightHand) {
                s *= self.own_hand_mult;
            }
        }
        s * (1.0 - self.skill_faster_per_level * level as f32).clamp(0.3, 1.0)
    }

    pub fn check(&self) -> Result<(), String> {
        let secs = [
            ("dress_secs", self.dress_secs),
            ("plaster_secs", self.plaster_secs),
            ("clean_secs", self.clean_secs),
            ("stitch_secs", self.stitch_secs),
            ("pill_secs", self.pill_secs),
            ("remove_secs", self.remove_secs),
            ("own_hand_mult", self.own_hand_mult),
            ("plaster_clean_h", self.plaster_clean_h),
            ("dressing_clean_h", self.dressing_clean_h),
            ("bleeding_clean_h", self.bleeding_clean_h),
            ("clean_pain", self.clean_pain),
            ("clean_pain_h", self.clean_pain_h),
            ("stitch_pain", self.stitch_pain),
            ("stitch_pain_h", self.stitch_pain_h),
            ("painkiller_relief", self.painkiller_relief),
            ("painkiller_h", self.painkiller_h),
            ("antibiotic_min_gap_h", self.antibiotic_min_gap_h),
            ("antibiotic_max_gap_h", self.antibiotic_max_gap_h),
            ("antibiotic_cover_h", self.antibiotic_cover_h),
        ];
        if let Some((name, v)) = secs.iter().find(|(_, v)| !(v.is_finite() && *v >= 0.0 && *v <= 1000.0)) {
            return Err(format!("health.first_aid.{name} is {v} (0 to 1000)"));
        }
        if !(0.0..=0.2).contains(&self.skill_faster_per_level) {
            return Err(format!("health.first_aid.skill_faster_per_level is {} (0 to 0.2)", self.skill_faster_per_level));
        }
        if self.antibiotic_course == 0 || self.antibiotic_max_gap_h < self.antibiotic_min_gap_h {
            return Err("health.first_aid: antibiotic_course must be 1 or more, antibiotic_max_gap_h at least antibiotic_min_gap_h".into());
        }
        Ok(())
    }
}

impl Health {
    /// Two painkiller doses working at once (for #11's drowsiness).
    pub fn drowsy(&self, t: &crate::HealthTuning) -> bool {
        self.aid.drowsy(&t.first_aid)
    }
}

/// Player `pi` is busy treating (its first job is a treatment): it stands still.
pub(crate) fn treating(jobs: &[crate::ItemJob]) -> bool {
    jobs.first().is_some_and(|j| matches!(j.kind, JobKind::Treat { .. }))
}

impl World {
    /// The First Aid level of player `pi` (#46; everyone is 0 until skills exist).
    fn first_aid_level(&self, _pi: usize) -> u32 {
        0
    }

    fn wound_view(&self, pi: usize, wound: u32) -> Option<WoundView> {
        let h = &self.players[pi].health;
        let stopped = h.healing_stopped();
        h.wounds.iter().find(|w| w.id == wound).map(|w| w.view(&self.config.health, stopped))
    }

    /// Player `id` does `t` to its wound `wound` (`None` for pills) with `item` (an index into the
    /// items; `None`: the best it carries, see [`best_item`]). Queued like an item move: it happens
    /// after [`FirstAidTuning::secs`], standing still, if it still can then. Asking again for one
    /// already queued does nothing. Refusals are noted for its client and returned.
    pub fn player_treat(&mut self, id: u32, wound: Option<u32>, t: Treatment, item: Option<u16>) -> Result<(), String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        if self.players[pi].dead {
            return self.refuse(id, "dead".into());
        }
        let wound = if t.on_wound() { wound } else { None };
        let view = match wound {
            Some(w) => match self.wound_view(pi, w) {
                Some(v) => Some(v),
                None => return self.refuse(id, format!("no wound {w} (it may have healed)")),
            },
            None if t.on_wound() => return self.refuse(id, "which wound?".into()),
            None => None,
        };
        let inv = &self.players[pi].inventory;
        let item = if !t.needs_item() {
            None
        } else {
            match item {
                Some(i) if (i as usize) >= self.config.items.len() => return self.refuse(id, format!("no item {i}")),
                Some(i) if !carries(inv, i) => {
                    let name = lower(&self.item_def(i).name);
                    return self.refuse(id, format!("you don't have a {name} to hand"));
                }
                Some(i) => Some(i),
                None => match pick_item(t, view.as_ref(), inv, &self.config.items) {
                    Some(i) => Some(i),
                    None => return self.refuse(id, format!("you have nothing to {} with", t.verb())),
                },
            }
        };
        let def = item.map(|i| self.item_def(i));
        if let Err(why) = check(t, view.as_ref(), def) {
            return self.refuse(id, why);
        }
        let aid_t = &self.config.health.first_aid;
        if t == Treatment::Antibiotic
            && let Some(why) = self.players[pi].health.aid.antibiotic_refusal(aid_t)
        {
            return self.refuse(id, why);
        }
        if self.players[pi].jobs.iter().any(|j| matches!(j.kind, JobKind::Treat { wound: w, treatment, .. } if w == wound && treatment == t)) {
            // Already on it (a repeated click).
            return Ok(());
        }
        let secs = aid_t.secs(t, view.as_ref(), def, self.first_aid_level(pi));
        self.queue_job(id, JobKind::Treat { wound, treatment: t, item }, secs)
    }

    /// Strikes interrupt treating: a wound landing on player `pi` drops its queued treatments.
    pub(crate) fn interrupt_treatment(&mut self, pi: usize) {
        let p = &mut self.players[pi];
        let before = p.jobs.len();
        p.jobs.retain(|j| !matches!(j.kind, JobKind::Treat { .. }));
        if p.jobs.len() != before {
            let id = p.id;
            self.private_dirty.insert(id);
            self.note(id, false, "interrupted: you were hit".into());
        }
    }

    /// Carry out a treatment (a due `JobKind::Treat`), checked again: the wound still there, the
    /// item still to hand, it still fits.
    pub(crate) fn do_treat(&mut self, pi: usize, wound: Option<u32>, t: Treatment, item: Option<u16>) -> Result<String, String> {
        let view = match wound {
            Some(w) => Some(self.wound_view(pi, w).ok_or("the wound is gone (healed)")?),
            None if t.on_wound() => return Err("which wound?".into()),
            None => None,
        };
        let def = item.map(|i| self.item_def(i).clone());
        if let Some(d) = &def
            && !carries(&self.players[pi].inventory, item.expect("with def"))
        {
            return Err(format!("the {} is gone", lower(&d.name)));
        }
        check(t, view.as_ref(), def.as_ref())?;
        let at = self.config.health.first_aid.clone();
        if t == Treatment::Antibiotic
            && let Some(why) = self.players[pi].health.aid.antibiotic_refusal(&at)
        {
            return Err(why);
        }
        let left = match (item, &def) {
            (Some(i), Some(d)) => self.use_up(pi, i, d.medical.as_ref().map_or(1, |m| m.uses)),
            _ => None,
        };
        let item_name = def.as_ref().map_or(String::new(), |d| lower(&d.name));
        let uses = match left {
            Some(n) => format!(" ({n} {} left)", if n == 1 { "use" } else { "uses" }),
            None => String::new(),
        };
        let where_ = view.map_or(String::new(), |v| format!("the {} on your {}", v.kind.name(), v.part.name()));
        let level = self.first_aid_level(pi);
        let bleed = |w: &crate::Wound, ht: &crate::HealthTuning| w.bleed_per_h(ht) > 0.0;
        let ht = self.config.health.clone();
        let mut rag = false;
        let text = {
            let h = &mut self.players[pi].health;
            let wi = wound.and_then(|id| h.wounds.iter().position(|w| w.id == id));
            match t {
                Treatment::Dress => {
                    let d = def.as_ref().expect("checked");
                    let w = &mut h.wounds[wi.expect("checked")];
                    rag = w.dressing != Dressing::None && !w.plaster;
                    let bleeding = bleed(w, &ht);
                    w.dressing = if dresses_clean(d) { Dressing::Clean } else { Dressing::Dirty };
                    w.plaster = is_plaster(d);
                    w.dressing_age_h = 0.0;
                    w.clean_for_h = if w.plaster {
                        at.plaster_clean_h
                    } else if bleeding {
                        at.bleeding_clean_h
                    } else {
                        at.dressing_clean_h
                    };
                    // A dressing stops bleeding at once; a gash keeps a little until it's stitched.
                    if w.kind != WoundKind::Gash {
                        w.bleeding = false;
                    }
                    let state = if w.dressing == Dressing::Clean { "clean" } else { "dirty: it may fester" };
                    format!("{} {where_} with the {item_name}{uses}: {state}", if rag { "changed the dressing on" } else { "dressed" })
                }
                Treatment::Undress => {
                    let w = &mut h.wounds[wi.expect("checked")];
                    rag = !w.plaster;
                    (w.dressing, w.plaster, w.dressing_age_h, w.clean_for_h, w.cleaned) = (Dressing::None, false, 0.0, 0.0, false);
                    format!("took the dressing off {where_}{}", if rag { ": a dirty rag" } else { "" })
                }
                Treatment::Clean => {
                    h.wounds[wi.expect("checked")].cleaned = true;
                    h.aid.sting(at.clean_pain, at.clean_pain_h);
                    format!("cleaned {where_} with the {item_name}{uses}")
                }
                Treatment::Stitch => {
                    let w = &mut h.wounds[wi.expect("checked")];
                    (w.stitched, w.bleeding) = (true, false);
                    let pain = if level >= at.skill_stitch_level { at.stitch_pain * 0.5 } else { at.stitch_pain };
                    h.aid.sting(pain, at.stitch_pain_h);
                    format!("stitched {where_}{uses}")
                }
                Treatment::Painkiller => {
                    Aid::push_dose(&mut h.aid.painkillers, at.painkiller_h);
                    format!("took painkillers{uses}{}", if h.aid.drowsy(&at) { ": two doses, drowsy" } else { "" })
                }
                Treatment::Antibiotic => {
                    let of = at.antibiotic_course.max(1);
                    let doses = match h.aid.course {
                        Some(c) if c.doses < of => c.doses + 1,
                        _ => 1,
                    };
                    h.aid.course = Some(Course { doses, since_h: 0.0 });
                    if doses >= of {
                        let cured = h.wounds.iter().filter(|w| w.festering).count();
                        h.wounds.iter_mut().for_each(|w| w.festering = false);
                        format!("took an antibiotic{uses}: the course is done{}", if cured > 0 { format!(", {cured} festering wound{} cured", if cured == 1 { "" } else { "s" }) } else { String::new() })
                    } else {
                        format!("took an antibiotic{uses}: {doses} of {of}, the next in ~{:.0} h", at.antibiotic_min_gap_h)
                    }
                }
            }
        };
        if rag {
            self.give_back(pi, RAG_ITEM);
        }
        self.refresh_carry(pi);
        Ok(text)
    }

    /// One use of `item` (with `uses` per item) from player `pi`'s opened one; the last use takes
    /// the item. Returns the uses left in it (`None` for a one-use item).
    fn use_up(&mut self, pi: usize, item: u16, uses: u8) -> Option<u8> {
        let uses = uses.max(1);
        let aid = &mut self.players[pi].health.aid;
        let k = aid.opened.iter().position(|o| o.item == item);
        let used = k.map_or(0, |k| aid.opened[k].used).saturating_add(1);
        if used >= uses {
            if let Some(k) = k {
                aid.opened.remove(k);
            }
            take_one(&mut self.players[pi].inventory, item);
            return (uses > 1).then_some(0);
        }
        match k {
            Some(k) => aid.opened[k].used = used,
            None => {
                if aid.opened.len() >= MAX_OPENED {
                    aid.opened.remove(0);
                }
                aid.opened.push(Opened { item, used });
            }
        }
        Some(uses - used)
    }

    /// Give player `pi` one of item `id` (by data id): into its bags if there's room, else at its feet.
    fn give_back(&mut self, pi: usize, id: &str) {
        let Some(i) = self.item_index(id) else { return };
        let def = self.item_def(i);
        let (size, vol) = (def.stack, def.volume_l);
        let c = self.carry(&self.players[pi]);
        if c.used_l + vol <= c.room_l + 1e-4 {
            merge_into(&mut self.players[pi].inventory.carried, ItemStack::new(i, 1), 1, size);
        } else {
            let p = &self.players[pi];
            let (pos, angle) = (p.state.pos, p.state.facing);
            self.put_on_floor(ItemStack::one(i), pos, angle);
        }
    }
}

/// Take one of `item` from what `inv` treats with (carried first, then the hands).
fn take_one(inv: &mut Inventory, item: u16) -> bool {
    if let Some(k) = inv.carried.iter().position(|s| s.item == item) {
        inv.carried[k].count -= 1;
        if inv.carried[k].count == 0 {
            inv.carried.remove(k);
        }
        return true;
    }
    for slot in [&mut inv.hand, &mut inv.off_hand] {
        if slot.is_some_and(|s| s.item == item) {
            *slot = None;
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TICK_DT;
    use crate::health::{InfectionMode, Part, WoundKind};
    use crate::map::TileMap;
    use crate::players::{Player, PlayerInput};
    use crate::template::BuildingTemplate;
    use crate::world::WorldConfig;
    use crate::{PlayerState, items};
    use glam::Vec2;

    fn config(seed: u64) -> WorldConfig {
        let templates = [include_str!("../../../assets/data/buildings/bungalow.ron"), include_str!("../../../assets/data/buildings/family_two_storey.ron")]
            .iter()
            .map(|t| BuildingTemplate::from_ron(t).unwrap())
            .collect();
        let weapons = items::parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        let items = items::parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap();
        let mut c = WorldConfig::new(seed, 0, templates).with_weapons(weapons).with_items(items);
        c.start_hour = 12.0;
        c.infection.mode = InfectionMode::Off;
        c
    }

    /// An empty field at noon, one player at (100.5, 100.5).
    fn arena(seed: u64) -> (World, u32) {
        let mut w = World::new(config(seed));
        w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
        w.map_changed();
        w.items.clear();
        let id = w.add_player(Vec2::new(100.5, 100.5));
        (w, id)
    }

    fn me(w: &World, id: u32) -> &Player {
        w.player(id).unwrap()
    }

    /// A wound of this kind on this part (no infection); its id.
    fn wound(w: &mut World, id: u32, part: Part, kind: WoundKind) -> u32 {
        w.inflict_wound(id, part, kind, Some(false)).unwrap();
        me(w, id).health.wounds.last().unwrap().id
    }

    fn the(w: &World, id: u32, wound: u32) -> crate::Wound {
        *me(w, id).health.wounds.iter().find(|x| x.id == wound).unwrap()
    }

    fn item(w: &World, id: &str) -> u16 {
        w.item_index(id).unwrap()
    }

    fn count(w: &World, id: u32, it: &str) -> u32 {
        let i = item(w, it);
        me(w, id).inventory.stacks().filter(|s| s.item == i).map(|s| s.count as u32).sum()
    }

    fn last_note(w: &World, id: u32) -> String {
        w.item_notes.iter().rev().find(|n| n.player == id).map(|n| n.text.clone()).unwrap_or_default()
    }

    /// Step until player `id` has nothing queued; the seconds it took.
    fn settle(w: &mut World, id: u32) -> f32 {
        for k in 0..400 {
            if me(w, id).jobs.is_empty() {
                return k as f32 * TICK_DT;
            }
            w.step();
        }
        panic!("jobs never finished");
    }

    fn treat(w: &mut World, id: u32, wound: Option<u32>, t: Treatment, it: Option<&str>) -> f32 {
        let it = it.map(|s| item(w, s));
        w.player_treat(id, wound, t, it).unwrap_or_else(|e| panic!("{t:?} refused: {e}"));
        settle(w, id)
    }

    fn needs_of(w: &World, id: u32, wound: u32) -> String {
        let v = w.health_view(id).unwrap();
        needs(v.wounds.iter().find(|x| x.id == wound).unwrap()).to_string()
    }

    /// A dressing stops a bite or a glass cut bleeding at once and a gash to a quarter; stitches
    /// stop the gash; the HP drain follows. Treating takes the time it should, and dressing a hand
    /// is slower.
    #[test]
    fn dressing_and_stitching_stop_the_bleeding() {
        let (mut w, id) = arena(3);
        // Only the bleeding here (festering costs HP too).
        (w.config.health.fester_clean, w.config.health.fester_undressed) = (0.0, 0.0);
        w.give_item(id, "gauze", 2);
        w.give_item(id, "sewing_kit", 1);
        let gash = wound(&mut w, id, Part::LeftArm, WoundKind::Gash);
        let bite = wound(&mut w, id, Part::Torso, WoundKind::Bite);
        let t = w.config.health.clone();
        assert_eq!(me(&w, id).health.losing_per_h(&t), 24.0);
        assert!(needs_of(&w, id, bite).contains("a dressing"));
        let secs = treat(&mut w, id, Some(bite), Treatment::Dress, Some("gauze"));
        assert!((secs - 4.0).abs() < 0.11, "dressing takes 4 s, took {secs}");
        assert!(!the(&w, id, bite).bleeding && the(&w, id, bite).dressing == Dressing::Clean);
        assert_eq!(me(&w, id).health.losing_per_h(&t), 10.0, "only the gash bleeds");
        treat(&mut w, id, Some(gash), Treatment::Dress, None);
        assert!((me(&w, id).health.losing_per_h(&t) - 2.5).abs() < 1e-5, "a dressed gash bleeds a quarter");
        assert!(needs_of(&w, id, gash).contains("stitches"));
        // A gauze roll has three uses: two gone, one left.
        assert_eq!(count(&w, id, "gauze"), 2);
        assert_eq!(me(&w, id).health.aid.left(item(&w, "gauze"), 3), Some(1));
        let hp = me(&w, id).hp;
        w.skip_time(2.0);
        assert!((hp - me(&w, id).hp - 5.0).abs() < 0.05, "{hp} -> {}", me(&w, id).hp);
        let secs = treat(&mut w, id, Some(gash), Treatment::Stitch, Some("sewing_kit"));
        assert!((secs - 8.0).abs() < 0.11, "stitching takes 8 s, took {secs}");
        assert!(the(&w, id, gash).stitched && me(&w, id).health.losing_per_h(&t) == 0.0);
        assert!(last_note(&w, id).contains("stitched the gash on your left arm"), "{}", last_note(&w, id));
        let hp = me(&w, id).hp;
        w.skip_time(1.0);
        assert!(me(&w, id).hp > hp, "nothing bleeding: HP comes back");
        // A hand: half as slow again. A plaster won't cover a glass cut.
        let cut = wound(&mut w, id, Part::RightHand, WoundKind::GlassCut);
        w.give_item(id, "plasters", 1);
        let v = the(&w, id, cut).view(&t, false);
        assert!(check(Treatment::Dress, Some(&v), Some(w.item_def(item(&w, "plasters")))).unwrap_err().contains("plaster won't cover"));
        assert_eq!(best_item(Treatment::Dress, Some(&v), &me(&w, id).inventory, &w.config.items), Some(item(&w, "gauze")));
        let secs = treat(&mut w, id, Some(cut), Treatment::Dress, None);
        assert!((secs - 6.0).abs() < 0.11, "a hand takes 6 s, took {secs}");
        assert!(!the(&w, id, cut).bleeding);
        assert_eq!(count(&w, id, "gauze"), 1, "the roll's last use took it");
    }

    /// Clean dressings turn dirty with time (sooner on a wound that was bleeding); taking one off
    /// gives a dirty rag back (a plaster gives nothing); a rag goes on dirty; dressing a dressed
    /// wound changes the dressing.
    #[test]
    fn dressings_get_dirty_and_can_be_changed() {
        let (mut w, id) = arena(5);
        w.give_item(id, "gauze", 1);
        w.give_item(id, "plasters", 1);
        w.give_item(id, "rag", 1);
        let bite = wound(&mut w, id, Part::LeftLeg, WoundKind::Bite);
        let scratch = wound(&mut w, id, Part::RightArm, WoundKind::Scratch);
        w.players[0].health.wounds.iter_mut().find(|x| x.id == scratch).unwrap().bleeding = false;
        treat(&mut w, id, Some(bite), Treatment::Dress, Some("gauze"));
        treat(&mut w, id, Some(scratch), Treatment::Dress, None);
        assert!(the(&w, id, scratch).plaster, "a plaster where one will do");
        let view = w.health_view(id).unwrap();
        let v = |wid: u32| *view.wounds.iter().find(|x| x.id == wid).unwrap();
        assert_eq!((v(bite).dirty_in_h, v(scratch).dirty_in_h), (Some(6.0), Some(24.0)), "bleeding when dressed: dirty in 6 h");
        w.skip_time(6.1);
        assert_eq!(the(&w, id, bite).dressing, Dressing::Dirty);
        assert_eq!(the(&w, id, scratch).dressing, Dressing::Clean);
        assert!(needs_of(&w, id, bite).contains("clean dressing"));
        // Change it: the dirty gauze comes off as a rag, the rag goes on dirty.
        let rags = count(&w, id, "rag");
        treat(&mut w, id, Some(bite), Treatment::Dress, Some("rag"));
        assert_eq!(the(&w, id, bite).dressing, Dressing::Dirty, "a rag goes on dirty");
        assert_eq!(count(&w, id, "rag"), rags, "one rag on, the old dressing back as a rag");
        treat(&mut w, id, Some(bite), Treatment::Undress, None);
        assert_eq!(the(&w, id, bite).dressing, Dressing::None);
        assert_eq!(count(&w, id, "rag"), rags + 1);
        assert!(!the(&w, id, bite).bleeding, "a bite stays stopped once a dressing stopped it");
        let rags = count(&w, id, "rag");
        treat(&mut w, id, Some(scratch), Treatment::Undress, None);
        assert_eq!(count(&w, id, "rag"), rags, "a plaster gives nothing back");
        // A box of plasters has eight uses.
        assert_eq!(me(&w, id).health.aid.left(item(&w, "plasters"), 8), Some(7));
    }

    /// Dirty dressings fester much more than clean ones; antiseptic halves it (seeded, the same
    /// every run).
    #[test]
    fn dirty_dressings_fester_more_than_clean_ones() {
        let fester = |seed: u64, dressing: Dressing, cleaned: bool| {
            let (mut w, id) = arena(seed);
            w.config.health.glass_cut.bleed_per_h = 0.0;
            w.players[0].hp = 1e6;
            for part in Part::ALL {
                let k = wound(&mut w, id, part, WoundKind::GlassCut);
                let wd = w.players[0].health.wounds.iter_mut().find(|x| x.id == k).unwrap();
                (wd.dressing, wd.clean_for_h, wd.cleaned) = (dressing, 100.0, cleaned);
            }
            w.skip_time(10.0);
            me(&w, id).health.wounds.iter().filter(|x| x.festering).count()
        };
        let total = |d: Dressing, c: bool| (100..120).map(|s| fester(s, d, c)).sum::<usize>();
        assert_eq!(fester(101, Dressing::Dirty, false), fester(101, Dressing::Dirty, false), "seeded");
        let (dirty, clean, bare, cleaned) = (total(Dressing::Dirty, false), total(Dressing::Clean, false), total(Dressing::None, false), total(Dressing::None, true));
        // 200 wounds for 10 h: dirty 6%/h ≈ 92, clean 0.5%/h ≈ 10, undressed 4%/h ≈ 67, cleaned 2%/h ≈ 37.
        assert!((60..=125).contains(&dirty), "dirty {dirty}");
        assert!(clean <= 25 && dirty > clean * 3, "clean {clean} vs dirty {dirty}");
        assert!(cleaned < bare && (40..=95).contains(&bare), "cleaned {cleaned} vs bare {bare}");
    }

    /// Antiseptic cleans an undressed wound (not a bruise, not twice) and stings for an hour;
    /// stitches heal a gash twice as fast and hurt for two hours.
    #[test]
    fn antiseptic_and_stitches_change_healing() {
        let (mut w, id) = arena(7);
        w.give_item(id, "antiseptic", 1);
        w.give_item(id, "sewing_kit", 1);
        w.give_item(id, "gauze", 2);
        let t = w.config.health.clone();
        let gash = wound(&mut w, id, Part::Torso, WoundKind::Gash);
        let bruise = wound(&mut w, id, Part::Head, WoundKind::Bruise);
        assert!(w.player_treat(id, Some(bruise), Treatment::Clean, None).unwrap_err().contains("bruise"));
        let chance = the(&w, id, gash).fester_chance(&t);
        let pain = me(&w, id).health.pain(&t);
        treat(&mut w, id, Some(gash), Treatment::Clean, Some("antiseptic"));
        assert!(the(&w, id, gash).cleaned);
        assert!((the(&w, id, gash).fester_chance(&t) - chance * 0.5).abs() < 1e-6);
        assert!((me(&w, id).health.pain(&t) - pain - 0.1).abs() < 0.01, "antiseptic stings");
        assert!(w.player_treat(id, Some(gash), Treatment::Clean, None).unwrap_err().contains("clean already"));
        let heals = w.health_view(id).unwrap().wounds.iter().find(|x| x.id == gash).unwrap().heals_in_h.unwrap();
        treat(&mut w, id, Some(gash), Treatment::Stitch, None);
        let after = w.health_view(id).unwrap().wounds.iter().find(|x| x.id == gash).unwrap().heals_in_h.unwrap();
        assert!((after - (heals / 2.0).ceil()).abs() <= 1.0, "stitched heals in half: {heals} -> {after}");
        assert!(me(&w, id).health.pain(&t) > pain + 0.25, "stitches hurt");
        assert!(w.player_treat(id, Some(gash), Treatment::Stitch, None).unwrap_err().contains("stitched already"));
        // Only a gash takes stitches; a dressed hand's dressing has to come off first.
        let hand = wound(&mut w, id, Part::LeftHand, WoundKind::Gash);
        treat(&mut w, id, Some(hand), Treatment::Dress, None);
        assert!(w.player_treat(id, Some(hand), Treatment::Stitch, None).unwrap_err().contains("dressing off your hand"));
        assert!(w.player_treat(id, Some(bruise), Treatment::Stitch, None).unwrap_err().contains("only a gash"));
        // Cleaning lasts until the next dressing comes off.
        treat(&mut w, id, Some(gash), Treatment::Dress, None);
        treat(&mut w, id, Some(gash), Treatment::Undress, None);
        assert!(!the(&w, id, gash).cleaned);
        w.skip_time(2.1);
        assert!(me(&w, id).health.aid.stings.is_empty(), "the stings wear off");
    }

    /// Painkillers take pain away for four hours, then wear off; a second dose while one works
    /// makes you drowsy.
    #[test]
    fn painkillers_wear_off() {
        let (mut w, id) = arena(9);
        w.give_item(id, "painkillers", 1);
        let t = w.config.health.clone();
        for part in [Part::Torso, Part::LeftArm, Part::RightLeg] {
            let k = wound(&mut w, id, part, WoundKind::Bite);
            w.players[0].health.wounds.iter_mut().find(|x| x.id == k).unwrap().bleeding = false;
        }
        let pain = me(&w, id).health.pain(&t);
        assert!(pain > 0.5);
        let secs = treat(&mut w, id, None, Treatment::Painkiller, None);
        assert!((secs - 1.0).abs() < 0.11);
        let eased = me(&w, id).health.pain(&t);
        assert!((pain - eased - 0.4).abs() < 0.01, "{pain} -> {eased}");
        assert!(!me(&w, id).health.drowsy(&t));
        assert_eq!(w.health_view(id).unwrap().aid.painkillers_h, Some(4.0));
        w.skip_time(2.0);
        treat(&mut w, id, None, Treatment::Painkiller, None);
        assert!(me(&w, id).health.drowsy(&t) && last_note(&w, id).contains("drowsy"));
        assert!((pain - me(&w, id).health.pain(&t) - 0.4).abs() < 0.05, "doses don't add up");
        w.skip_time(2.1);
        assert!(!me(&w, id).health.drowsy(&t), "the first dose wore off");
        w.skip_time(2.0);
        let p = me(&w, id).health.pain(&t);
        assert!(me(&w, id).health.aid.painkillers.is_empty() && p > eased + 0.3, "worn off: {p}");
        assert_eq!(me(&w, id).health.aid.left(item(&w, "painkillers"), 12), Some(10));
    }

    /// One antibiotic a day, two days: the second dose cures festering. Too soon is refused (no
    /// pill lost); while a dose works nothing starts festering; a lapsed course starts again; the
    /// zombie infection stays.
    #[test]
    fn antibiotics_cure_festering_over_a_course() {
        let mut c = config(11);
        c.infection.mode = InfectionMode::Default;
        c.infection.stage_mult = 3.0;
        let mut w = World::new(c);
        w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
        w.map_changed();
        let id = w.add_player(Vec2::new(100.5, 100.5));
        w.give_item(id, "antibiotics", 1);
        w.players[0].hp = 1e6;
        let a = wound(&mut w, id, Part::LeftLeg, WoundKind::Scratch);
        w.inflict_wound(id, Part::Torso, WoundKind::Bite, Some(true)).unwrap();
        for wd in &mut w.players[0].health.wounds {
            (wd.festering, wd.bleeding) = (wd.id == a, false);
        }
        treat(&mut w, id, None, Treatment::Antibiotic, None);
        assert!(the(&w, id, a).festering, "one dose isn't a course");
        assert!(last_note(&w, id).contains("1 of 2"), "{}", last_note(&w, id));
        assert!(w.player_treat(id, None, Treatment::Antibiotic, None).unwrap_err().contains("too soon"));
        assert_eq!(me(&w, id).health.aid.left(item(&w, "antibiotics"), 7), Some(6), "refused: no pill lost");
        w.skip_time(12.1);
        treat(&mut w, id, None, Treatment::Antibiotic, None);
        assert!(!the(&w, id, a).festering, "cured: {}", last_note(&w, id));
        assert!(last_note(&w, id).contains("1 festering wound cured"), "{}", last_note(&w, id));
        assert!(me(&w, id).health.infection.is_some(), "antibiotics don't touch the infection");
        // While the last dose works (a day), nothing starts festering, even undressed.
        for _ in 0..8 {
            let k = wound(&mut w, id, Part::RightArm, WoundKind::GlassCut);
            w.players[0].health.wounds.iter_mut().find(|x| x.id == k).unwrap().bleeding = false;
        }
        w.skip_time(23.0);
        assert!(me(&w, id).health.wounds.iter().all(|x| !x.festering));
        // A lapsed course: the next pill is dose 1 again.
        w.skip_time(14.0);
        assert!(me(&w, id).health.aid.course.is_none());
        treat(&mut w, id, None, Treatment::Antibiotic, None);
        assert_eq!(me(&w, id).health.aid.course.unwrap().doses, 1);
    }

    /// Treating takes time, standing still; swinging drops it, and so does a strike landing.
    /// Nothing is used up when it's interrupted.
    #[test]
    fn treating_takes_time_and_fighting_interrupts_it() {
        let (mut w, id) = arena(13);
        w.give_item(id, "gauze", 1);
        let bite = wound(&mut w, id, Part::Torso, WoundKind::Bite);
        w.player_treat(id, Some(bite), Treatment::Dress, None).unwrap();
        // Asking again is nothing more.
        w.player_treat(id, Some(bite), Treatment::Dress, None).unwrap();
        assert_eq!(me(&w, id).jobs.len(), 1);
        for _ in 0..40 {
            w.step();
        }
        assert!(the(&w, id, bite).bleeding, "not done after 2 s");
        assert!(me(&w, id).state.mods.speed == 0.0, "standing still");
        assert!(w.player_attack(id));
        assert!(me(&w, id).jobs.is_empty());
        w.step();
        assert!(me(&w, id).state.mods.speed > 0.0, "free to move from the next tick");
        for _ in 0..100 {
            w.step();
        }
        assert!(the(&w, id, bite).dressing == Dressing::None && count(&w, id, "gauze") == 1);
        assert!(me(&w, id).health.aid.opened.is_empty(), "no use gone");
        // A strike landing interrupts it too.
        w.player_treat(id, Some(bite), Treatment::Dress, None).unwrap();
        w.step();
        w.wound_player(0, Part::LeftArm, WoundKind::Scratch, 5.0, Vec2::new(101.5, 100.5), 1);
        assert!(me(&w, id).jobs.is_empty() && last_note(&w, id).contains("interrupted"));
        w.step();
        assert!(me(&w, id).state.mods.speed > 0.0);
        // Bleeding on doesn't: only being hit.
        w.player_treat(id, Some(bite), Treatment::Dress, None).unwrap();
        settle(&mut w, id);
        assert_eq!(the(&w, id, bite).dressing, Dressing::Clean);
    }

    /// No item, no treatment: not one you don't have, not one that doesn't do it, and nothing for
    /// a pill you don't carry. The check is made again when it's due.
    #[test]
    fn cant_treat_with_what_you_dont_have() {
        let (mut w, id) = arena(17);
        let gash = wound(&mut w, id, Part::Torso, WoundKind::Gash);
        let gauze = item(&w, "gauze");
        assert!(w.player_treat(id, Some(gash), Treatment::Dress, Some(gauze)).unwrap_err().contains("don't have"));
        assert!(w.player_treat(id, Some(gash), Treatment::Dress, None).unwrap_err().contains("nothing to dress"));
        assert!(w.player_treat(id, None, Treatment::Painkiller, None).unwrap_err().contains("nothing to take painkillers"));
        w.give_item(id, "tinned_beans", 1);
        let beans = item(&w, "tinned_beans");
        assert!(w.player_treat(id, Some(gash), Treatment::Dress, Some(beans)).unwrap_err().contains("can't dress"));
        assert!(w.player_treat(id, Some(999), Treatment::Undress, None).unwrap_err().contains("no wound"));
        assert!(w.player_treat(id, None, Treatment::Dress, None).unwrap_err().contains("which wound"));
        assert!(me(&w, id).jobs.is_empty());
        // Gone by the time it's due: refused then.
        w.give_item(id, "gauze", 1);
        w.player_treat(id, Some(gash), Treatment::Dress, None).unwrap();
        w.players[0].inventory.carried.retain(|s| s.item != gauze);
        settle(&mut w, id);
        assert!(last_note(&w, id).contains("is gone"), "{}", last_note(&w, id));
        assert_eq!(the(&w, id, gash).dressing, Dressing::None);
        // In hand counts.
        w.hold_item(id, crate::Slot::Hand, "gauze");
        let v = the(&w, id, gash).view(&w.config.health, false);
        assert_eq!(options(Some(&v), &me(&w, id).inventory, &w.config.items), vec![(Treatment::Dress, Some(gauze))]);
        treat(&mut w, id, Some(gash), Treatment::Dress, None);
        assert_eq!(the(&w, id, gash).dressing, Dressing::Clean);
    }

    /// While treating the player stands still, and the movement predicted from its state (with the
    /// modifiers snapshots carry) matches the server's every tick, through the start and the end.
    #[test]
    fn prediction_matches_while_treating() {
        let (mut w, id) = arena(19);
        w.give_item(id, "gauze", 1);
        let gash = wound(&mut w, id, Part::LeftLeg, WoundKind::Gash);
        for t in 0..200u32 {
            if t == 20 {
                w.player_treat(id, Some(gash), Treatment::Dress, None).unwrap();
            }
            if t == 21 {
                assert_eq!(me(&w, id).state.mods.speed, 0.0, "standing still from the tick after");
            }
            let input = PlayerInput { move_dir: Vec2::from_angle(t as f32 * 0.03), facing: 0.0, run: t % 7 == 0, sneak: false };
            let mut predicted: PlayerState = me(&w, id).state;
            crate::players::step_movement(&w.map, &mut predicted, &input, TICK_DT);
            let before = me(&w, id).state.pos;
            w.set_player_input(id, t + 1, input);
            w.step();
            assert_eq!(me(&w, id).state.pos, predicted.pos, "tick {t}");
            if (21..95).contains(&t) {
                assert_eq!(me(&w, id).state.pos, before, "standing still while dressing (tick {t})");
            }
        }
        assert_eq!(the(&w, id, gash).dressing, Dressing::Clean);
        assert!(me(&w, id).state.mods.speed > 0.0);
    }

    /// Dressings, stings, painkillers, a course, opened items and a treatment under way are all
    /// saved: a loaded world steps exactly like the original; and the same play twice is the same.
    #[test]
    fn first_aid_saves_in_lockstep_and_is_deterministic() {
        let run = || {
            let mut w = World::new(config(23));
            let id = w.add_player(w.player_spawn);
            for it in ["gauze", "rag", "antiseptic", "painkillers", "antibiotics", "sewing_kit"] {
                w.give_item(id, it, 1);
            }
            let gash = wound(&mut w, id, Part::LeftArm, WoundKind::Gash);
            let bite = wound(&mut w, id, Part::Torso, WoundKind::Bite);
            treat(&mut w, id, Some(gash), Treatment::Clean, None);
            treat(&mut w, id, Some(gash), Treatment::Dress, Some("gauze"));
            treat(&mut w, id, Some(bite), Treatment::Dress, Some("rag"));
            treat(&mut w, id, None, Treatment::Painkiller, None);
            treat(&mut w, id, None, Treatment::Antibiotic, None);
            w.player_treat(id, Some(gash), Treatment::Stitch, None).unwrap();
            for _ in 0..30 {
                w.step();
            }
            (w, id)
        };
        let (mut w, id) = run();
        assert!(!me(&w, id).jobs.is_empty(), "stitching under way");
        assert_eq!(run().0.state_hash(), w.state_hash(), "deterministic");
        let mut loaded = World::load(&w.save(), &w.config).unwrap();
        assert_eq!(loaded.players, w.players);
        assert_eq!(loaded.state_hash(), w.state_hash());
        assert_eq!(loaded.player(id).unwrap().state.mods, me(&w, id).state.mods, "still standing still after loading");
        for t in 0..400u32 {
            let input = PlayerInput { move_dir: Vec2::from_angle(t as f32 * 0.05), facing: t as f32 * 0.05, run: false, sneak: false };
            for world in [&mut w, &mut loaded] {
                world.set_player_input(id, t + 1, input);
                world.step();
            }
            assert_eq!(loaded.state_hash(), w.state_hash(), "tick {t}");
        }
        assert!(me(&w, id).health.wounds[0].stitched);
        w.skip_time(14.0);
        loaded.skip_time(14.0);
        assert_eq!(loaded.state_hash(), w.state_hash());
        // A treatment changes the hash.
        let h = w.state_hash();
        w.player_treat(id, None, Treatment::Antibiotic, None).unwrap();
        assert_ne!(w.state_hash(), h);
    }

    #[test]
    fn tuning_checks_and_ids_parse() {
        assert!(FirstAidTuning::default().check().is_ok());
        let bad = FirstAidTuning { stitch_secs: -1.0, ..Default::default() };
        assert!(bad.check().unwrap_err().contains("stitch_secs"));
        for t in Treatment::ALL {
            assert_eq!(Treatment::from_id(t.id()), Some(t));
        }
        assert_eq!(Treatment::from_id("Painkiller"), Some(Treatment::Painkiller));
        let (w, _) = arena(1);
        let of = |id: &str| Treatment::of_item(w.item_def(item(&w, id)));
        assert_eq!(
            ["plasters", "gauze", "rag", "clean_rag", "antiseptic", "painkillers", "antibiotics", "sewing_kit", "tinned_beans"].map(of),
            [Some(Treatment::Dress), Some(Treatment::Dress), Some(Treatment::Dress), Some(Treatment::Dress), Some(Treatment::Clean), Some(Treatment::Painkiller), Some(Treatment::Antibiotic), Some(Treatment::Stitch), None]
        );
        // Settings saved before first aid read with its defaults.
        let t: crate::HealthTuning = ron::from_str("(heal_spread: 0.2)").unwrap();
        assert_eq!(t.first_aid, FirstAidTuning::default());
    }
}
