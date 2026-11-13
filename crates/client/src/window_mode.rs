//! The window: full screen or not, and how big when not.
//!
//! **Full screen by default, for players.** A packaged game opens borderless full screen at the
//! desktop's resolution until its player chooses otherwise (`ui_scale::default_fullscreen`,
//! `ClientSettings::fullscreen_chosen`); from a checkout it opens as a window, so development, its
//! recipes and its tools are as they were. `--size` is always a window of exactly that size.
//!
//! **Alt+Enter or F11** switches between the two, on every screen (the main menu, the new-game and
//! join screens, the game), and the choice is kept in `settings.ron` like the settings' Fullscreen
//! switch. The keys are fixed, not `keys.ron`'s: they must work before there is a game.
//!
//! **A window keeps its size.** Resized, it is that size again at the next start
//! (`ClientSettings::window_size`, written once the size has stood still for [`SETTLE_FRAMES`]),
//! and a window bigger than the screen it opens on is brought down to fit ([`fit`]): the first
//! start's 1600 x 900 doesn't hang off a laptop's 1366 x 768.
//!
//! Each screen is a run of its own with its own window (`window_handoff.rs`), so all of this goes
//! through the settings file, which every run reads when it opens its window.

use std::path::PathBuf;

use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::window::{Monitor, PrimaryMonitor, PrimaryWindow, WindowMode};

use crate::settings_ui::window_mode;
use crate::ui_scale::{ClientSettings, SettingsStore};

/// The window a game opens with when nothing is remembered, in UI pixels.
pub const DEFAULT_SIZE: (u32, u32) = (1600, 900);
/// The smallest window remembered or fitted: the panels are laid out for at least this.
const MIN_SIZE: (u32, u32) = (960, 540);
/// The biggest remembered (a file can say anything).
const MAX_SIZE: (u32, u32) = (7680, 4320);
/// The share of a screen a window may take: the rest is the title bar, a taskbar, a dock.
const SCREEN_SHARE: f32 = 0.92;
/// Frames a new size has stood still before it is written (a drag changes it every frame).
const SETTLE_FRAMES: u32 = 45;
/// Frames the fit to the screen waits for the screen to be known, then gives up.
const FIT_FRAMES: u32 = 120;

/// A remembered size held to what a window can be.
pub fn sane_size(size: (u32, u32)) -> (u32, u32) {
    (size.0.clamp(MIN_SIZE.0, MAX_SIZE.0), size.1.clamp(MIN_SIZE.1, MAX_SIZE.1))
}

/// The size a window of `size` opens at on a screen of `screen` (both in UI pixels): its own if it
/// fits in [`SCREEN_SHARE`] of the screen, else brought down to that, keeping its shape.
pub fn fit(size: (f32, f32), screen: (f32, f32)) -> (f32, f32) {
    let room = (screen.0 * SCREEN_SHARE, screen.1 * SCREEN_SHARE);
    if !(size.0 > 0.0 && size.1 > 0.0 && room.0 > 0.0 && room.1 > 0.0) {
        return size;
    }
    let by = (room.0 / size.0).min(room.1 / size.1);
    if by >= 1.0 { size } else { ((size.0 * by).floor(), (size.1 * by).floor()) }
}

/// Whether these keys, pressed this frame, ask for the other window mode: F11, or Enter with Alt.
pub fn asks_toggle(f11: bool, enter: bool, alt: bool) -> bool {
    f11 || (enter && alt)
}

/// What the window's systems need of this run.
#[derive(Resource, Clone)]
struct Run {
    /// The settings file, for a screen with no [`SettingsStore`].
    file: PathBuf,
    /// `--size`: the window is as asked, and nothing of it is kept.
    fixed: bool,
}

pub struct WindowModePlugin {
    pub saves: PathBuf,
    pub fixed: bool,
}

impl Plugin for WindowModePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Run { file: ClientSettings::path(&self.saves), fixed: self.fixed })
            // Before anything else reads the keys: Alt+Enter's Enter is taken, so it doesn't also
            // press a menu's button or open the chat line.
            .add_systems(
                PreUpdate,
                toggle.after(InputSystems).before(crate::settings_ui::capture).before(crate::browser_ui::keys).before(crate::chat::chat_keys).before(crate::inventory_ui::TextEntry),
            )
            .add_systems(Update, (fit_to_screen, remember_size));
    }
}

/// Change the kept settings: through the store where the screen has one, else the file itself.
fn keep(run: &Run, store: Option<&mut SettingsStore>, f: impl FnOnce(&mut ClientSettings)) {
    let result = match store {
        Some(store) => store.change(f),
        None => {
            let (mut settings, _) = ClientSettings::load(&run.file);
            let was = settings.clone();
            f(&mut settings);
            if settings.fullscreen != was.fullscreen {
                settings.fullscreen_chosen = true;
            }
            if settings == was { Ok(()) } else { settings.save(&run.file) }
        }
    };
    if let Err(e) = result {
        eprintln!("{e}");
    }
}

fn toggle(run: Res<Run>, mut keys: ResMut<ButtonInput<KeyCode>>, mut windows: Query<&mut Window, With<PrimaryWindow>>, mut store: Option<ResMut<SettingsStore>>) {
    let enter = keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]);
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    if !asks_toggle(keys.just_pressed(KeyCode::F11), enter, alt) {
        return;
    }
    if enter && alt {
        keys.clear_just_pressed(KeyCode::Enter);
        keys.clear_just_pressed(KeyCode::NumpadEnter);
    }
    if run.fixed {
        return;
    }
    let Ok(mut window) = windows.single_mut() else { return };
    let full = window.mode == WindowMode::Windowed;
    window.mode = window_mode(full);
    println!("{}", if full { "full screen (Alt+Enter or F11 for a window)" } else { "a window (Alt+Enter or F11 for full screen)" });
    keep(&run, store.as_deref_mut(), |s| s.fullscreen = full);
}

/// Once, when the screen is known: a window bigger than its screen is brought down to fit.
fn fit_to_screen(run: Res<Run>, mut windows: Query<&mut Window, With<PrimaryWindow>>, primary: Query<&Monitor, With<PrimaryMonitor>>, monitors: Query<&Monitor>, mut frames: Local<u32>) {
    if *frames > FIT_FRAMES {
        return;
    }
    *frames += 1;
    let Some(monitor) = primary.iter().next().or_else(|| monitors.iter().next()) else { return };
    *frames = FIT_FRAMES + 1;
    if run.fixed {
        return;
    }
    let Ok(mut window) = windows.single_mut() else { return };
    if window.mode != WindowMode::Windowed {
        return;
    }
    let scale = if monitor.scale_factor > 0.0 { monitor.scale_factor as f32 } else { 1.0 };
    let screen = (monitor.physical_width as f32 / scale, monitor.physical_height as f32 / scale);
    let size = (window.resolution.width(), window.resolution.height());
    let fitted = fit(size, screen);
    if fitted != size {
        window.resolution.set(fitted.0, fitted.1);
    }
}

/// Keep the window's size once it has stood still, while it is a window.
fn remember_size(run: Res<Run>, windows: Query<&Window, With<PrimaryWindow>>, mut store: Option<ResMut<SettingsStore>>, mut seen: Local<Option<((u32, u32), u32)>>, mut kept: Local<Option<(u32, u32)>>) {
    if run.fixed {
        return;
    }
    let Ok(window) = windows.single() else { return };
    if window.mode != WindowMode::Windowed {
        *seen = None;
        return;
    }
    let size = (window.resolution.width().round() as u32, window.resolution.height().round() as u32);
    if size.0 < MIN_SIZE.0 / 2 || size.1 < MIN_SIZE.1 / 2 {
        // Minimised, or not yet a window.
        return;
    }
    match seen.as_mut() {
        Some((last, still)) if *last == size => *still += 1,
        _ => *seen = Some((size, 0)),
    }
    // The size the window opened at is what the file already led to: nothing to write.
    if kept.is_none() && seen.is_some_and(|(_, still)| still == 0) {
        *kept = Some(size);
    }
    if seen.is_some_and(|(_, still)| still == SETTLE_FRAMES) && *kept != Some(size) {
        *kept = Some(size);
        keep(&run, store.as_deref_mut(), |s| s.window_size = Some(sane_size(size)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_scale::starts_fullscreen;

    #[test]
    fn a_packaged_game_starts_full_screen_until_its_player_chooses() {
        // (saved, chosen, the default)
        // A first start: packaged fills the screen, a checkout is a window.
        assert!(starts_fullscreen(false, false, true));
        assert!(!starts_fullscreen(false, false, false));
        // The player chose a window: it stays one, packaged or not.
        assert!(!starts_fullscreen(false, true, true));
        // Full screen picked before the choice was kept is still full screen.
        assert!(starts_fullscreen(true, false, false));
        assert!(starts_fullscreen(true, true, true));
    }

    #[test]
    fn choosing_is_remembered_with_the_setting() {
        let mut store = SettingsStore::default();
        assert!(!store.settings.fullscreen_chosen);
        let was = store.settings.fullscreen;
        // Changing something else isn't choosing.
        store.change(|s| s.vsync = !s.vsync).unwrap();
        assert!(!store.settings.fullscreen_chosen);
        store.change(|s| s.fullscreen = !was).unwrap();
        assert!(store.settings.fullscreen_chosen);
        // And a file written with the choice reads back as chosen, whatever the default.
        let text = ron::ser::to_string(&store.settings).unwrap();
        let back: ClientSettings = ron::from_str(&text).unwrap();
        assert_eq!((back.fullscreen, back.fullscreen_chosen), (!was, true));
        assert!(!starts_fullscreen(false, back.fullscreen_chosen, true));
    }

    #[test]
    fn a_window_never_opens_bigger_than_its_screen() {
        // Room to spare: as it is.
        assert_eq!(fit((1600.0, 900.0), (2560.0, 1440.0)), (1600.0, 900.0));
        // The first start's window on a small laptop: brought down, the same shape.
        let (w, h) = fit((1600.0, 900.0), (1366.0, 768.0));
        assert!(w <= 1366.0 * SCREEN_SHARE && h <= 768.0 * SCREEN_SHARE, "{w} x {h}");
        assert!((w / h - 16.0 / 9.0).abs() < 0.01);
        // As big as the screen is too big: there is a title bar.
        assert!(fit((1920.0, 1080.0), (1920.0, 1080.0)).1 < 1080.0);
        // A tall window on a wide screen is held by its height.
        assert!(fit((900.0, 1600.0), (1920.0, 1080.0)).1 <= 1080.0 * SCREEN_SHARE);
        // A screen that isn't known changes nothing.
        assert_eq!(fit((1600.0, 900.0), (0.0, 0.0)), (1600.0, 900.0));
    }

    #[test]
    fn a_remembered_size_is_held_to_what_a_window_can_be() {
        assert_eq!(sane_size((1280, 720)), (1280, 720));
        assert_eq!(sane_size((10, 10)), MIN_SIZE);
        assert_eq!(sane_size((u32::MAX, 2000)), (MAX_SIZE.0, 2000));
    }

    #[test]
    fn alt_enter_and_f11_ask_for_the_other_mode() {
        assert!(asks_toggle(true, false, false));
        assert!(asks_toggle(false, true, true));
        // Enter alone is a menu's button or the chat line; Alt alone frees the pointer.
        assert!(!asks_toggle(false, true, false));
        assert!(!asks_toggle(false, false, true));
    }
}
