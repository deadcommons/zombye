//! Lag-compensated melee over a real socket (#23): a client on a line with 150 ms of ping (made up
//! by [`Remote::set_lag`]) punches a zombie where its own snapshots show it, and the server, which
//! has the zombie somewhere else by then, agrees. The rules themselves are tested in
//! `zombye_sim::combat::lag_tests`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use glam::Vec2;
use zombye_net::host::{Host, HostOptions, Spawn};
use zombye_net::remote::Remote;
use zombye_net::{Intent, Snapshot, content};
use zombye_sim::{TICK_DT, World, WorldConfig};

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

/// What a client says it sees reaches the world as its own (whoever it names), never ahead of the
/// world, and a client that says nothing is not compensated.
#[test]
fn what_a_client_saw_reaches_the_world() {
    let mut host = Host::bind(World::new(config()), "127.0.0.1:0", HostOptions { spawn: Some(Spawn::Together), ..Default::default() }).unwrap();
    let mut ada = join(&mut host, "ada");
    let bob = join(&mut host, "bob");
    for _ in 0..10 {
        host.tick();
    }
    // Ada names Bob; the server knows better.
    ada.send(Intent::Seen { player: bob.player(), tick: 7 });
    ada.send(Intent::Seen { player: bob.player(), tick: 1_000_000 });
    ada.flush();
    let start = Instant::now();
    while host.received(ada.player()) < ada.intents_sent {
        host.pump();
        assert!(start.elapsed() < Duration::from_secs(10));
    }
    host.tick();
    let (a, b) = (host.world.player(ada.player()).unwrap(), host.world.player(bob.player()).unwrap());
    assert!(a.seen_tick >= 10 && a.seen_tick < host.world.tick, "clamped to the world's tick: {}", a.seen_tick);
    assert_eq!(b.seen_tick, 0);
    assert_eq!(host.world.view_lag_secs(bob.player()), 0.0);
    assert!(!host.world.lag_history().is_empty());
}

/// A zombie walks away from a client's survivor at 3 m/s. The client, at 150 ms of ping, punches
/// when its own picture says the blow will land with the zombie still in reach. Returns the
/// zombies hit and the view lag the server compensated for (seconds).
fn punch_a_walker_at_150_ms(compensate: bool) -> (usize, f32) {
    const PER_TICK: f32 = 0.15;
    let mut config = config();
    if !compensate {
        config.combat.lag_comp_max_secs = 0.0;
    }
    let mut host = Host::bind(World::new(config), "127.0.0.1:0", HostOptions { spawn: Some(Spawn::Together), ..Default::default() }).unwrap();
    let mut client = join(&mut host, "ada");
    client.set_lag(Duration::from_millis(150));
    let me = client.player();
    // Unseen and unheard, so the zombie only goes where the test puts it.
    host.world.set_dev(me, Some(true), Some(true));
    let origin = host.world.player(me).unwrap().state.pos;
    let dir = (0..16)
        .map(|k| Vec2::from_angle(k as f32 * std::f32::consts::TAU / 16.0))
        .find(|d| zombye_sim::vision::clear_path(&host.world.map, origin, origin + *d * 6.0))
        .expect("open ground one way or another");
    let facing = dir.to_angle();
    let z = host.world.add_zombie(origin + dir * 0.3, facing, 0.0);
    host.world.zombies.wander_timer[z] = 1e9;
    let zid = host.world.zombies.id[z];
    let fists = host.world.wielded(None).clone();
    let reach = fists.range + zombye_sim::ai::ZOMBIE_RADIUS;
    // Ticks from the server taking the punch to its landing.
    let windup_ticks = (fists.windup_seconds / TICK_DT).round() - 1.0;
    // Punch when the picture has it here: the blow then lands on screen with it 0.3 m inside reach
    // (give or take the tick it took to notice).
    let punch_at = reach - 0.3 - windup_ticks * PER_TICK - PER_TICK * 0.5;

    let mut waiting: VecDeque<Snapshot> = VecDeque::new();
    let mut curr: Option<Snapshot> = None;
    let (mut seq, mut punched, mut hits, mut lag) = (0u32, false, 0usize, 0.0f32);
    let started = Instant::now();
    for k in 0..200u32 {
        // The server: the zombie stands a second and a half, then walks off.
        let walked = k.saturating_sub(30) as f32 * PER_TICK;
        if let Some(j) = host.world.zombies.index_of(zid) {
            host.world.zombies.pos[j] = origin + dir * (0.3 + walked);
        }
        host.pump();
        host.tick();
        hits += host.world.hit_events.iter().filter(|h| h.player == me).count();
        if host.world.action_events.iter().any(|a| a.player == me) {
            lag = host.world.view_lag_secs(me);
            break;
        }
        // The client, as the game's does it: take up a snapshot a tick, say which, send the input.
        waiting.extend(client.poll());
        let take = if waiting.len() >= 3 { 2 } else { waiting.len().min(1) };
        for s in waiting.drain(..take) {
            curr = Some(s);
        }
        if let Some(s) = &curr {
            let seen = s.zombies.iter().find(|v| v.id == zid).map(|v| Vec2::from(v.pos).distance(origin));
            if !punched && k > 30 && seen.is_some_and(|d| d >= punch_at) {
                punched = true;
                client.send(Intent::Attack { player: me });
            }
            seq += 1;
            client.send(Intent::Seen { player: me, tick: s.tick });
            client.send(Intent::PlayerInput { player: me, seq, move_dir: [0.0, 0.0], facing, run: false, sneak: false, aim: true });
        }
        client.flush();
        // Twenty ticks a second, by the clock.
        std::thread::sleep((started + Duration::from_millis(50) * (k + 1)).saturating_duration_since(Instant::now()));
    }
    assert!(punched, "the client never saw the zombie far enough off to punch");
    (hits, lag)
}

/// The issue's case: at 150 ms a punch thrown at a zombie the client sees in reach lands, though
/// the server has it 0.6 m or more further off by then. With compensation off the same punch
/// misses (how it was before).
#[test]
fn a_punch_at_150_ms_lands_where_the_client_saw_the_zombie() {
    let (hits, lag) = punch_a_walker_at_150_ms(true);
    assert_eq!(hits, 1, "the punch landed where the client saw the zombie (view lag {lag} s)");
    // 150 ms there and back, and both ends only look once a tick: 0.2 to 0.35 s.
    assert!((0.15..=0.35).contains(&lag), "view lag {lag} s");
    let (hits, _) = punch_a_walker_at_150_ms(false);
    assert_eq!(hits, 0, "without compensation the zombie has walked out of reach");
}
