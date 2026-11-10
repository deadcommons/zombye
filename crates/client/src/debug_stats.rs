//! The debug stats window (Linear DCM-57): the raw numbers behind the status icons, for tuning and
//! bug reports. Off by default; **F4** toggles it, `--debug-stats` starts with it open (for
//! pictures: `--render-to shot.png --hud --debug-stats`).
//!
//! It shows our own survivor only: HP and what is draining it, every body part's wounds, hunger,
//! thirst, tiredness, pain, the infection, food poisoning, endurance, the load, what all that does
//! to movement, every skill's level and XP, the status icons that follow from it, and the last
//! things that hurt us with their source. Temperature isn't simulated yet and says so.
//!
//! Client-side only: it reads what this client already has. In single-player that includes the
//! world in this process, so needs come unrounded and an infection that doesn't show yet is listed
//! (marked as hidden). On a multiplayer server it only opens when the server allows the dev tools
//! (`--allow-dev`), and then shows our private view as the server sent it: never another player's
//! state, and nothing the server keeps from us (a hidden infection stays hidden).

use bevy::prelude::*;
use zombye_sim::combat::CombatEventKind;
use zombye_sim::health::{Dressing, InfectionStage, Part};
use zombye_sim::{Carry, CombatEvent, HealthView, MoveMods, NeedsTuning, NeedsView, SkillView, StatusTag};

use crate::player::LocalPlayer;
use crate::saving::Notice;
use crate::server_link::{CombatEventsThisFrame, JoinInfo, NetInfo, ServerLink};

const TEXT: Color = Color::srgb(0.86, 0.92, 0.86);
const HEAD: Color = Color::srgb(0.35, 0.9, 1.0);
/// How many things that hurt us the window remembers.
const LOG_LEN: usize = 8;
/// Seconds a strike's damage waits for the HP drop it explains, before a drop counts as a drain.
const CREDIT_SECS: f32 = 1.0;

/// Is the window open.
#[derive(Resource, Default)]
pub struct DebugStats {
    pub open: bool,
}

/// Can the window be used here: always in single-player; on a server only with its dev tools on.
pub fn allowed(net: Option<&NetInfo>) -> bool {
    net.is_none_or(|n| n.allow_dev)
}

/// Something that cost us HP.
#[derive(Clone, Debug, PartialEq)]
pub struct Hurt {
    /// The game hour it happened (the latest, for a drain that goes on).
    pub hour: f32,
    pub hp: f32,
    pub source: String,
    /// A slow loss (bleeding, the fever, starving) added up, rather than one blow.
    pub drain: bool,
}

/// The last things that hurt us, newest last.
#[derive(Resource, Default)]
pub struct DamageLog {
    pub entries: Vec<Hurt>,
    last_hp: Option<f32>,
    /// Damage of strikes seen whose HP drop hasn't shown yet, and when the last one was.
    credit: f32,
    credit_at: f32,
}

impl DamageLog {
    fn push(&mut self, hurt: Hurt) {
        self.entries.push(hurt);
        if self.entries.len() > LOG_LEN {
            self.entries.remove(0);
        }
    }

    /// A strike that landed on us.
    pub fn strike(&mut self, now: f32, hour: f32, e: &CombatEvent) {
        let who = if e.by != 0 { format!("survivor #{}", e.by) } else { format!("zombie #{}", e.zombie) };
        let what = match (e.wound, e.part) {
            (Some(w), Some(p)) => format!(": {} on the {}", w.name(), p.name()),
            _ => String::new(),
        };
        self.push(Hurt { hour, hp: e.damage, source: format!("{who}{what}{}", if e.crit { " (crit)" } else { "" }), drain: false });
        (self.credit, self.credit_at) = (self.credit + e.damage, now);
    }

    /// Our HP as it is now: a drop no strike explains is a drain from `source`, added to the last
    /// entry if that is the same drain.
    pub fn hp(&mut self, now: f32, hour: f32, hp: f32, source: &str) {
        if now - self.credit_at > CREDIT_SECS {
            self.credit = 0.0;
        }
        let lost = self.last_hp.map_or(0.0, |last| last - hp);
        self.last_hp = Some(hp);
        if lost <= 0.0 {
            return;
        }
        let struck = lost.min(self.credit);
        self.credit -= struck;
        let drained = lost - struck;
        if drained <= 1e-4 {
            return;
        }
        match self.entries.last_mut() {
            Some(last) if last.drain && last.source == source => {
                last.hp += drained;
                last.hour = hour;
            }
            _ => self.push(Hurt { hour, hp: drained, source: source.to_string(), drain: true }),
        }
    }
}

/// What a slow loss of HP is most likely from, by what our views say is wrong with us.
pub fn drain_source(health: &HealthView, needs: &NeedsView, t: &NeedsTuning) -> &'static str {
    let bleeding = health.wounds.iter().any(|w| w.bleed_per_h > 0.0);
    match health.infection {
        Some(InfectionStage::Turning) => "the infection (turning)",
        Some(InfectionStage::Feverish) => "the infection (fever)",
        _ if bleeding => "bleeding",
        _ if health.losing_per_h > 0.0 => "a festering wound",
        _ if needs.thirst >= t.severe => "thirst",
        _ if needs.hunger >= t.severe => "hunger",
        _ => "unknown",
    }
}

/// What only the world itself knows (single-player): unrounded numbers, and what's kept from views.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Raw {
    pub hunger: f32,
    pub thirst: f32,
    pub tiredness: f32,
    pub sore_h: f32,
    /// The infection, hidden or not, with the game hours left in its stage.
    pub infection: Option<(InfectionStage, f32)>,
}

/// Everything the window shows.
pub struct Stats<'a> {
    pub hp: f32,
    pub max_hp: f32,
    pub endurance: f32,
    pub winded: bool,
    pub health: &'a HealthView,
    pub needs: &'a NeedsView,
    pub needs_tuning: &'a NeedsTuning,
    pub carry: &'a Carry,
    pub max_kg: f32,
    pub mods: MoveMods,
    pub skills: &'a [SkillView],
    pub tags: &'a [StatusTag],
    pub raw: Option<Raw>,
    pub log: &'a [Hurt],
    /// On a multiplayer server: the numbers are the server's view of us.
    pub online: bool,
}

fn clock(hour: f32) -> String {
    let h = hour.rem_euclid(24.0);
    format!("{:02}:{:02}", h.floor() as u32, ((h - h.floor()) * 60.0) as u32)
}

/// The window's two columns of text: health, wounds and damage; then needs, load and skills.
pub fn report(s: &Stats) -> (String, String) {
    let yes = |on: bool| if on { "yes" } else { "no" };
    let h = s.health;
    let mut a = String::new();
    a.push_str(&format!("HEALTH\nHP {:.1} / {:.0} ({:.0}%)\n", s.hp, s.max_hp, s.hp / s.max_hp.max(1.0) * 100.0));
    a.push_str(&format!("losing {:.1} HP per game hour (bleeding, festering)\n", h.losing_per_h));
    a.push_str(&format!("pain {:.2} (wounds {:.2} + sore {:.2})\n", h.pain + s.needs.sore, h.pain, s.needs.sore));
    if let Some(hours) = h.aid.painkillers_h {
        a.push_str(&format!("painkillers working for ~{hours:.0} h{}\n", if h.aid.drowsy { " (drowsy)" } else { "" }));
    }
    a.push_str("\nWOUNDS BY BODY PART\n");
    for part in Part::ALL {
        let wounds: Vec<String> = h
            .wounds
            .iter()
            .filter(|w| w.part == part)
            .map(|w| {
                let mut bits = vec![format!("healed {:.0}%", w.healed * 100.0)];
                if w.bleed_per_h > 0.0 {
                    bits.push(format!("bleeding {:.1}/h", w.bleed_per_h));
                }
                match (w.dressing, w.plaster) {
                    (Dressing::None, _) => {}
                    (Dressing::Clean, false) => bits.push("clean dressing".into()),
                    (Dressing::Dirty, false) => bits.push("dirty dressing".into()),
                    (_, true) => bits.push("plaster".into()),
                }
                if let Some(left) = w.heals_in_h {
                    bits.push(format!("heals in ~{left:.0} h"));
                }
                for (on, what) in [(w.cleaned, "cleaned"), (w.stitched, "stitched"), (w.festering, "FESTERING")] {
                    if on {
                        bits.push(what.into());
                    }
                }
                format!("{} #{} ({})", w.kind.name(), w.id, bits.join(", "))
            })
            .collect();
        a.push_str(&format!("{}: {}\n", part.name(), if wounds.is_empty() { "-".to_string() } else { wounds.join("; ") }));
    }
    a.push_str("\nRECENT DAMAGE\n");
    if s.log.is_empty() {
        a.push_str("none this session\n");
    }
    for e in s.log.iter().rev() {
        a.push_str(&format!("{}  -{:.1} HP  {}\n", clock(e.hour), e.hp, e.source));
    }

    let n = s.needs;
    let t = s.needs_tuning;
    let mut b = String::new();
    b.push_str(&format!("NEEDS (0 fine to 1 worst; effects from {:.2} / {:.2} / {:.2})\n", t.mild, t.strong, t.severe));
    let (hunger, thirst, tiredness) = s.raw.map_or((n.hunger, n.thirst, n.tiredness), |r| (r.hunger, r.thirst, r.tiredness));
    b.push_str(&format!("hunger {hunger:.3}\nthirst {thirst:.3}\ntiredness (fatigue) {tiredness:.3}\n"));
    b.push_str(&format!("asleep: {}\n", n.asleep.map_or("no".to_string(), |spot| format!("on the {}", spot.name()))));
    if let Some(r) = s.raw.filter(|r| r.sore_h > 0.0) {
        b.push_str(&format!("sore from the floor for {:.1} h more\n", r.sore_h));
    }
    b.push_str(&format!("endurance {:.3}, winded: {}\n", s.endurance, yes(s.winded)));
    b.push_str("\nILLNESS\n");
    let stage = |st: InfectionStage| match st {
        InfectionStage::Hidden => "hidden (the player isn't told)",
        InfectionStage::Feverish => "feverish",
        InfectionStage::Turning => "turning",
    };
    b.push_str(&match (s.raw, h.infection) {
        (Some(Raw { infection: Some((st, left)), .. }), _) => format!("infection: {}, {left:.1} h left in this stage\n", stage(st)),
        (Some(_), _) => "infection: none\n".to_string(),
        (None, Some(st)) => format!("infection: {}\n", stage(st)),
        (None, None) => "infection: none showing (the server doesn't say before it shows)\n".to_string(),
    });
    b.push_str(&format!("food poisoning: {}\n", yes(h.food_poisoning)));
    b.push_str(&format!("festering wounds: {}\n", h.wounds.iter().filter(|w| w.festering).count()));
    b.push_str("temperature: not simulated yet\n");
    b.push_str("\nENCUMBRANCE\n");
    let c = s.carry;
    b.push_str(&format!("{:.2} / {:.1} kg ({:.0}% of the limit; nothing more past {:.1} kg)\n", c.weight_kg, c.limit_kg, c.weight_kg / c.limit_kg.max(0.01) * 100.0, s.max_kg));
    b.push_str(&format!("room {:.1} / {:.1} L, burden: {:?}\n", c.used_l, c.room_l, c.burden));
    let m = s.mods;
    b.push_str(&format!("moving: speed x{:.2}, run x{:.2}, run drain x{:.2}, recovery x{:.2}{}{}\n", m.speed, m.run_speed, m.run_drain, m.regen, if m.can_run { "" } else { ", no running" }, if m.can_climb { "" } else { ", no climbing" }));
    // Kept short, so the whole window fits a 1280 x 720 screen without scrolling.
    b.push_str("\nSKILLS (level: XP / next level's)\n");
    let skills: Vec<String> = s.skills.iter().map(|v| format!("{} {}: {:.0} / {}", v.skill.name(), v.level, v.xp, v.next_at.map_or("top".to_string(), |x| format!("{x:.0}")))).collect();
    for pair in skills.chunks(2) {
        b.push_str(&format!("{}\n", pair.join("   ")));
    }
    b.push_str("\nSTATUS ICONS (severity)\n");
    let icons: Vec<String> = s.tags.iter().map(|tag| format!("{} {}", tag.label, tag.severity)).collect();
    b.push_str(&format!("{}\n", if icons.is_empty() { "none".to_string() } else { icons.join(", ") }));
    b.push_str(if s.online { "\nour survivor only, as the server sent it" } else { "\nour survivor only, from the world in this process" });
    (a, b)
}

#[derive(Component)]
struct PanelRoot;
#[derive(Component)]
struct Column(usize);

pub struct DebugStatsPlugin;

impl Plugin for DebugStatsPlugin {
    fn build(&self, app: &mut App) {
        let open = app.world().get_resource::<crate::Options>().is_some_and(|o| o.debug_stats);
        app.insert_resource(DebugStats { open }).init_resource::<DamageLog>().add_systems(Startup, spawn).add_systems(Update, (toggle, track, show).chain());
    }
}

fn font(size: f32) -> TextFont {
    TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }
}

fn spawn(mut commands: Commands) {
    commands
        .spawn((
            PanelRoot,
            crate::hud::TopPanel,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                top: px(70),
                right: px(16),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                padding: UiRect::all(px(10)),
                max_height: Val::Percent(85.0),
                overflow: Overflow::scroll_y(),
                border: UiRect::all(px(1)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.01, 0.03, 0.04, 0.96)),
            BorderColor::all(HEAD.with_alpha(0.5)),
            GlobalZIndex(15),
            Pickable::IGNORE,
        ))
        .with_children(|c| {
            c.spawn((Text::new("DEBUG STATS   F4: close"), font(13.0), TextColor(HEAD), Pickable::IGNORE));
            c.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(24), ..default() }, Pickable::IGNORE)).with_children(|row| {
                row.spawn((Column(0), Text::new(""), font(12.0), TextColor(TEXT), Node { width: px(400), ..default() }, Pickable::IGNORE));
                row.spawn((Column(1), Text::new(""), font(12.0), TextColor(TEXT), Node { width: px(360), ..default() }, Pickable::IGNORE));
            });
        });
}

fn toggle(time: Res<Time>, keys: Res<ButtonInput<KeyCode>>, link: Res<ServerLink>, mut panel: ResMut<DebugStats>, notice: Option<ResMut<Notice>>) {
    let ok = allowed(link.net.as_ref());
    if panel.open && !ok {
        // `--debug-stats` on a server without the dev tools.
        panel.open = false;
        println!("the debug stats window needs a server started with --allow-dev");
    }
    if keys.just_pressed(KeyCode::F4) {
        if ok {
            panel.open = !panel.open;
        } else if let Some(mut notice) = notice {
            notice.say(time.elapsed_secs(), "The debug stats window needs a server started with --allow-dev".to_string());
        }
    }
}

/// Keeps the damage log, open or not, so opening the window shows what just happened.
fn track(time: Res<Time>, link: Res<ServerLink>, join: Res<JoinInfo>, me: Res<LocalPlayer>, events: Res<CombatEventsThisFrame>, mut log: ResMut<DamageLog>) {
    let (now, hour) = (time.elapsed_secs(), link.curr.time_of_day);
    for e in events.0.iter().filter(|e| e.kind == CombatEventKind::PlayerHurt && e.player == me.id) {
        log.strike(now, hour, e);
    }
    let source = link.own(me.id).map_or("unknown", |v| drain_source(&v.health, &v.needs, &join.needs));
    log.hp(now, hour, me.hp.max(0.0), source);
}

#[allow(clippy::too_many_arguments)]
fn show(
    panel: Res<DebugStats>,
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    icons: Res<crate::needs_ui::StatusIcons>,
    me: Res<LocalPlayer>,
    log: Res<DamageLog>,
    mut root: Query<&mut Visibility, With<PanelRoot>>,
    mut columns: Query<(&Column, &mut Text)>,
) {
    let Ok(mut vis) = root.single_mut() else { return };
    let want = if panel.open { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != want {
        *vis = want;
    }
    if !panel.open {
        return;
    }
    let Some(own) = link.own(me.id) else { return };
    // Single-player: our own survivor in the world this process runs. Never anyone else's.
    let raw = link.world().and_then(|w| w.player(me.id)).map(|p| Raw {
        hunger: p.needs.hunger,
        thirst: p.needs.thirst,
        tiredness: p.needs.tiredness,
        sore_h: p.needs.sore_h,
        infection: p.health.infection.map(|i| (i.stage, i.left_h)),
    });
    let tags = icons.tags(&link, &join, &me);
    let stats = Stats {
        hp: me.hp.max(0.0),
        max_hp: join.combat.player_hp,
        endurance: me.curr.endurance,
        winded: me.curr.winded,
        health: &own.health,
        needs: &own.needs,
        needs_tuning: &join.needs,
        carry: &own.carry,
        max_kg: own.max_kg,
        mods: link.curr.players.iter().find(|p| p.id == me.id).map_or(MoveMods::default(), |p| p.mods),
        skills: &own.skills,
        tags: &tags,
        raw,
        log: &log.entries,
        online: link.net.is_some(),
    };
    let (a, b) = report(&stats);
    for (column, mut text) in &mut columns {
        let want = if column.0 == 0 { &a } else { &b };
        if text.0 != *want {
            text.0.clone_from(want);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zombye_sim::health::{WoundKind, WoundView};
    use zombye_sim::{Burden, Skill};

    fn wound(id: u32, part: Part, kind: WoundKind, bleed: f32) -> WoundView {
        WoundView { id, part, kind, bleed_per_h: bleed, healed: 0.15, heals_in_h: Some(30.0), dressing: Dressing::None, plaster: false, dirty_in_h: None, cleaned: false, stitched: false, festering: false }
    }

    fn strike(zombie: u32, by: u32, damage: f32) -> CombatEvent {
        CombatEvent { kind: CombatEventKind::PlayerHurt, pos: Vec2::ZERO, dir: Vec2::X, zombie, player: 1, damage, crit: false, part: Some(Part::LeftArm), wound: Some(WoundKind::Scratch), by }
    }

    #[test]
    fn the_window_shows_every_stat() {
        let health = HealthView {
            wounds: vec![wound(3, Part::Torso, WoundKind::Gash, 10.0), WoundView { festering: true, dressing: Dressing::Dirty, ..wound(4, Part::LeftLeg, WoundKind::Bite, 0.0) }],
            losing_per_h: 11.0,
            pain: 0.31,
            infection: None,
            food_poisoning: true,
            ..Default::default()
        };
        let needs = NeedsView { hunger: 0.41, thirst: 0.63, tiredness: 0.22, asleep: None, sore: 0.1 };
        let carry = Carry { weight_kg: 12.25, limit_kg: 15.5, room_l: 23.0, used_l: 4.0, burden: Burden::None };
        let skills = [SkillView { skill: Skill::Fitness, level: 1, xp: 130.0, next_at: Some(250.0), progress: 0.2 }];
        let tags = [StatusTag { kind: zombye_sim::TagKind::Thirst, label: "Thirsty".into(), severity: 2, tip: String::new() }];
        let log = [Hurt { hour: 14.5, hp: 8.2, source: "zombie #412: scratch on the left arm".into(), drain: false }, Hurt { hour: 15.0, hp: 2.0, source: "bleeding".into(), drain: true }];
        let raw = Raw { hunger: 0.4123, thirst: 0.6301, tiredness: 0.2199, sore_h: 1.5, infection: Some((InfectionStage::Hidden, 7.25)) };
        let mut stats = Stats {
            hp: 71.4,
            max_hp: 100.0,
            endurance: 0.84,
            winded: false,
            health: &health,
            needs: &needs,
            needs_tuning: &NeedsTuning::default(),
            carry: &carry,
            max_kg: 31.0,
            mods: MoveMods::default(),
            skills: &skills,
            tags: &tags,
            raw: Some(raw),
            log: &log,
            online: false,
        };
        let (a, b) = report(&stats);
        for want in [
            "HP 71.4 / 100 (71%)",
            "losing 11.0 HP per game hour",
            "pain 0.41 (wounds 0.31 + sore 0.10)",
            "head: -",
            "torso: gash #3 (healed 15%, bleeding 10.0/h, heals in ~30 h)",
            "left leg: bite #4 (healed 15%, dirty dressing, heals in ~30 h, FESTERING)",
            "right foot: -",
            // Newest first, with the source.
            "15:00  -2.0 HP  bleeding\n14:30  -8.2 HP  zombie #412: scratch on the left arm",
        ] {
            assert!(a.contains(want), "{want:?} not in:\n{a}");
        }
        // Every body part has its line.
        assert_eq!(Part::ALL.iter().filter(|p| a.contains(&format!("\n{}: ", p.name()))).count(), 10);
        for want in [
            "hunger 0.412\nthirst 0.630\ntiredness (fatigue) 0.220",
            "endurance 0.840, winded: no",
            "infection: hidden (the player isn't told), 7.2 h left in this stage",
            "food poisoning: yes",
            "festering wounds: 1",
            "temperature: not simulated yet",
            "12.25 / 15.5 kg (79% of the limit; nothing more past 31.0 kg)",
            "room 4.0 / 23.0 L, burden: None",
            "moving: speed x1.00",
            "Fitness 1: 130 / 250",
            "STATUS ICONS (severity)\nThirsty 2\n",
            "from the world in this process",
        ] {
            assert!(b.contains(want), "{want:?} not in:\n{b}");
        }
        // On a server: the view's rounded needs, and no word of an infection that doesn't show.
        stats.raw = None;
        stats.online = true;
        let (_, b) = report(&stats);
        assert!(b.contains("hunger 0.410") && b.contains("infection: none showing") && !b.contains("hidden (") && b.contains("as the server sent it"), "{b}");
    }

    #[test]
    fn only_with_the_dev_tools_on_a_server() {
        assert!(allowed(None), "single-player");
        assert!(!allowed(Some(&NetInfo::default())), "a server without --allow-dev");
        assert!(allowed(Some(&NetInfo { allow_dev: true, ..Default::default() })));
    }

    #[test]
    fn the_log_names_strikes_and_adds_up_drains() {
        let mut log = DamageLog::default();
        log.hp(0.0, 14.0, 100.0, "unknown");
        // A zombie's strike, then the HP drop it explains: one entry.
        log.strike(1.0, 14.0, &strike(412, 0, 8.0));
        log.hp(1.02, 14.0, 92.0, "bleeding");
        assert_eq!(log.entries, [Hurt { hour: 14.0, hp: 8.0, source: "zombie #412: scratch on the left arm".into(), drain: false }]);
        // Bleeding after it: one entry that grows, with the hour it last took something.
        for i in 1..=10 {
            log.hp(3.0 + i as f32, 14.0 + i as f32 * 0.1, 92.0 - i as f32 * 0.5, "bleeding");
        }
        assert_eq!(log.entries.len(), 2);
        assert!(log.entries[1].drain && (log.entries[1].hp - 5.0).abs() < 1e-3 && log.entries[1].hour == 15.0, "{:?}", log.entries[1]);
        // Another player's blow (PvP) says who; HP coming back is nothing.
        log.strike(20.0, 15.5, &strike(0, 2, 12.0));
        log.hp(20.0, 15.5, 75.0, "bleeding");
        log.hp(21.0, 15.5, 80.0, "bleeding");
        assert_eq!(log.entries.len(), 3);
        assert!(log.entries[2].source.starts_with("survivor #2"));
        // A strike that cost nothing (god mode) doesn't swallow a later drain.
        log.strike(30.0, 16.0, &strike(7, 0, 9.0));
        log.hp(32.0, 16.0, 78.0, "hunger");
        assert_eq!(log.entries.last().unwrap(), &Hurt { hour: 16.0, hp: 2.0, source: "hunger".into(), drain: true });
        // Only the last few are kept.
        for i in 0..20 {
            log.strike(40.0 + i as f32, 17.0, &strike(i, 0, 1.0));
        }
        assert_eq!(log.entries.len(), LOG_LEN);
    }

    #[test]
    fn a_drain_is_put_down_to_what_is_wrong() {
        let t = NeedsTuning::default();
        let fine = NeedsView::default();
        let bleeding = HealthView { wounds: vec![wound(1, Part::Head, WoundKind::Gash, 10.0)], losing_per_h: 10.0, ..Default::default() };
        assert_eq!(drain_source(&bleeding, &fine, &t), "bleeding");
        assert_eq!(drain_source(&HealthView { infection: Some(InfectionStage::Feverish), ..bleeding.clone() }, &fine, &t), "the infection (fever)");
        assert_eq!(drain_source(&HealthView { losing_per_h: 1.0, ..Default::default() }, &fine, &t), "a festering wound");
        assert_eq!(drain_source(&HealthView::default(), &NeedsView { thirst: 0.9, hunger: 0.9, ..fine }, &t), "thirst");
        assert_eq!(drain_source(&HealthView::default(), &NeedsView { hunger: 0.9, ..fine }, &t), "hunger");
        assert_eq!(drain_source(&HealthView::default(), &fine, &t), "unknown");
    }
}
