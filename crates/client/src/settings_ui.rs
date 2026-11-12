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
//! `--settings graphics|audio|controls|game` starts with the screen up on that tab (pictures).

use bevy::prelude::*;
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode};

use crate::graphics::{Graphics, MAX_VEGETATION, Preset};
use crate::menu_skin::{self as skin, BONE, DIM, Look};
use crate::theme::{self, BODY, SMALL, font};
use crate::ui_scale::{self, ClientSettings, SettingsStore};

/// (As wide inside the sheet's ragged edge.)
const WIDTH: f32 = 1040.0;
const TABS_W: f32 = 190.0;
const LABEL_W: f32 = 430.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Graphics,
    Audio,
    Controls,
    Game,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Graphics, Tab::Audio, Tab::Controls, Tab::Game];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Graphics => "Graphics",
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
    GroundDetail,
    Fullscreen,
    Vsync,
    Muted,
    FirstPerson,
    ZombieSilhouettes,
    OpeningHints,
    SkipMenu,
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
    /// Steps of `ui_scale::STEP`.
    UiScale(i32),
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
        Change::UiScale(n) => s.ui_scale = ui_scale::step(s.ui_scale, n > 0),
        Change::Defaults(tab) => {
            let d = ClientSettings::default();
            match tab {
                Tab::Graphics => (s.graphics, s.fullscreen, s.vsync) = (d.graphics, d.fullscreen, d.vsync),
                Tab::Audio => (s.volume, s.music, s.muted) = (d.volume, d.music, d.muted),
                Tab::Game => (s.first_person, s.ui_scale, s.zombie_silhouettes, s.opening_hints, s.skip_menu) = (d.first_person, d.ui_scale, d.zombie_silhouettes, d.opening_hints, d.skip_menu),
                Tab::Controls => {}
            }
        }
    }
    s.graphics = s.graphics.clone().clamped();
    if s.graphics != before && !matches!(c, Change::Preset(_) | Change::Defaults(_)) {
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
        Switch::GroundDetail => &mut s.graphics.ground_detail,
        Switch::Fullscreen => &mut s.fullscreen,
        Switch::Vsync => &mut s.vsync,
        Switch::Muted => &mut s.muted,
        Switch::FirstPerson => &mut s.first_person,
        Switch::ZombieSilhouettes => &mut s.zombie_silhouettes,
        Switch::OpeningHints => &mut s.opening_hints,
        Switch::SkipMenu => &mut s.skip_menu,
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

/// A tab's rows, from the settings as they are. (Controls has none: it is the key list, [`KEYS`].)
pub fn rows(tab: Tab, s: &ClientSettings) -> Vec<Row> {
    let g = &s.graphics;
    match tab {
        Tab::Graphics => vec![
            Row {
                label: "Preset",
                about: crate::graphics_ui::describe(g.preset),
                control: Control::Choice(Preset::CHOICES.into_iter().map(|p| (p.name().to_string(), p == g.preset, Change::Preset(p))).collect()),
            },
            switch("Fullscreen", "The window fills the screen, without a border.", s, Switch::Fullscreen),
            switch("VSync", "Frames wait for the screen's refresh: no tearing.", s, Switch::Vsync),
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
            switch("Bloom", "The glow round lamps, fires and lit windows.", s, Switch::Bloom),
            switch("Film grain", "A little noise over the picture.", s, Switch::Grain),
            switch("Tilt-shift", "Soft focus toward the top and bottom of the screen.", s, Switch::TiltShift),
            switch("Vignette", "Darkened corners.", s, Switch::Vignette),
            switch("Contact shadows", "A soft dark line where walls meet the ground. Changes on the next start.", s, Switch::ContactShadows),
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
                label: "UI scale",
                about: "The size of every panel and text. Ctrl and + or - while you play.",
                control: Control::Stepper {
                    value: format!("{}%", s.ui_scale),
                    down: (s.ui_scale > ui_scale::MIN_PCT).then_some(Change::UiScale(-1)),
                    up: (s.ui_scale < ui_scale::MAX_PCT).then_some(Change::UiScale(1)),
                },
            },
            switch("Opening hints", "Outlines of the doors and windows a building hides from the camera.", s, Switch::OpeningHints),
            switch("Zombie silhouettes", "Zombies you can see show through what covers them from the camera.", s, Switch::ZombieSilhouettes),
            switch("Skip the main menu", "Start straight in the game, as before there was a menu. For development.", s, Switch::SkipMenu),
        ],
    }
}

/// The keys, by what they are for. (Shown, not changed here: rebinding comes later.)
pub const KEYS: [(&str, &[(&str, &str)]); 6] = [
    ("Moving", &[("W A S D", "Walk"), ("Shift", "Run"), ("C", "Sneak"), ("E", "Climb a fence or window"), ("B", "Sleep, and get up")]),
    ("Fighting", &[("Right mouse", "Aim"), ("Left mouse", "Attack or shoot, while aiming"), ("Space", "Shove, or stomp one that's down"), ("R", "Reload")]),
    ("Things", &[("E", "Pick up, search, flick a switch"), ("F", "Open or shut a door or window"), ("Shift + F", "Roll a blind, or fit one"), ("G", "Smash a window"), ("1 - 9", "Hotbar"), ("X", "Stop what you're doing")]),
    ("Panels", &[("Tab or I", "Inventory"), ("H", "Health"), ("K", "Skills"), ("O", "Objectives and hints"), ("Esc", "Pause menu")]),
    ("Camera", &[(", and .", "Turn a quarter"), (";", "First person and back"), ("Wheel", "Zoom"), ("F10", "Walls up or cut away")]),
    ("Other", &[("Enter", "Chat"), ("V  U  P", "Wave, hands up, point"), ("M", "Map"), ("Ctrl + M", "Mute"), ("F5 / F9", "Quicksave / quickload"), ("F7", "Server browser"), ("Ctrl + / -", "UI scale")]),
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
}

impl SettingsUi {
    pub fn close(&mut self) {
        self.open = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    Tab(Tab),
    Change(Change),
    Close,
}

pub struct SettingsUiPlugin;

impl Plugin for SettingsUiPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().get_resource::<crate::Options>().cloned();
        SettingsStore::ensure(app);
        let tab = options.as_ref().and_then(|o| o.settings.as_deref()).map(|t| Tab::parse(t).unwrap_or_default());
        let in_game = app.world().contains_resource::<crate::server_link::ServerLink>();
        app.insert_resource(SettingsUi { open: tab.is_some(), tab: tab.unwrap_or_default(), in_game, ..default() }).add_systems(Startup, spawn).add_systems(Update, (act, apply_live, draw).chain());
    }
}

#[derive(Component)]
struct Root;
#[derive(Component)]
struct Panel;

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
            c.spawn((Panel, Node { width: px(WIDTH), flex_direction: FlexDirection::Column, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(8)), ..default() }, skin::panel()));
        });
}

fn act(mut ui: ResMut<SettingsUi>, mut store: ResMut<SettingsStore>) {
    for a in std::mem::take(&mut ui.acts) {
        match a {
            Act::Tab(t) => ui.tab = t,
            Act::Close => ui.open = false,
            Act::Change(c) => {
                if let Err(e) = store.change(|s| change(s, c)) {
                    eprintln!("settings not saved: {e}");
                }
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
    mut cams: Query<(&mut crate::camera::IsoCamera, &mut Projection)>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
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

fn draw(mut commands: Commands, mut ui: ResMut<SettingsUi>, store: Res<SettingsStore>, mut root: Query<&mut Node, With<Root>>, panel: Query<Entity, With<Panel>>) {
    let want = (ui.open, ui.tab, ui.changes);
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
            bar.spawn((Text::new("Kept as you change them."), font(SMALL), TextColor(DIM), TextLayout::no_wrap()));
            skin::spacer(bar);
            skin::button(bar, "Close", None, Look::Plain, Val::Auto).observe(|_: On<PointerPress>, mut ui: ResMut<SettingsUi>| ui.acts.push(Act::Close));
        });
        skin::rule(p);
        // The body: the tabs down the left, the open one's settings beside them.
        p.spawn(Node { align_items: AlignItems::Stretch, ..default() }).with_children(|body| {
            body.spawn(Node { width: px(TABS_W), flex_shrink: 0.0, flex_direction: FlexDirection::Column, row_gap: px(8), padding: UiRect::all(px(16)), ..default() }).with_children(|col| {
                for t in Tab::ALL {
                    chip(col, t.name(), t == tab, Some(Act::Tab(t)), percent(100));
                }
            });
            skin::divider(body);
            body.spawn(Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, padding: UiRect::axes(px(24), px(14)), min_height: px(300), ..default() }).with_children(|col| {
                if tab == Tab::Controls {
                    keys_list(col);
                }
                for r in rows(tab, &s) {
                    col.spawn(Node { align_items: AlignItems::Center, column_gap: px(16), min_height: px(38), ..default() }).with_children(|row| {
                        row.spawn(Node { width: px(LABEL_W), flex_shrink: 0.0, flex_direction: FlexDirection::Column, ..default() }).with_children(|l| {
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
                                c.spawn(Node { width: px(64), justify_content: JustifyContent::Center, ..default() }).with_children(|v| {
                                    v.spawn((Text::new(value.clone()), theme::strong(BODY), TextColor(BONE), TextLayout::no_wrap()));
                                });
                                chip(c, "+", false, up.map(Act::Change), Val::Auto);
                            }
                        });
                    });
                }
                let note = match tab {
                    Tab::Graphics if in_game => "Changes show at once round this screen.",
                    Tab::Graphics => "Auto measures the first seconds of a game and picks what this machine holds.",
                    Tab::Audio => "Flags on the command line win for their session.",
                    Tab::Controls => "The keys can't be changed yet.",
                    Tab::Game => "Difficulty and the world's own options are chosen when you start a new game.",
                };
                col.spawn((Text::new(note), font(SMALL), TextColor(DIM), Node { margin: UiRect::top(px(10)), ..default() }));
            });
        });
        skin::rule(p);
        // The footer: the keys that work here.
        p.spawn(Node { height: px(50), padding: UiRect::horizontal(px(24)), align_items: AlignItems::Center, column_gap: px(18), flex_shrink: 0.0, ..default() }).with_children(|row| {
            skin::keycap(row, "Esc", if in_game { "back to the menu" } else { "back" });
            skin::spacer(row);
            if tab != Tab::Controls {
                skin::button(row, &format!("{} defaults", tab.name()), None, Look::Plain, Val::Auto).observe(move |_: On<PointerPress>, mut ui: ResMut<SettingsUi>| ui.acts.push(Act::Change(Change::Defaults(tab))));
            }
        });
    });
}

/// The Controls tab: the keys in three columns of groups.
fn keys_list(p: &mut ChildSpawnerCommands) {
    p.spawn(Node { column_gap: px(22), align_items: AlignItems::FlexStart, ..default() }).with_children(|cols| {
        for pair in KEYS.chunks(2) {
            cols.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), flex_basis: percent(33), ..default() }).with_children(|col| {
                for (group, keys) in pair {
                    col.spawn((Text::new(group.to_uppercase()), theme::head(BODY + 2.0), TextColor(skin::GREEN), Node { margin: UiRect::top(px(6)), ..default() }));
                    for (key, does) in keys.iter() {
                        col.spawn(Node { column_gap: px(8), ..default() }).with_children(|row| {
                            row.spawn((Text::new(*key), theme::strong(SMALL), TextColor(BONE), TextLayout::no_wrap(), Node { width: px(92), flex_shrink: 0.0, ..default() }));
                            row.spawn((Text::new(*does), font(SMALL), TextColor(DIM)));
                        });
                    }
                }
            });
        }
    });
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
        assert!(matches!(&graphics[4].control, Control::Stepper { up: Some(_), down: None, .. }));
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
    fn tabs_and_keys() {
        assert_eq!(Tab::parse("audio"), Some(Tab::Audio));
        assert_eq!(Tab::parse(" Game "), Some(Tab::Game));
        assert_eq!(Tab::parse("sound"), None);
        // Every group has keys, and no key or line is empty.
        assert!(KEYS.iter().all(|(g, keys)| !g.is_empty() && !keys.is_empty() && keys.iter().all(|(k, d)| !k.is_empty() && !d.is_empty())));
    }
}
