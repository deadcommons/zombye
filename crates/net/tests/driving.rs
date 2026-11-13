//! Driving over a real socket (DCM-88): a client at 150 ms of ping (made up by
//! [`Remote::set_lag`]) predicts its own car as the game's client does (the snapshot's car with
//! the driving it hasn't had replayed on top) and the prediction is exactly where the server puts
//! the car; and a car far from a client is sent to it only now and then.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use glam::Vec2;
use zombye_net::host::{Host, HostOptions, Spawn};
use zombye_net::remote::Remote;
use zombye_net::wire::FAR_CAR_EVERY;
use zombye_net::{Intent, Snapshot, content};
use zombye_sim::{DriveInput, TICK_DT, VehicleOp, VehicleState, World, WorldConfig, step_vehicle};

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

/// Put player `me` at the wheel of a car with fuel in it: its id.
fn seat(world: &mut World, me: u32) -> u32 {
    let cars: Vec<(u32, VehicleState, f32, f32)> = world
        .vehicles()
        .iter()
        .filter(|v| v.cond.fuel > 5.0)
        .map(|v| {
            let k = &world.config.vehicles.kinds[v.kind as usize];
            (v.id, v.state, k.width, k.length)
        })
        .collect();
    for (vid, s, width, _) in cars {
        let side = Vec2::from_angle(s.heading - std::f32::consts::FRAC_PI_2);
        for at in [s.pos + side * (width / 2.0 + 0.5), s.pos - side * (width / 2.0 + 0.5)] {
            let p = world.players.iter_mut().find(|p| p.id == me).unwrap();
            p.state.pos = at;
            p.state.facing = (s.pos - at).to_angle();
            if world.player_vehicle(me, VehicleOp::Enter { vehicle: Some(vid) }).is_ok() {
                return vid;
            }
        }
    }
    panic!("no car with fuel to get into");
}

/// Throttle, steering and brake at tick `k`: away, a weave, a stop, reverse.
fn driving(k: u32) -> DriveInput {
    match k {
        0..60 => DriveInput { throttle: 1.0, steer: if (k / 15) % 2 == 0 { 0.6 } else { -0.6 }, brake: false },
        60..80 => DriveInput { throttle: 0.0, steer: 0.0, brake: true },
        _ => DriveInput { throttle: -1.0, steer: 0.3, brake: false },
    }
}

#[test]
fn a_driver_at_150_ms_predicts_its_car_exactly() {
    let mut host = Host::bind(World::new(config()), "127.0.0.1:0", HostOptions { spawn: Some(Spawn::Together), ..Default::default() }).unwrap();
    let mut client = join(&mut host, "ada");
    client.set_lag(Duration::from_millis(150));
    let me = client.player();
    host.world.set_dev(me, Some(true), Some(true));
    let vid = seat(&mut host.world, me);
    let map = host.world.map.clone();
    let kind = host.world.config.vehicles.kinds[host.world.vehicle_of(vid).unwrap().kind as usize].clone();
    let bounce = host.world.config.vehicles.tuning.bounce;

    let mut waiting: VecDeque<Snapshot> = VecDeque::new();
    let mut pending: VecDeque<(u32, DriveInput)> = VecDeque::new();
    // What the client predicted the car to be after each input (by its number).
    let mut predicted_at: HashMap<u32, VehicleState> = HashMap::new();
    let mut predicted: Option<VehicleState> = None;
    let mut perf = zombye_sim::Perf::NEW;
    let (mut seq, mut sent, mut checked, mut moved) = (0u32, 0u32, 0u32, 0.0f32);
    let started = Instant::now();
    for k in 0..220u32 {
        host.pump();
        host.tick();
        // The client, as the game's does: a snapshot a tick taken up, the car reconciled to it.
        waiting.extend(client.poll());
        let take = if waiting.len() >= 3 { 2 } else { waiting.len().min(1) };
        for s in waiting.drain(..take) {
            let Some(view) = s.players.iter().find(|p| p.id == me) else { continue };
            let Some(car) = s.vehicles.iter().find(|v| v.id == vid) else { continue };
            assert_eq!(car.driver, me, "still at the wheel");
            while pending.front().is_some_and(|(n, _)| *n <= view.last_input_seq) {
                pending.pop_front();
            }
            if let Some(want) = predicted_at.get(&view.last_input_seq) {
                assert_eq!(car.state, *want, "the server's car after input {} is where the client predicted it", view.last_input_seq);
                checked += 1;
            }
            perf = car.perf();
            let mut cs = car.state;
            for (_, d) in &pending {
                step_vehicle(&map, &kind, vid, &mut cs, *d, perf, TICK_DT, bounce);
            }
            if let Some(p) = predicted {
                assert_eq!(cs, p, "replayed on the snapshot, the prediction doesn't move");
            }
            predicted = Some(cs);
        }
        if let Some(mut cs) = predicted
            && sent < 200
        {
            let d = driving(sent);
            sent += 1;
            seq += 1;
            client.send(Intent::Drive { player: me, throttle: d.throttle, steer: d.steer, brake: d.brake });
            client.send(Intent::PlayerInput { player: me, seq, move_dir: [0.0, 0.0], facing: cs.heading, run: false, sneak: false, aim: false });
            let before = cs.pos;
            step_vehicle(&map, &kind, vid, &mut cs, d, perf, TICK_DT, bounce);
            moved += cs.pos.distance(before);
            pending.push_back((seq, d));
            predicted_at.insert(seq, cs);
            predicted = Some(cs);
        }
        client.flush();
        std::thread::sleep((started + Duration::from_millis(50) * (k + 1)).saturating_duration_since(Instant::now()));
    }
    assert!(moved > 20.0, "it drove ({moved:.1} m)");
    assert!(checked > 150, "nearly every snapshot checked ({checked})");
}

/// A car moving far from a client goes to it every `FAR_CAR_EVERY` ticks, a near one every tick.
#[test]
fn a_far_car_is_sent_now_and_then() {
    let mut host = Host::bind(World::new(config()), "127.0.0.1:0", HostOptions { spawn: Some(Spawn::Together), ..Default::default() }).unwrap();
    let mut driver = join(&mut host, "ada");
    let mut watcher = join(&mut host, "bob");
    let (me, them) = (driver.player(), watcher.player());
    host.world.set_dev(me, Some(true), Some(true));
    host.world.set_dev(them, Some(true), Some(true));
    let vid = seat(&mut host.world, me);
    let car_at = host.world.vehicle_of(vid).unwrap().state.pos;
    // Bob is put far off; then near.
    let far = host.world.config.tiers.interest_radius * 2.0;
    for (spot, every) in [(far, FAR_CAR_EVERY as usize), (3.0, 1)] {
        let p = host.world.players.iter_mut().find(|p| p.id == them).unwrap();
        p.state.pos = car_at + Vec2::new(0.0, spot);
        let _ = driver.poll();
        let _ = watcher.poll();
        let mut states = Vec::new();
        for k in 0..40u32 {
            host.world.player_drive(me, DriveInput { throttle: if k < 20 { 0.6 } else { -0.6 }, steer: 0.0, brake: false });
            host.pump();
            host.tick();
            // Keep Bob where he is.
            let car = host.world.vehicle_of(vid).unwrap().state.pos;
            let p = host.world.players.iter_mut().find(|p| p.id == them).unwrap();
            p.state.pos = car + Vec2::new(0.0, spot);
            let s = watcher.wait_snapshot(Duration::from_secs(5)).unwrap();
            if k >= 10 {
                states.push(s.vehicles.iter().find(|v| v.id == vid).unwrap().state);
            }
        }
        let changes = states.windows(2).filter(|w| w[0] != w[1]).count();
        let want = (states.len() - 1) / every;
        assert!(changes.abs_diff(want) <= 2, "{changes} updates in {} ticks at {spot} m, want about {want}", states.len());
    }
}
