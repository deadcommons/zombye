//! The loading screen: the logo over the whole window from the game's first frame until the
//! world behind it is ready to be looked at, then a short fade.
//!
//! The game builds its town and meshes before its first frame, but the GPU is still compiling
//! shaders for the frames after it, so things pop in one by one. This covers that: it stays up
//! until the render world has had no pipeline waiting for a few frames ([`ready`]), and never
//! longer than [`DEADLINE`]. In your own world time stands still behind it (`pause_menu::stop_time`),
//! so nothing happens to a survivor you can't see yet; on a server the world goes on.
//!
//! It shows when the game was started as a player starts it ([`wanted`]: from the main menu, or a
//! plain start with the menu skipped). A run driven by flags (`--load`, `--fight-demo`,
//! `--screenshot`...) goes straight in as before, so no recipe or recording changes.
//! `--loading-screen` shows it regardless and holds it up, for pictures:
//! `--render-to shot.png --hud --loading-screen`.
//!
//! With the brand folder (`release_brand.rs`) it is the logo; without, the name as text.
//!
//! **Loading lines:** under the logo, a line that says something is happening ("WAKING THE
//! DEAD..."), the next taking its place every couple of seconds, in a different order each start
//! ([`line_at`]). The lines are data: `assets/data/loading_lines.ron` ([`LoadingLines`]; a data
//! folder without the file gets the shipped one, and a bad file is a warning and the shipped one,
//! since a loading line is no reason not to start). A picture (`--loading-screen`) shows the
//! file's first line.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::render::render_resource::PipelineCache;
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::Options;
use crate::menu_skin::{BONE, DIM, GROUND};
use crate::release_brand::{self as brand, Mark};
use crate::theme;

/// Frames before the screen may go: the first few queue the pipelines it waits for.
const MIN_FRAMES: u32 = 12;
/// Frames in a row with no pipeline waiting.
const QUIET_FRAMES: u32 = 5;
/// The least it shows, so it is a screen and not a flash.
const MIN_SHOWN: Duration = Duration::from_millis(700);
/// The most: whatever is still compiling then pops in over the game.
const DEADLINE: Duration = Duration::from_secs(20);
/// The least the logo stands before the main menu on a plain start: long enough to be seen.
const SPLASH_SHOWN: Duration = Duration::from_millis(3200);
/// The logo's fade in from black on that first screen, and its slower fade out into the menu.
const SPLASH_IN_SECS: f32 = 0.9;
const SPLASH_OUT_SECS: f32 = 0.9;
/// The fade out.
const FADE_SECS: f32 = 0.35;
/// Frames the logo is up over a front-end screen before the game is started again in its place,
/// so it has been drawn when the window goes.
const COVER_FRAMES: u32 = 3;
/// The loading line: big and bright enough to be read from a chair, quieter than the logo.
const LINE_SIZE: f32 = 24.0;
const LINE_COLOUR: Color = BONE;
/// What the next run takes the line up from (`pass_on`, `handed`): "seed:milliseconds shown".
const HANDED: &str = "ZOMBYE_LOADING_LINE";
/// How wide the logo is drawn, in UI pixels.
const LOGO_WIDTH: f32 = 520.0;

/// The longest a line may be, in characters: it is one line of a 1280-pixel screen.
const MAX_LINE: usize = 60;

const SHIPPED: &str = include_str!("../../../assets/data/loading_lines.ron");

/// The loading screen's lines (`loading_lines.ron`).
#[derive(serde::Deserialize, Resource, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LoadingLines {
    /// Seconds a line stays before the next takes its place (0.5 to 30).
    #[serde(default = "default_every")]
    pub every_secs: f32,
    /// The lines, one shown at a time: each one line of at most 60 characters, drawn in capitals
    /// with three dots after it.
    pub lines: Vec<String>,
}

fn default_every() -> f32 {
    1.8
}

impl LoadingLines {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut l: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        if !(l.every_secs.is_finite() && (0.5..=30.0).contains(&l.every_secs)) {
            return Err(format!("every_secs is {}: it must be from 0.5 to 30", l.every_secs));
        }
        if l.lines.is_empty() {
            return Err("there are no lines".into());
        }
        for line in &mut l.lines {
            *line = line.trim().to_string();
            if line.is_empty() {
                return Err("a line is empty".into());
            }
            if line.chars().count() > MAX_LINE || line.chars().any(char::is_control) {
                return Err(format!("the line {line:?} must be one line of at most {MAX_LINE} characters"));
            }
        }
        Ok(l)
    }

    fn shipped() -> Self {
        Self::parse(SHIPPED).expect("the shipped loading_lines.ron is good (a test checks)")
    }

    /// The data folder `dir`'s lines: the shipped ones if it has no file, or (with a warning) a
    /// bad one.
    pub fn load(dir: &std::path::Path) -> Self {
        let path = dir.join("loading_lines.ron");
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).unwrap_or_else(|e| {
                eprintln!("{}: {e}; using the game's own lines", path.display());
                Self::shipped()
            }),
            Err(_) => Self::shipped(),
        }
    }

    /// The `k`th line shown of a start seeded `seed`, as it is drawn.
    fn shown(&self, seed: u64, k: u64) -> String {
        format!("{}...", self.lines[line_at(self.lines.len(), seed, k)].to_uppercase())
    }
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Which of `n` lines is the `k`th shown of a start seeded `seed`: every line once before any
/// comes again, from a different line and in a different order for different seeds (seed 0: the
/// file's own order).
pub fn line_at(n: usize, seed: u64, k: u64) -> usize {
    if n <= 1 {
        return 0;
    }
    let first = (seed % n as u64) as usize;
    // A step that shares no factor with `n` visits every line.
    let mut step = ((seed / n as u64) % n as u64) as usize;
    while gcd(step, n) != 1 {
        step += 1;
    }
    ((first as u64 + (k % n as u64) * step as u64) % n as u64) as usize
}

/// Whether this run starts behind the loading screen.
pub fn wanted(o: &Options) -> bool {
    o.loading_screen || (!o.offscreen() && (o.no_menu || o.plain || o.menu_layer) && o.screenshot.is_none() && o.exit_after.is_none())
}

/// The loading screen's state. `up` while it covers the game (fading included).
#[derive(Resource)]
pub struct Loading {
    pub up: bool,
    /// Where a start's time goes, printed once when the screen fades ([`Timeline`]).
    timeline: Timeline,
    /// Held up for a picture (`--loading-screen`).
    hold: bool,
    frames: u32,
    quiet: u32,
    started: Instant,
    /// Seconds into the fade, once it began.
    fading: Option<f32>,
    /// What orders this start's lines ([`line_at`]), and which of them is up.
    seed: u64,
    line: u64,
    /// The least it shows.
    least: Duration,
    /// Up until this run ends: the game is being started again behind it ([`leave`]).
    stay: bool,
    /// Seconds the logo and line take to come up out of black (0: there from the first frame).
    fade_in: f32,
    /// Seconds the fade out takes.
    fade_out: f32,
}

/// What the run before this one had up when it handed over, so the lines carry on.
static HAND_ON: std::sync::Mutex<Option<(u64, Duration)>> = std::sync::Mutex::new(None);

/// Tell the run started in this one's place where the lines were (`crate::relaunch`).
pub fn pass_on(command: &mut std::process::Command) {
    match HAND_ON.lock().ok().and_then(|h| *h) {
        Some((seed, shown)) => command.env(HANDED, format!("{seed}:{}", shown.as_millis())),
        None => command.env_remove(HANDED),
    };
}

/// The seed and time shown a run before us handed on ("seed:milliseconds").
fn handed(value: Option<&str>) -> Option<(u64, Duration)> {
    let (seed, ms) = value?.split_once(':')?;
    let ms: u64 = ms.parse().ok()?;
    // (A stale or made-up value is no reason for an odd start.)
    (ms <= 120_000).then_some(())?;
    Some((seed.parse().ok()?, Duration::from_millis(ms)))
}

/// Put the screen in `app`: `least` is the least it shows.
fn install(app: &mut App, hold: bool, offscreen: bool, data: &std::path::Path, least: Duration, splash: bool) {
    let lines = LoadingLines::load(data);
    let now = Instant::now();
    // A picture shows the file's first line; a player's start begins anywhere, or where the
    // screen before this one left off.
    let mut timeline = Timeline::default();
    let (seed, started) = if hold || offscreen {
        (0, now)
    } else if let Some((seed, shown)) = handed(std::env::var(HANDED).ok().as_deref()) {
        timeline.before_ms = shown.as_millis() as u64;
        (seed, now.checked_sub(shown).unwrap_or(now))
    } else {
        (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64 >> 10), now)
    };
    let line = (started.elapsed().as_secs_f32() / lines.every_secs) as u64;
    let waiting = Waiting::default();
    app.insert_resource(Loading { up: true, timeline, hold, frames: 0, quiet: 0, started, fading: None, seed, line, least: least + started.elapsed(), stay: false, fade_in: if splash && !hold { SPLASH_IN_SECS } else { 0.0 }, fade_out: if splash { SPLASH_OUT_SECS } else { FADE_SECS } })
        .insert_resource(lines)
        .insert_resource(waiting.clone())
        .add_systems(Startup, spawn)
        .add_systems(Update, drive.run_if(|l: Res<Loading>| l.up));
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.insert_resource(waiting).add_systems(Render, count_waiting.in_set(RenderSystems::Cleanup));
    }
}

/// The logo in front of the screens before the game (the main menu, the new-game screen): up
/// from the first frame and faded into the screen once it is ready, and up again while the game
/// is started in the screen's place, so a start is the logo, the menu, the logo, the game.
pub struct FrontLoadingPlugin;

/// Whether a front-end screen starts behind the logo: in a window a player sees.
pub fn front_wanted(o: &Options) -> bool {
    o.loading_screen || (!o.offscreen() && o.screenshot.is_none() && o.exit_after.is_none())
}

impl Plugin for FrontLoadingPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().resource::<Options>().clone();
        if front_wanted(&options) {
            // A plain start is the game's first screen: the logo stands a moment. A screen come
            // to from the game or another screen only bridges the change of window.
            let first = options.plain && !options.main_menu && !options.new_game;
            install(app, options.loading_screen, options.offscreen(), &options.data, if first { SPLASH_SHOWN } else { MIN_SHOWN }, first);
        }
        app.add_systems(Last, leave);
    }
}

/// Once a front-end screen has asked for the game to be started again (`crate::relaunch_pending`):
/// the logo over everything, and the exit a few frames later, when it has been drawn.
pub(crate) fn leave(
    mut commands: Commands,
    loading: Option<ResMut<Loading>>,
    lines: Option<Res<LoadingLines>>,
    root: Query<Entity, With<Root>>,
    mut exit: MessageWriter<AppExit>,
    mut frames: Local<u32>,
) {
    if !crate::relaunch_pending() || *frames > COVER_FRAMES {
        return;
    }
    let (Some(mut loading), Some(lines)) = (loading, lines) else {
        // No logo in this run (a picture, a test): straight on.
        *frames = COVER_FRAMES + 1;
        exit.write(AppExit::Success);
        return;
    };
    if *frames == 0 {
        for e in &root {
            commands.entity(e).despawn();
        }
        if !loading.up {
            // The lines begin again; a screen still up carries on with its own.
            loading.started = Instant::now();
            loading.line = 0;
        }
        loading.up = true;
        loading.stay = true;
        loading.fading = None;
        loading.fade_in = 0.0;
        put_up(&mut commands, lines.shown(loading.seed, loading.line), 1.0);
    }
    // (Each frame: the hand-over may be held back a while, `window_handoff.rs`.)
    if let Ok(mut h) = HAND_ON.lock() {
        *h = Some((loading.seed, loading.started.elapsed()));
    }
    *frames += 1;
    if *frames == COVER_FRAMES {
        exit.write(AppExit::Success);
    }
}

/// When this process began (`main` sets it first thing), for the start's timeline.
pub static PROCESS_START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
/// When the game's app was built (the town made, its data read), just before it ran.
pub static PLUGINS_BUILT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Where a start's time went: process start to the first frame (data, the town, meshes), and on
/// until the render world stopped waiting for shaders. One line on stdout, so the wait between a
/// screen's Start and the game can be measured.
#[derive(Default)]
struct Timeline {
    /// Milliseconds the screen before this run showed the logo for (handed on).
    before_ms: u64,
    first_frame: Option<Duration>,
    /// The last frame that had a pipeline waiting, and the most waiting at once.
    last_waiting: Option<Duration>,
    most_waiting: usize,
    told: bool,
}

impl Timeline {
    fn since_start() -> Duration {
        PROCESS_START.get().map_or(Duration::ZERO, |t| t.elapsed())
    }

    fn line(&self, fade_at: Duration) -> String {
        let ms = |d: Option<Duration>| d.map_or("-".to_string(), |d| format!("{} ms", d.as_millis()));
        format!(
            "start: {} before this run, app built at {}, first frame at {}, shaders done at {} ({} at most waiting), logo fades at {} ms",
            if self.before_ms > 0 { format!("{} ms", self.before_ms) } else { "nothing".into() },
            ms(PROCESS_START.get().zip(PLUGINS_BUILT.get()).map(|(a, b)| b.saturating_duration_since(*a))),
            ms(self.first_frame),
            ms(self.last_waiting),
            self.most_waiting,
            fade_at.as_millis()
        )
    }
}

/// Pipelines the render world is still waiting for, counted at the end of each render frame.
#[derive(Resource, Clone, Default)]
struct Waiting(Arc<AtomicUsize>);

#[derive(Component)]
struct Root;

/// The loading line's text.
#[derive(Component)]
struct Line;

/// On the screen's root: how much of the logo and line has come up out of black so far.
#[derive(Component)]
struct Coming(f32);

pub struct LoadingPlugin;

impl Plugin for LoadingPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().resource::<Options>();
        if !wanted(options) {
            return;
        }
        let (hold, offscreen, data) = (options.loading_screen, options.offscreen(), options.data.clone());
        // The game's first screen (#36: the menu over the game): the logo comes up and stands a
        // moment while everything loads behind it, and fades into the menu.
        let first = options.menu_layer && options.plain && !options.main_menu;
        install(app, hold, offscreen, &data, if first { SPLASH_SHOWN } else { MIN_SHOWN }, first);
    }
}

fn count_waiting(cache: Res<PipelineCache>, waiting: Res<Waiting>) {
    waiting.0.store(cache.waiting_pipelines().count(), Ordering::Relaxed);
}

fn spawn(mut commands: Commands, loading: Res<Loading>, lines: Res<LoadingLines>) {
    put_up(&mut commands, lines.shown(loading.seed, loading.line), if loading.fade_in > 0.0 { 0.0 } else { 1.0 });
}

/// The screen itself: the logo and a line, over everything.
/// `shown`: how much of the logo and line is there at first (the ground is always whole).
fn put_up(commands: &mut Commands, line: String, shown: f32) {
    commands
        .spawn((
            Root,
            Node { position_type: PositionType::Absolute, left: px(0), top: px(0), width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, row_gap: px(34), ..default() },
            BackgroundColor(GROUND),
            // Over every panel and the HUD.
            GlobalZIndex(i32::MAX - 8),
        ))
        .with_children(|c| {
            // The ground printed. (No horizon here: the logo has the sun already.)
            crate::menu_skin::ground(c);
            if !brand::title(c, Mark::Logo, LOGO_WIDTH, DIM) {
                brand::text_title(c, 104.0, BONE, DIM);
            }
            c.spawn((Line, Text::new(line), theme::note(LINE_SIZE), TextColor(LINE_COLOUR.with_alpha(shown)), TextLayout::no_wrap(), Pickable::IGNORE));
        })
        .insert(Coming(shown));
}

/// Whether the world behind the screen is ready to be shown.
fn ready(frames: u32, quiet: u32, shown: Duration, least: Duration) -> bool {
    shown >= DEADLINE.max(least) || (frames >= MIN_FRAMES && quiet >= QUIET_FRAMES && shown >= least)
}

/// How much of itself the screen still shows `secs` into its fade (1 to 0, eased).
fn fade(secs: f32, over: f32) -> f32 {
    let t = (secs / over.max(0.01)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

fn drive(
    mut commands: Commands,
    mut loading: ResMut<Loading>,
    waiting: Res<Waiting>,
    time: Res<Time<Real>>,
    root: Query<Entity, With<Root>>,
    children: Query<&Children>,
    mut backs: Query<&mut BackgroundColor, With<Root>>,
    mut texts: Query<&mut TextColor>,
    mut pictures: Query<&mut ImageNode>,
    lines: Res<LoadingLines>,
    mut line: Query<&mut Text, With<Line>>,
    mut coming: Query<&mut Coming>,
) {
    if loading.hold {
        return;
    }
    loading.frames += 1;
    // The next line, when its time comes.
    let k = (loading.started.elapsed().as_secs_f32() / lines.every_secs) as u64;
    if k != loading.line {
        loading.line = k;
        if let Ok(mut text) = line.single_mut() {
            text.0 = lines.shown(loading.seed, k);
        }
    }
    let now_waiting = waiting.0.load(Ordering::Relaxed);
    loading.quiet = if now_waiting == 0 { loading.quiet + 1 } else { 0 };
    let since = Timeline::since_start();
    if loading.timeline.first_frame.is_none() {
        loading.timeline.first_frame = Some(since);
    }
    if now_waiting > 0 {
        loading.timeline.last_waiting = Some(since);
        loading.timeline.most_waiting = loading.timeline.most_waiting.max(now_waiting);
    }
    if loading.fading.is_none() && !loading.stay && ready(loading.frames, loading.quiet, loading.started.elapsed(), loading.least) {
        loading.fading = Some(0.0);
        if !loading.timeline.told {
            loading.timeline.told = true;
            println!("{}", loading.timeline.line(since));
        }
    }
    // Coming up out of black: the logo and the line, over the ground.
    if loading.fading.is_none() && loading.fade_in > 0.0 {
        if let (Ok(root), Ok(mut coming)) = (root.single(), coming.single_mut()) {
            let a = 1.0 - fade(loading.started.elapsed().as_secs_f32(), loading.fade_in);
            if coming.0 != a {
                coming.0 = a;
                for e in children.iter_descendants(root) {
                    if let Ok(mut t) = texts.get_mut(e) {
                        t.0.set_alpha(a);
                    }
                    if let Ok(mut p) = pictures.get_mut(e) {
                        p.color.set_alpha(a);
                    }
                }
            }
        }
    }
    let Some(secs) = loading.fading.map(|s| s + time.delta_secs()) else { return };
    loading.fading = Some(secs);
    let Ok(root) = root.single() else {
        loading.up = false;
        return;
    };
    if secs >= loading.fade_out {
        commands.entity(root).despawn();
        loading.up = false;
        return;
    }
    let a = fade(secs, loading.fade_out);
    for e in std::iter::once(root).chain(children.iter_descendants(root)) {
        if let Ok(mut b) = backs.get_mut(e) {
            b.0.set_alpha(a);
        }
        if let Ok(mut t) = texts.get_mut(e) {
            t.0.set_alpha(a);
        }
        if let Ok(mut p) = pictures.get_mut(e) {
            p.color.set_alpha(a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_waits_for_the_shaders_but_not_for_ever() {
        let s = Duration::from_secs;
        // The first frames have queued nothing yet: not ready however quiet.
        assert!(!ready(3, 3, s(2), MIN_SHOWN));
        // Something still compiling.
        assert!(!ready(200, 0, s(5), MIN_SHOWN));
        // Quiet for long enough, but it only just came up.
        assert!(!ready(MIN_FRAMES, QUIET_FRAMES, Duration::from_millis(100), MIN_SHOWN));
        assert!(ready(MIN_FRAMES, QUIET_FRAMES, s(1), MIN_SHOWN));
        // The splash before the menu stands longer.
        assert!(!ready(MIN_FRAMES, QUIET_FRAMES, s(2), SPLASH_SHOWN) && ready(MIN_FRAMES, QUIET_FRAMES, s(4), SPLASH_SHOWN));
        // Never past the deadline.
        assert!(ready(5000, 0, DEADLINE, MIN_SHOWN));
    }

    #[test]
    fn the_lines_carry_on_across_a_start() {
        assert_eq!(handed(Some("77:2500")), Some((77, Duration::from_millis(2500))));
        for bad in [None, Some(""), Some("77"), Some("x:5"), Some("5:x"), Some("5:99999999")] {
            assert_eq!(handed(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_shipped_lines_are_good() {
        let l = LoadingLines::parse(SHIPPED).unwrap();
        assert!(l.lines.len() >= 12);
        assert_eq!(l.every_secs, default_every());
        let mut seen = l.lines.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), l.lines.len(), "a line is there twice");
        // A picture's line is the file's first, in capitals.
        assert_eq!(l.shown(0, 0), format!("{}...", l.lines[0].to_uppercase()));
    }

    #[test]
    fn bad_lines_are_refused() {
        assert!(LoadingLines::parse("(lines: [])").is_err());
        assert!(LoadingLines::parse("(lines: [\"  \"])").is_err());
        assert!(LoadingLines::parse("(lines: [\"two\\nlines\"])").is_err());
        assert!(LoadingLines::parse(&format!("(lines: [\"{}\"])", "x".repeat(MAX_LINE + 1))).is_err());
        assert!(LoadingLines::parse("(every_secs: 0.0, lines: [\"Waking\"])").is_err());
        assert!(LoadingLines::parse("(lines: [\"Waking\"], colour: 3)").is_err());
        assert_eq!(LoadingLines::parse("(lines: [\" Waking \"])").unwrap().lines, ["Waking"]);
    }

    #[test]
    fn every_line_has_its_turn_before_any_comes_again() {
        for n in [1usize, 2, 7, 12, 24] {
            for seed in [0u64, 1, 5, 23, 24, 1000, u64::MAX] {
                let mut turns: Vec<usize> = (0..n as u64).map(|k| line_at(n, seed, k)).collect();
                assert_eq!(line_at(n, seed, n as u64), turns[0]);
                turns.sort();
                assert_eq!(turns, (0..n).collect::<Vec<_>>(), "n {n}, seed {seed}");
            }
        }
        // Seed 0 is the file's order; another seed begins elsewhere.
        assert_eq!((line_at(24, 0, 0), line_at(24, 0, 1)), (0, 1));
        assert_ne!(line_at(24, 7, 0), 0);
    }

    #[test]
    fn the_fade_runs_from_all_to_nothing() {
        assert_eq!(fade(0.0, FADE_SECS), 1.0);
        assert_eq!(fade(FADE_SECS, FADE_SECS), 0.0);
        assert_eq!(fade(FADE_SECS * 5.0, FADE_SECS), 0.0);
        assert!(fade(FADE_SECS * 0.25, FADE_SECS) > fade(FADE_SECS * 0.75, FADE_SECS));
    }

    #[test]
    fn only_a_players_start_gets_the_screen() {
        let of = |args: &[&str]| Options::parse(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        // From the main menu, and a plain start with the menu skipped.
        assert!(wanted(&of(&["--load", "autosave", "--no-menu"])));
        assert!(wanted(&of(&[])));
        // A run set up by flags goes straight in, and a picture never has it unless asked.
        assert!(!wanted(&of(&["--fight-demo", "--autoplay"])));
        assert!(!wanted(&of(&["--render-to", "/tmp/x.png", "--no-menu"])));
        assert!(wanted(&of(&["--render-to", "/tmp/x.png", "--hud", "--loading-screen"])));
    }
}
