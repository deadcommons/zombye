//! The settings screen: graphics, audio, the controls and the game's own options, on tabs.
//!
//! One screen, drawn by two apps: the game (from the pause menu, `pause_menu.rs`) and the main
//! menu (`main_menu.rs`). It only ever changes the client settings (`ui_scale::SettingsStore`,
//! `settings.ron` in the saves folder), so it needs nothing of the game to be drawn; in the game
//! [`apply_live`] then puts each setting that changed into effect where the game keeps it (the
//! graphics resource, the sound's volumes, the UI scale, the camera's view, the window).
//!
//! **What is on it is data in one place:** [`rows`] lists a tab's rows from the settings, each a
//! label, a line on what it does and a control (a few choices, or a value with steps down and up),
//! and every control names the [`Change`] it makes, which [`change`] applies to the settings. A
//! new setting is a field in `ClientSettings`, a `Change`, a row, and a line in `apply_live`.
//!
//! Gameplay tuning and difficulty aren't here: they belong to a world, so they are chosen when a
//! new game is started (the new-game screen), and a server's are its own.
//!
//! `--settings display|graphics|effects|audio|controls|game` starts with the screen up on that tab (pictures).

use bevy::prelude::*;
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode};

use crate::graphics::{Graphics, MAX_VEGETATION, Preset};
use crate::film::{OldLook, OldLookNow};
use crate::help_text::{self, HelpText};
use crate::menu_skin::{self as skin, BONE, DIM, Look};
use crate::theme::{self, BODY, SMALL, font};
use crate::ui_scale::{self, ClientSettings, SettingsStore};

/// (As wide inside the sheet's ragged edge.)
const WIDTH: f32 = 1180.0;
const TABS_W: f32 = 190.0;
/// (Wide enough that a setting's line about itself stays one line.)
const LABEL_W: f32 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    /// The window: full screen or not, its size, vsync, the UI's scale.
    #[default]
    Display,
    Graphics,
    /// The film's effects over the picture: a look, apart from what costs frame rate.
    Effects,
    Audio,
    Controls,
    Game,
}

impl Tab {
    pub const ALL: [Tab; 6] = [Tab::Display, Tab::Graphics, Tab::Effects, Tab::Audio, Tab::Controls, Tab::Game];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Display => "Display",
            Tab::Graphics => "Graphics",
            Tab::Effects => "Effects",
            Tab::Audio => "Audio",
            Tab::Controls => "Controls",
            Tab::Game => "Game",
        }
    }

    pub fn parse(text: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| t.name().eq_ignore_ascii_case(text.trim()))
    }
}

/// A setting that is on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    Bloom,
    Grain,
    TiltShift,
    Vignette,
    ContactShadows,
    Occlusion,
    SoftShadows,
    Haze,
    Sky,
    SkyLight,
    LightShafts,
    GroundDetail,
    Fullscreen,
    Vsync,
    Muted,
    FirstPerson,
    ZombieSilhouettes,
    OpeningHints,
    SkipMenu,
    SendReports,
}

/// One change to the settings, as a control makes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    Preset(Preset),
    /// Steps of 5%: -1 or 1.
    Vegetation(i32),
    ShadowMap(u32),
    Msaa(u32),
    RenderScale(i32),
    Set(Switch, bool),
    /// Steps of 10%.
    Volume(i32),
    Music(i32),
    /// The window's size, a step down or up the list (`window_mode::SIZES`).
    WindowSize(i32),
    /// The 3D resolution in full screen, as a share of the screen (`render_scale::choices`).
    ScreenRes(f32),
    /// Steps of `ui_scale::STEP`.
    UiScale(i32),
    /// How much the UI explains (`help_text.rs`).
    HelpText(HelpText),
    /// The picture as a faded print (`film.rs`).
    OldLook(OldLook),
    /// The frame-rate read-out (`perf_monitor.rs`).
    PerfMonitor(crate::perf_monitor::PerfMonitor),
    /// Keyboard and mouse, the controller, or whichever was touched last (`pad.rs`).
    Input(crate::pad::InputSetting),
    /// Everything on this tab as the game ships it.
    Defaults(Tab),
}

/// A value moved `steps` steps of `step` percent, kept on the step grid and within `lo..=hi` percent.
fn stepped(value: f32, steps: i32, step: i32, lo: i32, hi: i32) -> f32 {
    let pct = (value * 100.0).round() as i32;
    let next = if steps > 0 { (pct.div_euclid(step) + steps) * step } else { (-((-pct).div_euclid(step)) + steps) * step };
    next.clamp(lo, hi) as f32 / 100.0
}

/// Make `c` in `s`. A graphics setting changed by itself makes the preset Custom.
pub fn change(s: &mut ClientSettings, c: Change) {
    let before = s.graphics.clone();
    match c {
        Change::Preset(p) => s.graphics = Graphics::of(p),
        Change::Vegetation(n) => s.graphics.vegetation = stepped(s.graphics.vegetation, n, 5, 0, (MAX_VEGETATION * 100.0) as i32),
        Change::ShadowMap(n) => s.graphics.shadow_map = n,
        Change::Msaa(n) => s.graphics.msaa = n,
        Change::RenderScale(n) => s.graphics.render_scale = stepped(s.graphics.render_scale, n, 5, 50, 100),
        Change::Set(switch, on) => *switch_of(s, switch) = on,
        Change::Volume(n) => s.volume = stepped(s.volume, n, 10, 0, 200),
        Change::Music(n) => s.music = stepped(s.music, n, 10, 0, 200),
        Change::WindowSize(n) => {
            if let Some(size) = crate::window_mode::step_size(s.window_size.unwrap_or(crate::window_mode::DEFAULT_SIZE), n > 0) {
                s.window_size = Some(size);
            }
        }
        Change::ScreenRes(share) => s.fullscreen_scale = Some(share),
        Change::UiScale(n) => s.ui_scale = ui_scale::step(s.ui_scale, n > 0),
        Change::HelpText(h) => s.help_text = h,
        Change::OldLook(o) => s.old_look = o,
        Change::PerfMonitor(m) => s.perf_monitor = m,
        Change::Input(i) => s.input = i,
        Change::Defaults(tab) => {
            let d = ClientSettings::default();
            match tab {
                Tab::Display => {
                    (s.fullscreen, s.vsync, s.window_size, s.ui_scale) = (d.fullscreen, d.vsync, d.window_size, d.ui_scale);
                    s.fullscreen_scale = d.fullscreen_scale;
                }
                Tab::Graphics => s.graphics = d.graphics,
                Tab::Effects => {
                    let g = &mut s.graphics;
                    let dg = &d.graphics;
                    (g.bloom, g.grain, g.tilt_shift, g.vignette, g.haze) = (dg.bloom, dg.grain, dg.tilt_shift, dg.vignette, dg.haze);
                    (g.sky, g.sky_light, g.light_shafts, g.ground_detail) = (dg.sky, dg.sky_light, dg.light_shafts, dg.ground_detail);
                    s.old_look = d.old_look;
                }
                Tab::Audio => (s.volume, s.music, s.muted) = (d.volume, d.music, d.muted),
                Tab::Game => {
                    (s.first_person, s.zombie_silhouettes, s.opening_hints, s.skip_menu, s.help_text, s.send_reports) = (d.first_person, d.zombie_silhouettes, d.opening_hints, d.skip_menu, d.help_text, d.send_reports);
                    s.perf_monitor = d.perf_monitor;
                }
                Tab::Controls => {
                    s.keys.clear();
                    s.pad.clear();
                    s.input = d.input;
                }
            }
        }
    }
    s.graphics = s.graphics.clone().clamped();
    if s.graphics != before && !matches!(c, Change::Preset(_) | Change::Defaults(Tab::Graphics)) {
        s.graphics.preset = Preset::Custom;
    }
}

fn switch_of(s: &mut ClientSettings, switch: Switch) -> &mut bool {
    match switch {
        Switch::Bloom => &mut s.graphics.bloom,
        Switch::Grain => &mut s.graphics.grain,
        Switch::TiltShift => &mut s.graphics.tilt_shift,
        Switch::Vignette => &mut s.graphics.vignette,
        Switch::ContactShadows => &mut s.graphics.contact_shadows,
        Switch::Occlusion => &mut s.graphics.occlusion,
        Switch::SoftShadows => &mut s.graphics.soft_shadows,
        Switch::Haze => &mut s.graphics.haze,
        Switch::Sky => &mut s.graphics.sky,
        Switch::SkyLight => &mut s.graphics.sky_light,
        Switch::LightShafts => &mut s.graphics.light_shafts,
        Switch::GroundDetail => &mut s.graphics.ground_detail,
        Switch::Fullscreen => &mut s.fullscreen,
        Switch::Vsync => &mut s.vsync,
        Switch::Muted => &mut s.muted,
        Switch::FirstPerson => &mut s.first_person,
        Switch::ZombieSilhouettes => &mut s.zombie_silhouettes,
        Switch::OpeningHints => &mut s.opening_hints,
        Switch::SkipMenu => &mut s.skip_menu,
        Switch::SendReports => &mut s.send_reports,
    }
}

/// What a row offers.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    /// A few named choices, the one in force marked.
    Choice(Vec<(String, bool, Change)>),
    /// A value, and the changes that step it down and up (`None` at an end).
    Stepper { value: String, down: Option<Change>, up: Option<Change> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: &'static str,
    /// One line on what it does.
    pub about: &'static str,
    pub control: Control,
}

fn switch(label: &'static str, about: &'static str, s: &ClientSettings, which: Switch) -> Row {
    let on = *switch_of(&mut s.clone(), which);
    Row { label, about, control: Control::Choice(vec![("Off".into(), !on, Change::Set(which, false)), ("On".into(), on, Change::Set(which, true))]) }
}

fn stepper(label: &'static str, about: &'static str, value: f32, lo: f32, hi: f32, make: fn(i32) -> Change) -> Row {
    let pct = (value * 100.0).round();
    Row { label, about, control: Control::Stepper { value: format!("{pct:.0}%"), down: (value > lo + 1e-3).then(|| make(-1)), up: (value < hi - 1e-3).then(|| make(1)) } }
}

/// Full screen's Resolution row: the 3D resolutions for a screen of `screen` pixels (not known
/// until a full-screen window has been seen), Auto marked. The window stays the screen's own size
/// and the UI its own sharpness; only the world is drawn smaller and scaled up.
pub(crate) fn screen_resolution(s: &ClientSettings, screen: Option<UVec2>) -> Row {
    use crate::render_scale::{auto_scale, choices, size_at};
    let about = "The size the world is drawn at, scaled up to fill the screen; the UI stays sharp. Auto follows the graphics preset: full size at High and Ultra, 1080 lines at Medium, 900 at Low.";
    let Some(screen) = screen else {
        return Row { label: "Resolution", about, control: Control::Stepper { value: "Auto".into(), down: None, up: None } };
    };
    let auto = auto_scale(screen, s.graphics.preset);
    let now = s.fullscreen_scale.unwrap_or(auto);
    let list = choices(screen);
    let at = list.iter().position(|c| (c - now).abs() < 0.01).unwrap_or_else(|| list.iter().rposition(|c| *c <= now).unwrap_or(0));
    let size = size_at(screen, list.get(at).copied().unwrap_or(now));
    let value = if s.fullscreen_scale.is_none() { format!("Auto ({} x {})", size.x, size.y) } else { format!("{} x {}", size.x, size.y) };
    Row {
        label: "Resolution",
        about,
        control: Control::Stepper { value, down: at.checked_sub(1).map(|i| Change::ScreenRes(list[i])), up: list.get(at + 1).map(|c| Change::ScreenRes(*c)) },
    }
}

/// A tab's rows, from the settings as they are. (Controls has none: it is the key list, [`KEYS`].)
pub fn rows(tab: Tab, s: &ClientSettings) -> Vec<Row> {
    let g = &s.graphics;
    match tab {
        Tab::Display => {
            let size = s.window_size.unwrap_or(crate::window_mode::DEFAULT_SIZE);
            let step = |up: bool| crate::window_mode::step_size(size, up).map(|_| Change::WindowSize(if up { 1 } else { -1 }));
            vec![
                switch("Fullscreen", "The window fills the screen, without a border. Alt+Enter or F11 switches.", s, Switch::Fullscreen),
                if s.fullscreen {
                    screen_resolution(s, crate::render_scale::screen())
                } else {
                    Row {
                        label: "Resolution",
                        about: "The window's size. One bigger than the screen is brought down to fit it.",
                        control: Control::Stepper { value: format!("{} x {}", size.0, size.1), down: step(false), up: step(true) },
                    }
                },
                switch("VSync", "Frames wait for the screen's refresh: no tearing.", s, Switch::Vsync),
                Row {
                    label: "UI scale",
                    about: "The size of every panel and text. Ctrl and + or - while you play.",
                    control: Control::Stepper {
                        value: format!("{}%", s.ui_scale),
                        down: (s.ui_scale > ui_scale::MIN_PCT).then_some(Change::UiScale(-1)),
                        up: (s.ui_scale < ui_scale::MAX_PCT).then_some(Change::UiScale(1)),
                    },
                },
            ]
        }
        Tab::Graphics => vec![
            Row {
                label: "Preset",
                about: crate::graphics_ui::describe(g.preset),
                control: Control::Choice(Preset::CHOICES.into_iter().map(|p| (p.name().to_string(), p == g.preset, Change::Preset(p))).collect()),
            },
            stepper("3D resolution", "The world's share of a Retina screen's pixels. The UI stays sharp.", g.render_scale, 0.5, 1.0, Change::RenderScale),
            stepper("Grass and flowers", "How dense the ground cover is. 0% is none.", g.vegetation, 0.0, MAX_VEGETATION, Change::Vegetation),
            Row {
                label: "Shadow detail",
                about: "The size of the sun's shadow map.",
                control: Control::Choice([1024u32, 2048, 4096, 8192].into_iter().map(|n| (n.to_string(), n == g.shadow_map, Change::ShadowMap(n))).collect()),
            },
            Row {
                label: "Anti-aliasing",
                about: "Samples a pixel. Window glass looks best at 4x.",
                control: Control::Choice([(1u32, "Off"), (2, "2x"), (4, "4x")].into_iter().map(|(n, name)| (name.to_string(), n == g.msaa, Change::Msaa(n))).collect()),
            },
            switch("Soft shadows", "Shadow edges that soften away from what casts them. Costs frame rate.", s, Switch::SoftShadows),
            switch("Contact shadows", "A soft dark line where walls meet the ground. Changes on the next start.", s, Switch::ContactShadows),
            switch("Ambient occlusion", "Soft shade in corners, under cars and where things meet the ground. Turns anti-aliasing to SMAA.", s, Switch::Occlusion),
        ],
        Tab::Effects => vec![
            Row {
                label: "Old look",
                about: match s.old_look {
                    OldLook::Off => "The picture as a faded old print: washed-out blacks, cream whites, more grain.",
                    OldLook::Subtle => "A light fade: warm blacks, cream whites, a little more grain.",
                    OldLook::Strong => "A heavy fade, like a print left in the sun for years.",
                },
                control: Control::Choice(OldLook::ALL.into_iter().map(|o| (o.name().to_string(), o == s.old_look, Change::OldLook(o))).collect()),
            },
            switch("Bloom", "The glow round lamps, fires and lit windows.", s, Switch::Bloom),
            switch("Film grain", "A little noise over the picture.", s, Switch::Grain),
            switch("Tilt-shift", "Soft focus toward the top and bottom of the screen.", s, Switch::TiltShift),
            switch("Vignette", "Darkened corners.", s, Switch::Vignette),
            switch("Haze", "A thin haze over the distance, thick and warm when the sun is low.", s, Switch::Haze),
            switch("Sky", "In first person, a sky that follows the hour, with the sun, moon and stars.", s, Switch::Sky),
            switch("Sky light", "The shade lit by the sky: bluer facing up, warmer facing down. Changes on the next start.", s, Switch::SkyLight),
            switch("Light shafts", "Sunlight scattering in the air. Experimental, and costs a lot of frame rate.", s, Switch::LightShafts),
            switch("Ground detail", "Patchy colour on the ground. Changes on the next start.", s, Switch::GroundDetail),
        ],
        Tab::Audio => vec![
            switch("Mute", "All sound off. Ctrl+M does the same while you play.", s, Switch::Muted),
            stepper("Master volume", "Everything you hear.", s.volume, 0.0, 2.0, Change::Volume),
            stepper("Music", "The music, under the master volume.", s.music, 0.0, 2.0, Change::Music),
        ],
        Tab::Controls => Vec::new(),
        Tab::Game => vec![
            Row {
                label: "View",
                about: "The view a game starts in. The ; key switches while you play.",
                control: Control::Choice(vec![("From above".into(), !s.first_person, Change::Set(Switch::FirstPerson, false)), ("First person".into(), s.first_person, Change::Set(Switch::FirstPerson, true))]),
            },
            Row {
                label: "Help text",
                about: match s.help_text {
                    HelpText::Full => "Cards and panels explain what things are for and how they work.",
                    HelpText::Short => "Cards keep names, numbers and what's needed. The explaining lines go.",
                    HelpText::Off => "As Short, and the reminders of what keys and clicks do go too.",
                },
                control: Control::Choice(HelpText::ALL.into_iter().map(|h| (h.name().to_string(), h == s.help_text, Change::HelpText(h))).collect()),
            },
            Row {
                label: "Performance monitor",
                about: match s.perf_monitor {
                    crate::perf_monitor::PerfMonitor::Off => "Shows the frame rate in the top right corner while you play.",
                    crate::perf_monitor::PerfMonitor::Fps => "The frame rate, in the top right corner.",
                    crate::perf_monitor::PerfMonitor::Full => "The frame rate, frame times, the slowest 1% of frames, the preset and the 3D resolution.",
                },
                control: Control::Choice(crate::perf_monitor::PerfMonitor::ALL.into_iter().map(|m| (m.name().to_string(), m == s.perf_monitor, Change::PerfMonitor(m))).collect()),
            },
            switch("Opening hints", "Outlines of the doors and windows a building hides from the camera.", s, Switch::OpeningHints),
            switch("Zombie silhouettes", "Zombies you can see show through what covers them from the camera.", s, Switch::ZombieSilhouettes),
            switch("Skip the main menu", "Start straight in the game, as before there was a menu. For development.", s, Switch::SkipMenu),
            switch("Send problem reports", "Crashes, F12 reports and bad stutters go to the developers, with no name or address.", s, Switch::SendReports),
        ],
    }
}

/// A line of the Controls tab: an action whose keys can be changed (`keys.rs`), the menu of the
/// thing faced (holding the interact key), or a key that stays as it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyRow {
    Bound(Action, &'static str),
    Hold(Action, &'static str),
    Fixed(&'static str, &'static str),
}

impl KeyRow {
    /// The key it shows, as bound now.
    pub fn key(self) -> String {
        match self {
            KeyRow::Bound(a, _) => crate::keys::label(a),
            KeyRow::Hold(a, _) => format!("Hold {}", crate::keys::short(a)),
            KeyRow::Fixed(k, _) => k.to_string(),
        }
    }

    pub fn does(self) -> &'static str {
        match self {
            KeyRow::Bound(_, d) | KeyRow::Hold(_, d) | KeyRow::Fixed(_, d) => d,
        }
    }
}

use crate::keys::Action;

/// The keys, by what they are for. The bound ones are changed by clicking them.
pub const KEYS: [(&str, &[KeyRow]); 6] = [
    (
        "Moving",
        &[
            KeyRow::Bound(Action::MoveUp, "Walk up the screen"),
            KeyRow::Bound(Action::MoveDown, "Walk down"),
            KeyRow::Bound(Action::MoveLeft, "Walk left"),
            KeyRow::Bound(Action::MoveRight, "Walk right"),
            KeyRow::Bound(Action::Run, "Run"),
            KeyRow::Bound(Action::Sneak, "Sneak"),
            KeyRow::Bound(Action::Sleep, "Sleep, and get up"),
            KeyRow::Bound(Action::Sit, "Sit down, and get up"),
        ],
    ),
    (
        "Fighting",
        &[
            KeyRow::Fixed("Right mouse", "Aim"),
            KeyRow::Fixed("Left mouse", "Attack or shoot, while aiming"),
            KeyRow::Bound(Action::Shove, "Shove, or stomp one that's down"),
            KeyRow::Bound(Action::Reload, "Reload"),
        ],
    ),
    (
        "The world",
        &[
            KeyRow::Bound(Action::Interact, "Open, climb, get in, pick up or search what you face"),
            KeyRow::Hold(Action::Interact, "Everything you can do with what you face"),
            KeyRow::Fixed("Right-click", "Everything you can do with what's under the pointer"),
            KeyRow::Bound(Action::Flashlight, "Flashlight or candle on and off"),
            KeyRow::Bound(Action::Smash, "Smash a window"),
            KeyRow::Bound(Action::Refuel, "Pour fuel into a car, or siphon it out"),
            KeyRow::Bound(Action::Horn, "Horn, at the wheel"),
            KeyRow::Bound(Action::Radio, "Car radio: the next station, or off"),
            KeyRow::Fixed("1 - 9", "Hotbar"),
            KeyRow::Bound(Action::Stop, "Stop what you're doing"),
        ],
    ),
    ("Camera", &[KeyRow::Fixed(", and .", "Turn a quarter"), KeyRow::Fixed(";", "First person and back"), KeyRow::Fixed("Wheel", "Zoom"), KeyRow::Fixed("F10", "Walls up or cut away")]),
    (
        "Screens",
        &[
            KeyRow::Bound(Action::Inventory, "Inventory"),
            KeyRow::Bound(Action::Health, "Health"),
            KeyRow::Bound(Action::Skills, "Skills"),
            KeyRow::Bound(Action::Make, "Make"),
            KeyRow::Bound(Action::Build, "Build"),
            KeyRow::Bound(Action::Map, "Map"),
            KeyRow::Bound(Action::Objectives, "Objectives and hints"),
            KeyRow::Fixed("Esc", "Pause menu"),
        ],
    ),
    (
        "Other",
        &[
            KeyRow::Fixed("Enter", "Chat"),
            KeyRow::Fixed("V  U  P", "Wave, hands up, point"),
            KeyRow::Fixed("Ctrl + M", "Mute"),
            KeyRow::Fixed("F5 / F9", "Quicksave / quickload"),
            KeyRow::Fixed("F7", "Server browser"),
            KeyRow::Fixed("Ctrl + / -", "UI scale"),
        ],
    ),
];

/// The screen's state. Whoever shows it (the pause menu, the main menu) opens and shuts it.
#[derive(Resource, Default)]
pub struct SettingsUi {
    pub open: bool,
    pub tab: Tab,
    /// What clicks asked for, done by [`act`].
    acts: Vec<Act>,
    /// The panel shows this (rebuilt when it changes).
    shown: Option<(bool, Tab, u32)>,
    /// Counts every change made here, so [`apply_live`] sees them.
    pub changes: u32,
    /// Drawn in the game (which puts changes into effect at once) or in the main menu.
    pub in_game: bool,
    /// Waiting for the key to bind to this action (the Controls tab: [`capture`]).
    pub capture: Option<Action>,
    /// The Controls tab shows the controller's buttons (`None`: whichever is in use).
    pub pad_view: Option<bool>,
}

impl SettingsUi {
    pub fn close(&mut self) {
        self.open = false;
    }

    /// The tab `by` along from this one, round the end (the pad's bumpers: `focus.rs`).
    pub fn step_tab(&mut self, by: i32) {
        let n = Tab::ALL.len() as i32;
        let at = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0) as i32;
        self.acts.push(Act::Tab(Tab::ALL[(at + by).rem_euclid(n) as usize]));
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    Tab(Tab),
    Change(Change),
    /// Wait for the next key, for this action.
    Rebind(Action),
    /// The Controls tab's list: the controller's (true) or the keyboard's.
    PadView(bool),
    Close,
}

pub struct SettingsUiPlugin;

impl Plugin for SettingsUiPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().get_resource::<crate::Options>().cloned();
        // (A flag's level for the session stays until the setting itself is changed here.)
        let settings = SettingsStore::ensure(app);
        // The keys: the data folder's keys.ron with the player's picks over it.
        crate::keys::init(options.as_ref().map(|o| o.data.as_path()), &settings.keys);
        help_text::set(options.as_ref().and_then(|o| o.help_text).unwrap_or(settings.help_text));
        let tab = options.as_ref().and_then(|o| o.settings.as_deref()).map(|t| Tab::parse(t).unwrap_or_default());
        let in_game = app.world().contains_resource::<crate::server_link::ServerLink>();
        app.insert_resource(SettingsUi { open: tab.is_some(), tab: tab.unwrap_or_default(), in_game, ..default() }).add_systems(Startup, spawn).add_systems(PreUpdate, capture.after(bevy::input::InputSystems)).add_systems(Update, ((act, apply_live, draw).chain(), scroll));
    }
}

#[derive(Component)]
struct Root;
#[derive(Component)]
struct Panel;
/// The open tab's settings, which scroll when the window is too short for them.
#[derive(Component)]
struct Scroll;

/// The wheel over the settings scrolls them.
fn scroll(mut wheel: MessageReader<bevy::input::mouse::MouseWheel>, ui: Res<SettingsUi>, window: Query<&Window, With<bevy::window::PrimaryWindow>>, mut scroll: Query<(&ComputedNode, &UiGlobalTransform, &mut ScrollPosition), With<Scroll>>) {
    use bevy::input::mouse::MouseScrollUnit;
    let px = wheel.read().map(|ev| if ev.unit == MouseScrollUnit::Line { ev.y * 40.0 } else { ev.y }).fold(0.0, |a, b| a + b);
    if px == 0.0 || !ui.open {
        return;
    }
    let Some(cursor) = window.single().ok().and_then(|w| w.physical_cursor_position()) else { return };
    for (node, at, mut pos) in &mut scroll {
        if node.contains_point(*at, cursor) {
            // Physical pixels to UI pixels, as `ScrollPosition` is.
            let max = (node.content_size.y - node.size.y) * node.inverse_scale_factor();
            let y = crate::hud::scrolled(pos.y, px, max);
            if pos.y != y {
                pos.y = y;
            }
        }
    }
}

fn spawn(mut commands: Commands, ui: Res<SettingsUi>) {
    // Over the whole screen, so nothing under it takes a click meant for it. In a game the world shows
    // round the panel (a graphics change is seen at once); before one, the menu behind is covered.
    commands
        .spawn((
            Root,
            Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), justify_content: JustifyContent::Center, align_items: AlignItems::Center, display: Display::None, ..default() },
            BackgroundColor(if ui.in_game { skin::VEIL } else { skin::GROUND }),
            GlobalZIndex(31),
        ))
        .with_children(|c| {
            // Before a game, the menu's printed ground round the panel.
            if !ui.in_game {
                skin::ground(c);
            }
            c.spawn((Panel, Node { width: px(WIDTH), max_height: percent(96), flex_direction: FlexDirection::Column, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(8)), ..default() }, skin::panel()));
        });
}

fn act(mut ui: ResMut<SettingsUi>, mut store: ResMut<SettingsStore>) {
    for a in std::mem::take(&mut ui.acts) {
        match a {
            Act::Tab(t) => ui.tab = t,
            Act::Close => {
                ui.open = false;
                ui.capture = None;
            }
            Act::Rebind(a) => {
                ui.capture = if ui.capture == Some(a) { None } else { Some(a) };
                ui.changes += 1;
            }
            Act::PadView(on) => {
                ui.pad_view = Some(on);
                ui.capture = None;
                ui.changes += 1;
            }
            Act::Change(c) => {
                if let Err(e) = store.change(|s| change(s, c)) {
                    eprintln!("settings not saved: {e}");
                }
                crate::keys::set_overrides(&store.settings.keys);
                crate::pad::set_overrides(&store.settings.pad);
                ui.changes += 1;
            }
        }
    }
}

/// The window's mode for the fullscreen setting.
pub fn window_mode(fullscreen: bool) -> WindowMode {
    if fullscreen { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed }
}

/// Put what the screen changed into effect where the running game keeps it: only the settings
/// that differ from the last time, so a flag's or a key's choice for this session (`--volume`,
/// the `;` view) stays until its own setting is changed here.
#[allow(clippy::too_many_arguments)]
fn apply_live(
    ui: Res<SettingsUi>,
    store: Res<SettingsStore>,
    options: Option<Res<crate::Options>>,
    mut last: Local<Option<(u32, ClientSettings)>>,
    graphics: Option<ResMut<Graphics>>,
    auto: Option<ResMut<crate::graphics_ui::AutoMeasure>>,
    audio: Option<ResMut<crate::audio::AudioSettings>>,
    music: Option<ResMut<crate::music::Music>>,
    scale: Option<ResMut<ui_scale::UiScalePct>>,
    silhouettes: Option<ResMut<crate::silhouette::ZombieSilhouettes>>,
    hints: Option<ResMut<crate::opening_hints::OpeningHints>>,
    old_look: Option<ResMut<OldLookNow>>,
    mut cams: Query<(&mut crate::camera::IsoCamera, &mut Projection)>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    monitors: Query<&bevy::window::Monitor>,
) {
    let (seen, was) = last.get_or_insert_with(|| (ui.changes, store.settings.clone()));
    if *seen == ui.changes {
        // Changes made elsewhere (a key that saves its setting) are already in effect.
        if *was != store.settings {
            *was = store.settings.clone();
        }
        return;
    }
    let now = &store.settings;
    if was.graphics != now.graphics
        && let Some(mut g) = graphics
    {
        *g = now.graphics.clone();
        if let Some(mut auto) = auto {
            *auto = default();
        }
    }
    if let Some(mut a) = audio {
        if was.volume != now.volume {
            a.volume = now.volume;
        }
        if was.muted != now.muted {
            a.muted = now.muted;
        }
    }
    if was.music != now.music
        && let Some(mut m) = music
    {
        m.level = now.music;
    }
    if was.ui_scale != now.ui_scale
        && let Some(mut s) = scale
    {
        s.pct = now.ui_scale;
    }
    if was.zombie_silhouettes != now.zombie_silhouettes
        && let Some(mut z) = silhouettes
    {
        z.0 = now.zombie_silhouettes;
    }
    if was.help_text != now.help_text {
        help_text::set(now.help_text);
    }
    if was.old_look != now.old_look
        && let Some(mut o) = old_look
    {
        o.0 = now.old_look;
    }
    if was.opening_hints != now.opening_hints
        && let Some(mut h) = hints
    {
        h.on = now.opening_hints;
    }
    if was.first_person != now.first_person {
        for (mut cam, mut projection) in &mut cams {
            if cam.shoulder() != now.first_person {
                cam.toggle_view();
                *projection = cam.projection();
            }
        }
    }
    if (was.fullscreen, was.vsync) != (now.fullscreen, now.vsync)
        && let Ok(mut window) = windows.single_mut()
    {
        let sized = options.as_ref().is_some_and(|o| o.size_given);
        let mode = window_mode(now.fullscreen && !sized);
        if window.mode != mode {
            window.mode = mode;
        }
        let present = if now.vsync && !options.as_ref().is_some_and(|o| o.novsync) { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
        if window.present_mode != present {
            window.present_mode = present;
        }
    }
    // A size picked here: the window takes it now, held to its screen as at a start.
    if was.window_size != now.window_size
        && !options.as_ref().is_some_and(|o| o.size_given)
        && let Some(size) = now.window_size
        && let Ok(mut window) = windows.single_mut()
        && window.mode == WindowMode::Windowed
    {
        let mut size = (size.0 as f32, size.1 as f32);
        if let Some(m) = monitors.iter().next() {
            let scale = if m.scale_factor > 0.0 { m.scale_factor as f32 } else { 1.0 };
            size = crate::window_mode::fit(size, (m.physical_width as f32 / scale, m.physical_height as f32 / scale));
        }
        if (window.resolution.width(), window.resolution.height()) != size {
            window.resolution.set(size.0, size.1);
        }
    }
    (*seen, *was) = (ui.changes, now.clone());
}

/// A chip that does `act` when pressed; with none (a value at its end) it is there, but faint.
fn chip(parent: &mut ChildSpawnerCommands, label: &str, on: bool, act: Option<Act>, width: Val) {
    let look = if act.is_some() { Look::Chip(on) } else { Look::Disabled };
    let mut b = skin::button(parent, label, None, look, width);
    if let Some(act) = act {
        b.observe(move |_: On<PointerPress>, mut ui: ResMut<SettingsUi>| ui.acts.push(act));
    }
}

fn draw(mut commands: Commands, mut ui: ResMut<SettingsUi>, store: Res<SettingsStore>, mode: Option<Res<crate::pad::InputMode>>, mut root: Query<&mut Node, With<Root>>, panel: Query<Entity, With<Panel>>) {
    // The Controls tab opens on the list of what is in use; the controller's buttons by its maker's names.
    let (in_use, family, input) = mode.map_or((false, crate::pad::Family::Xbox, store.settings.input), |m| (m.pad, m.family, m.setting));
    let pad_view = ui.pad_view.unwrap_or(in_use);
    let want = (ui.open, ui.tab, ui.changes + ((pad_view as u32) << 30) + ((family as u32) << 28) + ((input as u32) << 26) + ((in_use as u32) << 25));
    if ui.shown == Some(want) {
        return;
    }
    ui.shown = Some(want);
    let (Ok(mut node), Ok(panel)) = (root.single_mut(), panel.single()) else { return };
    node.display = if ui.open { Display::Flex } else { Display::None };
    commands.entity(panel).despawn_related::<Children>();
    if !ui.open {
        return;
    }
    let (tab, in_game) = (ui.tab, ui.in_game);
    let s = store.settings.clone();
    commands.entity(panel).with_children(|p| {
        // The top bar: what this is, and the way out.
        p.spawn(Node { height: px(58), padding: UiRect::horizontal(px(24)), align_items: AlignItems::Center, column_gap: px(16), flex_shrink: 0.0, ..default() }).with_children(|bar| {
            skin::title(bar, "Settings");
            skin::stripes(bar, 44.0);
            bar.spawn((Text::new("Kept as you change them."), font(SMALL), TextColor(DIM), TextLayout::no_wrap()));
            skin::spacer(bar);
            skin::button(bar, "Close", None, Look::Plain, Val::Auto).observe(|_: On<PointerPress>, mut ui: ResMut<SettingsUi>| ui.acts.push(Act::Close));
        });
        skin::rule(p);
        // The body: the tabs down the left, the open one's settings beside them.
        // (It scrolls when the window is too short for it: the wheel, or the pad's focus moving.)
        p.spawn(Node { align_items: AlignItems::Stretch, min_height: px(0), flex_shrink: 1.0, ..default() }).with_children(|body| {
            body.spawn(Node { width: px(TABS_W), flex_shrink: 0.0, flex_direction: FlexDirection::Column, row_gap: px(8), padding: UiRect::all(px(20)), ..default() }).with_children(|col| {
                for t in Tab::ALL {
                    chip(col, t.name(), t == tab, Some(Act::Tab(t)), percent(100));
                }
            });
            skin::divider(body);
            // The key list is the tallest thing here: it keeps tight margins so it fits 1280 x 720.
            let (pad, gap) = if tab == Tab::Controls { (12, 0) } else { (20, 8) };
            body.spawn((Scroll, ScrollPosition::default(), Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, padding: UiRect::axes(px(28), px(pad)), row_gap: px(gap), min_height: px(0), overflow: Overflow::scroll_y(), ..default() })).with_children(|col| {
                if tab == Tab::Controls {
                    controls_head(col, input, pad_view);
                    if pad_view {
                        pad_list(col, family);
                    } else {
                        keys_list(col, ui.capture);
                    }
                }
                for r in rows(tab, &s) {
                    col.spawn(Node { align_items: AlignItems::Center, column_gap: px(24), min_height: px(44), flex_shrink: 0.0, ..default() }).with_children(|row| {
                        row.spawn(Node { width: px(LABEL_W), flex_shrink: 0.0, flex_direction: FlexDirection::Column, row_gap: px(2), ..default() }).with_children(|l| {
                            l.spawn((Text::new(r.label), theme::strong(BODY), TextColor(BONE)));
                            l.spawn((Text::new(r.about), font(SMALL), TextColor(DIM)));
                        });
                        row.spawn(Node { align_items: AlignItems::Center, column_gap: px(8), ..default() }).with_children(|c| match &r.control {
                            Control::Choice(choices) => {
                                for (name, on, change) in choices {
                                    chip(c, name, *on, Some(Act::Change(*change)), Val::Auto);
                                }
                            }
                            Control::Stepper { value, down, up } => {
                                chip(c, "-", false, down.map(Act::Change), Val::Auto);
                                c.spawn(Node { min_width: px(64), justify_content: JustifyContent::Center, ..default() }).with_children(|v| {
                                    v.spawn((Text::new(value.clone()), theme::strong(BODY), TextColor(BONE), TextLayout::no_wrap()));
                                });
                                chip(c, "+", false, up.map(Act::Change), Val::Auto);
                            }
                        });
                    });
                }
                let note = match tab {
                    Tab::Display => "Kept between sessions. For more frame rate in full screen, pick a smaller Resolution: the UI stays sharp.",
                    Tab::Graphics if in_game => "Changes show at once round this screen.",
                    Tab::Graphics => "Auto measures the first seconds of a game and picks what this machine holds.",
                    Tab::Effects => "The look over the picture. No preset turns the old look on or off.",
                    Tab::Audio => "Flags on the command line win for their session.",
                    Tab::Controls if ui.capture.is_some() => "Press the new key (with Shift, Ctrl or Alt if you like). Esc keeps the old one.",
                    Tab::Controls if pad_view => "The buttons are in pad.ron; choosing them here comes later. Auto plays with what you touched last: a button or stick, a key or a click.",
                    Tab::Controls => "Click a key to change it; the game's own keys are in keys.ron. Auto plays with what you touched last.",
                    Tab::Game => "Difficulty and the world's own options are chosen when you start a new game.",
                };
                col.spawn((Text::new(note), font(SMALL), TextColor(DIM), Node { margin: UiRect::top(px(if tab == Tab::Controls { 6 } else { 10 })), flex_shrink: 0.0, ..default() }));
            });
        });
        skin::rule(p);
        // The footer: the keys that work here.
        p.spawn(Node { height: px(50), padding: UiRect::horizontal(px(24)), align_items: AlignItems::Center, column_gap: px(18), flex_shrink: 0.0, ..default() }).with_children(|row| {
            skin::keycap(row, "Esc", if in_game { "back to the menu" } else { "back" });
            skin::spacer(row);
            {
                skin::button(row, &format!("{} defaults", tab.name()), None, Look::Plain, Val::Auto).observe(move |_: On<PointerPress>, mut ui: ResMut<SettingsUi>| ui.acts.push(Act::Change(Change::Defaults(tab))));
            }
        });
    });
}

/// The top of the Controls tab: which the game is played with, and which list is shown.
fn controls_head(p: &mut ChildSpawnerCommands, input: crate::pad::InputSetting, pad_view: bool) {
    // One row, so the keyboard's list below still fits a 900-pixel screen.
    p.spawn(Node { align_items: AlignItems::Center, column_gap: px(8), min_height: px(38), flex_shrink: 0.0, ..default() }).with_children(|row| {
        row.spawn((Text::new("Play with"), theme::strong(BODY), TextColor(BONE), Node { margin: UiRect::right(px(4)), ..default() }));
        for i in crate::pad::InputSetting::ALL {
            chip(row, i.name(), i == input, Some(Act::Change(Change::Input(i))), Val::Auto);
        }
        skin::spacer(row);
        row.spawn((Text::new("Show"), theme::strong(BODY), TextColor(BONE), Node { margin: UiRect::right(px(4)), ..default() }));
        chip(row, "Keys", !pad_view, Some(Act::PadView(false)), Val::Auto);
        chip(row, "Controller", pad_view, Some(Act::PadView(true)), Val::Auto);
    });
    skin::rule(p);
}

/// A line of the Controls tab's controller list: an action and its buttons (`pad.ron`), a button
/// that does one thing, or a stick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PadRow {
    Bound(crate::pad::PadAction, &'static str),
    Button(crate::pad::Button, &'static str),
    Fixed(&'static str, &'static str),
    /// An action's button, held.
    Held(crate::pad::PadAction, &'static str),
}

impl PadRow {
    /// The button it shows, by this pad's names.
    pub fn button(self, family: crate::pad::Family) -> String {
        match self {
            PadRow::Bound(a, _) => crate::pad::current().label(a, family),
            PadRow::Button(b, _) => b.label(family).to_string(),
            PadRow::Fixed(k, _) => k.to_string(),
            PadRow::Held(a, _) => format!("Hold {}", crate::pad::current().label(a, family)),
        }
    }

    pub fn does(self) -> &'static str {
        match self {
            PadRow::Bound(_, d) | PadRow::Button(_, d) | PadRow::Fixed(_, d) | PadRow::Held(_, d) => d,
        }
    }
}

use crate::pad::{Button as Pb, Only, PadAction as Pa};

/// The controller's buttons, by what they are for: playing, the screens and camera, at the wheel.
pub const PAD_ROWS: [(&str, &[PadRow]); 3] = [
    (
        "Play",
        &[
            PadRow::Fixed("Left stick", "Walk; part way over, slower"),
            PadRow::Bound(Pa::Key(Action::Run), "Run, until you stop"),
            PadRow::Bound(Pa::Key(Action::Sneak), "Sneak"),
            PadRow::Bound(Pa::Only(Only::Aim), "Aim, held; the right stick turns the aim"),
            PadRow::Bound(Pa::Only(Only::Attack), "Swing or shoot; aims by itself if you aren't"),
            PadRow::Bound(Pa::Key(Action::Shove), "Shove, or stomp one that's down"),
            PadRow::Bound(Pa::Key(Action::Reload), "Reload"),
            PadRow::Bound(Pa::Key(Action::Stop), "Stop what you're doing"),
            PadRow::Bound(Pa::Key(Action::Interact), "Open, shut, climb, get in, pick up, search"),
            PadRow::Held(Pa::Key(Action::Interact), "A wheel of everything you can do with what you face"),
            PadRow::Bound(Pa::Only(Only::ItemWheel), "Item wheel: point at one, let go to use it"),
            PadRow::Bound(Pa::Only(Only::MakeWheel), "Make wheel: point at one, let go to make it"),
            PadRow::Bound(Pa::Only(Only::Chat), "Say something, on the on-screen keyboard"),
            PadRow::Bound(Pa::Key(Action::Flashlight), "Flashlight or candle on and off"),
            PadRow::Bound(Pa::Key(Action::Sit), "Sit down, and get up"),
            PadRow::Bound(Pa::Key(Action::Sleep), "Sleep, and get up"),
        ],
    ),
    (
        "Screens and camera",
        &[
            PadRow::Bound(Pa::Key(Action::Inventory), "Inventory"),
            PadRow::Bound(Pa::Key(Action::Map), "Map"),
            PadRow::Bound(Pa::Only(Only::Pause), "Pause menu"),
            PadRow::Button(Pb::B, "Back, on a screen"),
            PadRow::Bound(Pa::Only(Only::TurnLeft), "Turn the camera left"),
            PadRow::Bound(Pa::Only(Only::TurnRight), "Turn the camera right"),
            PadRow::Bound(Pa::Only(Only::ZoomIn), "Zoom in, held"),
            PadRow::Bound(Pa::Only(Only::ZoomOut), "Zoom out, held"),
            PadRow::Bound(Pa::Only(Only::FirstPerson), "First person and back"),
            PadRow::Fixed("Right stick", "Look round, in first person"),
        ],
    ),
    (
        "At the wheel",
        &[
            PadRow::Button(Pb::RT, "Drive"),
            PadRow::Button(Pb::LT, "Brake, and reverse"),
            PadRow::Fixed("Left stick", "Steer"),
            PadRow::Bound(Pa::Key(Action::Shove), "Handbrake"),
            PadRow::Bound(Pa::Key(Action::Horn), "Horn"),
            PadRow::Bound(Pa::Key(Action::Interact), "Get out"),
        ],
    ),
];

/// The Controls tab's controller list: three columns, an action and its button a line.
fn pad_list(p: &mut ChildSpawnerCommands, family: crate::pad::Family) {
    p.spawn(Node { column_gap: px(22), align_items: AlignItems::FlexStart, flex_shrink: 0.0, ..default() }).with_children(|cols| {
        for (group, rows) in PAD_ROWS {
            cols.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), flex_basis: percent(33), ..default() }).with_children(|col| {
                col.spawn((Text::new(group.to_uppercase()), theme::head(BODY + 2.0), TextColor(skin::ACCENT), Node { margin: UiRect::top(px(6)), ..default() }));
                for row in rows.iter().copied() {
                    col.spawn(Node { column_gap: px(8), align_items: AlignItems::Center, min_height: px(28), ..default() }).with_children(|line| {
                        line.spawn(Node { width: px(104), flex_shrink: 0.0, ..default() }).with_children(|k| {
                            k.spawn((Text::new(row.button(family)), theme::strong(SMALL), TextColor(BONE), TextLayout::no_wrap()));
                        });
                        line.spawn((Text::new(row.does()), font(SMALL), TextColor(DIM)));
                    });
                }
            });
        }
    });
}

/// The Controls tab: the keys in three columns of groups. A bound key is a chip that waits for
/// the new one when clicked; one shared with another action says so.
fn keys_list(p: &mut ChildSpawnerCommands, capture: Option<Action>) {
    let bindings = crate::keys::current();
    p.spawn(Node { column_gap: px(22), align_items: AlignItems::FlexStart, flex_shrink: 0.0, ..default() }).with_children(|cols| {
        for pair in KEYS.chunks(2) {
            cols.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), flex_basis: percent(33), ..default() }).with_children(|col| {
                for (group, keys) in pair {
                    col.spawn((Text::new(group.to_uppercase()), theme::brand(BODY + 6.0), TextColor(skin::ACCENT), Node { margin: UiRect::top(px(6)), ..default() }));
                    for row in keys.iter().copied() {
                        col.spawn(Node { column_gap: px(8), align_items: AlignItems::Center, ..default() }).with_children(|line| {
                            line.spawn(Node { width: px(104), flex_shrink: 0.0, ..default() }).with_children(|k| match row {
                                KeyRow::Bound(a, _) => {
                                    let label = if capture == Some(a) { "Press a key".to_string() } else { row.key() };
                                    chip(k, &label, capture == Some(a), Some(Act::Rebind(a)), Val::Auto);
                                }
                                _ => {
                                    k.spawn((Text::new(row.key()), theme::strong(SMALL), TextColor(BONE), TextLayout::no_wrap()));
                                }
                            });
                            let clash = match row {
                                KeyRow::Bound(a, _) => bindings.clashes(a),
                                _ => Vec::new(),
                            };
                            let does = match clash.first() {
                                Some(o) => format!("{} (also {})", row.does(), KEYS.iter().flat_map(|g| g.1.iter()).find_map(|r| matches!(r, KeyRow::Bound(b, _) if b == o).then(|| r.does().to_lowercase())).unwrap_or_default()),
                                None => row.does().to_string(),
                            };
                            line.spawn((Text::new(does), font(SMALL), TextColor(if clash.is_empty() { DIM } else { theme::flat::RED_TEXT })));
                        });
                    }
                }
            });
        }
    });
}

/// Rebinding: while the Controls tab waits for a key, the next one pressed (with the modifiers
/// held) becomes the action's key, saved in the settings and in force at once. A modifier pressed
/// and let go alone is bound alone (Shift to run). Esc keeps the old key. Runs before the pause
/// menu's keys, and takes every key it sees, so none reaches the game or closes the screen.
pub fn capture(mut keys: ResMut<ButtonInput<KeyCode>>, mut ui: ResMut<SettingsUi>, mut store: ResMut<SettingsStore>, mut lone: Local<Option<KeyCode>>) {
    let Some(action) = ui.capture.filter(|_| ui.open && ui.tab == Tab::Controls) else {
        *lone = None;
        return;
    };
    let is_mod = |k: KeyCode| matches!(k, KeyCode::ShiftLeft | KeyCode::ShiftRight | KeyCode::ControlLeft | KeyCode::ControlRight | KeyCode::SuperLeft | KeyCode::SuperRight | KeyCode::AltLeft | KeyCode::AltRight);
    let mut picked: Option<String> = None;
    if keys.just_pressed(KeyCode::Escape) {
        ui.capture = None;
        ui.changes += 1;
    } else if let Some(k) = keys.get_just_pressed().copied().find(|k| !is_mod(*k)) {
        *lone = None;
        if let Some(name) = crate::keys::name_of(k) {
            let mut text = String::new();
            for (held, m) in [([KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::SuperLeft, KeyCode::SuperRight].as_slice(), "Ctrl"), (&[KeyCode::AltLeft, KeyCode::AltRight], "Alt"), (&[KeyCode::ShiftLeft, KeyCode::ShiftRight], "Shift")] {
                if keys.any_pressed(held.iter().copied()) {
                    text.push_str(m);
                    text.push('+');
                }
            }
            text.push_str(&name);
            picked = Some(text);
        }
    } else if let Some(k) = keys.get_just_pressed().copied().find(|k| is_mod(*k)) {
        *lone = Some(k);
    } else if let Some(k) = *lone
        && keys.just_released(k)
    {
        *lone = None;
        picked = crate::keys::name_of(k);
    }
    if let Some(text) = picked {
        if let Err(e) = store.change(|s| {
            s.keys.insert(action.id().to_string(), vec![text]);
        }) {
            eprintln!("settings not saved: {e}");
        }
        crate::keys::set_overrides(&store.settings.keys);
        ui.capture = None;
        ui.changes += 1;
    }
    keys.reset_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_control_changes_what_it_shows() {
        // Each choice a row offers, made, is then the one that row marks; each step moves its value.
        for tab in Tab::ALL {
            let start = ClientSettings::default();
            for (k, row) in rows(tab, &start).iter().enumerate() {
                match &row.control {
                    Control::Choice(choices) => {
                        assert_eq!(choices.iter().filter(|c| c.1).count(), 1, "{}: one choice in force", row.label);
                        for (n, (name, _, c)) in choices.iter().enumerate() {
                            let mut s = start.clone();
                            change(&mut s, *c);
                            let Control::Choice(after) = &rows(tab, &s)[k].control else { panic!() };
                            assert!(after[n].1, "{}: {name} isn't marked after picking it", row.label);
                        }
                    }
                    Control::Stepper { value, down, up } => {
                        for c in [down, up].into_iter().flatten() {
                            let mut s = start.clone();
                            change(&mut s, *c);
                            let Control::Stepper { value: after, .. } = &rows(tab, &s)[k].control else { panic!() };
                            assert_ne!(after, value, "{}", row.label);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_old_look_is_a_choice_of_three_no_preset_touches() {
        let mut s = ClientSettings::default();
        let row = |s: &ClientSettings| rows(Tab::Effects, s).into_iter().find(|r| r.label == "Old look").expect("on the Effects tab");
        let Control::Choice(choices) = row(&s).control else { panic!("a choice") };
        assert_eq!(choices.iter().map(|(n, on, _)| (n.as_str(), *on)).collect::<Vec<_>>(), [("Off", true), ("Subtle", false), ("Strong", false)]);
        change(&mut s, choices[2].2);
        assert_eq!(s.old_look, OldLook::Strong);
        // A look, not a cost: it leaves the preset alone, and a preset leaves it alone.
        assert_eq!(s.graphics.preset, Preset::High);
        change(&mut s, Change::Preset(Preset::Low));
        assert_eq!(s.old_look, OldLook::Strong);
        let text = ron::ser::to_string(&s).unwrap();
        assert_eq!(ron::from_str::<ClientSettings>(&text).unwrap().old_look, OldLook::Strong);
        assert_eq!(ron::from_str::<ClientSettings>("(ui_scale: 110)").unwrap().old_look, OldLook::Off);
        change(&mut s, Change::Defaults(Tab::Graphics));
        assert_eq!(s.old_look, OldLook::Strong, "the Graphics tab's defaults leave the Effects tab's look");
        // The Effects tab's defaults put its switches and the look back.
        change(&mut s, Change::Defaults(Tab::Effects));
        assert_eq!((s.old_look, s.graphics.clone()), (OldLook::Off, Graphics::high()));
        // From Low, they are a change by hand.
        change(&mut s, Change::Preset(Preset::Low));
        change(&mut s, Change::Defaults(Tab::Effects));
        assert_eq!(s.graphics, Graphics { preset: Preset::Custom, bloom: true, grain: true, tilt_shift: true, vignette: true, haze: true, sky: true, ground_detail: true, ..Graphics::low() });
    }

    #[test]
    fn help_text_is_a_choice_of_three_kept_in_the_settings() {
        let mut s = ClientSettings::default();
        let row = |s: &ClientSettings| rows(Tab::Game, s).into_iter().find(|r| r.label == "Help text").expect("on the Game tab");
        let Control::Choice(choices) = row(&s).control else { panic!("a choice") };
        assert_eq!(choices.iter().map(|(n, on, _)| (n.as_str(), *on)).collect::<Vec<_>>(), [("Full", true), ("Short", false), ("Off", false)]);
        let full = row(&s).about;
        change(&mut s, choices[1].2);
        assert_eq!(s.help_text, HelpText::Short);
        assert_ne!(row(&s).about, full, "the line says what the level in force does");
        // It is read back from the file, and an older file without it is Full.
        let text = ron::ser::to_string(&s).unwrap();
        assert_eq!(ron::from_str::<ClientSettings>(&text).unwrap().help_text, HelpText::Short);
        assert_eq!(ron::from_str::<ClientSettings>("(ui_scale: 110)").unwrap().help_text, HelpText::Full);
        change(&mut s, Change::Defaults(Tab::Game));
        assert_eq!(s.help_text, HelpText::Full);
    }

    #[test]
    fn steps_stay_on_the_grid_and_in_range() {
        assert_eq!(stepped(1.0, 1, 10, 0, 200), 1.1);
        assert_eq!(stepped(1.0, -1, 10, 0, 200), 0.9);
        assert_eq!(stepped(0.35, 1, 5, 0, 200), 0.4);
        assert_eq!(stepped(0.37, -1, 5, 0, 200), 0.35);
        assert_eq!(stepped(0.37, 1, 5, 0, 200), 0.4);
        assert_eq!(stepped(2.0, 1, 10, 0, 200), 2.0);
        assert_eq!(stepped(0.0, -1, 10, 0, 200), 0.0);
        // The ends offer no further step.
        let mut s = ClientSettings::default();
        s.volume = 2.0;
        s.graphics.vegetation = 0.0;
        let audio = rows(Tab::Audio, &s);
        assert!(matches!(&audio[1].control, Control::Stepper { up: None, down: Some(_), .. }));
        let graphics = rows(Tab::Graphics, &s);
        assert!(matches!(&graphics[2].control, Control::Stepper { up: Some(_), down: None, .. }));
    }

    #[test]
    fn one_graphics_setting_makes_the_preset_custom_and_a_preset_sets_them_all() {
        let mut s = ClientSettings::default();
        change(&mut s, Change::Set(Switch::Grain, false));
        assert_eq!(s.graphics, Graphics { preset: Preset::Custom, grain: false, ..Graphics::high() });
        change(&mut s, Change::Preset(Preset::Low));
        assert_eq!(s.graphics, Graphics::low());
        // The same value again changes nothing, the preset neither.
        change(&mut s, Change::Msaa(2));
        assert_eq!(s.graphics.preset, Preset::Low);
        // Sound and the game's switches leave the graphics alone.
        change(&mut s, Change::Volume(-1));
        change(&mut s, Change::Set(Switch::FirstPerson, true));
        assert_eq!((s.graphics.preset, s.volume, s.first_person), (Preset::Low, 0.9, true));
        // Defaults put back that tab only.
        change(&mut s, Change::Defaults(Tab::Graphics));
        assert_eq!((s.graphics.clone(), s.volume, s.first_person), (Graphics::high(), 0.9, true));
        change(&mut s, Change::Defaults(Tab::Audio));
        change(&mut s, Change::Defaults(Tab::Game));
        assert_eq!(s, ClientSettings::default());
    }

    #[test]
    fn the_settings_file_keeps_the_new_settings_and_reads_older_files() {
        let mut s = ClientSettings::default();
        (s.volume, s.music, s.muted, s.first_person, s.fullscreen, s.vsync) = (0.5, 0.0, true, true, true, false);
        let text = ron::to_string(&s).unwrap();
        assert_eq!(ron::from_str::<ClientSettings>(&text).unwrap(), s);
        // A file from before them: as the game was.
        let old = ron::from_str::<ClientSettings>("(ui_scale: 110)").unwrap();
        assert_eq!((old.volume, old.music, old.muted, old.first_person, old.fullscreen, old.vsync), (1.0, 1.0, false, false, false, true));
    }

    #[test]
    fn a_resolution_is_picked_for_a_window_and_full_screen_has_the_screens() {
        let mut s = ClientSettings::default();
        let row = |s: &ClientSettings| rows(Tab::Display, s).into_iter().find(|r| r.label == "Resolution").expect("on the Display tab");
        s.fullscreen = false;
        assert!(matches!(row(&s).control, Control::Stepper { ref value, down: Some(_), up: Some(_) } if value == "1600 x 900"));
        change(&mut s, Change::WindowSize(1));
        assert_eq!(s.window_size, Some((1920, 1080)));
        change(&mut s, Change::WindowSize(-1));
        change(&mut s, Change::WindowSize(-1));
        assert_eq!(s.window_size, Some((1366, 768)));
        // It leaves the graphics preset alone.
        assert_eq!(s.graphics, ClientSettings::default().graphics);
        // Full screen, before the screen is known: Auto, nothing to step.
        s.fullscreen = true;
        assert!(matches!(row(&s).control, Control::Stepper { ref value, down: None, up: None } if value == "Auto"));
        // On a 6K screen at High: Auto is the screen's own, and the list steps down from it.
        let six_k = Some(UVec2::new(6016, 3384));
        let Control::Stepper { value, down: Some(down), up: None } = screen_resolution(&s, six_k).control else { panic!() };
        assert_eq!(value, "Auto (6016 x 3384)");
        change(&mut s, down);
        assert!(matches!(screen_resolution(&s, six_k).control, Control::Stepper { ref value, .. } if value == "4512 x 2538"));
        // At Medium Auto is 1080 lines.
        let mut t = s.clone();
        t.fullscreen_scale = None;
        t.graphics.preset = crate::graphics::Preset::Medium;
        assert!(matches!(screen_resolution(&t, six_k).control, Control::Stepper { ref value, .. } if value == "Auto (1920 x 1080)"));
        // The biggest is the screen's own, with nothing above it.
        t.fullscreen_scale = Some(1.0);
        assert!(matches!(screen_resolution(&t, six_k).control, Control::Stepper { ref value, up: None, .. } if value == "6016 x 3384"));
        assert_eq!(s.graphics, ClientSettings::default().graphics);
        change(&mut s, Change::Defaults(Tab::Display));
        assert_eq!((s.window_size, s.fullscreen_scale), (None, None));
    }

    #[test]
    fn tabs_and_keys() {
        assert_eq!(Tab::parse("audio"), Some(Tab::Audio));
        assert_eq!(Tab::parse(" Game "), Some(Tab::Game));
        assert_eq!(Tab::parse("sound"), None);
        // No tab is taller than the screen it must fit: ten rows at most.
        assert!(Tab::ALL.into_iter().all(|t| rows(t, &ClientSettings::default()).len() <= 10));
        // Every group has keys, and no key or line is empty.
        assert!(KEYS.iter().all(|(g, keys)| !g.is_empty() && !keys.is_empty() && keys.iter().all(|r| !r.key().is_empty() && !r.does().is_empty())));
        // Every action that can be rebound is on the tab.
        for a in crate::keys::Action::ALL {
            assert!(KEYS.iter().any(|(_, rows)| rows.contains(&KeyRow::Bound(a, rows.iter().find_map(|r| match r { KeyRow::Bound(b, d) if *b == a => Some(*d), _ => None }).unwrap_or("")))), "{} isn't on the Controls tab", a.id());
        }
    }

    /// Every action the controller has a button for is on the Controls tab's controller list.
    #[test]
    fn the_controller_list_shows_every_button() {
        let b = crate::pad::current();
        let listed = |a: crate::pad::PadAction| PAD_ROWS.iter().any(|(_, rows)| rows.iter().any(|r| matches!(r, PadRow::Bound(x, _) if *x == a)));
        for a in Action::ALL.into_iter().map(crate::pad::PadAction::Key).chain(crate::pad::Only::ALL.into_iter().map(crate::pad::PadAction::Only)) {
            if !b.binds(a).is_empty() {
                assert!(listed(a), "{} has a button but isn't on the Controls tab", a.id());
            }
        }
        let mut s = ClientSettings::default();
        change(&mut s, Change::Input(crate::pad::InputSetting::Controller));
        s.pad.insert("interact".into(), vec!["X".into()]);
        let text = ron::to_string(&s).unwrap();
        assert_eq!(ron::from_str::<ClientSettings>(&text).unwrap().input, crate::pad::InputSetting::Controller);
        change(&mut s, Change::Defaults(Tab::Controls));
        assert!(s.pad.is_empty() && s.input == crate::pad::InputSetting::Auto);
    }

    #[test]
    fn controls_defaults_drop_the_players_keys() {
        let mut s = ClientSettings::default();
        s.keys.insert("interact".into(), vec!["F".into()]);
        let text = ron::ser::to_string(&s).unwrap();
        assert_eq!(ron::from_str::<ClientSettings>(&text).unwrap().keys, s.keys);
        assert!(ron::from_str::<ClientSettings>("(ui_scale: 110)").unwrap().keys.is_empty());
        change(&mut s, Change::Defaults(Tab::Controls));
        assert!(s.keys.is_empty());
    }
}
