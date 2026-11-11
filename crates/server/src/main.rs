//! `zombye-server`: headless authoritative server.
//!
//! Modes:
//! - `zombye-server --mcp` — MCP server on stdio. Time only advances when an agent calls
//!   `step_ticks`, so agent runs are deterministic and reproducible.
//! - `zombye-server --mcp-http [ADDR[:PORT]]` — the same MCP server over streamable HTTP
//!   (`mcp_http.rs`, #87), for agents that reach it over the network: `POST /mcp`, by default on
//!   `127.0.0.1:8787`. One world, shared by everyone who connects. Needs a bearer token
//!   (`ZOMBYE_MCP_TOKEN`, or `--mcp-token T`; `--mcp-token-in-url` also takes it as `/mcp/T`) or
//!   `--mcp-no-auth` (open, and then without the debug tools); `--mcp-no-dev` turns the debug
//!   tools off with a token too; `--mcp-origin ORIGIN` allows a browser origin. No TLS: put a
//!   reverse proxy or a tunnel in front.
//! - `zombye-server --bench [--ticks N] [--players N] [--horde N]` — run N ticks and print timing.
//!   With no players every zombie heads for a debug goal (all of them stay on the active tier);
//!   with players they start on the road west of town (in the forest on a big map), run in
//!   circles and make a 60 m noise every 10 s, `--horde N` spawns N more zombies round the first,
//!   and the report breaks the tick down by tier, and says what the worst tick was made of.
//!   `--spread` is a realistic server instead (#70): the players on a grid over the whole map, one
//!   in four running and making a noise now and then, the rest walking.
//!   `--bench --bots N` is that bench over the real transport: a server on a loopback TCP port and
//!   N headless bots, each its own connection (the players' places as above, the bots playing
//!   them), ticking at the real rate; it reports the whole tick (intents, world, snapshots), the
//!   active zombies and what each client was sent.
//! - `zombye-server --host [ADDR[:PORT]]` — the multiplayer server (#21, `multiplayer.rs`,
//!   `zombye_net::host`): game clients join with `zombye --connect HOST[:PORT]`. Listens on every
//!   interface, TCP port 7777, unless told otherwise. `--max-players N` (default 32, at most 64),
//!   `--pvp on|off` (default on: players can hurt each other; off is co-op), `--spawn
//!   spread|together` (default: spread round the town with PvP, together by the first house
//!   without), `--name NAME`, `--password P`, `--allow-dev` (take dev toggles and debug intents
//!   from clients), `--budget KB` (kilobytes a second down to each client, default 32, 8 to
//!   1000: what its snapshots are kept within, #22), `--fair-sight on|off` (#66: a client is told
//!   of another player only while its survivor sees them, and of one it only hears as a sound;
//!   default on with PvP, off in co-op), `--logout-secs S` (default 30: how long a
//!   survivor lies in the world, asleep, after its player leaves), `--logout-safe-secs S` (default
//!   5: the same when it was left somewhere safe), `--spawn-protection S` (default 10: seconds a
//!   new survivor can't be hurt by other players; #68). New survivors start with the data folder's
//!   `start_kit.ron`, or with `--start-kit ITEM[:N],ITEM[:N]...` (`none`: nothing). `--camera
//!   isometric|first-person` and `--agents off|on|marked` are the camera and agents rules (#69:
//!   told to every client that joins, and to anyone who asks with `zombye --server-info HOST`;
//!   clients that say they are programs are refused, let in, or let in and marked). With `--save PATH` the world is loaded or made, autosaved and saved on
//!   shutdown as below, with whose survivor is whose in `PATH.players`.
//!   `--admin-mcp [ADDR[:PORT]]` (#26, `admin.rs`) adds the operator's MCP endpoint to the running
//!   server, by default on `127.0.0.1:8788`: list, kick and ban players, save, change settings, set
//!   off events, read the log. It always needs a bearer token (`ZOMBYE_ADMIN_TOKEN`, or
//!   `--admin-token T`); `--admin-origin ORIGIN` allows a browser origin.
//! - `zombye-server --config FILE` — the dedicated server (#20, `config.rs`, docs/self-hosting.md):
//!   `--host` with its settings read from one RON file (written with the defaults, commented, if
//!   it isn't there yet; `server.example.ron` beside this crate's manifest). The file stands for
//!   the flags above, so a flag on the command line wins over it; its paths are relative to its
//!   folder, and its world is saved in `world.zsave` there unless it says otherwise. This is what
//!   the Docker image runs (`Dockerfile`: `--config /data/server.ron`).
//! - `zombye-server --bots N --connect HOST[:PORT] [--secs S] [--bots-hostile] [--lag MS]` — N headless bot
//!   clients, each on its own connection, that wander and fight; prints what each costs. `--lag MS`
//!   makes them play as if that many milliseconds of ping away (lag compensation, #23).
//! - `zombye-server --benchmark list | table | ID | all` — the agent survival benchmark (#29,
//!   `benchmark.rs`, docs/benchmark.md): fixed scenarios from the data folder's `scenarios.ron`.
//!   `list` prints them; with `--mcp` or `--mcp-http`, `--benchmark ID` serves that scenario's
//!   world to an agent (a player's tools only, one life, time stops when the run is over) and
//!   writes its result as JSON into `--results DIR` (default `saves/benchmark`) under `--agent
//!   NAME`; `--benchmark ID|all --baseline idle|brawler|runner` plays it with a built-in script instead;
//!   `table` ranks the results in the folder.
//!
//! Common options: `--zombies N` (default: as many as `population.ron` gives for the map, about 2,600
//! on the default map), `--seed S` (default 1), `--map-chunks N` (a square
//! map N × N chunks of 16 m; default 30, so 480 m; 64 is about 1 km), `--data DIR` (content
//! folder; default `$ZOMBYE_DATA`, else the repo's `assets/data`, else a `data` folder beside the
//! binary: `content::installed_data_dir`).
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

mod admin;
mod benchmark;
mod config;
mod mcp;
mod mcp_http;
mod multiplayer;
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
    /// The benchmark scenario this MCP session plays (`--benchmark ID`, #29), if it plays one.
    pub benchmark: Option<benchmark::Run>,
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
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let admin_token = std::env::var("ZOMBYE_ADMIN_TOKEN").ok().filter(|t| !t.trim().is_empty());
    // `--config FILE` (#20, `config.rs`): the dedicated server's settings from one file, as the
    // flags they stand for, behind the command line's own.
    if let Some(at) = args.iter().position(|a| a == "--config") {
        let Some(path) = args.get(at + 1).filter(|p| !p.starts_with("--")).map(PathBuf::from) else {
            eprintln!("--config needs a file: --config server.ron");
            std::process::exit(2);
        };
        match config::load(&path, &args, admin_token.is_some()) {
            Ok(loaded) => {
                for note in &loaded.notes {
                    eprintln!("{note}");
                }
                args = loaded.args;
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        }
    }
    let flag = |name: &str| args.iter().any(|a| a == name);
    let text = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let value = |name: &str| text(name).and_then(|v| v.parse::<u64>().ok());

    let data = text("--data").map(PathBuf::from).unwrap_or_else(content::installed_data_dir);
    let mut config = match content::load_config(&data, value("--seed").unwrap_or(1)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to load content: {e}");
            std::process::exit(1);
        }
    };
    // Mods (#25, docs/modding.md): `--mod DIR`, as often as wanted, in order. Their data joins the
    // game's and their Luau scripts run in every world made from this config.
    let mod_dirs: Vec<PathBuf> = args.windows(2).filter(|w| w[0] == "--mod").map(|w| PathBuf::from(&w[1])).collect();
    // What a hosting server tells whoever asks what it runs (#27: the server browser's mods), and
    // hands to the clients that join without them (#24): each mod's data, named by its hash.
    let mod_packs: Vec<zombye_net::modpack::ModPack> = match zombye_mods::load_into(&mut config, &mod_dirs) {
        Ok(mods) => {
            if !mods.is_empty() {
                eprintln!("{}", zombye_mods::describe(&mods));
            }
            mods.iter().map(zombye_mods::Mod::pack).collect()
        }
        Err(e) => {
            eprintln!("failed to load mods: {e}");
            std::process::exit(1);
        }
    };
    // What a new world's containers hold: rolled from a fresh seed each time, so every game finds
    // different things, unless `--loot-seed S` fixes it (a loaded save keeps its own).
    config.loot_seed = Some(value("--loot-seed").unwrap_or_else(content::fresh_loot_seed));
    // Without --zombies, as many as the population file's densities give for the map.
    config.zombies = value("--zombies").map(|n| n as u32);
    // Player against player (docs/PLAN.md §4.8) for a new world: `--pvp on` (a loaded save keeps
    // its own setting; `--host` decides for itself, on unless `--pvp off`).
    config.combat.pvp = text("--pvp").as_deref() == Some("on");
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
        benchmark: None,
    };

    // The benchmark (#29, `benchmark.rs`): `list`, `table`, or a scenario to play, by a baseline
    // here or over MCP below (on the scenario's world instead of the one the options describe).
    let mut scenario_world = None;
    if let Some(what) = text("--benchmark") {
        let results = text("--results").map_or_else(|| PathBuf::from(benchmark::DEFAULT_DIR), PathBuf::from);
        let fail = |e: String| -> ! {
            eprintln!("{e}");
            std::process::exit(2);
        };
        if !mod_dirs.is_empty() {
            // Results are compared across runs of the same game: a mod's scripts would change it.
            fail("--benchmark plays the game as shipped: it can't be used with --mod".to_string());
        }
        let defs = content::load_scenarios(&data, &config.items, &config.weapons).unwrap_or_else(|e| fail(e));
        let mcp = flag("--mcp") || flag("--mcp-http");
        if what == "list" {
            print!("{}", benchmark::list(&defs));
            return;
        }
        if what == "table" {
            print!("{}", benchmark::table(&results).unwrap_or_else(|e| fail(e)));
            return;
        }
        let chosen: Vec<_> = defs.iter().filter(|d| d.id == what || (what == "all" && !mcp)).collect();
        if chosen.is_empty() {
            fail(format!("no scenario {what:?} (there are: {}; or list, table{})", defs.iter().map(|d| d.id.as_str()).collect::<Vec<_>>().join(", "), if mcp { "" } else { ", all" }));
        }
        if mcp {
            if persist.path.is_some() {
                fail("--benchmark starts its scenario's own world: it can't be used with --save".to_string());
            }
            let (world, player) = chosen[0].start(&config);
            eprintln!("benchmark: scenario {} ({}), results to {}", chosen[0].id, chosen[0].name, results.display());
            persist.benchmark = Some(benchmark::Run::new(chosen[0].clone(), &world, player, text("--agent").unwrap_or_else(|| "unnamed".into()), results));
            persist.autosave = None;
            scenario_world = Some(world);
        } else {
            let Some(name) = text("--baseline") else {
                fail(format!("--benchmark {what} needs --mcp or --mcp-http (an agent plays it) or --baseline {} (a built-in script does)", benchmark::BASELINES.join("|")));
            };
            for def in chosen {
                let start = Instant::now();
                let result = benchmark::baseline(def, &config, &name).unwrap_or_else(|e| fail(e));
                let wrote = benchmark::write_result(&results, &result).map_or_else(|e| format!("not written: {e}"), |p| p.display().to_string());
                println!("{}\n  {:.1} s, {} ticks, hash {}; {wrote}", benchmark::line(&result), start.elapsed().as_secs_f32(), result["ticks"], result["state_hash"].as_str().unwrap_or(""));
            }
            return;
        }
    }
    if flag("--mcp-http") {
        // The same server as `--mcp`, reached over HTTP instead of stdio (#87, `mcp_http.rs`).
        let http = match mcp_http::HttpArgs::parse(&args, std::env::var("ZOMBYE_MCP_TOKEN").ok()) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        };
        shutdown::install();
        let world = match &persist.path {
            Some(path) => load_or_create(path, config, load_options, mcp::add_agent_player),
            None => scenario_world.take().unwrap_or_else(|| World::new(config)),
        };
        let timeout = text("--screenshot-timeout").and_then(|v| v.parse::<f64>().ok()).filter(|s| *s > 0.0 && s.is_finite()).map_or(screenshot::DEFAULT_TIMEOUT, Duration::from_secs_f64);
        let renderer = screenshot::Renderer::new(text("--client").map(PathBuf::from), data.clone(), timeout).with_mods(&mod_dirs);
        mcp_http::serve(world, &mut persist, &renderer, &http);
    } else if flag("--mcp") {
        shutdown::install();
        let world = match &persist.path {
            Some(path) => load_or_create(path, config, load_options, mcp::add_agent_player),
            None => scenario_world.take().unwrap_or_else(|| World::new(config)),
        };
        let timeout = text("--screenshot-timeout").and_then(|v| v.parse::<f64>().ok()).filter(|s| *s > 0.0 && s.is_finite()).map_or(screenshot::DEFAULT_TIMEOUT, Duration::from_secs_f64);
        let renderer = screenshot::Renderer::new(text("--client").map(PathBuf::from), data.clone(), timeout).with_mods(&mod_dirs);
        mcp::serve(world, &mut persist, &renderer, mcp::Options { debug_tools: !flag("--no-debug-tools"), ..mcp::Options::default() });
    } else if flag("--bench") && flag("--bots") {
        // The bench over the network: a real server on a loopback port, each bot its own TCP connection.
        let bots = value("--bots").unwrap_or(1).clamp(1, zombye_net::wire::MAX_PLAYERS as u64) as u32;
        multiplayer::bench(World::new(config), bots, value("--horde").unwrap_or(0) as u32, flag("--spread"), value("--ticks").unwrap_or(400), value("--lag").unwrap_or(0).min(2000) as u32);
    } else if flag("--bots") {
        let Some(addr) = text("--connect") else {
            eprintln!("--bots N needs --connect HOST[:PORT]");
            std::process::exit(2);
        };
        multiplayer::bots(multiplayer::BotArgs {
            addr,
            count: value("--bots").unwrap_or(1).clamp(1, zombye_net::wire::MAX_PLAYERS as u64) as usize,
            secs: text("--secs").and_then(|v| v.parse::<f32>().ok()).filter(|s| *s > 0.0),
            name: text("--name").unwrap_or_else(|| "bot".into()),
            password: text("--password").unwrap_or_default(),
            hostile: flag("--bots-hostile"),
            lag_ms: value("--lag").unwrap_or(0).min(2000) as u32,
            stop: None,
        });
    } else if flag("--host") {
        // The flags, and what new survivors start with (the data folder's `start_kit.ron`, #68).
        let parsed = multiplayer::HostArgs::parse(&args, admin_token).and_then(|mut a| {
            // (`--start-kit`, or `start_kit` in the config file, says it instead: #69.)
            a.options.kit = a.kit(content::load_start_kit(&data, &config.items)?, &config.items)?;
            for p in &mod_packs {
                eprintln!("mod {}: {} of data for joining clients to download, sha256 {}", p.id, zombye_net::modpack::kb(p.size()), zombye_net::modpack::hex(&p.hash()));
            }
            for id in a.options.with_mods(mod_packs) {
                eprintln!("mod {id} is too big to hand out (over {}): clients need it installed and --mod", zombye_net::modpack::kb(zombye_net::modpack::MAX_PACK_BYTES));
            }
            Ok(a)
        });
        let host_args = match parsed {
            Ok(a) => a,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        };
        let new_world = !persist.path.as_ref().is_some_and(|p| p.exists());
        let world = match &persist.path {
            Some(path) => load_or_create(path, config, load_options, |_| {}),
            None => World::new(config),
        };
        multiplayer::host(world, new_world, host_args, &mut persist);
    } else if flag("--bench") {
        let ticks = value("--ticks").unwrap_or(1000);
        bench(config, &mut persist, load_options, ticks, value("--players").unwrap_or(0) as u32, value("--horde").unwrap_or(0) as u32, flag("--spread"), flag("--save-bench"));
    } else {
        eprintln!(
            "usage: zombye-server (--config FILE | --mcp | --mcp-http [ADDR[:PORT]] [--mcp-token T | --mcp-no-auth] [--mcp-token-in-url] [--mcp-no-dev] [--mcp-origin ORIGIN] | --bench [--ticks N] [--players N] [--horde N] [--save-bench] \
             | --host [ADDR[:PORT]] [--name NAME] [--max-players N] [--pvp on|off] [--spawn spread|together] [--password P] [--allow-dev] [--budget KB] [--fair-sight on|off] \
             [--logout-secs S] [--logout-safe-secs S] [--spawn-protection S] [--admin-mcp [ADDR[:PORT]] [--admin-token T] [--admin-origin ORIGIN]] \
             | --bots N --connect HOST[:PORT] [--secs S] [--name PREFIX] [--password P] [--bots-hostile] [--lag MS] \
             | --benchmark list|table|ID|all [--baseline idle|brawler|runner] [--agent NAME] [--results DIR] (ID with --mcp or --mcp-http: an agent plays it)) \
             [--zombies N] [--seed S] [--map-chunks N] [--pvp on] [--data DIR] \
             [--save PATH] [--load-anyway] [--autosave-mins N] [--saves DIR] [--client PATH] [--screenshot-timeout SECS] [--no-debug-tools]"
        );
        std::process::exit(2);
    }
}

fn bench(config: WorldConfig, persist: &mut Persist, load_options: LoadOptions, ticks: u64, players: u32, horde: u32, spread: bool, save_bench: bool) {
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
        ids = place_bench_players(&mut world, players, horde, spread);
        for p in &mut world.players {
            // Unkillable, so the run measures a horde on someone rather than a corpse.
            p.hp = 1e9;
        }
    }
    let start = Instant::now();
    let mut worst: f32 = 0.0;
    let (mut worst_at, mut worst_stats) = (0u64, TickStats::default());
    let mut over = [0u32; 2];
    let mut sum = TickStats::default();
    for t in 0..ticks {
        for (k, &id) in ids.iter().enumerate() {
            let dir = Vec2::from_angle(t as f32 * 0.01 + k as f32);
            let seq = world.player(id).map_or(1, |p| p.last_input_seq + 1);
            // Spread out: a realistic server. One in four runs, and now and then one of them makes
            // a loud noise (staggered); the rest walk.
            let run = !spread || k % 4 == 0;
            world.set_player_input(id, seq, PlayerInput { move_dir: dir, facing: 0.0, run, sneak: false, aim: false });
            let noisy = if spread { k % 4 == 0 && (t + k as u64 * 25) % 400 == 0 } else { t % 200 == 0 };
            if noisy {
                world.emit_sound(world.player(id).expect("joined").state.pos, 60.0);
            }
        }
        world.step();
        let s = world.stats;
        if s.tick_ms > worst {
            (worst, worst_at, worst_stats) = (s.tick_ms, t, s);
        }
        over[0] += (s.tick_ms > TICK_DT * 500.0) as u32;
        over[1] += (s.tick_ms > TICK_DT * 1000.0) as u32;
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
    let w = worst_stats;
    println!(
        "worst tick ({worst_at}): tiers+index {:.3} ms ({} promoted, {} demoted), senses {:.3} ms, flow {:.3} ms ({} fields), nav {:.3} ms ({} plans), active AI {:.3} ms ({} active), near {:.3} ms, virtual {:.3} ms; \
         {} ticks over half the budget, {} over it",
        w.tier_ms, w.promoted, w.demoted, w.senses_ms, w.flow_ms, w.flow_builds, w.nav_ms, w.nav_plans, w.active_ms, w.active, w.near_ms, w.virtual_ms, over[0], over[1],
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
                        w.set_player_input(id, seq, PlayerInput { move_dir: dir, facing: 0.0, run: true, sneak: false, aim: false });
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

/// The bench's players: in a row 40 m apart on the road west of town, or with `spread` on a grid
/// over the whole map, with `horde` more zombies round the first. Returns their ids.
pub fn place_bench_players(world: &mut World, players: u32, horde: u32, spread: bool) -> Vec<u32> {
    let road_y = (world.town_min.y + world.town_max.y) as f32 / 2.0;
    let mut ids = Vec::new();
    for k in 0..players {
        let at = if spread { spread_point(world, k, players) } else { Vec2::new(world.town_min.x as f32 * 0.5 + k as f32 * 40.0, road_y) };
        let p = open_ground_near(world, at);
        if k == 0 && horde > 0 {
            world.spawn_zombies(horde, p, 45.0);
        }
        ids.push(world.add_player(p));
    }
    ids
}

/// Where the `k`th of `n` bench players starts with `--spread`: a grid over the whole map, 60 m
/// in from its edges, so they are in the town, on its edge and out in the forest.
fn spread_point(world: &World, k: u32, n: u32) -> Vec2 {
    let cols = (n as f32).sqrt().ceil().max(1.0) as u32;
    let rows = n.div_ceil(cols).max(1);
    let (w, h) = (world.map.width() as f32, world.map.height() as f32);
    let margin = 60.0f32.min(w * 0.25).min(h * 0.25);
    let cell = Vec2::new((w - 2.0 * margin) / cols as f32, (h - 2.0 * margin) / rows as f32);
    Vec2::splat(margin) + cell * Vec2::new((k % cols) as f32 + 0.5, (k / cols) as f32 + 0.5)
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
