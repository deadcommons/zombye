//! Multiplayer over real sockets (#21): a [`Host`] and [`Remote`] clients on the loopback
//! interface, the host ticked by hand so every test is deterministic about what the server did.

use std::time::{Duration, Instant};

use glam::Vec2;
use zombye_net::host::{Host, HostOptions, Spawn};
use zombye_net::remote::{Remote, build_map};
use zombye_net::{Intent, Snapshot, content};
use zombye_sim::players::step_movement;
use zombye_sim::{CombatEventKind, PlayerInput, PlayerState, TICK_DT, World, WorldConfig};

fn config(zombies: u32) -> WorldConfig {
    let mut config = content::load_config(&content::default_data_dir(), 1).expect("the shipped content");
    config.zombies = Some(zombies);
    config
}

fn host(zombies: u32, pvp: bool, options: HostOptions) -> Host {
    let mut config = config(zombies);
    config.combat.pvp = pvp;
    Host::bind(World::new(config), "127.0.0.1:0", options).expect("a free port")
}

/// Connect a client while pumping the host (which is in this thread, so can't answer by itself).
fn join_as(host: &mut Host, name: &str, password: &str) -> Result<Remote, String> {
    let addr = host.local_addr().unwrap().to_string();
    let (name, password) = (name.to_string(), password.to_string());
    let client = std::thread::spawn(move || Remote::connect(&addr, &name, &password, Duration::from_secs(10)));
    while !client.is_finished() {
        host.pump();
        std::thread::sleep(Duration::from_millis(1));
    }
    client.join().unwrap()
}

fn join(host: &mut Host, name: &str) -> Remote {
    join_as(host, name, "").unwrap_or_else(|e| panic!("{name} can't join: {e}"))
}

/// Pump the host until `done`, failing after a few seconds.
fn until(host: &mut Host, what: &str, mut done: impl FnMut(&mut Host) -> bool) {
    let start = Instant::now();
    loop {
        host.pump();
        if done(host) {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "waited too long for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// One tick with everything each client queued delivered first; returns each client's snapshot.
fn tick(host: &mut Host, clients: &mut [&mut Remote]) -> Vec<Snapshot> {
    for c in clients.iter_mut() {
        c.flush();
    }
    let sent: Vec<(u32, u64)> = clients.iter().map(|c| (c.player(), c.intents_sent)).collect();
    until(host, "the clients' intents", |h| sent.iter().all(|&(p, n)| h.received(p) == n));
    host.tick();
    clients.iter_mut().map(|c| c.wait_snapshot(Duration::from_secs(10)).expect("a snapshot")).collect()
}

fn input(seq: u32, move_dir: Vec2, facing: f32, run: bool, aim: bool) -> (Intent, PlayerInput) {
    (Intent::PlayerInput { player: 0, seq, move_dir: move_dir.to_array(), facing, run, sneak: false, aim }, PlayerInput { move_dir, facing, run, sneak: false, aim })
}

/// A client joins over TCP, builds the server's map itself, and its prediction (the shared
/// movement code on its own map copy) matches the server tick for tick, through a door it opens.
#[test]
fn a_remote_client_joins_and_predicts_exactly() {
    let mut host = host(150, false, HostOptions { spawn: Some(Spawn::Together), ..Default::default() });
    // A door changed before anyone joined: it reaches the client as a map edit.
    let (door, kind) = {
        let m = &host.world.map;
        let mut found = None;
        'find: for y in 0..m.height() {
            for x in 0..m.width() {
                let t = glam::IVec2::new(x, y);
                for edge in [zombye_sim::map::OwnedEdge::North, zombye_sim::map::OwnedEdge::West] {
                    if m.owned_edge(t, edge) == zombye_sim::map::Edge::DoorClosed {
                        found = Some(zombye_sim::map::EdgeRef::new(t, edge));
                        break 'find;
                    }
                }
            }
        }
        (found.expect("a shut door"), zombye_sim::map::Edge::DoorOpen)
    };
    host.world.set_edge(door, kind);

    let mut client = join(&mut host, "ada");
    let me = client.player();
    assert_eq!(client.welcome.name, "ada");
    assert!(!client.welcome.rules.pvp);
    let joined = build_map(&client.welcome, &config(0)).expect("the same content builds the same map");
    assert_eq!(joined.map.owned_edge(door.tile, door.edge), kind, "the open door came with the welcome");
    assert_eq!(zombye_sim::save::map_hash(&joined.map), zombye_sim::save::map_hash(&host.world.map), "the client's map is the server's");
    assert_eq!(joined.config.weapons.len(), host.world.config.weapons.len());

    host.tick();
    let first = client.wait_snapshot(Duration::from_secs(10)).unwrap();
    let view = first.players.iter().find(|p| p.id == me).expect("our player");
    assert_eq!(first.private.len(), 1, "our inventory with the first snapshot");
    assert_eq!(first.runs.len(), 1);
    let mut predicted: PlayerState = view.state();
    let mut seq = view.last_input_seq;
    for t in 0..240u32 {
        seq += 1;
        let a = t as f32 * 0.05;
        let (intent, inp) = input(seq, Vec2::from_angle(a), a, t % 80 < 40, false);
        client.send(intent);
        step_movement(&joined.map, &mut predicted, &inp, TICK_DT);
        let snap = tick(&mut host, &mut [&mut client]).pop().unwrap();
        let view = snap.players.iter().find(|p| p.id == me).unwrap();
        assert_eq!(view.last_input_seq, seq, "one input a tick");
        assert_eq!(view.state(), predicted, "prediction diverged at tick {t}");
        assert_eq!(snap.tick, host.world.tick);
    }
    assert!(predicted.pos.distance(Vec2::from(view.pos)) > 1.0, "it walked");
    // What the client sees of the zombies is the server's: to the wire's precision close by, and
    // within 1/400 of its distance further off (#22).
    let snap = tick(&mut host, &mut [&mut client]).pop().unwrap();
    assert!(!snap.zombies.is_empty(), "zombies in view");
    for z in &snap.zombies {
        let i = host.world.zombies.index_of(z.id).expect("a zombie the server has");
        let dist = host.world.zombies.pos[i].distance(host.world.player(me).unwrap().state.pos);
        assert!(host.world.zombies.pos[i].distance(Vec2::from(z.pos)) < 0.04 + dist / 280.0, "zombie {} ({dist:.0} m away) drawn where it is", z.id);
        assert!(host.world.zombies.pos[i].distance(host.world.player(me).unwrap().state.pos) <= host.world.config.tiers.interest_radius + 0.1, "only what is near us");
    }
}

/// Inputs that arrive in bursts (none one tick, two the next) are still applied one a tick, in
/// order, so each moves the player exactly one tick's worth.
#[test]
fn bursts_of_input_are_applied_one_a_tick() {
    let mut host = host(0, false, HostOptions::default());
    let mut client = join(&mut host, "burst");
    let me = client.player();
    host.tick();
    let start = client.wait_snapshot(Duration::from_secs(10)).unwrap().players.iter().find(|p| p.id == me).unwrap().state();
    let map = host.world.map.clone();
    let mut predicted = start;
    // Three inputs at once, then three ticks.
    for seq in 1..=3 {
        let (intent, inp) = input(seq, Vec2::X, 0.0, false, false);
        client.send(intent);
        step_movement(&map, &mut predicted, &inp, TICK_DT);
    }
    client.flush();
    until(&mut host, "three inputs", |h| h.queued(me) == 3);
    for seq in 1..=3u32 {
        host.tick();
        let p = host.world.player(me).unwrap();
        assert_eq!(p.last_input_seq, seq);
    }
    assert_eq!(host.world.player(me).unwrap().state, predicted);
    // A long stall's worth at once: the server skips to the newest rather than fall behind.
    for seq in 4..=40 {
        client.send(input(seq, Vec2::X, 0.0, false, false).0);
    }
    client.flush();
    until(&mut host, "the backlog", |h| h.queued(me) == 37);
    host.tick();
    assert!(host.world.player(me).unwrap().last_input_seq >= 39, "caught up to {}", host.world.player(me).unwrap().last_input_seq);
}

/// A client acts only as its own player, dev and debug intents are dropped, and the join rules
/// hold: password, room, one client a name, and a name gets its survivor back.
#[test]
fn clients_are_their_own_player_and_the_door_has_rules() {
    let mut host = host(0, false, HostOptions { max_players: 2, password: "brains".into(), ..Default::default() });
    assert!(join_as(&mut host, "eve", "").unwrap_err().contains("wrong password"));
    let mut ada = join_as(&mut host, "ada", "brains").unwrap();
    let mut bob = join_as(&mut host, "bob", "brains").unwrap();
    assert!(join_as(&mut host, "cy", "brains").unwrap_err().contains("full"));
    assert_ne!(ada.player(), bob.player());
    assert_eq!(host.clients().len(), 2);
    tick(&mut host, &mut [&mut ada, &mut bob]);

    // Ada sends input naming Bob's player, a dev toggle, and a debug spawn.
    let bob_before = host.world.player(bob.player()).unwrap().state;
    let ada_before = host.world.player(ada.player()).unwrap().state;
    let zombies = host.world.zombies.len();
    for seq in 1..=10 {
        ada.send(Intent::PlayerInput { player: bob.player(), seq, move_dir: [1.0, 0.0], facing: 0.0, run: false, sneak: false, aim: false });
        ada.send(Intent::Dev { player: ada.player(), god: Some(true), ignored: Some(true) });
        ada.send(Intent::SpawnZombies { count: 50, center: ada_before.pos.to_array(), radius: 5.0 });
        ada.send(Intent::SkipTime { hours: 5.0 });
        tick(&mut host, &mut [&mut ada, &mut bob]);
    }
    assert_eq!(host.world.player(bob.player()).unwrap().state.pos, bob_before.pos, "Bob didn't move");
    assert!(host.world.player(ada.player()).unwrap().state.pos.distance(ada_before.pos) > 0.5, "it moved Ada herself");
    assert_eq!(host.world.player(ada.player()).unwrap().dev, Default::default());
    assert_eq!(host.world.zombies.len(), zombies);
    assert_eq!(host.stats.refused_dev, 30);
    assert_eq!(host.world.tick, 11);

    // Each client's snapshot has only its own private parts.
    let snaps = tick(&mut host, &mut [&mut ada, &mut bob]);
    for (snap, me) in snaps.iter().zip([ada.player(), bob.player()]) {
        assert!(snap.runs.iter().all(|r| r.player == me) && snap.floor.iter().all(|f| f.player == me) && snap.private.iter().all(|v| v.player == me));
        assert_eq!(snap.players.len(), 2, "they start together in co-op, in view of each other");
    }

    // Bob leaves: his survivor stays; nobody else can be Ada meanwhile; Bob gets his back.
    let bob_id = bob.player();
    drop(bob);
    until(&mut host, "Bob to go", |h| {
        h.tick();
        h.clients().len() == 1
    });
    ada.poll();
    assert!(host.world.player(bob_id).is_some());
    assert!(join_as(&mut host, "ada", "brains").unwrap_err().contains("already playing"));
    let bob = join_as(&mut host, "bob", "brains").unwrap();
    assert_eq!(bob.player(), bob_id, "the same survivor");
    assert_eq!(host.world.players.len(), 2);
    assert_eq!(host.names().get("bob"), Some(&bob_id));
}

/// PvP is the server's rule: on, one client's swing wounds another's survivor and both hear of
/// it; off (co-op) it passes through.
#[test]
fn pvp_is_a_server_rule() {
    for pvp in [true, false] {
        let mut host = host(0, pvp, HostOptions { spawn: Some(Spawn::Together), ..Default::default() });
        let mut ada = join(&mut host, "ada");
        let mut bob = join(&mut host, "bob");
        assert_eq!(ada.welcome.rules.pvp, pvp);
        // Face to face, Ada with a bat.
        let at = host.world.player(ada.player()).unwrap().state.pos;
        host.world.players.iter_mut().find(|p| p.id == bob.player()).unwrap().state.pos = at + Vec2::new(0.9, 0.0);
        assert!(host.world.hold_item(ada.player(), zombye_sim::Slot::Hand, "baseball_bat"));
        let mut hurt = 0;
        let mut bob_hp = 100.0;
        for seq in 1..=60 {
            ada.send(input(seq, Vec2::ZERO, 0.0, false, true).0);
            ada.send(Intent::Attack { player: 0 });
            let snaps = tick(&mut host, &mut [&mut ada, &mut bob]);
            hurt += snaps[1].hits.iter().filter(|h| h.kind == CombatEventKind::PlayerHurt && h.player == bob.player()).count();
            bob_hp = snaps[1].players.iter().find(|p| p.id == bob.player()).unwrap().hp;
        }
        if pvp {
            assert!(hurt >= 1 && bob_hp < 100.0, "pvp on: {hurt} hits, hp {bob_hp}");
        } else {
            assert!(hurt == 0 && bob_hp == 100.0, "co-op: {hurt} hits, hp {bob_hp}");
        }
    }
}

/// The issue's size: 32 clients on one server, each walking about a town of some 3,000 zombies,
/// within the tick budget (50 ms) and a bandwidth budget per client. Spawns are spread round the
/// town (PvP on), so most clients see different zombies.
#[test]
fn thirty_two_clients_share_a_world_within_budget() {
    const CLIENTS: usize = 32;
    const TICKS: u32 = 200;
    /// Bytes a second down to one client: a quarter of a megabit.
    const BUDGET: f64 = 32_000.0;
    let mut config = content::load_config(&content::default_data_dir(), 1).unwrap();
    config.combat.pvp = true;
    let mut host = Host::bind(World::new(config), "127.0.0.1:0", HostOptions::default()).unwrap();
    let mut clients: Vec<Remote> = (0..CLIENTS).map(|k| join(&mut host, &format!("bot-{k}"))).collect();
    assert_eq!(host.world.players.len(), CLIENTS);
    // Spread out: no two start on top of each other.
    let starts: Vec<Vec2> = host.world.players.iter().map(|p| p.state.pos).collect();
    for (i, a) in starts.iter().enumerate() {
        for b in &starts[i + 1..] {
            assert!(a.distance(*b) > 3.0, "two survivors start {} m apart", a.distance(*b));
        }
    }
    host.tick();
    for c in &mut clients {
        c.wait_snapshot(Duration::from_secs(10)).unwrap();
    }
    let (out0, in0) = (host.stats.bytes_out, host.stats.bytes_in);
    let centre = (host.world.town_min + host.world.town_max).as_vec2() * 0.5;
    let (mut worst, mut sum) = (0.0f32, 0.0f32);
    let mut seen = 0usize;
    for t in 0..TICKS {
        for (k, c) in clients.iter_mut().enumerate() {
            // Into town, weaving.
            let me = host.world.player(c.player()).unwrap().state.pos;
            let dir = (centre - me).normalize_or_zero().rotate(Vec2::from_angle((t as f32 * 0.05 + k as f32).sin()));
            c.send(Intent::PlayerInput { player: 0, seq: t + 1, move_dir: dir.to_array(), facing: dir.to_angle(), run: t % 100 < 30, sneak: false, aim: false });
        }
        let mut refs: Vec<&mut Remote> = clients.iter_mut().collect();
        let snaps = tick(&mut host, &mut refs);
        worst = worst.max(host.stats.tick_ms);
        sum += host.stats.tick_ms;
        seen = seen.max(snaps.iter().map(|s| s.zombies.len()).max().unwrap_or(0));
        for s in &snaps {
            assert_eq!(s.tick, host.world.tick);
        }
    }
    let secs = TICKS as f64 * TICK_DT as f64;
    let down = (host.stats.bytes_out - out0) as f64 / secs / CLIENTS as f64;
    let up = (host.stats.bytes_in - in0) as f64 / secs / CLIENTS as f64;
    let worst_client = host.clients().iter().map(|c| c.bytes_out).max().unwrap() as f64 / secs;
    println!(
        "{CLIENTS} clients, {} zombies ({} active), {TICKS} ticks: tick {:.2} ms mean, {worst:.2} ms worst; per client {:.1} KB/s down (worst {:.1}), {:.2} KB/s up; most zombies in one view {seen}",
        host.world.zombies.len(),
        host.world.stats.active,
        sum / TICKS as f32,
        down / 1000.0,
        worst_client / 1000.0,
        up / 1000.0
    );
    assert!(sum / (TICKS as f32) < 50.0, "the mean tick is over budget");
    // Well inside it since #22 (about 5 KB/s): zombies are carried on between corrections.
    assert!(down < BUDGET / 2.0, "{down:.0} bytes a second down per client");
    assert!(worst_client < BUDGET, "{worst_client:.0} bytes a second down to the busiest client");
    assert_eq!(host.stats.held, 0, "and nothing had to wait for it");
    assert!(up < 2_000.0, "{up:.0} bytes a second up per client");
    for p in &host.world.players {
        assert_eq!(p.last_input_seq, TICKS, "every client's every input was applied");
    }
}

/// The worst case for a client's budget (#22): all 32 survivors in one place (co-op) with a horde
/// of 1,500 round them, everyone in everyone's view. Each client's snapshots stay within its
/// budget (far zombies wait their turn), nobody is dropped, and every client still has the whole
/// horde, the zombies close to it where the server has them.
#[test]
fn thirty_two_clients_in_one_horde_keep_to_their_budget() {
    const CLIENTS: usize = 32;
    const TICKS: u32 = 240;
    let mut config = config(0);
    config.combat.pvp = false;
    // Under the default budget (which this horde fits, at about 25 KB/s), so it is more than fits.
    let options = HostOptions { spawn: Some(Spawn::Together), budget: 20_000, ..Default::default() };
    let budget = options.budget as f64;
    let mut host = Host::bind(World::new(config), "127.0.0.1:0", options).unwrap();
    let mut clients: Vec<Remote> = (0..CLIENTS).map(|k| join(&mut host, &format!("bot-{k}"))).collect();
    let centre = host.world.players.iter().map(|p| p.state.pos).sum::<Vec2>() / CLIENTS as f32;
    let ids: Vec<u32> = host.world.players.iter().map(|p| p.id).collect();
    for id in ids {
        host.world.set_dev(id, Some(true), None);
    }
    assert!(host.world.spawn_zombies(1500, centre, 60.0) > 1400);
    let mut seq = 0;
    let mut step = |host: &mut Host, clients: &mut Vec<Remote>| {
        seq += 1;
        for (k, c) in clients.iter_mut().enumerate() {
            let dir = Vec2::from_angle(seq as f32 * 0.04 + k as f32);
            c.send(Intent::PlayerInput { player: 0, seq, move_dir: dir.to_array(), facing: dir.to_angle(), run: false, sneak: false, aim: false });
        }
        let mut refs: Vec<&mut Remote> = clients.iter_mut().collect();
        tick(host, &mut refs)
    };
    // Past the first rush (everything is new to everyone), then measure.
    for _ in 0..60 {
        step(&mut host, &mut clients);
    }
    let before: Vec<u64> = host.clients().iter().map(|c| c.bytes_out).collect();
    let held0 = host.stats.held;
    let (mut worst_tick, mut near_off, mut fewest) = (0.0f32, 0.0f32, usize::MAX);
    for _ in 0..TICKS {
        let snaps = step(&mut host, &mut clients);
        worst_tick = worst_tick.max(host.stats.tick_ms);
        let z = &host.world.zombies;
        for (c, s) in clients.iter().zip(&snaps) {
            fewest = fewest.min(s.zombies.len());
            let me = host.world.player(c.player()).unwrap().state.pos;
            for v in s.zombies.iter().filter(|v| me.distance(v.pos.into()) < 6.0) {
                let k = z.id.binary_search(&v.id).expect("a zombie the server has");
                near_off = near_off.max(z.pos[k].distance(v.pos.into()));
            }
        }
    }
    let secs = TICKS as f64 * TICK_DT as f64;
    let rates: Vec<f64> = host.clients().iter().zip(&before).map(|(c, b)| (c.bytes_out - b) as f64 / secs).collect();
    let (mean, worst) = (rates.iter().sum::<f64>() / CLIENTS as f64, rates.iter().cloned().fold(0.0, f64::max));
    println!(
        "{CLIENTS} clients in one horde of {}: per client {:.1} KB/s down (worst {:.1}, budget {:.0}), {} zombie updates held back a tick per client, at least {fewest} zombies in a view; within 6 m off by at most {:.1} cm; worst tick {worst_tick:.1} ms",
        host.world.zombies.len(),
        mean / 1000.0,
        worst / 1000.0,
        budget / 1000.0,
        (host.stats.held - held0) / TICKS as u64 / CLIENTS as u64,
        near_off * 100.0,
    );
    assert_eq!(host.clients().len(), CLIENTS, "nobody was dropped");
    assert!(worst <= budget * 1.05, "{worst:.0} bytes a second down to the busiest client (budget {budget:.0})");
    assert!(host.stats.held > held0, "the budget was what held it there");
    assert!(fewest > 1200, "every client has the horde ({fewest} zombies in the emptiest view)");
    assert!(near_off < 0.15, "zombies in reach are where the server has them ({near_off} m)");
    assert!(worst_tick < 50.0, "a tick took {worst_tick:.1} ms");
}
