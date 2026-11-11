//! Blinds: fitting, rolling, taking down, zombies' sight through glass, glazed doors, the town's
//! own blinds, saves and replication.

use glam::{IVec2, Vec2};

use super::*;
use crate::items;
use crate::map::{Floor, Room, Side};
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;
use crate::zombies::ZombieState;

const ITEMS: &str = include_str!("../../../../assets/data/items/items.ron");
const WEAPONS: &str = include_str!("../../../../assets/data/items/weapons.ron");

fn templates() -> Vec<BuildingTemplate> {
    [include_str!("../../../../assets/data/buildings/bungalow.ron"), include_str!("../../../../assets/data/buildings/family_two_storey.ron")]
        .iter()
        .map(|t| BuildingTemplate::from_ron(t).unwrap())
        .collect()
}

fn config(seed: u64) -> WorldConfig {
    let weapons = items::parse_weapons(WEAPONS).unwrap();
    let items = items::parse_items(ITEMS, &weapons).unwrap();
    let mut config = WorldConfig::new(seed, 0, templates()).with_weapons(weapons).with_items(items);
    config.start_hour = 12.0;
    config
}

/// The window in the east wall of the room in [`room`].
const WINDOW: EdgeRef = EdgeRef { tile: IVec2::new(105, 102), edge: OwnedEdge::West };
/// The door in its south wall.
const DOOR: EdgeRef = EdgeRef { tile: IVec2::new(102, 105), edge: OwnedEdge::North };

/// A room (tiles 100..105 both ways) on an empty field at noon, a window in its east wall and a
/// door in its south wall, and a player inside by the window, facing it.
fn room() -> (World, u32) {
    let mut w = World::new(config(3));
    w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
    w.map.rooms.push(Room { building: 0, name: "Bedroom".into(), room_type: "bedroom".into(), floor: Floor::Wood });
    for y in 100..105 {
        for x in 100..105 {
            w.map.tile_mut(IVec2::new(x, y)).unwrap().room = 1;
        }
    }
    for k in 100..105 {
        w.map.set_edge(IVec2::new(k, 100), Side::N, Edge::Wall);
        w.map.set_edge(IVec2::new(k, 104), Side::S, Edge::Wall);
        w.map.set_edge(IVec2::new(100, k), Side::W, Edge::Wall);
        w.map.set_edge(IVec2::new(104, k), Side::E, Edge::Wall);
    }
    w.map.set_owned_edge(WINDOW.tile, WINDOW.edge, Edge::Window);
    w.map.set_owned_edge(DOOR.tile, DOOR.edge, Edge::DoorClosed);
    w.map_changed();
    w.items.clear();
    w.spawn_building = 0;
    let id = w.add_player(Vec2::new(104.4, 102.5));
    face(&mut w, id, 0.0);
    (w, id)
}

fn face(w: &mut World, id: u32, facing: f32) {
    let pi = w.players.iter().position(|p| p.id == id).unwrap();
    w.players[pi].state.facing = facing;
}

fn give_blind(w: &mut World, id: u32) {
    assert!(w.hold_item(id, crate::inventory::Slot::OffHand, "roller_blind"));
}

fn run(w: &mut World, secs: f32) {
    for _ in 0..(secs / crate::TICK_DT).ceil() as u32 {
        w.step();
    }
}

fn notes(w: &mut World) -> Vec<String> {
    std::mem::take(&mut w.item_notes).into_iter().map(|n| n.text).collect()
}

#[test]
fn a_blind_is_fitted_rolled_and_taken_down() {
    let (mut w, id) = room();
    assert_eq!(w.player_blind(id, BlindOp::Toggle), Err("no blind here: face a window or a glass door with one, from inside".to_string()));
    assert!(w.player_blind(id, BlindOp::Fit).unwrap_err().contains("no blind to fit"));
    give_blind(&mut w, id);
    w.player_blind(id, BlindOp::Use).unwrap();
    assert!(matches!(w.player(id).unwrap().jobs[0].kind, JobKind::Blind { edge: WINDOW, fit: true }));
    run(&mut w, BlindTuning::default().fit_secs + 0.2);
    assert_eq!(w.map.blind(WINDOW), Some(true), "fitted down");
    assert!(w.player(id).unwrap().inventory.off_hand.is_none(), "the blind is used");
    assert!(notes(&mut w).iter().any(|n| n.starts_with("fitted a blind")));
    let changes = w.take_blind_changes();
    assert_eq!(changes, vec![BlindChange { tile: WINDOW.tile, edge: WINDOW.edge, blind: Some(true) }]);

    // The key's likely thing now rolls it up, and again down.
    w.player_blind(id, BlindOp::Use).unwrap();
    assert_eq!(w.map.blind(WINDOW), Some(false));
    w.player_blind(id, BlindOp::Toggle).unwrap();
    assert_eq!(w.map.blind(WINDOW), Some(true));
    assert_eq!(w.take_blind_changes().len(), 2);

    w.player_blind(id, BlindOp::TakeDown).unwrap();
    run(&mut w, BlindTuning::default().take_down_secs + 0.2);
    assert_eq!(w.map.blind(WINDOW), None);
    let blind = w.item_index("roller_blind").unwrap();
    assert!(w.player(id).unwrap().inventory.carried.iter().any(|s| s.item == blind), "back in the bags");
}

#[test]
fn only_from_inside_and_within_reach() {
    let (mut w, id) = room();
    give_blind(&mut w, id);
    // Outside the window, facing it.
    let pi = w.players.iter().position(|p| p.id == id).unwrap();
    w.players[pi].state.pos = Vec2::new(105.6, 102.5);
    face(&mut w, id, std::f32::consts::PI);
    assert!(w.player_blind(id, BlindOp::Fit).is_err());
    // Inside again, but it walks off before the job is done.
    w.players[pi].state.pos = Vec2::new(104.4, 102.5);
    face(&mut w, id, 0.0);
    w.player_blind(id, BlindOp::Fit).unwrap();
    w.players[pi].state.pos = Vec2::new(101.5, 101.5);
    run(&mut w, BlindTuning::default().fit_secs + 0.2);
    assert_eq!(w.map.blind(WINDOW), None);
    assert!(notes(&mut w).iter().any(|n| n == "moved away from the window"));
}

/// Does a zombie standing outside the window notice the player standing inside it?
fn seen_through_window(down: Option<bool>) -> bool {
    let (mut w, id) = room();
    if let Some(down) = down {
        w.set_blind(WINDOW, Some(down));
    }
    let _ = id;
    let z = w.add_zombie(Vec2::new(106.8, 102.5), std::f32::consts::PI, 0.0);
    w.zombies.wander_timer[z] = 1e9;
    run(&mut w, 4.0);
    // Noticed: after the player, or come over for a look.
    w.zombies.state[z] != ZombieState::Wander
}

#[test]
fn a_blind_down_keeps_zombies_from_seeing_in() {
    assert!(seen_through_window(None), "no blind: seen through the glass");
    assert!(seen_through_window(Some(false)), "rolled up: seen");
    assert!(!seen_through_window(Some(true)), "down: not seen");
}

#[test]
fn a_glazed_door_lets_sight_through_until_its_blind_is_down() {
    let (mut w, id) = room();
    let (inside, outside) = (Vec2::new(102.5, 104.5), Vec2::new(102.5, 108.5));
    assert!(!crate::vision::line_of_sight(&w.map, inside, outside), "a plain door");
    w.map.glazed.insert(DOOR.key());
    assert!(crate::vision::line_of_sight(&w.map, inside, outside), "glass in it");
    // Fitted from inside, facing the door.
    let pi = w.players.iter().position(|p| p.id == id).unwrap();
    w.players[pi].state.pos = Vec2::new(102.5, 104.4);
    face(&mut w, id, std::f32::consts::FRAC_PI_2);
    give_blind(&mut w, id);
    w.player_blind(id, BlindOp::Fit).unwrap();
    run(&mut w, BlindTuning::default().fit_secs + 0.2);
    assert_eq!(w.map.blind(DOOR), Some(true));
    assert!(!crate::vision::line_of_sight(&w.map, inside, outside));
    // Battered down: the blind falls inside.
    w.break_edge(DOOR, SoundSource::World);
    assert_eq!(w.map.blind(DOOR), None);
    let blind = w.item_index("roller_blind").unwrap();
    assert!(w.items.iter().any(|it| it.stack.item == blind && it.pos.y < 105.0));
}

#[test]
fn the_town_has_blinds_and_glazed_doors_from_the_start() {
    let w = World::new(config(1));
    let blinds = w.map.blinds.len();
    let down = w.map.blinds.values().filter(|d| **d).count();
    assert!(blinds > 20, "{blinds} blinds");
    assert!(down > 0 && down < blinds, "{down} of {blinds} down");
    assert!(!w.map.glazed.is_empty());
    for (&key, _) in &w.map.blinds {
        assert!(w.map.takes_blind(crate::world::edge_of_key(key)));
    }
    // The same for the same seed.
    let again = World::new(config(1));
    assert_eq!(again.map.blinds, w.map.blinds);
    assert_eq!(again.map.glazed, w.map.glazed);
    // And none with none asked for.
    let mut bare = config(1);
    bare.blinds = BlindTuning::none();
    let bare = World::new(bare);
    assert!(bare.map.blinds.is_empty() && bare.map.glazed.is_empty());
}

#[test]
fn blinds_are_saved_and_loaded() {
    let mut w = World::new(config(2));
    let id = w.add_player(w.player_spawn);
    let (&first, &down) = w.map.blinds.iter().next().unwrap();
    let e = crate::world::edge_of_key(first);
    w.set_blind(e, Some(!down));
    let other = w.map.blinds.keys().nth(1).copied().unwrap();
    w.set_blind(crate::world::edge_of_key(other), None);
    let hash = w.state_hash();
    let bytes = w.save();
    let loaded = World::load(&bytes, &config(2)).unwrap();
    assert_eq!(loaded.map.blinds, w.map.blinds);
    assert_eq!(loaded.map.glazed, w.map.glazed);
    assert_eq!(loaded.state_hash(), hash);
    let _ = id;
}
