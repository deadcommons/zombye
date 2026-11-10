//! `zombye-server --host`: the multiplayer server (#21), and `--bots N --connect ADDR`: headless
//! bot clients that join one, for filling a server and measuring it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use glam::Vec2;
use zombye_net::host::{BUDGET_RANGE, DEFAULT_BUDGET, Host, HostOptions, Spawn};
use zombye_net::remote::Remote;
use zombye_net::{Intent, Snapshot, shutdown, wire};
use zombye_sim::logout::StartKit;
use zombye_sim::{TICK_DT, World, ZombieAnim};

use crate::Persist;
use crate::admin::{Admin, Log};
use crate::mcp_http::HttpArgs;

/// What the command line says about hosting.
pub struct HostArgs {
    /// `--host [ADDR]`: where to listen (default every interface, port 7777).
    pub addr: String,
    pub options: HostOptions,
    /// `--pvp on|off`: `None` leaves a loaded world's rule as it is (a new world: on).
    pub pvp: Option<bool>,
    /// `--start-kit LIST`: what new survivors start with, instead of the data folder's
    /// `start_kit.ron` (#69). `None`: the data folder's.
    pub start_kit: Option<StartKit>,
    /// `--admin-mcp`: the operator's MCP endpoint (#26, `admin.rs`), if asked for.
    pub admin: Option<HttpArgs>,
}

impl HostArgs {
    /// From the command line and `ZOMBYE_ADMIN_TOKEN` (`admin_token`).
    pub fn parse(args: &[String], admin_token: Option<String>) -> Result<Self, String> {
        let text = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).filter(|v| !v.starts_with("--")).cloned();
        let flag = |name: &str| args.iter().any(|a| a == name);
        let on_off = |name: &str| match text(name).as_deref() {
            None => Ok(None),
            Some("on" | "true" | "yes") => Ok(Some(true)),
            Some("off" | "false" | "no") => Ok(Some(false)),
            Some(v) => Err(format!("{name} is on or off, not {v}")),
        };
        let addr = match text("--host") {
            None => format!("0.0.0.0:{}", wire::DEFAULT_PORT),
            Some(a) if a.parse::<u16>().is_ok() => format!("0.0.0.0:{a}"),
            Some(a) if a.contains(':') => a,
            Some(a) => format!("{a}:{}", wire::DEFAULT_PORT),
        };
        let max_players = match text("--max-players") {
            None => wire::DEFAULT_PLAYERS,
            Some(v) => v.parse::<usize>().ok().filter(|n| (1..=wire::MAX_PLAYERS).contains(n)).ok_or(format!("--max-players is 1 to {}, not {v}", wire::MAX_PLAYERS))?,
        };
        let spawn = match text("--spawn").as_deref() {
            None => None,
            Some("spread") => Some(Spawn::Spread),
            Some("together") => Some(Spawn::Together),
            Some(v) => return Err(format!("--spawn is spread or together, not {v}")),
        };
        let (least, most) = (BUDGET_RANGE.0 / 1000, BUDGET_RANGE.1 / 1000);
        let budget = match text("--budget") {
            None => DEFAULT_BUDGET,
            Some(v) => v.parse::<u32>().ok().filter(|kb| (least..=most).contains(kb)).ok_or(format!("--budget is {least} to {most} (KB a second per client), not {v}"))? * 1000,
        };
        // Seconds, 0 to an hour (#68).
        let secs = |name: &str, default: f32| match text(name) {
            None => Ok(default),
            Some(v) => v.parse::<f32>().ok().filter(|s| (0.0..=zombye_sim::logout::MAX_LOGOUT_SECS).contains(s)).ok_or(format!("{name} is seconds, 0 to 3600, not {v}")),
        };
        let defaults = HostOptions::default();
        let logout_secs = secs("--logout-secs", defaults.logout_secs)?;
        let options = HostOptions {
            name: text("--name").unwrap_or_else(|| "Zombye server".into()),
            max_players,
            allow_dev: flag("--allow-dev"),
            password: text("--password").unwrap_or_default(),
            spawn,
            logout_secs,
            logout_safe_secs: secs("--logout-safe-secs", defaults.logout_safe_secs)?.min(logout_secs),
            spawn_protection_secs: secs("--spawn-protection", defaults.spawn_protection_secs)?,
            kit: Vec::new(),
            budget,
            fair_sight: on_off("--fair-sight")?,
            camera: match text("--camera") {
                None => wire::Camera::default(),
                Some(v) => wire::Camera::parse(&v).ok_or(format!("--camera is isometric or first-person, not {v}"))?,
            },
            agents: match text("--agents") {
                None => wire::Agents::default(),
                Some(v) => wire::Agents::parse(&v).ok_or(format!("--agents is off, on or marked, not {v}"))?,
            },
            // The mods loaded with `--mod`, filled in by `main` (it reads them).
            mods: Vec::new(),
            packs: Vec::new(),
        };
        let start_kit = text("--start-kit").map(|v| parse_kit(&v)).transpose()?;
        Ok(Self { addr, options, pvp: on_off("--pvp")?, start_kit, admin: crate::admin::parse(args, admin_token)? })
    }

    /// What new survivors start with: `--start-kit` if given, else the data folder's
    /// `start_kit.ron` (`from_data`), checked against the items either way.
    pub fn kit(&self, from_data: StartKit, items: &[zombye_sim::ItemDef]) -> Result<StartKit, String> {
        match &self.start_kit {
            None => Ok(from_data),
            Some(kit) => zombye_sim::logout::check_start_kit(kit.clone(), items).map_err(|e| format!("--start-kit: {e}")),
        }
    }
}

/// `--start-kit ITEM[:N],ITEM[:N]...` (`none`: nothing): item ids and how many of each. The ids
/// are checked against the game data later (`HostArgs::kit`).
fn parse_kit(text: &str) -> Result<StartKit, String> {
    if text.trim().eq_ignore_ascii_case("none") {
        return Ok(StartKit::new());
    }
    text.split(',')
        .map(|part| {
            let (item, count) = match part.trim().split_once(':') {
                Some((item, n)) => (item.trim(), n.trim().parse::<u16>().map_err(|_| format!("--start-kit: {part}: the count is a number, as in rag:2"))?),
                None => (part.trim(), 1),
            };
            if item.is_empty() {
                return Err(format!("--start-kit is item ids with counts, as in water_bottle,rag:2 (or none), not {text}"));
            }
            Ok((item.to_string(), count))
        })
        .collect()
}

/// The file beside a save that says whose each survivor is: a line per name, `id<TAB>name`.
fn names_path(save: &Path) -> PathBuf {
    let mut name = save.as_os_str().to_os_string();
    name.push(".players");
    PathBuf::from(name)
}

fn read_names(save: &Path) -> Vec<(String, u32)> {
    let Ok(text) = std::fs::read_to_string(names_path(save)) else { return Vec::new() };
    text.lines().filter_map(|l| l.split_once('\t')).filter_map(|(id, name)| Some((name.to_string(), id.parse().ok()?))).collect()
}

pub fn write_names(save: &Path, host: &Host) {
    let text: String = host.names().iter().map(|(name, id)| format!("{id}\t{name}\n")).collect();
    if let Err(e) = std::fs::write(names_path(save), text) {
        eprintln!("can't write {}: {e}", names_path(save).display());
    }
}

/// Serve `world` until told to stop (Ctrl-C, SIGTERM, SIGHUP), autosaving as `persist` says.
pub fn host(mut world: World, new_world: bool, args: HostArgs, persist: &mut Persist) {
    // PvP is on unless the server says otherwise (docs/PLAN.md §4.8); a loaded world keeps its rule.
    if let Some(pvp) = args.pvp.or(new_world.then_some(true)) {
        world.config.combat.pvp = pvp;
    }
    shutdown::install();
    let mut host = match Host::bind(world, &args.addr, args.options) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("can't listen on {}: {e}", args.addr);
            std::process::exit(1);
        }
    };
    if let Some(save) = &persist.path {
        host.set_names(read_names(save));
    }
    // Survivors a loaded world has standing about with nobody playing them lie down and leave.
    let unplayed = host.log_out_unplayed();
    if unplayed > 0 {
        println!("{unplayed} survivors with nobody playing them: logged out");
    }
    let rules = host.rules();
    println!(
        "{}: listening on {} ({} players at most, {} KB/s down each, pvp {}, {}{}{}; logout {:.0} s, {:.0} s somewhere safe; spawn protection {:.0} s; start kit: {}); {} zombies on a {}x{} m map, seed {}",
        rules.name,
        host.local_addr().map_or(args.addr.clone(), |a| a.to_string()),
        rules.max_players,
        host.options.budget / 1000,
        if rules.pvp { "on" } else { "off: co-op" },
        if rules.fair_sight { "other players only when seen or heard" } else { "other players always shown" },
        if rules.allow_dev { ", dev intents allowed" } else { "" },
        if host.options.password.is_empty() { "" } else { ", password set" },
        rules.logout_secs,
        rules.logout_safe_secs,
        if rules.pvp { rules.spawn_protection_secs } else { 0.0 },
        if host.options.kit.is_empty() { "none".to_string() } else { host.options.kit.iter().map(|(item, n)| format!("{n} {item}")).collect::<Vec<_>>().join(", ") },
        host.world.zombies.len(),
        host.world.map.width(),
        host.world.map.height(),
        host.world.config.generation.seed,
    );
    // The rules as players are told them (#69), a line each.
    for (label, text) in rules.lines() {
        println!("  {label}: {text}");
    }
    if rules.camera == wire::Camera::FirstPerson {
        println!("  note: no game client has the first-person camera yet (#71), so the game's own client won't join this server; headless clients (bots, agents) still can");
    }
    // The operator's side (#26, `admin.rs`): the log, and the admin MCP endpoint if asked for.
    let mut admin = Admin::default();
    if let Some(http) = &args.admin {
        match admin.listen(http) {
            Ok(addr) => println!("admin MCP on http://{addr}/mcp (bearer token). No TLS here: put a reverse proxy or a tunnel in front for HTTPS."),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
    let mut report = Report::new(&host);
    host.run(|h| {
        // What the host logged, deaths, and the admin calls waiting, between two ticks.
        persist.dirty |= admin.tick(h, persist);
        persist.dirty |= !h.clients().is_empty();
        if persist.autosave_due() {
            persist.save_now(&h.world, "autosave");
            if let Some(save) = &persist.path {
                write_names(save, h);
                admin.log.add(h.world.tick as u64, format!("autosaved to {}", save.display()));
            }
        }
        report.tick(h, &mut admin.log);
        !shutdown::requested()
    });
    host.close("the server is shutting down");
    if let Some(save) = persist.path.clone() {
        persist.save_now(&host.world, "shutting down");
        write_names(&save, &host);
    }
}

/// A status line every ten seconds while anyone is on.
struct Report {
    since: Instant,
    ticks: u64,
    bytes_out: u64,
    held: u64,
    /// Each client's bytes out when the count began, by player.
    clients: Vec<(u32, u64)>,
    ms: f32,
    worst: f32,
}

impl Report {
    fn new(host: &Host) -> Self {
        let clients = host.clients().iter().map(|c| (c.player, c.bytes_out)).collect();
        Self { since: Instant::now(), ticks: host.stats.ticks, bytes_out: host.stats.bytes_out, held: host.stats.held, clients, ms: 0.0, worst: 0.0 }
    }

    fn tick(&mut self, host: &Host, log: &mut Log) {
        self.ms += host.stats.tick_ms;
        self.worst = self.worst.max(host.stats.tick_ms);
        let secs = self.since.elapsed().as_secs_f32();
        if secs < 10.0 {
            return;
        }
        let ticks = (host.stats.ticks - self.ticks).max(1);
        let online = host.clients().len();
        if online > 0 {
            let s = host.world.stats;
            let clients = host.clients();
            // The busiest client (one that joined since the count began: all it was sent), and
            // the fullest view.
            let busiest = clients.iter().map(|c| c.bytes_out.saturating_sub(self.clients.iter().find(|(p, _)| *p == c.player).map_or(0, |(_, b)| *b))).max().unwrap_or(0);
            log.add(host.world.tick as u64, format!(
                "tick {}: {online} online | {:.1} ticks/s, {:.2} ms a tick (worst {:.2}, budget {:.0}) | zombies {} active, {} near, {} virtual | {:.1} KB/s down per client (busiest {:.1}, budget {}) | up to {} zombies in a view, {:.0} updates a tick held back | blows rewound up to {:.0} ms (the most a client's view is behind; cap {:.0})",
                host.world.tick,
                ticks as f32 / secs,
                self.ms / ticks as f32,
                self.worst,
                TICK_DT * 1000.0,
                s.active,
                s.near,
                s.virtual_,
                (host.stats.bytes_out - self.bytes_out) as f32 / secs / online as f32 / 1000.0,
                busiest as f32 / secs / 1000.0,
                host.options.budget / 1000,
                clients.iter().map(|c| c.zombies).max().unwrap_or(0),
                (host.stats.held - self.held) as f32 / ticks as f32,
                clients.iter().map(|c| host.world.view_lag_secs(c.player)).fold(0.0, f32::max) * 1000.0,
                host.world.config.combat.lag_comp_max_secs * 1000.0,
            ));
        }
        *self = Self::new(host);
    }
}

/// What the command line says about bots.
pub struct BotArgs {
    pub addr: String,
    pub count: usize,
    /// How long they play (`--secs`; default until stopped).
    pub secs: Option<f32>,
    pub name: String,
    pub password: String,
    /// `--bots-hostile`: they go for other players as they go for zombies (on a PvP server).
    pub hostile: bool,
    /// `--lag MS`: they play as if that many milliseconds of ping away (#23).
    pub lag_ms: u32,
    /// Set from outside to make them leave (the bench); `None`: only a shutdown signal does.
    pub stop: Option<Arc<AtomicBool>>,
}

/// `--bench --bots N`: the server bench over the real transport (#70). `world` is served on a
/// loopback TCP port; `players` survivors are placed as the bench places them (`spread`: on a grid
/// over the whole map; `horde` more zombies round the first) and unkillable, and as many bots
/// join, each on its own connection, and play them from another thread. The server ticks at the
/// real rate; once every bot is on, `ticks` ticks are measured: the whole tick (intents, the world,
/// every client's snapshot), the active zombies and the bytes each client was sent.
pub fn bench(mut world: World, players: u32, horde: u32, spread: bool, ticks: u64, lag_ms: u32) {
    world.config.combat.pvp = true;
    let zombies = world.zombies.len();
    let ids = crate::place_bench_players(&mut world, players, horde, spread);
    for &id in &ids {
        world.set_dev(id, Some(true), None);
    }
    let options = HostOptions { name: "bench".into(), max_players: players as usize, ..HostOptions::default() };
    let mut host = Host::bind(world, "127.0.0.1:0", options).expect("a free loopback port");
    // Each bot takes up the survivor placed for it.
    host.set_names(ids.iter().enumerate().map(|(k, &id)| (format!("bot-{}", k + 1), id)));
    let addr = host.local_addr().expect("bound").to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let args = BotArgs { addr: addr.clone(), count: players as usize, secs: None, name: "bot".into(), password: String::new(), hostile: false, lag_ms, stop: Some(stop.clone()) };
    let playing = std::thread::Builder::new().name("bots".into()).spawn(move || bots(args)).expect("a thread for the bots");
    println!(
        "bench over TCP on {addr}: {}x{} m map, {zombies} zombies + horde {horde}, {players} bots {}, {ticks} ticks at {:.0} a second",
        host.world.map.width(),
        host.world.map.height(),
        if spread { "spread over the map" } else { "in a row" },
        1.0 / TICK_DT,
    );
    // Until everyone is on: ticks that aren't counted (reported apart: joining is its own load).
    let (mut joining, mut joining_worst) = (0u64, 0.0f32);
    let deadline = Instant::now() + Duration::from_secs(120);
    host.run(|h| {
        joining += 1;
        joining_worst = joining_worst.max(h.stats.tick_ms);
        h.log.clear();
        h.clients().len() < players as usize && Instant::now() < deadline && !playing.is_finished()
    });
    if host.clients().len() < players as usize {
        eprintln!("only {} of {players} bots joined", host.clients().len());
        std::process::exit(1);
    }
    let before: Vec<(u32, u64, u64)> = host.clients().iter().map(|c| (c.player, c.bytes_out, c.bytes_in)).collect();
    let held0 = host.stats.held;
    let start = Instant::now();
    let (mut n, mut sum, mut sim, mut worst, mut worst_sim) = (0u64, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let (mut worst_at, mut worst_stats) = (0u64, host.world.stats);
    let (mut active, mut most_active, mut over, mut views) = (0u64, 0u32, [0u32; 2], 0u32);
    host.run(|h| {
        let (ms, s) = (h.stats.tick_ms, h.world.stats);
        n += 1;
        sum += ms;
        sim += h.stats.sim_ms;
        worst_sim = worst_sim.max(h.stats.sim_ms);
        if ms > worst {
            (worst, worst_at, worst_stats) = (ms, n, s);
        }
        active += s.active as u64;
        most_active = most_active.max(s.active);
        over[0] += (ms > TICK_DT * 500.0) as u32;
        over[1] += (ms > TICK_DT * 1000.0) as u32;
        if n % 20 == 0 {
            views = views.max(h.clients().iter().map(|c| c.zombies).max().unwrap_or(0));
        }
        h.log.clear();
        n < ticks
    });
    let secs = start.elapsed().as_secs_f32();
    let clients = host.clients();
    let sent = |c: &zombye_net::host::ClientInfo| before.iter().find(|b| b.0 == c.player).map_or((0, 0), |b| (c.bytes_out - b.1, c.bytes_in - b.2));
    let online = clients.len().max(1) as f32;
    let down: u64 = clients.iter().map(|c| sent(c).0).sum();
    let up: u64 = clients.iter().map(|c| sent(c).1).sum();
    let busiest = clients.iter().map(|c| sent(c).0).max().unwrap_or(0);
    let (s, w, t) = (host.world.stats, worst_stats, n.max(1) as f32);
    println!(
        "{} clients, {n} ticks in {secs:.1} s ({:.1} a second): avg {:.3} ms/tick, worst {worst:.3} ms, budget {:.0} ms ({} ticks over half of it, {} over it); the world alone avg {:.3} ms, worst {worst_sim:.3} ms\n\
         zombies: {:.0} active on average (most {most_active}); now {} active, {} near, {} virtual ({} hunting)\n\
         per client: {:.1} KB/s down (busiest {:.1}, budget {}), {:.2} KB/s up; up to {views} zombies in a view, {:.1} updates a tick held back\n\
         worst tick ({worst_at}): tiers+index {:.3} ms ({} promoted, {} demoted), senses {:.3} ms, flow {:.3} ms ({} fields), nav {:.3} ms ({} plans), active AI {:.3} ms ({} active), near {:.3} ms, virtual {:.3} ms, the world {:.3} ms\n\
         joining: {joining} ticks, worst {joining_worst:.3} ms",
        clients.len(),
        n as f32 / secs,
        sum / t,
        TICK_DT * 1000.0,
        over[0],
        over[1],
        sim / t,
        active as f32 / t,
        s.active,
        s.near,
        s.virtual_,
        s.chasing,
        down as f32 / secs / online / 1000.0,
        busiest as f32 / secs / 1000.0,
        host.options.budget / 1000,
        up as f32 / secs / online / 1000.0,
        (host.stats.held - held0) as f32 / t,
        w.tier_ms,
        w.promoted,
        w.demoted,
        w.senses_ms,
        w.flow_ms,
        w.flow_builds,
        w.nav_ms,
        w.nav_plans,
        w.active_ms,
        w.active,
        w.near_ms,
        w.virtual_ms,
        w.tick_ms,
    );
    stop.store(true, Ordering::Relaxed);
    host.close("the bench is over");
    let _ = playing.join();
}

struct Bot {
    remote: Remote,
    last: Snapshot,
    seq: u32,
    heading: f32,
    /// Where it was a second ago, to notice it is stuck against something.
    was: Vec2,
    snapshots: u64,
}

/// A cheap, repeatable 0..1 from two numbers.
fn noise(a: u32, b: u32) -> f32 {
    let mut x = a.wrapping_mul(0x9E37_79B9) ^ b.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65536.0
}

/// Join `count` bots to the server at `addr`, each its own connection, and play them at the tick
/// rate: they wander, turn when stuck, pick up what lies at their feet, fight zombies that come
/// close (and players, if hostile) and start again when dead. Prints what the connections cost.
pub fn bots(args: BotArgs) {
    shutdown::install();
    let mut bots: Vec<Bot> = Vec::new();
    for k in 0..args.count {
        let name = format!("{}-{}", args.name, k + 1);
        // Bots say what they are (#69): a server's rules may refuse agents, or mark them.
        match Remote::connect_as(&args.addr, &name, &args.password, true, Duration::from_secs(10)).and_then(|mut r| r.wait_snapshot(Duration::from_secs(10)).map(|s| (r, s))) {
            Ok((mut remote, last)) => {
                remote.set_lag(Duration::from_millis(args.lag_ms as u64));
                let seq = last.players.iter().find(|p| p.id == remote.player()).map_or(0, |p| p.last_input_seq);
                bots.push(Bot { remote, last, seq, heading: noise(k as u32, 7) * std::f32::consts::TAU, was: Vec2::ZERO, snapshots: 1 });
            }
            Err(e) => eprintln!("{name}: {e}"),
        }
    }
    if bots.is_empty() {
        eprintln!("no bot could join {}", args.addr);
        std::process::exit(1);
    }
    let rules = bots[0].remote.welcome.rules.clone();
    println!("{} bots on {} ({}, pvp {})", bots.len(), args.addr, rules.name, if rules.pvp { "on" } else { "off" });
    let start = Instant::now();
    let step = Duration::from_secs_f32(TICK_DT);
    let mut next = start + step;
    let mut tick = 0u32;
    let (mut in0, mut out0, mut t0) = (bots.iter().map(|b| b.remote.bytes_in()).sum::<u64>(), bots.iter().map(|b| b.remote.bytes_out).sum::<u64>(), start);
    let stopped = || args.stop.as_ref().is_some_and(|s| s.load(Ordering::Relaxed));
    while !shutdown::requested() && !stopped() && args.secs.is_none_or(|s| start.elapsed().as_secs_f32() < s) {
        tick += 1;
        for (k, bot) in bots.iter_mut().enumerate() {
            play(bot, k as u32, tick, args.hostile && rules.pvp);
        }
        bots.retain(|b| match &b.remote.closed {
            Some(why) => {
                eprintln!("{}: {why}", b.remote.welcome.name);
                false
            }
            None => true,
        });
        if bots.is_empty() {
            if stopped() {
                break;
            }
            eprintln!("every bot lost its connection");
            std::process::exit(1);
        }
        if t0.elapsed() >= Duration::from_secs(5) {
            let (i, o) = (bots.iter().map(|b| b.remote.bytes_in()).sum::<u64>(), bots.iter().map(|b| b.remote.bytes_out).sum::<u64>());
            let (secs, n) = (t0.elapsed().as_secs_f32(), bots.len() as f32);
            let pings: Vec<f32> = bots.iter().filter_map(|b| b.remote.ping_ms).collect();
            let alive = bots.iter().filter(|b| b.last.players.iter().any(|p| p.id == b.remote.player() && !p.dead)).count();
            println!(
                "{} bots ({alive} alive): {:.1} KB/s down and {:.2} KB/s up each, {:.1} snapshots/s, ping {:.0} ms, {:.0} zombies in view",
                bots.len(),
                (i - in0) as f32 / secs / n / 1000.0,
                (o - out0) as f32 / secs / n / 1000.0,
                bots.iter().map(|b| b.snapshots).sum::<u64>() as f32 / secs / n,
                pings.iter().sum::<f32>() / pings.len().max(1) as f32,
                bots.iter().map(|b| b.last.zombies.len()).sum::<usize>() as f32 / n,
            );
            for b in &mut bots {
                b.snapshots = 0;
            }
            (in0, out0, t0) = (i, o, Instant::now());
        }
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
            next += step;
        } else {
            next = now + step;
        }
    }
}

/// One tick of one bot: take what came, decide, send.
fn play(bot: &mut Bot, k: u32, tick: u32, hostile: bool) {
    for s in bot.remote.poll() {
        bot.last = s;
        bot.snapshots += 1;
    }
    let me = bot.remote.player();
    let Some(view) = bot.last.players.iter().find(|p| p.id == me).copied() else {
        bot.remote.flush();
        return;
    };
    if view.dead {
        if tick % 60 == 0 {
            bot.remote.send(Intent::Respawn { player: me });
        }
        bot.remote.flush();
        return;
    }
    let pos = Vec2::from(view.pos);
    // The nearest thing to fight: a zombie on its feet, or (hostile) another living player.
    let zombies = bot.last.zombies.iter().filter(|z| !matches!(z.anim, ZombieAnim::Dead | ZombieAnim::Down)).map(|z| Vec2::from(z.pos));
    let players = bot.last.players.iter().filter(|p| hostile && p.id != me && !p.dead).map(|p| Vec2::from(p.pos));
    let target = zombies.chain(players).map(|p| (p.distance(pos), p)).filter(|(d, _)| *d < 5.0).min_by(|a, b| a.0.total_cmp(&b.0));
    // Stuck against a wall or a fence: another way.
    if tick % 20 == 0 {
        if bot.was.distance(pos) < 0.3 && target.is_none() {
            bot.heading += 1.5 + noise(k, tick) * 3.0;
        }
        bot.was = pos;
    }
    if tick % 100 == k % 100 {
        bot.heading += (noise(k, tick) - 0.5) * 2.0;
    }
    let (move_dir, facing, aim) = match target {
        Some((d, at)) => {
            let to = (at - pos).normalize_or_zero();
            if d < 1.3 && view.ready_in <= 0.0 {
                bot.remote.send(Intent::Attack { player: me });
            }
            (if d > 0.9 { to } else { Vec2::ZERO }, to.to_angle(), true)
        }
        None => (Vec2::from_angle(bot.heading), bot.heading, false),
    };
    if tick % 40 == k % 40 && !bot.last.floor_of(me).is_empty() && view.weapon.is_none() {
        bot.remote.send(Intent::PickUp { player: me });
    }
    bot.seq += 1;
    // What it decided on, so its blows land where it saw their targets (lag compensation, #23).
    bot.remote.send(Intent::Seen { player: me, tick: bot.last.tick });
    bot.remote.send(Intent::PlayerInput { player: me, seq: bot.seq, move_dir: move_dir.to_array(), facing, run: false, sneak: false, aim });
    bot.remote.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<HostArgs, String> {
        HostArgs::parse(&line.split_whitespace().map(String::from).collect::<Vec<_>>(), None)
    }

    #[test]
    fn host_flags_parse() {
        let a = parse("--host").unwrap();
        assert_eq!(a.addr, "0.0.0.0:7777");
        assert_eq!((a.pvp, a.options.max_players, a.options.allow_dev, a.options.spawn), (None, 32, false, None));
        assert!(a.options.password.is_empty());
        // `--host` with nothing after it but another flag still listens on the default.
        let a = parse("--host --pvp off --max-players 64 --spawn spread --password brains --allow-dev --name Dead-Commons").unwrap();
        assert_eq!(a.addr, "0.0.0.0:7777");
        assert_eq!((a.pvp, a.options.max_players, a.options.allow_dev, a.options.spawn), (Some(false), 64, true, Some(Spawn::Spread)));
        assert_eq!((a.options.password.as_str(), a.options.name.as_str()), ("brains", "Dead-Commons"));
        assert_eq!(parse("--host 9000").unwrap().addr, "0.0.0.0:9000");
        assert_eq!(parse("--host 127.0.0.1:9000 --pvp on").unwrap().addr, "127.0.0.1:9000");
        assert_eq!(parse("--host 10.0.0.5").unwrap().addr, "10.0.0.5:7777");
        assert_eq!(parse("--host --pvp on").unwrap().pvp, Some(true));
        assert_eq!((parse("--host").unwrap().options.budget, parse("--host --budget 64").unwrap().options.budget), (32_000, 64_000));
        assert_eq!((parse("--host").unwrap().options.fair_sight, parse("--host --fair-sight off").unwrap().options.fair_sight), (None, Some(false)));
        // The logout and spawn protection times (#68).
        let d = parse("--host").unwrap().options;
        assert_eq!((d.logout_secs, d.logout_safe_secs, d.spawn_protection_secs), (30.0, 5.0, 10.0));
        let o = parse("--host --logout-secs 120 --logout-safe-secs 0 --spawn-protection 3.5").unwrap().options;
        assert_eq!((o.logout_secs, o.logout_safe_secs, o.spawn_protection_secs), (120.0, 0.0, 3.5));
        assert_eq!(parse("--host --logout-secs 2").unwrap().options.logout_safe_secs, 2.0, "never longer than the unsafe time");
        for bad in ["--host --fair-sight dim", "--host --pvp maybe", "--host --max-players 65", "--host --max-players 0", "--host --spawn nowhere", "--host --budget 2", "--host --budget lots", "--host --logout-secs soon", "--host --spawn-protection 99999"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    /// The names file beside a save: written, read back, and a damaged line is skipped.
    #[test]
    fn names_are_kept_beside_the_save() {
        let dir = std::env::temp_dir().join(format!("zombye-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let save = dir.join("server.zsave");
        assert_eq!(names_path(&save), dir.join("server.zsave.players"));
        assert!(read_names(&save).is_empty());
        std::fs::write(names_path(&save), "3\tada\nnot a line\n7\tbob the builder\nx\tcy\n").unwrap();
        assert_eq!(read_names(&save), vec![("ada".to_string(), 3), ("bob the builder".to_string(), 7)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
