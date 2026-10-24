//! `zombye-server`: headless authoritative server.
//!
//! Modes:
//! - `zombye-server --mcp` — MCP server on stdio. Time only advances when an agent calls
//!   `step_ticks`, so agent runs are deterministic and reproducible.
//! - `zombye-server --bench [--ticks N] [--players N] [--horde N]` — run N ticks and print timing.
//!   With no players every zombie heads for a debug goal (all of them stay on the active tier);
//!   with players they start on the road west of town (in the forest on a big map), run in
//!   circles and make a 60 m noise every 10 s, `--horde N` spawns N more zombies round the first,
//!   and the report breaks the tick down by tier.
//!
//! Common options: `--zombies N` (default: as many as `population.ron` gives for the map, about 2,600
//! on the default map), `--seed S` (default 1), `--map-chunks N` (a square
//! map N × N chunks of 16 m; default 30, so 480 m; 64 is about 1 km), `--data DIR` (content
//! folder, default the repo's `assets/data`).
//!
//! Saves: `--save PATH` loads that save if it exists (the world options are then ignored) or
//! makes a new world and saves it there; the server then autosaves to it every
//! `--autosave-mins N` minutes of real time (default 5; 0 = never) and when it shuts down (stdin
//! closed, Ctrl-C, SIGTERM or SIGHUP; on Windows also closing the console window). A save made with game data changed since in ways it can't take (a building
//! template that changes the map, a weapon it uses removed) is refused unless `--load-anyway`, which
//! loads it dropping what no longer fits; compatible changes load with a note. `--saves DIR` is the folder for the MCP `save`/`load`/`saves` tools
//! (default: the `--save` file's folder, else `saves`). `--bench --save-bench` also times saving
//! and loading the world at the end of the run.
//!
//! Screenshots (MCP `screenshot`, `screenshot.rs`): `--client PATH` is the game client binary to
//! render with (default `$ZOMBYE_CLIENT`, else `zombye` next to this binary) and
//! `--screenshot-timeout SECS` how long a render may take (default 60).

// The MCP tool list is one big `json!`.
#![recursion_limit = "256"]

mod mcp;
mod screenshot;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use zombye_net::{content, save_file, shutdown};
use glam::Vec2;
use zombye_sim::world::TickStats;
use zombye_sim::{LoadOptions, PlayerInput, TICK_DT, World, WorldConfig};

/// Where the server keeps its world, and when it last saved it.
pub struct Persist {
    /// `--save PATH`: autosaved and saved on shutdown.
    pub path: Option<PathBuf>,
    /// Folder for named saves (MCP `save` / `load` / `saves`).
    pub dir: PathBuf,
    /// The content saves are loaded against (templates, weapons, population).
    pub content: WorldConfig,
    pub autosave: Option<Duration>,
    pub last_save: Instant,
    /// The world changed since it was last saved or loaded.
    pub dirty: bool,
}

impl Persist {
    /// Is an autosave due (something changed and the interval has passed)?
    pub fn autosave_due(&self) -> bool {
        self.dirty && self.path.is_some() && self.autosave.is_some_and(|every| self.last_save.elapsed() >= every)
    }

    /// Save to the `--save` file, if there is one, logging to stderr (stdout may be the MCP pipe).
    pub fn save_now(&mut self, world: &World, why: &str) {
        let Some(path) = self.path.clone() else { return };
        match save_file::save(world, &path) {
            Ok(saved) => eprintln!("{why}: saved tick {} to {saved}", world.tick),
            Err(e) => eprintln!("{why}: {e}"),
        }
        self.last_save = Instant::now();
        self.dirty = false;
    }
}

/// `--save PATH`: load the save there (anyway, with `--load-anyway`), or make a new world from
/// `config`, `prepare` it (players) and save it there.
fn load_or_create(path: &Path, config: WorldConfig, options: LoadOptions, prepare: impl FnOnce(&mut World)) -> World {
    if path.exists() {
        match save_file::load(path, &config, options) {
            Ok((loaded, ms)) => {
                for note in &loaded.notes {
                    eprintln!("note: {note}");
                }
                let world = loaded.world;
                eprintln!("loaded {} in {ms:.0} ms: tick {}, {} zombies, {} players", path.display(), world.tick, world.zombies.len(), world.players.len());
                world
            }
            Err(e) => {
                eprintln!("can't load the save: {}", e.hint("--load-anyway"));
                std::process::exit(1);
            }
        }
    } else {
        let mut world = World::new(config);
        prepare(&mut world);
        match save_file::save(&world, path) {
            Ok(saved) => eprintln!("new world, saved to {saved}"),
            Err(e) => {
                eprintln!("can't save the new world: {e}");
                std::process::exit(1);
            }
        }
        world
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let text = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let value = |name: &str| text(name).and_then(|v| v.parse::<u64>().ok());

    let data = text("--data").map(PathBuf::from).unwrap_or_else(content::default_data_dir);
    let mut config = match content::load_config(&data, value("--seed").unwrap_or(1)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to load content: {e}");
            std::process::exit(1);
        }
    };
    // Without --zombies, as many as the population file's densities give for the map.
    config.zombies = value("--zombies").map(|n| n as u32);
    if let Some(chunks) = value("--map-chunks") {
        config = config.with_map_chunks(chunks.clamp(1, 256) as i32);
    }

    let save_path = text("--save").map(PathBuf::from);
    let load_options = LoadOptions { force: flag("--load-anyway") };
    let mut persist = Persist {
        dir: text("--saves").map(PathBuf::from).or_else(|| save_path.as_ref().and_then(|p| p.parent()).filter(|d| !d.as_os_str().is_empty()).map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from(save_file::DEFAULT_DIR)),
        path: save_path,
        content: config.clone(),
        autosave: Some(text("--autosave-mins").and_then(|v| v.parse::<f64>().ok()).unwrap_or(5.0))
            .filter(|m| *m > 0.0 && m.is_finite())
            .map(|m| Duration::from_secs_f64(m * 60.0)),
        last_save: Instant::now(),
        dirty: false,
    };

    if flag("--mcp") {
        shutdown::install();
        let world = match &persist.path {
            Some(path) => load_or_create(path, config, load_options, mcp::add_agent_player),
            None => World::new(config),
        };
        let timeout = text("--screenshot-timeout").and_then(|v| v.parse::<f64>().ok()).filter(|s| *s > 0.0 && s.is_finite()).map_or(screenshot::DEFAULT_TIMEOUT, Duration::from_secs_f64);
        let renderer = screenshot::Renderer::new(text("--client").map(PathBuf::from), data.clone(), timeout);
        mcp::serve(world, &mut persist, &renderer);
    } else if flag("--bench") {
        let ticks = value("--ticks").unwrap_or(1000);
        bench(config, &mut persist, load_options, ticks, value("--players").unwrap_or(0) as u32, value("--horde").unwrap_or(0) as u32, flag("--save-bench"));
    } else {
        eprintln!(
            "usage: zombye-server (--mcp | --bench [--ticks N] [--players N] [--horde N] [--save-bench]) [--zombies N] [--seed S] [--map-chunks N] [--data DIR] \
             [--save PATH] [--load-anyway] [--autosave-mins N] [--saves DIR] [--client PATH] [--screenshot-timeout SECS]"
        );
        std::process::exit(2);
    }
}

fn bench(config: WorldConfig, persist: &mut Persist, load_options: LoadOptions, ticks: u64, players: u32, horde: u32, save_bench: bool) {
    let zombies = config.zombies.map_or("natural".to_string(), |n| n.to_string());
    let start = Instant::now();
    let loaded = persist.path.as_ref().is_some_and(|p| p.exists());
    let mut world = match &persist.path {
        Some(path) => load_or_create(path, config, load_options, |_| {}),
        None => World::new(config),
    };
    let gen_ms = start.elapsed().as_secs_f32() * 1000.0;
    let mut ids: Vec<u32> = Vec::new();
    if loaded {
        // A saved world keeps its own players (and goal, if it had one).
        ids = world.players.iter().map(|p| p.id).collect();
    } else if players == 0 {
        world.set_goal(Some(world.player_spawn));
    } else {
        let road_y = (world.town_min.y + world.town_max.y) as f32 / 2.0;
        for k in 0..players {
            let p = open_ground_near(&world, Vec2::new(world.town_min.x as f32 * 0.5 + k as f32 * 40.0, road_y));
            if k == 0 && horde > 0 {
                world.spawn_zombies(horde, p, 45.0);
            }
            let id = world.add_player(p);
            // Unkillable, so the run measures a horde on someone rather than a corpse.
            world.players.last_mut().expect("just added").hp = 1e9;
            ids.push(id);
        }
    }
    let start = Instant::now();
    let mut worst: f32 = 0.0;
    let mut sum = TickStats::default();
    for t in 0..ticks {
        for (k, &id) in ids.iter().enumerate() {
            let dir = Vec2::from_angle(t as f32 * 0.01 + k as f32);
            let seq = world.player(id).map_or(1, |p| p.last_input_seq + 1);
            world.set_player_input(id, seq, PlayerInput { move_dir: dir, facing: 0.0, run: true, sneak: false });
            if t % 200 == 0 {
                world.emit_sound(world.player(id).expect("joined").state.pos, 60.0);
            }
        }
        world.step();
        let s = world.stats;
        worst = worst.max(s.tick_ms);
        sum.senses_ms += s.senses_ms;
        sum.flow_ms += s.flow_ms;
        sum.tier_ms += s.tier_ms;
        sum.active_ms += s.active_ms;
        sum.near_ms += s.near_ms;
        sum.virtual_ms += s.virtual_ms;
        sum.promoted += s.promoted;
        sum.demoted += s.demoted;
    }
    let total = start.elapsed().as_secs_f32();
    let n = ticks.max(1) as f32;
    let s = world.stats;
    println!(
        "map {}x{} m, {} buildings, generated in {gen_ms:.0} ms; {} zombies ({zombies} + horde {horde}), {} players, {ticks} ticks ({:.0} s of game time) in {:.2} s: \
         avg {:.3} ms/tick, worst {:.3} ms, budget {:.0} ms, hash {:016x}\n\
         tiers now: {} active, {} near, {} virtual ({} hunting); {} promotions, {} demotions\n\
         per tick: tiers+index {:.3} ms, senses {:.3} ms, flow {:.3} ms, active AI {:.3} ms, near {:.3} ms, virtual {:.3} ms\n\
         population: target {:.0} in {} regions, {} attractors now, {} respawned",
        world.map.width(),
        world.map.height(),
        world.map.buildings.len(),
        world.zombies.len(),
        players,
        ticks as f32 * TICK_DT,
        total,
        total * 1000.0 / n,
        worst,
        TICK_DT * 1000.0,
        world.state_hash(),
        s.active,
        s.near,
        s.virtual_,
        s.chasing,
        sum.promoted,
        sum.demoted,
        sum.tier_ms / n,
        sum.senses_ms / n,
        sum.flow_ms / n,
        sum.active_ms / n,
        sum.near_ms / n,
        sum.virtual_ms / n,
        world.population().total_target(),
        world.regions().regions(),
        world.population().attractors.len(),
        world.population().respawned,
    );
    if save_bench {
        let start = Instant::now();
        let bytes = world.save();
        let save_ms = start.elapsed().as_secs_f32() * 1000.0;
        let start = Instant::now();
        let loaded = World::load(&bytes, &persist.content);
        let load_ms = start.elapsed().as_secs_f32() * 1000.0;
        let same = loaded.as_ref().is_ok_and(|w| w.state_hash() == world.state_hash());
        // Determinism across the save: both copies take the same inputs for a few more seconds.
        let mut later = None;
        if let Ok(mut copy) = loaded.as_ref().map(|_| World::load(&bytes, &persist.content).expect("loaded once")) {
            for t in 0..100u64 {
                for w in [&mut world, &mut copy] {
                    for (k, &id) in ids.iter().enumerate() {
                        let dir = Vec2::from_angle((ticks + t) as f32 * 0.01 + k as f32);
                        let seq = w.player(id).map_or(1, |p| p.last_input_seq + 1);
                        w.set_player_input(id, seq, PlayerInput { move_dir: dir, facing: 0.0, run: true, sneak: false });
                    }
                    w.step();
                }
            }
            later = Some(copy.state_hash() == world.state_hash());
        }
        println!(
            "save: {} ({} bytes, {:.0} per zombie) in {save_ms:.1} ms; load in {load_ms:.1} ms (map regenerated); state hash after load {}, after 100 more ticks {}",
            save_file::size(bytes.len() as u64),
            bytes.len(),
            bytes.len() as f32 / world.zombies.len().max(1) as f32,
            match &loaded {
                Ok(_) if same => "matches".to_string(),
                Ok(_) => "DIFFERS".to_string(),
                Err(e) => format!("not loaded: {e}"),
            },
            match later {
                Some(true) => "matches",
                Some(false) => "DIFFERS",
                None => "-",
            }
        );
    }
    if persist.path.is_some() {
        persist.save_now(&world, "bench done");
    }
}

/// The nearest open outdoor tile to `p`.
fn open_ground_near(world: &World, p: Vec2) -> Vec2 {
    let c = p.floor().as_ivec2();
    for r in 0..60 {
        for dy in -r..=r {
            for dx in -r..=r {
                let t = c + glam::IVec2::new(dx, dy);
                if dx.abs().max(dy.abs()) == r && world.map.in_bounds(t) && !world.map.tile_blocked(t) && world.map.tile(t).room == 0 {
                    return t.as_vec2() + Vec2::splat(0.5);
                }
            }
        }
    }
    world.player_spawn
}
