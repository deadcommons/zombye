//! Vehicles: the shipped file, driving, walls, getting in and out, bodies pushed aside, the town's
//! cars, saves and determinism.

use glam::{IVec2, Vec2};

use super::*;
use crate::map::{Edge, Side};
use crate::street_props::parse_street_props;
use crate::template::tests::shipped_templates;
use crate::world::WorldConfig;

fn shipped() -> VehicleData {
    parse_vehicles(include_str!("../../../../assets/data/vehicles.ron")).unwrap()
}

fn run(w: &mut World, secs: f32) {
    for _ in 0..(secs / crate::TICK_DT).ceil() as u32 {
        w.step();
    }
}

fn pi(w: &World, id: u32) -> usize {
    w.players.iter().position(|p| p.id == id).unwrap()
}

/// An empty field at noon with the shipped vehicle kinds, a car parked facing east at (100, 100)
/// and a player standing beside its driver's door (north of it), facing it.
fn field() -> (World, u32, u32) {
    let mut config = WorldConfig::new(5, 0, Vec::new());
    config.start_hour = 12.0;
    config.vehicles = shipped();
    config.vehicles.drivable_share = 0.0;
    let mut w = World::new(config);
    w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    let vid = park(&mut w, "car", Vec2::new(100.0, 100.0), 0.0);
    let id = w.add_player(Vec2::new(100.0, 98.4));
    let i = pi(&w, id);
    w.players[i].state.facing = std::f32::consts::FRAC_PI_2;
    (w, id, vid)
}

/// Put a parked vehicle of kind `kind` on the map.
fn park(w: &mut World, kind: &str, pos: Vec2, heading: f32) -> u32 {
    let k = w.config.vehicles.by_id(kind).unwrap();
    w.vehicles.next_id = w.vehicles.next_id.max(1);
    let id = w.vehicles.next_id;
    w.vehicles.next_id += 1;
    let state = VehicleState { pos, heading, speed: 0.0, steer: 0.0 };
    let cond = Condition { fuel: w.config.vehicles.kinds[k].tank_l, ..Condition::default() };
    w.vehicles.list.push(Vehicle { id, kind: k as u16, state, input: DriveInput::PARKED, driver: None, from: IVec2::new(-1, -1), cond });
    let boxes = w.vehicle_boxes();
    w.map.set_vehicles(boxes);
    id
}

fn drive(w: &mut World, id: u32, throttle: f32, steer: f32, secs: f32) {
    for _ in 0..(secs / crate::TICK_DT).ceil() as u32 {
        w.player_drive(id, DriveInput { throttle, steer, brake: false });
        w.step();
    }
}

fn car(w: &World, vid: u32) -> VehicleState {
    w.vehicles.list.iter().find(|v| v.id == vid).unwrap().state
}

#[test]
fn the_shipped_file_parses_and_mistakes_are_named() {
    let data = shipped();
    assert!(data.kinds.len() >= 6);
    assert!(data.of_prop("car_wreck").is_none(), "wrecks never drive");
    let text = include_str!("../../../../assets/data/vehicles.ron");
    assert!(parse_vehicles(&text.replace("drivable_share: 1.0", "drivable_share: 1.5")).unwrap_err().contains("drivable_share"));
    assert!(parse_vehicles(&text.replace("wheelbase: 2.7", "wheelbase: 9.0")).unwrap_err().contains("\"car\""));
    assert!(parse_vehicles(&text.replace("bounce: 0.2", "bounce: 0.2, extra: 1")).is_err());
    assert!(parse_vehicles(&text.replace("prop: \"van\"", "prop: \"car\"")).unwrap_err().contains("already parked"));
    assert_eq!(parse_vehicles("()").unwrap(), VehicleData::default());
    // Every kind is parked as a prop the street file has.
    let props = parse_street_props(include_str!("../../../../assets/data/street_props.ron")).unwrap();
    for k in &data.kinds {
        assert!(props.kinds.iter().any(|p| p.id == k.prop), "{} parked as an unknown prop {}", k.id, k.prop);
    }
}

#[test]
fn getting_in_driving_and_getting_out() {
    let (mut w, id, vid) = field();
    // The car is a box nobody walks through.
    assert!(w.map.resolve_circle(Vec2::new(100.0, 100.0), 0.3).distance(Vec2::new(100.0, 100.0)) > 0.5);
    let msg = w.player_vehicle(id, VehicleOp::Enter { vehicle: None }).unwrap();
    assert!(msg.contains("car"), "{msg}");
    let i = pi(&w, id);
    assert_eq!(w.players[i].vehicle, Some(vid));
    assert_eq!(w.players[i].state.mods.speed, 0.0);
    // Walking input does nothing: the body is in the seat.
    w.set_player_input(id, 1, crate::players::PlayerInput { move_dir: Vec2::Y, facing: 0.0, ..Default::default() });
    run(&mut w, 0.5);
    let seat = seat_of(&w.config.vehicles.kinds[0], &car(&w, vid));
    assert!(w.players[i].state.pos.distance(seat) < 1e-4);
    // Throttle: it goes east, faster and faster.
    drive(&mut w, id, 1.0, 0.0, 3.0);
    let s = car(&w, vid);
    assert!(s.pos.x > 110.0 && (s.pos.y - 100.0).abs() < 0.01, "{s:?}");
    assert!(s.speed > 8.0, "{s:?}");
    // Can't get out at speed.
    assert!(w.player_vehicle(id, VehicleOp::Exit).is_err());
    // Steering right (y down: south) turns it clockwise.
    drive(&mut w, id, 0.3, 1.0, 1.0);
    let t = car(&w, vid);
    assert!(t.heading > 0.2 && t.heading < 3.0, "{t:?}");
    assert!(t.pos.y > s.pos.y, "{t:?}");
    // The brake stops it.
    for _ in 0..80 {
        w.player_drive(id, DriveInput { throttle: 0.0, steer: 0.0, brake: true });
        w.step();
    }
    assert_eq!(car(&w, vid).speed, 0.0);
    // Out on the driver's side, standing clear of the box.
    w.player_vehicle(id, VehicleOp::Exit).unwrap();
    let i = pi(&w, id);
    assert_eq!(w.players[i].vehicle, None);
    assert!(w.vehicles.list[0].driver.is_none());
    let b = w.vehicles.list[0].boxed(&w.config.vehicles.kinds[0]);
    assert!(b.distance(w.players[i].state.pos) > 0.29);
    assert!(w.players[i].state.mods.speed > 0.0);
}

#[test]
fn a_wall_stops_a_car() {
    let (mut w, id, vid) = field();
    for y in 90..111 {
        w.map.set_edge(IVec2::new(112, y), Side::W, Edge::Wall);
    }
    w.map_changed();
    w.player_vehicle(id, VehicleOp::Enter { vehicle: Some(vid) }).unwrap();
    drive(&mut w, id, 1.0, 0.0, 4.0);
    let s = car(&w, vid);
    let half = w.config.vehicles.kinds[0].length * 0.5;
    assert!(s.pos.x + half <= 112.0 + 0.05, "nose through the wall: {s:?}");
    assert!(s.speed.abs() < 3.0, "the speed into the wall is gone: {s:?}");
    // It backs off it again.
    drive(&mut w, id, -1.0, 0.0, 2.0);
    assert!(car(&w, vid).pos.x < s.pos.x - 1.0);
}

#[test]
fn a_car_pushes_zombies_and_survivors_aside() {
    let (mut w, id, vid) = field();
    let other = w.add_player(Vec2::new(108.0, 100.0));
    w.spawn_zombies(1, Vec2::new(112.0, 100.2), 0.0);
    w.player_vehicle(id, VehicleOp::Enter { vehicle: Some(vid) }).unwrap();
    drive(&mut w, id, 1.0, 0.0, 3.0);
    let b = w.vehicles.list[0].boxed(&w.config.vehicles.kinds[0]);
    let o = pi(&w, other);
    assert!(b.distance(w.players[o].state.pos) > 0.25, "standing in the car: {}", w.players[o].state.pos);
    for (p, st) in w.zombies.pos.iter().zip(&w.zombies.state) {
        assert!(*st == crate::zombies::ZombieState::Dead || b.distance(*p) > 0.25, "a zombie in the car at {p}");
    }
}

#[test]
fn nobody_gets_in_from_too_far_or_into_a_taken_seat() {
    let (mut w, id, vid) = field();
    let far = w.add_player(Vec2::new(100.0, 94.0));
    assert!(w.player_vehicle(far, VehicleOp::Enter { vehicle: Some(vid) }).is_err());
    w.player_vehicle(id, VehicleOp::Enter { vehicle: None }).unwrap();
    let near = w.add_player(Vec2::new(100.0, 101.6));
    assert!(w.player_vehicle(near, VehicleOp::Enter { vehicle: Some(vid) }).unwrap_err().contains("wheel"));
}

#[test]
fn a_driver_who_dies_lets_go() {
    let (mut w, id, vid) = field();
    w.player_vehicle(id, VehicleOp::Enter { vehicle: None }).unwrap();
    let i = pi(&w, id);
    w.hurt_player(i, 1000.0);
    run(&mut w, 0.2);
    assert!(w.vehicles.list.iter().find(|v| v.id == vid).unwrap().driver.is_none());
    assert_eq!(w.players[pi(&w, id)].vehicle, None);
}

/// A town with the shipped street props and every parked car drivable.
fn town() -> World {
    let mut config = WorldConfig::new(3, 0, shipped_templates());
    config.street_props = parse_street_props(include_str!("../../../../assets/data/street_props.ron")).unwrap();
    config.vehicles = shipped();
    config.vehicles.drivable_share = 1.0;
    World::new(config)
}

#[test]
fn the_town_parks_its_cars_as_vehicles_and_the_same_seed_parks_the_same() {
    let a = town();
    assert!(a.vehicles.list.len() >= 5, "{} vehicles", a.vehicles.list.len());
    for v in &a.vehicles.list {
        // The prop it was is gone from the map, and the box stands where it stood.
        assert!(a.map.street_prop(v.from).is_none());
        assert!(a.map.vehicles.iter().any(|b| b.id == v.id && b.pos == v.state.pos));
    }
    let b = town();
    assert_eq!(a.vehicles, b.vehicles);
    assert_eq!(a.state_hash(), b.state_hash());
    // None at a share of 0: the props stay.
    let mut config = WorldConfig::new(3, 0, shipped_templates());
    config.street_props = a.config.street_props.clone();
    config.vehicles = shipped();
    config.vehicles.drivable_share = 0.0;
    let none = World::new(config);
    assert!(none.vehicles.list.is_empty());
    assert_eq!(none.map_base_hash(), a.map_base_hash(), "the base map is the same, with every prop");
}

#[test]
fn a_saved_world_keeps_its_cars_where_they_were_driven() {
    let mut w = town();
    let v = w.vehicles.list[0].clone();
    let kind = w.config.vehicles.kinds[v.kind as usize].clone();
    // Stand a player beside its driver's door and drive it a little.
    let left = Vec2::new(v.state.forward().y, -v.state.forward().x);
    let id = w.add_player(v.state.pos + left * (kind.width * 0.5 + 0.5));
    let i = pi(&w, id);
    w.players[i].state.facing = (-left).to_angle();
    w.player_vehicle(id, VehicleOp::Enter { vehicle: Some(v.id) }).unwrap();
    drive(&mut w, id, -0.5, 0.0, 1.0);
    let bytes = w.save();
    let mut content = WorldConfig::new(3, 0, shipped_templates());
    content.street_props = w.config.street_props.clone();
    content.vehicles = w.config.vehicles.clone();
    let mut l = World::load(&bytes, &content).unwrap();
    assert_eq!(l.vehicles, w.vehicles);
    assert_eq!(l.map.vehicles, w.map.vehicles);
    assert_eq!(l.players[pi(&l, id)].vehicle, Some(v.id));
    assert_eq!(l.state_hash(), w.state_hash());
    // And carries on the same.
    drive(&mut w, id, 1.0, 0.5, 1.0);
    drive(&mut l, id, 1.0, 0.5, 1.0);
    assert_eq!(l.state_hash(), w.state_hash());
    // A world with none saves no section, and loads with none.
    let mut bare = WorldConfig::new(3, 0, shipped_templates());
    bare.street_props = w.config.street_props.clone();
    let b = World::new(bare.clone());
    let lb = World::load(&b.save(), &bare).unwrap();
    assert!(lb.vehicles.list.is_empty());
    assert_eq!(lb.state_hash(), b.state_hash());
}

#[test]
fn driving_is_deterministic() {
    let run_once = || {
        let (mut w, id, _) = field();
        w.player_vehicle(id, VehicleOp::Enter { vehicle: None }).unwrap();
        for k in 0..200 {
            let steer = ((k / 20) % 3) as f32 - 1.0;
            w.player_drive(id, DriveInput { throttle: 0.8, steer, brake: k % 50 == 49 });
            w.step();
        }
        (w.state_hash(), car(&w, w.vehicles.list[0].id))
    };
    assert_eq!(run_once(), run_once());
}

fn vehicle(w: &World, vid: u32) -> &Vehicle {
    w.vehicles.list.iter().find(|v| v.id == vid).unwrap()
}

#[test]
fn the_engine_starts_burns_fuel_and_dies_when_the_tank_is_dry() {
    let (mut w, id, vid) = field();
    let vi = w.vehicles.list.iter().position(|v| v.id == vid).unwrap();
    w.vehicles.list[vi].cond.fuel = 0.04;
    w.player_vehicle(id, VehicleOp::Enter { vehicle: None }).unwrap();
    assert!(!vehicle(&w, vid).cond.engine_on);
    drive(&mut w, id, 1.0, 0.0, 0.2);
    assert!(vehicle(&w, vid).cond.engine_on, "the throttle starts it");
    // A loud start: a sound bigger than the idle's.
    drive(&mut w, id, 1.0, 0.0, 8.0);
    let c = vehicle(&w, vid).cond;
    assert_eq!(c.fuel, 0.0, "{c:?}");
    assert!(!c.engine_on);
    // Dry: the throttle does nothing more; it coasts to a stop.
    let x = car(&w, vid).pos.x;
    drive(&mut w, id, 1.0, 0.0, 20.0);
    let s = car(&w, vid);
    assert_eq!(s.speed, 0.0, "{s:?}");
    drive(&mut w, id, 1.0, 0.0, 2.0);
    assert_eq!(car(&w, vid).pos, s.pos);
    assert!(s.pos.x > x);
    // Getting out stops an engine too.
    let mut w2 = field().0;
    let id2 = w2.players[0].id;
    w2.player_vehicle(id2, VehicleOp::Enter { vehicle: None }).unwrap();
    drive(&mut w2, id2, 0.2, 0.0, 0.3);
    for _ in 0..60 {
        w2.player_drive(id2, DriveInput::PARKED);
        w2.step();
    }
    w2.player_vehicle(id2, VehicleOp::Exit).unwrap();
    assert!(!w2.vehicles.list[0].cond.engine_on);
}

#[test]
fn a_running_engine_and_the_horn_are_heard() {
    let (mut w, id, _) = field();
    w.player_vehicle(id, VehicleOp::Enter { vehicle: None }).unwrap();
    let t = w.config.vehicles.tuning.clone();
    let mut loudest = 0.0f32;
    for _ in 0..60 {
        w.player_drive(id, DriveInput { throttle: 1.0, steer: 0.0, brake: false });
        w.step();
        loudest = w.sounds_heard().iter().fold(loudest, |a, s| a.max(s.radius));
    }
    assert!(loudest >= t.start_noise.min(t.rev_noise) - 0.01, "loudest {loudest}");
    w.player_vehicle(id, VehicleOp::Horn).unwrap();
    w.step();
    assert!(w.sounds_heard().iter().any(|s| (s.radius - t.horn_noise).abs() < 0.01));
    // Out of a car there is no horn.
    let (mut w2, id2, _) = field();
    assert!(w2.player_vehicle(id2, VehicleOp::Horn).is_err());
}

#[test]
fn a_fast_car_runs_a_zombie_over_and_a_slow_one_only_pushes() {
    // Fast: a zombie standing 20 m on is run over, and the kill is the driver's.
    let (mut w, id, vid) = field();
    w.spawn_zombies(1, Vec2::new(125.0, 100.0), 0.0);
    w.player_vehicle(id, VehicleOp::Enter { vehicle: None }).unwrap();
    let hp = w.zombies.hp[0];
    drive(&mut w, id, 1.0, 0.0, 4.0);
    assert!(w.zombies.hp[0] < hp, "not hit: {} of {hp}", w.zombies.hp[0]);
    assert!(matches!(w.zombies.state[0], crate::zombies::ZombieState::Dead | crate::zombies::ZombieState::Down), "{:?}", w.zombies.state[0]);
    let dead = w.zombies.state[0] == crate::zombies::ZombieState::Dead;
    assert_eq!(w.players[pi(&w, id)].run.kills, dead as u32);
    assert!(vehicle(&w, vid).cond.parts[3] < 1.0, "the body takes a knock");
    // Slow: one met at walking pace is only pushed.
    let (mut w, id, _) = field();
    w.spawn_zombies(1, Vec2::new(103.0, 100.0), 0.0);
    w.player_vehicle(id, VehicleOp::Enter { vehicle: None }).unwrap();
    let hp = w.zombies.hp[0];
    drive(&mut w, id, 0.15, 0.0, 2.0);
    assert!(car(&w, vid).speed < w.config.vehicles.tuning.run_over_min_speed);
    assert_eq!(w.zombies.hp[0], hp);
}

#[test]
fn a_crash_wears_the_car_and_hurts_the_driver() {
    let (mut w, id, vid) = field();
    for y in 90..111 {
        w.map.set_edge(IVec2::new(125, y), Side::W, Edge::Wall);
    }
    w.map_changed();
    w.player_vehicle(id, VehicleOp::Enter { vehicle: Some(vid) }).unwrap();
    let i = pi(&w, id);
    let hp = w.players[i].hp;
    drive(&mut w, id, 1.0, 0.0, 5.0);
    let c = vehicle(&w, vid).cond;
    assert!(c.parts.iter().all(|p| *p < 1.0), "{c:?}");
    assert!(w.players[i].hp < hp);
    // A worn engine and tyres go slower.
    let worn = Condition { fuel: 10.0, engine_on: true, parts: [0.4, 0.5, 1.0, 1.0] };
    let p = worn.perf();
    assert!(p.runs && p.top < 0.8 && p.power < 0.8);
    assert!(!Condition { parts: [0.0, 1.0, 1.0, 1.0], ..worn }.perf().runs);
    assert!(!Condition { fuel: 0.0, ..worn }.perf().runs);
    // Killed in it: a crash.
    let (mut w, id, vid) = field();
    for y in 90..111 {
        w.map.set_edge(IVec2::new(125, y), Side::W, Edge::Wall);
    }
    w.map_changed();
    w.player_vehicle(id, VehicleOp::Enter { vehicle: Some(vid) }).unwrap();
    let i = pi(&w, id);
    w.players[i].hp = 1.0;
    drive(&mut w, id, 1.0, 0.0, 5.0);
    let d = w.players[pi(&w, id)].run.death.expect("dead");
    assert_eq!(d.cause, crate::run::DeathCause::Crash);
}

/// Items for fuel: a can and a hose.
fn fuel_items(w: &mut World) {
    let text = r#"[
        (id: "petrol_can", name: "Petrol can", category: Tool, weight_kg: 1.2, volume_l: 12.0, fuel_l: Some(10)),
        (id: "siphon_hose", name: "Siphon hose", category: Tool, weight_kg: 0.4, volume_l: 1.0, tags: ["siphons"]),
    ]"#;
    w.config.items = crate::items::parse_items(text, &[]).unwrap();
}

#[test]
fn a_can_fills_the_tank_and_a_hose_siphons_it_out() {
    let (mut w, id, vid) = field();
    fuel_items(&mut w);
    let vi = w.vehicles.list.iter().position(|v| v.id == vid).unwrap();
    w.vehicles.list[vi].cond.fuel = 3.0;
    let i = pi(&w, id);
    assert!(w.player_vehicle(id, VehicleOp::Refuel { vehicle: None }).unwrap_err().contains("fuel can"));
    w.give_item(id, "petrol_can", 1);
    assert_eq!(w.players[i].inventory.carried[0].used, 10, "a new can is full");
    let heavy = crate::inventory::stack_weight(&w.config.items, &w.players[i].inventory.carried[0]);
    let msg = w.player_vehicle(id, VehicleOp::Refuel { vehicle: None }).unwrap();
    assert!(msg.contains("10 L"), "{msg}");
    assert_eq!(vehicle(&w, vid).cond.fuel, 13.0);
    let can = w.players[i].inventory.carried[0];
    assert_eq!(can.used, 0);
    assert!(crate::inventory::stack_weight(&w.config.items, &can) < heavy, "an empty can is lighter");
    assert!(w.player_vehicle(id, VehicleOp::Refuel { vehicle: None }).is_err(), "the can is empty");
    // Siphoning needs a hose.
    assert!(w.player_vehicle(id, VehicleOp::Siphon { vehicle: None }).unwrap_err().contains("hose"));
    w.give_item(id, "siphon_hose", 1);
    w.player_vehicle(id, VehicleOp::Siphon { vehicle: None }).unwrap();
    assert_eq!(vehicle(&w, vid).cond.fuel, 3.0);
    assert_eq!(w.players[pi(&w, id)].inventory.carried.iter().find(|s| s.item == 0).unwrap().used, 10);
    // From the wheel you can't.
    w.player_vehicle(id, VehicleOp::Enter { vehicle: None }).unwrap();
    assert!(w.player_vehicle(id, VehicleOp::Refuel { vehicle: None }).is_err());
}

#[test]
fn a_saved_car_keeps_its_fuel_and_damage() {
    let mut w = town();
    let vid = w.vehicles.list[0].id;
    w.vehicles.list[0].cond = Condition { fuel: 7.5, engine_on: false, parts: [0.5, 0.75, 0.25, 0.6] };
    let mut content = WorldConfig::new(3, 0, shipped_templates());
    content.street_props = w.config.street_props.clone();
    content.vehicles = w.config.vehicles.clone();
    let l = World::load(&w.save(), &content).unwrap();
    assert_eq!(vehicle(&l, vid).cond, vehicle(&w, vid).cond);
    assert_eq!(l.state_hash(), w.state_hash());
    // Found cars hold a seeded share of their tanks, the same for the same seed.
    let t = &w.config.vehicles.tuning;
    for v in &town().vehicles.list {
        let tank = w.config.vehicles.kinds[v.kind as usize].tank_l;
        assert!(v.cond.fuel >= tank * t.start_fuel.0 - 0.1 && v.cond.fuel <= tank * t.start_fuel.1 + 0.1, "{v:?}");
    }
}
