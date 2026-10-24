//! Zombye client.
//!
//! M1: you wake up in a house in a generated town ringed by forest. Fixed isometric camera, fog of
//! war, wall/roof cutaways, day/night, doors, and a horde of GPU-instanced zombies driven by an
//! in-process server. M2: the player is simulated by that server and predicted locally, and can
//! fight (weapons found in houses, shoves, stomps) or flee.
//!
//! Controls: WASD move, Shift run, C sneak (toggle), mouse to face, left mouse attack, Space shove
//! (or stomp a downed zombie in front), E pick up the item you face (or search the container you
//! face), Tab or I the inventory panel, F open/close the door or window you
//! face, G smash the window you face, T skip an hour, mouse wheel
//! zoom, Z spawn 250 zombies, F1 god mode, F2 ignored by zombies (dev toggles), `,` / `.` rotate between the four fixed angles (only with `--rotate`,
//! the "Camera rotation" setting).
//!
//! Options: `--zombies N` `--seed S` `--time HOUR` `--at X,Y` `--rotate` `--novsync`
//! `--screenshot PATH` (saved after `--screenshot-after SECS[,SECS...]`, default 5) `--exit-after SECS` `--zoom METRES`
//! `--autoplay` (the player fights by itself) `--fight-demo` (a bat and a few zombies close by)
//! `--siege` (outside your house with a bat, zombies battering its door) `--sneak` (start sneaking)
//! `--inventory` (start with the inventory panel open) `--god` / `--ignored` (start with F1 / F2 on)
//! `--map-chunks N` (a square map N × N chunks of 16 m; default 30) `--saves DIR` (where saves go;
//! default `saves`) `--load NAME|PATH` (start from a save; `--load-anyway` loads one whose game data
//! changed in a way it can't simply take, dropping what no longer fits). F5 quicksaves, F9
//! quickloads, and quitting (closing the window, Ctrl-C or SIGTERM in the terminal) autosaves
//! (`autosave` in the saves folder); `--quicksave-after SECS` and
//! `--quickload-after SECS` do F5 / F9 by themselves, for verification. Dead, the death screen
//! (`death.rs`) sums up the run: R starts a new run (the same town from the start), Q or Esc quits;
//! `--new-run-after SECS` presses R by itself. `--angle N` starts at
//! one of the four fixed camera angles (0..3), `--player ID` plays as that player of a loaded
//! save, `--data DIR` reads game data from another folder.
//!
//! `--render-to PATH` renders one PNG offscreen and exits, without ever opening a window
//! (`offscreen.rs`): usually with `--load`, plus `--size WxH`, `--zoom`, `--angle`, `--player` and
//! `--center X,Y`. The MCP `screenshot` tool runs the client this way.
//!
//! `--crowd-vat PATH` (or `ZOMBYE_CROWD_VAT=PATH`) draws the zombies with a character baked by
//! `tools/blender/zvat.py` instead of the grey-box (`none` forces the grey-box); see `zombie_render.rs`.
//!
//! Sound (`audio.rs`): `--mute` starts with it off, `--volume V` sets the master volume (0..2,
//! default 1), M mutes and unmutes.

mod audio;
mod autoplay;
mod camera;
mod daynight;
mod death;
mod fog;
mod furniture_models;
mod fx;
mod health_ui;
mod hud;
mod inventory_ui;
mod items_render;
mod materials;
mod meshgen;
mod needs_ui;
mod offscreen;
mod player;
mod rig;
mod saving;
mod server_link;
mod skills_ui;
mod survivor;
mod world_render;
mod zombie_render;
mod zvat;

use std::path::PathBuf;
use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use bevy::window::{ExitCondition, PresentMode};

/// Command-line options, available to every system.
#[derive(Resource, Clone, Debug)]
pub struct Options {
    /// `--zombies N`; without it, as many as `assets/data/population.ron` gives for the map.
    pub zombies: Option<u32>,
    pub seed: u64,
    pub start_hour: Option<f32>,
    /// Debug: start the player here instead of the bedroom (`--at X,Y`).
    pub at: Option<[f32; 2]>,
    pub rotate: bool,
    pub novsync: bool,
    pub screenshot: Option<String>,
    /// When to take it (seconds); several times (`--screenshot-after 3,4.5,6`) number the files
    /// (`shot-1.png`, `shot-2.png`, ...).
    pub screenshot_after: Vec<f32>,
    /// Metres of world visible vertically at start (`--zoom`, default 26).
    pub zoom: Option<f32>,
    pub exit_after: Option<f32>,
    /// Verification: the local player fights the nearest zombie by itself (`--autoplay`).
    pub autoplay: bool,
    /// Verification: start on open ground with a bat and a few zombies close by (`--fight-demo`).
    pub fight_demo: bool,
    /// Verification: start outside your house with a bat and a few zombies battering its door
    /// (`--siege`).
    pub siege: bool,
    /// Start sneaking (as if C had been pressed; `--sneak`).
    pub sneak: bool,
    /// Start with the inventory panel open (as if Tab had been pressed; `--inventory`).
    pub inventory: bool,
    /// Start with the dev toggles on, as if F1 / F2 had been pressed (`--god`, `--ignored`).
    pub god: bool,
    pub ignored: bool,
    /// A square map this many 16 m chunks a side (`--map-chunks`, default 30: 480 m).
    pub map_chunks: Option<i32>,
    /// Folder for quicksaves and the autosave (`--saves`, default `saves`).
    pub saves: std::path::PathBuf,
    /// Start from this save, by name in the saves folder or by path (`--load`).
    pub load: Option<String>,
    /// Load `--load`'s save (and F9's quicksave) even when the game data changed in a way it can't
    /// simply take, dropping what no longer fits (`--load-anyway`); no prompt.
    pub load_anyway: bool,
    /// Verification: quicksave / quickload this many seconds in, as if F5 / F9 were pressed
    /// (`--quicksave-after`, `--quickload-after`).
    pub quicksave_after: Option<f32>,
    pub quickload_after: Option<f32>,
    /// Verification: once dead and this many seconds in, start a new run as if R was pressed on
    /// the death screen (`--new-run-after`).
    pub new_run_after: Option<f32>,
    /// The camera's starting angle, one of the four fixed ones (`--angle 0..3`).
    pub angle: i32,
    /// Play as this player of a loaded save (`--player ID`; default its first player).
    pub player: Option<u32>,
    /// Game data folder (`--data`, default the repo's `assets/data`): templates, weapons, population.
    pub data: PathBuf,
    /// Render one frame offscreen to this PNG and exit, with no window (`--render-to`).
    pub render_to: Option<PathBuf>,
    /// Size of that image (`--size WxH`, default 1280x720).
    pub size: UVec2,
    /// Centre the offscreen camera here instead of on the player (`--center X,Y`). Fog of war and
    /// cutaways are still the player's.
    pub center: Option<[f32; 2]>,
    /// Draw the crowd with this baked character (`--crowd-vat PATH`, else `ZOMBYE_CROWD_VAT`; `none`
    /// for the grey-box). See `zombie_render.rs`.
    pub crowd_vat: Option<String>,
    /// Start with the sound off (`--mute`; M toggles it).
    pub mute: bool,
    /// Master volume (`--volume V`, 0..2, default 1).
    pub volume: f32,
}

/// Default and largest `--size` for `--render-to`.
pub const DEFAULT_RENDER_SIZE: UVec2 = UVec2::new(1280, 720);
pub const MAX_RENDER_SIZE: UVec2 = UVec2::new(3840, 2160);

/// `"1280x720"` → (1280, 720); each side at least 16 and at most [`MAX_RENDER_SIZE`].
pub fn parse_size(text: &str) -> Option<UVec2> {
    let (w, h) = text.trim().split_once(['x', 'X'])?;
    let size = UVec2::new(w.trim().parse().ok()?, h.trim().parse().ok()?);
    (size.cmpge(UVec2::splat(16)).all()).then(|| size.min(MAX_RENDER_SIZE))
}

/// `"12.5,-3"` → [12.5, -3].
fn parse_point(text: &str) -> Option<[f32; 2]> {
    let (x, y) = text.split_once(',')?;
    let p = [x.trim().parse().ok()?, y.trim().parse().ok()?];
    p.iter().all(|v: &f32| v.is_finite()).then_some(p)
}

impl Options {
    fn from_args() -> Self {
        Self::parse(&std::env::args().skip(1).collect::<Vec<_>>())
    }

    /// Options from command-line arguments (without the program name).
    pub fn parse(args: &[String]) -> Self {
        let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
        let flag = |name: &str| args.iter().any(|a| a == name);
        Self {
            zombies: value("--zombies").and_then(|v| v.parse().ok()),
            seed: value("--seed").and_then(|v| v.parse().ok()).unwrap_or(1),
            start_hour: value("--time").and_then(|v| v.parse().ok()),
            at: value("--at").and_then(|v| parse_point(&v)),
            rotate: flag("--rotate"),
            novsync: flag("--novsync"),
            screenshot: value("--screenshot"),
            screenshot_after: value("--screenshot-after")
                .map(|v| v.split(',').filter_map(|t| t.trim().parse().ok()).collect::<Vec<f32>>())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| vec![5.0]),
            zoom: value("--zoom").and_then(|v| v.parse().ok()),
            exit_after: value("--exit-after").and_then(|v| v.parse().ok()),
            autoplay: flag("--autoplay"),
            fight_demo: flag("--fight-demo"),
            siege: flag("--siege"),
            sneak: flag("--sneak"),
            inventory: flag("--inventory"),
            god: flag("--god"),
            ignored: flag("--ignored"),
            map_chunks: value("--map-chunks").and_then(|v| v.parse().ok()).filter(|n: &i32| (1..=256).contains(n)),
            saves: value("--saves").map(Into::into).unwrap_or_else(|| zombye_net::save_file::DEFAULT_DIR.into()),
            load: value("--load"),
            load_anyway: flag("--load-anyway"),
            quicksave_after: value("--quicksave-after").and_then(|v| v.parse().ok()),
            quickload_after: value("--quickload-after").and_then(|v| v.parse().ok()),
            new_run_after: value("--new-run-after").and_then(|v| v.parse().ok()),
            angle: value("--angle").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0).rem_euclid(4),
            player: value("--player").and_then(|v| v.parse().ok()),
            data: value("--data").map(Into::into).unwrap_or_else(zombye_net::content::default_data_dir),
            render_to: value("--render-to").map(Into::into),
            size: value("--size").and_then(|v| parse_size(&v)).unwrap_or(DEFAULT_RENDER_SIZE),
            center: value("--center").and_then(|v| parse_point(&v)),
            crowd_vat: value("--crowd-vat"),
            mute: flag("--mute"),
            volume: value("--volume").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).map_or(1.0, |v| v.clamp(0.0, 2.0)),
        }
    }

    /// How to load saves (`--load-anyway`).
    pub fn load_options(&self) -> zombye_sim::LoadOptions {
        zombye_sim::LoadOptions { force: self.load_anyway }
    }

    /// Rendering one frame offscreen (`--render-to`) rather than playing in a window.
    pub fn offscreen(&self) -> bool {
        self.render_to.is_some()
    }
}

/// Run condition: the game is live (not a frozen offscreen render). The local server doesn't tick
/// and the player isn't predicted in an offscreen render, so it shows the loaded state exactly.
pub fn live(options: Res<Options>) -> bool {
    !options.offscreen()
}

/// Ctrl-C, SIGTERM or SIGHUP in the terminal (`zombye_net::shutdown`): quit as if the window was
/// closed, so `saving.rs` autosaves. (Our own handler rather than Bevy's `TerminalCtrlCHandlerPlugin`,
/// whose `ctrlc` gives up on every signal when one was ignored at start, and has no SIGTERM.)
fn exit_on_signal(mut exit: MessageWriter<AppExit>, mut sent: Local<bool>) {
    if !*sent && zombye_net::shutdown::requested() {
        *sent = true;
        println!("Interrupted: quitting");
        exit.write(AppExit::from_code(130));
    }
}

fn main() {
    let options = Options::from_args();
    zombye_net::shutdown::install();
    // Wake the window's event loop when a signal comes in while it idles (unfocused), as Bevy's own
    // handler would; the loop then runs `exit_on_signal`.
    std::thread::spawn(|| {
        while !zombye_net::shutdown::requested() {
            std::thread::sleep(Duration::from_millis(50));
        }
        bevy::app::TerminalCtrlCHandlerPlugin::gracefully_exit();
    });
    let assets = AssetPlugin { file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(), ..default() };
    let plugins = if options.offscreen() {
        // No window, ever: no primary window, no winit (on macOS it would make this a foreground
        // app that takes focus), no audio device, no gamepads. The loop is driven by the schedule
        // runner and `offscreen.rs` exits when the picture is written.
        DefaultPlugins
            .set(WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, ..default() })
            .set(assets)
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::audio::AudioPlugin>()
            .disable::<bevy::gilrs::GilrsPlugin>()
            .disable::<bevy::app::TerminalCtrlCHandlerPlugin>()
    } else {
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Zombye".into(),
                    resolution: (1600, 900).into(),
                    present_mode: if options.novsync { PresentMode::AutoNoVsync } else { PresentMode::AutoVsync },
                    ..default()
                }),
                ..default()
            })
            .set(assets)
            .disable::<bevy::app::TerminalCtrlCHandlerPlugin>()
    };
    let offscreen = options.offscreen();
    let mut app = App::new();
    app.insert_resource(ClearColor(Color::srgb(0.05, 0.05, 0.06))).add_plugins(plugins).insert_resource(options).add_systems(First, exit_on_signal);
    if offscreen {
        app.add_plugins((ScheduleRunnerPlugin::run_loop(Duration::ZERO), offscreen::OffscreenPlugin));
    }
    app.add_plugins((
            server_link::ServerLinkPlugin,
            fog::FogPlugin,
            materials::WorldMaterialPlugin,
            world_render::WorldRenderPlugin,
            daynight::DayNightPlugin,
            player::PlayerPlugin,
            camera::IsoCameraPlugin,
            zombie_render::ZombieRenderPlugin,
            items_render::ItemsRenderPlugin,
            (hud::HudPlugin, inventory_ui::InventoryUiPlugin, health_ui::HealthUiPlugin, death::DeathPlugin, needs_ui::NeedsUiPlugin, skills_ui::SkillsUiPlugin),
            fx::FxPlugin,
            survivor::SurvivorPlugin,
            autoplay::AutoplayPlugin,
            saving::SavingPlugin,
        ));
    // An offscreen render has no audio device (Bevy's audio is off there).
    if !offscreen {
        app.add_plugins(audio::GameAudioPlugin);
    }
    if app.run().is_error() {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn offscreen_options_parse() {
        let o = Options::parse(&args("--render-to /tmp/x.png --load /tmp/w.zsave --size 640x360 --zoom 30 --angle 2 --player 3 --center 10.5,-2"));
        assert!(o.offscreen());
        assert_eq!(o.render_to.as_deref(), Some(std::path::Path::new("/tmp/x.png")));
        assert_eq!(o.load.as_deref(), Some("/tmp/w.zsave"));
        assert_eq!((o.size, o.zoom, o.angle, o.player, o.center), (UVec2::new(640, 360), Some(30.0), 2, Some(3), Some([10.5, -2.0])));

        let o = Options::parse(&args("--zombies 10"));
        assert!(!o.offscreen());
        assert_eq!((o.size, o.angle, o.player, o.center), (DEFAULT_RENDER_SIZE, 0, None, None));
        assert_eq!(Options::parse(&args("--angle 5")).angle, 1);
        assert_eq!(Options::parse(&args("--angle -1")).angle, 3);
    }

    #[test]
    fn sizes_are_checked_and_capped() {
        assert_eq!(parse_size("1280x720"), Some(UVec2::new(1280, 720)));
        assert_eq!(parse_size(" 800 X 600 "), Some(UVec2::new(800, 600)));
        assert_eq!(parse_size("99999x99999"), Some(MAX_RENDER_SIZE));
        for bad in ["", "1280", "8x8", "0x720", "axb", "-5x10"] {
            assert_eq!(parse_size(bad), None, "{bad:?}");
        }
        assert_eq!(parse_point("1,2"), Some([1.0, 2.0]));
        assert_eq!(parse_point("1;2"), None);
        assert_eq!(parse_point("nan,2"), None);
    }
}
