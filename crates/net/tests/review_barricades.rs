//! Saves with the spawn house boarded up, for reviewing barricades in offscreen pictures
//! (`zombye --render-to shot.png --load PATH`). Ignored: run it by hand with the folder to write
//! them to (without one: `target/review`):
//!
//! `ZOMBYE_REVIEW_DIR=/tmp/review cargo test -p zombye-net --release --test review_barricades -- --ignored --nocapture`
//!
//! `barricades`: every outside window and door of the house a player wakes up in boarded from
//! inside, with one to five planks (so each count shows), the player standing outside its front;
//! `barricades_inside` the same with the player inside.

use std::path::PathBuf;

use glam::{IVec2, Vec2};
use zombye_net::{content, save_file};
use zombye_sim::World;
use zombye_sim::map::{Edge, EdgeRef, OwnedEdge};

fn save(w: &World, name: &str) {
    let dir = std::env::var("ZOMBYE_REVIEW_DIR").map_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/review"), PathBuf::from);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.zsave"));
    save_file::save(w, &path).unwrap();
    println!("wrote {}", path.display());
}

/// The spawn house's outside windows and doors, each with the tile inside it.
fn openings(w: &World) -> Vec<(EdgeRef, IVec2)> {
    let b = &w.map.buildings[w.spawn_building];
    let inside = |r: u16| r != 0 && w.map.rooms[r as usize].building as usize == w.spawn_building;
    let mut out = Vec::new();
    for y in b.min.y..=b.min.y + b.size.y {
        for x in b.min.x..=b.min.x + b.size.x {
            let t = IVec2::new(x, y);
            for which in [OwnedEdge::North, OwnedEdge::West] {
                let kind = w.map.owned_edge(t, which);
                if !(kind.is_window() || kind == Edge::DoorClosed || kind == Edge::DoorOpen) {
                    continue;
                }
                let other = if which == OwnedEdge::North { t - IVec2::Y } else { t - IVec2::X };
                let (a, b) = (inside(w.map.tile(t).room), inside(w.map.tile(other).room));
                if a != b && (w.map.tile(t).room == 0 || w.map.tile(other).room == 0) {
                    out.push((EdgeRef::new(t, which), if a { t } else { other }));
                }
            }
        }
    }
    out
}

fn boarded() -> (World, u32) {
    let mut config = content::load_config(&content::default_data_dir(), 1).unwrap();
    config.zombies = Some(0);
    config.start_hour = 15.0;
    let mut w = World::new(config);
    let id = w.add_player(w.player_spawn);
    assert!(w.give_item(id, "claw_hammer", 1));
    for (n, (e, inside)) in openings(&w).into_iter().enumerate() {
        let door = w.map.owned_edge(e.tile, e.edge).is_door();
        let build = if door { "board_door" } else { "board_window" };
        let planks = 1 + n % 5;
        assert!(w.give_item(id, "wooden_plank", planks as u16) && w.give_item(id, "nail", 4 * planks as u16));
        let from = inside.as_vec2() + Vec2::splat(0.5);
        let to = e.mid() - from;
        let pi = w.players.iter().position(|p| p.id == id).unwrap();
        w.players[pi].state.pos = from;
        w.players[pi].state.facing = to.y.atan2(to.x);
        for _ in 0..planks {
            if let Err(why) = w.player_build_id(id, build) {
                println!("{e:?}: {why}");
                break;
            }
            for _ in 0..400 {
                if w.player(id).is_some_and(|p| p.jobs.is_empty()) {
                    break;
                }
                w.step();
            }
        }
    }
    (w, id)
}

#[test]
#[ignore]
fn barricade_saves() {
    let (mut w, id) = boarded();
    save(&w, "barricades_inside");
    let pi = w.players.iter().position(|p| p.id == id).unwrap();
    let b = w.map.buildings[w.spawn_building].clone();
    // Outside, a few metres off the house's south-west corner, looking at it.
    w.players[pi].state.pos = Vec2::new(b.min.x as f32 - 3.0, (b.min.y + b.size.y) as f32 + 3.0);
    w.step();
    save(&w, "barricades");
    println!("house at {:?} size {:?}; {} boarded", b.min, b.size, w.map.barricades.len());
}
