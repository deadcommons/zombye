//! Hosting a game from the main menu (Multiplayer, Host a game): the game starts the dedicated
//! server that ships beside it on this computer, for friends, and joins it.
//!
//! Nothing here is a second kind of server. The menu's choices (a name, a password, PvP or co-op,
//! how many players) are written as the dedicated server's own config file
//! (`zombye-server --config`, docs/self-hosting.md) in `hosted/` in the saves folder, beside the
//! world it keeps (`world.zsave`) and its log (`server.log`), and `zombye-server` is started on it
//! as a process of its own. Once it answers on this computer the menu joins it like any other
//! server (`--connect 127.0.0.1:PORT`), so everything a join does (the join screen, the player's
//! key, mods) is as it is anywhere.
//!
//! **It stops by itself:** the config says `stop_when_empty_secs`, so the server saves and stops
//! once nobody has been on it for a minute (and two minutes after starting if nobody ever came).
//! The game doesn't hold it: it starts again for every screen (`crate::relaunch`), so there is no
//! one process to tie a server to, and friends still playing when the host leaves aren't thrown
//! out. Hosting again while it is still up joins it as it is.
//!
//! **Who can reach it:** anyone who can open a TCP connection to this computer's port (7777
//! unless changed): others on the same network at [`local_address`], and over the internet only
//! when the router forwards that port here. The menu says both.

use std::net::{IpAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use zombye_net::remote;

/// The folder in the saves folder a hosted game keeps its config, world and log in.
pub const FOLDER: &str = "hosted";
const CONFIG: &str = "server.ron";
const WORLD: &str = "world.zsave";
const LOG: &str = "server.log";
/// Seconds with nobody on after which a hosted server saves and stops.
pub const STOP_WHEN_EMPTY_SECS: u32 = 60;
/// How long a server may take to answer after it is started (a big saved world loads in a second
/// or two; a slow disk or a virus scanner can take far longer).
const START_WAIT: Duration = Duration::from_secs(90);
/// The player counts the menu offers.
pub const PLAYER_CHOICES: [u32; 5] = [2, 4, 8, 16, 32];

/// What the Host a game screen sets, kept in `settings.ron` between sessions.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct HostSetup {
    /// What players see when they join ("": "NAME's game", or a plain default).
    pub name: String,
    /// "" for none. It keeps strangers out; it goes over the network in the clear.
    pub password: String,
    /// Player against player, or co-op. Friends' games start co-op.
    pub pvp: bool,
    pub max_players: u32,
    pub port: u16,
}

impl Default for HostSetup {
    fn default() -> Self {
        Self { name: String::new(), password: String::new(), pvp: false, max_players: 8, port: zombye_net::wire::DEFAULT_PORT }
    }
}

impl HostSetup {
    /// The server's name: the one typed, else the host's own name's, else a plain one.
    pub fn server_name(&self, player: &str) -> String {
        match (self.name.trim(), player.trim()) {
            ("", "") => "A friend's game".to_string(),
            ("", player) => format!("{player}'s game"),
            (name, _) => name.to_string(),
        }
    }

    /// Where this computer's own game joins it.
    pub fn local(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

/// A string as RON writes it.
fn quoted(text: &str) -> String {
    // RON writes an apostrophe escaped, which reads the same and looks worse.
    ron::to_string(text).unwrap_or_else(|_| "\"\"".to_string()).replace("\\'", "'")
}

/// The dedicated server's config for this game: only fields `zombye-server --config` knows
/// (`crates/server/src/config.rs` holds a copy of this shape to its parser). `seed` only for a new
/// world; `data` the game data this game plays with, so the server's is the same.
pub fn config_text(setup: &HostSetup, player: &str, seed: Option<u64>, data: &Path) -> String {
    let mut out = String::from("// Written by the game each time you host from its menu (Multiplayer, Host a game):\n// change these there. docs/self-hosting.md has every setting a server takes.\n(\n");
    let mut line = |field: &str, value: String| out.push_str(&format!("    {field}: {value},\n"));
    line("name", quoted(&setup.server_name(player)));
    line("listen", quoted(&format!("0.0.0.0:{}", setup.port)));
    line("max_players", setup.max_players.clamp(1, zombye_net::wire::MAX_PLAYERS as u32).to_string());
    line("password", quoted(&setup.password));
    line("pvp", setup.pvp.to_string());
    line("stop_when_empty_secs", STOP_WHEN_EMPTY_SECS.to_string());
    line("save", quoted(WORLD));
    if let Some(seed) = seed {
        line("seed", seed.to_string());
    }
    line("data", quoted(&data.to_string_lossy()));
    out.push_str(")\n");
    out
}

/// The hosted game's folder.
pub fn folder(saves: &Path) -> PathBuf {
    saves.join(FOLDER)
}

/// Whether there is a hosted world to carry on with.
pub fn world_exists(saves: &Path) -> bool {
    folder(saves).join(WORLD).is_file()
}

/// The server program: `$ZOMBYE_SERVER`, else `zombye-server` beside this game.
pub fn server_binary() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("ZOMBYE_SERVER").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()) {
        return if p.is_file() { Ok(p) } else { Err(format!("ZOMBYE_SERVER names {}, which isn't there", p.display())) };
    }
    let exe = std::env::current_exe().map_err(|e| format!("can't tell where the game is: {e}"))?;
    let name = if cfg!(windows) { "zombye-server.exe" } else { "zombye-server" };
    let beside = exe.parent().map(|d| d.join(name)).filter(|p| p.is_file());
    beside.ok_or_else(|| format!("The server program ({name}) isn't beside the game. From a checkout, build it first: cargo build --release -p zombye-server"))
}

/// This computer's address on its network, for friends on the same one. No packet is sent: a UDP
/// socket is only pointed outward to learn which of our addresses the system would use.
pub fn local_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// How a hosted server's start is going.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    Starting,
    /// It answers: join it.
    Ready,
    /// It didn't start, and why (its own last words when it has any).
    Failed(String),
}

/// A server being started.
pub struct Hosting {
    child: Option<Child>,
    log: PathBuf,
    since: Instant,
    answered: Arc<AtomicBool>,
}

/// What [`start`] did.
pub enum Started {
    /// A server (ours from before, most likely) already answers on that port: join it as it is.
    Already(String),
    New(Hosting),
}

/// Start the server for `setup` in the saves folder's `hosted/`. `new_world` sets the world there
/// aside (as `world.old.zsave`) and starts a fresh one with `seed`.
pub fn start(saves: &Path, setup: &HostSetup, player: &str, new_world: bool, seed: u64, data: &Path) -> Result<Started, String> {
    let addr = setup.local();
    if let Ok((info, _)) = remote::probe(&addr, Duration::from_millis(400)) {
        return Ok(Started::Already(info.rules.name));
    }
    let server = server_binary()?;
    let dir = folder(saves);
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    let world = dir.join(WORLD);
    let fresh = new_world || !world.is_file();
    if new_world && world.is_file() {
        // The one before is kept once, in case New world was a slip.
        let players = |w: &Path| PathBuf::from(format!("{}.players", w.display()));
        let old = dir.join("world.old.zsave");
        std::fs::rename(&world, &old).map_err(|e| format!("can't set the old world aside: {e}"))?;
        let _ = std::fs::rename(players(&world), players(&old));
    }
    let data = std::fs::canonicalize(data).unwrap_or_else(|_| data.to_path_buf());
    let config = dir.join(CONFIG);
    std::fs::write(&config, config_text(setup, player, fresh.then_some(seed), &data)).map_err(|e| format!("can't write {}: {e}", config.display()))?;
    let log = dir.join(LOG);
    let out = std::fs::File::create(&log).map_err(|e| format!("can't write {}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| format!("can't write {}: {e}", log.display()))?;
    let mut command = Command::new(&server);
    command.arg("--config").arg(&config).stdin(Stdio::null()).stdout(out).stderr(err);
    // Its own settings only: not what this game was told about another server.
    command.env_remove("ZOMBYE_PASSWORD").env_remove("ZOMBYE_ADMIN_TOKEN");
    #[cfg(windows)]
    {
        // No console window beside the game (CREATE_NO_WINDOW).
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let child = command.spawn().map_err(|e| format!("can't start {}: {e}", server.display()))?;
    println!("hosting: started {} --config {} (its log: {})", server.display(), config.display(), log.display());
    // Ask until it answers, on a thread of its own: the menu never waits on a socket.
    let answered = Arc::new(AtomicBool::new(false));
    let (asked, tell) = (addr, answered.clone());
    let ask = move || {
        let until = Instant::now() + START_WAIT + Duration::from_secs(5);
        while Instant::now() < until {
            if remote::probe(&asked, Duration::from_millis(800)).is_ok() {
                tell.store(true, Ordering::Relaxed);
                return;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    };
    std::thread::Builder::new().name("hosting".into()).spawn(ask).map_err(|e| format!("can't start a thread: {e}"))?;
    Ok(Started::New(Hosting { child: Some(child), log, since: Instant::now(), answered }))
}

/// The last few lines of a server's log that say anything.
fn last_words(log: &Path) -> String {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(3)..].join(" | ").chars().take(400).collect()
}

impl Hosting {
    /// Where it has got to. Call it once a frame.
    pub fn poll(&mut self) -> Progress {
        if self.answered.load(Ordering::Relaxed) {
            return Progress::Ready;
        }
        let ended = match self.child.as_mut().map(Child::try_wait) {
            Some(Ok(Some(status))) => Some(format!("{status}")),
            Some(Err(e)) => Some(e.to_string()),
            _ => None,
        };
        if let Some(how) = ended {
            self.child = None;
            let words = last_words(&self.log);
            return Progress::Failed(if words.is_empty() { format!("The server stopped as it started ({how}). Its log: {}", self.log.display()) } else { format!("The server didn't start: {words}") });
        }
        if self.since.elapsed() > START_WAIT {
            self.stop();
            return Progress::Failed(format!("The server didn't answer in {} s. Its log: {}", START_WAIT.as_secs(), self.log.display()));
        }
        Progress::Starting
    }

    /// Call it off: the server is stopped before anyone was on it.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hosted_game_is_a_dedicated_server_config() {
        let setup = HostSetup { name: "  ".into(), password: "say \"friend\"".into(), pvp: true, max_players: 4, port: 7790 };
        let text = config_text(&setup, "ada", Some(42), Path::new("/games/zombye/assets/data"));
        // It is RON, of the fields the server's config has (and no others: it refuses unknown ones).
        let value: ron::Value = ron::from_str(&text).unwrap_or_else(|e| panic!("{e} in:\n{text}"));
        let ron::Value::Map(map) = value else { panic!("not a struct: {text}") };
        // (A RON map comes back sorted by key, so the order isn't looked at.)
        let mut fields: Vec<String> = map.iter().map(|(k, _)| k.clone().into_rust::<String>().unwrap()).collect();
        fields.sort();
        assert_eq!(fields, ["data", "listen", "max_players", "name", "password", "pvp", "save", "seed", "stop_when_empty_secs"]);
        for expected in ["name: \"ada's game\",", "listen: \"0.0.0.0:7790\",", "max_players: 4,", "password: \"say \\\"friend\\\"\",", "pvp: true,", "stop_when_empty_secs: 60,", "save: \"world.zsave\",", "seed: 42,", "data: \"/games/zombye/assets/data\","] {
            assert!(text.contains(expected), "{expected} in:\n{text}");
        }
        // Carrying on with a world: no seed (the save has its own).
        assert!(!config_text(&setup, "ada", None, Path::new("d")).contains("seed"));
        // The server's name: typed, else the host's, else a plain one.
        assert_eq!(HostSetup { name: " Friday night ".into(), ..setup.clone() }.server_name("ada"), "Friday night");
        assert_eq!(setup.server_name(" "), "A friend's game");
        assert_eq!((setup.local(), HostSetup::default().local()), ("127.0.0.1:7790".to_string(), "127.0.0.1:7777".to_string()));
        // Friends' games are co-op unless asked otherwise, and of a size the menu offers.
        let d = HostSetup::default();
        assert!(!d.pvp && PLAYER_CHOICES.contains(&d.max_players) && d.password.is_empty());
        // Settings files from before hosting read as the defaults.
        assert_eq!(ron::from_str::<HostSetup>("()").unwrap(), d);
    }

    #[test]
    fn a_server_that_cant_start_says_why() {
        let dir = std::env::temp_dir().join(format!("zombye-hosting-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!world_exists(&dir));
        let log = dir.join(LOG);
        std::fs::write(&log, "loading\n\n  can't listen on 0.0.0.0:7777: address in use  \n").unwrap();
        assert_eq!(last_words(&log), "loading | can't listen on 0.0.0.0:7777: address in use");
        assert_eq!(last_words(&dir.join("none.log")), "");
        std::fs::create_dir_all(folder(&dir)).unwrap();
        std::fs::write(folder(&dir).join(WORLD), "x").unwrap();
        assert!(world_exists(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }
}
