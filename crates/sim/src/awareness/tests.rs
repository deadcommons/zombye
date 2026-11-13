//! Who sees and hears whom among the players (#66): in plain view, behind a wall, outside the
//! view cone, in the dark, sneaking, with a light, the grace period, and noise heard unseen.

use super::*;
use crate::items::{self, ItemDef, ItemStack, WeaponDef};
use crate::map::{CHUNK_SIZE, Edge, Side, TileMap};
use crate::players::PlayerInput;
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;
use glam::IVec2;
use std::f32::consts::PI;

fn weapons() -> Vec<WeaponDef> {
    items::parse_weapons(include_str!("../../../../assets/data/items/weapons.ron")).unwrap()
}

fn item_defs() -> Vec<ItemDef> {
    items::parse_items(include_str!("../../../../assets/data/items/items.ron"), &weapons()).unwrap()
}

/// An empty field at `hour`: the viewer at (60.5, 60.5) facing east, and another player at `other`.
fn field(hour: f32, other: Vec2) -> (World, u32, u32) {
    let templates = vec![BuildingTemplate::from_ron(include_str!("../../../../assets/data/buildings/bungalow.ron")).unwrap()];
    let mut c = WorldConfig::new(7, 0, templates).with_weapons(weapons()).with_items(item_defs()).with_map_chunks(10);
    c.start_hour = hour;
    c.needs = crate::needs::NeedsTuning::off();
    let mut w = World::new(c);
    w.map = TileMap::new(w.map.width() / CHUNK_SIZE, w.map.height() / CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    w.sites.clear();
    let a = w.add_player(Vec2::new(60.5, 60.5));
    let b = w.add_player(other);
    (w, a, b)
}

fn pi_of(w: &World, id: u32) -> usize {
    w.players.iter().position(|p| p.id == id).unwrap()
}

/// A wall along the west side of column `x`, the whole height of the field.
fn wall(w: &mut World, x: i32) {
    for y in 0..w.map.height() {
        w.map.set_edge(IVec2::new(x, y), Side::W, Edge::Wall);
    }
    w.map_changed();
}

/// Player `id` holds `input` for `n` ticks.
fn hold(w: &mut World, id: u32, input: PlayerInput, n: u32) {
    for _ in 0..n {
        let seq = w.player(id).unwrap().last_input_seq + 1;
        w.set_player_input(id, seq, input);
        w.step();
    }
}

fn east() -> Vec2 {
    Vec2::new(60.5, 60.5)
}

#[test]
fn a_player_in_plain_view_is_seen() {
    let (mut w, a, b) = field(12.0, east() + Vec2::new(30.0, 0.0));
    assert!(!w.player_sees(a, b), "nothing is known before the first tick");
    w.step();
    assert!(w.player_sees(a, b), "30 m ahead at noon");
    assert!(w.player_sees(a, a));
    // The other has its back to us... no: it faces east too, away from us, so it doesn't see us.
    assert!(!w.player_sees(b, a), "we are behind it");
    assert!(w.awareness().heard_now(a).next().is_none(), "seen, so not a sound");
}

#[test]
fn a_player_behind_a_wall_is_not_seen_but_through_a_window_is() {
    let (mut w, a, b) = field(12.0, east() + Vec2::new(6.0, 0.0));
    wall(&mut w, 63);
    w.step();
    assert!(!w.player_sees(a, b), "a wall between us");
    assert!(!w.sees_now(a, b));
    // Right behind the wall, within the range that needs no cone: still not.
    let k = pi_of(&w, b);
    w.players[k].state.pos = Vec2::new(63.5, 60.5);
    for _ in 0..40 {
        w.step();
    }
    assert!(!w.player_sees(a, b), "2 m away through a wall");
    w.map.set_edge(IVec2::new(63, 60), Side::W, Edge::Window);
    w.map_changed();
    w.step();
    assert!(w.player_sees(a, b), "through the window");
    w.map.set_edge(IVec2::new(63, 60), Side::W, Edge::DoorClosed);
    w.map_changed();
    for _ in 0..40 {
        w.step();
    }
    assert!(!w.player_sees(a, b), "a shut door");
}

#[test]
fn a_player_outside_the_view_cone_is_not_seen() {
    // 20 m behind the viewer (who faces east).
    let (mut w, a, b) = field(12.0, east() - Vec2::new(20.0, 0.0));
    w.step();
    assert!(!w.player_sees(a, b), "behind us");
    assert!(w.player_sees(b, a), "it faces east, toward us");
    // We turn round: now we do.
    let k = pi_of(&w, a);
    w.players[k].state.facing = PI;
    w.players[k].input.facing = PI;
    w.step();
    assert!(w.player_sees(a, b));
    // And right behind us, within arm's length, anyone is noticed.
    let (mut w, a, b) = field(12.0, east() - Vec2::new(1.5, 0.0));
    w.step();
    assert!(w.player_sees(a, b), "1.5 m behind");
}

#[test]
fn a_player_in_the_dark_is_not_seen_unless_close_or_lit() {
    let (mut w, a, b) = field(1.0, east() + Vec2::new(20.0, 0.0));
    w.step();
    assert!(!w.player_sees(a, b), "20 m off at 1 am");
    // The same place at noon.
    let (mut day, da, db) = field(12.0, east() + Vec2::new(20.0, 0.0));
    day.step();
    assert!(day.player_sees(da, db));
    // Close up, even at night.
    let k = pi_of(&w, b);
    w.players[k].state.pos = east() + Vec2::new(6.0, 0.0);
    w.step();
    assert!(w.player_sees(a, b), "6 m off at night");
    // Further off again, but holding a flashlight: a light in the dark shows.
    w.players[k].state.pos = east() + Vec2::new(25.0, 0.0);
    for _ in 0..40 {
        w.step();
    }
    assert!(!w.player_sees(a, b), "25 m off, dark");
    let torch = w.item_index("flashlight").expect("a flashlight in the items");
    w.players[k].inventory.hand = Some(ItemStack::new(torch, 1));
    w.step();
    assert!(w.player_sees(a, b), "its light gives it away");
    // Ours shows whoever stands in the beam (12 m + 2), not beside it.
    w.players[k].inventory.hand = None;
    w.players[k].state.pos = east() + Vec2::new(13.0, 0.0);
    for _ in 0..40 {
        w.step();
    }
    assert!(!w.player_sees(a, b), "13 m off, dark, no lights");
    let me = pi_of(&w, a);
    w.players[me].inventory.hand = Some(ItemStack::new(torch, 1));
    w.step();
    assert!(w.player_sees(a, b), "in our beam");
    w.players[k].state.pos = east() + Vec2::new(0.0, 13.0);
    for _ in 0..40 {
        w.step();
    }
    assert!(!w.player_sees(a, b), "beside the beam");
}

#[test]
fn sneaking_and_stealth_shorten_the_range() {
    let t = PlayerSightTuning::default();
    let viewer = Body::standing(1, east(), 0.0);
    let walker = Body::standing(2, east() + Vec2::new(10.0, 0.0), 0.0);
    let ahead = sight_range(&t, 1.0, &viewer, &walker);
    assert_eq!(ahead, t.sight_day);
    let sneak = sight_range(&t, 1.0, &viewer, &Body { stance: Stance::Sneak, ..walker });
    let run = sight_range(&t, 1.0, &viewer, &Body { stance: Stance::Run, ..walker });
    let still = sight_range(&t, 1.0, &viewer, &Body { moving: false, ..walker });
    let stealthy = sight_range(&t, 1.0, &viewer, &Body { sight_mult: 0.7, ..walker });
    assert!(sneak < still && still < ahead && ahead < run, "{sneak} {still} {ahead} {run}");
    assert!((stealthy - ahead * 0.7).abs() < 1e-3, "Stealth's multiplier applies: {stealthy}");
    let night = sight_range(&t, 0.0, &viewer, &walker);
    assert_eq!(night, t.sight_night);
    // To the side less, behind only arm's length.
    let side = sight_range(&t, 1.0, &viewer, &Body { pos: east() + Vec2::from_angle(115f32.to_radians()) * 10.0, ..walker });
    let behind = sight_range(&t, 1.0, &viewer, &Body { pos: east() - Vec2::new(10.0, 0.0), ..walker });
    assert!((side - ahead * t.peripheral_mult).abs() < 1e-3 && behind == t.rear_range, "{side} {behind}");
    // Asleep, eyes shut.
    let map = TileMap::new(8, 8);
    assert!(sees(&map, &t, 1.0, &viewer, &walker));
    assert!(!sees(&map, &t, 1.0, &Body { asleep: true, ..viewer }, &walker));

    // In the world: someone sneaking 50 m off by day isn't made out, walking is.
    let (mut w, a, b) = field(12.0, east() + Vec2::new(50.0, 0.0));
    hold(&mut w, b, PlayerInput { move_dir: Vec2::Y, sneak: true, ..Default::default() }, 30);
    assert!(!w.player_sees(a, b), "sneaking at 50 m");
    hold(&mut w, b, PlayerInput { move_dir: Vec2::Y, ..Default::default() }, 5);
    assert!(w.player_sees(a, b), "walking at 50 m");
}

#[test]
fn sight_lasts_a_moment_after_it_is_lost() {
    let (mut w, a, b) = field(12.0, east() + Vec2::new(8.0, 0.0));
    wall(&mut w, 70);
    w.step();
    assert!(w.player_sees(a, b));
    // It steps behind the wall.
    let k = pi_of(&w, b);
    w.players[k].state.pos = Vec2::new(72.5, 60.5);
    let grace = (w.config.player_sight.grace_secs * crate::TICK_RATE as f32) as u32;
    for n in 1..=grace {
        w.step();
        assert!(w.player_sees(a, b), "still told of it {n} ticks after");
        assert!(!w.sees_now(a, b));
    }
    w.step();
    assert!(!w.player_sees(a, b), "gone after the grace period");
}

#[test]
fn an_unseen_player_is_heard_as_a_sound_at_a_position() {
    // Behind a wall 3 m east of us.
    let (mut w, a, b) = field(12.0, Vec2::new(63.5, 60.5));
    wall(&mut w, 62);
    // Standing still: nothing.
    for _ in 0..30 {
        w.step();
        assert!(!w.player_sees(a, b));
        assert!(w.awareness().heard_now(a).next().is_none(), "it makes no sound");
    }
    // Running on the spot against the wall would not move it; run along the wall instead.
    let mut told = Vec::new();
    for _ in 0..40 {
        hold(&mut w, b, PlayerInput { move_dir: Vec2::Y, run: true, ..Default::default() }, 1);
        assert!(!w.player_sees(a, b), "a wall between us");
        told.extend(w.awareness().heard_now(a).copied());
    }
    assert!(!told.is_empty(), "its running is heard through the wall");
    // Not every tick: about one every 0.4 s over these 2 s.
    assert!(told.len() <= 6, "{} sounds in 2 s", told.len());
    let first = told[0];
    assert!((first.radius - w.config.zombie.run_noise).abs() < 1e-3, "as loud as running: {first:?}");
    assert!(first.pos.distance(Vec2::new(63.5, 60.5)) < 1.0, "where it was: {first:?}");
    assert!(w.awareness().heard_lately(a).count() >= told.len().min(2));
    // The runner hears nothing of us (we stand still).
    assert!(w.awareness().heard_lately(b).next().is_none());

    // Sneaking past behind the wall isn't heard (1.2 m carries 0.5 m through a wall).
    let (mut w, a, b) = field(12.0, Vec2::new(63.5, 60.5));
    wall(&mut w, 62);
    for _ in 0..40 {
        hold(&mut w, b, PlayerInput { move_dir: Vec2::Y, sneak: true, ..Default::default() }, 1);
        assert!(w.awareness().heard_now(a).next().is_none(), "a sneaker behind a wall");
    }
    // In the open behind our back, walking 3 m away is heard, 30 m away isn't.
    let (mut w, a, b) = field(12.0, east() - Vec2::new(3.0, 0.0));
    hold(&mut w, b, PlayerInput { move_dir: Vec2::Y, ..Default::default() }, 3);
    assert!(!w.player_sees(a, b) && w.awareness().heard_lately(a).next().is_some(), "footsteps behind us");
    let (mut w, a, b) = field(12.0, east() - Vec2::new(30.0, 0.0));
    hold(&mut w, b, PlayerInput { move_dir: Vec2::Y, run: true, ..Default::default() }, 20);
    assert!(!w.player_sees(a, b) && w.awareness().heard_lately(a).next().is_none(), "too far to hear");
}

#[test]
fn awareness_leaves_the_simulation_alone() {
    // Two worlds, the same but for how survivors see each other: the same state hash every tick.
    let run = |sight: PlayerSightTuning| {
        let (mut w, _, b) = field(12.0, east() + Vec2::new(12.0, 0.0));
        w.config.player_sight = sight;
        let mut hashes = Vec::new();
        for _ in 0..60 {
            hold(&mut w, b, PlayerInput { move_dir: -Vec2::X, run: true, ..Default::default() }, 1);
            hashes.push(w.state_hash());
        }
        hashes
    };
    assert_eq!(run(PlayerSightTuning::default()), run(PlayerSightTuning { sight_day: 1.0, sight_night: 1.0, rear_range: 0.0, ..Default::default() }));
    // One player alone: nothing is kept.
    let (mut w, a, _) = field(12.0, east() + Vec2::new(5.0, 0.0));
    w.players.pop();
    for _ in 0..5 {
        w.step();
    }
    assert_eq!(w.awareness().now(), 0);
    assert!(w.player_sees(a, a));
}
