//! `zombye-server --host`: the multiplayer server (#21), and `--bots N --connect ADDR`: headless
//! bot clients that join one, for filling a server and measuring it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glam::Vec2;
use zombye_net::host::{BUDGET_RANGE, DEFAULT_BUDGET, Host, HostOptions, Spawn};
use zombye_net::remote::Remote;
use zombye_net::{Intent, Snapshot, shutdown, wire};
use zombye_sim::{TICK_DT, World, ZombieAnim};

use crate::Persist;

/// What the command line says about hosting.
pub struct HostArgs {
    /// `--host [ADDR]`: where to listen (default every interface, port 7777).
    pub addr: String,
    pub options: HostOptions,
    /// `--pvp on|off`: `None` leaves a loaded world's rule as it is (a new world: on).
    pub pvp: Option<bool>,
}

impl HostArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
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
        let options = HostOptions { name: text("--name").unwrap_or_else(|| "Zombye server".into()), max_players, allow_dev: flag("--allow-dev"), password: text("--password").unwrap_or_default(), spawn, budget, fair_sight: on_off("--fair-sight")? };
        Ok(Self { addr, options, pvp: on_off("--pvp")? })
    }
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

fn write_names(save: &Path, host: &Host) {
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
    let rules = host.rules();
    println!(
        "{}: listening on {} ({} players at most, {} KB/s down each, pvp {}, {}{}{}); {} zombies on a {}x{} m map, seed {}",
        rules.name,
        host.local_addr().map_or(args.addr.clone(), |a| a.to_string()),
        rules.max_players,
        host.options.budget / 1000,
        if rules.pvp { "on" } else { "off: co-op" },
        if rules.fair_sight { "other players only when seen or heard" } else { "other players always shown" },
        if rules.allow_dev { ", dev intents allowed" } else { "" },
        if host.options.password.is_empty() { "" } else { ", password set" },
        host.world.zombies.len(),
        host.world.map.width(),
        host.world.map.height(),
        host.world.config.generation.seed,
    );
    let mut report = Report::new(&host);
    host.run(|h| {
        for line in h.log.drain(..) {
            println!("{line}");
            persist.dirty = true;
        }
        persist.dirty |= !h.clients().is_empty();
        if persist.autosave_due() {
            persist.save_now(&h.world, "autosave");
            if let Some(save) = &persist.path {
                write_names(save, h);
            }
        }
        report.tick(h);
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

    fn tick(&mut self, host: &Host) {
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
            println!(
                "tick {}: {online} online | {:.1} ticks/s, {:.2} ms a tick (worst {:.2}, budget {:.0}) | zombies {} active, {} near, {} virtual | {:.1} KB/s down per client (busiest {:.1}, budget {}) | up to {} zombies in a view, {:.0} updates a tick held back",
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
            );
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
        match Remote::connect(&args.addr, &name, &args.password, Duration::from_secs(10)).and_then(|mut r| r.wait_snapshot(Duration::from_secs(10)).map(|s| (r, s))) {
            Ok((remote, last)) => {
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
    while !shutdown::requested() && args.secs.is_none_or(|s| start.elapsed().as_secs_f32() < s) {
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
    bot.remote.send(Intent::PlayerInput { player: me, seq: bot.seq, move_dir: move_dir.to_array(), facing, run: false, sneak: false, aim });
    bot.remote.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<HostArgs, String> {
        HostArgs::parse(&line.split_whitespace().map(String::from).collect::<Vec<_>>())
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
        for bad in ["--host --fair-sight dim", "--host --pvp maybe","--host --max-players 65", "--host --max-players 0", "--host --spawn nowhere", "--host --budget 2", "--host --budget lots"] {
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
