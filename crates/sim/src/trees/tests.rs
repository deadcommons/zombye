//! Chopping: only what chops bites, a tree comes down in about the swings its hit points say,
//! leaves a stump that bodies walk over and logs on the ground, and a save keeps it all.

use glam::{IVec2, Vec2};

use super::*;
use crate::PlayerInput;
use crate::inventory::{Place, Slot};
use crate::items;
use crate::map::TileMap;
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;

const WEAPONS: &str = include_str!("../../../../assets/data/items/weapons.ron");
const ITEMS: &str = include_str!("../../../../assets/data/items/items.ron");
const RECIPES: &str = include_str!("../../../../assets/data/items/recipes.ron");
const ORIGIN: Vec2 = Vec2::new(100.5, 100.5);
const TREE: IVec2 = IVec2::new(101, 100);

fn config(seed: u64) -> WorldConfig {
    let templates = vec![BuildingTemplate::from_ron(include_str!("../../../../assets/data/buildings/bungalow.ron")).unwrap()];
    let weapons = items::parse_weapons(WEAPONS).unwrap();
    let items = items::parse_items(ITEMS, &weapons).unwrap();
    let mut config = WorldConfig::new(seed, 0, templates).with_weapons(weapons).with_items(items);
    config.start_hour = 12.0;
    config
}

/// An empty field with a broadleaf a metre east of a player at `ORIGIN` holding `held`.
fn field(held: &str) -> (World, u32) {
    let mut w = World::new(config(3));
    // No deadwood: the logs counted are the tree's.
    w.config.chop.deadwood_chance = 0.0;
    w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
    w.map.tile_mut(TREE).unwrap().feature = Feature::Tree;
    w.map_changed();
    w.items.clear();
    w.spawn_building = 0;
    let id = w.add_player(ORIGIN);
    assert!(w.hold_item(id, Slot::Hand, held));
    w.set_dev(id, None, Some(true));
    (w, id)
}

/// One swing east, stepped until the player can act again.
fn swing(w: &mut World, id: u32) -> Vec<ChopEvent> {
    let seq = w.tick as u32 + 1;
    w.set_player_input(id, seq, PlayerInput { facing: 0.0, aim: true, ..Default::default() });
    assert!(w.player_attack(id));
    let mut chops = Vec::new();
    for _ in 0..200 {
        w.step();
        chops.extend_from_slice(w.chop_events());
        let p = w.player(id).unwrap();
        if p.action.is_none() && p.ready_in <= 0.0 {
            break;
        }
    }
    chops
}

fn logs_on_floor(w: &World) -> u16 {
    let log = w.item_index("log").unwrap();
    w.items().iter().filter(|it| it.stack.item == log).map(|it| it.stack.count).sum()
}

#[test]
fn a_hatchet_fells_a_tree_into_a_stump_and_logs() {
    let (mut w, id) = field("hatchet");
    let (hp, max) = w.tree_hp(TREE).unwrap();
    assert_eq!(hp, max);
    let mut swings = 0;
    let mut events = Vec::new();
    while w.map.tile(TREE).feature == Feature::Tree {
        let chops = swing(&mut w, id);
        assert_eq!(chops.len(), 1, "each swing bites once");
        events.extend(chops);
        swings += 1;
        assert!(swings < 40, "never came down");
        let pi = w.players.iter().position(|p| p.id == id).unwrap();
        w.players[pi].state.endurance = 1.0;
        w.players[pi].state.winded = false;
    }
    // Middle damage 36 into 480 hit points.
    assert!((12..=16).contains(&swings), "{swings} swings");
    assert!(events.windows(2).all(|p| p[1].left < p[0].left));
    assert!(events.last().unwrap().felled);
    assert_eq!(w.map.tile(TREE).feature, Feature::Stump);
    assert_eq!(w.tree_hp(TREE), None);
    assert_eq!(logs_on_floor(&w), w.chop_tuning().tree_logs);
    assert_eq!(w.stumps(), vec![TreeChange { tile: TREE, feature: Feature::Stump }]);
    assert_eq!(w.take_tree_changes().len(), 1);
    assert!(w.take_tree_changes().is_empty());
    // A stump is stepped past: nothing pushes a body off its tile any more.
    let middle = TREE.as_vec2() + Vec2::splat(0.5);
    assert_eq!(w.map.resolve_circle(middle, 0.3), middle);
    assert!(!Feature::Stump.blocks_movement());
}

#[test]
fn only_what_chops_bites_and_a_body_in_the_way_takes_the_blow() {
    // A bat hits nothing: the tree is untouched and the bat takes no knocks.
    let (mut w, id) = field("baseball_bat");
    for _ in 0..5 {
        assert!(swing(&mut w, id).is_empty());
    }
    assert_eq!(w.tree_hp(TREE).map(|(hp, max)| hp == max), Some(true));
    assert_eq!(w.player(id).unwrap().inventory.hand.unwrap().used, 0);

    // A zombie between the hatchet and the trunk takes the blow.
    let (mut w, id) = field("hatchet");
    let i = w.add_zombie(ORIGIN + Vec2::new(0.55, 0.0), std::f32::consts::PI, 0.0);
    w.zombies.wander_timer[i] = 1e9;
    w.zombies.hp[i] = 1e5;
    assert!(swing(&mut w, id).is_empty());
    assert!(w.zombies.hp[i] < 1e5);
}

#[test]
fn chopping_wears_the_blade_and_is_noisy() {
    let (mut w, id) = field("hatchet");
    for _ in 0..40 {
        swing(&mut w, id);
        if w.map.tile(TREE).feature != Feature::Tree {
            break;
        }
        let pi = w.players.iter().position(|p| p.id == id).unwrap();
        w.players[pi].state.endurance = 1.0;
    }
    assert!(w.player(id).unwrap().inventory.hand.unwrap().used > 0, "a tree's worth of chops leaves a knock");
    // The fall is heard a long way off.
    let (mut w, _) = field("hatchet");
    w.fell_tree(TREE, Vec2::X);
    assert!(w.sounds.iter().any(|s| s.radius == w.chop_tuning().fall_noise));
}

#[test]
fn a_save_keeps_stumps_and_chopped_trees() {
    let (mut w, id) = field("hatchet");
    let other = TREE + IVec2::new(0, 3);
    w.map.tile_mut(other).unwrap().feature = Feature::PineTree;
    w.map_changed();
    swing(&mut w, id);
    w.fell_tree(TREE, Vec2::X);
    w.trees.hp.insert((other.x, other.y), 123.0);
    let mut back = World::load(&w.save(), &w.config).unwrap();
    // Dev toggles aren't saved.
    back.set_dev(id, None, Some(true));
    assert_eq!(back.trees.felled, w.trees.felled);
    assert_eq!(back.trees.hp, w.trees.hp);
    assert_eq!(back.map.tile(TREE).feature, Feature::Stump);
    assert_eq!(back.state_hash(), w.state_hash());
    assert_eq!(back.tree_hp(other), Some((123.0, w.chop_tuning().pine_hp)));
    assert_eq!(back.stumps(), w.stumps());
}

#[test]
fn a_regenerated_map_takes_its_stumps_back() {
    // A world on its generated map: the save stores edits, and the stumps go back on it on load.
    let mut w = World::new(config(5));
    let tree = (0..w.map.height())
        .flat_map(|y| (0..w.map.width()).map(move |x| IVec2::new(x, y)))
        .find(|&t| w.map.tile(t).feature == Feature::PineTree)
        .expect("a pine in the forest");
    let before = w.state_hash();
    w.fell_tree(tree, Vec2::Y);
    assert_ne!(w.state_hash(), before);
    assert_eq!(w.chop_tuning().pine_logs, logs_on_floor(&w));
    let back = World::load(&w.save(), &w.config).unwrap();
    assert_eq!(back.map.tile(tree).feature, Feature::Stump);
    assert_eq!(back.state_hash(), w.state_hash());
    // And it steps alike.
    let (mut a, mut b) = (w, back);
    for _ in 0..40 {
        a.step();
        b.step();
    }
    assert_eq!(a.state_hash(), b.state_hash());
}

#[test]
fn tree_saves_are_checked() {
    let mut w = World::new(config(5));
    let tree = (0..w.map.height())
        .flat_map(|y| (0..w.map.width()).map(move |x| IVec2::new(x, y)))
        .find(|&t| w.map.tile(t).feature == Feature::Tree)
        .expect("a tree");
    let bad = |save: TreeSave| {
        let mut w = World::new(config(5));
        w.restore_trees(save, false).unwrap_err()
    };
    assert!(bad(TreeSave { hp: vec![], felled: vec![((tree.x, tree.y), Feature::Bush)] }).contains("Bush"));
    assert!(bad(TreeSave { hp: vec![((0, 0), 10.0)], felled: vec![] }).contains("no"));
    assert!(bad(TreeSave { hp: vec![((tree.x, tree.y), -1.0)], felled: vec![] }).contains("hit points"));
    assert!(bad(TreeSave { hp: vec![], felled: vec![((tree.x, tree.y), Feature::Tree), ((tree.x, tree.y), Feature::Tree)] }).contains("order"));
    // On a map that changed, misfits are dropped instead.
    assert_eq!(w.restore_trees(TreeSave { hp: vec![((0, 0), 10.0)], felled: vec![] }, true), Ok(1));
}

#[test]
fn logs_saw_into_planks() {
    let c = config(1);
    let recipes = crate::crafting::parse_recipes(RECIPES).unwrap();
    crate::crafting::check_recipes(&recipes, &c.items).unwrap();
    let saw = recipes.iter().find(|r| r.id == "saw_log_planks").unwrap();
    assert_eq!(saw.inputs[0].item, "log");
    assert_eq!(saw.outputs[0].item, "wooden_plank");
    // Something that chops is in the content (the hatchet and the fire axe).
    assert!(c.items.iter().filter(|d| d.has_tag(&ChopTuning::default().tag)).count() >= 2);
    assert!(c.items.iter().any(|d| d.id == ChopTuning::default().log_item));
}


#[test]
fn a_log_on_the_floor_is_sawn_where_it_lies() {
    let (mut w, id) = field("hatchet");
    w.config.recipes = crate::crafting::parse_recipes(RECIPES).unwrap();
    let recipe = w.recipe_index("saw_log_planks").unwrap();
    // Too big for the pockets, so it can't be picked up: it lies at our feet.
    w.spawn_item("log", 1, ORIGIN + Vec2::new(0.5, 0.0)).unwrap();
    assert!(w.player_craft(id, recipe).is_err(), "no saw yet");
    assert!(w.give_item(id, "saw", 1));
    w.player_craft(id, recipe).unwrap();
    for _ in 0..2000 {
        w.step();
        if w.player(id).unwrap().jobs.is_empty() {
            break;
        }
    }
    assert_eq!(logs_on_floor(&w), 0, "the log was used");
    let plank = w.item_index("wooden_plank").unwrap();
    let carried: u16 = w.player(id).unwrap().inventory.carried.iter().filter(|s| s.item == plank).map(|s| s.count).sum();
    let lying: u16 = w.items().iter().filter(|it| it.stack.item == plank).map(|it| it.stack.count).sum();
    assert_eq!(carried + lying, 3);
}

/// A log lying on the floor is split where it lies, the hatchet in hand, and the firewood falls at
/// our feet when the pockets can't take it.
#[test]
fn a_log_on_the_floor_is_split_where_it_lies() {
    let (mut w, id) = field("hatchet");
    w.config.recipes = crate::crafting::parse_recipes(RECIPES).unwrap();
    let recipe = w.recipe_index("split_log").unwrap();
    w.spawn_item("log", 1, ORIGIN + Vec2::new(0.5, 0.0)).unwrap();
    w.player_craft(id, recipe).unwrap();
    for _ in 0..2000 {
        w.step();
        if w.player(id).unwrap().jobs.is_empty() {
            break;
        }
    }
    assert_eq!(logs_on_floor(&w), 0, "the log was used");
    let wood = w.item_index("firewood").unwrap();
    let carried: u16 = w.player(id).unwrap().inventory.carried.iter().filter(|s| s.item == wood).map(|s| s.count).sum();
    let lying: u16 = w.items().iter().filter(|it| it.stack.item == wood).map(|it| it.stack.count).sum();
    assert_eq!(carried + lying, 4);
}

/// A log taken up from the floor doesn't fit the pockets, so it goes into both hands, and what the
/// hand held goes into the pockets.
#[test]
fn a_log_taken_up_is_carried_in_the_hands() {
    let (mut w, id) = field("hatchet");
    w.spawn_item("log", 1, ORIGIN + Vec2::new(0.5, 0.0)).unwrap();
    let ground = w.items().iter().find(|it| it.stack.item == w.item_index("log").unwrap()).unwrap().id;
    w.player_move_item(id, Place::Ground, ground, Place::Carried, 0).unwrap();
    for _ in 0..200 {
        w.step();
        if w.player(id).unwrap().jobs.is_empty() {
            break;
        }
    }
    let inv = &w.player(id).unwrap().inventory;
    assert_eq!(inv.hand.map(|s| s.item), w.item_index("log"), "the log is in the hands");
    assert!(inv.carried.iter().any(|s| Some(s.item) == w.item_index("hatchet")), "the hatchet went into the pockets");
    assert_eq!(logs_on_floor(&w), 0);
}

/// Deadwood: logs lie on the forest's spots near a player, go back when nobody is near, and a
/// log taken is gone from its spot for good; a save keeps it all.
#[test]
fn deadwood_lies_near_players_and_is_taken_back() {
    let mut w = World::new(config(5));
    let town = w.town();
    let seed = w.config.generation.seed;
    let chance = w.config.chop.deadwood_chance;
    let spot = (0..w.map.height() as i32)
        .flat_map(|y| (0..w.map.width() as i32).map(move |x| IVec2::new(x, y)))
        .find(|&t| t.x > 50 && t.y > 50 && deadwood_spot(&w.map, &town, seed, chance, t))
        .expect("a deadwood spot");
    let start = w.items.len();
    let id = w.add_player(spot.as_vec2() + Vec2::new(0.5, 3.0));
    for _ in 0..w.config.chop.deadwood_every_ticks {
        w.step();
    }
    let k = (spot.x, spot.y);
    let log = *w.trees.dead_out.get(&k).expect("a log on the spot");
    assert!(w.items.iter().any(|it| it.id == log && it.pos.floor().as_ivec2() == spot));
    let laid = w.items.len() - start;
    assert!(laid >= 1 && laid < 200, "{laid} logs");
    // A save keeps them, and the world carries on the same.
    let mut copy = World::load(&w.save(), &w.config).expect("loads");
    assert_eq!(copy.state_hash(), w.state_hash());
    // Taken: gone from its spot, and not laid again.
    w.items.retain(|it| it.id != log);
    for _ in 0..w.config.chop.deadwood_every_ticks {
        w.step();
        copy.step();
    }
    assert!(w.trees.dead_gone.contains(&k) && !w.trees.dead_out.contains_key(&k));
    // Far away: the untouched ones are taken back.
    w.players[0].state.pos = Vec2::new(5.0, 5.0);
    let _ = (&mut copy, id);
    for _ in 0..w.config.chop.deadwood_every_ticks {
        w.step();
    }
    assert!(w.trees.dead_out.keys().all(|&(x, y)| Vec2::new(x as f32, y as f32).distance(Vec2::splat(5.0)) < 70.0));
    assert!(w.trees.dead_gone.contains(&k));
}
