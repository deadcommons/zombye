//! The server browser (#27, docs/PLAN.md §3a): F7 in the game, and Multiplayer, Join a game on the
//! main menu, list the official servers and the community's, each with its players, ping, PvP or
//! co-op and mods, and join the one you pick.
//!
//! **Official servers** ([`official_source`]) are the ones the game's own files name
//! (`official_servers.ron`, `zombye_net::browser::read_official`): this repo ships that list
//! empty, the official build carries its makers'. They have the section at the top, and nothing a
//! community list or a player says can put a server there or change one of them.
//!
//! **Community servers:** a list anyone can keep (`zombye_net::browser`, RON): the file
//! or `http://` address given with `--server-list`, else `server_list` in the client's
//! `settings.ron`, else the data folder's `servers.ron` (shipped empty: there is no hosted
//! service, and the game asks none of ours). Servers you type in yourself (the Address field) are
//! kept in `settings.ron` and asked like the rest. Every server is asked what it is with the
//! query that doesn't join (#69), on threads of its own; the panel shows answers as they come.
//!
//! **Keys:** F7 or Esc shuts it; Up / Down pick a server (or click one); Enter (or Join) joins it;
//! R asks them all again; Delete takes a server you added off the list; Tab goes round the Name,
//! Password and Address fields (Enter in Address adds that server; Esc leaves a field). While it
//! is open the game's other keys and the mouse's buttons are held back, as with the chat line.
//!
//! A server's mods needn't be here: the game downloads their data on joining and checks it by
//! hash (#24, `server_link::server_content`); the details say how much there is to download.
//! **Joining** is checked first against what the server said (version, camera, agents, mods,
//! room), so a server this game can't join says why in the panel and nothing is left. Then the
//! game starts again on that server: your single-player world is autosaved as on quitting, and
//! the same program is run with `--connect` (the map and its meshes are built once per world at
//! start-up, so a server's world is another start). If the server then refuses (a wrong password,
//! the name in use, other game data), the game comes back to your autosaved world with the
//! browser open and the reason on it.

use std::sync::Mutex;

use bevy::input::InputSystems;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use zombye_net::browser::{self, Browser, ListState, Row, Status};

use crate::Options;
use crate::server_link::ServerLink;
use crate::menu_skin::{self as skin, Look};
use crate::theme::{self, BODY, DIM, GOLD, INK, ROW_HOVER, ROW_PICKED, SMALL, TEXT, TITLE, font};
use crate::ui_scale::SettingsStore;

/// (As wide inside the sheet's ragged edge as before it had one.)
const WIDTH: f32 = 932.0 + 2.0 * theme::SHEET_PAD.0;
/// Lines of the list on show at once (servers and the two section headings).
const VISIBLE: usize = 10;
const LINE_H: f32 = 24.0;
// In the game the skin is `theme.rs`'s: a sheet, light worn buttons, flat rows. Over the main
// menu it is the menu's (`Skin`).
const GOOD: Color = Color::srgb(0.6, 0.85, 0.55);
const WARN: Color = Color::srgb(0.95, 0.62, 0.35);
const ROW_BG: Color = theme::ROW;
const FIELD_BG: Color = Color::srgba(1.0, 1.0, 1.0, 0.08);
/// How the panel is drawn: in the game it is one of the HUD's painted sheets; opened from the main
/// menu it is a screen of the menu's, in the menu's look (`menu_skin.rs`).
#[derive(Clone, Copy)]
struct Skin {
    menu: bool,
    text: Color,
    dim: Color,
    accent: Color,
    row: Color,
    row_hover: Color,
    row_picked: Color,
    field: Color,
    field_line: Color,
}

impl Skin {
    fn of(menu: bool) -> Self {
        if menu {
            Skin { menu, text: skin::BONE, dim: skin::DIM, accent: skin::ACCENT, row: skin::CELL, row_hover: Color::srgb(0.244, 0.212, 0.189), row_picked: Color::srgb(0.36, 0.29, 0.19), field: skin::CELL, field_line: skin::LINE }
        } else {
            Skin { menu, text: TEXT, dim: DIM, accent: GOLD, row: ROW_BG, row_hover: ROW_HOVER, row_picked: ROW_PICKED, field: FIELD_BG, field_line: Color::NONE }
        }
    }
}

const MAX_NAME: usize = 24;
const MAX_FIELD: usize = 80;
/// The columns' widths, in UI pixels: name, players, ping, mode, locked, mods (the note takes the rest).
const COLS: [f32; 6] = [250.0, 70.0, 62.0, 56.0, 76.0, 180.0];

pub struct BrowserUiPlugin;

impl Plugin for BrowserUiPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().resource::<Options>().clone();
        SettingsStore::ensure(app);
        let start = app.world_mut().remove_resource::<BrowserStart>();
        let mut ui = BrowserUi { name: options.name.clone(), password: options.password.clone(), ..default() };
        if let Some(start) = start {
            ui.note = start.note;
            ui.selected = Some(zombye_net::remote::with_port(&start.addr));
            ui.warn = true;
            ui.want_open = true;
        }
        ui.want_open |= options.browser;
        app.insert_resource(ui)
            .init_resource::<LoadedMods>()
            .add_systems(Startup, spawn)
            // Before the chat line and every other key handler: while the browser is open the keys are its own.
            .add_systems(PreUpdate, keys.after(InputSystems).before(crate::chat::chat_keys).before(crate::inventory_ui::TextEntry).run_if(resource_exists::<SettingsStore>))
            .add_systems(Update, (script, act, poll, draw).chain().run_if(resource_exists::<SettingsStore>));
    }
}

/// The hashes of the mods this game has (#24): those it was started with (`--mod`) and those it
/// downloaded before. A server's other mods are downloaded on joining; the details say how much.
#[derive(Resource, Default, Clone)]
pub struct LoadedMods(pub Vec<zombye_net::modpack::Hash>);

/// Put in before the plugins are built when a join asked for in the browser failed: the game is
/// back in its own world, and the browser opens with the reason.
#[derive(Resource)]
pub struct BrowserStart {
    pub note: String,
    /// The server that refused, to have it picked again.
    pub addr: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Name,
    Password,
    Address,
}

/// What a click asked for; done in `act`.
#[derive(Clone, Debug, PartialEq)]
enum Act {
    Pick(String),
    Join,
    Refresh,
    Close,
    /// Take the picked server off our own servers.
    Remove,
    Focus(Field),
}

#[derive(Resource, Default)]
pub struct BrowserUi {
    pub open: bool,
    /// Open at the first frame (`--browser`, or back from a join that failed).
    want_open: bool,
    browser: Option<Browser>,
    /// The picked server's address.
    selected: Option<String>,
    /// The first line of the list on show.
    top: usize,
    field: Option<Field>,
    name: String,
    password: String,
    address: String,
    /// The line at the foot: what happened, or why not.
    note: String,
    warn: bool,
    acts: Vec<Act>,
    dirty: bool,
    /// `--browser-join` done.
    scripted: bool,
    /// Drawn as a screen of the main menu's (the menu is up), not as the game's panel.
    menu: bool,
}

impl BrowserUi {
    /// Open at the next frame (the main menu's Join a game).
    pub fn ask_open(&mut self) {
        self.want_open = true;
    }

    pub fn is_over_menu(&self) -> bool {
        self.menu
    }

    /// Whether the main menu is up: the panel is then drawn in the menu's look.
    pub fn over_menu(&mut self, menu: bool) {
        if self.menu != menu {
            (self.menu, self.dirty) = (menu, true);
        }
    }
}

#[derive(Component)]
struct Root;

/// What starting again on a server needs: the arguments, and the password (through the
/// environment, not the command line, where other programs could read it).
pub struct Relaunch {
    pub args: Vec<String>,
    pub password: String,
}

static RELAUNCH: Mutex<Option<Relaunch>> = Mutex::new(None);

pub fn relaunch_asked() -> bool {
    RELAUNCH.lock().is_ok_and(|r| r.is_some())
}

/// The join asked for in the browser, once the game has shut down (`main` runs it).
pub fn take_relaunch() -> Option<Relaunch> {
    RELAUNCH.lock().ok().and_then(|mut r| r.take())
}

/// The arguments to start again with on `addr`: this run's, without what chose its world or was
/// for this run only, and with the server, the name and where to come back to if it refuses
/// (`from`: `single`, our own autosaved world; `server`, nothing; `menu`, the main menu's
/// Multiplayer screen, where the join was asked for).
pub fn relaunch_args(args: &[String], addr: &str, name: &str, from: &str) -> Vec<String> {
    const WITH_VALUE: [&str; 11] = ["--connect", "--name", "--password", "--load", "--from-browser", "--browser-join", "--server-info", "--skip-hours", "--at", "--join-error", "--menu-screen"];
    const ALONE: [&str; 11] = ["--joined", "--browser", "--load-anyway", "--fight-demo", "--siege", "--duel", "--talk-demo", "--talk-wall", "--morning", "--pvp", "--main-menu"];
    let mut out = Vec::new();
    let mut k = 0;
    while k < args.len() {
        let a = args[k].as_str();
        if WITH_VALUE.contains(&a) {
            k += 2;
            continue;
        }
        if !ALONE.contains(&a) {
            out.push(args[k].clone());
        }
        k += 1;
    }
    out.extend(["--connect".to_string(), addr.to_string()]);
    if !name.trim().is_empty() {
        out.extend(["--name".to_string(), name.trim().to_string()]);
    }
    out.extend(["--from-browser".to_string(), from.to_string()]);
    out
}

/// The official servers' file: `$ZOMBYE_OFFICIAL_SERVERS`, else `official_servers.ron` in the
/// release brand folder (the official build's; `release_brand.rs`), else the data folder's, which
/// this repo ships empty.
pub fn official_source(options: &Options) -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("ZOMBYE_OFFICIAL_SERVERS").filter(|p| !p.is_empty()) {
        return p.into();
    }
    match crate::release_brand::root().map(|d| d.join(browser::OFFICIAL_FILE)).filter(|p| p.is_file()) {
        Some(p) => p,
        None => options.data.join(browser::OFFICIAL_FILE),
    }
}

/// Where the list comes from: `--server-list`, else the settings file's, else the data folder's
/// `servers.ron` if it is there (none: only the servers added by hand).
pub fn list_source(options: &Options, settings: Option<&str>) -> String {
    if let Some(s) = options.server_list.as_deref().or(settings).map(str::trim).filter(|s| !s.is_empty()) {
        return s.to_string();
    }
    let shipped = options.data.join(browser::LIST_FILE);
    if shipped.is_file() { shipped.to_string_lossy().into_owned() } else { String::new() }
}

/// A line of the list as shown.
#[derive(Clone, Debug, PartialEq)]
enum Line {
    Head(&'static str, usize),
    Server(String),
    /// A section with nothing in it says why.
    Note(&'static str),
}

/// The two sections, each under its heading: the official servers, then the community's (a
/// list's featured ones first).
fn lines(b: &Browser) -> Vec<Line> {
    let (official, community) = (b.official(), b.community());
    let mut out = vec![Line::Head("Official servers", official.len())];
    if official.is_empty() {
        out.push(Line::Note("This build lists none."));
    }
    out.extend(official.iter().map(|r| Line::Server(r.addr.clone())));
    out.push(Line::Head("Community servers", community.len()));
    if community.is_empty() {
        out.push(Line::Note(if b.list == ListState::Loading { "Reading the list..." } else { "None yet. Type a server's address in the Address field below and press Enter to add it." }));
    }
    out.extend(community.iter().map(|r| Line::Server(r.addr.clone())));
    out
}

/// The first line to show so that line `at` of `n` is among the `VISIBLE` on show, moving as
/// little as possible from `top`.
fn scrolled(top: usize, at: Option<usize>, n: usize) -> usize {
    let most = n.saturating_sub(VISIBLE);
    let mut top = top.min(most);
    if let Some(at) = at {
        // A server just under a heading shows its heading too.
        if at.saturating_sub(1) < top {
            top = at.saturating_sub(1);
        } else if at >= top + VISIBLE {
            top = at + 1 - VISIBLE;
        }
    }
    top.min(most)
}

/// A row's cells: name, players, ping, mode, mods, the note; and the colour of its state.
struct Cells {
    name: String,
    players: String,
    ping: String,
    mode: String,
    /// "password" when joining needs one.
    locked: String,
    mods: String,
    note: String,
    /// Up and joinable; up but not joinable (the reason is in the details); not up.
    state: RowState,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowState {
    Asking,
    Joinable,
    Blocked,
    Offline,
}

/// A whole number of milliseconds; a server on this machine or the next answers in under one.
fn ping_text(ms: u32) -> String {
    if ms == 0 { "<1 ms".into() } else { format!("{ms} ms") }
}

fn cells(row: &Row, mods: &[zombye_net::modpack::Hash], agent: bool) -> Cells {
    let name = format!("{}{}", row.name(), if row.mine { "  (added by you)" } else { "" });
    match &row.status {
        Status::Asking => Cells { name, players: String::new(), ping: "...".into(), mode: String::new(), locked: String::new(), mods: String::new(), note: row.entry.note.clone(), state: RowState::Asking },
        Status::Offline(_) => Cells { name, players: String::new(), ping: "-".into(), mode: String::new(), locked: String::new(), mods: String::new(), note: "no answer".into(), state: RowState::Offline },
        Status::Online { info, ping_ms } => {
            let blocked = browser::can_join(info, mods, agent).is_err();
            Cells {
                name,
                players: format!("{} / {}", info.online, info.rules.max_players),
                ping: ping_text(*ping_ms),
                mode: if info.rules.pvp { "PvP" } else { "Co-op" }.into(),
                locked: if info.password { "password" } else { "" }.into(),
                mods: browser::mods_text(&info.mods),
                note: row.entry.note.clone(),
                state: if blocked { RowState::Blocked } else { RowState::Joinable },
            }
        }
    }
}

/// The details of the picked server: a heading, a line on whether it can be joined (and its
/// colour), and its rules.
fn details(row: &Row, mods: &[zombye_net::modpack::Hash], agent: bool) -> (String, String, Color, Vec<(&'static str, String)>) {
    match &row.status {
        Status::Asking => (row.name().to_string(), format!("{}: asking...", row.addr), DIM, Vec::new()),
        Status::Offline(why) => (row.name().to_string(), format!("No answer: {why}"), WARN, Vec::new()),
        Status::Online { info, ping_ms } => {
            let head = format!("{}   {}   version {}   {}", row.name(), row.addr, info.version, ping_text(*ping_ms));
            let (line, colour) = match browser::can_join(info, mods, agent) {
                Ok(()) if info.password => ("Needs a password: type it below, then Enter joins".to_string(), GOLD),
                Ok(()) => ("Enter joins".to_string(), GOOD),
                Err(why) => (format!("Can't join: {why}"), WARN),
            };
            let mut rules = info.rules.lines();
            // With what joining would download of them (#24).
            rules.push(("Mods", browser::mods_note(&info.mods, mods)));
            if !row.entry.note.is_empty() {
                rules.push(("Note", row.entry.note.clone()));
            }
            (head, line, colour, rules)
        }
    }
}

fn spawn(mut commands: Commands) {
    commands.spawn((
        Root,
        Node {
            position_type: PositionType::Absolute,
            top: px(14),
            left: percent(50),
            margin: UiRect::left(px(-WIDTH / 2.0)),
            width: px(WIDTH),
            flex_direction: FlexDirection::Column,
            row_gap: px(4),
            padding: UiRect::axes(px(theme::SHEET_PAD.0), px(theme::SHEET_PAD.1)),
            ..default()
        },
        theme::sheet(),
        GlobalZIndex(30),
        Visibility::Hidden,
    ));
}

impl BrowserUi {
    fn say(&mut self, text: impl Into<String>, warn: bool) {
        self.note = text.into();
        self.warn = warn;
        self.dirty = true;
    }

    fn open_now(&mut self, options: &Options, store: &SettingsStore) {
        self.open = true;
        self.dirty = true;
        self.field = None;
        if self.name.is_empty() {
            self.name = store.settings.name.clone();
        }
        match &mut self.browser {
            Some(b) => b.refresh(),
            None => self.browser = Some(Browser::open_with(&browser::read_official(&official_source(options)), &list_source(options, store.settings.server_list.as_deref()), &store.settings.servers)),
        }
    }

    fn text_of(&mut self, field: Field) -> (&mut String, usize) {
        match field {
            Field::Name => (&mut self.name, MAX_NAME),
            Field::Password => (&mut self.password, MAX_FIELD),
            Field::Address => (&mut self.address, MAX_FIELD),
        }
    }

    /// Up or down the servers on show.
    fn step(&mut self, by: i32) {
        let Some(b) = &self.browser else { return };
        let shown: Vec<String> = b.shown().iter().map(|r| r.addr.clone()).collect();
        if shown.is_empty() {
            return;
        }
        let at = self.selected.as_ref().and_then(|s| shown.iter().position(|a| a == s));
        let next = match at {
            Some(i) => (i as i32 + by).clamp(0, shown.len() as i32 - 1) as usize,
            None => 0,
        };
        self.selected = Some(shown[next].clone());
        self.dirty = true;
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn keys(
    mut typed: MessageReader<KeyboardInput>,
    mut wheel: MessageReader<MouseWheel>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut ui: ResMut<BrowserUi>,
    options: Res<Options>,
    store: Res<SettingsStore>,
    chat: Option<Res<crate::chat::ChatEntry>>,
    find: Option<Res<crate::inventory_ui::Find>>,
) {
    if std::mem::take(&mut ui.want_open) {
        ui.open_now(&options, &store);
    }
    if !ui.open {
        typed.clear();
        wheel.clear();
        // Not while a line is being typed elsewhere.
        let typing = chat.is_some_and(|c| c.typing) || find.is_some_and(|f| f.typing);
        if !typing && keys.just_pressed(KeyCode::F7) {
            ui.open_now(&options, &store);
            keys.reset_all();
        }
        return;
    }
    for w in wheel.read() {
        let lines = if w.unit == MouseScrollUnit::Line { w.y } else { w.y / LINE_H };
        let n = ui.browser.as_ref().map_or(0, |b| lines_len(b));
        let top = (ui.top as f32 - lines.round()).clamp(0.0, n.saturating_sub(VISIBLE) as f32) as usize;
        if top != ui.top {
            ui.top = top;
            ui.dirty = true;
        }
    }
    for k in typed.read().filter(|k| k.state.is_pressed()) {
        if let Some(field) = ui.field {
            match &k.logical_key {
                Key::Character(c) => {
                    let (text, max) = ui.text_of(field);
                    let room = max.saturating_sub(text.chars().count());
                    text.extend(c.chars().filter(|c| !c.is_control() && (field != Field::Address || !c.is_whitespace())).take(room));
                }
                Key::Space if field != Field::Address => {
                    let (text, max) = ui.text_of(field);
                    if text.chars().count() < max {
                        text.push(' ');
                    }
                }
                Key::Backspace => {
                    ui.text_of(field).0.pop();
                }
                Key::Tab => {
                    ui.field = Some(match field {
                        Field::Name => Field::Password,
                        Field::Password => Field::Address,
                        Field::Address => Field::Name,
                    })
                }
                Key::Enter => match field {
                    Field::Address => ui.acts.push(Act::Focus(Field::Address)),
                    // Enter in the password is "join with this".
                    Field::Password => {
                        ui.field = None;
                        ui.acts.push(Act::Join);
                    }
                    Field::Name => ui.field = None,
                },
                Key::Escape => ui.field = None,
                _ => {}
            }
            ui.dirty = true;
            continue;
        }
        match (&k.logical_key, k.key_code) {
            (Key::Escape, _) | (_, KeyCode::F7) => ui.acts.push(Act::Close),
            (Key::ArrowUp, _) => ui.step(-1),
            (Key::ArrowDown, _) => ui.step(1),
            (Key::Enter, _) => ui.acts.push(Act::Join),
            (Key::Tab, _) => {
                ui.field = Some(Field::Name);
                ui.dirty = true;
            }
            (Key::Delete, _) => ui.acts.push(Act::Remove),
            (_, KeyCode::KeyR) | (_, KeyCode::F5) => ui.acts.push(Act::Refresh),
            _ => {}
        }
    }
    keys.reset_all();
    mouse.reset_all();
}

fn lines_len(b: &Browser) -> usize {
    lines(b).len()
}

/// `--browser-join N[@SECS]`: join the Nth server on show (from 1) once SECS have gone by and it
/// has answered. For recordings.
fn script(time: Res<Time>, options: Res<Options>, mut ui: ResMut<BrowserUi>) {
    let Some((n, after)) = options.browser_join else { return };
    if ui.scripted || !ui.open || time.elapsed_secs() < after {
        return;
    }
    let Some(b) = &ui.browser else { return };
    if !b.settled() {
        return;
    }
    let nth = b.shown().get(n.saturating_sub(1)).map(|r| r.addr.clone());
    ui.scripted = true;
    match nth {
        Some(addr) => {
            ui.acts.push(Act::Pick(addr));
            ui.acts.push(Act::Join);
        }
        None => ui.say(format!("--browser-join {n}: the browser shows fewer servers"), true),
    }
}

/// Do what the keys and clicks asked for.
#[allow(clippy::too_many_arguments)]
fn act(mut ui: ResMut<BrowserUi>, options: Res<Options>, mods: Res<LoadedMods>, link: Option<Res<ServerLink>>, mut store: ResMut<SettingsStore>, mut exit: MessageWriter<AppExit>) {
    for a in std::mem::take(&mut ui.acts) {
        ui.dirty = true;
        match a {
            Act::Pick(addr) => {
                ui.selected = Some(addr);
                ui.field = None;
            }
            Act::Close => {
                ui.open = false;
                ui.field = None;
            }
            Act::Refresh => {
                if let Some(b) = &mut ui.browser {
                    b.refresh();
                }
                ui.say("Asking every server again", false);
            }
            // A click on a field, or Enter in the address: add what was typed.
            Act::Focus(Field::Address) if ui.field == Some(Field::Address) && !ui.address.trim().is_empty() => {
                let typed = ui.address.trim().to_string();
                let Some(b) = ui.browser.as_mut() else { continue };
                match b.add(&typed) {
                    Ok(addr) => {
                        let mine = b.mine().to_vec();
                        ui.selected = Some(addr.clone());
                        ui.address.clear();
                        ui.field = None;
                        ui.say(format!("Added {addr}"), false);
                        if let Err(e) = store.change(|s| s.servers = mine) {
                            ui.say(format!("Added {addr}, but the settings file couldn't be written: {e}"), true);
                        }
                    }
                    Err(e) => ui.say(e, true),
                }
            }
            Act::Remove => {
                let picked = ui.selected.clone();
                let (Some(addr), Some(b)) = (picked, ui.browser.as_mut()) else { continue };
                if b.remove(&addr) {
                    let mine = b.mine().to_vec();
                    ui.selected = None;
                    ui.say(format!("Took {addr} off your servers"), false);
                    if let Err(e) = store.change(|s| s.servers = mine) {
                        ui.say(format!("The settings file couldn't be written: {e}"), true);
                    }
                } else if ui.browser.as_ref().and_then(|b| b.row(&addr)).is_some_and(|r| r.official) {
                    ui.say("That is an official server: only ones you added can be taken off", true);
                } else {
                    ui.say("That server is from the list: only ones you added can be taken off", true);
                }
            }
            Act::Focus(f) => ui.field = Some(f),
            Act::Join => join(&mut ui, &options, &mods, link.as_deref(), &mut store, &mut exit),
        }
    }
}

fn join(ui: &mut BrowserUi, options: &Options, mods: &LoadedMods, link: Option<&ServerLink>, store: &mut SettingsStore, exit: &mut MessageWriter<AppExit>) {
    let Some(row) = ui.selected.as_ref().and_then(|s| ui.browser.as_ref()?.row(s)).cloned() else {
        return ui.say("Pick a server first (click one, or Up / Down)", true);
    };
    let info = match &row.status {
        Status::Online { info, .. } => info,
        Status::Asking => return ui.say(format!("{} hasn't answered yet", row.name()), true),
        Status::Offline(why) => return ui.say(format!("Can't join {}: {why}", row.name()), true),
    };
    if let Err(why) = browser::can_join(info, &mods.0, options.agent || options.autoplay) {
        return ui.say(format!("Can't join {}: {why}", row.name()), true);
    }
    if info.password && ui.password.is_empty() {
        ui.field = Some(Field::Password);
        return ui.say(format!("{} needs a password: type it and press Enter", row.name()), true);
    }
    if options.offscreen() {
        return ui.say("An offscreen render joins nothing", true);
    }
    let name = ui.name.trim().to_string();
    if let Err(e) = store.change(|s| s.name = name.clone()) {
        eprintln!("can't write the settings: {e}");
    }
    // (No world here: the main menu, which a join that fails comes back to.)
    let single = link.is_some_and(|l| l.net.is_none());
    let from = if link.is_none() { "menu" } else if single { "single" } else { "server" };
    let args = relaunch_args(&std::env::args().skip(1).collect::<Vec<_>>(), &row.addr, &name, from);
    println!("server browser: joining {} at {}{}", row.name(), row.addr, if single { " (your world is autosaved)" } else { "" });
    if let Ok(mut r) = RELAUNCH.lock() {
        *r = Some(Relaunch { args, password: ui.password.clone() });
    }
    ui.say(format!("Joining {}...", row.name()), false);
    exit.write(AppExit::Success);
}

fn poll(mut ui: ResMut<BrowserUi>) {
    let open = ui.open;
    if let Some(b) = &mut ui.browser
        && b.poll()
        && open
    {
        ui.dirty = true;
    }
}

fn cell(row: &mut ChildSpawnerCommands, text: String, width: Option<f32>, colour: Color, size: f32) {
    let node = match width {
        Some(w) => Node { width: px(w), flex_shrink: 0.0, overflow: Overflow::clip(), ..default() },
        None => Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), overflow: Overflow::clip(), ..default() },
    };
    row.spawn((node, Pickable::IGNORE)).with_children(|c| {
        c.spawn((Text::new(text), font(size), TextColor(colour), TextLayout::no_wrap(), Pickable::IGNORE));
    });
}

fn button<'a>(parent: &'a mut ChildSpawnerCommands, label: &str, act: Act, k: Skin) -> EntityCommands<'a> {
    let mut e = if k.menu {
        skin::button(parent, label, None, if act == Act::Join { Look::Primary } else { Look::Plain }, Val::Auto)
    } else {
        let mut e = parent.spawn((Node { padding: UiRect::axes(px(12), px(4)), flex_shrink: 0.0, ..default() }, theme::button(false)));
        e.with_children(|b| {
            b.spawn((Text::new(label.to_string()), theme::strong(BODY), TextColor(INK), TextLayout::no_wrap(), Pickable::IGNORE));
        });
        e
    };
    e.observe(move |_: On<PointerPress>, mut ui: ResMut<BrowserUi>| ui.acts.push(act.clone()));
    e
}

fn field(parent: &mut ChildSpawnerCommands, label: &str, text: String, focused: bool, width: f32, which: Field, k: Skin) {
    parent.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(6), align_items: AlignItems::Center, ..default() }, Pickable::IGNORE)).with_children(|f| {
        f.spawn((Text::new(label.to_string()), font(BODY), TextColor(k.dim), Pickable::IGNORE));
        f.spawn((Node { width: px(width), height: px(if k.menu { 30.0 } else { 24.0 }), padding: UiRect::axes(px(6), px(3)), align_items: AlignItems::Center, overflow: Overflow::clip(), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(if k.menu { 4.0 } else { 0.0 })), ..default() }, BackgroundColor(k.field), BorderColor::all(if focused { k.accent } else { k.field_line })))
            .with_children(|b| {
                b.spawn((Text::new(if focused { format!("{text}_") } else { text }), font(BODY), TextColor(k.text), TextLayout::no_wrap(), Pickable::IGNORE));
            })
            .observe(move |_: On<PointerPress>, mut ui: ResMut<BrowserUi>| ui.acts.push(Act::Focus(which)));
    });
}

fn draw(mut commands: Commands, mut ui: ResMut<BrowserUi>, mods: Res<LoadedMods>, options: Res<Options>, mut root: Query<(Entity, &mut Visibility, &mut Node), With<Root>>, mut drawn_as: Local<Option<bool>>) {
    let Ok((root, mut vis, mut node)) = root.single_mut() else { return };
    let want = if ui.open { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != want {
        *vis = want;
    }
    if !ui.open || !std::mem::take(&mut ui.dirty) {
        return;
    }
    let k = Skin::of(ui.menu);
    if *drawn_as != Some(k.menu) {
        *drawn_as = Some(k.menu);
        // In the menu it stands where the menu's own screens do, on the menu's ground; in the
        // game it is a sheet at the top of the window.
        if k.menu {
            (node.left, node.top, node.margin, node.width, node.padding) = (percent(9), px(18), UiRect::ZERO, px(WIDTH - 2.0 * theme::SHEET_PAD.0), UiRect::ZERO);
            commands.entity(root).remove::<ImageNode>();
        } else {
            (node.left, node.top, node.margin, node.width, node.padding) = (percent(50), px(14), UiRect::left(px(-WIDTH / 2.0)), px(WIDTH), UiRect::axes(px(theme::SHEET_PAD.0), px(theme::SHEET_PAD.1)));
            commands.entity(root).insert(theme::sheet());
        }
    }
    let agent = options.agent || options.autoplay;
    let Some(b) = &ui.browser else { return };
    let all = lines(b);
    // The picked server stays picked while it's listed, and on show; with none picked, the first.
    let first = all.iter().find_map(|l| if let Line::Server(a) = l { Some(a.clone()) } else { None });
    let selected = ui.selected.clone().filter(|s| b.row(s).is_some()).or(first);
    let at = selected.as_ref().and_then(|s| all.iter().position(|l| *l == Line::Server(s.clone())));
    let top = scrolled(ui.top, at, all.len());
    // A file is named by its name alone: its whole path says nothing to a player.
    let named = |s: &str| if s.starts_with("http://") || s.starts_with("https://") { s.to_string() } else { std::path::Path::new(s).file_name().map_or(s.to_string(), |n| n.to_string_lossy().into_owned()) };
    let source = match (&b.list, b.source()) {
        (_, "") => "Community list: none set (--server-list FILE or http://..., or server_list in settings.ron), so only servers you add".to_string(),
        (ListState::Loading, s) => format!("Reading the community list: {}", named(s)),
        (ListState::Ready, s) => format!("Community list: {}", named(s)),
        (ListState::Failed(e), _) => format!("Can't use the community list: {e}"),
    };
    let source_colour = if matches!(b.list, ListState::Failed(_)) { WARN } else { k.dim };
    let summary = b.summary();
    let picked = selected.as_ref().and_then(|s| b.row(s)).map(|r| details(r, &mods.0, agent));
    let rows: Vec<(Line, Option<Cells>)> = all.iter().skip(top).take(VISIBLE).map(|l| (l.clone(), if let Line::Server(a) = l { b.row(a).map(|r| cells(r, &mods.0, agent)) } else { None })).collect();
    let more = (top, all.len().saturating_sub(top + VISIBLE));
    let (focus, name, password, address, note, warn) = (ui.field, ui.name.clone(), "*".repeat(ui.password.chars().count()), ui.address.clone(), ui.note.clone(), ui.warn);
    ui.top = top;
    ui.selected = selected.clone();

    commands.entity(root).despawn_children();
    commands.entity(root).with_children(|c| {
        // Opaque inside the sheet's ragged edge: it is a screen of its own, and the HUD under it
        // would read through. (In the menu the menu's own ground is behind it.)
        if !k.menu {
            c.spawn((
                Node { position_type: PositionType::Absolute, left: px(theme::SHEET_PAD.0 - 4.0), right: px(theme::SHEET_PAD.0 - 4.0), top: px(theme::SHEET_PAD.1 - 4.0), bottom: px(theme::SHEET_PAD.1 - 4.0), ..default() },
                BackgroundColor(theme::PANEL_BG.with_alpha(1.0)),
                Pickable::IGNORE,
            ));
        }
        // Title, and what the list holds.
        c.spawn((Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, ..default() }, Pickable::IGNORE)).with_children(|t| {
            if k.menu {
                t.spawn((Text::new("JOIN A GAME"), theme::brand(28.0), TextColor(k.text), Pickable::IGNORE));
            } else {
                t.spawn((Text::new("SERVERS"), theme::head(TITLE), TextColor(TEXT), Pickable::IGNORE));
            }
            t.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(10), align_items: AlignItems::Center, ..default() }, Pickable::IGNORE)).with_children(|r| {
                r.spawn((Text::new(summary), font(BODY), TextColor(k.dim), Pickable::IGNORE));
                button(r, "Refresh (R)", Act::Refresh, k);
                button(r, if k.menu { "Back (Esc)" } else { "Close (Esc)" }, Act::Close, k);
            });
        });
        c.spawn((Node { overflow: Overflow::clip(), ..default() }, Pickable::IGNORE)).with_children(|s| {
            s.spawn((Text::new(source), font(SMALL), TextColor(source_colour), TextLayout::no_wrap(), Pickable::IGNORE));
        });
        // The columns.
        c.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(8), padding: UiRect::horizontal(px(8)), margin: UiRect::top(px(4)), ..default() }, Pickable::IGNORE)).with_children(|h| {
            for (n, label) in ["Name", "Players", "Ping", "Mode", "Locked", "Mods"].into_iter().enumerate() {
                cell(h, label.to_string(), Some(COLS[n]), k.dim, SMALL);
            }
            cell(h, if more.0 > 0 { format!("Note   ({} more above)", more.0) } else { "Note".to_string() }, None, k.dim, SMALL);
        });
        // The list: always the same height, so the rest doesn't jump as answers come.
        c.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(2), height: px(VISIBLE as f32 * (LINE_H + 2.0)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|list| {
            for (line, cells) in rows {
                match (line, cells) {
                    (Line::Note(text), _) => {
                        list.spawn((Node { height: px(LINE_H), align_items: AlignItems::Center, padding: UiRect::horizontal(px(8)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|h| {
                            h.spawn((Text::new(text), font(BODY), TextColor(k.dim), TextLayout::no_wrap(), Pickable::IGNORE));
                        });
                    }
                    (Line::Head(title, n), _) => {
                        list.spawn((Node { height: px(LINE_H), align_items: AlignItems::End, padding: UiRect::horizontal(px(8)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|h| {
                            h.spawn((Text::new(format!("{title}  ({n})").to_uppercase()), if k.menu { theme::brand(BODY + 4.0) } else { theme::head(BODY + 1.0) }, TextColor(k.accent), Pickable::IGNORE));
                        });
                    }
                    (Line::Server(addr), Some(v)) => {
                        let is_picked = selected.as_deref() == Some(addr.as_str());
                        let rest = if is_picked { k.row_picked } else { k.row };
                        let over = if is_picked { k.row_picked } else { k.row_hover };
                        let colour = match v.state {
                            RowState::Joinable => k.text,
                            RowState::Blocked => WARN,
                            RowState::Asking | RowState::Offline => k.dim,
                        };
                        list.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(8), height: px(LINE_H), align_items: AlignItems::Center, padding: UiRect::horizontal(px(8)), flex_shrink: 0.0, ..default() }, BackgroundColor(rest)))
                            .with_children(|r| {
                                cell(r, v.name, Some(COLS[0]), colour, BODY);
                                cell(r, v.players, Some(COLS[1]), colour, BODY);
                                cell(r, v.ping, Some(COLS[2]), colour, BODY);
                                cell(r, v.mode, Some(COLS[3]), colour, BODY);
                                cell(r, v.locked, Some(COLS[4]), colour, BODY);
                                cell(r, v.mods, Some(COLS[5]), colour, BODY);
                                cell(r, v.note, None, k.dim, BODY);
                            })
                            .observe(move |ev: On<PointerOver>, mut q: Query<&mut BackgroundColor>| {
                                if let Ok(mut bg) = q.get_mut(ev.entity) {
                                    bg.0 = over;
                                }
                            })
                            .observe(move |ev: On<PointerOut>, mut q: Query<&mut BackgroundColor>| {
                                if let Ok(mut bg) = q.get_mut(ev.entity) {
                                    bg.0 = rest;
                                }
                            })
                            .observe(move |_: On<PointerPress>, mut ui: ResMut<BrowserUi>| ui.acts.push(Act::Pick(addr.clone())));
                    }
                    _ => {}
                }
            }
        });
        if more.1 > 0 {
            c.spawn((Text::new(format!("{} more below (wheel, or Down)", more.1)), font(SMALL), TextColor(k.dim), Pickable::IGNORE));
        }
        if k.menu {
            c.spawn((Node { width: percent(100), height: px(1), flex_shrink: 0.0, margin: UiRect::vertical(px(4)), ..default() }, BackgroundColor(skin::LINE), Pickable::IGNORE));
        } else {
            c.spawn((Node { width: percent(100), height: px(3), flex_shrink: 0.0, margin: UiRect::vertical(px(3)), ..default() }, theme::rule(), Pickable::IGNORE));
        }
        // The picked server: can it be joined, and its rules.
        c.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(1), min_height: px(238), ..default() }, Pickable::IGNORE)).with_children(|d| match picked {
            None => {
                d.spawn((Text::new("Pick a server to see its rules: click one, or Up / Down."), font(BODY), TextColor(k.dim), Pickable::IGNORE));
            }
            Some((head, line, colour, rules)) => {
                d.spawn((Node { overflow: Overflow::clip(), ..default() }, Pickable::IGNORE)).with_children(|h| {
                    h.spawn((Text::new(head), font(BODY), TextColor(k.text), TextLayout::no_wrap(), Pickable::IGNORE));
                });
                d.spawn((Text::new(line), font(BODY), TextColor(colour), Pickable::IGNORE));
                for (label, text) in rules {
                    d.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(10), ..default() }, Pickable::IGNORE)).with_children(|row| {
                        row.spawn((Text::new(label), font(SMALL), TextColor(k.dim), Node { width: px(112), flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
                        row.spawn((Text::new(text), font(SMALL), TextColor(k.text), Pickable::IGNORE));
                    });
                }
            }
        });
        if k.menu {
            c.spawn((Node { width: percent(100), height: px(1), flex_shrink: 0.0, margin: UiRect::vertical(px(4)), ..default() }, BackgroundColor(skin::LINE), Pickable::IGNORE));
        } else {
            c.spawn((Node { width: percent(100), height: px(3), flex_shrink: 0.0, margin: UiRect::vertical(px(3)), ..default() }, theme::rule(), Pickable::IGNORE));
        }
        // Who you join as, and a server of your own.
        c.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(14), align_items: AlignItems::Center, ..default() }, Pickable::IGNORE)).with_children(|f| {
            field(f, "Name", name, focus == Some(Field::Name), 150.0, Field::Name, k);
            field(f, "Password", password, focus == Some(Field::Password), 130.0, Field::Password, k);
            field(f, "Address", address, focus == Some(Field::Address), 230.0, Field::Address, k);
            f.spawn((Node { flex_grow: 1.0, ..default() }, Pickable::IGNORE));
            button(f, "Join (Enter)", Act::Join, k);
        });
        let hint = match focus {
            Some(Field::Address) => "Type host or host:port, Enter adds it to your servers  |  Tab next field  |  Esc done",
            Some(Field::Password) => "Enter joins with this password  |  Tab next field  |  Esc done",
            Some(Field::Name) => "The name you play under (none: the server makes one up)  |  Tab next field  |  Enter or Esc done",
            None => if k.menu { "Up / Down pick  |  Enter join  |  R refresh  |  Tab fields  |  Delete takes off a server you added  |  Esc back" } else { "Up / Down pick  |  Enter join  |  R refresh  |  Tab fields  |  Delete takes off a server you added  |  F7 or Esc close" },
        };
        c.spawn((Text::new(hint), font(SMALL), TextColor(k.dim), Pickable::IGNORE));
        if !note.is_empty() {
            c.spawn((Text::new(note), font(BODY), TextColor(if warn { WARN } else { k.accent }), Pickable::IGNORE));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use zombye_net::browser::ListEntry;
    use zombye_net::wire::{ModInfo, PROTOCOL, Rules, ServerInfo};

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn starting_again_on_a_server_keeps_the_session_and_drops_the_world() {
        let was = args("--novsync --saves my/saves --load autosave --fight-demo --zoom 12 --server-list lists/a.ron --browser --name old --password hunter2 --mod mods/alarm_clock --at 3,4");
        let now = relaunch_args(&was, "play.example:7777", " ann ", "single");
        assert_eq!(now, args("--novsync --saves my/saves --zoom 12 --server-list lists/a.ron --mod mods/alarm_clock --connect play.example:7777 --name ann --from-browser single"));
        // The password never goes on the command line.
        assert!(!now.iter().any(|a| a.contains("hunter2") || a == "--password"));
        // Hopping from one server to another: no name given, nothing to come back to.
        let hop = relaunch_args(&args("--connect a.example --from-browser single --mute"), "b.example:7777", "", "server");
        assert_eq!(hop, args("--mute --connect b.example:7777 --from-browser server"));
        // From the main menu's Multiplayer screen: the menu's own flags go, and it is where a join
        // that fails comes back to.
        let menu = relaunch_args(&args("--saves s --main-menu --menu-screen multiplayer"), "b.example:7777", "ann", "menu");
        assert_eq!(menu, args("--saves s --connect b.example:7777 --name ann --from-browser menu"));
    }

    fn row(status: Status, mine: bool) -> Row {
        Row { entry: ListEntry { address: "play.example".into(), name: "Listed name".into(), featured: false, note: "EU".into() }, addr: "play.example:7777".into(), mine, official: false, status }
    }

    fn online(info: ServerInfo) -> Status {
        Status::Online { info: Box::new(info), ping_ms: 42 }
    }

    fn info() -> ServerInfo {
        ServerInfo { protocol: PROTOCOL, version: "0.0.1".into(), rules: Rules { name: "Dead Commons".into(), max_players: 32, pvp: true, ..Default::default() }, online: 7, password: false, mods: Vec::new() }
    }

    #[test]
    fn a_row_says_players_ping_mode_and_mods() {
        let c = cells(&row(online(info()), false), &[], false);
        assert_eq!((c.name.as_str(), c.players.as_str(), c.ping.as_str(), c.mode.as_str(), c.mods.as_str(), c.note.as_str(), c.state), ("Dead Commons", "7 / 32", "42 ms", "PvP", "none", "EU", RowState::Joinable));
        // Locked, co-op, modded with a mod we haven't got: shown, and joinable (#24: its mods
        // are downloaded on joining, and the details say how much that is).
        let modded = ServerInfo { password: true, mods: vec![ModInfo { id: "alarm_clock".into(), name: "Alarm clock".into(), version: "1.0".into(), hash: [3; 32], size: 1536 }], rules: Rules { pvp: false, ..info().rules }, ..info() };
        let c = cells(&row(online(modded.clone()), true), &[], false);
        assert_eq!((c.name.as_str(), c.mode.as_str(), c.locked.as_str(), c.mods.as_str(), c.state), ("Dead Commons  (added by you)", "Co-op", "password", "Alarm clock 1.0", RowState::Joinable));
        assert_eq!((ping_text(0), ping_text(87)), ("<1 ms".to_string(), "87 ms".to_string()));
        let (_, _, _, rules) = details(&row(online(modded.clone()), false), &[], false);
        assert!(rules.iter().any(|(l, t)| *l == "Mods" && t == "Alarm clock 1.0 (1 to download, 1.5 KB)"), "{rules:?}");
        let (_, line, colour, rules) = details(&row(online(modded.clone()), false), &[[3; 32]], false);
        assert!(line.contains("password") && colour == GOLD);
        assert_eq!(rules.last().unwrap(), &("Note", "EU".to_string()));
        assert!(rules.iter().any(|(l, t)| *l == "Mods" && t == "Alarm clock 1.0") && rules.iter().any(|(l, _)| *l == "PvP"));
        // A mod too big for a server to hand out blocks the row unless we have it.
        let huge = ServerInfo { mods: vec![ModInfo { size: 64 << 20, ..modded.mods[0].clone() }], ..modded };
        assert_eq!(cells(&row(online(huge.clone()), false), &[], false).state, RowState::Blocked);
        assert_eq!(cells(&row(online(huge), false), &[[3; 32]], false).state, RowState::Joinable);
        // Not answered yet, and not answering: the list's name, and no numbers made up.
        let c = cells(&row(Status::Asking, false), &[], false);
        assert_eq!((c.name.as_str(), c.players.as_str(), c.ping.as_str(), c.state), ("Listed name", "", "...", RowState::Asking));
        let c = cells(&row(Status::Offline("can't connect".into()), false), &[], false);
        assert_eq!((c.ping.as_str(), c.note.as_str(), c.state), ("-", "no answer", RowState::Offline));
        let (_, line, _, rules) = details(&row(Status::Offline("can't connect to 1.2.3.4".into()), false), &[], false);
        assert!(line.contains("can't connect to 1.2.3.4") && rules.is_empty());
        // A full server.
        let full = ServerInfo { online: 32, ..info() };
        let (_, line, colour, _) = details(&row(online(full), false), &[], false);
        assert!(line.starts_with("Can't join: it is full") && colour == WARN);
    }

    #[test]
    fn the_browser_has_an_official_and_a_community_section() {
        use std::time::Duration;
        // Nothing listens on port 1, so every server is "no answer" at once.
        let official = [ListEntry { address: "127.0.0.1:1".into(), name: "Ours".into(), ..Default::default() }];
        let mut b = Browser::with_all(&official, "", &["127.0.0.2:1".into()], Duration::from_secs(2));
        assert!(b.wait(Duration::from_secs(20)));
        assert_eq!(lines(&b), [Line::Head("Official servers", 1), Line::Server("127.0.0.1:1".into()), Line::Head("Community servers", 1), Line::Server("127.0.0.2:1".into())]);
        // Both headings stay when a section is empty, each saying so.
        let mut none = Browser::with_timeout("", &[], Duration::from_secs(2));
        assert!(none.wait(Duration::from_secs(20)));
        let all = lines(&none);
        assert_eq!((all.len(), &all[0], &all[2]), (4, &Line::Head("Official servers", 0), &Line::Head("Community servers", 0)));
        assert!(matches!((&all[1], &all[3]), (Line::Note(_), Line::Note(_))));
        // This repo ships no official servers, in a file that is a list.
        let shipped = zombye_net::content::default_data_dir().join(browser::OFFICIAL_FILE);
        assert!(shipped.is_file() && browser::read_official(&shipped).is_empty());
        assert_eq!(browser::parse_list(&std::fs::read_to_string(shipped).unwrap()).unwrap().servers.len(), 0);
    }

    #[test]
    fn the_list_scrolls_to_keep_the_picked_server_on_show() {
        assert_eq!(scrolled(0, None, 5), 0);
        assert_eq!(scrolled(3, None, 5), 0, "a short list doesn't scroll");
        assert_eq!(scrolled(0, Some(VISIBLE), 30), 1);
        assert_eq!(scrolled(0, Some(29), 30), 30 - VISIBLE);
        assert_eq!(scrolled(20, Some(5), 30), 4, "the line above comes along (a heading)");
        assert_eq!(scrolled(4, Some(6), 30), 4, "already on show: stays put");
        assert_eq!(scrolled(99, None, 30), 30 - VISIBLE);
    }

    #[test]
    fn the_list_comes_from_the_flag_then_the_settings_then_the_data_folder() {
        let data = std::env::temp_dir().join(format!("zombye-browser-ui-{}", std::process::id()));
        std::fs::create_dir_all(&data).unwrap();
        let mut o = Options::parse(&args(&format!("--data {}", data.display())));
        assert_eq!(list_source(&o, None), "", "no file in the data folder: no list");
        std::fs::write(data.join(browser::LIST_FILE), "(servers: [])").unwrap();
        assert_eq!(list_source(&o, None), data.join(browser::LIST_FILE).to_string_lossy());
        assert_eq!(list_source(&o, Some("http://lists.example/z.ron")), "http://lists.example/z.ron");
        assert_eq!(list_source(&o, Some("  ")), data.join(browser::LIST_FILE).to_string_lossy());
        o.server_list = Some("mine.ron".into());
        assert_eq!(list_source(&o, Some("http://lists.example/z.ron")), "mine.ron");
        let _ = std::fs::remove_dir_all(data);
        // The shipped list is a list (and empty: no hosted service).
        let shipped = zombye_net::content::default_data_dir().join(browser::LIST_FILE);
        assert_eq!(browser::parse_list(&std::fs::read_to_string(shipped).unwrap()).unwrap().servers.len(), 0);
    }
}
