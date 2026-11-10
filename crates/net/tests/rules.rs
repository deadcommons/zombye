//! A server's rules (#69, docs/PLAN.md §4.8) over real sockets: every rule is in the welcome a
//! joining client gets, anyone can ask for them without joining, and the agents rule is kept at
//! the door and in what players are shown.

use std::time::{Duration, Instant};

use zombye_net::host::{Host, HostOptions, Spawn};
use zombye_net::remote::{self, Remote};
use zombye_net::wire::{self, Agents, Camera, ClientMsg, ServerMsg};
use zombye_net::{Snapshot, content};
use zombye_sim::World;

fn host(pvp: bool, options: HostOptions) -> Host {
    let mut config = content::load_config(&content::default_data_dir(), 1).expect("the shipped content");
    config.zombies = Some(40);
    config.combat.pvp = pvp;
    Host::bind(World::new(config), "127.0.0.1:0", options).expect("a free port")
}

/// Do `work` (a client's blocking call) on another thread while pumping the host in this one.
fn with_host<T: Send + 'static>(host: &mut Host, work: impl FnOnce(String) -> T + Send + 'static) -> T {
    let addr = host.local_addr().unwrap().to_string();
    let client = std::thread::spawn(move || work(addr));
    while !client.is_finished() {
        host.pump();
        std::thread::sleep(Duration::from_millis(1));
    }
    client.join().unwrap()
}

fn join(host: &mut Host, name: &str, agent: bool) -> Result<Remote, String> {
    let name = name.to_string();
    with_host(host, move |addr| Remote::connect_as(&addr, &name, "", agent, Duration::from_secs(10)))
}

/// One tick, and each client's snapshot of it.
fn tick(host: &mut Host, clients: &mut [&mut Remote]) -> Vec<Snapshot> {
    host.tick();
    let now = host.world.tick;
    clients.iter_mut().map(|c| c.wait_tick(now, Duration::from_secs(10)).expect("a snapshot")).collect()
}

/// Every rule the server was set up with is what a joining client is told, in the welcome.
#[test]
fn a_joining_client_is_told_every_rule() {
    let kit = vec![("water_bottle".to_string(), 1), ("rag".to_string(), 2)];
    let options = HostOptions {
        name: "Dead Commons".into(),
        max_players: 12,
        allow_dev: true,
        spawn: Some(Spawn::Together),
        logout_secs: 45.0,
        logout_safe_secs: 3.0,
        spawn_protection_secs: 7.0,
        kit,
        fair_sight: Some(false),
        camera: Camera::Isometric,
        agents: Agents::Allowed,
        ..Default::default()
    };
    let mut host = host(true, options);
    let ada = join(&mut host, "ada", false).expect("ada joins");
    let r = &ada.welcome.rules;
    assert_eq!(*r, host.rules(), "the welcome carries the host's rules");
    assert_eq!((r.name.as_str(), r.max_players, r.pvp, r.allow_dev, r.fair_sight), ("Dead Commons", 12, true, true, false));
    assert_eq!((r.logout_secs, r.logout_safe_secs, r.spawn_protection_secs), (45.0, 3.0, 7.0));
    assert_eq!((r.camera, r.agents, r.spread_spawns), (Camera::Isometric, Agents::Allowed, false));
    // The kit by the items' names, as a player reads it; the world as it is.
    assert_eq!(r.start_kit, [("Bottle of water".to_string(), 1), ("Rag".to_string(), 2)]);
    assert_eq!(r.map_m, [host.world.map.width() as u32, host.world.map.height() as u32]);
    assert_eq!(r.zombies, 40);
    // And the survivor got what the rules say.
    let me = host.world.player(ada.player()).unwrap();
    assert_eq!((me.inventory.carried.len(), me.protected), (2, 7.0));

    // In words: a line a rule, nothing left out.
    let lines = r.lines();
    let line = |label: &str| lines.iter().find(|(l, _)| *l == label).unwrap_or_else(|| panic!("no {label} line in {lines:?}")).1.clone();
    assert_eq!(line("Players"), "up to 12");
    assert!(line("PvP").starts_with("on"));
    assert_eq!(line("Camera"), "isometric");
    assert!(line("Agents").contains("not marked"));
    assert!(line("Other players").contains("always shown"));
    assert_eq!(line("Logging out"), "your survivor stays in the world for 45 s (3 s somewhere safe)");
    assert_eq!(line("New survivors"), "start together, by one house; other players can't hurt them for 7 s");
    assert_eq!(line("Start kit"), "Bottle of water, 2 x Rag");
    assert_eq!(line("World"), format!("{} x {} m, 40 zombies", r.map_m[0], r.map_m[1]));
    assert!(line("Dev tools").starts_with("on"));

    // The defaults: open PvP's (spread spawns, fair sight), agents marked, the isometric camera.
    let mut plain = self::host(true, HostOptions::default());
    let bob = join(&mut plain, "bob", false).unwrap();
    let r = &bob.welcome.rules;
    assert_eq!((r.camera, r.agents, r.spread_spawns, r.fair_sight, r.start_kit.is_empty()), (Camera::Isometric, Agents::Marked, true, true, true));
    let lines = r.lines();
    assert!(lines.iter().any(|(l, t)| *l == "Start kit" && t == "nothing") && !lines.iter().any(|(l, _)| *l == "Dev tools"), "{lines:?}");
    // Co-op: nothing about protection from players, who can't hurt each other anyway.
    let mut coop = self::host(false, HostOptions::default());
    let cy = join(&mut coop, "cy", false).unwrap();
    let lines = cy.welcome.rules.lines();
    assert!(lines.iter().any(|(l, t)| *l == "New survivors" && t == "start together, by one house"), "{lines:?}");
    assert!(lines.iter().any(|(l, t)| *l == "PvP" && t.starts_with("off")));
}

/// Anyone can ask a server what it is without joining: no password, no survivor, nobody counted
/// as a player. What the server browser will show.
#[test]
fn a_server_says_its_rules_to_anyone_who_asks() {
    let mut host = host(true, HostOptions { name: "Ask me".into(), password: "sesame".into(), max_players: 2, camera: Camera::FirstPerson, agents: Agents::Refused, ..Default::default() });
    let asked = with_host(&mut host, |addr| remote::query(&addr, Duration::from_secs(10))).expect("an answer");
    assert_eq!(asked.rules, host.rules());
    assert_eq!((asked.protocol, asked.online, asked.password), (wire::PROTOCOL, 0, true));
    assert_eq!((asked.rules.camera, asked.rules.agents), (Camera::FirstPerson, Agents::Refused));
    assert_eq!(asked.version, env!("CARGO_PKG_VERSION"));
    assert!(host.world.players.is_empty() && host.clients().is_empty(), "asking joins nobody");
    assert_eq!((host.stats.queries, host.stats.joined, host.stats.refused), (1, 0, 0));

    // With someone on it says so; a full server still answers.
    let name = "ada".to_string();
    let _ada = with_host(&mut host, move |addr| Remote::connect(&addr, &name, "sesame", Duration::from_secs(10))).expect("ada joins");
    let _bob = with_host(&mut host, |addr| Remote::connect(&addr, "bob", "sesame", Duration::from_secs(10))).expect("bob joins");
    let asked = with_host(&mut host, |addr| remote::query(&addr, Duration::from_secs(10))).expect("an answer from a full server");
    assert_eq!((asked.online, asked.rules.max_players), (2, 2));
    // In words, for `--server-info`.
    let text = remote::describe(&asked, "example:7777").join("\n");
    for part in ["Ask me at example:7777: 2 of 2 players on, needs a password", "this game can join", "Camera: first person only", "Agents: not allowed", "PvP: on"] {
        assert!(text.contains(part), "{part:?} in:\n{text}");
    }
    let mut older = asked.clone();
    older.protocol -= 1;
    assert!(remote::describe(&older, "x").join("\n").contains("can't join"));

    // A first message that is neither a hello nor a query gets no answer.
    let addr = host.local_addr().unwrap();
    let mut raw = std::net::TcpStream::connect(addr).unwrap();
    raw.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    wire::write_frame(&mut raw, &ClientMsg::Bye).unwrap();
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(100) {
        host.pump();
    }
    assert!(wire::read_frame::<ServerMsg>(&mut raw, wire::MAX_SERVER_FRAME).is_err(), "neither a hello nor a query: just closed");
    // Nothing at the address: an error in words, not a hang.
    drop(host);
    assert!(remote::query(&addr.to_string(), Duration::from_secs(2)).is_err());
}

/// The agents rule: a client that says it is a program is refused, let in unmarked, or let in and
/// shown as an agent to the players who see it.
#[test]
fn agents_are_refused_allowed_or_marked() {
    // Refused: people get in, programs don't, and they are told why.
    let mut host = self::host(false, HostOptions { agents: Agents::Refused, ..Default::default() });
    let why = join(&mut host, "bot-1", true).err().expect("an agent is refused");
    assert!(why.contains("don't allow agents"), "{why}");
    assert!(join(&mut host, "ada", false).is_ok());
    assert_eq!((host.stats.refused, host.world.players.len()), (1, 1));

    // Marked (co-op, so everyone near is shown): the others see which survivor is the agent's,
    // and its own client is told too.
    let mut host = self::host(false, HostOptions { agents: Agents::Marked, ..Default::default() });
    let mut ada = join(&mut host, "ada", false).unwrap();
    let mut bot = join(&mut host, "bot-1", true).unwrap();
    let (ada_id, bot_id) = (ada.player(), bot.player());
    assert!(host.is_agent(bot_id) && !host.is_agent(ada_id));
    let snaps = tick(&mut host, &mut [&mut ada, &mut bot]);
    for (who, snap) in ["ada", "the bot"].iter().zip(&snaps) {
        let agent = |id: u32| snap.players.iter().find(|p| p.id == id).unwrap_or_else(|| panic!("{who} sees player {id}")).agent;
        assert_eq!((agent(ada_id), agent(bot_id)), (false, true), "as {who} is shown them");
    }
    assert!(host.log.iter().any(|l| l.contains("bot-1 (an agent) joined")), "{:?}", host.log);
    // The same name back with a person at the keys is a person's survivor again.
    drop(bot);
    let start = Instant::now();
    while host.clients().len() > 1 {
        host.tick();
        assert!(start.elapsed() < Duration::from_secs(10), "the bot's connection is noticed gone");
    }
    let mut person = join(&mut host, "bot-1", false).unwrap();
    assert_eq!(person.player(), bot_id);
    let snaps = tick(&mut host, &mut [&mut ada, &mut person]);
    assert!(snaps[0].players.iter().all(|p| !p.agent), "nobody is marked now");

    // Allowed: it joins, and nobody is told, itself included.
    let mut host = self::host(false, HostOptions { agents: Agents::Allowed, ..Default::default() });
    let mut ada = join(&mut host, "ada", false).unwrap();
    let mut bot = join(&mut host, "bot-1", true).unwrap();
    assert!(host.is_agent(bot.player()), "the server knows");
    let snaps = tick(&mut host, &mut [&mut ada, &mut bot]);
    assert!(snaps.iter().all(|s| s.players.len() == 2 && s.players.iter().all(|p| !p.agent)));

    // With fair sight (open PvP) the mark goes only with the survivor: seen, it is marked; an
    // agent's own client always knows what it is.
    let mut host = self::host(true, HostOptions { spawn: Some(Spawn::Together), ..Default::default() });
    let mut ada = join(&mut host, "ada", false).unwrap();
    let mut bot = join(&mut host, "bot-1", true).unwrap();
    let bot_id = bot.player();
    let snaps = tick(&mut host, &mut [&mut ada, &mut bot]);
    assert!(snaps[1].players.iter().find(|p| p.id == bot_id).unwrap().agent);
    assert!(snaps[0].players.iter().filter(|p| p.id == bot_id).all(|p| p.agent), "if ada sees it, she sees what it is");
}
