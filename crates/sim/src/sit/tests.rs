//! Sitting down: on the ground and on a sofa, resting, getting up, saves.

use glam::Vec2;

use super::*;
use crate::map::{Building, PlacedFurniture, Room, Side};
use crate::players::PlayerInput;
use crate::world::WorldConfig;

/// An empty field at noon, needs on, and a player in the middle of it.
fn field() -> (World, u32) {
    let mut config = WorldConfig::new(5, 0, Vec::new());
    config.start_hour = 12.0;
    let mut w = World::new(config);
    w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    let id = w.add_player(Vec2::new(100.5, 100.5));
    (w, id)
}

fn pi(w: &World, id: u32) -> usize {
    w.players.iter().position(|p| p.id == id).unwrap()
}

fn run(w: &mut World, secs: f32) {
    for _ in 0..(secs / crate::TICK_DT).ceil() as u32 {
        w.step();
    }
}

#[test]
fn sitting_on_the_ground_rests_and_holds_you_still() {
    let (mut w, id) = field();
    let i = pi(&w, id);
    w.players[i].needs.tiredness = 0.5;
    w.players[i].state.endurance = 0.2;
    assert!(w.player_sit(id, None).unwrap().contains("ground"));
    assert_eq!(w.players[i].seated, Some(SeatKind::Ground));
    assert_eq!(w.players[i].state.mods.speed, 0.0);
    w.skip_time(1.0);
    assert!(w.players[i].needs.tiredness < 0.5, "rested: {}", w.players[i].needs.tiredness);
    // Breath comes back faster than standing still.
    let mut standing = field();
    let j = pi(&standing.0, standing.1);
    standing.0.players[j].state.endurance = 0.2;
    w.players[i].state.endurance = 0.2;
    run(&mut w, 2.0);
    run(&mut standing.0, 2.0);
    assert!(w.players[i].state.endurance > standing.0.players[j].state.endurance + 0.01);
    // Moving gets you up.
    let pos = w.players[i].state.pos;
    w.set_player_input(id, 1, PlayerInput { move_dir: Vec2::X, facing: 0.0, ..Default::default() });
    run(&mut w, 1.0);
    assert_eq!(w.players[i].seated, None);
    assert!(w.players[i].state.pos.distance(pos) > 0.5, "walked off");
}

#[test]
fn a_sofa_beside_you_is_sat_on() {
    let (mut w, id) = field();
    w.map.rooms.push(Room { building: 0, name: "Living".into(), room_type: "living".into(), floor: crate::map::Floor::Wood });
    for y in 98..104 {
        for x in 98..104 {
            w.map.tile_mut(glam::IVec2::new(x, y)).unwrap().room = 1;
        }
    }
    let sofa = PlacedFurniture { kind: "sofa".into(), min: glam::IVec2::new(99, 101), size: glam::IVec2::new(2, 1), facing: Side::N, level: 0 };
    w.map.buildings.push(Building {
        template: "test".into(),
        min: glam::IVec2::new(98, 98),
        size: glam::IVec2::new(6, 6),
        levels: 1,
        upper_floors: Vec::new(),
        upper_edges: Vec::new(),
        furniture: vec![sofa],
        roof: crate::map::RoofStyle::Flat,
        siding: 0,
        roof_color: 0,
        look: None,
    });
    let seat = seat_at(&w.map, Vec2::new(100.5, 100.5), 0);
    assert_eq!(seat.kind, SeatKind::Sofa);
    assert!(seat.at.y > 101.0 && seat.at.y < 102.0, "on it: {:?}", seat.at);
    assert!(w.player_sit(id, Some(true)).unwrap().contains("sofa"));
    assert!(w.player_sit(id, Some(false)).unwrap().contains("get up"));
}

#[test]
fn no_sitting_mid_climb_or_dead_and_a_swing_stands_you_up() {
    let (mut w, id) = field();
    let i = pi(&w, id);
    w.players[i].dead = true;
    assert!(w.player_sit(id, None).is_err());
    w.players[i].dead = false;
    w.player_sit(id, None).unwrap();
    w.players[i].input.aim = true;
    w.player_attack(id);
    run(&mut w, 0.2);
    assert_eq!(w.players[i].seated, None);
}

#[test]
fn sitting_is_saved_and_hashed() {
    let (mut w, id) = field();
    let before = w.state_hash();
    w.player_sit(id, None).unwrap();
    assert_ne!(w.state_hash(), before);
    let l = World::load(&w.save(), &w.config).unwrap();
    assert_eq!(l.players[0].seated, Some(SeatKind::Ground));
    assert_eq!(l.state_hash(), w.state_hash());
}
