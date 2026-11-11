//! Building: barricades on windows and doors (boarding up, battering down, prising off, saves).

use glam::{IVec2, Vec2};

use super::*;
use crate::items;
use crate::map::{Floor, Room, Side};
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;

const ITEMS: &str = include_str!("../../../../assets/data/items/items.ron");
const WEAPONS: &str = include_str!("../../../../assets/data/items/weapons.ron");
const BUILDS: &str = include_str!("../../../../assets/data/build.ron");

fn config() -> WorldConfig {
    let templates = vec![BuildingTemplate::from_ron(include_str!("../../../../assets/data/buildings/bungalow.ron")).unwrap()];
    let weapons = items::parse_weapons(WEAPONS).unwrap();
    let items = items::parse_items(ITEMS, &weapons).unwrap();
    let builds = parse_builds_for(BUILDS, &items).unwrap();
    let mut config = WorldConfig::new(5, 0, templates).with_weapons(weapons).with_items(items).with_builds(builds);
    config.start_hour = 12.0;
    config.needs = crate::needs::NeedsTuning::off();
    config
}

/// The window in the east wall of the room in [`room`].
const WINDOW: EdgeRef = EdgeRef { tile: IVec2::new(105, 102), edge: OwnedEdge::West };

/// A room (tiles 100..105 both ways) on an empty field, the east wall's middle edge `kind`, and a
/// player inside by it, facing it, with planks, nails and a claw hammer.
fn room(kind: Edge) -> (World, u32) {
    let mut w = World::new(config());
    // One plain one-storey building, so the room belongs to one (a save checks that).
    let mut house = w.map.buildings[0].clone();
    (house.min, house.size, house.levels) = (IVec2::new(100, 100), IVec2::new(5, 5), 1);
    house.upper_floors.clear();
    house.upper_edges.clear();
    house.furniture.clear();
    w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
    w.map.buildings.push(house);
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
    w.map.set_owned_edge(WINDOW.tile, WINDOW.edge, kind);
    w.map_changed();
    w.items.clear();
    w.spawn_building = 0;
    let id = w.add_player(Vec2::new(104.4, 102.5));
    let pi = w.players.iter().position(|p| p.id == id).unwrap();
    w.players[pi].state.facing = 0.0;
    assert!(w.give_item(id, "wooden_plank", 6));
    assert!(w.give_item(id, "nail", 40));
    assert!(w.give_item(id, "claw_hammer", 1));
    // A pack on the back, so planks prised off have room to go.
    assert!(w.hold_item(id, crate::inventory::Slot::Back, "hiking_pack"));
    (w, id)
}

fn idle(w: &mut World, id: u32) {
    for _ in 0..4000 {
        if w.player(id).is_some_and(|p| p.jobs.is_empty()) {
            return;
        }
        w.step();
    }
    panic!("the jobs never finished");
}

fn build(w: &mut World, id: u32, what: &str) {
    let b = w.build_index(what).unwrap();
    w.player_build(id, b).unwrap();
    idle(w, id);
}

fn planks_carried(w: &World, id: u32) -> u32 {
    let i = w.item_index("wooden_plank").unwrap();
    crate::crafting::have(&w.player(id).unwrap().inventory, i)
}

#[test]
fn the_shipped_catalog_loads() {
    let c = config();
    assert!(c.builds.iter().any(|d| d.id == "board_window" && d.on == BuildOn::Window));
    assert!(c.builds.iter().any(|d| d.id == "board_door" && d.on == BuildOn::Door));
    assert!(parse_builds_for("[(id: \"x\", name: \"X\", category: Barricades, on: Window, secs: 1.0, inputs: [(item: \"no_such\")], hp: 1.0, max: 1, remove: (secs: 1.0))]", &c.items).unwrap_err().contains("no_such"));
}

#[test]
fn a_boarded_window_blocks_walking_and_sight_and_comes_down_plank_by_plank() {
    let (mut w, id) = room(Edge::Window);
    let e = WINDOW;
    build(&mut w, id, "board_window");
    assert_eq!(w.map.owned_edge(e.tile, e.edge), Edge::WindowBoarded);
    let b = *w.map.barricade(e).unwrap();
    assert_eq!((b.planks, b.under, b.owner_side), (1, Edge::Window, false));
    assert!(!w.map.can_step(IVec2::new(104, 102), Side::E), "boards block walking");
    assert!(w.map.blocks_view(IVec2::new(104, 102), Side::E), "and sight");
    assert_eq!(planks_carried(&w, id), 5, "a plank used");
    build(&mut w, id, "board_window");
    build(&mut w, id, "board_window");
    assert_eq!(w.map.barricade(e).unwrap().planks, 3);
    assert_eq!(w.edge_hp(e), Some((75.0, 75.0)));
    // Battered: a plank comes down for each 25 hp.
    assert!(w.damage_edge(e, 30.0));
    assert_eq!(w.map.barricade(e).unwrap().planks, 2);
    assert!(w.damage_edge(e, 44.0));
    assert_eq!(w.map.barricade(e).unwrap().planks, 1);
    assert!(w.damage_edge(e, 10.0));
    assert_eq!(w.map.owned_edge(e.tile, e.edge), Edge::Window, "the window as it was");
    assert!(w.map.barricade(e).is_none());
    let (hp, max) = w.edge_hp(e).unwrap();
    assert_eq!(hp, max, "the glass still whole");
}

#[test]
fn a_full_window_takes_no_more_and_a_crowbar_takes_planks_off() {
    let (mut w, id) = room(Edge::WindowBroken);
    for _ in 0..4 {
        build(&mut w, id, "board_window");
    }
    assert_eq!(w.map.barricade(WINDOW).unwrap().planks, 4);
    let b = w.build_index("board_window").unwrap();
    assert!(w.player_build(id, b).unwrap_err().contains("as far as it goes"));
    let before = planks_carried(&w, id);
    w.player_unbuild(id).unwrap();
    idle(&mut w, id);
    assert_eq!(w.map.barricade(WINDOW).unwrap().planks, 3);
    assert_eq!(planks_carried(&w, id), before + 1, "the plank back");
    for _ in 0..3 {
        w.player_unbuild(id).unwrap();
        idle(&mut w, id);
    }
    assert_eq!(w.map.owned_edge(WINDOW.tile, WINDOW.edge), Edge::WindowBroken, "the broken window as it was");
    assert!(w.player_unbuild(id).is_err());
}

#[test]
fn no_hammer_no_boards() {
    let (mut w, id) = room(Edge::Window);
    let pi = w.players.iter().position(|p| p.id == id).unwrap();
    let hammer = w.item_index("claw_hammer").unwrap();
    w.players[pi].inventory.carried.retain(|s| s.item != hammer);
    w.players[pi].inventory.hand = None;
    let b = w.build_index("board_window").unwrap();
    let err = w.player_build(id, b).unwrap_err();
    assert!(err.contains("claw hammer"), "{err}");
    assert_eq!(w.map.owned_edge(WINDOW.tile, WINDOW.edge), Edge::Window);
}

#[test]
fn an_open_door_is_shut_first_and_a_boarded_one_doesnt_open() {
    let (mut w, id) = room(Edge::DoorOpen);
    let b = w.build_index("board_door").unwrap();
    let err = w.player_build(id, b).unwrap_err();
    assert!(err.contains("shut"), "{err}");
    w.set_edge(WINDOW, Edge::DoorClosed);
    w.player_build(id, b).unwrap();
    idle(&mut w, id);
    assert_eq!(w.map.owned_edge(WINDOW.tile, WINDOW.edge), Edge::DoorBoarded);
    assert!(w.player_interact(id).is_none(), "a boarded door doesn't open");
    // A window build doesn't go on a door.
    let bw = w.build_index("board_window").unwrap();
    assert!(w.player_build(id, bw).is_err());
}

#[test]
fn zombies_batter_through_the_boards_then_the_window() {
    let (mut w, id) = room(Edge::Window);
    build(&mut w, id, "board_window");
    build(&mut w, id, "board_window");
    // Zombies outside, east, drawn to a noise just inside: they batter the boards.
    w.spawn_zombies(4, Vec2::new(108.5, 102.5), 1.0);
    let mut lost_plank = false;
    for k in 0..(240.0 / crate::TICK_DT) as usize {
        if k % 40 == 0 {
            w.make_noise(Vec2::new(103.5, 102.5), 15.0, false);
        }
        w.step();
        if w.map.barricade(WINDOW).is_some_and(|b| b.planks == 1) {
            lost_plank = true;
        }
        if !w.map.owned_edge(WINDOW.tile, WINDOW.edge).is_boarded() {
            break;
        }
    }
    assert!(lost_plank, "a plank came down first");
    assert!(!w.map.owned_edge(WINDOW.tile, WINDOW.edge).is_boarded(), "the boards gave in the end");
}

#[test]
fn barricades_save_and_load() {
    let (mut w, id) = room(Edge::Window);
    build(&mut w, id, "board_window");
    build(&mut w, id, "board_window");
    w.damage_edge(WINDOW, 10.0);
    let bytes = w.save();
    let back = World::load(&bytes, &w.config).unwrap();
    assert_eq!(back.map.barricade(WINDOW), w.map.barricade(WINDOW));
    assert_eq!(back.map.owned_edge(WINDOW.tile, WINDOW.edge), Edge::WindowBoarded);
    assert_eq!(back.state_hash(), w.state_hash());
    // A catalog without the build: the window comes back as it was, with a note.
    let mut other = w.config.clone();
    other.builds.clear();
    let loaded = World::load_with(&bytes, &other, Default::default()).unwrap();
    assert_eq!(loaded.world.map.owned_edge(WINDOW.tile, WINDOW.edge), Edge::Window);
    assert!(loaded.notes.iter().any(|n| n.contains("barricades")), "{:?}", loaded.notes);
}

/// A glass door boarded up keeps its glass and blind through a save (the save's checks take a
/// boarded door for a door).
#[test]
fn a_boarded_glass_door_saves() {
    let (mut w, id) = room(Edge::DoorClosed);
    w.map.glazed.insert(WINDOW.key());
    w.map.blinds.insert(WINDOW.key(), true);
    build(&mut w, id, "board_door");
    assert_eq!(w.map.owned_edge(WINDOW.tile, WINDOW.edge), Edge::DoorBoarded);
    let back = World::load(&w.save(), &w.config).unwrap();
    assert!(back.map.glazed(WINDOW) && back.map.blind(WINDOW) == Some(true));
    assert_eq!(back.state_hash(), w.state_hash());
}
