//! Hold-ups and groups over real sockets (#73, docs/PLAN.md §4.8): a [`Host`] and three [`Remote`]
//! clients on the loopback interface, the host ticked by hand. One puts its hands up, another
//! ties them, searches it and takes from it; two form a group, see each other's names and share
//! a marker, and the third is told none of it.

use std::time::{Duration, Instant};

use glam::Vec2;
use zombye_net::host::{Host, HostOptions, Spawn};
use zombye_net::remote::Remote;
use zombye_net::{GroupOp, Intent, PrivateView, Snapshot, content};
use zombye_sim::map::{CHUNK_SIZE, TileMap};
use zombye_sim::{Gesture, HandsAct, ItemNote, Place, TICK_RATE, World};

fn host() -> Host {
    let mut config = content::load_config(&content::default_data_dir(), 1).expect("the shipped content");
    config.zombies = Some(0);
    config.combat.pvp = true;
    config.start_hour = 12.0;
    config.needs = zombye_sim::NeedsTuning::off();
    let mut world = World::new(config);
    world.map = TileMap::new(world.map.width() / CHUNK_SIZE, world.map.height() / CHUNK_SIZE);
    world.map_changed();
    Host::bind(world, "127.0.0.1:0", HostOptions { spawn: Some(Spawn::Together), spawn_protection_secs: 0.0, ..Default::default() }).expect("a free port")
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

/// Ada facing east, Bob 1 m in front of her facing her, Cy 30 m north.
struct Scene {
    host: Host,
    clients: [Remote; 3],
    seq: u32,
    /// Ada aims (to swing).
    aim: bool,
    /// The latest of each client's own private view, snapshot and notes.
    private: [Option<PrivateView>; 3],
    last: [Option<Snapshot>; 3],
    notes: [Vec<ItemNote>; 3],
}

impl Scene {
    fn new() -> Self {
        let mut host = host();
        let clients = [join(&mut host, "ada"), join(&mut host, "bob"), join(&mut host, "cy")];
        let mut s = Self { host, clients, seq: 0, aim: false, private: Default::default(), last: Default::default(), notes: Default::default() };
        for (k, at) in [ADA, ADA + Vec2::new(0.8, 0.0), ADA - Vec2::new(0.0, 30.0)].into_iter().enumerate() {
            let id = s.clients[k].player();
            let p = s.host.world.players.iter_mut().find(|p| p.id == id).unwrap();
            p.state.pos = at;
            p.inventory = Default::default();
        }
        s.run(1.0);
        s
    }

    fn id(&self, k: usize) -> u32 {
        self.clients[k].player()
    }

    fn tick(&mut self) {
        self.seq += 1;
        for (k, c) in self.clients.iter_mut().enumerate() {
            let facing = if k == 1 { std::f32::consts::PI } else { 0.0 };
            c.send(Intent::PlayerInput { player: 0, seq: self.seq, move_dir: [0.0; 2], facing, run: false, sneak: false, aim: self.aim && k == 0 });
            c.flush();
        }
        let sent: Vec<(u32, u64)> = self.clients.iter().map(|c| (c.player(), c.intents_sent)).collect();
        let start = Instant::now();
        while !sent.iter().all(|&(p, n)| self.host.received(p) == n) {
            self.host.pump();
            assert!(start.elapsed() < Duration::from_secs(10), "waited too long for the clients' intents");
            std::thread::sleep(Duration::from_millis(1));
        }
        self.host.tick();
        for (k, c) in self.clients.iter_mut().enumerate() {
            let mut snap = c.wait_snapshot(Duration::from_secs(10)).expect("a snapshot");
            if let Some(v) = snap.private.pop() {
                self.private[k] = Some(v);
            }
            self.notes[k].append(&mut snap.item_notes);
            self.last[k] = Some(snap);
        }
    }

    fn run(&mut self, secs: f32) {
        for _ in 0..(secs * TICK_RATE as f32).round() as u32 {
            self.tick();
        }
    }

    fn snap(&self, k: usize) -> &Snapshot {
        self.last[k].as_ref().unwrap()
    }

    fn private(&self, k: usize) -> &PrivateView {
        self.private[k].as_ref().unwrap()
    }

    fn noted(&self, k: usize, text: &str) -> bool {
        self.notes[k].iter().any(|n| n.text.contains(text))
    }
}

#[test]
fn a_hold_up_over_the_network() {
    let mut s = Scene::new();
    let (ada, bob) = (s.id(0), s.id(1));
    s.host.world.give_item(ada, "cable_tie", 2);
    s.host.world.give_item(ada, "scissors", 1);
    s.host.world.give_item(bob, "tinned_beans", 2);
    let seen = |s: &Scene, k: usize, id: u32| s.snap(k).players.iter().find(|p| p.id == id).copied();

    // Hands up, and Ada ties them.
    s.clients[0].send(Intent::Restrain { player: 0, target: None });
    s.run(0.2);
    assert!(s.noted(0, "hands up"), "refused until he surrenders: {:?}", s.notes[0]);
    s.clients[1].send(Intent::Gesture { player: 0, gesture: Some(Gesture::HandsUp) });
    s.run(0.2);
    assert_eq!(seen(&s, 0, bob).unwrap().gesture, Some(Gesture::HandsUp));
    s.clients[0].send(Intent::Restrain { player: 0, target: Some(bob) });
    s.run(1.0);
    let own = seen(&s, 0, ada).unwrap();
    assert!(own.hands == Some(HandsAct::Tie) && own.hands_left > 1.5 && own.hands_total == 3.0, "her bar: {own:?}");
    assert_eq!(seen(&s, 1, ada).unwrap().hands, Some(HandsAct::Tie), "he sees what she is at");
    s.run(2.3);
    assert!(seen(&s, 0, bob).unwrap().restrained, "she sees his hands tied");
    let his = seen(&s, 1, bob).unwrap();
    assert!(his.restrained && !his.mods.can_run && his.gesture.is_none(), "{his:?}");
    assert!(s.private(1).restraint.is_some());
    assert!(s.noted(1, "Your hands are tied"));
    // Tied, his things are out of his reach, and he can't swing.
    s.clients[1].send(Intent::Drop { player: 0, from: Place::Carried, index: 0, count: 0 });
    s.clients[1].send(Intent::Attack { player: 0 });
    s.run(0.5);
    assert!(s.noted(1, "your hands are tied") && seen(&s, 0, bob).unwrap().action.is_none());

    // She searches him and takes the tins.
    s.clients[0].send(Intent::Frisk { player: 0, target: None });
    s.run(2.2);
    let frisk = s.private(0).frisk.clone().expect("his things, shown to her");
    assert_eq!(frisk.target, bob);
    let beans = s.host.world.item_index("tinned_beans").unwrap();
    let at = frisk.inventory.carried.iter().position(|st| st.item == beans).expect("the tins are there") as u32;
    assert!(s.private(2).frisk.is_none() && s.private(1).frisk.is_none(), "nobody else is shown them");
    s.clients[0].send(Intent::TakeFrom { player: 0, from: Place::Carried, index: at, item: beans, count: 0 });
    s.run(1.0);
    assert!(s.private(0).inventory.carried.iter().any(|st| st.item == beans && st.count == 2), "{:?}", s.private(0).inventory);
    assert!(!s.private(1).inventory.carried.iter().any(|st| st.item == beans));
    assert!(s.private(0).frisk.as_ref().is_some_and(|f| f.inventory.carried.is_empty()), "her view of him follows");
    assert!(s.noted(1, "took your 2 x"));

    // He struggles; she cuts him loose before he is out.
    s.clients[1].send(Intent::Struggle { player: 0, on: true });
    s.run(3.0);
    let his = seen(&s, 1, bob).unwrap();
    assert!(his.struggling && his.struggle > 0.02 && his.struggle < 0.1, "{}", his.struggle);
    assert!(seen(&s, 0, bob).unwrap().struggling, "she sees him at it");
    s.clients[0].send(Intent::Free { player: 0, target: None });
    s.run(2.7);
    assert!(!seen(&s, 0, bob).unwrap().restrained && seen(&s, 1, bob).unwrap().mods.can_run);
    assert!(s.private(0).frisk.is_none(), "free, he can't be searched");
}

#[test]
fn a_group_shares_names_and_markers_and_nobody_else_is_told() {
    let mut s = Scene::new();
    let (ada, bob) = (s.id(0), s.id(1));
    assert!(s.snap(0).groups.is_empty());
    s.clients[0].send(Intent::Group { player: 0, op: GroupOp::Invite { target: None } });
    s.run(0.2);
    let g = s.snap(1).group_of(bob).expect("the invitation").clone();
    assert!(g.members.is_empty() && g.invites.len() == 1 && g.invites[0].name == "ada" && g.invites[0].id == ada);
    assert!(s.snap(0).groups.is_empty() && s.snap(2).groups.is_empty());
    s.clients[1].send(Intent::Group { player: 0, op: GroupOp::Accept { from: None } });
    s.run(0.2);
    for k in [0, 1] {
        let g = s.snap(k).group_of(s.id(k)).expect("in the group");
        assert_eq!(g.members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["ada", "bob"]);
    }
    assert!(s.snap(2).groups.is_empty(), "Cy isn't told");
    // A marker one puts down, the other sees and takes away.
    let at = ADA + Vec2::new(12.0, 5.0);
    s.clients[0].send(Intent::Group { player: 0, op: GroupOp::Mark { pos: at.into() } });
    s.run(0.2);
    let g = s.snap(1).group_of(bob).unwrap();
    assert!(g.markers.len() == 1 && g.markers[0].pos == at && g.markers[0].by == ada);
    s.clients[1].send(Intent::Group { player: 0, op: GroupOp::Unmark { marker: None, near: (at + Vec2::new(1.0, 0.0)).into() } });
    s.run(0.2);
    assert!(s.snap(0).group_of(ada).unwrap().markers.is_empty());
    // Groups never switch damage off.
    let hp = s.snap(1).players.iter().find(|p| p.id == bob).unwrap().hp;
    s.aim = true;
    s.run(0.2);
    for _ in 0..6 {
        s.clients[0].send(Intent::Attack { player: 0 });
        s.run(1.0);
    }
    assert!(s.snap(1).players.iter().find(|p| p.id == bob).unwrap().hp < hp, "a group member is hit like anyone");
    // Leaving ends it for both.
    s.clients[1].send(Intent::Group { player: 0, op: GroupOp::Leave });
    s.run(0.2);
    assert!(s.snap(0).groups.is_empty() && s.snap(1).groups.is_empty());
}
