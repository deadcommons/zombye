//! Guns over a real socket (#72): a client on a line with 150 ms of ping shoots a rifle at a
//! zombie walking across its line of fire, aiming (facing and pitch) where its own snapshots show
//! it, and the server, which has the zombie most of a metre further on by then, agrees. The shot
//! comes back to the client as an event with the line it flew, and the round is gone from the gun
//! in its own view. The rules themselves are tested in `zombye_sim::ranged::tests`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use glam::Vec2;
use zombye_net::host::{Host, HostOptions, Spawn};
use zombye_net::remote::Remote;
use zombye_net::{Intent, Snapshot, content};
use zombye_sim::ranged::MUZZLE_HEIGHT;
use zombye_sim::{CombatEventKind, Slot, World, WorldConfig};

fn config() -> WorldConfig {
    let mut config = content::load_config(&content::default_data_dir(), 1).expect("the shipped content");
    config.zombies = Some(0);
    config
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

/// What came of the shot: zombies hit on the server, whether the client was sent the shot's
/// line, the rounds its own view says are left in the gun, and the view lag compensated for.
struct Shot {
    hits: usize,
    seen_line: bool,
    rounds: Option<u8>,
    lag: f32,
}

/// A zombie walks across a client's line of fire 12 m off at 3 m/s. The client, at 150 ms of
/// ping, shoots a rifle at where its own picture has it.
fn shoot_a_walker_at_150_ms(compensate: bool) -> Shot {
    const PER_TICK: f32 = 0.15;
    const AWAY: f32 = 12.0;
    let mut config = config();
    if !compensate {
        config.combat.lag_comp_max_secs = 0.0;
    }
    let mut host = Host::bind(World::new(config), "127.0.0.1:0", HostOptions { spawn: Some(Spawn::Together), ..Default::default() }).unwrap();
    let mut client = join(&mut host, "ada");
    client.set_lag(Duration::from_millis(150));
    let me = client.player();
    // Unseen and unheard, so the zombie only goes where the test puts it; a loaded rifle in hand.
    host.world.set_dev(me, Some(true), Some(true));
    assert!(host.world.hold_item(me, Slot::Hand, "hunting_rifle"));
    assert_eq!(host.world.loaded(me), Some((5, 5)));
    let origin = host.world.player(me).unwrap().state.pos;
    // Open ground to shoot over, and for the zombie to cross it on.
    let clear = |from: Vec2, to: Vec2| zombye_sim::vision::clear_path(&host.world.map, from, to);
    let dir = (0..32)
        .map(|k| Vec2::from_angle(k as f32 * std::f32::consts::TAU / 32.0))
        .find(|d| [-5.0f32, 0.0, 5.0].iter().all(|s| clear(origin, origin + *d * AWAY + d.perp() * *s)))
        .expect("open ground one way or another");
    let across = dir.perp();
    let at = |k: u32| origin + dir * AWAY + across * (k.saturating_sub(30) as f32 * PER_TICK - 4.0);
    let z = host.world.add_zombie(at(0), 0.0, 0.0);
    host.world.zombies.wander_timer[z] = 1e9;
    let zid = host.world.zombies.id[z];

    let mut waiting: VecDeque<Snapshot> = VecDeque::new();
    let mut curr: Option<Snapshot> = None;
    let mut out = Shot { hits: 0, seen_line: false, rounds: None, lag: 0.0 };
    let (mut seq, mut shot_at, mut landed) = (0u32, None, false);
    let started = Instant::now();
    for k in 0..200u32 {
        if let Some(j) = host.world.zombies.index_of(zid) {
            host.world.zombies.pos[j] = at(k);
        }
        host.pump();
        host.tick();
        out.hits += host.world.hit_events.iter().filter(|h| h.player == me).count();
        if !landed && host.world.action_events.iter().any(|a| a.player == me) {
            landed = true;
            out.lag = host.world.view_lag_secs(me);
        }
        // The client, as the game's does it: take up a snapshot a tick, say which, aim, send the input.
        waiting.extend(client.poll());
        let take = if waiting.len() >= 3 { 2 } else { waiting.len().min(1) };
        for s in waiting.drain(..take) {
            out.seen_line |= s.hits.iter().any(|h| h.kind == CombatEventKind::Shot && h.player == me && (Vec2::from(h.pos) - Vec2::from(h.dir) * h.damage).distance(origin) < 0.05);
            if let Some(v) = s.private.iter().find(|v| v.player == me) {
                out.rounds = v.inventory.hand.map(|g| g.used);
            }
            curr = Some(s);
        }
        if let Some(s) = &curr {
            let seen = s.zombies.iter().find(|v| v.id == zid).map(|v| Vec2::from(v.pos) - origin);
            let to = seen.unwrap_or(dir * AWAY);
            seq += 1;
            client.send(Intent::Seen { player: me, tick: s.tick });
            client.send(Intent::Aim { player: me, pitch: ((1.1 - MUZZLE_HEIGHT) / to.length()).atan() });
            // Once it is well on its way across: the trigger, then the input that faces it.
            if shot_at.is_none() && k > 50 && seen.is_some() {
                shot_at = Some(k);
                client.send(Intent::Attack { player: me });
            }
            client.send(Intent::PlayerInput { player: me, seq, move_dir: [0.0, 0.0], facing: to.to_angle(), run: false, sneak: false, aim: true });
        }
        client.flush();
        // A while longer after the shot, for its news to come back down the line.
        if shot_at.is_some_and(|t| k > t + 25) {
            break;
        }
        std::thread::sleep((started + Duration::from_millis(50) * (k + 1)).saturating_duration_since(Instant::now()));
    }
    assert!(shot_at.is_some() && landed, "the client never shot");
    out
}

/// At 150 ms a rifle shot at a zombie crossing the line of fire lands where the client saw it,
/// though the server has it most of a metre on by then; the client is sent the line the bullet
/// flew and sees the round gone. With compensation off the same shot passes behind it.
#[test]
fn a_shot_at_150_ms_lands_where_the_client_saw_the_zombie() {
    let shot = shoot_a_walker_at_150_ms(true);
    assert_eq!(shot.hits, 1, "the shot landed where the client saw the zombie (view lag {} s)", shot.lag);
    assert!((0.15..=0.35).contains(&shot.lag), "view lag {} s", shot.lag);
    assert!(shot.seen_line, "the client got the shot's line, from its own muzzle");
    assert_eq!(shot.rounds, Some(4), "and its view of the gun: one round gone");
    let shot = shoot_a_walker_at_150_ms(false);
    assert_eq!(shot.hits, 0, "without compensation the zombie has walked out of the line");
    assert_eq!(shot.rounds, Some(4), "the round is spent all the same");
}
