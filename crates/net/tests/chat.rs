//! Proximity chat and gestures over real sockets (#67, docs/PLAN.md §4.8): a [`Host`] and three
//! [`Remote`] clients on the loopback interface, the host ticked by hand. The issue's "done
//! when": two players talk in text across a room and through a wall, and a third out of range
//! hears nothing.

use std::time::{Duration, Instant};

use glam::{IVec2, Vec2};
use zombye_net::host::{Host, HostOptions, Spawn};
use zombye_net::remote::Remote;
use zombye_net::{Intent, Snapshot, content};
use zombye_sim::map::{CHUNK_SIZE, Edge, Side, TileMap};
use zombye_sim::{Gesture, Said, TICK_RATE, Volume, World};

/// An empty field at noon with one long wall north to south along x = 103, and no zombies.
fn host(pvp: bool) -> Host {
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
    Host::bind(world, "127.0.0.1:0", HostOptions { spawn: Some(Spawn::Together), ..Default::default() }).expect("a free port")
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

const ADA: Vec2 = Vec2::new(100.5, 100.5);

/// Ada 2.5 m west of the wall facing east, Bob wherever a test puts him, Cy 30 m north of Ada.
struct Scene {
    host: Host,
    ada: Remote,
    bob: Remote,
    cy: Remote,
    seq: u32,
}

impl Scene {
    fn new(pvp: bool) -> Self {
        let mut host = host(pvp);
        let (ada, bob, cy) = (join(&mut host, "ada"), join(&mut host, "bob"), join(&mut host, "cy"));
        let mut s = Self { host, ada, bob, cy, seq: 0 };
        s.put(s.ada.player(), ADA);
        s.put(s.cy.player(), ADA - Vec2::new(0.0, 30.0));
        s
    }

    fn put(&mut self, player: u32, at: Vec2) {
        self.host.world.players.iter_mut().find(|p| p.id == player).unwrap().state.pos = at;
    }

    fn put_bob(&mut self, at: Vec2) {
        self.put(self.bob.player(), at);
        self.run(2 * TICK_RATE);
    }

    /// One tick, everyone standing (Ada facing east), with whatever was queued delivered first.
    /// Returns Ada's, Bob's and Cy's snapshots.
    fn tick(&mut self) -> [Snapshot; 3] {
        self.seq += 1;
        let mut clients = [&mut self.ada, &mut self.bob, &mut self.cy];
        for c in clients.iter_mut() {
            c.send(Intent::PlayerInput { player: 0, seq: self.seq, move_dir: [0.0; 2], facing: 0.0, run: false, sneak: false, aim: false });
            c.flush();
        }
        let sent: Vec<(u32, u64)> = clients.iter().map(|c| (c.player(), c.intents_sent)).collect();
        let start = Instant::now();
        while !sent.iter().all(|&(p, n)| self.host.received(p) == n) {
            self.host.pump();
            assert!(start.elapsed() < Duration::from_secs(10), "waited too long for the clients' intents");
            std::thread::sleep(Duration::from_millis(1));
        }
        self.host.tick();
        clients.map(|c| c.wait_snapshot(Duration::from_secs(10)).expect("a snapshot"))
    }

    /// `n` ticks; everything each client heard said in them.
    fn run(&mut self, n: u32) -> [Vec<Said>; 3] {
        let mut heard: [Vec<Said>; 3] = Default::default();
        for _ in 0..n {
            for (all, snap) in heard.iter_mut().zip(self.tick()) {
                all.extend(snap.speech);
            }
        }
        heard
    }
}

#[test]
fn two_players_talk_across_a_room_and_through_a_wall_and_a_third_out_of_range_hears_nothing() {
    let mut s = Scene::new(true);
    let (ada, bob) = (s.ada.player(), s.bob.player());

    // Across the room: Bob 8 m off in the open, where Ada sees him.
    s.put_bob(ADA + Vec2::new(6.0, 0.0) - Vec2::new(4.0, -7.0));
    let bob_at = s.host.world.player(bob).unwrap().state.pos;
    s.ada.send(Intent::Say { player: 0, text: "Hello? Friendly.".into(), volume: Volume::Talk });
    let [a, b, c] = s.run(3);
    assert_eq!(a.len(), 1, "her own line back, once");
    assert!(a[0].speaker == ada && a[0].text == "Hello? Friendly." && !a[0].muffled);
    assert_eq!(b.len(), 1);
    assert!(b[0].speaker == ada && b[0].text == "Hello? Friendly." && !b[0].muffled && b[0].pos.distance(ADA) < 0.01 && b[0].volume == Volume::Talk, "{:?}", b[0]);
    assert!(c.is_empty(), "30 m off: nothing");
    s.bob.send(Intent::Say { player: 0, text: "Friendly. Don't swing.".into(), volume: Volume::Talk });
    let [a, _, c] = s.run(3);
    assert!(a.len() == 1 && a[0].speaker == bob && a[0].pos.distance(bob_at) < 0.01 && !a[0].muffled, "seen, so she knows who: {a:?}");
    assert!(c.is_empty());

    // Through the wall: Bob 4 m east of Ada, the wall between. She hears it muffled, and not from whom.
    s.put_bob(ADA + Vec2::new(4.0, 0.0));
    s.bob.send(Intent::Say { player: 0, text: "I'm behind the wall.".into(), volume: Volume::Talk });
    let [a, b, c] = s.run(3);
    assert!(a.len() == 1 && a[0].muffled && a[0].text == "I'm behind the wall.", "{a:?}");
    assert_eq!(a[0].speaker, 0, "a voice, not a name: she doesn't see him");
    assert!(a[0].pos.distance(ADA + Vec2::new(4.0, 0.0)) < 0.01, "from where it came");
    assert!(b.len() == 1 && b[0].speaker == bob, "his own line");
    assert!(c.is_empty());
    s.ada.send(Intent::Say { player: 0, text: "I hear you.".into(), volume: Volume::Talk });
    let [_, b, c] = s.run(3);
    assert!(b.len() == 1 && b[0].muffled && b[0].speaker == 0 && b[0].text == "I hear you.", "{b:?}");
    assert!(c.is_empty(), "the third never heard a word");

    // Further behind the wall talk doesn't get through; a shout does, and reaches the third.
    s.put_bob(ADA + Vec2::new(9.0, 0.0));
    s.bob.send(Intent::Say { player: 0, text: "Still there?".into(), volume: Volume::Talk });
    let [a, _, _] = s.run(3);
    assert!(a.is_empty(), "9 m and a wall");
    s.run(TICK_RATE);
    s.bob.send(Intent::Say { player: 0, text: "STILL THERE?".into(), volume: Volume::Shout });
    let [a, _, c] = s.run(3);
    assert!(a.len() == 1 && a[0].muffled && a[0].volume == Volume::Shout);
    assert!(c.is_empty(), "31 m and a wall: a shout only gets 16 m through one");
    s.ada.send(Intent::Say { player: 0, text: "YES!".into(), volume: Volume::Shout });
    let [_, b, c] = s.run(3);
    assert!(b.len() == 1 && b[0].muffled);
    assert!(c.len() == 1 && c[0].text == "YES!" && !c[0].muffled, "in the open a shout carries 40 m");
}

#[test]
fn a_server_takes_a_line_every_half_second() {
    let mut s = Scene::new(true);
    s.put_bob(ADA + Vec2::new(1.5, 0.0));
    for k in 0..5 {
        s.bob.send(Intent::Say { player: 0, text: format!("line {k}"), volume: Volume::Talk });
    }
    let [a, _, _] = s.run(8);
    assert!(a.len() == 1 && a[0].text == "line 0", "{a:?}");
    s.run(4);
    s.bob.send(Intent::Say { player: 0, text: "later".into(), volume: Volume::Talk });
    let [a, _, _] = s.run(3);
    assert!(a.len() == 1 && a[0].text == "later");
    // Nobody speaks as someone else: the server puts the sender's own player on every line.
    s.run(TICK_RATE);
    s.bob.send(Intent::Say { player: s.ada.player(), text: "I am Ada".into(), volume: Volume::Talk });
    let [a, _, _] = s.run(3);
    assert!(a.len() == 1 && a[0].speaker == s.bob.player());
}

#[test]
fn gestures_reach_those_who_see_them() {
    let mut s = Scene::new(true);
    let bob = s.bob.player();
    s.put_bob(ADA + Vec2::new(1.5, 0.0));
    let gesture = |snap: &Snapshot| snap.players.iter().find(|p| p.id == bob).and_then(|p| p.gesture);
    for g in Gesture::ALL {
        s.bob.send(Intent::Gesture { player: 0, gesture: Some(g) });
        let [a, b, c] = s.tick();
        assert_eq!(gesture(&a), Some(g), "Ada sees it");
        assert_eq!(gesture(&b), Some(g), "his own client too");
        assert_eq!(gesture(&c), Some(g), "and Cy, 30 m off with him in view");
    }
    // Behind the wall nobody sees him, so nobody is told.
    s.put(bob, ADA + Vec2::new(4.0, 0.0));
    for _ in 0..2 * TICK_RATE {
        s.bob.send(Intent::Gesture { player: 0, gesture: Some(Gesture::Wave) });
        s.tick();
    }
    assert!(s.tick()[0].players.iter().all(|p| p.id != bob));
    s.put(bob, ADA + Vec2::new(1.5, 0.0));
    s.run(TICK_RATE);
    s.bob.send(Intent::Gesture { player: 0, gesture: Some(Gesture::Point) });
    s.tick();
    // The last was a point: it runs out. Hands up stays until lowered.
    let secs = s.host.world.config.speech.gesture_secs;
    for _ in 0..(secs * TICK_RATE as f32) as u32 + 2 {
        s.tick();
    }
    assert_eq!(gesture(&s.tick()[0]), None);
    s.bob.send(Intent::Gesture { player: 0, gesture: Some(Gesture::HandsUp) });
    for _ in 0..5 * TICK_RATE {
        s.tick();
    }
    assert_eq!(gesture(&s.tick()[0]), Some(Gesture::HandsUp));
    s.bob.send(Intent::Gesture { player: 0, gesture: None });
    assert_eq!(gesture(&s.tick()[0]), None);
}

/// In co-op (fair information off) a voice through a wall still comes muffled, but with its speaker.
#[test]
fn in_co_op_a_voice_has_a_name() {
    let mut s = Scene::new(false);
    let bob = s.bob.player();
    s.put_bob(ADA + Vec2::new(4.0, 0.0));
    s.bob.send(Intent::Say { player: 0, text: "Over here.".into(), volume: Volume::Talk });
    let [a, _, c] = s.run(3);
    assert!(a.len() == 1 && a[0].muffled && a[0].speaker == bob, "{a:?}");
    assert!(c.is_empty());
}
