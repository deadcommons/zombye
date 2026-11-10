//! Fair information over real sockets (#66, docs/PLAN.md §4.8): what a client's snapshots hold of
//! another player is only what its survivor could see or hear. A [`Host`] and two [`Remote`]
//! clients on the loopback interface, the host ticked by hand.

use std::time::{Duration, Instant};

use glam::{IVec2, Vec2};
use zombye_net::host::{Host, HostOptions, Spawn};
use zombye_net::remote::Remote;
use zombye_net::{Intent, Snapshot, content};
use zombye_sim::map::{CHUNK_SIZE, Edge, Side, TileMap};
use zombye_sim::{TICK_RATE, World};

/// An empty field at noon with one long wall north to south along x = 103, and nobody else.
fn host(pvp: bool, options: HostOptions) -> Host {
    let mut config = content::load_config(&content::default_data_dir(), 1).expect("the shipped content");
    config.zombies = Some(0);
    config.combat.pvp = pvp;
    config.start_hour = 12.0;
    config.needs = zombye_sim::NeedsTuning::off();
    let mut world = World::new(config);
    world.map = TileMap::new(world.map.width() / CHUNK_SIZE, world.map.height() / CHUNK_SIZE);
    for y in 0..world.map.height() {
        world.map.set_edge(IVec2::new(103, y), Side::W, Edge::Wall);
    }
    world.map_changed();
    Host::bind(world, "127.0.0.1:0", options).expect("a free port")
}

fn join(host: &mut Host, name: &str) -> Remote {
    let addr = host.local_addr().unwrap().to_string();
    let name = name.to_string();
    let client = std::thread::spawn(move || Remote::connect(&addr, &name, "", Duration::from_secs(10)));
    while !client.is_finished() {
        host.pump();
        std::thread::sleep(Duration::from_millis(1));
    }
    client.join().unwrap().expect("joined")
}

/// One tick with everything each client queued delivered first; returns each client's snapshot.
fn tick(host: &mut Host, clients: &mut [&mut Remote]) -> Vec<Snapshot> {
    for c in clients.iter_mut() {
        c.flush();
    }
    let sent: Vec<(u32, u64)> = clients.iter().map(|c| (c.player(), c.intents_sent)).collect();
    let start = Instant::now();
    while !sent.iter().all(|&(p, n)| host.received(p) == n) {
        host.pump();
        assert!(start.elapsed() < Duration::from_secs(10), "waited too long for the clients' intents");
        std::thread::sleep(Duration::from_millis(1));
    }
    host.tick();
    clients.iter_mut().map(|c| c.wait_snapshot(Duration::from_secs(10)).expect("a snapshot")).collect()
}

/// Ada and Bob on the field: Ada at (100.5, 100.5), 2.5 m west of the wall, facing east.
struct Scene {
    host: Host,
    ada: Remote,
    bob: Remote,
    seq: u32,
}

const ADA: Vec2 = Vec2::new(100.5, 100.5);

impl Scene {
    fn new(pvp: bool, options: HostOptions) -> Self {
        let mut host = host(pvp, HostOptions { spawn: Some(Spawn::Together), ..options });
        let ada = join(&mut host, "ada");
        let bob = join(&mut host, "bob");
        let mut s = Self { host, ada, bob, seq: 0 };
        s.put(s.ada.player(), ADA);
        s
    }

    fn put(&mut self, player: u32, at: Vec2) {
        self.host.world.players.iter_mut().find(|p| p.id == player).unwrap().state.pos = at;
    }

    fn put_bob(&mut self, at: Vec2) {
        self.put(self.bob.player(), at);
    }

    /// `n` ticks with Ada facing `facing` and Bob holding `bob` (a move direction, running or
    /// sneaking); returns Ada's snapshots.
    fn run(&mut self, n: u32, facing: f32, bob: (Vec2, bool, bool)) -> Vec<Snapshot> {
        (0..n)
            .map(|_| {
                self.seq += 1;
                self.ada.send(Intent::PlayerInput { player: 0, seq: self.seq, move_dir: [0.0; 2], facing, run: false, sneak: false, aim: false });
                self.bob.send(Intent::PlayerInput { player: 0, seq: self.seq, move_dir: bob.0.to_array(), facing: 0.0, run: bob.1, sneak: bob.2, aim: false });
                tick(&mut self.host, &mut [&mut self.ada, &mut self.bob]).swap_remove(0)
            })
            .collect()
    }

    /// Two seconds standing (longer than the grace period); Ada's last snapshot.
    fn settle(&mut self, facing: f32) -> Snapshot {
        self.run(2 * TICK_RATE, facing, (Vec2::ZERO, false, false)).pop().unwrap()
    }
}

fn has(snap: &Snapshot, player: u32) -> bool {
    snap.players.iter().any(|p| p.id == player)
}

/// The issue's "done when": a player behind a wall, outside the view cone or in the dark doesn't
/// appear in another client's snapshot, and one in plain view does.
#[test]
fn a_snapshot_lists_another_player_only_when_our_survivor_sees_them() {
    let mut s = Scene::new(true, HostOptions::default());
    let (ada, bob) = (s.ada.player(), s.bob.player());
    assert!(s.ada.welcome.rules.fair_sight, "on with PvP");

    // In plain view: 2 m ahead of Ada, this side of the wall.
    s.put_bob(ADA + Vec2::new(1.5, 0.0));
    let snap = s.settle(0.0);
    assert!(has(&snap, ada) && has(&snap, bob), "in plain view");
    assert!(snap.outfits.iter().any(|o| o.id == bob), "and what it wears");
    let seen = snap.players.iter().find(|p| p.id == bob).unwrap();
    assert!(Vec2::from(seen.pos).distance(ADA + Vec2::new(1.5, 0.0)) < 0.05);

    // Behind the wall, 4 m from Ada: for the grace period it is still told, then not.
    s.put_bob(ADA + Vec2::new(4.0, 0.0));
    let snaps = s.run(2 * TICK_RATE, 0.0, (Vec2::ZERO, false, false));
    assert!(has(&snaps[0], bob), "a moment's grace");
    let snap = snaps.last().unwrap();
    assert!(has(snap, ada) && !has(snap, bob), "behind a wall");
    assert!(snap.outfits.iter().all(|o| o.id != bob), "nor its outfit");
    assert!(snaps.iter().all(|s| s.heard.is_empty()), "standing still, it makes no sound");

    // Outside the view cone: 20 m behind Ada in the open.
    s.put_bob(ADA - Vec2::new(20.0, 0.0));
    let snap = s.settle(0.0);
    assert!(!has(&snap, bob), "behind our back");
    // Ada turns round.
    let snap = s.settle(std::f32::consts::PI);
    assert!(has(&snap, bob), "now we face it");

    // In the dark (1 am): 20 m off in the open, in front of us, not; 6 m off, yes.
    s.host.world.skip_time(13.0);
    let snap = s.settle(std::f32::consts::PI);
    assert!(snap.time_of_day > 0.5 && snap.time_of_day < 2.0, "{}", snap.time_of_day);
    assert!(!has(&snap, bob), "in the dark");
    s.put_bob(ADA - Vec2::new(6.0, 0.0));
    let snap = s.settle(std::f32::consts::PI);
    assert!(has(&snap, bob), "close enough even at night");
    // Bob's client, for its part, always has Bob.
    assert!(has(&tick(&mut s.host, &mut [&mut s.ada, &mut s.bob])[1], bob));
}

/// A player heard and not seen is a sound at a position; its blows and what it drops stay unknown.
#[test]
fn an_unseen_player_is_only_a_sound() {
    let mut s = Scene::new(true, HostOptions::default());
    let bob = s.bob.player();
    let behind = ADA + Vec2::new(4.0, 0.0);
    s.put_bob(behind);
    s.settle(0.0);

    // Bob runs along the far side of the wall: Ada's client gets sounds, never the player.
    let snaps = s.run(TICK_RATE, 0.0, (Vec2::Y, true, false));
    assert!(snaps.iter().all(|s| !has(s, bob)), "never listed");
    let heard: Vec<_> = snaps.iter().flat_map(|s| s.heard.iter()).collect();
    assert!(!heard.is_empty() && heard.len() <= 4, "heard now and then: {}", heard.len());
    assert!(heard[0].pos.distance(behind) < 1.0 && heard[0].radius > 10.0, "where it ran, as loud as running: {:?}", heard[0]);
    // Sneaking back, nothing.
    let snaps = s.run(2 * TICK_RATE, 0.0, (-Vec2::Y, false, true));
    assert!(snaps[TICK_RATE as usize..].iter().all(|s| s.heard.is_empty() && !has(s, bob)), "a sneaker behind a wall");

    // Something dropped behind the wall isn't on Ada's list; on her side it is. Bob has both.
    s.put_bob(behind);
    let hidden = s.host.world.spawn_item("tinned_beans", 1, behind + Vec2::new(0.5, 0.5)).expect("an item");
    let shown = s.host.world.spawn_item("tinned_beans", 1, ADA + Vec2::new(1.0, 1.0)).expect("an item");
    let snap = s.settle(0.0);
    assert!(snap.items.iter().any(|i| i.id == shown), "in the open");
    assert!(snap.items.iter().all(|i| i.id != hidden), "behind the wall");
    let bobs = tick(&mut s.host, &mut [&mut s.ada, &mut s.bob]).swap_remove(1);
    assert!(bobs.items.iter().any(|i| i.id == hidden), "its own side of the wall");

    // Bob swings a bat at nothing behind the wall: no hit reaches Ada, only (perhaps) the noise.
    assert!(s.host.world.hold_item(bob, zombye_sim::Slot::Hand, "baseball_bat"));
    let mut hits = 0;
    for _ in 0..TICK_RATE * 2 {
        s.seq += 1;
        s.bob.send(Intent::PlayerInput { player: 0, seq: s.seq, move_dir: [0.0; 2], facing: 0.0, run: false, sneak: false, aim: true });
        s.bob.send(Intent::Attack { player: 0 });
        let snap = tick(&mut s.host, &mut [&mut s.ada, &mut s.bob]).swap_remove(0);
        hits += snap.hits.len();
        assert!(!has(&snap, bob) && snap.outfits.iter().all(|o| o.id != bob), "a bat in hand behind the wall stays unknown");
    }
    assert_eq!(hits, 0);
}

/// The rule is the server's: off (co-op's default, or `--fair-sight off`), everyone within the
/// interest radius is told, walls or not, and nothing is sent as a sound.
#[test]
fn fair_sight_is_a_server_rule() {
    for (pvp, option, fair) in [(false, None, false), (true, Some(false), false), (false, Some(true), true), (true, None, true)] {
        let mut s = Scene::new(pvp, HostOptions { fair_sight: option, ..Default::default() });
        let bob = s.bob.player();
        assert_eq!(s.ada.welcome.rules.fair_sight, fair, "pvp {pvp}, --fair-sight {option:?}");
        s.put_bob(ADA + Vec2::new(4.0, 0.0));
        s.settle(0.0);
        let snaps = s.run(TICK_RATE, 0.0, (Vec2::Y, true, false));
        assert_eq!(snaps.iter().all(|s| has(s, bob)), !fair, "behind the wall, pvp {pvp}, --fair-sight {option:?}");
        assert_eq!(snaps.iter().any(|s| !s.heard.is_empty()), fair);
    }
}
