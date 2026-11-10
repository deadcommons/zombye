//! The first night on screen (#19): what to do next, and how the night went.
//!
//! Three pieces, all read from our first-night record in the latest snapshot (`sim::firstnight`,
//! `Snapshot::first_night_of`), so they are right after a quickload or `--load` too:
//!
//! - **The objectives panel** (bottom right): the hour, how long until dark (or until dawn), and
//!   the five objectives with the done ones ticked.
//! - **The hint line** (bottom centre, over the current action): one sentence on what to do now and
//!   which key does it, chosen by [`hint`] from where the player is in the loop: waking up,
//!   searching, taking things, arming, packing food and drink, the evening's stay-or-go, the night,
//!   and before any of that whatever is urgent (asleep, bleeding, a zombie in reach, thirst).
//! - **The morning screen**: at the first dawn, where the night was spent and what it cost, with
//!   Enter to carry on and R for a new run. The world doesn't stop behind it.
//!
//! **O** hides or shows the panel and the hints (for this session). They aren't shown in the demo
//! setups (`--fight-demo`, `--siege`), for a life with no record (a save from before #19), once
//! dead (the death screen says how far it got), or after the morning screen: the loop is done.
//! `--morning` starts with the morning screen up when the world has an outcome (for pictures, with
//! `--time 21 --skip-hours 9.5` or a save made after dawn).

use bevy::input::InputSystems;
use bevy::prelude::*;
use zombye_sim::firstnight::{clock, span};
use zombye_sim::{FirstNight, Objective, Phase, RunStats, TagKind, ZombieAnim};

use crate::Options;
use crate::player::LocalPlayer;
use crate::saving::{Notice, WorldSwap};
use crate::server_link::{ClientMap, JoinInfo, ServerLink};

const TEXT: Color = Color::srgb(0.93, 0.9, 0.82);
const DIM: Color = Color::srgb(0.62, 0.6, 0.55);
const GOLD: Color = Color::srgb(0.95, 0.8, 0.4);
const DONE: Color = Color::srgb(0.45, 0.75, 0.4);
const TODO: Color = Color::srgba(1.0, 1.0, 1.0, 0.16);
const BG: Color = Color::srgba(0.02, 0.02, 0.03, 0.6);
const RULE: Color = Color::srgba(0.93, 0.9, 0.82, 0.18);
/// Narrow enough to clear the vitals and the objectives panel on a 1280 px screen.
const HINT_WIDTH: f32 = 540.0;
/// A zombie this close (metres) is the hint's business.
const THREAT_M: f32 = 5.0;
/// An outcome this fresh (ticks) is this session's dawn, not a quickload's.
const FRESH_TICKS: u64 = 100;

#[derive(Component)]
struct ObjectivesPanel;
#[derive(Component)]
struct ObjectivesBody;
#[derive(Component)]
struct HintBox;
#[derive(Component)]
struct HintText;
#[derive(Component)]
struct MorningScreen;
#[derive(Component)]
struct MorningBody;

/// What the panel shows, to rebuild it only when it changes.
#[derive(Resource, Default, PartialEq)]
struct Shown {
    title: String,
    when: String,
    rows: Vec<(bool, String)>,
}

/// The morning screen and the O toggle.
#[derive(Resource, Default)]
pub struct FirstNightUi {
    /// The panel and hints are hidden (O).
    pub hidden: bool,
    /// The morning screen is up.
    pub morning: bool,
    /// The morning screen was shut (or the outcome is an old one): the loop is done.
    over: bool,
    /// Whether the last frame's record had an outcome (`None` before the first frame).
    had_outcome: Option<bool>,
}

pub struct FirstNightUiPlugin;

impl Plugin for FirstNightUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Shown>()
            .init_resource::<FirstNightUi>()
            .add_systems(Startup, spawn)
            .add_systems(PreUpdate, keys.after(InputSystems).after(crate::inventory_ui::TextEntry).run_if(crate::live))
            .add_systems(Update, (follow, update_panel, update_hint, update_morning).chain());
    }
}

fn font(size: f32) -> TextFont {
    TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }
}

fn spawn(mut commands: Commands) {
    commands
        .spawn((
            ObjectivesPanel,
            Node { position_type: PositionType::Absolute, bottom: px(16), right: px(16), width: px(330), flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect::all(px(10)), ..default() },
            BackgroundColor(BG),
            Visibility::Hidden,
            Pickable::IGNORE,
        ))
        .with_children(|c| {
            c.spawn((ObjectivesBody, Node { flex_direction: FlexDirection::Column, row_gap: px(4), ..default() }, Pickable::IGNORE));
        });
    commands
        .spawn((
            HintBox,
            Node { position_type: PositionType::Absolute, bottom: px(150), left: percent(50), margin: UiRect::left(px(-HINT_WIDTH / 2.0)), width: px(HINT_WIDTH), justify_content: JustifyContent::Center, padding: UiRect::axes(px(12), px(7)), ..default() },
            BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.72)),
            Visibility::Hidden,
            Pickable::IGNORE,
        ))
        .with_children(|c| {
            c.spawn((HintText, Text::new(""), font(16.0), TextColor(GOLD), TextLayout::justify(Justify::Center), Pickable::IGNORE));
        });
    commands
        .spawn((
            MorningScreen,
            Visibility::Hidden,
            Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
            BackgroundColor(Color::srgba(0.1, 0.07, 0.02, 0.4)),
            GlobalZIndex(9),
        ))
        .with_children(|screen| {
            screen.spawn((
                MorningBody,
                Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Stretch, padding: UiRect::axes(px(36), px(26)), row_gap: px(4), min_width: px(540), ..default() },
                BackgroundColor(Color::srgba(0.03, 0.03, 0.035, 0.9)),
            ));
        });
}

/// What [`hint`] goes by.
#[derive(Clone, Debug, PartialEq)]
pub struct HintInput {
    pub phase: Phase,
    /// Game hours until dark and until dawn.
    pub to_dusk: f64,
    pub to_dawn: f64,
    pub dusk: String,
    pub dawn: String,
    pub searched: u32,
    pub taken: u32,
    pub kills: u32,
    pub moved_m: f64,
    pub armed: bool,
    pub food: bool,
    pub drink: bool,
    pub indoors: bool,
    pub asleep: bool,
    pub bleeding: bool,
    /// Thirsty or parched; hungry or starving.
    pub thirsty: bool,
    pub hungry: bool,
    /// A standing zombie within [`THREAT_M`].
    pub threat: bool,
    /// A container is open (searched and still in reach), by name.
    pub open: Option<String>,
}

/// The one thing to say now: what's urgent first, then the next step of the loop.
pub fn hint(c: &HintInput) -> Option<String> {
    if c.phase == Phase::Morning {
        return None;
    }
    if c.asleep {
        return Some(format!("Asleep: the hours fly by. A noise or a blow wakes you; B or WASD gets you up. Dawn at {}.", c.dawn));
    }
    if c.threat {
        return Some(if c.armed {
            "Zombie! Hold the right mouse button to aim, left click to swing, Space to shove it back.".to_string()
        } else {
            "Zombie, and nothing in your hands! Hold right mouse and press Space to shove it back, or get away: they are slow.".to_string()
        });
    }
    if c.bleeding {
        return Some("You're bleeding. Press H: click the wound, then a bandage or rag to dress it.".to_string());
    }
    if c.thirsty && c.drink {
        return Some("You're thirsty. Press Tab, right-click a drink and drink it.".to_string());
    }
    if c.hungry && c.food {
        return Some("You're hungry. Press Tab, right-click some food and eat it.".to_string());
    }
    let night = match c.phase {
        Phase::Night if !c.indoors => Some(format!("It's night: you can barely see, and they hear every step. Get indoors and shut the door behind you (F). Dawn at {}.", c.dawn)),
        Phase::Night => Some(format!("Night. Keep the doors shut (F) and stay quiet (C sneaks), or lie down by a bed and sleep it away (B). Dawn at {}, in {}.", c.dawn, span(c.to_dawn))),
        Phase::Evening if !c.indoors => Some(format!("Dark at {}, in {}. Be indoors by then: head home, or pick a house and shut its doors (F).", c.dusk, span(c.to_dusk))),
        Phase::Evening => Some(format!("Dark at {}, in {}. Stay or go? Stay here and shut the doors (F), or find somewhere better while you can still see.", c.dusk, span(c.to_dusk))),
        _ => None,
    };
    if night.is_some() {
        return night;
    }
    Some(if c.moved_m < 1.5 && c.searched == 0 {
        "You wake up at home. It's too quiet. WASD to walk; the mouse wheel zooms.".to_string()
    } else if let (Some(name), 0) = (&c.open, c.taken) {
        format!("The {name} is open. Press Tab and click a thing to take it; right-click for what else you can do with it.")
    } else if c.searched == 0 {
        "Search the house: stand facing a cupboard, wardrobe, fridge or shelf and press E.".to_string()
    } else if !c.armed {
        "Find a weapon: knives in kitchen drawers, tools on shelves, a bat in a wardrobe. Press Tab, right-click it and take it in hand.".to_string()
    } else if !c.food || !c.drink {
        let what = match (c.food, c.drink) {
            (true, false) => "something to drink",
            (false, true) => "something to eat",
            _ => "food and drink",
        };
        format!("Pack {what}: kitchens and fridges first. The house next door has more (F opens doors).")
    } else if c.searched < zombye_sim::firstnight::SEARCH_GOAL {
        "Keep searching: every room holds something. Bandages and a bag are worth finding.".to_string()
    } else if c.kills == 0 {
        format!("You're set for now. Dark at {}: stay here, or look further afield while it's light. Outside, sneak (C) and keep away from groups.", c.dusk)
    } else {
        format!("Dark at {}, in {}. Use the daylight; be indoors when it goes.", c.dusk, span(c.to_dusk))
    })
}

/// Is the first-night UI for this session at all, and is there a record to show?
fn record<'a>(options: &Options, link: &'a ServerLink, me: &LocalPlayer) -> Option<(&'a FirstNight, &'a RunStats)> {
    if options.fight_demo || options.siege {
        return None;
    }
    Some((link.curr.first_night_of(me.id)?, link.curr.run_of(me.id)?))
}

/// The morning screen comes up at this session's first dawn (or with `--morning`), and goes when
/// the world is swapped for one with no outcome or we die.
fn follow(options: Res<Options>, link: Res<ServerLink>, me: Res<LocalPlayer>, mut ui: ResMut<FirstNightUi>) {
    let outcome = record(&options, &link, &me).and_then(|(f, _)| f.outcome);
    let has = outcome.is_some();
    let fresh = outcome.is_some_and(|o| link.curr.tick.saturating_sub(o.tick) <= FRESH_TICKS);
    match ui.had_outcome {
        None if has => (ui.morning, ui.over) = (options.morning, !options.morning),
        Some(false) if has => (ui.morning, ui.over) = (fresh, !fresh),
        _ => {}
    }
    if !has {
        (ui.morning, ui.over) = (false, false);
    }
    if me.dead {
        ui.morning = false;
    }
    // The first frames have no snapshot of ours yet: nothing known.
    if link.curr.run_of(me.id).is_some() {
        ui.had_outcome = Some(has);
    }
}

/// Enter shuts the morning screen, R there starts a new run; O hides the panel and hints.
fn keys(keys: Res<ButtonInput<KeyCode>>, time: Res<Time>, mut ui: ResMut<FirstNightUi>, mut swap: WorldSwap, mut notice: ResMut<Notice>) {
    if keys.just_pressed(KeyCode::KeyO) {
        ui.hidden = !ui.hidden;
        notice.say(time.elapsed_secs(), if ui.hidden { "Objectives and hints hidden (O shows them)".to_string() } else { "Objectives and hints shown".to_string() });
    }
    if !ui.morning {
        return;
    }
    if keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]) {
        (ui.morning, ui.over) = (false, true);
    } else if keys.just_pressed(KeyCode::KeyR) && swap.link.world().is_some() {
        // (Single-player only: on a server the world goes on, and the living don't respawn.)
        crate::death::start_new_run(&mut swap, &mut notice, time.elapsed_secs());
    }
}

fn update_panel(
    mut commands: Commands,
    options: Res<Options>,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    ui: Res<FirstNightUi>,
    mut shown: ResMut<Shown>,
    mut panel: Query<&mut Visibility, With<ObjectivesPanel>>,
    body: Query<Entity, With<ObjectivesBody>>,
) {
    let (Ok(mut vis), Ok(body)) = (panel.single_mut(), body.single()) else { return };
    let rec = record(&options, &link, &me).filter(|_| !ui.hidden && !ui.over && !ui.morning && !me.dead);
    let want = if rec.is_some() { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != want {
        *vis = want;
    }
    let Some((f, run)) = rec else { return };
    let when = match f.phase() {
        Phase::Day | Phase::Evening => format!("{}  -  dark in {}", clock(f.now), span(f.hours_to_dusk())),
        Phase::Night => format!("{}  -  night, dawn in {}", clock(f.now), span(f.hours_to_dawn())),
        Phase::Morning => format!("{}  -  morning", clock(f.now)),
    };
    let now = Shown { title: "THE FIRST NIGHT".to_string(), when, rows: Objective::ALL.iter().map(|&o| (f.done(o, run), f.label(o, run))).collect() };
    if *shown == now {
        return;
    }
    commands.entity(body).despawn_children();
    commands.entity(body).with_children(|c| {
        c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, ..default() }).with_children(|row| {
            row.spawn((Text::new(now.title.clone()), font(14.0), TextColor(GOLD)));
            row.spawn((Text::new("O hides"), font(11.0), TextColor(DIM)));
        });
        c.spawn((Text::new(now.when.clone()), font(13.0), TextColor(if f.phase() == Phase::Day { DIM } else { GOLD })));
        for (done, label) in &now.rows {
            c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), ..default() }).with_children(|row| {
                row.spawn((Node { width: px(10), height: px(10), flex_shrink: 0.0, ..default() }, BackgroundColor(if *done { DONE } else { TODO })));
                row.spawn((Text::new(label.clone()), font(13.0), TextColor(if *done { DIM } else { TEXT })));
            });
        }
    });
    *shown = now;
}

#[allow(clippy::too_many_arguments)]
fn update_hint(
    options: Res<Options>,
    link: Res<ServerLink>,
    map: Res<ClientMap>,
    join: Res<JoinInfo>,
    icons: Res<crate::needs_ui::StatusIcons>,
    me: Res<LocalPlayer>,
    ui: Res<FirstNightUi>,
    mut hint_box: Query<&mut Visibility, With<HintBox>>,
    mut text: Query<&mut Text, With<HintText>>,
) {
    let (Ok(mut vis), Ok(mut text)) = (hint_box.single_mut(), text.single_mut()) else { return };
    let line = record(&options, &link, &me).filter(|_| !ui.hidden && !ui.over && !ui.morning && !me.dead).and_then(|(f, run)| {
        let own = link.own(me.id);
        let tags = icons.tags(&link, &join, &me);
        let tag = |kind: TagKind, from: u8| tags.iter().any(|t| t.kind == kind && t.severity >= from);
        let threat = link.curr.zombies.iter().any(|z| !matches!(z.anim, ZombieAnim::Dead | ZombieAnim::Down) && Vec2::from(z.pos).distance(me.curr.pos) <= THREAT_M);
        hint(&HintInput {
            phase: f.phase(),
            to_dusk: f.hours_to_dusk(),
            to_dawn: f.hours_to_dawn(),
            dusk: clock(f.dusk),
            dawn: clock(f.dawn),
            searched: run.searched,
            taken: run.items_taken,
            kills: run.kills,
            moved_m: run.distance_m,
            armed: me.weapon.is_some(),
            food: f.food,
            drink: f.drink,
            indoors: map.0.building_at(me.curr.pos).is_some(),
            asleep: tag(TagKind::Asleep, 0),
            bleeding: tag(TagKind::Bleeding, 0),
            thirsty: tag(TagKind::Thirst, 2),
            hungry: tag(TagKind::Hunger, 2),
            threat,
            open: own.and_then(|v| v.open.as_ref()).map(|c| c.name.to_lowercase()),
        })
    });
    let want = if line.is_some() { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != want {
        *vis = want;
    }
    let line = line.unwrap_or_default();
    if text.0 != line {
        text.0 = line;
    }
}

/// The morning screen's rows for an outcome: label and value.
fn morning_rows(f: &FirstNight, run: &RunStats) -> Vec<(&'static str, String)> {
    let Some(o) = f.outcome else { return Vec::new() };
    let plural = |n: u32, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    vec![
        ("Slept", if o.slept_h < 0.05 { "not at all".to_string() } else { span(o.slept_h as f64) }),
        ("Zombies killed in the night", o.kills.to_string()),
        ("Wounds taken in the night", plural(o.wounds, "wound", "wounds")),
        ("Health at dawn", format!("{:.0} HP ({:.0} lost in the night)", o.hp, o.hp_lost)),
        ("Objectives done", format!("{} of {}", o.objectives, Objective::ALL.len())),
        ("Since you woke", format!("{}, {} looted, {} walked", run.survived(), plural(run.items_taken, "thing", "things"), run.distance())),
    ]
}

fn update_morning(
    mut commands: Commands,
    options: Res<Options>,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    ui: Res<FirstNightUi>,
    mut screen: Query<&mut Visibility, With<MorningScreen>>,
    body: Query<Entity, With<MorningBody>>,
    mut built: Local<Option<u64>>,
) {
    let (Ok(mut vis), Ok(body)) = (screen.single_mut(), body.single()) else { return };
    let rec = record(&options, &link, &me).filter(|(f, _)| ui.morning && f.outcome.is_some());
    let want = if rec.is_some() { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != want {
        *vis = want;
    }
    let Some((f, run)) = rec else {
        *built = None;
        return;
    };
    let Some(o) = f.outcome else { return };
    if *built == Some(o.tick) {
        return;
    }
    *built = Some(o.tick);
    let rule = |c: &mut ChildSpawnerCommands| {
        c.spawn((Node { width: Val::Percent(100.0), height: px(1), margin: UiRect::vertical(px(6)), ..default() }, BackgroundColor(RULE)));
    };
    let centred = || Node { align_self: AlignSelf::Center, ..default() };
    commands.entity(body).despawn_children();
    commands.entity(body).with_children(|c| {
        c.spawn((Text::new("DAWN"), font(56.0), TextColor(GOLD), centred()));
        c.spawn((Text::new("You lived through the first night."), font(24.0), TextColor(TEXT), centred()));
        c.spawn((Text::new(o.headline()), font(16.0), TextColor(DIM), centred()));
        c.spawn((Text::new(o.verdict()), font(16.0), TextColor(DIM), centred()));
        rule(c);
        for (label, value) in morning_rows(f, run) {
            c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(40), ..default() }).with_children(|row| {
                row.spawn((Text::new(label), font(18.0), TextColor(DIM)));
                row.spawn((Text::new(value), font(18.0), TextColor(TEXT)));
            });
        }
        rule(c);
        c.spawn((Text::new("The town is still out there, and so are they. How long can you last?"), font(15.0), TextColor(DIM), centred()));
        c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(40), margin: UiRect::top(px(6)), ..default() }).with_children(|row| {
            row.spawn((Text::new("Enter  carry on"), font(16.0), TextColor(TEXT)));
            row.spawn((Text::new("R  new run (same town, from the start)"), font(16.0), TextColor(TEXT)));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use zombye_sim::firstnight::{Outcome, Shelter};

    fn waking() -> HintInput {
        HintInput {
            phase: Phase::Day,
            to_dusk: 6.0,
            to_dawn: 16.0,
            dusk: "20:00".into(),
            dawn: "06:00".into(),
            searched: 0,
            taken: 0,
            kills: 0,
            moved_m: 0.0,
            armed: false,
            food: false,
            drink: false,
            indoors: true,
            asleep: false,
            bleeding: false,
            thirsty: false,
            hungry: false,
            threat: false,
            open: None,
        }
    }

    fn says(c: &HintInput, what: &str) {
        let h = hint(c).unwrap_or_default();
        assert!(h.contains(what), "expected {what:?} in {h:?}");
    }

    #[test]
    fn hints_walk_a_new_player_through_the_day() {
        let mut c = waking();
        says(&c, "WASD");
        c.moved_m = 4.0;
        says(&c, "press E");
        c.searched = 1;
        c.open = Some("wardrobe".into());
        says(&c, "The wardrobe is open. Press Tab");
        c.taken = 2;
        says(&c, "Find a weapon");
        c.armed = true;
        says(&c, "Pack food and drink");
        c.food = true;
        says(&c, "Pack something to drink");
        c.drink = true;
        says(&c, "Keep searching");
        c.searched = 3;
        says(&c, "You're set for now. Dark at 20:00");
        c.kills = 1;
        says(&c, "Use the daylight");
    }

    #[test]
    fn evening_and_night_hints_put_stay_or_go() {
        let mut c = HintInput { phase: Phase::Evening, to_dusk: 1.5, searched: 3, armed: true, food: true, drink: true, moved_m: 80.0, ..waking() };
        says(&c, "Stay or go?");
        says(&c, "in 1 h 30 min");
        c.indoors = false;
        says(&c, "Be indoors by then");
        c.phase = Phase::Night;
        says(&c, "Get indoors");
        c.indoors = true;
        c.to_dawn = 3.25;
        says(&c, "sleep it away (B). Dawn at 06:00, in 3 h 15 min");
        c.phase = Phase::Morning;
        assert_eq!(hint(&c), None);
    }

    #[test]
    fn what_is_urgent_comes_first() {
        let mut c = HintInput { phase: Phase::Night, armed: true, food: true, drink: true, thirsty: true, hungry: true, bleeding: true, threat: true, asleep: true, ..waking() };
        says(&c, "Asleep");
        c.asleep = false;
        says(&c, "left click to swing");
        c.armed = false;
        says(&c, "nothing in your hands");
        c.threat = false;
        says(&c, "bleeding");
        c.bleeding = false;
        says(&c, "thirsty");
        c.drink = false;
        says(&c, "hungry");
        // Hungry with nothing to eat: no use saying so; the night's hint stands.
        c.food = false;
        says(&c, "Night.");
    }

    #[test]
    fn the_morning_rows_read_plainly() {
        let mut f = FirstNight::begin(14.0);
        assert!(morning_rows(&f, &RunStats::default()).is_empty());
        f.outcome = Some(Outcome { tick: 9, shelter: Shelter::Home, slept_h: 7.5, kills: 2, wounds: 1, hp_lost: 12.4, hp: 87.6, objectives: 4 });
        let run = RunStats { hours: 16.0, items_taken: 9, distance_m: 310.0, ..Default::default() };
        let rows: Vec<String> = morning_rows(&f, &run).into_iter().map(|(_, v)| v).collect();
        assert_eq!(rows, ["7 h 30 min", "2", "1 wound", "88 HP (12 lost in the night)", "4 of 5", "16 hours, 9 things looted, 310 m walked"]);
    }
}
