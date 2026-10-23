//! The death screen and run summary (#13).
//!
//! When our player dies the screen goes dark red and a grey-box panel says how (the cause of death
//! in plain words, from the server's run stats, `sim::run`), when (game day and hour), and what this
//! life came to: game time survived and time played, zombies killed, distance walked, things looted,
//! containers searched, wounds taken and HP lost. Everything comes from our run in the latest
//! snapshot (`Snapshot::runs`), so it is the same after a quickload or `--load` (a save carries the
//! stats; one from before #13 shows the cause as unknown).
//!
//! **R starts a new run:** a fresh world with the same seed and map size (and settings) as the one
//! being played, so the same town from the beginning: you wake up in the bedroom at the start hour,
//! with new zombies, untouched houses and empty run stats. The same seed because the client's map
//! copy and meshes are built once for that map; the new world is taken up like a quickload
//! (`saving::WorldSwap`). The world options of the command line that only set up the first world
//! (`--at`, `--fight-demo`, `--siege`) don't apply again. **Q or Esc quits** (autosaving, as
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

const TEXT: Color = Color::srgb(0.93, 0.9, 0.82);
const DIM: Color = Color::srgb(0.62, 0.6, 0.55);
const RED: Color = Color::srgb(0.85, 0.12, 0.08);
const RULE: Color = Color::srgba(0.93, 0.9, 0.82, 0.18);

/// The rows of the summary, top to bottom.
const ROWS: [&str; 8] = ["Survived (game time)", "Time played", "Zombies killed", "Distance walked", "Things looted", "Containers searched", "Wounds taken", "HP lost"];

#[derive(Component)]
struct DeathScreen;
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
            .add_systems(PreUpdate, death_keys.after(InputSystems).run_if(crate::live))
            .add_systems(Update, update_screen);
    }
}

fn font(size: f32) -> TextFont {
    TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }
}

fn rule(c: &mut ChildSpawnerCommands) {
    c.spawn((Node { width: Val::Percent(100.0), height: px(1), margin: UiRect::vertical(px(6)), ..default() }, BackgroundColor(RULE)));
}

fn spawn_screen(mut commands: Commands) {
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
                    Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Stretch, padding: UiRect::axes(px(36), px(26)), row_gap: px(4), min_width: px(520), ..default() },
                    BackgroundColor(Color::srgba(0.03, 0.03, 0.035, 0.88)),
                ))
                .with_children(|c| {
                    c.spawn((Text::new("YOU DIED"), font(56.0), TextColor(RED), Node { align_self: AlignSelf::Center, ..default() }));
                    c.spawn((CauseText, Text::new(""), font(24.0), TextColor(TEXT), Node { align_self: AlignSelf::Center, margin: UiRect::top(px(4)), ..default() }));
                    c.spawn((EpilogueText, Text::new(""), font(15.0), TextColor(DIM), Node { align_self: AlignSelf::Center, ..default() }));
                    c.spawn((WhenText, Text::new(""), font(15.0), TextColor(DIM), Node { align_self: AlignSelf::Center, ..default() }));
                    rule(c);
                    for (k, label) in ROWS.iter().enumerate() {
                        c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(40), ..default() }).with_children(|row| {
                            row.spawn((Text::new(*label), font(18.0), TextColor(DIM)));
                            row.spawn((StatValue(k), Text::new(""), font(18.0), TextColor(TEXT)));
                        });
                    }
                    rule(c);
                    c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(40), ..default() }).with_children(|row| {
                        row.spawn((Text::new("R  new run (same town, from the start)"), font(16.0), TextColor(TEXT)));
                        row.spawn((Text::new("Q / Esc  quit"), font(16.0), TextColor(TEXT)));
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
        run.kills.to_string(),
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

#[allow(clippy::type_complexity)]
fn update_screen(
    me: Res<LocalPlayer>,
    link: Res<ServerLink>,
    mut screen: Query<&mut Visibility, With<DeathScreen>>,
    mut texts: ParamSet<(Query<&mut Text, With<CauseText>>, Query<&mut Text, With<EpilogueText>>, Query<&mut Text, With<WhenText>>, Query<(&mut Text, &StatValue)>)>,
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
    let set = |t: &mut Text, s: String| {
        if t.0 != s {
            t.0 = s;
        }
    };
    let run = link.curr.run_of(me.id).cloned().unwrap_or_default();
    let death = run.death;
    if let Ok(mut t) = texts.p0().single_mut() {
        set(&mut t, death.map_or_else(|| "Cause unknown".to_string(), |d| d.describe()));
    }
    if let Ok(mut t) = texts.p1().single_mut() {
        set(&mut t, death.and_then(|d| d.epilogue()).unwrap_or("").to_string());
    }
    if let Ok(mut t) = texts.p2().single_mut() {
        set(&mut t, death.map_or_else(String::new, |d| when(d.day, d.hour)));
    }
    let values = values(&run);
    for (mut t, k) in texts.p3().iter_mut() {
        set(&mut t, values[k.0].clone());
    }
}

/// Dead: R (or `--new-run-after`) starts a new run, Q or Esc quits.
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
    if keys.any_just_pressed([KeyCode::KeyQ, KeyCode::Escape]) {
        println!("Quitting from the death screen");
        exit.write(AppExit::Success);
        return;
    }
    let due = !*automated && options.new_run_after.is_some_and(|t| now >= t);
    *automated |= due;
    if !(keys.just_pressed(KeyCode::KeyR) || due) {
        return;
    }
    let (world, id) = LocalServer::new_run(&swap.link.world().config);
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
}
