//! UI scale (#109): one setting, 75% to 150%, that every panel follows (the HUD, status tags,
//! inventory, skills, health, the current action, the death screen, the light preview and the
//! yellow message line), through Bevy's [`UiScale`], which scales every `px` size and font.
//!
//! In game Ctrl (or Cmd) with `=` / `-` steps it by [`STEP`] (numpad `+` / `-` too), Ctrl+0 puts
//! it back to 100%, and the new value shows in the yellow line. `--ui-scale PCT` starts at that
//! value for this session without saving it (for screenshots and tests).
//!
//! The panels are laid out for a screen of at least 1280 x 720 UI pixels ([`FIT_VIEW`]), so above
//! 100% the scale used is never more than the window has room for ([`fitted_pct`]): 150% is all of
//! it on a 1920 x 1080 window, 125% on 1600 x 900, 100% on 1280 x 720, and the yellow line says so
//! ("UI scale 150% (125% fits this window)"). Resize the window and it follows. The setting itself
//! stays as chosen.
//!
//! The setting is kept between sessions in the client settings file, `settings.ron` in the saves
//! folder (`--saves DIR`, default `saves/`): [`ClientSettings`], RON, every field with a default so
//! more settings can join it later (#97's panel positions). It's written when the scale changes in
//! game; a missing or damaged file means the defaults (100%), with a warning for a damaged one. An
//! offscreen render (`--render-to`) never reads or writes it: its pictures depend on the flags alone.

use std::path::{Path, PathBuf};

use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use serde::{Deserialize, Serialize};

use crate::Options;
use crate::saving::Notice;

/// The smallest and largest UI scale, in percent.
pub const MIN_PCT: u32 = 75;
pub const MAX_PCT: u32 = 150;
pub const DEFAULT_PCT: u32 = 100;
/// One key press, in percent.
pub const STEP: u32 = 5;
/// The client settings file, in the saves folder.
pub const SETTINGS_FILE: &str = "settings.ron";

/// `pct` within [`MIN_PCT`]..=[`MAX_PCT`].
pub fn clamp_pct(pct: i64) -> u32 {
    pct.clamp(MIN_PCT as i64, MAX_PCT as i64) as u32
}

/// One step up or down from `pct`, onto the [`STEP`] grid and within range: 100 → 105 / 95,
/// 103 → 105 / 100, 150 up stays 150.
pub fn step(pct: u32, up: bool) -> u32 {
    let pct = clamp_pct(pct as i64);
    let next = if up {
        (pct / STEP + 1) * STEP
    } else {
        pct.div_ceil(STEP).saturating_sub(1) * STEP
    };
    clamp_pct(next as i64)
}

/// `"125"` or `"125%"` → 125, clamped to range; `None` if it isn't a number.
pub fn parse_pct(text: &str) -> Option<u32> {
    let t = text.trim().trim_end_matches('%').trim();
    let v: f64 = t.parse().ok()?;
    v.is_finite().then(|| clamp_pct(v.round() as i64))
}

/// What's remembered of one panel (#97): whether it was open, and where its top-left corner was
/// dragged to, in UI pixels (`None`: where it's laid out by default).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(default)]
pub struct PanelMemory {
    pub open: bool,
    pub at: Option<(f32, f32)>,
}

/// The panels that are remembered (`panels.rs`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(default)]
pub struct PanelsMemory {
    pub inventory: PanelMemory,
    pub health: PanelMemory,
    pub skills: PanelMemory,
}

/// What the client keeps between sessions (`settings.ron` in the saves folder).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ClientSettings {
    /// UI scale in percent ([`MIN_PCT`]..=[`MAX_PCT`]).
    pub ui_scale: u32,
    /// Each panel's open state and place (#97).
    pub panels: PanelsMemory,
    /// The hotbar's slots, keys 1-9: the item id each holds (`hotbar.rs`, #97).
    pub hotbar: Vec<Option<String>>,
    /// Vegetation density and the post effects (`graphics.rs`, Linear DCM-59).
    pub graphics: crate::graphics::Graphics,
    /// The server browser (`browser_ui.rs`, #27): the name you join servers under, the list of
    /// servers it shows (a file or an `http://` address; `None`: the data folder's `servers.ron`)
    /// and the servers you added by hand.
    pub name: String,
    pub server_list: Option<String>,
    pub servers: Vec<String>,
    /// The inventory panel's Worn tab shows the body view, not the list (`inventory_ui/body.rs`, DCM-43).
    pub inventory_body: bool,
    /// Zombies you see show through what covers them from the camera (`silhouette.rs`; Shift+F10).
    pub zombie_silhouettes: bool,
    /// Doors and windows a building hides from the camera are outlined through it
    /// (`opening_hints.rs`; Ctrl+F10).
    pub opening_hints: bool,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            ui_scale: DEFAULT_PCT,
            panels: PanelsMemory::default(),
            hotbar: Vec::new(),
            graphics: crate::graphics::Graphics::default(),
            name: String::new(),
            server_list: None,
            servers: Vec::new(),
            inventory_body: false,
            zombie_silhouettes: false,
            opening_hints: true,
        }
    }
}

/// The client settings as they are now, and the file they're written to when they change.
/// Every part of the client that keeps something between sessions goes through it, so one's
/// change never loses another's.
#[derive(Resource, Clone, Debug, Default)]
pub struct SettingsStore {
    pub settings: ClientSettings,
    /// `None` offscreen: nothing is read or written.
    file: Option<PathBuf>,
}

impl SettingsStore {
    /// Read the settings file in the saves folder (not offscreen: the defaults, never written).
    pub fn from_options(options: &Options) -> (Self, Option<String>) {
        if options.offscreen() {
            return (Self::default(), None);
        }
        let path = ClientSettings::path(&options.saves);
        let (settings, warning) = ClientSettings::load(&path);
        (
            Self {
                settings,
                file: Some(path),
            },
            warning,
        )
    }

    /// The app's store, read from the file the first time anything asks (with a warning on stderr
    /// for a damaged one).
    pub fn ensure(app: &mut App) -> ClientSettings {
        if let Some(store) = app.world().get_resource::<Self>() {
            return store.settings.clone();
        }
        let (store, warning) = match app.world().get_resource::<Options>() {
            Some(options) => Self::from_options(options),
            None => (Self::default(), None),
        };
        if let Some(w) = warning {
            eprintln!("{w}");
        }
        let settings = store.settings.clone();
        app.insert_resource(store);
        settings
    }

    /// Change the settings, and write the file if that changed anything.
    pub fn change(&mut self, f: impl FnOnce(&mut ClientSettings)) -> Result<(), String> {
        let mut next = self.settings.clone();
        f(&mut next);
        if next == self.settings {
            return Ok(());
        }
        self.settings = next;
        match &self.file {
            Some(file) => self.settings.save(file),
            None => Ok(()),
        }
    }

    /// Whether changes are written to a file (not offscreen).
    #[cfg(test)]
    pub fn saved(&self) -> bool {
        self.file.is_some()
    }
}

impl ClientSettings {
    /// The settings file in `saves`.
    pub fn path(saves: &Path) -> PathBuf {
        saves.join(SETTINGS_FILE)
    }

    /// Read from `path`: no file is the defaults; a damaged one is the defaults and a warning.
    /// Out-of-range values are clamped.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => {
                return (
                    Self::default(),
                    Some(format!(
                        "can't read {}: {e}; using the defaults",
                        path.display()
                    )),
                );
            }
        };
        match ron::from_str::<Self>(&text) {
            Ok(mut s) => {
                s.ui_scale = clamp_pct(s.ui_scale as i64);
                // A place that isn't a point on any screen is no place.
                let ok = |at: (f32, f32)| at.0.is_finite() && at.1.is_finite();
                for p in [
                    &mut s.panels.inventory,
                    &mut s.panels.health,
                    &mut s.panels.skills,
                ] {
                    p.at = p.at.filter(|&at| ok(at));
                }
                s.hotbar.truncate(crate::hotbar::SLOTS);
                (s, None)
            }
            Err(e) => (
                Self::default(),
                Some(format!(
                    "{} is damaged ({e}); using the defaults",
                    path.display()
                )),
            ),
        }
    }

    /// Write to `path` (atomically: a crash mid-write leaves the old file whole).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())?;
        zombye_net::save_file::write_atomic(path, format!("{text}\n").as_bytes())
            .map_err(|e| format!("can't write {}: {e}", path.display()))
    }
}

/// The UI scale in use, in percent.
#[derive(Resource, Clone, Debug)]
pub struct UiScalePct {
    pub pct: u32,
}

impl UiScalePct {
    /// The scale to start with: `--ui-scale`, else the settings file's (not read offscreen), else 100%.
    pub fn from_options(options: &Options, settings: &ClientSettings) -> Self {
        Self {
            pct: options.ui_scale.unwrap_or(settings.ui_scale),
        }
    }
}

/// The smallest screen, in UI pixels, the panels are laid out for: at 100% on a 1280 x 720 window
/// the inventory, the vitals, the status tags and the current action all fit beside each other.
pub const FIT_VIEW: Vec2 = Vec2::new(1280.0, 720.0);

/// The scale actually used for `pct` on a `view` (the window's size in logical pixels, or the
/// offscreen picture's): growing past 100% stops where the panels would no longer fit (the UI's
/// screen smaller than [`FIT_VIEW`]), on the [`STEP`] grid, so a 150% setting is 125% on a
/// 1600 x 900 window and 100% on 1280 x 720. 100% and smaller are always used as set.
pub fn fitted_pct(pct: u32, view: Option<Vec2>) -> u32 {
    let Some(view) = view.filter(|v| v.x > 0.0 && v.y > 0.0) else {
        return pct;
    };
    if pct <= DEFAULT_PCT {
        return pct;
    }
    let fit = (view / FIT_VIEW).min_element() * 100.0;
    let fit = (fit + 1e-3).floor() as u32 / STEP * STEP;
    pct.min(fit.max(DEFAULT_PCT))
}

/// The screen the UI is drawn on, in logical pixels: the window's, or the offscreen picture's.
pub fn view(options: &Options, window: &Query<&Window, With<PrimaryWindow>>) -> Option<Vec2> {
    if options.offscreen() {
        Some(options.size.as_vec2())
    } else {
        window
            .single()
            .ok()
            .map(|w| Vec2::new(w.resolution.width(), w.resolution.height()))
    }
}

/// "UI scale 150%", and "(125% fits this window)" when it's [fitted](fitted_pct) down.
pub fn describe(pct: u32, used: u32) -> String {
    if used == pct {
        format!("UI scale {pct}%")
    } else {
        format!("UI scale {pct}% ({used}% fits this window)")
    }
}

pub struct UiScalePlugin;

impl Plugin for UiScalePlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().resource::<Options>().clone();
        let settings = SettingsStore::ensure(app);
        let scale = UiScalePct::from_options(&options, &settings);
        app.insert_resource(UiScale(scale.pct as f32 / 100.0))
            .insert_resource(scale)
            // After the search box, which holds keys back while you type.
            .add_systems(
                PreUpdate,
                keys.after(InputSystems)
                    .after(crate::inventory_ui::TextEntry),
            )
            // Every frame: the window can be resized.
            .add_systems(Update, apply);
    }
}

/// What a key press this frame asks of the scale: `Some(Some(up))` a step, `Some(None)` back to 100%.
fn asked(keys: &ButtonInput<KeyCode>) -> Option<Option<bool>> {
    if !keys.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]) {
        return None;
    }
    if keys.any_just_pressed([KeyCode::Equal, KeyCode::NumpadAdd]) {
        Some(Some(true))
    } else if keys.any_just_pressed([KeyCode::Minus, KeyCode::NumpadSubtract]) {
        Some(Some(false))
    } else if keys.any_just_pressed([KeyCode::Digit0, KeyCode::Numpad0]) {
        Some(None)
    } else {
        None
    }
}

fn keys(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    options: Res<Options>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut scale: ResMut<UiScalePct>,
    mut store: ResMut<SettingsStore>,
    notice: Option<ResMut<Notice>>,
) {
    let Some(ask) = asked(&keys) else { return };
    let pct = match ask {
        Some(up) => step(scale.pct, up),
        None => DEFAULT_PCT,
    };
    let mut line = describe(pct, fitted_pct(pct, view(&options, &window)));
    if pct != scale.pct {
        scale.pct = pct;
        if let Err(e) = store.change(|s| s.ui_scale = pct) {
            line = format!("{line} (not saved: {e})");
        }
    }
    if let Some(mut n) = notice {
        n.say(time.elapsed_secs(), line);
    }
}

fn apply(
    scale: Res<UiScalePct>,
    options: Res<Options>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut ui: ResMut<UiScale>,
) {
    let f = fitted_pct(scale.pct, view(&options, &window)) as f32 / 100.0;
    if ui.0 != f {
        ui.0 = f;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zombye-ui-scale-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn clamps_to_range() {
        assert_eq!(clamp_pct(10), MIN_PCT);
        assert_eq!(clamp_pct(-5), MIN_PCT);
        assert_eq!(clamp_pct(100), 100);
        assert_eq!(clamp_pct(999), MAX_PCT);
        assert_eq!(parse_pct("125"), Some(125));
        assert_eq!(parse_pct(" 80% "), Some(80));
        assert_eq!(parse_pct("40"), Some(MIN_PCT));
        assert_eq!(parse_pct("400"), Some(MAX_PCT));
        assert_eq!(parse_pct("112.6"), Some(113));
        for bad in ["", "big", "nan", "inf", "%"] {
            assert_eq!(parse_pct(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn steps_on_the_grid_and_stops_at_the_ends() {
        assert_eq!(step(100, true), 105);
        assert_eq!(step(100, false), 95);
        assert_eq!(step(103, true), 105);
        assert_eq!(step(103, false), 100);
        assert_eq!(step(MAX_PCT, true), MAX_PCT);
        assert_eq!(step(MIN_PCT, false), MIN_PCT);
        assert_eq!(step(500, false), 145);
        // From one end to the other in whole steps.
        let mut p = MIN_PCT;
        let mut n = 0;
        while p < MAX_PCT {
            p = step(p, true);
            n += 1;
        }
        assert_eq!(n, (MAX_PCT - MIN_PCT) / STEP);
    }

    #[test]
    fn growing_stops_where_the_panels_still_fit() {
        let v = |w: f32, h: f32| Some(Vec2::new(w, h));
        assert_eq!(fitted_pct(150, v(1920.0, 1080.0)), 150);
        assert_eq!(fitted_pct(150, v(1600.0, 900.0)), 125);
        assert_eq!(fitted_pct(150, v(1280.0, 720.0)), 100);
        assert_eq!(fitted_pct(110, v(1600.0, 900.0)), 110);
        // The narrower side decides; onto the step grid.
        assert_eq!(fitted_pct(150, v(3000.0, 900.0)), 125);
        assert_eq!(fitted_pct(150, v(1700.0, 2000.0)), 130);
        // 100% and less are kept, even on a small window; no window, nothing to fit.
        assert_eq!(fitted_pct(100, v(800.0, 600.0)), 100);
        assert_eq!(fitted_pct(75, v(800.0, 600.0)), 75);
        assert_eq!(fitted_pct(150, v(800.0, 600.0)), 100);
        assert_eq!(fitted_pct(150, None), 150);
        assert_eq!(fitted_pct(150, v(0.0, 0.0)), 150);
        assert_eq!(describe(150, 150), "UI scale 150%");
        assert_eq!(describe(150, 125), "UI scale 150% (125% fits this window)");
    }

    #[test]
    fn settings_save_and_load() {
        let d = dir("roundtrip");
        let path = ClientSettings::path(&d);
        let s = ClientSettings {
            ui_scale: 130,
            panels: PanelsMemory {
                inventory: PanelMemory {
                    open: true,
                    at: Some((300.0, 120.5)),
                },
                skills: PanelMemory {
                    open: false,
                    at: Some((0.0, 80.0)),
                },
                ..default()
            },
            hotbar: vec![Some("baseball_bat".into()), None, Some("flashlight".into())],
            ..default()
        };
        s.save(&path).unwrap();
        assert_eq!(ClientSettings::load(&path), (s, None));
        // Saved again over the old one.
        ClientSettings {
            ui_scale: 85,
            ..default()
        }
        .save(&path)
        .unwrap();
        assert_eq!(ClientSettings::load(&path).0, ClientSettings { ui_scale: 85, ..default() });
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_or_damaged_file_means_100_percent() {
        let d = dir("damaged");
        let path = ClientSettings::path(&d);
        assert_eq!(
            ClientSettings::load(&path),
            (ClientSettings::default(), None)
        );
        assert_eq!(ClientSettings::default().ui_scale, 100);
        std::fs::create_dir_all(&d).unwrap();
        for bad in [
            "(ui_scale: ",
            "not ron at all",
            "(ui_scale: \"big\")",
            "\u{0}\u{1}",
        ] {
            std::fs::write(&path, bad).unwrap();
            let (s, warning) = ClientSettings::load(&path);
            assert_eq!(s.ui_scale, 100, "{bad:?}");
            assert!(warning.is_some_and(|w| w.contains("damaged")), "{bad:?}");
        }
        // Out of range is clamped, unknown and missing fields are fine (later settings, older files).
        std::fs::write(&path, "(ui_scale: 400)").unwrap();
        assert_eq!(ClientSettings::load(&path).0.ui_scale, MAX_PCT);
        std::fs::write(&path, "(later_setting: [], ui_scale: 90)").unwrap();
        assert_eq!(ClientSettings::load(&path).0.ui_scale, 90);
        // A file from before the panels and the hotbar were kept: they start as laid out, and empty.
        let (old, warning) = ClientSettings::load(&path);
        assert_eq!((old.panels, old.hotbar.len(), warning), (PanelsMemory::default(), 0, None));
        // A place that is no point on a screen is dropped; a hotbar longer than the keys is cut.
        std::fs::write(&path, "(panels: (health: (open: true, at: Some((inf, 4.0)))), hotbar: [None, None, None, None, None, None, None, None, None, Some(\"axe\")])").unwrap();
        let s = ClientSettings::load(&path).0;
        assert_eq!(s.panels.health, PanelMemory { open: true, at: None });
        assert_eq!(s.hotbar.len(), crate::hotbar::SLOTS);
        std::fs::write(&path, "()").unwrap();
        assert_eq!(
            ClientSettings::load(&path),
            (ClientSettings::default(), None)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_flag_wins_and_offscreen_never_reads_the_file() {
        let d = dir("flag");
        ClientSettings {
            ui_scale: 120,
            ..default()
        }
        .save(&ClientSettings::path(&d))
        .unwrap();
        let opts = |extra: &str| {
            Options::parse(
                &format!("--saves {} {extra}", d.display())
                    .split_whitespace()
                    .map(String::from)
                    .collect::<Vec<_>>(),
            )
        };
        let pct = |extra: &str| {
            let o = opts(extra);
            let store = SettingsStore::from_options(&o).0;
            (UiScalePct::from_options(&o, &store.settings).pct, store.saved())
        };
        assert_eq!(pct(""), (120, true));
        assert_eq!(pct("--ui-scale 80"), (80, true));
        assert_eq!(pct("--ui-scale 20"), (MIN_PCT, true));
        assert_eq!(pct("--render-to /tmp/x.png"), (100, false));
        assert_eq!(pct("--render-to /tmp/x.png --ui-scale 150"), (150, false));
        // Offscreen nothing is written either, whatever changes.
        let mut off = SettingsStore::from_options(&opts("--render-to /tmp/x.png")).0;
        off.change(|s| s.ui_scale = 90).unwrap();
        assert_eq!(ClientSettings::load(&ClientSettings::path(&d)).0.ui_scale, 120);
        // One part's change keeps the others'.
        let mut store = SettingsStore::from_options(&opts("")).0;
        store.change(|s| s.panels.skills.open = true).unwrap();
        store.change(|s| s.hotbar = vec![Some("axe".into())]).unwrap();
        let read = ClientSettings::load(&ClientSettings::path(&d)).0;
        assert_eq!((read.ui_scale, read.panels.skills.open, read.hotbar.len()), (120, true, 1));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn keys_step_and_save_the_scale() {
        let d = dir("keys");
        let mut app = App::new();
        let options = Options::parse(&["--saves".to_string(), d.display().to_string()]);
        app.insert_resource(options)
            .insert_resource(Time::<()>::default())
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Notice>();
        app.add_plugins(UiScalePlugin);
        let press = |app: &mut App, keys: &[KeyCode]| {
            let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            input.reset_all();
            for &k in keys {
                input.press(k);
            }
            app.update();
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .release_all();
        };
        app.update();
        assert_eq!(app.world().resource::<UiScale>().0, 1.0);
        press(&mut app, &[KeyCode::ControlLeft, KeyCode::Equal]);
        assert_eq!(app.world().resource::<UiScalePct>().pct, 105);
        assert!((app.world().resource::<UiScale>().0 - 1.05).abs() < 1e-6);
        assert_eq!(
            ClientSettings::load(&ClientSettings::path(&d)).0.ui_scale,
            105
        );
        // `=` alone does nothing.
        press(&mut app, &[KeyCode::Equal]);
        assert_eq!(app.world().resource::<UiScalePct>().pct, 105);
        for _ in 0..10 {
            press(&mut app, &[KeyCode::SuperLeft, KeyCode::Minus]);
        }
        assert_eq!(app.world().resource::<UiScalePct>().pct, MIN_PCT);
        assert_eq!(
            ClientSettings::load(&ClientSettings::path(&d)).0.ui_scale,
            MIN_PCT
        );
        press(&mut app, &[KeyCode::ControlRight, KeyCode::Digit0]);
        assert_eq!(app.world().resource::<UiScalePct>().pct, 100);
        assert_eq!(app.world().resource::<UiScale>().0, 1.0);
        let _ = std::fs::remove_dir_all(&d);
    }
}
