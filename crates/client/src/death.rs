//! The death screen and run summary (#13).
//!
//! When our player dies the screen goes dark red and a grey-box panel says how (the cause of death
//! in plain words, from the server's run stats, `sim::run`), when (game day and hour), and what this
//! life came to: game time survived and time played, zombies killed, distance walked, things looted,
//! containers searched, wounds taken and HP lost. Everything comes from our run in the latest
//! snapshot (`Snapshot::runs`), so it is the same after a quickload or `--load` (a save carries the
//! stats; one from before #13 shows the cause as unknown).
//!
//! **The headline is a line of our own, a different one each death** (`LINES`, or `RISES` when the
//! infection will raise the body), picked by a hash of the death (its tick and the player), so the
//! same death reads the same after a quickload or in an offscreen picture, and never the line the
//! last death showed.
//!
//! **R starts a new run:** a fresh world with the same seed and map size (and settings) as the one
//! being played, so the same town from the beginning: you wake up in the bedroom at the start hour,
//! with new zombies, untouched houses and empty run stats. The same seed because the client's map
//! copy and meshes are built once for that map; the new world is taken up like a quickload
//! (`saving::WorldSwap`). The world options of the command line that only set up the first world
//! (`--at`, `--fight-demo`, `--siege`) don't apply again. **Q quits** and Esc is the pause menu (autosaving, as
//! closing the window does). `--new-run-after SECS` presses R by itself once we're dead and SECS
//! have passed, for verification.

use bevy::input::InputSystems;
use bevy::prelude::*;
use zombye_net::LocalServer;
use zombye_sim::RunStats;

use crate::Options;
use crate::player::LocalPlayer;
use crate::saving::{Notice, WorldSwap};
use crate::server_link::ServerLink;

use crate::theme::{self, DIM, TEXT, font};

const RED: Color = Color::srgb(0.9, 0.16, 0.1);

/// The rows of the summary, top to bottom.
const ROWS: [&str; 8] = ["Survived (game time)", "Time played", "Zombies killed", "Distance walked", "Things looted", "Containers searched", "Wounds taken", "HP lost"];

/// The headlines: what the screen says in place of a flat "you died". Ours; quiet, a little wry,
/// never gory. Written in sentence case and shown in capitals (the headline face is made for them),
/// so keep each under about 45 characters. Add a line by adding it here.
const LINES: [&str; 28] = [
    "The town keeps you now.",
    "Quiet takes the house again.",
    "The street forgets your footsteps.",
    "No one is left to latch the door.",
    "Morning comes anyway.",
    "The birds will sing tomorrow, without you.",
    "Somewhere a kettle goes cold.",
    "The dead make room for one more.",
    "You were here. The dust will say so.",
    "The last light in town goes out.",
    "So ends a small, stubborn life.",
    "The forest leans in to listen. Nothing.",
    "All your careful steps led here.",
    "The night was patient. It could wait.",
    "A door swings, and nobody shuts it.",
    "You held on. Then you let go.",
    "The silence you kept now keeps you.",
    "Leaves will cover what you were.",
    "One heartbeat fewer in the dark.",
    "The hungry are never lonely for long.",
    "Here the story sets you down.",
    "Nobody will finish the tin you opened.",
    "The clocks run on with no one to mind them.",
    "Your shadow goes on ahead without you.",
    "Rain will wash the porch by spring.",
    "The dark was only ever waiting its turn.",
    "A bed stays made that you will not sleep in.",
    "You ran out of doors to close.",
];

/// The headlines for a death the infection follows up: the body will rise.
const RISES: [&str; 6] = [
    "You lie down. Something else gets up.",
    "Soon you will walk again, and not be you.",
    "The fever keeps what it took.",
    "Your hunger will outlive you.",
    "Sleep, then. The body has other plans.",
    "The town gains one more wanderer.",
];

/// The headline for a death: one of [`RISES`] if the body will rise, else of [`LINES`], by a hash
/// of the death's tick and the player (so the same death always reads the same), moved on by one
/// if that is the line the last death showed.
fn headline(death: Option<&zombye_sim::Death>, player: u32, last: Option<&str>) -> &'static str {
    let pool: &[&'static str] = if death.is_some_and(|d| d.rises) { &RISES } else { &LINES };
    let tick = death.map_or(0, |d| d.tick);
    let k = (zombye_sim::rng::hash01(0x0DEA_D11E, tick, player as u64, 0) * pool.len() as f32) as usize % pool.len();
    if last == Some(pool[k]) { pool[(k + 1) % pool.len()] } else { pool[k] }
}

/// The headline on show, and the death it was picked for (its tick, 0 for one with no record).
#[derive(Default)]
struct Shown {
    death: Option<u64>,
    line: Option<&'static str>,
}

#[derive(Component)]
struct DeathScreen;
#[derive(Component)]
struct HeadlineText;
#[derive(Component)]
struct CauseText;
#[derive(Component)]
struct EpilogueText;
#[derive(Component)]
struct WhenText;
/// The value of row `k` of [`ROWS`].
#[derive(Component)]
struct StatValue(usize);

pub struct DeathPlugin;

impl Plugin for DeathPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_screen)
            // Before Update (as F9 is), so renderers see a new run's edge changes in the same frame.
            .add_systems(PreUpdate, death_keys.after(InputSystems).after(crate::inventory_ui::TextEntry).run_if(crate::live))
            .add_systems(Update, update_screen);
    }
}

fn spawn_screen(mut commands: Commands, link: Option<Res<crate::server_link::ServerLink>>) {
    let rule = theme::spawn_rule;
    // On a multiplayer server R is a new survivor in the same world, which goes on (#21).
    let again = if link.is_some_and(|l| l.net.is_some()) { "a new survivor (the world goes on)" } else { "new run (same town, from the start)" };
    commands
        .spawn((
            DeathScreen,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.12, 0.0, 0.0, 0.5)),
            // Over the HUD and the panels.
            GlobalZIndex(10),
        ))
        .with_children(|screen| {
            screen
                .spawn((
                    Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Stretch, padding: UiRect::axes(px(44), px(30)), row_gap: px(6), min_width: px(560), ..default() },
                    theme::sheet(),
                ))
                .with_children(|c| {
                    c.spawn((
                        HeadlineText,
                        Text::new(""),
                        theme::head(40.0),
                        TextColor(RED),
                        TextLayout::justify(Justify::Center),
                        Node { align_self: AlignSelf::Center, max_width: px(680), ..default() },
                    ));
                    c.spawn((CauseText, Text::new(""), theme::strong(22.0), TextColor(TEXT), Node { align_self: AlignSelf::Center, margin: UiRect::top(px(4)), ..default() }));
                    c.spawn((EpilogueText, Text::new(""), font(15.0), TextColor(DIM), Node { align_self: AlignSelf::Center, ..default() }));
                    c.spawn((WhenText, Text::new(""), font(15.0), TextColor(DIM), Node { align_self: AlignSelf::Center, ..default() }));
                    rule(c);
                    for (k, label) in ROWS.iter().enumerate() {
                        c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(40), ..default() }).with_children(|row| {
                            row.spawn((Text::new(*label), font(18.0), TextColor(DIM)));
                            row.spawn((StatValue(k), Text::new(""), theme::strong(18.0), TextColor(TEXT)));
                        });
                    }
                    rule(c);
                    c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(40), margin: UiRect::top(px(4)), ..default() }).with_children(|row| {
                        theme::keycap(row, "R", again);
                        theme::keycap(row, "Q", "quit");
                        theme::keycap(row, "Esc", "menu");
                    });
                });
        });
}

/// The summary's values for `run`, in [`ROWS`] order.
fn values(run: &RunStats) -> [String; 8] {
    let plural = |n: u32, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    [
        run.survived(),
        run.played(),
        // Other players killed (PvP) are counted apart, and only shown when there are any.
        if run.player_kills == 0 { run.kills.to_string() } else { format!("{} (and {})", run.kills, plural(run.player_kills, "survivor", "survivors")) },
        run.distance(),
        plural(run.items_taken, "thing", "things"),
        run.searched.to_string(),
        run.wounds.to_string(),
        format!("{:.0}", run.hp_lost),
    ]
}

/// "Day 2, 14:05".
fn when(day: u32, hour: f32) -> String {
    let mins = (hour.rem_euclid(24.0) * 60.0) as u32;
    format!("Day {day}, {:02}:{:02}", mins / 60, mins % 60)
}

/// For a life that ended before its first dawn (#19): how far it got.
fn first_night_line(f: &zombye_sim::FirstNight, run: &RunStats) -> Option<String> {
    use zombye_sim::firstnight::{clock, span};
    if f.outcome.is_some() {
        return None;
    }
    let short = match f.phase() {
        zombye_sim::Phase::Night => format!("{} short of the first dawn ({})", span(f.hours_to_dawn()), clock(f.dawn)),
        _ => "Before the first night had even fallen".to_string(),
    };
    Some(format!("{short}. Objectives done: {} of {}.", f.done_count(run), zombye_sim::Objective::ALL.len()))
}

#[allow(clippy::type_complexity)]
fn update_screen(
    me: Res<LocalPlayer>,
    link: Res<ServerLink>,
    join: Res<crate::server_link::JoinInfo>,
    mut screen: Query<&mut Visibility, With<DeathScreen>>,
    mut texts: ParamSet<(
        Query<&mut Text, With<CauseText>>,
        Query<&mut Text, With<EpilogueText>>,
        Query<&mut Text, With<WhenText>>,
        Query<(&mut Text, &StatValue)>,
        Query<&mut Text, With<HeadlineText>>,
    )>,
    mut shown: Local<Shown>,
) {
    let want = if me.dead { Visibility::Inherited } else { Visibility::Hidden };
    if let Ok(mut v) = screen.single_mut()
        && *v != want
    {
        *v = want;
    }
    if !me.dead {
        return;
    }
    // The `Mut` itself, so an unchanged text doesn't count as changed (see `hud::update_vitals`).
    let set = |t: &mut Mut<Text>, s: String| {
        if t.0 != s {
            t.0 = s;
        }
    };
    let run = link.curr.run_of(me.id).cloned().unwrap_or_default();
    let death = run.death;
    // A new death (not the same one seen again after a quickload): a new headline.
    let key = Some(death.map_or(0, |d| d.tick));
    if shown.death != key {
        *shown = Shown { death: key, line: Some(headline(death.as_ref(), me.id, shown.line)) };
    }
    if let Ok(mut t) = texts.p4().single_mut() {
        set(&mut t, shown.line.unwrap_or("").to_uppercase());
    }
    if let Ok(mut t) = texts.p0().single_mut() {
        set(&mut t, death.map_or_else(|| "Cause unknown".to_string(), |d| d.describe_with(&join.weapons)));
    }
    if let Ok(mut t) = texts.p1().single_mut() {
        set(&mut t, death.and_then(|d| d.epilogue()).unwrap_or("").to_string());
    }
    if let Ok(mut t) = texts.p2().single_mut() {
        let night = link.curr.first_night_of(me.id).and_then(|f| first_night_line(f, &run)).map_or_else(String::new, |l| format!("\n{l}"));
        set(&mut t, death.map_or_else(String::new, |d| when(d.day, d.hour)) + &night);
    }
    let values = values(&run);
    for (mut t, k) in texts.p3().iter_mut() {
        set(&mut t, values[k.0].clone());
    }
}

/// Dead: R (or `--new-run-after`) starts a new run, Q quits (Esc is the pause menu, `pause_menu.rs`).
fn death_keys(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    options: Res<Options>,
    mut swap: WorldSwap,
    mut notice: ResMut<Notice>,
    mut exit: MessageWriter<AppExit>,
    mut automated: Local<bool>,
) {
    if !swap.dead() {
        return;
    }
    let now = time.elapsed_secs();
    if keys.just_pressed(KeyCode::KeyQ) {
        println!("Quitting from the death screen");
        exit.write(AppExit::Success);
        return;
    }
    let due = !*automated && options.new_run_after.is_some_and(|t| now >= t);
    *automated |= due;
    if keys.just_pressed(KeyCode::KeyR) || due {
        start_new_run(&mut swap, &mut notice, now);
    }
}

/// A new run: a fresh world like the one being played (same seed, map and settings), taken up
/// like a quickload. From the death screen and the first morning's (`firstnight_ui.rs`).
pub(crate) fn start_new_run(swap: &mut WorldSwap, notice: &mut Notice, now: f32) {
    let Some(config) = swap.link.world().map(|w| w.config.clone()) else {
        // On a multiplayer server (#21) the world goes on: a new survivor in it.
        let me = swap.player();
        swap.link.send(zombye_net::Intent::Respawn { player: me });
        return notice.say(now, "A new survivor".to_string());
    };
    let (world, id) = LocalServer::new_run(&config);
    if !swap.fits(&world) {
        // A map edited by hand (tests, editors) can't be generated again.
        return notice.say(now, "Can't start a new run here: this map wasn't generated from its seed".to_string());
    }
    let seed = world.config.seed();
    swap.take_up(world, Some(id));
    notice.say(now, format!("New run: the same town from the start (seed {seed})"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_values_read_plainly() {
        let run = RunStats { hours: 30.0, secs: 125.0, kills: 4, distance_m: 640.0, items_taken: 1, searched: 2, wounds: 3, hp_lost: 99.6, ..Default::default() };
        assert_eq!(values(&run), ["1 day 6 hours", "2:05", "4", "640 m", "1 thing", "2", "3", "100"].map(String::from));
        assert_eq!(when(2, 14.09), "Day 2, 14:05");
    }

    fn death(tick: u64, rises: bool) -> zombye_sim::Death {
        zombye_sim::Death { cause: zombye_sim::DeathCause::Other, tick, day: 1, hour: 12.0, rises }
    }

    #[test]
    fn every_headline_is_its_own_and_fits() {
        let all: Vec<&str> = LINES.iter().chain(&RISES).copied().collect();
        for (k, line) in all.iter().enumerate() {
            assert!(!line.trim().is_empty() && line.len() <= 45, "{line:?} is empty or too long for the panel");
            assert!(!line.to_lowercase().contains("you died"), "{line:?}");
            assert!(!all[..k].contains(line), "{line:?} is listed twice");
        }
    }

    #[test]
    fn a_death_keeps_its_headline_and_the_next_gets_another() {
        // The same death reads the same every time; with no record there is still a line.
        assert_eq!(headline(Some(&death(812, false)), 1, None), headline(Some(&death(812, false)), 1, None));
        assert!(LINES.contains(&headline(None, 1, None)));
        // A body that will rise gets a line that says so.
        assert!(RISES.contains(&headline(Some(&death(812, true)), 1, None)));
        // Never the last death's line again, even for a death that hashes to it; and over many
        // deaths every line turns up.
        let mut last = None;
        let mut seen = std::collections::BTreeSet::new();
        for tick in 0..2000 {
            let d = death(tick, false);
            let line = headline(Some(&d), 1, last);
            assert_ne!(Some(line), last);
            seen.insert(line);
            last = Some(line);
        }
        assert_eq!(seen.len(), LINES.len());
    }
}
