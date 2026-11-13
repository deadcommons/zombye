//! The main menu: what the game starts on when nothing on the command line chooses a world.
//!
//! Continue (the autosave quitting left), New game, Load game (every save in the saves folder),
//! Multiplayer (the server browser), Mods (the mods folder: which are on, their order, what is
//! wrong with one; `zombye_mods::folder`), Settings (`settings_ui.rs`) and Quit.
//!
//! The game builds its map and meshes once per world at start-up, so like the join screen
//! (`join_screen.rs`) this is its own small Bevy app in front of the game: a window, the skin, the
//! menu. Whatever is picked starts the game again with the arguments that say so (`--load NAME`,
//! `--new-game --seed N`, `--browser`), plus `--no-menu` and everything else this run was given
//! (`--saves`, `--data`, `--mod`, graphics flags...). The pause menu's way back here
//! (`pause_menu.rs`) is the same in the other direction: the game quits, autosaving, and starts
//! again with `--main-menu`.
//!
//! **When it shows** ([`wanted`]): in a window, when every flag given is one that only sets the
//! scene ([`plain`]: where saves and data are, graphics, sound, the name). Any flag that chooses a
//! world or is there for a test, a recording or a picture (`--load`, `--seed`, `--zombies`,
//! `--fight-demo`, `--exit-after`, `--connect`...) goes straight into the game as before, so
//! nothing that drives the game by flags changes. `--main-menu` shows it regardless, `--no-menu`
//! never. `--new-game` (and `--new-game-from`) belong to the new-game screen: the New game
//! button passes `--new-game` on, and until that screen is in the game the flag is simply
//! ignored and a fresh world with the seed given starts.
//!
//! For pictures: `--main-menu --render-to shot.png` draws it offscreen (`offscreen.rs`), with
//! `--menu-screen load|new|mods` on one of its other screens or `--settings TAB` under the settings.

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use bevy::app::ScheduleRunnerPlugin;
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use zombye_net::save_file::{self, SaveEntry};

use crate::Options;
use crate::browser_ui::Relaunch;
use crate::join_screen::args_without;
use crate::settings_ui::SettingsUi;
use crate::menu_skin::{self as skin, BONE, DIM, Look};
use crate::release_brand::{self as brand, Mark};
use crate::theme::{self, BODY, SMALL, font};
use zombye_mods::folder::{self as mods_folder, Folder};

/// Flags (with a value, and alone) that only set the scene: with nothing but these, the game
/// starts on the menu.
const SCENE_WITH_VALUE: [&str; 23] = ["--biome", "--biomes", "--size", "--saves", "--data", "--mod", "--mods-dir", "--graphics", "--vegetation", "--no-post", "--msaa", "--shadow-map", "--render-scale", "--ui-scale", "--volume", "--music", "--name", "--server-list", "--crowd-vat", "--survivor", "--opening-hints", "--help-text", "--old-look"];
const SCENE_ALONE: [&str; 11] = ["--watch-biomes", "--dev", "--no-mods", "--novsync", "--mute", "--load-anyway", "--first-person", "--third-person", "--zombie-silhouettes", "--walls-up", "--rotate"];
/// What chose this run's world or screen, dropped when another is chosen.
const WORLD_WITH_VALUE: [&str; 13] = ["--connect", "--password", "--load", "--from-browser", "--browser-join", "--join-error", "--seed", "--new-game-from", "--settings", "--menu-screen", "--server-info", "--skip-hours", "--at"];
const WORLD_ALONE: [&str; 6] = ["--joined", "--browser", "--no-menu", "--main-menu", "--new-game", "--pause"];
/// Saves listed on the Load screen (the newest).
const LISTED: usize = 9;
/// Mods listed on the Mods screen.
const LISTED_MODS: usize = 10;
/// The Mods screen's details pane.
const DETAILS_WIDTH: f32 = 440.0;
/// A mod that won't load, on the Mods screen.
const PROBLEM: Color = theme::flat::RED_TEXT;

/// Every argument is a flag that only sets the scene (or there are none).
pub fn plain(args: &[String]) -> bool {
    let mut k = 0;
    while k < args.len() {
        let a = args[k].as_str();
        if SCENE_WITH_VALUE.contains(&a) {
            k += 2;
        } else if SCENE_ALONE.contains(&a) {
            k += 1;
        } else {
            return false;
        }
    }
    true
}

/// Whether to show the menu instead of going straight into the game (see the module docs).
pub fn wanted(o: &Options) -> bool {
    !o.new_game && o.new_game_from.is_none() && o.connect.is_none() && o.server_info.is_none() && (o.main_menu || (!o.no_menu && o.plain && !o.offscreen()))
}

/// Whether a menu that would show is skipped for development: `ZOMBYE_NO_MENU` set (to anything
/// but nothing or 0), or "Skip the main menu" in the settings. `--main-menu` shows it regardless
/// (the pause menu's way back, and the way to the switch again).
pub fn skipped(o: &Options) -> bool {
    !o.main_menu && skip_asked(std::env::var("ZOMBYE_NO_MENU").ok().as_deref(), || crate::ui_scale::ClientSettings::load(&crate::ui_scale::ClientSettings::path(&o.saves)).0.skip_menu)
}

fn skip_asked(env: Option<&str>, setting: impl FnOnce() -> bool) -> bool {
    env.is_some_and(|v| !v.is_empty() && v != "0") || setting()
}

/// This run's arguments for the game itself, in the world `add` says.
fn game_args(args: &[String], add: &[&str]) -> Vec<String> {
    let mut out = args_without(args, &WORLD_WITH_VALUE, &WORLD_ALONE, add);
    out.push("--no-menu".to_string());
    out
}

/// Into the game from a save (a name in the saves folder).
pub fn load_args(args: &[String], name: &str) -> Vec<String> {
    game_args(args, &["--load", name])
}

/// Into a new game: the new-game screen when the game has one (`--new-game`), a fresh world with
/// this seed either way.
pub fn new_game_args(args: &[String], seed: u64) -> Vec<String> {
    game_args(args, &["--new-game", "--seed", &seed.to_string()])
}

/// Into the server browser, over our own world (the autosave when there is one).
pub fn multiplayer_args(args: &[String], autosave: bool) -> Vec<String> {
    if autosave { game_args(args, &["--load", crate::saving::AUTOSAVE, "--browser"]) } else { game_args(args, &["--browser"]) }
}

/// Back to the menu from the game.
pub fn to_menu_args(args: &[String]) -> Vec<String> {
    args_without(args, &WORLD_WITH_VALUE, &WORLD_ALONE, &["--main-menu"])
}

static NEXT: Mutex<Option<Relaunch>> = Mutex::new(None);

/// Start the game again with `args` once this app has shut down (`main` does it).
pub fn ask_relaunch(args: Vec<String>, options: &Options) {
    if let Ok(mut n) = NEXT.lock() {
        *n = Some(Relaunch { args, password: options.password.clone() });
    }
}

pub fn relaunch_asked() -> bool {
    NEXT.lock().is_ok_and(|n| n.is_some())
}

pub fn take_relaunch() -> Option<Relaunch> {
    NEXT.lock().ok().and_then(|mut n| n.take())
}

/// A seed for a new game's town: different every time, and short enough to say.
fn fresh_seed() -> u64 {
    zombye_net::content::fresh_loot_seed() % 999_999 + 1
}

/// "5 min ago", "3 h ago", "2 days ago".
pub fn ago(secs: u64) -> String {
    match secs {
        0..=89 => "just now".to_string(),
        90..=5399 => format!("{} min ago", (secs + 30) / 60),
        5400..=172_799 => format!("{} h ago", (secs + 1800) / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// "14:30".
fn clock(hour: f32) -> String {
    let minutes = ((hour.rem_euclid(24.0)) * 60.0).floor() as u32;
    format!("{:02}:{:02}", minutes / 60 % 24, minutes % 60)
}

/// One line on a save: its game time, the dead about and when it was written; or why it can't be read.
pub fn describe(save: &SaveEntry, now: SystemTime) -> String {
    let when = save.modified.and_then(|m| now.duration_since(m).ok()).map(|d| ago(d.as_secs()));
    match &save.info {
        Ok(info) => {
            let mut line = format!("{} in the game, {} zombies", clock(info.time_of_day), info.alive);
            if let Some(when) = when {
                line = format!("{line}, saved {when}");
            }
            line
        }
        Err(e) => format!("can't be read: {e}"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Screen {
    #[default]
    Home,
    Load,
    /// Asking before a new game when there is an autosave it would come to replace.
    ConfirmNew,
    /// The mods folder: which mods are on and in what order.
    Mods,
}

#[derive(Clone, Debug, PartialEq)]
enum Act {
    Continue,
    New,
    StartNew,
    LoadScreen,
    Load(String),
    Multiplayer,
    Mods,
    /// Turn a mod on or off (by id).
    ToggleMod(String),
    /// Move a mod that is on one place earlier (true) or later in the load order.
    ShiftMod(String, bool),
    /// Show a mod's details.
    PickMod(String),
    OpenModsFolder,
    /// Read the mods folder again (after putting a mod in it).
    RescanMods,
    Settings,
    Back,
    Quit,
}

#[derive(Resource)]
struct Menu {
    options: Options,
    /// The saves folder's saves, newest first.
    saves: Vec<SaveEntry>,
    screen: Screen,
    acts: Vec<Act>,
    dirty: bool,
    /// How many mods `enabled.ron` turns on (the home screen's Mods button says).
    mods_on: usize,
    /// The mods folder, read and checked when the Mods screen opens.
    mods: Option<Folder>,
    /// The game's data the mods are checked against (read once, when first needed).
    base: Option<Result<zombye_sim::WorldConfig, String>>,
    /// The mod whose details are shown.
    picked: Option<String>,
    /// What went wrong saving the list or opening the folder.
    mods_note: Option<String>,
}

impl Menu {
    fn new(options: Options) -> Self {
        let saves = save_file::list(&options.saves).unwrap_or_else(|e| {
            eprintln!("can't list the saves in {}: {e}", options.saves.display());
            Vec::new()
        });
        let screen = match std::env::args().skip_while(|a| a != "--menu-screen").nth(1).as_deref() {
            Some("load") => Screen::Load,
            Some("new") => Screen::ConfirmNew,
            Some("mods") => Screen::Mods,
            _ => Screen::Home,
        };
        let mods_on = mods_folder::read_enabled(&options.mods_dir).map_or(0, |v| v.len());
        let mut menu = Menu { options, saves, screen, acts: Vec::new(), dirty: true, mods_on, mods: None, base: None, picked: None, mods_note: None };
        if screen == Screen::Mods {
            menu.read_mods();
        }
        menu
    }

    /// Read the mods folder and check every mod in it against the game's data.
    fn read_mods(&mut self) {
        let mut folder = mods_folder::scan(&self.options.mods_dir);
        let base = self.base.get_or_insert_with(|| zombye_net::content::load_config(&self.options.data, 1));
        match base {
            Ok(base) => folder.check(base),
            Err(e) => folder.problems.push(format!("can't read the game's data to check the mods against: {e}")),
        }
        if self.picked.as_ref().is_none_or(|id| !folder.entries.iter().any(|e| &e.id == id)) {
            self.picked = folder.entries.first().map(|e| e.id.clone());
        }
        self.mods_on = folder.enabled_ids().len();
        self.mods = Some(folder);
    }

    /// Change which mods are on, save it, and check them all again.
    fn change_mods(&mut self, change: impl FnOnce(&mut Folder) -> bool) {
        let Some(folder) = self.mods.as_mut() else { return };
        if change(folder) {
            self.mods_note = folder.save().err().map(|e| format!("Couldn't save which mods are on: {e}"));
            self.read_mods();
        }
    }

    /// The autosave, if it is there and can be read.
    fn autosave(&self) -> Option<&SaveEntry> {
        self.saves.iter().find(|s| s.name == crate::saving::AUTOSAVE && s.info.is_ok())
    }
}

/// Runs the menu and then whatever was picked: the game, or nothing (Quit).
pub fn run(options: Options) -> ! {
    let offscreen = options.offscreen();
    let mut app = App::new();
    app.insert_resource(ClearColor(skin::GROUND)).add_plugins(crate::window_plugins(&options)).insert_resource(options.clone()).insert_resource(Menu::new(options));
    if offscreen {
        app.add_plugins((ScheduleRunnerPlugin::run_loop(Duration::ZERO), crate::offscreen::OffscreenPlugin));
    }
    app.add_plugins((theme::ThemePlugin, crate::release_brand::BrandPlugin, crate::ui_scale::UiScalePlugin, crate::settings_ui::SettingsUiPlugin))
        .add_systems(Startup, camera)
        .add_systems(First, crate::exit_on_signal)
        .add_systems(Update, (keys, act, draw).chain());
    let failed = app.run().is_error();
    if let Some(next) = take_relaunch() {
        crate::relaunch(next);
    }
    std::process::exit(if failed { 1 } else { 0 });
}

const WIDTH: f32 = 520.0;
/// The logo over the home screen's buttons, and the word alone over the other screens.
const LOGO_WIDTH: f32 = 440.0;
const WORDMARK_WIDTH: f32 = 360.0;

fn camera(mut commands: Commands, target: Option<Res<crate::offscreen::OffscreenTarget>>) {
    let mut cam = commands.spawn(Camera2d);
    if let Some(target) = target {
        cam.insert((RenderTarget::Image(target.0.clone().into()), IsDefaultUiCamera));
    }
}

fn keys(keys: Res<ButtonInput<KeyCode>>, mut menu: ResMut<Menu>, mut settings: ResMut<SettingsUi>) {
    if keys.just_pressed(KeyCode::Escape) {
        if settings.open {
            settings.close();
        } else if menu.screen != Screen::Home {
            menu.acts.push(Act::Back);
        }
    }
    if keys.just_pressed(KeyCode::Enter) && !settings.open {
        let a = match menu.screen {
            Screen::Home if menu.autosave().is_some() => Act::Continue,
            Screen::Home => Act::New,
            Screen::ConfirmNew => Act::StartNew,
            Screen::Load | Screen::Mods => return,
        };
        menu.acts.push(a);
    }
}

fn act(mut menu: ResMut<Menu>, mut settings: ResMut<SettingsUi>, mut exit: MessageWriter<AppExit>) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    for a in std::mem::take(&mut menu.acts) {
        let next = match a {
            Act::Continue => Some(load_args(&args, crate::saving::AUTOSAVE)),
            Act::New if menu.autosave().is_some() => {
                menu.screen = Screen::ConfirmNew;
                None
            }
            Act::New | Act::StartNew => Some(new_game_args(&args, fresh_seed())),
            Act::LoadScreen => {
                menu.screen = Screen::Load;
                None
            }
            Act::Load(name) => Some(load_args(&args, &name)),
            Act::Multiplayer => Some(multiplayer_args(&args, menu.autosave().is_some())),
            Act::Mods => {
                menu.read_mods();
                menu.screen = Screen::Mods;
                None
            }
            Act::ToggleMod(id) => {
                let on = menu.mods.as_ref().and_then(|f| f.entries.iter().find(|e| e.id == id)).is_some_and(|e| !e.enabled);
                menu.change_mods(|f| f.set_enabled(&id, on));
                menu.picked = Some(id);
                None
            }
            Act::ShiftMod(id, up) => {
                menu.change_mods(|f| f.shift(&id, up));
                None
            }
            Act::PickMod(id) => {
                menu.picked = Some(id);
                None
            }
            Act::OpenModsFolder => {
                let dir = menu.options.mods_dir.clone();
                match std::fs::create_dir_all(&dir) {
                    Ok(()) => crate::report::reveal(&dir),
                    Err(e) => menu.mods_note = Some(format!("Couldn't make the mods folder {}: {e}", dir.display())),
                }
                None
            }
            Act::RescanMods => {
                menu.read_mods();
                None
            }
            Act::Settings => {
                settings.open = true;
                None
            }
            Act::Back => {
                menu.screen = Screen::Home;
                None
            }
            Act::Quit => {
                exit.write(AppExit::Success);
                None
            }
        };
        if let Some(next) = next {
            ask_relaunch(next, &menu.options);
            exit.write(AppExit::Success);
        }
        menu.dirty = true;
    }
}

#[derive(Component)]
struct Root;

fn draw(mut commands: Commands, mut menu: ResMut<Menu>, root: Option<Single<Entity, With<Root>>>) {
    if !menu.dirty {
        return;
    }
    menu.dirty = false;
    let root = match root {
        Some(r) => *r,
        None => commands.spawn((Root, Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, ..default() })).id(),
    };
    commands.entity(root).despawn_children();
    let now = SystemTime::now();
    commands.entity(root).with_children(|c| {
        c.spawn(Node { flex_grow: 1.0, padding: UiRect::left(percent(9)), flex_direction: FlexDirection::Column, justify_content: JustifyContent::Center, row_gap: px(8), ..default() }).with_children(|col| {
            title(col, menu.screen);
            match menu.screen {
                Screen::Home => home(col, &menu, now),
                Screen::Load => load(col, &menu, now),
                Screen::ConfirmNew => confirm_new(col),
                Screen::Mods => mods(col, &menu),
            }
        });
        skin::rule(c);
        c.spawn(Node { height: px(46), padding: UiRect::horizontal(percent(9)), align_items: AlignItems::Center, column_gap: px(22), flex_shrink: 0.0, ..default() }).with_children(|row| {
            match menu.screen {
                Screen::Home => skin::keycap(row, "Enter", if menu.autosave().is_some() { "continue" } else { "new game" }),
                Screen::Load | Screen::Mods => skin::keycap(row, "Esc", "back"),
                Screen::ConfirmNew => {
                    skin::keycap(row, "Enter", "start a new game");
                    skin::keycap(row, "Esc", "back");
                }
            }
            skin::spacer(row);
            row.spawn((Text::new(brand::NOTICE), font(SMALL), TextColor(skin::FAINT), TextLayout::no_wrap()));
            // The build (the packaging script names it: commit and date), so a report says what it was.
            let report = if zombye_net::install::packaged() { "F12 in game: report a problem   " } else { "" };
            row.spawn((Text::new(format!("{report}{}", zombye_net::install::build_name())), font(SMALL), TextColor(DIM)));
        });
    });
}

/// The head of a screen: the logo on the home screen and the word alone on the others, which
/// need the room for their lists. A build without the brand's pictures (`release_brand.rs`) has
/// the name as text.
fn title(col: &mut ChildSpawnerCommands, screen: Screen) {
    let (mark, width) = if screen == Screen::Home { (Mark::Logo, LOGO_WIDTH) } else { (Mark::Wordmark, WORDMARK_WIDTH) };
    let mut drawn = false;
    // (Out to the left by the picture's own margin, so the word starts where the buttons do,
    // and the room below that the tagline's margin gives the text title.)
    col.spawn(Node { margin: UiRect::new(px(-width * 0.04), px(0), px(0), px(18)), flex_shrink: 0.0, ..default() }).with_children(|c| drawn = brand::title(c, mark, width, DIM));
    if !drawn {
        brand::text_title(col, 104.0, BONE, DIM);
        col.spawn((Text::new("ZOMBIE SURVIVAL"), theme::head(20.0), TextColor(skin::GREEN), TextLayout::no_wrap(), Node { margin: UiRect::new(px(3), px(0), px(-12), px(26)), ..default() }));
    }
}

fn button(parent: &mut ChildSpawnerCommands, label: &str, detail: Option<&str>, look: Look, act: Act) {
    skin::button(parent, label, detail, look, px(WIDTH)).observe(move |_: On<PointerPress>, mut menu: ResMut<Menu>| menu.acts.push(act.clone()));
}

fn home(col: &mut ChildSpawnerCommands, menu: &Menu, now: SystemTime) {
    // One main button: carrying on if there is a game to carry on, else starting one.
    let first = match menu.autosave() {
        Some(save) => {
            button(col, "Continue", Some(&describe(save, now)), Look::Primary, Act::Continue);
            Look::Plain
        }
        None => Look::Primary,
    };
    button(col, "New game", None, first, Act::New);
    if !menu.saves.is_empty() {
        button(col, "Load game", None, Look::Plain, Act::LoadScreen);
    }
    button(col, "Multiplayer", None, Look::Plain, Act::Multiplayer);
    let on = (menu.mods_on > 0).then(|| format!("{} on", menu.mods_on));
    button(col, "Mods", on.as_deref(), Look::Plain, Act::Mods);
    button(col, "Settings", None, Look::Plain, Act::Settings);
    button(col, "Quit", None, Look::Plain, Act::Quit);
}

fn load(col: &mut ChildSpawnerCommands, menu: &Menu, now: SystemTime) {
    col.spawn((Text::new("LOAD GAME"), theme::head(22.0), TextColor(BONE)));
    for save in menu.saves.iter().take(LISTED) {
        let line = describe(save, now);
        if save.info.is_ok() {
            button(col, &save.name, Some(&line), Look::Row, Act::Load(save.name.clone()));
        } else {
            col.spawn((Text::new(format!("{}: {line}", save.name)), font(SMALL), TextColor(DIM), Node { width: px(WIDTH), ..default() }));
        }
    }
    if menu.saves.len() > LISTED {
        col.spawn((Text::new(format!("and {} older in {}", menu.saves.len() - LISTED, menu.options.saves.display())), font(SMALL), TextColor(DIM)));
    }
    col.spawn(Node { margin: UiRect::top(px(8)), ..default() }).with_children(|row| {
        skin::button(row, "Back", None, Look::Plain, Val::Auto).observe(|_: On<PointerPress>, mut menu: ResMut<Menu>| menu.acts.push(Act::Back));
    });
}

/// "1st", "2nd", "3rd", "4th".
fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// A small button that pushes `act`, or a faint one that does nothing.
fn small_button(parent: &mut ChildSpawnerCommands, label: &str, act: Option<Act>, width: Val) {
    match act {
        Some(act) => {
            skin::button(parent, label, None, Look::Plain, width).observe(move |_: On<PointerPress>, mut menu: ResMut<Menu>| menu.acts.push(act.clone()));
        }
        None => {
            skin::button(parent, label, None, Look::Disabled, width);
        }
    }
}

/// The Mods screen: the mods in the folder, on or off, in load order, each with what is wrong with
/// it; the one picked in a pane beside them; and the folder.
fn mods(col: &mut ChildSpawnerCommands, menu: &Menu) {
    let text = |c: &mut ChildSpawnerCommands, t: String, size: f32, colour: Color, width: f32| {
        c.spawn((Text::new(t), font(size), TextColor(colour), Node { max_width: px(width), ..default() }));
    };
    col.spawn((Text::new("MODS"), theme::head(22.0), TextColor(BONE)));
    text(col, "A mod is a folder in the mods folder. Turn one on and it loads in every game you start or load from now on, in the order shown (a later mod wins where two change the same thing). A mod that doesn't load stays off, with the reason, and the game starts without it. Multiplayer servers play with their own mods.".to_string(), BODY, DIM, WIDTH + DETAILS_WIDTH + 28.0);
    let Some(folder) = &menu.mods else { return };
    for p in &folder.problems {
        text(col, p.clone(), SMALL, PROBLEM, WIDTH + DETAILS_WIDTH + 28.0);
    }
    let on = folder.entries.iter().filter(|e| e.enabled).count();
    col.spawn(Node { column_gap: px(28), align_items: AlignItems::FlexStart, margin: UiRect::vertical(px(8)), ..default() }).with_children(|row| {
        row.spawn(Node { width: px(WIDTH), flex_direction: FlexDirection::Column, row_gap: px(6), flex_shrink: 0.0, ..default() }).with_children(|list| {
            if folder.entries.is_empty() {
                text(list, format!("No mods yet. Put a mod's folder in {}, then press Look again.", folder.dir.display()), BODY, DIM, WIDTH);
            }
            for (k, e) in folder.entries.iter().take(LISTED_MODS).enumerate() {
                let picked = menu.picked.as_deref() == Some(e.id.as_str());
                list.spawn(Node { column_gap: px(6), align_items: AlignItems::Center, ..default() }).with_children(|r| {
                    // The one whose details are shown: a green mark at its left.
                    r.spawn((Node { width: px(3), height: px(26), flex_shrink: 0.0, ..default() }, BackgroundColor(if picked { skin::GREEN } else { Color::NONE })));
                    let id = e.id.clone();
                    skin::button(r, if e.enabled { "On" } else { "Off" }, None, Look::Chip(e.enabled), px(58)).observe(move |_: On<PointerPress>, mut menu: ResMut<Menu>| menu.acts.push(Act::ToggleMod(id.clone())));
                    let detail = match (&e.problem, e.enabled) {
                        (Some(_), true) => "left off".to_string(),
                        (Some(_), false) => "won't load".to_string(),
                        (None, true) => format!("loads {}", ordinal(k + 1)),
                        (None, false) => e.version.clone(),
                    };
                    let id = e.id.clone();
                    skin::button(r, &e.name, Some(&detail), Look::Row, px(WIDTH - 3.0 - 58.0 - 2.0 * 70.0 - 4.0 * 6.0)).observe(move |_: On<PointerPress>, mut menu: ResMut<Menu>| menu.acts.push(Act::PickMod(id.clone())));
                    small_button(r, "Up", (e.enabled && k > 0).then(|| Act::ShiftMod(e.id.clone(), true)), px(70));
                    small_button(r, "Down", (e.enabled && k + 1 < on).then(|| Act::ShiftMod(e.id.clone(), false)), px(70));
                });
                if let Some(p) = &e.problem {
                    let short: String = p.chars().take(110).collect();
                    let more = if short.len() < p.len() { "..." } else { "" };
                    list.spawn((Text::new(format!("{short}{more}")), font(SMALL), TextColor(PROBLEM), Node { margin: UiRect::left(px(9)), max_width: px(WIDTH - 9.0), ..default() }));
                }
            }
            if folder.entries.len() > LISTED_MODS {
                text(list, format!("and {} more in {}", folder.entries.len() - LISTED_MODS, folder.dir.display()), SMALL, DIM, WIDTH);
            }
        });
        if let Some(e) = menu.picked.as_ref().and_then(|id| folder.entries.iter().find(|e| &e.id == id)) {
            let (bg, border) = skin::panel();
            row.spawn((Node { width: px(DETAILS_WIDTH), flex_direction: FlexDirection::Column, row_gap: px(8), padding: UiRect::all(px(16)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(6)), flex_shrink: 0.0, ..default() }, bg, border)).with_children(|d| {
                d.spawn((Text::new(e.name.to_uppercase()), theme::head(20.0), TextColor(BONE), Node { max_width: px(DETAILS_WIDTH - 32.0), ..default() }));
                let version = if e.version.is_empty() { String::new() } else { format!("version {}, ", e.version) };
                text(d, format!("{version}id {}", e.id), SMALL, DIM, DETAILS_WIDTH - 32.0);
                if !e.description.is_empty() {
                    text(d, e.description.clone(), BODY, BONE, DETAILS_WIDTH - 32.0);
                }
                if e.loaded.is_some() {
                    text(d, format!("Has {}.", e.contents()), SMALL, DIM, DETAILS_WIDTH - 32.0);
                }
                let state = match (&e.problem, e.enabled) {
                    (Some(_), true) => "On, but left off: it doesn't load.".to_string(),
                    (Some(_), false) => "Off. It wouldn't load if turned on.".to_string(),
                    (None, true) => format!("On: loads {} of {on}.", ordinal(folder.entries.iter().position(|o| o.id == e.id).unwrap_or(0) + 1)),
                    (None, false) => "Off.".to_string(),
                };
                text(d, state, BODY, BONE, DETAILS_WIDTH - 32.0);
                if let Some(p) = &e.problem {
                    text(d, p.clone(), SMALL, PROBLEM, DETAILS_WIDTH - 32.0);
                }
                text(d, e.dir.display().to_string(), SMALL, skin::FAINT, DETAILS_WIDTH - 32.0);
            });
        }
    });
    if let Some(note) = &menu.mods_note {
        text(col, note.clone(), SMALL, PROBLEM, WIDTH + DETAILS_WIDTH + 28.0);
    }
    text(col, format!("Mods folder: {}", folder.dir.display()), SMALL, DIM, WIDTH + DETAILS_WIDTH + 28.0);
    col.spawn(Node { column_gap: px(8), margin: UiRect::top(px(4)), ..default() }).with_children(|row| {
        small_button(row, "Open mods folder", Some(Act::OpenModsFolder), Val::Auto);
        small_button(row, "Look again", Some(Act::RescanMods), Val::Auto);
        small_button(row, "Back", Some(Act::Back), Val::Auto);
    });
}

fn confirm_new(col: &mut ChildSpawnerCommands) {
    col.spawn((Text::new("START A NEW GAME?"), theme::head(22.0), TextColor(BONE)));
    col.spawn((Text::new("The game you can continue is its autosave, and the new game's will take its place the first time you leave it. Quicksaves and other saves stay."), font(BODY), TextColor(DIM), Node { width: px(WIDTH), margin: UiRect::bottom(px(8)), ..default() }));
    button(col, "Start a new game", None, Look::Primary, Act::StartNew);
    button(col, "Back", None, Look::Plain, Act::Back);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    fn options(line: &str) -> Options {
        Options::parse(&args(line))
    }

    #[test]
    fn the_menu_is_for_a_plain_start_in_a_window() {
        for line in ["", "--saves /tmp/s --graphics low", "--mute --volume 0.5 --name ada", "--zombies 10 --main-menu", "--main-menu --render-to m.png"] {
            assert!(wanted(&options(line)), "{line}");
        }
        // Anything that chooses a world, or is there for a test or a picture, goes straight in.
        for line in ["--no-menu", "--load autosave", "--seed 4", "--zombies 500", "--fight-demo --autoplay", "--exit-after 5", "--screenshot s.png", "--render-to x.png", "--connect a", "--browser", "--new-game", "--new-game-from f.ron", "--time 9", "stray", "--server-info a --main-menu"] {
            assert!(!wanted(&options(line)), "{line}");
        }
    }

    #[test]
    fn development_can_skip_the_menu() {
        assert!(!skip_asked(None, || false) && !skip_asked(Some(""), || false) && !skip_asked(Some("0"), || false));
        assert!(skip_asked(Some("1"), || false) && skip_asked(None, || true));
        // `--main-menu` shows it whatever the settings or the environment say.
        assert!(!skipped(&options("--main-menu")));
    }

    #[test]
    fn what_is_picked_keeps_the_scene_and_drops_the_last_world() {
        let a = args("--saves s --main-menu --graphics low --load old --seed 3");
        assert_eq!(load_args(&a, "autosave"), args("--saves s --graphics low --load autosave --no-menu"));
        assert_eq!(new_game_args(&a, 77), args("--saves s --graphics low --new-game --seed 77 --no-menu"));
        assert_eq!(multiplayer_args(&a, true), args("--saves s --graphics low --load autosave --browser --no-menu"));
        assert_eq!(multiplayer_args(&a, false), args("--saves s --graphics low --browser --no-menu"));
        // None of them comes back to the menu by itself, and none is a join.
        for next in [load_args(&a, "autosave"), new_game_args(&a, 77), multiplayer_args(&a, false)] {
            let o = Options::parse(&next);
            assert!(!wanted(&o) && !crate::join_screen::wanted(&o), "{next:?}");
        }
        // Back from the game (a server's too): the menu, with nothing of that world left.
        let back = to_menu_args(&args("--saves s --connect 1.2.3.4 --joined --from-browser single --load autosave --no-menu --pause"));
        assert_eq!(back, args("--saves s --main-menu"));
        assert!(wanted(&Options::parse(&back)));
        // Back from the new-game screen: the menu again, though `--no-menu` brought the screen up.
        let back = crate::new_game::back_args(&new_game_args(&a, 77));
        assert!(wanted(&Options::parse(&back)), "{back:?}");
        assert!(crate::new_game::wanted(&Options::parse(&new_game_args(&a, 77))));
        // A new game's seed is one the game takes.
        let seed = fresh_seed();
        assert!((1..=999_999).contains(&seed));
        assert_eq!(Options::parse(&new_game_args(&[], seed)).seed, seed);
    }

    #[test]
    fn saves_read_in_plain_words() {
        assert_eq!(ago(20), "just now");
        assert_eq!(ago(300), "5 min ago");
        assert_eq!(ago(3 * 3600), "3 h ago");
        assert_eq!(ago(3 * 86_400 + 5), "3 days ago");
        assert_eq!(clock(14.5), "14:30");
        assert_eq!(clock(0.0), "00:00");
        assert_eq!(clock(23.999), "23:59");
        assert_eq!([1, 2, 3, 4, 11, 12, 13, 21, 22, 101].map(ordinal), ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st"]);
        let now = SystemTime::now();
        let bad = SaveEntry { name: "x".into(), path: "x.zsave".into(), bytes: 3, modified: None, info: Err("not a Zombye save".into()) };
        assert_eq!(describe(&bad, now), "can't be read: not a Zombye save");
    }
}
