//! Zombye client.
//!
//! M1: you wake up in a house in a generated town ringed by forest. Fixed isometric camera, fog of
//! war, wall/roof cutaways, day/night, doors, and a horde of GPU-instanced zombies driven by an
//! in-process server. M2: the player is simulated by that server and predicted locally, and can
//! fight (weapons found in houses, shoves, stomps) or flee.
//!
//! Controls: WASD move, Shift run, C sneak (toggle), mouse to face, left mouse attack, Space shove
//! (or stomp a downed zombie in front), E pick up a weapon, F open/close the door or window you
//! face, G smash the window you face, R get up again after dying, T skip an hour, mouse wheel
//! zoom, Z spawn 250 zombies, `,` / `.` rotate between the four fixed angles (only with `--rotate`,
//! the "Camera rotation" setting).
//!
//! Options: `--zombies N` `--seed S` `--time HOUR` `--at X,Y` `--rotate` `--novsync`
//! `--screenshot PATH` (saved after `--screenshot-after SECS[,SECS...]`, default 5) `--exit-after SECS` `--zoom METRES`
//! `--autoplay` (the player fights by itself) `--fight-demo` (a bat and a few zombies close by)
//! `--siege` (outside your house with a bat, zombies battering its door) `--sneak` (start sneaking)
//! `--map-chunks N` (a square map N × N chunks of 16 m; default 20) `--saves DIR` (where saves go;
//! default `saves`) `--load NAME|PATH` (start from a save). F5 quicksaves, F9 quickloads, and
//! quitting autosaves (`autosave` in the saves folder); `--quicksave-after SECS` and
//! `--quickload-after SECS` do F5 / F9 by themselves, for verification.

mod autoplay;
mod camera;
mod daynight;
mod fog;
mod fx;
mod hud;
mod items_render;
mod materials;
mod meshgen;
mod player;
mod rig;
mod saving;
mod server_link;
mod survivor;
mod world_render;
mod zombie_render;

use bevy::prelude::*;
use bevy::window::PresentMode;

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
    /// A square map this many 16 m chunks a side (`--map-chunks`, default 20: 320 m).
    pub map_chunks: Option<i32>,
    /// Folder for quicksaves and the autosave (`--saves`, default `saves`).
    pub saves: std::path::PathBuf,
    /// Start from this save, by name in the saves folder or by path (`--load`).
    pub load: Option<String>,
    /// Verification: quicksave / quickload this many seconds in, as if F5 / F9 were pressed
    /// (`--quicksave-after`, `--quickload-after`).
    pub quicksave_after: Option<f32>,
    pub quickload_after: Option<f32>,
}

impl Options {
    fn from_args() -> Self {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
        let flag = |name: &str| args.iter().any(|a| a == name);
        Self {
            zombies: value("--zombies").and_then(|v| v.parse().ok()),
            seed: value("--seed").and_then(|v| v.parse().ok()).unwrap_or(1),
            start_hour: value("--time").and_then(|v| v.parse().ok()),
            at: value("--at").and_then(|v| {
                let (x, y) = v.split_once(',')?;
                Some([x.trim().parse().ok()?, y.trim().parse().ok()?])
            }),
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
            map_chunks: value("--map-chunks").and_then(|v| v.parse().ok()).filter(|n: &i32| (1..=256).contains(n)),
            saves: value("--saves").map(Into::into).unwrap_or_else(|| zombye_net::save_file::DEFAULT_DIR.into()),
            load: value("--load"),
            quicksave_after: value("--quicksave-after").and_then(|v| v.parse().ok()),
            quickload_after: value("--quickload-after").and_then(|v| v.parse().ok()),
        }
    }
}

fn main() {
    let options = Options::from_args();
    App::new()
        .insert_resource(ClearColor(Color::srgb(0.05, 0.05, 0.06)))
        .add_plugins(
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
                .set(AssetPlugin { file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(), ..default() }),
        )
        .insert_resource(options)
        .add_plugins((
            server_link::ServerLinkPlugin,
            fog::FogPlugin,
            materials::WorldMaterialPlugin,
            world_render::WorldRenderPlugin,
            daynight::DayNightPlugin,
            player::PlayerPlugin,
            camera::IsoCameraPlugin,
            zombie_render::ZombieRenderPlugin,
            items_render::ItemsRenderPlugin,
            hud::HudPlugin,
            fx::FxPlugin,
            survivor::SurvivorPlugin,
            autoplay::AutoplayPlugin,
            saving::SavingPlugin,
        ))
        .run();
}
