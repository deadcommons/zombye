//! Status icons (Linear DCM-57): what the HUD shows instead of stat bars.
//!
//! [`status_tags`] turns a player's state ([`StatusInput`]: all of it in the player's own client's
//! views) into the icons that apply, each a [`StatusTag`] with a label, a severity (1 to 4; 0 for a
//! good state such as "Well fed") and a one-line tip. **Which icons there are and where their levels
//! start is data** ([`StatusData`], `assets/data/status.ron`, checked by [`parse_status`]): this
//! file only says what each kind's value is ([`value`]). The shipped file is also built in
//! ([`StatusData::builtin`], what `World::status_tags` and so the MCP tools use); a client reads
//! its own data folder's (`net::content::load_status`).
//!
//! Display only: nothing in the simulation reads a tag, so the file changes no world.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::health::InfectionStage;
use crate::needs::{NeedsTuning, NeedsView};
use crate::world::World;

/// What kind of status an icon is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TagKind {
    Health,
    Hunger,
    Thirst,
    Tiredness,
    Endurance,
    Load,
    Pain,
    Bleeding,
    Sick,
    Queasy,
    Asleep,
}

impl TagKind {
    pub const ALL: [TagKind; 11] = [
        TagKind::Health,
        TagKind::Hunger,
        TagKind::Thirst,
        TagKind::Tiredness,
        TagKind::Endurance,
        TagKind::Load,
        TagKind::Pain,
        TagKind::Bleeding,
        TagKind::Sick,
        TagKind::Queasy,
        TagKind::Asleep,
    ];
}

/// The most levels an icon can have, and the worst severity.
pub const MAX_SEVERITY: u8 = 4;

/// One status icon: a short label, how bad (1 mild to 4 worst; 0 a good state) and, for hovering,
/// one line saying what it means.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusTag {
    pub kind: TagKind,
    pub label: String,
    pub severity: u8,
    pub tip: String,
}

/// One level of an icon in `status.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelDef {
    /// The value at which this level starts.
    pub at: f32,
    pub label: String,
    #[serde(default)]
    pub tip: String,
    /// 1 to 4; 0 (left out): its place in the list.
    #[serde(default)]
    pub severity: u8,
}

/// An icon's good state: shown dimmer while the value is below `below`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoodDef {
    pub below: f32,
    pub label: String,
    #[serde(default)]
    pub tip: String,
}

/// One icon in `status.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IconDef {
    pub kind: TagKind,
    #[serde(default)]
    pub good: Option<GoodDef>,
    pub levels: Vec<LevelDef>,
}

/// `status.ron`: the icons, in the order they are shown.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusData {
    pub icons: Vec<IconDef>,
}

/// Parse and check `status.ron`; errors name the icon. Severities left out are filled in.
pub fn parse_status(text: &str) -> Result<StatusData, String> {
    let mut data: StatusData = ron::from_str(text).map_err(|e| format!("status: {e}"))?;
    for i in 0..data.icons.len() {
        let kind = data.icons[i].kind;
        if data.icons[..i].iter().any(|o| o.kind == kind) {
            return Err(format!("status: {kind:?} is listed twice"));
        }
        let icon = &mut data.icons[i];
        if icon.levels.is_empty() || icon.levels.len() > MAX_SEVERITY as usize {
            return Err(format!("status: {kind:?} has {} levels (1 to {MAX_SEVERITY})", icon.levels.len()));
        }
        let (mut last_at, mut last_severity) = (f32::NEG_INFINITY, 0);
        for (n, l) in icon.levels.iter_mut().enumerate() {
            if l.severity == 0 {
                l.severity = n as u8 + 1;
            }
            if l.label.trim().is_empty() {
                return Err(format!("status: {kind:?} level {} has no label", n + 1));
            }
            if !l.at.is_finite() || l.at <= last_at {
                return Err(format!("status: {kind:?} level {} ({}) starts at {}: each level must start higher than the last", n + 1, l.label, l.at));
            }
            if l.severity > MAX_SEVERITY || l.severity < last_severity {
                return Err(format!("status: {kind:?} level {} ({}) has severity {}: 1 to {MAX_SEVERITY}, never less than the level before", n + 1, l.label, l.severity));
            }
            (last_at, last_severity) = (l.at, l.severity);
        }
        if let Some(g) = &icon.good {
            if g.label.trim().is_empty() {
                return Err(format!("status: {kind:?}'s good state has no label"));
            }
            if !g.below.is_finite() || g.below > icon.levels[0].at {
                return Err(format!("status: {kind:?}'s good state ({}) is below {}: it must end before the first level starts ({})", g.label, g.below, icon.levels[0].at));
            }
        }
    }
    Ok(data)
}

impl StatusData {
    /// The shipped `assets/data/status.ron`, compiled in (no file is read at run time).
    pub fn builtin() -> &'static StatusData {
        static DATA: OnceLock<StatusData> = OnceLock::new();
        DATA.get_or_init(|| parse_status(include_str!("../../../assets/data/status.ron")).expect("the shipped status.ron is valid"))
    }

    pub fn icon(&self, kind: TagKind) -> Option<&IconDef> {
        self.icons.iter().find(|i| i.kind == kind)
    }
}

/// What the status icons are worked out from: all of it is in the player's own client's views.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatusInput {
    pub needs: NeedsView,
    /// The share of full HP lost, 0..1.
    pub hp_lost: f32,
    /// The wounds' pain (`HealthView::pain`); soreness is added from `needs`.
    pub pain: f32,
    /// A painkiller dose is working (`AidView::painkillers_h`): agony doesn't stop sleep.
    pub painkillers: bool,
    /// HP per game hour lost to bleeding.
    pub bleeding: f32,
    pub food_poisoned: bool,
    pub festering: bool,
    pub infection: Option<InfectionStage>,
    pub endurance: f32,
    pub winded: bool,
    /// The weight carried over the carry limit (1 = at the limit).
    pub load: f32,
    /// The game time scale now (`World::time_scale`).
    pub time_scale: f32,
}

/// The number `kind`'s levels in `status.ron` are measured against (the file says the same).
pub fn value(kind: TagKind, s: &StatusInput) -> f32 {
    let flag = |on: bool| if on { 1.0 } else { 0.0 };
    match kind {
        TagKind::Health => s.hp_lost,
        TagKind::Hunger => s.needs.hunger,
        TagKind::Thirst => s.needs.thirst,
        TagKind::Tiredness => s.needs.tiredness,
        TagKind::Endurance => (1.0 - s.endurance).clamp(0.0, 1.0) + flag(s.winded),
        TagKind::Load => s.load,
        TagKind::Pain => s.pain + s.needs.sore,
        TagKind::Bleeding => s.bleeding,
        TagKind::Sick => match s.infection {
            Some(InfectionStage::Turning) => 3.0,
            Some(InfectionStage::Feverish) => 2.0,
            _ => flag(s.festering),
        },
        TagKind::Queasy => flag(s.food_poisoned),
        TagKind::Asleep => flag(s.needs.asleep.is_some()),
    }
}

/// The level of `icon` a value has reached (an index into its levels). The load passes a level
/// only when over it, as the burden stages do (`CarryTuning::burden`).
pub fn level_of(icon: &IconDef, v: f32) -> Option<usize> {
    let over = icon.kind == TagKind::Load;
    icon.levels.iter().rposition(|l| if over { v > l.at } else { v >= l.at })
}

/// The status icons that apply, in the file's order. `t` only for what sleep makes of agony.
pub fn status_tags(data: &StatusData, t: &NeedsTuning, s: &StatusInput) -> Vec<StatusTag> {
    let mut out = Vec::new();
    for icon in &data.icons {
        let kind = icon.kind;
        let v = value(kind, s);
        let line = |label: &str, tip: &str| if tip.is_empty() { label.to_string() } else { format!("{label}: {tip}") };
        let Some(l) = level_of(icon, v).map(|i| &icon.levels[i]) else {
            if let Some(g) = icon.good.as_ref().filter(|g| v < g.below) {
                out.push(StatusTag { kind, label: g.label.clone(), severity: 0, tip: line(&g.label, &g.tip) });
            }
            continue;
        };
        let (mut label, mut tip) = (l.label.clone(), line(&l.label, &l.tip));
        match kind {
            // Agony and sleep: the one thing a tip says that depends on something else.
            TagKind::Pain if t.pain_level(v) >= 3 => {
                tip.push_str(if s.painkillers { " The painkillers let you sleep until they wear off." } else { " You can't sleep without painkillers." });
            }
            TagKind::Asleep => {
                if let Some(spot) = s.needs.asleep {
                    tip = line(&format!("{} on the {}", l.label, spot.name()), &l.tip);
                }
                if s.time_scale > 1.0 {
                    label = format!("{label} x{:.0}", s.time_scale);
                    tip.push_str(&format!(" Time runs {:.0}x as fast.", s.time_scale));
                }
            }
            _ => {}
        }
        out.push(StatusTag { kind, label, severity: l.severity, tip });
    }
    out
}

/// The status icons as one line ("Hungry, Tired, Bleeding"), good states left out.
pub fn tags_line(tags: &[StatusTag]) -> String {
    tags.iter().filter(|t| t.severity > 0).map(|t| t.label.as_str()).collect::<Vec<_>>().join(", ")
}

impl World {
    /// What player `pi`'s status icons are worked out from.
    pub fn status_input(&self, pi: usize) -> StatusInput {
        let p = &self.players[pi];
        let h = &self.config.health;
        let carry = self.carry(p);
        StatusInput {
            needs: p.needs.view(&self.config.needs),
            hp_lost: 1.0 - (p.hp / self.config.combat.player_hp.max(1.0)).clamp(0.0, 1.0),
            pain: p.health.pain(h),
            painkillers: !p.health.aid.painkillers.is_empty(),
            bleeding: p.health.wounds.iter().fold(0.0, |hp, w| hp + w.bleed_per_h(h)),
            food_poisoned: p.health.food_poisoned(),
            festering: p.health.wounds.iter().any(|w| w.festering),
            infection: p.health.visible_infection(),
            endurance: p.state.endurance,
            winded: p.state.winded,
            load: carry.weight_kg / carry.limit_kg.max(0.01),
            time_scale: self.time_scale(),
        }
    }

    /// Player `id`'s status icons by the built-in `status.ron` (the MCP tools'; the HUD works its
    /// own out from its views and its data folder's file).
    pub fn status_tags(&self, id: u32) -> Vec<StatusTag> {
        self.status_tags_with(id, StatusData::builtin())
    }

    pub fn status_tags_with(&self, id: u32, data: &StatusData) -> Vec<StatusTag> {
        match self.players.iter().position(|p| p.id == id) {
            Some(pi) if !self.players[pi].dead => status_tags(data, &self.config.needs, &self.status_input(pi)),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::needs::SleepSpot;

    fn fine() -> StatusInput {
        StatusInput {
            needs: NeedsView { hunger: 0.2, thirst: 0.2, tiredness: 0.2, asleep: None, sore: 0.0 },
            hp_lost: 0.0,
            pain: 0.0,
            painkillers: false,
            bleeding: 0.0,
            food_poisoned: false,
            festering: false,
            infection: None,
            endurance: 1.0,
            winded: false,
            load: 0.5,
            time_scale: 1.0,
        }
    }

    fn tags(s: &StatusInput) -> Vec<StatusTag> {
        status_tags(StatusData::builtin(), &NeedsTuning::default(), s)
    }

    fn one(s: &StatusInput, kind: TagKind) -> Option<(String, u8)> {
        tags(s).into_iter().find(|t| t.kind == kind).map(|t| (t.label, t.severity))
    }

    #[test]
    fn the_shipped_file_has_every_icon_and_matches_the_worlds_settings() {
        let data = StatusData::builtin();
        for kind in TagKind::ALL {
            let icon = data.icon(kind).unwrap_or_else(|| panic!("no {kind:?} icon"));
            assert!(icon.levels.iter().all(|l| (1..=MAX_SEVERITY).contains(&l.severity) && !l.tip.is_empty()), "{kind:?}");
        }
        // The icons change where the effects do: every threshold the world's settings have is a level.
        let at = |kind| data.icon(kind).unwrap().levels.iter().map(|l| l.at).collect::<Vec<_>>();
        let n = NeedsTuning::default();
        for kind in [TagKind::Hunger, TagKind::Thirst, TagKind::Tiredness] {
            assert!([n.mild, n.strong, n.severe].iter().all(|v| at(kind).contains(v)), "{kind:?}");
        }
        assert!([n.pain_sore, n.pain_hurting, n.pain_agony].iter().all(|v| at(TagKind::Pain).contains(v)));
        let c = crate::inventory::CarryTuning::default();
        assert!([c.burdened_at, c.heavy_at, c.overloaded_at].iter().all(|v| at(TagKind::Load).contains(v)));
        assert!(at(TagKind::Endurance).contains(&(1.0 - crate::combat::CombatTuning::default().tired_below)));
    }

    #[test]
    fn nothing_shows_until_a_level_is_passed() {
        assert_eq!(tags(&fine()), []);
        // Good states show dimmer (severity 0), and are left out of the line.
        let fed = StatusInput { needs: NeedsView { hunger: 0.0, tiredness: 0.01, ..fine().needs }, ..fine() };
        let t = tags(&fed);
        assert_eq!(t.iter().map(|t| (t.label.as_str(), t.severity)).collect::<Vec<_>>(), [("Well fed", 0), ("Rested", 0)]);
        assert_eq!(tags_line(&t), "");
        assert!(t[0].tip.starts_with("Well fed: "));
    }

    #[test]
    fn icons_worsen_and_ease_through_four_levels() {
        let thirst = |v: f32| one(&StatusInput { needs: NeedsView { thirst: v, ..fine().needs }, ..fine() }, TagKind::Thirst);
        let seen: Vec<_> = [0.34, 0.35, 0.6, 0.75, 0.85, 1.0, 0.8, 0.5, 0.1].iter().map(|&v| thirst(v)).collect();
        let l = |s: &str, n: u8| Some((s.to_string(), n));
        assert_eq!(seen, [None, l("Dry", 1), l("Thirsty", 2), l("Very thirsty", 3), l("Parched", 4), l("Parched", 4), l("Very thirsty", 3), l("Dry", 1), None]);
        let health = |lost: f32| one(&StatusInput { hp_lost: lost, ..fine() }, TagKind::Health);
        assert_eq!([0.05, 0.1, 0.5, 0.7, 0.95].map(health), [None, l("Hurt", 1), l("Wounded", 2), l("Badly wounded", 3), l("Near death", 4)]);
        let bleeding = |per_h: f32| one(&StatusInput { bleeding: per_h, ..fine() }, TagKind::Bleeding);
        assert_eq!([0.0, 3.0, 10.0, 14.0, 24.0].map(bleeding), [None, l("Bleeding a little", 1), l("Bleeding", 2), l("Bleeding badly", 3), l("Bleeding out", 4)]);
        // The load passes a stage only when over it, as the burden does.
        let load = |ratio: f32| one(&StatusInput { load: ratio, ..fine() }, TagKind::Load);
        assert_eq!([1.0, 1.01, 1.31, 1.61, 1.99].map(load), [None, l("Burdened", 1), l("Heavy", 2), l("Overloaded", 3), l("Crushed", 4)]);
    }

    #[test]
    fn conditions_have_their_icons() {
        let l = |s: &str, n: u8| Some((s.to_string(), n));
        assert_eq!(one(&StatusInput { endurance: 0.4, ..fine() }, TagKind::Endurance), l("Short of breath", 1));
        assert_eq!(one(&StatusInput { endurance: 0.4, winded: true, ..fine() }, TagKind::Endurance), l("Winded", 3));
        assert_eq!(one(&StatusInput { festering: true, ..fine() }, TagKind::Sick), l("Sick", 2));
        assert_eq!(one(&StatusInput { festering: true, infection: Some(InfectionStage::Feverish), ..fine() }, TagKind::Sick), l("Feverish", 3));
        assert_eq!(one(&StatusInput { infection: Some(InfectionStage::Turning), ..fine() }, TagKind::Sick), l("Turning", 4));
        // A hidden infection shows nothing.
        assert_eq!(one(&StatusInput { infection: Some(InfectionStage::Hidden), ..fine() }, TagKind::Sick), None);
        // Food poisoning has its own icon, beside a festering wound's.
        let both = StatusInput { festering: true, food_poisoned: true, ..fine() };
        assert_eq!((one(&both, TagKind::Sick), one(&both, TagKind::Queasy)), (l("Sick", 2), l("Queasy", 2)));
        let asleep = StatusInput { needs: NeedsView { asleep: Some(SleepSpot::Bed), ..fine().needs }, time_scale: 30.0, ..fine() };
        let t = tags(&asleep).into_iter().find(|t| t.kind == TagKind::Asleep).unwrap();
        assert_eq!((t.label.as_str(), t.tip.as_str()), ("Asleep x30", "Asleep on the bed: resting, healing faster. Move or press B to get up. Time runs 30x as fast."));
        // Agony says what it does to sleep.
        let agony = StatusInput { pain: 0.8, ..fine() };
        assert!(tags(&agony)[0].tip.ends_with("You can't sleep without painkillers."));
        assert!(tags(&StatusInput { painkillers: true, ..agony })[0].tip.contains("let you sleep"));
    }

    #[test]
    fn the_levels_come_from_the_data() {
        let data = parse_status(r#"(icons: [(kind: Thirst, good: Some((below: 0.1, label: "Watered")), levels: [(at: 0.2, label: "Could drink"), (at: 0.5, label: "Gasping", severity: 4, tip: "drink.")])])"#).unwrap();
        let at = |v: f32| status_tags(&data, &NeedsTuning::default(), &StatusInput { needs: NeedsView { thirst: v, hunger: 0.9, ..fine().needs }, ..fine() });
        // Only the icons the file lists, at its levels; a tip left out is just the label.
        assert_eq!(at(0.05), [StatusTag { kind: TagKind::Thirst, label: "Watered".into(), severity: 0, tip: "Watered".into() }]);
        assert_eq!(at(0.15), []);
        assert_eq!(at(0.3)[0].label, "Could drink");
        assert_eq!(at(0.5), [StatusTag { kind: TagKind::Thirst, label: "Gasping".into(), severity: 4, tip: "Gasping: drink.".into() }]);
    }

    #[test]
    fn status_ron_mistakes_are_refused() {
        let bad = |text: &str, what: &str| {
            let e = parse_status(text).expect_err(text);
            assert!(e.contains(what), "{e}");
        };
        bad(r#"(icons: [(kind: Thirst, levels: [])])"#, "0 levels");
        bad(r#"(icons: [(kind: Thirst, levels: [(at: 0.5, label: "A"), (at: 0.5, label: "B")])])"#, "higher than the last");
        bad(r#"(icons: [(kind: Thirst, levels: [(at: 0.5, label: " ")])])"#, "no label");
        bad(r#"(icons: [(kind: Thirst, levels: [(at: 0.5, label: "A", severity: 5)])])"#, "severity 5");
        bad(r#"(icons: [(kind: Thirst, levels: [(at: 0.5, label: "A", severity: 3), (at: 0.6, label: "B", severity: 2)])])"#, "never less");
        bad(r#"(icons: [(kind: Thirst, levels: [(at: 0.5, label: "A")]), (kind: Thirst, levels: [(at: 0.5, label: "A")])])"#, "listed twice");
        bad(r#"(icons: [(kind: Thirst, good: Some((below: 0.6, label: "G")), levels: [(at: 0.5, label: "A")])])"#, "good state");
        bad(r#"(icons: [(kind: Thirst, levels: [(at: 0.5, label: "A", colour: "red")])])"#, "colour");
        bad(r#"(icons: [(kind: Cold, levels: [(at: 0.5, label: "A")])])"#, "Cold");
        bad(r#"(icons: [(kind: Thirst, levels: [(at: 1, label: "A"), (at: 2, label: "B"), (at: 3, label: "C"), (at: 4, label: "D"), (at: 5, label: "E")])])"#, "5 levels");
    }
}
