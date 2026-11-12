//! The join screen (DCM-70): what a join is doing while it does it, and what went wrong in words a
//! player understands, with Retry, Back and Quit.
//!
//! The game builds its map and meshes once per world at start-up, so a join still starts the game
//! again on the server (`browser_ui`'s relaunch). This screen sits in front of that: `main` runs it
//! instead of the game when `--connect` is given in a window ([`wanted`]). It is its own small Bevy
//! app (a window, the skin, one panel), and a worker thread does the join step by step:
//! asking the server what it is ([`Step::Connect`]), whether this game can join it
//! ([`Step::Check`]: protocol, camera, agents, room, a password), getting its mods
//! ([`Step::Mods`]), saying hello with our key ([`Step::Join`]), building the town from its seed
//! and checking it against the server's ([`Step::Town`]) and waiting for the first snapshot. When
//! all of it went through, the connection is closed (the server keeps our survivor, by our key) and
//! the game is started again with `--joined`, which joins for real; a game whose join fails then
//! comes back here with `--join-error` and the reason. Any step that fails shows
//! [`zombye_net::joining::explain`]'s title and sentence.
//!
//! Back starts the game in its own world with the server browser open (the autosave the join left,
//! when it came from one), Quit quits.

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use bevy::prelude::*;
use zombye_net::joining::{self, Problem};
use zombye_net::modpack::{self, ModPack};
use zombye_net::remote::{self, Remote};
use zombye_net::save_file;

use crate::Options;
use crate::browser_ui::Relaunch;
use crate::theme::{self, BAD, BODY, DIM, GOLD, INK, SMALL, TEXT, TITLE, font};

/// A step of a join, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Connect,
    Check,
    Mods,
    Join,
    Town,
    Ready,
}

impl Step {
    pub const ALL: [Step; 6] = [Step::Connect, Step::Check, Step::Mods, Step::Join, Step::Town, Step::Ready];

    pub fn label(self) -> &'static str {
        match self {
            Step::Connect => "Connecting",
            Step::Check => "Checking the server",
            Step::Mods => "Getting the server's mods",
            Step::Join => "Joining",
            Step::Town => "Building the town",
            Step::Ready => "Ready",
        }
    }
}

/// What the worker says as it goes.
#[derive(Debug)]
enum Msg {
    /// Now at this step, with a line on what it's doing.
    At(Step, String),
    /// The server answered: its name and version.
    Info { name: String, version: String },
    Failed(String),
    Done,
}

/// Whether to show the join screen instead of going straight into the game: a join in a window,
/// not one made again after the screen (`--joined`), nor a run with no one watching (offscreen,
/// pictures, `--exit-after`) or played by a program (`--agent`, `--autoplay`), which fail on
/// stderr as before.
pub fn wanted(o: &Options) -> bool {
    o.connect.is_some() && !o.joined && !o.offscreen() && o.exit_after.is_none() && o.screenshot.is_none() && !o.autoplay && !o.agent && o.bench_live.is_none() && o.frame_bench.is_none()
}

/// The join, step by step, as the game will make it. Errors are the step's own words, for
/// [`joining::explain`].
fn attempt(options: &Options, tx: &Sender<Msg>) -> Result<(), String> {
    let addr = options.connect.clone().unwrap_or_default();
    let say = |step: Step, text: String| {
        let _ = tx.send(Msg::At(step, text));
    };
    let timeout = Duration::from_secs(10);
    say(Step::Connect, format!("Asking {addr} what it is"));
    let info = remote::query(&addr, timeout)?;
    let _ = tx.send(Msg::Info { name: info.rules.name.clone(), version: info.version.clone() });

    say(Step::Check, format!("{} of {} players on", info.online, info.rules.max_players));
    let (base, _, own) = crate::server_link::local_content(options)?;
    let cache = modpack::Cache::in_saves(&options.saves);
    let have: Vec<modpack::Hash> = own.iter().map(ModPack::hash).chain(cache.listed()).collect();
    zombye_net::browser::can_join(&info, &have, false)?;
    if info.password && options.password.is_empty() {
        return Err(format!("{} needs a password", info.rules.name));
    }

    let (n, bytes) = modpack::to_download(&info.mods, &have);
    say(
        Step::Mods,
        match (info.mods.len(), n) {
            (0, _) => "The server runs no mods".to_string(),
            (all, 0) => format!("{all} mod{}, all here already", if all == 1 { "" } else { "s" }),
            (_, n) => format!("Downloading {n} mod{} ({})", if n == 1 { "" } else { "s" }, modpack::kb(bytes)),
        },
    );
    let (content, _) = crate::server_link::content_for(&addr, &info, options, &base, &own, &cache)?;

    let name = if options.name.trim().is_empty() { "a new name".to_string() } else { options.name.trim().to_string() };
    say(Step::Join, format!("Saying hello as {name}"));
    let identity = zombye_net::identity::Identity::load_or_create(&options.saves.join(zombye_net::identity::FILE))?;
    let mut remote = Remote::connect_with(&addr, &options.name, &options.password, false, &identity, timeout)?;
    if remote.welcome.rules.camera != zombye_net::wire::Camera::Isometric {
        return Err(format!("{} is first person only, and this version of the game has no first-person camera yet", remote.welcome.rules.name));
    }

    say(Step::Town, format!("From the server's seed, {} x {} m", remote.welcome.rules.map_m[0], remote.welcome.rules.map_m[1]));
    remote::build_map(&remote.welcome, &content)?;
    remote.wait_snapshot(timeout)?;
    // Our survivor stays on the server, lying where it is until the game joins again (by our key).
    drop(remote);
    say(Step::Ready, "Starting the game".to_string());
    Ok(())
}

fn work(options: Options) -> Receiver<Msg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let end = match attempt(&options, &tx) {
            Ok(()) => Msg::Done,
            Err(e) => Msg::Failed(e),
        };
        let _ = tx.send(end);
    });
    rx
}

/// Only the server's name and version, for the words of a failure the game brought back.
fn ask(addr: String) -> Receiver<Msg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        if let Ok(info) = remote::query(&addr, Duration::from_secs(5)) {
            let _ = tx.send(Msg::Info { name: info.rules.name, version: info.version });
        }
    });
    rx
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Retry,
    Back,
    Quit,
}

#[derive(Resource)]
struct Join {
    options: Options,
    addr: String,
    server: String,
    version: Option<String>,
    step: Step,
    detail: String,
    /// The step that failed, the raw error and how it reads.
    failed: Option<(Step, String, Problem)>,
    rx: Mutex<Option<Receiver<Msg>>>,
    acts: Vec<Act>,
    dirty: bool,
    since: f32,
}

impl Join {
    fn new(options: Options) -> Self {
        let addr = options.connect.clone().unwrap_or_default();
        let mut join = Join { options, server: addr.clone(), addr, version: None, step: Step::Connect, detail: String::new(), failed: None, rx: Mutex::new(None), acts: Vec::new(), dirty: true, since: 0.0 };
        match join.options.join_error.clone() {
            // The game's own join failed after this screen's went through (`--join-error`).
            Some(raw) => {
                join.fail(Step::Join, raw);
                *join.rx.lock().unwrap() = Some(ask(join.addr.clone()));
            }
            None => join.start(),
        }
        join
    }

    fn start(&mut self) {
        self.failed = None;
        self.step = Step::Connect;
        self.detail = String::new();
        self.dirty = true;
        *self.rx.lock().unwrap() = Some(work(self.options.clone()));
    }

    fn fail(&mut self, step: Step, raw: String) {
        eprintln!("can't join {}: {raw}", self.addr);
        let p = self.problem(&raw);
        self.failed = Some((step, raw, p));
        self.dirty = true;
    }

    fn problem(&self, raw: &str) -> Problem {
        let name = if self.server == self.addr { "" } else { self.server.as_str() };
        joining::explain(raw, &joining::Server { name, addr: &self.addr, version: self.version.as_deref() })
    }
}

/// Runs the join screen and then whatever it ended in: the game on the server, the game in its own
/// world, or nothing (Quit).
pub fn run(options: Options) -> ! {
    let mut app = App::new();
    app.insert_resource(ClearColor(Color::srgb(0.05, 0.05, 0.06)))
        .add_plugins(crate::window_plugins(&options))
        .insert_resource(Join::new(options))
        .add_plugins(theme::ThemePlugin)
        .add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2d);
        })
        .add_systems(First, crate::exit_on_signal)
        .add_systems(Update, (poll, keys, act, draw).chain());
    let failed = app.run().is_error();
    if let Some(next) = NEXT.lock().ok().and_then(|mut n| n.take()) {
        crate::relaunch(next);
    }
    std::process::exit(if failed { 1 } else { 0 });
}

static NEXT: Mutex<Option<Relaunch>> = Mutex::new(None);

/// This run's arguments without `drop` (flags, and flags with a value) and with `add`.
pub(crate) fn args_without(args: &[String], with_value: &[&str], alone: &[&str], add: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut k = 0;
    while k < args.len() {
        let a = args[k].as_str();
        if with_value.contains(&a) {
            k += 2;
            continue;
        }
        if !alone.contains(&a) {
            out.push(args[k].clone());
        }
        k += 1;
    }
    out.extend(add.iter().map(|s| s.to_string()));
    out
}

/// Into the game on the server: the same arguments, joining for real.
pub fn joined_args(args: &[String]) -> Vec<String> {
    args_without(args, &["--join-error", "--password"], &["--joined"], &["--joined"])
}

/// Back to this screen from the game, with what its join said.
pub fn failed_args(args: &[String], raw: &str) -> Vec<String> {
    let mut out = args_without(args, &["--join-error", "--password"], &["--joined"], &[]);
    out.extend(["--join-error".to_string(), raw.to_string()]);
    out
}

/// Back: the game in its own world with the server browser open (the autosave the join left, when
/// it came from one in the browser), not joining anything.
pub fn back_args(args: &[String], autosave: Option<&str>) -> Vec<String> {
    let mut out = args_without(args, &["--connect", "--password", "--from-browser", "--browser-join", "--join-error", "--load"], &["--joined", "--browser"], &["--browser"]);
    if let Some(path) = autosave {
        out.extend(["--load".to_string(), path.to_string()]);
    }
    out
}

fn poll(mut join: ResMut<Join>, time: Res<Time>) {
    let msgs: Vec<Msg> = match join.rx.lock() {
        Ok(rx) => rx.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    for msg in msgs {
        match msg {
            Msg::At(step, text) => {
                join.step = step;
                join.detail = text;
                join.since = time.elapsed_secs();
            }
            Msg::Info { name, version } => {
                if !name.trim().is_empty() {
                    join.server = name;
                }
                join.version = Some(version);
                // A failure shown before the server's version was known reads better with it.
                if let Some((step, raw, _)) = join.failed.clone() {
                    let p = join.problem(&raw);
                    join.failed = Some((step, raw, p));
                }
            }
            Msg::Failed(raw) => {
                let step = join.step;
                join.fail(step, raw);
            }
            Msg::Done => {
                let args = joined_args(&std::env::args().skip(1).collect::<Vec<_>>());
                if let Ok(mut n) = NEXT.lock() {
                    *n = Some(Relaunch { args, password: join.options.password.clone() });
                }
                join.acts.push(Act::Quit);
            }
        }
        join.dirty = true;
    }
}

fn keys(keys: Res<ButtonInput<KeyCode>>, mut join: ResMut<Join>) {
    if keys.just_pressed(KeyCode::Enter) && join.failed.is_some() {
        join.acts.push(Act::Retry);
    }
    if keys.just_pressed(KeyCode::Escape) {
        join.acts.push(Act::Back);
    }
}

fn act(mut join: ResMut<Join>, mut exit: MessageWriter<AppExit>) {
    let acts = std::mem::take(&mut join.acts);
    for a in acts {
        match a {
            Act::Retry => join.start(),
            Act::Back => {
                let o = &join.options;
                let autosave = save_file::path_for(&o.saves, crate::saving::AUTOSAVE).ok().filter(|p| p.is_file() && o.from_browser.as_deref() == Some("single"));
                let args = back_args(&std::env::args().skip(1).collect::<Vec<_>>(), autosave.as_ref().and_then(|p| p.to_str()));
                if let Ok(mut n) = NEXT.lock() {
                    *n = Some(Relaunch { args, password: o.password.clone() });
                }
                exit.write(AppExit::Success);
            }
            Act::Quit => {
                exit.write(AppExit::Success);
            }
        }
    }
}

#[derive(Component)]
struct Root;

/// The spinner's dots on the step under way, and the panel when something changed.
fn draw(mut commands: Commands, mut join: ResMut<Join>, time: Res<Time>, root: Option<Single<Entity, With<Root>>>, mut dots: Query<&mut Text, With<Dots>>) {
    let n = ((time.elapsed_secs() - join.since) * 3.0) as usize % 4;
    for mut t in &mut dots {
        let s = ".".repeat(n);
        if t.0 != s {
            t.0 = s;
        }
    }
    if !join.dirty {
        return;
    }
    join.dirty = false;
    let root = match root {
        Some(r) => *r,
        None => commands
            .spawn((Root, Node { width: percent(100), height: percent(100), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }))
            .id(),
    };
    commands.entity(root).despawn_children();
    commands.entity(root).with_children(|c| {
        c.spawn((
            Node { width: px(560), flex_direction: FlexDirection::Column, row_gap: px(6), padding: UiRect::axes(px(theme::SHEET_PAD.0 + 8.0), px(theme::SHEET_PAD.1 + 8.0)), ..default() },
            theme::sheet(),
        ))
        .with_children(|p| panel(p, &join));
    });
}

#[derive(Component)]
struct Dots;

fn panel(p: &mut ChildSpawnerCommands, join: &Join) {
    theme::title(p, &format!("Joining {}", join.server));
    let mut sub = join.addr.clone();
    if let Some(v) = join.version.as_deref().filter(|v| !v.trim().is_empty()) {
        sub = format!("{sub}  ·  Zombye {v}");
    }
    p.spawn((Text::new(sub), font(SMALL), TextColor(DIM), Node { align_self: AlignSelf::Center, ..default() }));
    theme::spawn_rule(p);
    let failed_at = join.failed.as_ref().map(|(s, _, _)| *s);
    for step in Step::ALL {
        let (colour, text_colour) = match (failed_at, step.cmp(&join.step)) {
            (Some(f), _) if step == f => (BAD, TEXT),
            (Some(f), _) if step > f => (DIM.with_alpha(0.35), DIM),
            (_, std::cmp::Ordering::Less) => (theme::BUTTON_REST, DIM),
            (None, std::cmp::Ordering::Equal) => (GOLD, TEXT),
            _ => (DIM.with_alpha(0.35), DIM),
        };
        p.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), ..default() }).with_children(|row| {
            row.spawn((Node { width: px(14), height: px(14), flex_shrink: 0.0, ..default() }, theme::dab(colour)));
            row.spawn((Text::new(step.label()), theme::strong(BODY), TextColor(text_colour)));
            if step == join.step && join.failed.is_none() {
                row.spawn((Text::new(""), font(BODY), TextColor(GOLD), Dots));
            }
        });
        if step == join.step && join.failed.is_none() && !join.detail.is_empty() {
            p.spawn((Text::new(join.detail.clone()), font(SMALL), TextColor(DIM), Node { margin: UiRect::left(px(24)), ..default() }));
        }
    }
    if let Some((_, _, problem)) = &join.failed {
        theme::spawn_rule(p);
        p.spawn((Text::new(problem.title.to_uppercase()), theme::head(TITLE), TextColor(BAD)));
        p.spawn((Text::new(problem.text.clone()), font(BODY), TextColor(TEXT)));
        p.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(10), margin: UiRect::top(px(10)), ..default() }).with_children(|row| {
            button(row, if problem.retry_helps { "Retry" } else { "Try again" }, Act::Retry);
            button(row, "Back", Act::Back);
            button(row, "Quit", Act::Quit);
        });
        p.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(18), margin: UiRect::top(px(6)), ..default() }).with_children(|row| {
            theme::keycap(row, "Enter", "try again");
            theme::keycap(row, "Esc", "back");
        });
    } else {
        theme::spawn_rule(p);
        p.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(10), ..default() }).with_children(|row| {
            button(row, "Cancel", Act::Back);
        });
    }
}

fn button(parent: &mut ChildSpawnerCommands, label: &str, act: Act) {
    parent
        .spawn((Node { padding: UiRect::axes(px(14), px(5)), flex_shrink: 0.0, ..default() }, theme::button(false)))
        .with_children(|b| {
            b.spawn((Text::new(label.to_string()), theme::strong(BODY), TextColor(INK), TextLayout::no_wrap(), Pickable::IGNORE));
        })
        .observe(move |_: On<PointerPress>, mut join: ResMut<Join>| join.acts.push(act));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn the_screen_is_for_people_joining_in_a_window() {
        assert!(wanted(&Options::parse(&args("--connect 1.2.3.4 --name ada"))));
        for line in ["", "--connect a --joined", "--connect a --autoplay", "--connect a --agent", "--connect a --render-to x.png", "--connect a --exit-after 5", "--connect a --screenshot s.png"] {
            assert!(!wanted(&Options::parse(&args(line))), "{line}");
        }
    }

    #[test]
    fn its_ways_out_keep_the_rest_of_the_arguments() {
        let a = args("--zombies 10 --connect 1.2.3.4 --name ada --from-browser single --password x");
        assert_eq!(joined_args(&a), args("--zombies 10 --connect 1.2.3.4 --name ada --from-browser single --joined"));
        let j = joined_args(&a);
        assert_eq!(failed_args(&j, "refused: the server is full"), [args("--zombies 10 --connect 1.2.3.4 --name ada --from-browser single"), vec!["--join-error".into(), "refused: the server is full".into()]].concat());
        // Back: no server, the browser open, our autosaved world if the join left one.
        assert_eq!(back_args(&failed_args(&j, "x"), Some("saves/autosave.zsave")), args("--zombies 10 --name ada --browser --load saves/autosave.zsave"));
        assert_eq!(back_args(&args("--connect a --browser"), None), args("--browser"));
        let o = Options::parse(&failed_args(&j, "x y"));
        assert_eq!((o.join_error.as_deref(), o.joined, wanted(&o)), (Some("x y"), false, true));
    }

    #[test]
    fn every_step_has_a_label() {
        assert!(Step::ALL.windows(2).all(|w| w[0] < w[1]));
        assert!(Step::ALL.iter().all(|s| !s.label().is_empty()));
    }
}
