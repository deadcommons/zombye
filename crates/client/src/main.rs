//! Zombye client.
//!
//! M1: you wake up in a house in a generated town ringed by forest. Fixed isometric camera, fog of
//! war, wall/roof cutaways, day/night, doors, and a horde of GPU-instanced zombies driven by an
//! in-process server. M2: the player is simulated by that server and predicted locally, and can
//! fight (weapons found in houses, shoves, stomps) or flee.
//!
//! Controls: WASD move, Shift run, C sneak (toggle), right mouse (held) aim at the cursor, and aiming: left mouse attack, Space shove
//! (or stomp a downed zombie in front), E pick up the item you face (or search the container you
//! face), Tab or I the inventory panel, F open/close the door or window you
//! face, G smash the window you face, T skip an hour, mouse wheel
//! zoom, Z spawn 250 zombies, F1 god mode, F2 ignored by zombies (dev toggles), `,` / `.` turn the camera a quarter between the four fixed angles
//! (an eased 0.3 s turn; `--turn N@SECS,...` turns by itself; `--rotate`, which this used to need, is accepted and ignored),
//! Ctrl+F10 the outlines of doors and windows a building hides (`--opening-hints on|off`).
//!
//! Options: `--zombies N` `--seed S` `--time HOUR` `--at X,Y` `--novsync`
//! `--screenshot PATH` (saved after `--screenshot-after SECS[,SECS...]`, default 5) `--exit-after SECS` `--zoom METRES`
//! `--pvp` (player against player on) `--duel` (PvP on, and another survivor with a crowbar comes at you)
//! `--autoplay` (the player fights by itself) `--fight-demo` (a bat and a few zombies close by)
//! `--siege` (outside your house with a bat, zombies battering its door) `--sneak` (start sneaking)
//! `--morning` (start with the first morning's screen up) `--skip-hours H` (skip game time before the first frame)
//! `--inventory` / `--skills` / `--health` (start with that panel open) `--god` / `--ignored` (start with F1 / F2 on)
//! `--tab rows|floor` / `--sort name|type|weight` / `--expand` (the inventory panel's tabs, order and opened stacks)
//! `--hud` (offscreen, draw the HUD and panels too) `--hover-tag N` (show the Nth status tag's tooltip)
//! `--map-chunks N` (a square map N × N chunks of 16 m; default 30) `--saves DIR` (where saves go;
//! default `saves`) `--load NAME|PATH` (start from a save; `--load-anyway` loads one whose game data
//! changed in a way it can't simply take, dropping what no longer fits). F5 quicksaves, F9
//! quickloads, and quitting (closing the window, Ctrl-C or SIGTERM in the terminal) autosaves
//! (`autosave` in the saves folder); `--quicksave-after SECS` and
//! `--quickload-after SECS` do F5 / F9 by themselves, for verification. Dead, the death screen
//! (`death.rs`) sums up the run: R starts a new run (the same town from the start), Q quits, Esc the pause menu;
//! `--new-run-after SECS` presses R by itself. `--angle N` starts at
//! one of the four fixed camera angles (0..3), `--player ID` plays as that player of a loaded
//! save, `--data DIR` reads game data from another folder.
//!
//! Menus: a plain start (no flag that sets up a world or a scene) opens the main menu first
//! (`main_menu.rs`; `--main-menu` forces it, `--no-menu` skips it); Esc while playing is the pause menu
//! (`pause_menu.rs`; `--pause` starts with it up), whose Settings is the settings screen (`settings_ui.rs`;
//! `--settings graphics|effects|audio|controls|game` starts with it up).
//!
//! `--render-to PATH` renders one PNG offscreen and exits, without ever opening a window
//! (`offscreen.rs`): usually with `--load`, plus `--size WxH`, `--zoom`, `--angle`, `--player` and
//! `--center X,Y`. The MCP `screenshot` tool runs the client this way.
//!
//! `--crowd-vat PATH` (or `ZOMBYE_CROWD_VAT=PATH`) draws the zombies with a character baked by
//! `tools/blender/zvat.py` instead of the grey-box (`none` forces the grey-box); see `zombie_render.rs`.
//! `--survivor PATH` (or `ZOMBYE_SURVIVOR=PATH`) draws survivors as a modular character's body GLB
//! (or the `*_body.glb` in a folder) with its wearables; see `survivor_model.rs`.
//!
//! Sound (`audio.rs`): `--mute` starts with it off, `--volume V` sets the master volume (0..2,
//! default 1), Ctrl+M mutes and unmutes (M is the map). Music (`music.rs`): `--music V` its volume under the master
//! (0..2, default 1; 0: none).
//!
//! Talking (`chat.rs`, #67): Enter opens the chat line (Tab there picks whisper, talk or shout, Enter
//! says it, Esc drops it); V waves, U puts your hands up (again lowers them), P points. `--say TEXT[@SECS]`,
//! `--shout`, `--whisper` and `--gesture NAME[@SECS]` do it by themselves; `--talk-demo` puts another
//! survivor a few metres off who talks and gestures, `--talk-wall` outside the wall of your room.
//!
//! Hold-ups and groups (`holdup.rs`, #73): Y ties the hands of the survivor you face (hands up, or
//! on the ground) or frees tied ones, L searches a tied survivor, hold J to struggle out of ties,
//! N invites into your group or accepts (Shift+N leaves), the middle mouse button puts down or
//! takes away a shared marker. `--holdup-demo` puts a survivor with its hands up in front of you
//! (PvP on; you get cable ties), `--holdup-auto` plays your side too.
//!
//! Graphics (`graphics.rs`, Linear DCM-59): `--graphics low|medium|high|ultra|auto` picks a preset (F8 in game), `--vegetation
//! off|low|high|N` the density of the instanced grass, flowers and litter (`vegetation.rs`), and
//! `--no-post grain,tilt,vignette,bloom,ao,ground|all` switches effects off one by one; kept in
//! `settings.ron` as `graphics`. `--frame-bench N` (offscreen) prints the cost of N frames.
//!
//! UI scale (`ui_scale.rs`): Ctrl (or Cmd) with `=` / `-` steps every panel's size by 5% (75-150%),
//! Ctrl+0 back to 100%, kept in `settings.ron` in the saves folder; `--ui-scale PCT` for one session.
//!
//! Hotbar (`hotbar.rs`): keys 1-9 swap to (or use) what's on that slot; `--hotbar ID,ID,...` sets the
//! slots for one session. The inventory, health and skills panels are dragged by their titles and
//! remembered with their open state in the same settings file (`panels.rs`).

// A packaged Windows build (`--features windows-gui`) opens no console window beside the game.
#![cfg_attr(all(windows, feature = "windows-gui"), windows_subsystem = "windows")]

mod interact;
mod keys;
mod aim_marker;
mod audio;
mod music;
mod release_audio;
mod release_brand;
mod pack;
mod report;
mod release_anim;
mod release_chars;
mod release_models;
mod autoplay;
mod browser_ui;
mod duel;
mod holdup;
mod hosting;
mod camera;
mod campfire;
mod character;
mod chat;
mod cutaway;
mod daynight;
mod dev_time;
mod death;
mod fog;
mod furniture_models;
mod film;
mod firstnight_ui;
mod fx;
mod fps_log;
mod graphics;
mod graphics_ui;
mod render_scale;
mod bench_live;
mod biome_watch;
mod black_watch;
#[cfg(feature = "profile")]
mod profile;
mod vegetation;
mod heard;
mod health_ui;
mod help_text;
mod hotbar;
mod hud;
mod inventory_ui;
mod join_screen;
mod items_render;
mod vehicles_render;
mod lights;
mod loading;
mod main_menu;
mod map_ui;
mod menu_skin;
mod materials;
mod meshgen;
mod action_ui;
mod debug_stats;
mod needs_ui;
mod new_game;
mod offscreen;
mod opening_hints;
mod panels;
mod pause_menu;
mod player;
mod rig;
mod rules_ui;
mod saving;
mod server_link;
mod settings_ui;
mod silhouette;
mod build_ui;
mod skills_ui;
mod survivor;
mod survivor_model;
mod theme;
mod ui_scale;
mod world_actions;
mod world_render;
mod zombie_render;
mod zvat;
mod window_handoff;
mod window_mode;

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
    /// `--loot-seed S`: what containers hold rolled from S (the same every game). Without it a
    /// new world rolls its loot from a fresh seed each time (`content::fresh_loot_seed`).
    pub loot_seed: Option<u64>,
    pub start_hour: Option<f32>,
    /// Debug: start the player here instead of the bedroom (`--at X,Y`).
    pub at: Option<[f32; 2]>,
    /// For recordings: turn the camera N quarter turns (1: as `.`, -1: as `,`) once SECS have gone
    /// by (`--turn N@SECS[,N@SECS...]`), in time order. `--rotate` (rotation used to need it) is
    /// still accepted and does nothing.
    pub turns: Vec<(i32, f32)>,
    /// For recordings and for timing it: switch between the view from above and first person (as `;`)
    /// once each SECS has gone by (`--view-at SECS[,SECS...]`).
    pub view_at: Vec<f32>,
    /// For pictures of the switch: hold the camera this far (0..1 of its time) on the way from the view
    /// from above to first person (`--view-mix U`).
    pub view_mix: Option<f32>,
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
    /// `--weapons-demo`: the fight demo played by itself, unhurt, with a new weapon in hand every few
    /// seconds and zombies kept coming, to watch every weapon's stance and blows (`autoplay.rs`).
    pub weapons_demo: bool,
    /// Verification: start outside your house with a bat and a few zombies battering its door
    /// (`--siege`).
    pub siege: bool,
    /// Player against player on in the world this client starts (`--pvp`; docs/PLAN.md §4.8).
    /// Single-player has nobody else, so it matters for testing, `--duel` and later servers.
    pub pvp: bool,
    /// Verification: PvP on, you on open ground with a hammer and another survivor a few metres
    /// off with a crowbar, who comes at you by itself (`--duel`; `duel.rs`).
    pub duel: bool,
    /// Start sneaking (as if C had been pressed; `--sneak`).
    pub sneak: bool,
    /// Start with the inventory panel open (as if Tab had been pressed; `--inventory`).
    pub inventory: bool,
    /// Start with the skills or the health panel open (as if K or H had been pressed; `--skills`,
    /// `--health`).
    pub skills: bool,
    /// `--build-menu`: start with the build menu (Shift+B) open, for pictures of it.
    pub build_menu: bool,
    pub health: bool,
    /// Start with the first morning's screen up, if the world has that outcome (`--morning`), and
    /// skip this many game hours before the first frame (`--skip-hours H`, a new world only): for
    /// pictures of the first night's screens (#19).
    pub morning: bool,
    /// Start with the graphics panel (F8) open (`--graphics-panel`): for pictures.
    pub graphics_panel: bool,
    /// Start with the map screen (M) open (`--map`), at this scale (`--map-zoom PX`, UI pixels a
    /// metre), looking here (`--map-center X,Y`), everything shown as explored (`--map-all`): for
    /// pictures. See `map_ui.rs`.
    pub map: bool,
    pub map_zoom: Option<f32>,
    pub map_center: Option<[f32; 2]>,
    pub map_all: bool,
    pub skip_hours: Option<f32>,
    /// Start the inventory panel searching for this, showing one category, with these carried
    /// stacks picked (`--find TEXT`, `--filter CATEGORY[,CATEGORY]`, `--pick N,N`: for pictures of it).
    pub find: Option<String>,
    pub filter: Option<String>,
    pub pick: Vec<u32>,
    /// Start the inventory screen another way (`--tab rows`: the rows view, `--tab floor`: the
    /// floor as where your things go, or both with a comma),
    /// sorted by a column (`--sort name|type|weight`), with rows of several stacks opened
    /// (`--expand`): for pictures of it.
    pub tab: Option<String>,
    pub sort: Option<String>,
    pub expand: bool,
    /// For recordings: Ctrl-click carried row A then Shift-click row B, once SECS have gone by
    /// (`--pick-range A,B[@SECS]`, default at once), and press the move-to row's "floor" after
    /// `--drop-picked-after SECS`.
    pub pick_range: Option<(u32, u32, f32)>,
    pub drop_picked_after: Option<f32>,
    /// Start with an item's right-click menu open (`--menu carried:N|worn:N|hand|container:N|ground[:N]`, #95).
    pub menu: Option<String>,
    /// For recordings: pick the open menu's Nth entry (from 1, as its number key) once SECS have
    /// gone by (`--menu-pick N[@SECS]`, default at once).
    pub menu_pick: Option<(usize, f32)>,
    /// Offscreen, draw the HUD and panels too (`--hud`; without it the picture is the world alone).
    pub hud: bool,
    /// Show the tooltip of the `N`th status tag (from 0), as if the pointer were over it
    /// (`--hover-tag N`: offscreen there is no pointer).
    pub hover_tag: Option<usize>,
    /// Start with the debug stats window open, as if F4 had been pressed (`--debug-stats`).
    pub debug_stats: bool,
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
    /// The camera's starting angle, one of the four fixed ones (`--angle 0..3`; a fraction, e.g.
    /// 0.5, is that far through the turn to the next: a picture of a turn under way).
    pub angle: f32,
    /// Play as this player of a loaded save (`--player ID`; default its first player).
    pub player: Option<u32>,
    /// Game data folder (`--data`, default the repo's `assets/data`): templates, weapons, population.
    pub data: PathBuf,
    /// Mod folders (`--mod DIR`, as often as wanted, in order; #25, docs/modding.md): their data
    /// joins the game's and, in single-player, their scripts run in the world.
    pub mods: Vec<PathBuf>,
    /// The mods folder (`--mods-dir DIR`, default `zombye_net::install::mods_dir`): the mods in it
    /// that its `enabled.ron` turns on load before the `--mod` ones (`zombye_mods::folder`).
    pub mods_dir: PathBuf,
    /// Leave the mods folder alone this run (`--no-mods`); `--mod` folders still load.
    pub no_mods: bool,
    /// Render one frame offscreen to this PNG and exit, with no window (`--render-to`).
    pub render_to: Option<PathBuf>,
    /// Size of that image (`--size WxH`, default 1280x720).
    pub size: UVec2,
    /// `--size` given: in a window, the window is that many pixels exactly (scale factor 1, so a
    /// `--screenshot` is WxH on a high-DPI screen too); without it, 1600x900 at the screen's scale.
    pub size_given: bool,
    /// Centre the offscreen camera here instead of on the player (`--center X,Y`). Fog of war and
    /// cutaways are still the player's.
    pub center: Option<[f32; 2]>,
    /// Draw the crowd with this baked character (`--crowd-vat PATH`, else `ZOMBYE_CROWD_VAT`; `none`
    /// for the grey-box). See `zombie_render.rs`.
    pub crowd_vat: Option<String>,
    /// Draw survivors as this modular character (`--survivor PATH`, a body GLB or a folder with one,
    /// else `ZOMBYE_SURVIVOR`; `none` for the grey-box). See `survivor_model.rs`.
    pub survivor: Option<String>,
    /// Hold every survivor in one pose (each a step further), for reviewing animation in offscreen pictures (`--pose
    /// walk:0.3`, see `survivor.rs`).
    pub pose: Option<String>,
    /// Recordings: the local player walks from point to point by itself (`--walk X,Y;X,Y;...`,
    /// `autoplay.rs`).
    pub walk: Vec<Vec2>,
    /// Show the aim marker in an offscreen picture as if aiming with the cursor on the ground this
    /// many metres east and south of the player (`--aim-at DX,DY`, `aim_marker.rs`).
    pub aim_at: Option<[f32; 2]>,
    /// The same, with the cursor on the nearest standing zombie (`--aim-at zombie`).
    pub aim_at_zombie: bool,
    /// Start in the first-person view (`--first-person`, or its first name `--third-person`; `;` switches in game, `camera.rs`).
    pub third_person: bool,
    /// Start with the cutaways off (`--walls-up`; F10 toggles them, `cutaway.rs`).
    pub walls_up: bool,
    /// `--watch-biomes`: start the game again where it is whenever the biomes file is saved
    /// (`biome_watch.rs`, docs/biomes.md).
    pub watch_biomes: bool,
    /// Show the silhouettes of zombies you see that something covers (`--zombie-silhouettes`;
    /// Shift+F10 toggles it and saves it, `silhouette.rs`). `None`: as the settings file says.
    pub zombie_silhouettes: Option<bool>,
    /// Outlines of the doors and windows a building hides from the camera (`--opening-hints
    /// on|off`; Ctrl+F10 toggles it and saves it, `opening_hints.rs`). `None`: as the settings
    /// file says (offscreen: on).
    pub opening_hints: Option<bool>,
    /// Start with the sound off (`--mute`; Ctrl+M toggles it).
    pub mute: bool,
    /// Master volume for this session (`--volume V`, 0..2; without it the settings file's, else 1).
    pub volume: Option<f32>,
    /// Music volume under the master for this session (`--music V`, 0..2; 0: no music is loaded;
    /// without it the settings file's, else 1). See `music.rs`.
    pub music: Option<f32>,
    /// The main menu (`main_menu.rs`): `--main-menu` shows it whatever else is asked for,
    /// `--no-menu` never; without either it is shown when nothing on the command line chooses a
    /// world or is there for a test or a picture (`plain`).
    pub main_menu: bool,
    pub no_menu: bool,
    pub plain: bool,
    /// Start with the pause menu up (`--pause`), or the settings screen on a tab
    /// (`--settings graphics|effects|audio|controls|game`): for pictures.
    pub pause: bool,
    /// Show the loading screen and hold it up (`--loading-screen`): for pictures (`loading.rs`).
    pub loading_screen: bool,
    pub settings: Option<String>,
    /// How much the UI explains, for this session, not saved (`--help-text full|short|off`; without
    /// it the settings file's, else full). See `help_text.rs`.
    pub help_text: Option<help_text::HelpText>,
    /// The old look for this session, not saved (`--old-look off|subtle|strong`; without it the
    /// settings file's, else off). See `film.rs`.
    pub old_look: Option<film::OldLook>,
    /// UI scale in percent for this session, not saved (`--ui-scale PCT`, 75..150; without it the
    /// client settings file's, else 100). See `ui_scale.rs`.
    pub ui_scale: Option<u32>,
    /// The hotbar's slots for this session, by item id (`--hotbar baseball_bat,-,flashlight`; `-` an
    /// empty slot), instead of the settings file's: for pictures. See `hotbar.rs`.
    pub hotbar: Option<String>,
    /// For recordings: press the hotbar's key K once SECS have gone by (`--hotbar-press K@SECS[,K@SECS...]`).
    pub hotbar_press: Vec<(usize, f32)>,
    /// Start with panels where they'd been dragged to, in UI pixels
    /// (`--panel-at inventory:X,Y[;health:X,Y][;skills:X,Y]`): for pictures. See `panels.rs`.
    pub panel_at: Option<String>,
    /// For recordings: drag a panel by its title, DX,DY UI pixels, once SECS have gone by
    /// (`--drag-panel inventory|health|skills:DX,DY[@SECS]`).
    pub drag_panel: Option<String>,
    /// Play on a multiplayer server instead of an in-process world (`--connect HOST[:PORT]`, #21),
    /// under this name (`--name`; the server makes one up without) and with its join password
    /// (`--password`). The world options, `--load` and the demo setups don't apply: the world is
    /// the server's.
    pub connect: Option<String>,
    pub name: String,
    pub password: String,
    /// `--agent`: with `--connect`, tell the server a program plays this client (#69; `--autoplay`
    /// says so by itself). A server's rules may refuse agents, or mark them to other players.
    pub agent: bool,
    /// `--server-info HOST[:PORT]`: ask that server for its rules without joining, print them and
    /// exit (#69). No window, no game data needed.
    pub server_info: Option<String>,
    /// The server browser (#27, `browser_ui.rs`; F7): the list it shows, a file or an `http://`
    /// address (`--server-list SRC`; else the settings file's, else the data folder's
    /// `servers.ron`); `--browser` starts with it open; `--browser-join N[@SECS]` joins the Nth
    /// server on show by itself (recordings).
    pub server_list: Option<String>,
    pub browser: bool,
    pub browser_join: Option<(usize, f32)>,
    /// `--from-browser single|server`: this run was started by a join in the browser. If the
    /// server refuses, the game doesn't stop: it starts in its own world (`single`: the autosave
    /// the join left) with the browser open and the reason on it.
    pub from_browser: Option<String>,
    /// The join screen (DCM-70, `join_screen.rs`) went through and started the game again to join
    /// for real (`--joined`); a game whose join then fails goes back to the screen with the reason
    /// (`--join-error RAW`, which starts the screen showing it).
    pub joined: bool,
    pub join_error: Option<String>,
    /// Graphics for this session, over the settings file's (`graphics.rs`): a whole preset
    /// (`--graphics low|medium|high|ultra|auto`), the vegetation's density (`--vegetation off|low|high|N`), and
    /// effects to switch off (`--no-post grain,tilt,vignette,bloom,ao,ground|all`).
    pub graphics: Option<String>,
    pub vegetation: Option<String>,
    pub no_post: Option<String>,
    /// Multisampling (1, 2 or 4) and the sun's shadow map size, over the preset's.
    pub msaa: Option<u32>,
    pub shadow_map: Option<u32>,
    /// The 3D world's share of the window's resolution on a HiDPI screen (0.5..1).
    pub render_scale: Option<f32>,
    /// `--bench-live N` (offscreen): time N frames with the world running and the player walking a
    /// fixed loop (`bench_live.rs`), so the sim and the client's systems count, not only the GPU.
    pub bench_live: Option<u32>,
    /// `--crowd N`: N zombies round the player at the start, called over by a noise (stress tests).
    pub crowd: Option<u32>,
    /// Offscreen, after the PNG: render this many more frames of the same picture and print their
    /// cost (`--frame-bench N`, else `ZOMBYE_FRAME_BENCH`; see `offscreen.rs`).
    pub frame_bench: Option<u32>,
    /// `--lag MS`: with `--connect`, play as if the server were that many milliseconds of ping
    /// away (half each way), to try out lag compensation on one machine (#23).
    pub lag_ms: u32,
    /// Start a new world holding this gun, loaded, with ammunition for it (`--gun ID`, #72: an id
    /// in weapons.ron, e.g. `pistol`); `--fight-demo` then gives no bat and puts its zombies 16 m off.
    pub gun: Option<String>,
    /// Start a new world with these in our pockets (`--give ID[:N],...`, item ids in items.ron):
    /// for trying things out without searching the town for them.
    pub give: Option<String>,
    /// `--new-game`: the new-game screen first (`new_game.rs`): who the survivor is and how the
    /// world is set; it starts the game again with `--new-game-from`.
    pub new_game: bool,
    /// `--new-game-from FILE`: start a new world as that file says (`zombye_net::newgame`).
    pub new_game_from: Option<String>,
    /// The same choices as flags (they go over the file's): `--background ID`,
    /// `--strengths ID,ID`, `--flaws ID,ID`, `--outfit ITEM,ITEM` (from the starting wardrobe;
    /// `none`: nothing), `--difficulty ID`.
    pub background: Option<String>,
    pub strengths: Option<Vec<String>>,
    pub flaws: Option<Vec<String>>,
    pub outfit: Option<Vec<String>>,
    pub difficulty: Option<String>,
    /// `--new-game-tab survivor|picks|world`: the new-game screen's tab at the start (pictures).
    pub new_game_tab: Option<String>,
    /// For recordings and pictures of the chat (#67): lines our survivor says by itself
    /// (`--say TEXT[@SECS]`, `--shout TEXT[@SECS]`, `--whisper TEXT[@SECS]`, each as often as
    /// wanted) and gestures it makes (`--gesture wave|hands_up|point[@SECS]`).
    pub say: Vec<(zombye_sim::Volume, String, f32)>,
    pub gestures: Vec<(zombye_sim::Gesture, f32)>,
    /// Verification: another survivor a few metres off who talks and gestures by itself
    /// (`--talk-demo`; `chat.rs`), or stands outside the wall of the room you wake up in and
    /// talks through it (`--talk-wall`).
    pub talk_demo: bool,
    pub talk_wall: bool,
    /// Verification of hold-ups (#73, `holdup.rs`): PvP on, another survivor right in front of
    /// you with its hands up and a few things on it, and cable ties and scissors for you
    /// (`--holdup-demo`); with `--holdup-auto` your side plays itself too.
    pub holdup_demo: bool,
    pub holdup_auto: bool,
}

/// `"Hello there@2.5"` → ("Hello there", 2.5); `"Hello"` → ("Hello", 0). The time is what follows
/// the last `@`, if that is a number.
fn parse_timed(text: &str) -> (String, f32) {
    match text.rsplit_once('@') {
        Some((t, secs)) => match secs.trim().parse::<f32>() {
            Ok(s) if s.is_finite() && s >= 0.0 => (t.to_string(), s),
            _ => (text.to_string(), 0.0),
        },
        None => (text.to_string(), 0.0),
    }
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

/// `"1,4@2.5"` → (1, 4, 2.5); `"1,4"` → (1, 4, 0).
fn parse_pick_range(text: &str) -> Option<(u32, u32, f32)> {
    let (rows, after) = text.split_once('@').unwrap_or((text, "0"));
    let (a, b) = rows.split_once(',')?;
    let after: f32 = after.trim().parse().ok().filter(|s: &f32| s.is_finite() && *s >= 0.0)?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?, after))
}

/// `"1@3.5"` → (1, 3.5); `"-1"` → (-1, 0): quarter turns of the camera, at most a full turn at once.
fn parse_turn(text: &str) -> Option<(i32, f32)> {
    let (n, after) = text.split_once('@').unwrap_or((text, "0"));
    let after: f32 = after.trim().parse().ok().filter(|s: &f32| s.is_finite() && *s >= 0.0)?;
    Some((n.trim().parse().ok().filter(|n| (-4..=4).contains(n) && *n != 0)?, after))
}

/// `"2@3.5"` → (2, 3.5); `"2"` → (2, 0). Entries count from 1.
fn parse_menu_pick(text: &str) -> Option<(usize, f32)> {
    let (n, after) = text.split_once('@').unwrap_or((text, "0"));
    let after: f32 = after.trim().parse().ok().filter(|s: &f32| s.is_finite() && *s >= 0.0)?;
    Some((n.trim().parse().ok().filter(|n| (1..=9).contains(n))?, after))
}

impl Options {
    fn from_args() -> Self {
        Self::parse(&std::env::args().skip(1).collect::<Vec<_>>())
    }

    /// Options from command-line arguments (without the program name).
    pub fn parse(args: &[String]) -> Self {
        let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
        let flag = |name: &str| args.iter().any(|a| a == name);
        // Every value given for a flag that may be repeated.
        let values = |name: &str| args.windows(2).filter(|w| w[0] == name).map(|w| w[1].clone()).collect::<Vec<_>>();
        let mut say = Vec::new();
        for volume in zombye_sim::Volume::ALL {
            let name = if volume == zombye_sim::Volume::Talk { "--say".to_string() } else { format!("--{}", volume.name()) };
            say.extend(values(&name).iter().map(|v| parse_timed(v)).map(|(text, at)| (volume, text, at)));
        }
        say.sort_by(|a, b| a.2.total_cmp(&b.2));
        let mut gestures: Vec<_> = values("--gesture").iter().map(|v| parse_timed(v)).filter_map(|(g, at)| Some((zombye_sim::Gesture::parse(&g)?, at))).collect();
        gestures.sort_by(|a, b| a.1.total_cmp(&b.1));
        Self {
            say,
            gestures,
            talk_demo: flag("--talk-demo") || flag("--talk-wall"),
            talk_wall: flag("--talk-wall"),
            holdup_demo: flag("--holdup-demo") || flag("--holdup-auto"),
            holdup_auto: flag("--holdup-auto"),
            zombies: value("--zombies").and_then(|v| v.parse().ok()),
            seed: value("--seed").and_then(|v| v.parse().ok()).unwrap_or(1),
            loot_seed: value("--loot-seed").and_then(|v| v.parse().ok()),
            start_hour: value("--time").and_then(|v| v.parse().ok()),
            at: value("--at").and_then(|v| parse_point(&v)),
            turns: {
                let mut turns: Vec<(i32, f32)> = value("--turn").map_or(Vec::new(), |v| v.split(',').filter_map(parse_turn).collect());
                turns.sort_by(|a, b| a.1.total_cmp(&b.1));
                turns
            },
            novsync: flag("--novsync"),
            screenshot: value("--screenshot"),
            screenshot_after: value("--screenshot-after")
                .map(|v| v.split(',').filter_map(|t| t.trim().parse().ok()).collect::<Vec<f32>>())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| vec![5.0]),
            zoom: value("--zoom").and_then(|v| v.parse().ok()),
            exit_after: value("--exit-after").and_then(|v| v.parse().ok()),
            autoplay: flag("--autoplay") || flag("--weapons-demo"),
            fight_demo: flag("--fight-demo") || flag("--weapons-demo"),
            weapons_demo: flag("--weapons-demo"),
            siege: flag("--siege"),
            pvp: flag("--pvp") || flag("--duel") || flag("--holdup-demo") || flag("--holdup-auto"),
            duel: flag("--duel"),
            sneak: flag("--sneak"),
            inventory: flag("--inventory"),
            skills: flag("--skills"),
            build_menu: flag("--build-menu"),
            health: flag("--health"),
            morning: flag("--morning"),
            graphics_panel: flag("--graphics-panel"),
            map: flag("--map"),
            map_zoom: value("--map-zoom").and_then(|v| v.parse::<f32>().ok()).filter(|z| z.is_finite() && *z > 0.0),
            map_center: value("--map-center").and_then(|v| parse_point(&v)),
            map_all: flag("--map-all"),
            skip_hours: value("--skip-hours").and_then(|v| v.parse::<f32>().ok()).filter(|h| h.is_finite() && (0.0..=240.0).contains(h)),
            hud: flag("--hud"),
            find: value("--find"),
            filter: value("--filter"),
            pick: value("--pick").map_or(Vec::new(), |v| v.split(',').filter_map(|k| k.trim().parse().ok()).collect()),
            tab: value("--tab"),
            sort: value("--sort"),
            expand: flag("--expand"),
            pick_range: value("--pick-range").and_then(|v| parse_pick_range(&v)),
            drop_picked_after: value("--drop-picked-after").and_then(|v| v.parse().ok()),
            menu: value("--menu"),
            menu_pick: value("--menu-pick").and_then(|v| parse_menu_pick(&v)),
            hover_tag: value("--hover-tag").and_then(|v| v.parse().ok()),
            debug_stats: flag("--debug-stats"),
            god: flag("--god"),
            ignored: flag("--ignored"),
            map_chunks: value("--map-chunks").and_then(|v| v.parse().ok()).filter(|n: &i32| (1..=256).contains(n)),
            saves: value("--saves").map(Into::into).unwrap_or_else(zombye_net::install::default_saves_dir),
            load: value("--load"),
            load_anyway: flag("--load-anyway"),
            quicksave_after: value("--quicksave-after").and_then(|v| v.parse().ok()),
            quickload_after: value("--quickload-after").and_then(|v| v.parse().ok()),
            new_run_after: value("--new-run-after").and_then(|v| v.parse().ok()),
            angle: value("--angle").and_then(|v| v.parse::<f32>().ok()).filter(|a| a.is_finite()).unwrap_or(0.0).rem_euclid(4.0),
            player: value("--player").and_then(|v| v.parse().ok()),
            data: value("--data").map(Into::into).unwrap_or_else(zombye_net::content::default_data_dir),
            mods: values("--mod").into_iter().map(Into::into).collect(),
            mods_dir: value("--mods-dir").map(Into::into).unwrap_or_else(zombye_net::install::mods_dir),
            no_mods: flag("--no-mods"),
            render_to: value("--render-to").map(Into::into),
            size: value("--size").and_then(|v| parse_size(&v)).unwrap_or(DEFAULT_RENDER_SIZE),
            size_given: value("--size").and_then(|v| parse_size(&v)).is_some(),
            center: value("--center").and_then(|v| parse_point(&v)),
            crowd_vat: value("--crowd-vat"),
            survivor: value("--survivor"),
            pose: value("--pose"),
            walk: value("--walk").map(|v| v.split(';').filter_map(parse_point).map(Vec2::from).collect()).unwrap_or_default(),
            aim_at: value("--aim-at").and_then(|v| parse_point(&v)),
            aim_at_zombie: value("--aim-at").is_some_and(|v| v == "zombie"),
            third_person: flag("--first-person") || flag("--third-person"),
            view_mix: value("--view-mix").and_then(|v| v.parse::<f32>().ok()).filter(|u| u.is_finite()).map(|u| u.clamp(0.0, 1.0)),
            view_at: {
                let mut at: Vec<f32> = value("--view-at").map_or(Vec::new(), |v| v.split(',').filter_map(|t| t.trim().parse().ok()).filter(|t: &f32| t.is_finite() && *t >= 0.0).collect());
                at.sort_by(f32::total_cmp);
                at
            },
            walls_up: flag("--walls-up"),
            watch_biomes: flag("--watch-biomes"),
            zombie_silhouettes: flag("--zombie-silhouettes").then_some(true),
            opening_hints: value("--opening-hints").and_then(|v| match v.as_str() {
                "on" => Some(true),
                "off" => Some(false),
                _ => None,
            }),
            mute: flag("--mute"),
            volume: value("--volume").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 2.0)),
            music: value("--music").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 2.0)),
            main_menu: flag("--main-menu"),
            no_menu: flag("--no-menu"),
            plain: main_menu::plain(args),
            pause: flag("--pause"),
            loading_screen: flag("--loading-screen"),
            settings: value("--settings"),
            help_text: value("--help-text").and_then(|v| help_text::HelpText::parse(&v)),
            old_look: value("--old-look").and_then(|v| film::OldLook::parse(&v)),
            ui_scale: value("--ui-scale").and_then(|v| ui_scale::parse_pct(&v)),
            hotbar: value("--hotbar"),
            hotbar_press: value("--hotbar-press").map_or(Vec::new(), |v| v.split(',').filter_map(parse_menu_pick).collect()),
            panel_at: value("--panel-at"),
            drag_panel: value("--drag-panel"),
            connect: value("--connect"),
            name: value("--name").unwrap_or_default(),
            // (The browser's join hands the password over in the environment: a command line can be read by other programs.)
            password: value("--password").or_else(|| std::env::var("ZOMBYE_PASSWORD").ok()).unwrap_or_default(),
            agent: flag("--agent"),
            server_info: value("--server-info"),
            server_list: value("--server-list"),
            browser: flag("--browser"),
            browser_join: value("--browser-join").and_then(|v| {
                let (n, after) = v.split_once('@').unwrap_or((&v, "0"));
                Some((n.trim().parse().ok().filter(|n| *n >= 1)?, after.trim().parse().ok().filter(|s: &f32| s.is_finite() && *s >= 0.0)?))
            }),
            from_browser: value("--from-browser"),
            joined: flag("--joined"),
            join_error: value("--join-error"),
            graphics: value("--graphics"),
            vegetation: value("--vegetation"),
            no_post: value("--no-post"),
            msaa: value("--msaa").and_then(|v| v.parse().ok()),
            shadow_map: value("--shadow-map").and_then(|v| v.parse().ok()),
            render_scale: value("--render-scale").and_then(|v| v.parse().ok()),
            bench_live: value("--bench-live").and_then(|v| v.parse().ok()),
            crowd: value("--crowd").and_then(|v| v.parse().ok()),
            frame_bench: value("--frame-bench").and_then(|v| v.parse().ok()),
            lag_ms: value("--lag").and_then(|v| v.parse::<u32>().ok()).map_or(0, |v| v.min(2000)),
            gun: value("--gun"),
            give: value("--give"),
            new_game: flag("--new-game"),
            new_game_from: value("--new-game-from"),
            background: value("--background"),
            strengths: value("--strengths").map(|v| id_list(&v)),
            flaws: value("--flaws").map(|v| id_list(&v)),
            outfit: value("--outfit").map(|v| id_list(&v)),
            difficulty: value("--difficulty"),
            new_game_tab: value("--new-game-tab"),
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

    /// The mods folder to load from, if this run reads it: not with `--no-mods`, and not offscreen,
    /// whose pictures depend on the flags alone (the MCP `screenshot` tool passes its world's
    /// `--mod`s), as they never read the settings file either.
    pub fn mods_folder(&self) -> Option<&std::path::Path> {
        (!self.no_mods && !self.offscreen()).then_some(self.mods_dir.as_path())
    }
}

/// `a,b,c` as ids (`none` or nothing: none).
fn id_list(v: &str) -> Vec<String> {
    v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != "none").collect()
}

/// Run condition: the game is live (not a frozen offscreen render). The local server doesn't tick
/// and the player isn't predicted in an offscreen render, so it shows the loaded state exactly.
/// The log plugin; with the `profile` feature it also times every system (`profile.rs`).
fn log_plugin() -> bevy::log::LogPlugin {
    #[cfg(feature = "profile")]
    return bevy::log::LogPlugin { custom_layer: profile::layer, ..default() };
    #[cfg(not(feature = "profile"))]
    bevy::log::LogPlugin::default()
}

pub fn live(options: Res<Options>) -> bool {
    // `--bench-live` is offscreen but runs the game as it plays (input, prediction, UI), so its
    // timings count all of it.
    !options.offscreen() || options.bench_live.is_some()
}

/// Ctrl-C, SIGTERM or SIGHUP in the terminal (`zombye_net::shutdown`): quit as if the window was
/// closed, so `saving.rs` autosaves. (Our own handler rather than Bevy's `TerminalCtrlCHandlerPlugin`,
/// whose `ctrlc` gives up on every signal when one was ignored at start, and has no SIGTERM.)
pub(crate) fn exit_on_signal(mut exit: MessageWriter<AppExit>, mut sent: Local<bool>) {
    if !*sent && zombye_net::shutdown::requested() {
        *sent = true;
        println!("Interrupted: quitting");
        exit.write(AppExit::from_code(130));
    }
}

fn main() {
    // `--write-pack OUT DIR...`: pack the release folders for a packaged build (`pack.rs`).
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--write-pack") {
        pack::write_from_args(&args[2..]);
    }
    if args.get(1).is_some_and(|a| a == "--version") {
        println!("Zombye {}", zombye_net::install::build_name());
        return;
    }
    // A packaged game runs itself again with its output to a log file (`report.rs`).
    if let Some(code) = report::supervise() {
        std::process::exit(code);
    }
    // The supervised child: a panic leaves its report for the parent to send (`report.rs`).
    report::capture_panics();
    let options = Options::from_args();
    // `--server-info HOST`: what that server is and its rules, without joining it (#69).
    if let Some(addr) = &options.server_info {
        match zombye_net::remote::query(addr, Duration::from_secs(10)) {
            Ok(info) => {
                for line in zombye_net::remote::describe(&info, addr) {
                    println!("{line}");
                }
                return;
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    // What grows on the wild ground: `--biomes FILE` and `--biome ID` (docs/biomes.md), told to the
    // content loader before any world is made.
    {
        let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|k| args.get(k + 1)).cloned();
        zombye_net::content::set_biome_flags(value("--biomes").map(Into::into), value("--biome"));
    }
    // The world's colours, from the data folder's palette.ron (#60), before anything is drawn.
    if let Err(e) = meshgen::palette::load(&options.data) {
        eprintln!("can't use the palette: {e}");
        std::process::exit(1);
    }
    // How the light looks (light.ron) and where the ground cover grows (vegetation.ron).
    if let Err(e) = daynight::load(&options.data).and_then(|_| vegetation::load_look(&options.data)).and_then(|_| meshgen::decay::load(&options.data)) {
        eprintln!("can't use the look: {e}");
        std::process::exit(1);
    }
    // How the fog of war looks, from the data folder's fog.ron.
    let fog_look = match fog::FogLook::load(&options.data) {
        Ok(look) => look,
        Err(e) => {
            eprintln!("can't use the fog's look: {e}");
            std::process::exit(1);
        }
    };
    zombye_net::shutdown::install();
    // Wake the window's event loop when a signal comes in while it idles (unfocused), as Bevy's own
    // handler would; the loop then runs `exit_on_signal`.
    std::thread::spawn(|| {
        while !zombye_net::shutdown::requested() {
            std::thread::sleep(Duration::from_millis(50));
        }
        bevy::app::TerminalCtrlCHandlerPlugin::gracefully_exit();
    });
    // `--connect` in a window: the join screen first (DCM-70), which starts the game again once
    // the join has gone through.
    if join_screen::wanted(&options) {
        join_screen::run(options);
    }
    // Started with nothing that chooses a world: the main menu first, which starts the game again
    // with what was picked there.
    if main_menu::wanted(&options) && !main_menu::skipped(&options) {
        main_menu::run(options);
    }
    // `--new-game`: who the survivor is and how the world is set, before there is a world
    // (`new_game.rs`); it starts the game again with the choices made.
    if new_game::wanted(&options) {
        new_game::run(options);
    }
    let plugins = window_plugins(&options);
    let offscreen = options.offscreen();
    let graphics = graphics::Graphics::from_options(&options);
    let mut app = App::new();
    app.insert_resource(ClearColor(Color::srgb(0.05, 0.05, 0.06))).add_plugins(plugins).insert_resource(options).insert_resource(fog_look).insert_resource(graphics).add_systems(First, exit_on_signal);
    if offscreen {
        app.add_plugins((ScheduleRunnerPlugin::run_loop(Duration::ZERO), offscreen::OffscreenPlugin));
    }
    app.add_plugins((
            (theme::ThemePlugin, release_brand::BrandPlugin, loading::LoadingPlugin, server_link::ServerLinkPlugin),
            (fog::FogPlugin, cutaway::CutawayPlugin, silhouette::SilhouettePlugin, opening_hints::OpeningHintsPlugin),
            materials::WorldMaterialPlugin,
            world_render::WorldRenderPlugin,
            (daynight::DayNightPlugin, lights::LightsPlugin),
            dev_time::DevTimePlugin,
            (player::PlayerPlugin, interact::InteractPlugin),
            camera::IsoCameraPlugin,
            zombie_render::ZombieRenderPlugin,
            (items_render::ItemsRenderPlugin, vehicles_render::VehiclesRenderPlugin),
            (hud::HudPlugin, inventory_ui::InventoryUiPlugin, health_ui::HealthUiPlugin, death::DeathPlugin, needs_ui::NeedsUiPlugin, action_ui::ActionUiPlugin, skills_ui::SkillsUiPlugin, debug_stats::DebugStatsPlugin, build_ui::BuildUiPlugin),
            (fx::FxPlugin, heard::HeardPlugin, chat::ChatPlugin, film::FilmPlugin, graphics_ui::GraphicsUiPlugin, render_scale::RenderScalePlugin, vegetation::VegetationPlugin, rules_ui::RulesUiPlugin, browser_ui::BrowserUiPlugin, map_ui::MapUiPlugin),
            survivor::SurvivorPlugin,
            (autoplay::AutoplayPlugin, duel::DuelPlugin, holdup::HoldupPlugin, aim_marker::AimMarkerPlugin),
            (saving::SavingPlugin, ui_scale::UiScalePlugin, panels::PanelsPlugin, hotbar::HotbarPlugin, firstnight_ui::FirstNightUiPlugin, settings_ui::SettingsUiPlugin, pause_menu::PauseMenuPlugin),
        ));
    // An offscreen render has no audio device (Bevy's audio is off there).
    if !offscreen {
        app.add_plugins((audio::GameAudioPlugin, music::MusicPlugin));
    }
    app.add_plugins((fps_log::FpsLogPlugin, bench_live::BenchLivePlugin, report::ReportPlugin, biome_watch::BiomeWatchPlugin));
    if black_watch::wanted() && !offscreen {
        app.add_plugins(black_watch::BlackWatchPlugin);
    }
    let failed = app.run().is_error();
    // A join asked for in the server browser (#27): the game has shut down (and autosaved); start
    // it again on that server.
    if let Some(next) = browser_ui::take_relaunch().or_else(main_menu::take_relaunch) {
        relaunch(next);
    }
    if failed {
        std::process::exit(1);
    }
}

/// Bevy's plugins for this run: a window (or, offscreen, none), the assets folder, our own signal
/// handling.
pub(crate) fn window_plugins(options: &Options) -> bevy::app::PluginGroupBuilder {
    let assets = AssetPlugin { file_path: zombye_net::install::assets_dir().to_string_lossy().into_owned(), ..default() };
    if options.offscreen() {
        // No window, ever: no primary window, no winit (on macOS it would make this a foreground
        // app that takes focus), no audio device, no gamepads. The loop is driven by the schedule
        // runner and `offscreen.rs` exits when the picture is written.
        DefaultPlugins
            .set(WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, ..default() })
            .set(assets)
            .set(log_plugin())
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::audio::AudioPlugin>()
            .disable::<bevy::gilrs::GilrsPlugin>()
            .disable::<bevy::app::TerminalCtrlCHandlerPlugin>()
    } else {
        // The window's own settings (the settings screen, `settings_ui.rs`).
        let kept = ui_scale::ClientSettings::load(&ui_scale::ClientSettings::path(&options.saves)).0;
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Zombye".into(),
                    mode: settings_ui::window_mode((kept.fullscreen || window_handoff::fullscreen_asked()) && !options.size_given),
                    resolution: if options.size_given {
                        bevy::window::WindowResolution::new(options.size.x, options.size.y).with_scale_factor_override(1.0)
                    } else {
                        kept.window_size.unwrap_or(window_mode::DEFAULT_SIZE).into()
                    },
                    present_mode: if options.novsync || !kept.vsync { PresentMode::AutoNoVsync } else { PresentMode::AutoVsync },
                    ..default()
                }),
                ..default()
            })
            .set(assets)
            .disable::<bevy::app::TerminalCtrlCHandlerPlugin>()
            .add(window_handoff::HandoffPlugin)
            .add(window_mode::WindowModePlugin { saves: options.saves.clone(), fixed: options.size_given })
    }
}

/// Whether this run is to end in a start-again (`relaunch`).
pub(crate) fn relaunch_pending() -> bool {
    browser_ui::relaunch_asked() || main_menu::relaunch_asked() || new_game::relaunch_asked() || join_screen::relaunch_asked()
}

/// Run this program again with `next`'s arguments, in this one's place.
pub(crate) fn relaunch(next: browser_ui::Relaunch) -> ! {
    let exe = std::env::current_exe().unwrap_or_else(|e| {
        eprintln!("can't start the game again: {e}");
        std::process::exit(1);
    });
    let mut command = std::process::Command::new(exe);
    command.args(&next.args);
    window_handoff::pass_on(&mut command);
    loading::pass_on(&mut command);
    if next.password.is_empty() {
        command.env_remove("ZOMBYE_PASSWORD");
    } else {
        command.env("ZOMBYE_PASSWORD", &next.password);
    }
    #[cfg(unix)]
    {
        // The same process, so whoever started the game (a terminal, a launcher) still has it.
        use std::os::unix::process::CommandExt;
        let e = command.exec();
        eprintln!("can't start the game again: {e}");
        std::process::exit(1);
    }
    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(0)),
            Err(e) => {
                eprintln!("can't start the game again: {e}");
                std::process::exit(1);
            }
        }
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
        assert_eq!((o.size, o.zoom, o.angle, o.player, o.center), (UVec2::new(640, 360), Some(30.0), 2.0, Some(3), Some([10.5, -2.0])));

        let o = Options::parse(&args("--zombies 10"));
        assert!(!o.offscreen());
        assert_eq!((o.size, o.angle, o.player, o.center), (DEFAULT_RENDER_SIZE, 0.0, None, None));
        assert!(!o.size_given && Options::parse(&args("--size 1920x1080")).size_given);
        assert!(!Options::parse(&args("--size big")).size_given);
        assert_eq!(Options::parse(&args("--ui-scale 125")).ui_scale, Some(125));
        assert_eq!(Options::parse(&args("--ui-scale 300")).ui_scale, Some(150));
        assert_eq!((o.ui_scale, Options::parse(&args("--ui-scale x")).ui_scale), (None, None));
        let o = Options::parse(&args("--hotbar bat,-,flashlight --hotbar-press 2@3.5,1@6,x,12 --panel-at skills:40,300"));
        assert_eq!((o.hotbar.as_deref(), o.hotbar_press, o.panel_at.as_deref()), (Some("bat,-,flashlight"), vec![(2, 3.5), (1, 6.0)], Some("skills:40,300")));
        assert_eq!(Options::parse(&args("--angle 5")).angle, 1.0);
        assert_eq!(Options::parse(&args("--angle -1")).angle, 3.0);
        // A fraction is a turn under way; `--rotate` (rotation is always on now) is taken and ignored.
        assert_eq!(Options::parse(&args("--angle 0.5 --rotate")).angle, 0.5);
        assert_eq!(Options::parse(&args("--angle nan")).angle, 0.0);
        assert_eq!(Options::parse(&args("--turn 1@6,-1@2.5,0@1,9,x")).turns, vec![(-1, 2.5), (1, 6.0)]);
        let hints = |a: &str| Options::parse(&args(a)).opening_hints;
        assert_eq!((hints("--opening-hints off"), hints("--opening-hints on"), hints("--zombies 1")), (Some(false), Some(true), None));
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

    #[test]
    fn chat_options_parse() {
        use zombye_sim::{Gesture, Volume};
        let line = ["--say", "Hello there@2.5", "--shout", "RUN!@6", "--say", "mail me a@b", "--gesture", "hands_up@4", "--gesture", "wave", "--gesture", "dance", "--talk-wall"];
        let o = Options::parse(&line.map(String::from));
        assert_eq!(o.say, [(Volume::Talk, "mail me a@b".to_string(), 0.0), (Volume::Talk, "Hello there".to_string(), 2.5), (Volume::Shout, "RUN!".to_string(), 6.0)]);
        assert_eq!(o.gestures, [(Gesture::Wave, 0.0), (Gesture::HandsUp, 4.0)]);
        assert!(o.talk_demo && o.talk_wall);
        let o = Options::parse(&args("--talk-demo"));
        assert!(o.talk_demo && !o.talk_wall && o.say.is_empty() && o.gestures.is_empty());
    }
}
