//! Saving and loading in single-player: F5 quicksaves, F9 quickloads, and quitting autosaves.
//!
//! The world is the local server's, saved with `net::save_file` to `quicksave.zsave` and
//! `autosave.zsave` in the saves folder (`--saves`, default `saves`; `--load NAME` starts from one).
//! A quickload swaps the loaded world into the local server and the client takes it up like a
//! rejoin: the map edits and edge damage that differ arrive as changes (so doors, windows and
//! cracks update), the player's prediction restarts from the loaded state, and zombies and items
//! follow from the first snapshot. The run stats (kills, time alive, distance... and the cause of
//! death, `sim::run`) are the save's own, so a quickload shows the life as it was saved (dead in the
//! save, the death screen), and the blood splats and particles of the timeline left behind are
//! cleared. A quicksave of another map (other seed or size) can't be swapped in under the meshes
//! already built; start the client with `--load` for that. A new run from the death screen
//! (`death.rs`) is taken up the same way ([`WorldSwap`]).

use bevy::app::OnAppExitSystems;
use bevy::ecs::system::SystemParam;
use bevy::input::InputSystems;
use bevy::prelude::*;
use zombye_net::{EdgeDamage, Intent, save_file};
use zombye_sim::World;
use zombye_sim::map::{CHUNK_SIZE, EdgeChange, OwnedEdge, TileMap};

use crate::Options;
use crate::fx::ClearFx;
use crate::player::LocalPlayer;
use crate::server_link::{ClientMap, EdgeChangesThisFrame, EdgeDamageMap, EdgeDamageThisFrame, JoinInfo, ServerLink};

const QUICKSAVE: &str = "quicksave";
const AUTOSAVE: &str = "autosave";
/// Seconds a "Saved" / "Loaded" line stays up.
const NOTICE_SECS: f32 = 4.0;

pub struct SavingPlugin;

impl Plugin for SavingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Notice>()
            .add_systems(Startup, spawn_notice)
            // Before Update, so renderers see this frame's edge changes from a quickload.
            .add_systems(PreUpdate, quick_save_load.after(InputSystems))
            .add_systems(Update, show_notice);
        add_autosave(app);
    }
}

/// The autosave on quitting runs in `Last` after Bevy's own exit systems (closing the window sends
/// its `AppExit` from `Last`, in `ExitSystems`), so whichever way the game quits, the exit that ends
/// the frame is seen before the frame ends. (Unordered, it could run first and miss it.)
fn add_autosave(app: &mut App) {
    app.add_systems(Last, autosave_on_exit.in_set(OnAppExitSystems));
}

/// A short message under the HUD ("Saved ...", "Loaded ...", or what went wrong).
#[derive(Resource, Default)]
pub(crate) struct Notice {
    text: String,
    until: f32,
}

#[derive(Component)]
struct NoticeText;

fn spawn_notice(mut commands: Commands) {
    commands.spawn((
        NoticeText,
        Text::new(""),
        TextFont { font_size: bevy::text::FontSize::Px(18.0), ..default() },
        TextColor(Color::srgb(0.95, 0.85, 0.45)),
        Node { position_type: PositionType::Absolute, top: px(60), left: px(12), ..default() },
    ));
}

fn show_notice(time: Res<Time>, notice: Res<Notice>, mut text: Query<&mut Text, With<NoticeText>>) {
    let Ok(mut t) = text.single_mut() else { return };
    let want = if time.elapsed_secs() < notice.until { notice.text.as_str() } else { "" };
    if t.0 != want {
        t.0 = want.to_string();
    }
}

impl Notice {
    pub(crate) fn say(&mut self, now: f32, text: String) {
        println!("{text}");
        (self.text, self.until) = (text, now + NOTICE_SECS);
    }
}

fn save_to(link: &ServerLink, options: &Options, name: &str) -> Result<save_file::Saved, String> {
    save_file::save(link.world(), &save_file::path_for(&options.saves, name)?)
}

/// What taking up another world touches: the server link, our map copy and edge damage, our
/// player's prediction and id, and the effects of the timeline left behind.
#[derive(SystemParam)]
pub(crate) struct WorldSwap<'w> {
    pub link: ResMut<'w, ServerLink>,
    map: ResMut<'w, ClientMap>,
    changes: ResMut<'w, EdgeChangesThisFrame>,
    damage_map: ResMut<'w, EdgeDamageMap>,
    damage: ResMut<'w, EdgeDamageThisFrame>,
    me: ResMut<'w, LocalPlayer>,
    join: ResMut<'w, JoinInfo>,
    clear_fx: MessageWriter<'w, ClearFx>,
}

impl WorldSwap<'_> {
    /// Our player is dead (the death screen is up).
    pub fn dead(&self) -> bool {
        self.me.dead
    }

    /// Does `world` stand on the ground our meshes were built for (only doors and windows differ)?
    pub fn fits(&self, world: &World) -> bool {
        same_ground(&self.map.0, &world.map)
    }

    /// Serve `world` from now on and take it up like a rejoin, playing `player` (else the one we
    /// played, else its first): the doors and windows that differ arrive as edge changes, every
    /// crack afresh, the prediction restarts from its state, this session's dev toggles carry over,
    /// and blood splats and particles are cleared. `world` must [`fit`](Self::fits).
    pub fn take_up(&mut self, world: World, player: Option<u32>) {
        debug_assert!(self.fits(&world));
        // Doors and windows that differ, as edge changes, and every crack afresh.
        for c in edge_differences(&self.map.0, &world.map) {
            self.map.0.set_owned_edge(c.tile, c.edge, c.kind);
            self.changes.0.push(c);
        }
        let fresh: Vec<EdgeDamage> = EdgeDamage::all(&world);
        for &(tile, edge) in self.damage_map.0.keys() {
            if !fresh.iter().any(|d| d.tile == tile && d.edge == edge) {
                self.damage.0.push(EdgeDamage { tile, edge, hp: 1.0 });
            }
        }
        self.damage_map.0 = fresh.iter().map(|d| ((d.tile, d.edge), d.hp)).collect();
        self.damage.0.extend(fresh);
        // Dev toggles aren't saved; they're this session's, so they carry over to the new world.
        let dev = self.me.dev;
        let want = player.unwrap_or(self.join.player_id);
        let snap = self.link.replace_world(world);
        if let Some(view) = snap.players.iter().find(|p| p.id == want).or(snap.players.first()) {
            self.me.resume(view, snap.tick);
            self.join.player_id = view.id;
        }
        if dev != zombye_sim::DevFlags::default() {
            self.link.send(Intent::Dev { player: self.join.player_id, god: Some(dev.god), ignored: Some(dev.ignored) });
        }
        self.clear_fx.write(ClearFx);
    }
}

fn quick_save_load(keys: Res<ButtonInput<KeyCode>>, time: Res<Time>, options: Res<Options>, mut swap: WorldSwap, mut notice: ResMut<Notice>, mut automated: Local<(bool, bool)>) {
    let now = time.elapsed_secs();
    // `--quicksave-after` / `--quickload-after`: the same as pressing the key, once.
    let due = |at: Option<f32>, done: &mut bool| {
        let fire = !*done && at.is_some_and(|t| now >= t);
        *done |= fire;
        fire
    };
    let save = keys.just_pressed(KeyCode::F5) | due(options.quicksave_after, &mut automated.0);
    let load = keys.just_pressed(KeyCode::F9) | due(options.quickload_after, &mut automated.1);
    if save {
        let text = match save_to(&swap.link, &options, QUICKSAVE) {
            Ok(saved) => format!("Saved: {saved}"),
            Err(e) => format!("Save failed: {e}"),
        };
        notice.say(now, text);
    }
    if !load {
        return;
    }
    let loaded = save_file::path_for(&options.saves, QUICKSAVE).and_then(|path| {
        if !path.exists() {
            return Err(format!("no quicksave yet ({}): press F5 first", path.display()));
        }
        save_file::load(&path, &swap.link.world().config, options.load_options()).map_err(|e| e.hint("starting with --load-anyway"))
    });
    let (loaded, ms) = match loaded {
        Ok(l) => l,
        Err(e) => return notice.say(now, format!("Load failed: {e}")),
    };
    for note in &loaded.notes {
        println!("note: {note}");
    }
    let world = loaded.world;
    if !swap.fits(&world) {
        let g = &world.config.generation;
        return notice.say(now, format!("The quicksave is of another map (seed {}, {}x{} chunks): start with --load {QUICKSAVE}", g.seed, g.chunks.x, g.chunks.y));
    }
    let (tick, zombies) = (world.tick, world.zombies.len());
    swap.take_up(world, None);
    let noted = if loaded.notes.is_empty() { String::new() } else { format!(" ({} notes on stdout: the game data changed)", loaded.notes.len()) };
    notice.say(now, format!("Loaded {QUICKSAVE} in {ms:.0} ms: tick {tick}, {zombies} zombies{noted}"));
}

/// Quitting (closing the window, `--exit-after`, or Ctrl-C, SIGTERM or SIGHUP in the terminal,
/// which `main.rs` turns into an `AppExit`) saves to `autosave`; a second Ctrl-C quits at once,
/// unsaved. An offscreen render (`--render-to`) never saves: it only looks.
fn autosave_on_exit(mut exit: MessageReader<AppExit>, link: Res<ServerLink>, options: Res<Options>, mut done: Local<bool>) {
    if exit.read().next().is_none() || *done {
        return;
    }
    *done = true;
    if !options.offscreen() {
        match save_to(&link, &options, AUTOSAVE) {
            Ok(saved) => println!("Autosaved: {saved}"),
            Err(e) => eprintln!("Autosave failed: {e}"),
        }
    }
    // A Windows console being closed waits for this before the process is ended.
    zombye_net::shutdown::finished();
}

/// Same size, floors, trees, furniture and rooms: only edges (doors, windows) may differ.
fn same_ground(a: &TileMap, b: &TileMap) -> bool {
    if (a.chunks_x, a.chunks_y, a.rooms.len(), a.buildings.len()) != (b.chunks_x, b.chunks_y, b.rooms.len(), b.buildings.len()) {
        return false;
    }
    (0..a.chunks_y).all(|cy| {
        (0..a.chunks_x).all(|cx| {
            let (ta, tb) = (&a.chunk(cx, cy).expect("in range").tiles, &b.chunk(cx, cy).expect("in range").tiles);
            ta.iter().zip(tb).all(|(x, y)| (x.floor, x.feature, x.room) == (y.floor, y.feature, y.room))
        })
    })
}

/// Every owned edge of `to` that differs from `from` (same-sized maps).
fn edge_differences(from: &TileMap, to: &TileMap) -> Vec<EdgeChange> {
    let mut out = Vec::new();
    for cy in 0..from.chunks_y {
        for cx in 0..from.chunks_x {
            let (ta, tb) = (&from.chunk(cx, cy).expect("in range").tiles, &to.chunk(cx, cy).expect("in range").tiles);
            for (k, (x, y)) in ta.iter().zip(tb).enumerate() {
                let tile = IVec2::new(cx * CHUNK_SIZE + k as i32 % CHUNK_SIZE, cy * CHUNK_SIZE + k as i32 / CHUNK_SIZE);
                if x.north != y.north {
                    out.push(EdgeChange { tile, edge: OwnedEdge::North, kind: y.north });
                }
                if x.west != y.west {
                    out.push(EdgeChange { tile, edge: OwnedEdge::West, kind: y.west });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::{ExitCondition, WindowPlugin};

    /// Every way of quitting autosaves: closing the window (Bevy's exit system in `Last`), an
    /// `AppExit` sent during the frame (`--exit-after` from `Update`, Ctrl-C from `First`).
    #[test]
    fn quitting_autosaves_however_it_happens() {
        let dir = std::env::temp_dir().join(format!("zombye-autosave-test-{}", std::process::id()));
        for how in ["window", "update", "first"] {
            let _ = std::fs::remove_dir_all(&dir);
            let mut app = App::new();
            app.add_plugins((MinimalPlugins, WindowPlugin { exit_condition: ExitCondition::OnAllClosed, ..default() }));
            let (link, _) = ServerLink::for_test();
            app.insert_resource(link).insert_resource(Options::parse(&["--saves".to_string(), dir.display().to_string()]));
            add_autosave(&mut app);
            let quit = |mut exit: MessageWriter<AppExit>, mut frames: Local<u32>| {
                *frames += 1;
                if *frames == 2 {
                    exit.write(AppExit::Success);
                }
            };
            match how {
                "update" => app.add_systems(Update, quit),
                "first" => app.add_systems(First, quit),
                _ => &mut app,
            };
            app.update();
            assert!(app.should_exit().is_none(), "{how}: quit too soon");
            if how == "window" {
                let mut windows = app.world_mut().query_filtered::<Entity, With<Window>>();
                let window = windows.single(app.world()).expect("a primary window");
                app.world_mut().despawn(window);
            }
            app.update();
            assert!(app.should_exit().is_some(), "{how}: didn't quit");
            assert!(dir.join("autosave.zsave").exists(), "{how}: no autosave");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
