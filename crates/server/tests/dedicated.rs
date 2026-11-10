//! The dedicated server as a process (#20): `zombye-server --config FILE`, started the way the
//! Docker image starts it (a config path in an empty folder, the game data found through
//! `ZOMBYE_DATA`, the working folder somewhere else), and a game client joining it over a socket.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use zombye_net::remote::Remote;

struct Scratch(PathBuf);

impl Scratch {
    fn new(what: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("zombye-dedicated-test-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Server {
    child: Child,
    /// The line it printed when it began listening, and the address in it.
    line: String,
    addr: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The binary as the image runs it: no stdin, no admin token, the working folder `cwd`, the
/// game data from the environment.
fn command(cwd: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zombye-server"));
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
    command.args(args).current_dir(cwd).env("ZOMBYE_DATA", data).env_remove("ZOMBYE_ADMIN_TOKEN").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command
}

fn start(cwd: &Path, args: &[&str]) -> Server {
    let mut child = command(cwd, args).stderr(Stdio::null()).spawn().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = Vec::new();
    let line = loop {
        let line = lines.next().unwrap_or_else(|| panic!("the server stopped before listening: {seen:?}")).unwrap();
        if line.contains(": listening on ") {
            break line;
        }
        seen.push(line);
    };
    // Keep reading what it says, so it never blocks on a full pipe.
    std::thread::spawn(move || lines.for_each(drop));
    let addr = line.split_once(": listening on ").unwrap().1.split(' ').next().unwrap().to_string();
    Server { child, line, addr }
}

/// Ask it to stop as `docker stop` does, and wait for it to have gone.
#[cfg(unix)]
fn stop(mut server: Server) {
    let status = Command::new("kill").args(["-TERM", &server.child.id().to_string()]).status().unwrap();
    assert!(status.success());
    let start = Instant::now();
    while server.child.try_wait().unwrap().is_none() {
        assert!(start.elapsed() < Duration::from_secs(30), "the server stops on SIGTERM");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(server.child.try_wait().unwrap().and_then(|s| s.code()), Some(0));
}

#[test]
fn a_config_file_is_the_whole_server() {
    let dir = Scratch::new("file");
    let (etc, cwd) = (dir.0.join("etc"), dir.0.join("elsewhere"));
    std::fs::create_dir_all(&etc).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let config = etc.join("server.ron");
    std::fs::write(
        &config,
        r#"(
            name: "Dead Commons test",
            listen: "127.0.0.1:0",
            max_players: 2,
            password: "sesame",
            pvp: false,
            logout_secs: 12,
            logout_safe_secs: 2,
            start_kit: [(item: "rag", count: 3)],
            camera: Isometric,
            agents: Refused,
            save: "worlds/town.zsave",
            seed: 7,
            map_chunks: 8,
            zombies: 30,
        )"#,
    )
    .unwrap();
    let server = start(&cwd, &["--config", config.to_str().unwrap()]);
    assert!(server.line.starts_with("Dead Commons test: listening on 127.0.0.1:"), "{}", server.line);
    for want in ["2 players at most", "pvp off: co-op", "password set", "); 30 zombies on a ", " m map, seed 7"] {
        assert!(server.line.contains(want), "{want:?} in {}", server.line);
    }
    // Its world is where the file says, from the file's folder, not the working folder.
    let save = etc.join("worlds").join("town.zsave");
    assert!(save.exists() && !cwd.join("worlds").exists());

    // The password holds the door, and a client with it plays.
    let refused = Remote::connect(&server.addr, "eve", "guess", Duration::from_secs(20)).err().expect("the wrong password is refused");
    assert!(refused.to_lowercase().contains("password"), "{refused}");
    let mut ada = Remote::connect(&server.addr, "ada", "sesame", Duration::from_secs(20)).expect("ada joins");
    assert_eq!((ada.welcome.rules.name.as_str(), ada.welcome.rules.pvp, ada.welcome.rules.max_players), ("Dead Commons test", false, 2));
    // Every rule in the file is what a joining client is told (#69), and what anyone who asks
    // without joining (or a password) is told; the agents rule holds at the door.
    let r = ada.welcome.rules.clone();
    assert_eq!((r.logout_secs, r.logout_safe_secs, r.camera, r.agents), (12.0, 2.0, zombye_net::wire::Camera::Isometric, zombye_net::wire::Agents::Refused));
    assert_eq!((r.start_kit.clone(), r.zombies, r.spread_spawns), (vec![("Rag".to_string(), 3)], 30, false));
    // (8 chunks is too small for the town: the map is grown to fit it.)
    assert!(r.map_m[0] >= 128 && r.map_m[1] >= 128, "{:?}", r.map_m);
    let asked = zombye_net::remote::query(&server.addr, Duration::from_secs(20)).expect("the server says what it is");
    assert_eq!((asked.rules, asked.online, asked.password), (r, 1, true));
    let agent = Remote::connect_as(&server.addr, "bot-1", "sesame", true, Duration::from_secs(20)).err().expect("an agent is refused");
    assert!(agent.contains("don't allow agents"), "{agent}");
    let first = ada.wait_snapshot(Duration::from_secs(20)).expect("her first snapshot");
    let later = ada.wait_tick(first.tick + 10, Duration::from_secs(20)).expect("the world ticks on");
    assert!(later.players.iter().any(|p| p.id == ada.player()), "her survivor is in the world");

    // Stopped as a container is stopped, it saves the world and whose survivor is whose; started
    // again from the same file it has both.
    #[cfg(unix)]
    {
        let written = std::fs::metadata(&save).unwrap().modified().unwrap();
        drop(ada);
        stop(server);
        assert!(std::fs::metadata(&save).unwrap().modified().unwrap() >= written);
        let names = std::fs::read_to_string(etc.join("worlds").join("town.zsave.players")).unwrap();
        assert!(names.contains("\tada"), "{names:?}");
        let again = start(&cwd, &["--config", config.to_str().unwrap()]);
        let mut ada = Remote::connect(&again.addr, "ada", "sesame", Duration::from_secs(20)).expect("ada is back");
        assert!(ada.wait_snapshot(Duration::from_secs(20)).unwrap().tick > later.tick, "the saved world, carried on");
        assert_eq!(ada.player(), 1, "and her own survivor");
    }
}

/// `docker run` on an empty volume: no config yet. The server writes the commented defaults
/// there, makes its world beside them and takes players. (The port is the command line's here,
/// so the test doesn't need 7777 to be free: a flag wins over the file.)
#[test]
fn an_empty_folder_becomes_a_server() {
    let dir = Scratch::new("empty");
    let (data, cwd) = (dir.0.join("data"), dir.0.join("elsewhere"));
    std::fs::create_dir_all(&cwd).unwrap();
    let config = data.join("server.ron");
    let server = start(&cwd, &["--config", config.to_str().unwrap(), "--host", "127.0.0.1:0", "--map-chunks", "8", "--zombies", "30"]);
    assert!(server.line.starts_with("Zombye server: listening on 127.0.0.1:"), "{}", server.line);
    for want in ["32 players at most", "pvp on", "); 30 zombies on a ", " m map, seed 1"] {
        assert!(server.line.contains(want), "{want:?} in {}", server.line);
    }
    let written = std::fs::read_to_string(&config).expect("the config it wrote");
    assert!(written.contains("max_players: 32") && written.contains("listen: \"0.0.0.0:7777\""), "{written}");
    assert!(data.join("world.zsave").exists());
    let mut ada = Remote::connect(&server.addr, "ada", "", Duration::from_secs(20)).expect("ada joins");
    ada.wait_snapshot(Duration::from_secs(20)).expect("her first snapshot");
}

/// The server browser's row (#27): a server started with a mod says so to whoever asks what it
/// is, with its players and a ping, and a game without that mod is told it can't join.
#[test]
fn a_modded_server_is_a_row_in_the_browser() {
    use zombye_net::browser::{self, Browser, ListState};
    let dir = Scratch::new("browser");
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mods/alarm_clock");
    let server = start(&dir.0, &["--host", "127.0.0.1:0", "--name", "Clockwork", "--mod", sample.to_str().unwrap(), "--map-chunks", "8", "--zombies", "30"]);
    let list = dir.0.join("servers.ron");
    std::fs::write(&list, format!("(servers: [(address: \"{}\", featured: true, note: \"a test\")])", server.addr)).unwrap();
    let mut b = Browser::open(list.to_str().unwrap(), &[]);
    assert!(b.wait(Duration::from_secs(30)));
    assert_eq!(b.list, ListState::Ready);
    let featured = b.featured();
    let info = featured[0].info().unwrap_or_else(|| panic!("{:?}", featured[0].status));
    assert_eq!((featured[0].name(), info.online, info.rules.max_players, info.password), ("Clockwork", 0, 32, false));
    assert_eq!(info.mods.iter().map(|m| (m.id.as_str(), m.version.as_str())).collect::<Vec<_>>(), [("alarm_clock", "1.0")]);
    // A game without the mod can join all the same (#24): the row says what it would download.
    let pack = zombye_mods::Mod::load(&sample).unwrap().pack();
    assert_eq!((info.mods[0].hash, info.mods[0].size as usize), (pack.hash(), pack.size()));
    assert_eq!(browser::can_join(info, &[], false), Ok(()));
    assert!(browser::mods_note(&info.mods, &[]).contains("1 to download"));
    assert_eq!(browser::mods_note(&info.mods, &[pack.hash()]), "Wind-up alarm clocks 1.0");
    // `--server-info` says the mods too.
    assert!(zombye_net::remote::describe(info, &server.addr).iter().any(|l| l.contains("Mods: Wind-up alarm clocks 1.0")));
}

/// Joining a modded server just works (#24): a game with none of the server's mods asks what it
/// runs, downloads their data (an item, a building template that changes the town), checks it by
/// hash, and then has the server's game data and builds the server's town. Its script stays on
/// the server.
#[test]
fn a_game_without_the_mods_downloads_them_and_joins() {
    use zombye_net::modpack::{self, Cache, Source};
    use zombye_net::{content, remote};
    let dir = Scratch::new("download");
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
    // A mod with everything in it: an item, loot, a building, a script.
    let folder = dir.0.join("mods/huts");
    std::fs::create_dir_all(folder.join("buildings")).unwrap();
    std::fs::write(folder.join("mod.ron"), r#"(id: "huts", name: "Huts and whistles", version: "0.3", scripts: ["huts.luau"])"#).unwrap();
    std::fs::write(folder.join("items.ron"), r#"[(id: "whistle", name: "Whistle", category: Junk, weight_kg: 0.02, volume_l: 0.02, tags: ["reusable"])]"#).unwrap();
    std::fs::write(folder.join("loot.ron"), r#"[(room: "bedroom", container: "any", entries: [(item: "whistle", weight: 50)])]"#).unwrap();
    std::fs::write(folder.join("huts.luau"), "function on_use(game, player, item) if item == 'whistle' then game:set('blown', true) end end").unwrap();
    let hut = std::fs::read_to_string(data.join("buildings/bungalow.ron")).unwrap().replace("\"house_bungalow\"", "\"mod_hut\"");
    std::fs::write(folder.join("buildings/hut.ron"), hut).unwrap();
    let server = start(&dir.0, &["--host", "127.0.0.1:0", "--name", "Huts", "--mod", folder.to_str().unwrap(), "--seed", "4", "--map-chunks", "8", "--zombies", "30", "--password", "sesame"]);
    let wait = Duration::from_secs(20);

    // The game as installed: no mods. Joining with its own data alone is refused, as it was.
    let base = content::load_config(&data, 1).unwrap();
    let plain = Remote::connect(&server.addr, "ann", "sesame", wait).unwrap();
    let e = remote::build_map(&plain.welcome, &base).err().expect("other game data");
    assert!(e.contains("building templates") && e.contains("items"), "{e}");
    drop(plain);

    // What the game does on joining: ask, download what's missing, check, keep, apply.
    let info = remote::query(&server.addr, wait).unwrap();
    assert_eq!(info.mods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["huts"]);
    let cache = Cache::in_saves(&dir.0.join("saves"));
    let got = modpack::gather(&server.addr, "sesame", &info.mods, &[], Some(&cache), wait).unwrap();
    assert_eq!((got.packs.len(), got.packs[0].1), (1, Source::Downloaded));
    let pack = &got.packs[0].0;
    assert_eq!(pack.hash(), info.mods[0].hash);
    assert_eq!(pack.files.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["items.ron", "loot.ron", "buildings/hut.ron"], "data, and no script");
    let mut modded = base.clone();
    zombye_mods::apply_packs(&mut modded, &[pack.clone()]).unwrap();
    assert!(modded.scripts.is_none());
    let mut on = Remote::connect(&server.addr, "ann", "sesame", wait).unwrap();
    let joined = remote::build_map(&on.welcome, &modded).expect("the server's data, the server's town");
    assert!(joined.config.items.iter().any(|i| i.id == "whistle"));
    assert!(on.wait_snapshot(wait).is_ok());
    // The next time nothing is downloaded.
    let again = modpack::gather(&server.addr, "sesame", &info.mods, &[], Some(&cache), wait).unwrap();
    assert_eq!(again.packs[0].1, Source::Cached);
    // Without the password, no mods.
    assert!(modpack::gather(&server.addr, "", &info.mods, &[], None, wait).unwrap_err().contains("wrong password"));
}

/// A config with a mistake in it stops the server before it makes a world, naming the file and
/// the mistake.
#[test]
fn a_mistaken_config_is_no_server() {
    let dir = Scratch::new("mistake");
    let config = dir.0.join("server.ron");
    for (text, want) in [("(max_playerz: 8)", "max_playerz"), ("(max_players: 500)", "--max-players is 1 to 64"), ("(admin: (listen: \"127.0.0.1:0\"))", "needs a token")] {
        std::fs::write(&config, text).unwrap();
        let out = command(&dir.0, &["--config", config.to_str().unwrap()]).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{text}");
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(said.contains(want), "{want:?} in {said}");
        assert!(!dir.0.join("world.zsave").exists(), "{text}: it stopped before making a world");
    }
    let said = String::from_utf8_lossy(&command(&dir.0, &["--config", config.to_str().unwrap(), "--bogus"]).output().unwrap().stderr).to_string();
    assert!(said.contains("needs a token"), "the file is still read with other flags about: {said}");
    let out = command(&dir.0, &["--config"]).output().unwrap();
    assert!(out.status.code() == Some(2) && String::from_utf8_lossy(&out.stderr).contains("--config needs a file"));
}
