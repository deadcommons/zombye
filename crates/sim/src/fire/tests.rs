//! Fires and cooking: building a campfire, fuel, lighting and putting out, burning out, standing in
//! one, cooking only over heat, raw food, firelight, saves.

use glam::Vec2;

use super::*;
use crate::health::WoundKind;
use crate::inventory::Slot;
use crate::items;
use crate::world::WorldConfig;

const ITEMS: &str = include_str!("../../../../assets/data/items/items.ron");
const WEAPONS: &str = include_str!("../../../../assets/data/items/weapons.ron");
const RECIPES: &str = include_str!("../../../../assets/data/items/recipes.ron");
const FIRE: &str = include_str!("../../../../assets/data/fire.ron");

fn config() -> WorldConfig {
    let weapons = items::parse_weapons(WEAPONS).unwrap();
    let items = items::parse_items(ITEMS, &weapons).unwrap();
    let recipes = crate::crafting::parse_recipes_for(RECIPES, &items).unwrap();
    let mut config = WorldConfig::new(5, 0, Vec::new()).with_weapons(weapons).with_items(items).with_recipes(recipes).with_fire(parse_fire(FIRE).unwrap());
    config.start_hour = 12.0;
    config
}

/// An empty field at noon and a player in the middle of it, facing east.
fn field() -> (World, u32) {
    let mut w = World::new(config());
    w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    let id = w.add_player(Vec2::new(100.5, 100.5));
    let pi = pi(&w, id);
    w.players[pi].state.facing = 0.0;
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

fn carried(w: &World, id: u32, item: &str) -> u32 {
    let ix = w.item_index(item).unwrap();
    w.players[pi(w, id)].inventory.carried.iter().filter(|s| s.item == ix).map(|s| s.count as u32).sum()
}

fn craft(w: &mut World, id: u32, recipe: &str) -> Result<(), String> {
    let r = w.recipe_index(recipe).unwrap();
    w.player_craft(id, r)
}

#[test]
fn the_shipped_file_is_the_defaults() {
    assert_eq!(parse_fire(FIRE).unwrap(), FireTuning::default());
    FireTuning::default().check().unwrap();
}

#[test]
fn a_campfire_is_built_from_firewood_and_lit_with_matches() {
    let (mut w, id) = field();
    assert!(w.give_item(id, "firewood", 2));
    assert!(craft(&mut w, id, "build_campfire").is_err(), "nothing to light it with");
    assert!(w.give_item(id, "matches", 1));
    craft(&mut w, id, "build_campfire").unwrap();
    run(&mut w, 30.0);
    assert_eq!(w.fires().len(), 1, "one campfire");
    let f = w.fires()[0];
    assert!(f.lit, "it burns");
    let want = 2.0 * 1.5 * w.config.fire.hours_per_kg;
    // (It has burned a little since: a game hour is 150 s.)
    assert!(f.fuel_h <= want && f.fuel_h > want - 0.3, "fuel {} against {want}", f.fuel_h);
    assert!(f.pos.distance(Vec2::new(101.5, 100.5)) < 0.1, "a metre in front: {:?}", f.pos);
    assert_eq!(carried(&w, id, "firewood"), 0, "the firewood went into it");
    assert_eq!(carried(&w, id, "matches"), 1, "the matches are kept");
}

#[test]
fn no_campfire_indoors_or_on_top_of_another() {
    let (mut w, id) = field();
    w.map.rooms.push(crate::map::Room { building: 0, name: "Kitchen".into(), room_type: "kitchen".into(), floor: crate::map::Floor::Wood });
    w.map.tile_mut(glam::IVec2::new(101, 100)).unwrap().room = 1;
    let pi = pi(&w, id);
    assert!(w.campfire_spot(pi).unwrap_err().contains("indoors"));
    w.map.tile_mut(glam::IVec2::new(101, 100)).unwrap().room = 0;
    w.add_fire(Vec2::new(102.0, 100.5), 1.0, false);
    assert!(w.campfire_spot(pi).unwrap_err().contains("already"));
}

#[test]
fn a_fire_burns_its_fuel_and_goes_out() {
    let (mut w, _) = field();
    let f = w.add_fire(Vec2::new(110.5, 100.5), 0.5, true);
    w.skip_time(0.3);
    assert!(w.fires.get(f).unwrap().lit);
    w.skip_time(0.3);
    let fire = w.fires.get(f).unwrap();
    assert!(!fire.lit && fire.fuel_h <= 0.0, "out of fuel: {fire:?}");
}

#[test]
fn fuel_light_and_put_out() {
    let (mut w, id) = field();
    let f = w.add_fire(Vec2::new(101.5, 100.5), 0.0, false);
    assert!(w.player_fire(id, f, FireOp::Light).unwrap_err().contains("fuel"));
    assert!(w.give_item(id, "firewood", 1));
    let index = w.players[pi(&w, id)].inventory.carried.iter().position(|s| s.item == w.item_index("firewood").unwrap()).unwrap() as u32;
    w.player_fire(id, f, FireOp::AddFuel { index }).unwrap();
    assert_eq!(carried(&w, id, "firewood"), 0);
    assert!(w.fires.get(f).unwrap().fuel_h > 1.0);
    assert!(w.player_fire(id, f, FireOp::Light).unwrap_err().contains("light"), "nothing to light it with");
    assert!(w.give_item(id, "matches", 1));
    w.player_fire(id, f, FireOp::Light).unwrap();
    assert!(w.fires.get(f).unwrap().lit);
    w.player_fire(id, f, FireOp::PutOut).unwrap();
    let fire = w.fires.get(f).unwrap();
    assert!(!fire.lit && fire.fuel_h > 1.0, "out, fuel kept");
    // Out of reach.
    let far = w.add_fire(Vec2::new(120.5, 100.5), 1.0, true);
    assert!(w.player_fire(id, far, FireOp::PutOut).unwrap_err().contains("reach"));
}

#[test]
fn standing_in_a_fire_burns_your_feet() {
    let (mut w, id) = field();
    let pos = w.players[pi(&w, id)].state.pos;
    w.add_fire(pos, 2.0, true);
    let hp = w.players[pi(&w, id)].hp;
    run(&mut w, 4.0);
    let p = &w.players[pi(&w, id)];
    assert!(p.health.wounds.iter().any(|x| x.kind == WoundKind::Burn), "burned: {:?}", p.health.wounds);
    assert!(p.health.wounds.iter().filter(|x| x.kind == WoundKind::Burn).all(|x| matches!(x.part, crate::health::Part::LeftLeg | crate::health::Part::RightLeg | crate::health::Part::LeftFoot | crate::health::Part::RightFoot)));
    assert!(p.hp < hp);
}

#[test]
fn cooking_needs_heat_and_cooks_the_food() {
    let (mut w, id) = field();
    assert!(w.give_item(id, "raw_chicken", 1));
    assert!(w.hold_item(id, Slot::OffHand, "frying_pan"));
    assert!(craft(&mut w, id, "cook_chicken").unwrap_err().ends_with(crate::crafting::NEEDS_HEAT));
    let pi = pi(&w, id);
    assert!(!w.heat_for(pi));
    let f = w.add_fire(Vec2::new(101.5, 100.5), 2.0, false);
    assert!(!w.heat_for(pi), "an unlit fire is no heat");
    w.fires.list.iter_mut().find(|x| x.id == f).unwrap().lit = true;
    assert!(w.heat_for(pi));
    craft(&mut w, id, "cook_chicken").unwrap();
    run(&mut w, 120.0);
    assert_eq!(carried(&w, id, "raw_chicken"), 0);
    assert_eq!(carried(&w, id, "cooked_chicken"), 1);
}

#[test]
fn heat_near_goes_by_reach_and_walls() {
    let t = FireTuning::default();
    let map = TileMap::new(8, 8);
    let fires = [(Vec2::new(10.0, 10.0), true)];
    assert!(heat_near(&map, &t, fires, Vec2::new(11.0, 10.0), 0, |_| true));
    assert!(!heat_near(&map, &t, fires, Vec2::new(13.0, 10.0), 0, |_| true), "too far");
    assert!(!heat_near(&map, &t, fires, Vec2::new(11.0, 10.0), 1, |_| true), "upstairs");
}

#[test]
fn raw_food_can_make_you_ill() {
    let (mut w, id) = field();
    let ix = w.item_index("raw_chicken").unwrap() as usize;
    assert!(w.config.items[ix].food.as_ref().unwrap().poison > 0.0);
    let cooked = w.item_index("cooked_chicken").unwrap() as usize;
    assert_eq!(w.config.items[cooked].food.as_ref().unwrap().poison, 0.0);
    // Sure to: every go poisons.
    w.config.items[ix].food.as_mut().unwrap().poison = 1.0;
    assert!(w.give_item(id, "raw_chicken", 1));
    let index = w.players[pi(&w, id)].inventory.carried.iter().position(|s| s.item == ix as u16).unwrap() as u32;
    w.player_use(id, crate::inventory::Place::Carried, index).unwrap();
    run(&mut w, 20.0);
    assert!(w.players[pi(&w, id)].health.food_poisoning_h > 0.0, "poisoned");
}

#[test]
fn firelight_shows_you_at_night() {
    let (mut w, _) = field();
    let t = w.config.fire.clone();
    assert_eq!(firelight(&w.fires, &t, Vec2::new(105.0, 100.0)), 0.0);
    w.add_fire(Vec2::new(105.0, 100.0), 2.0, true);
    assert!(firelight(&w.fires, &t, Vec2::new(106.0, 100.0)) > 0.3);
    assert_eq!(firelight(&w.fires, &t, Vec2::new(105.0 + t.light_radius + 1.0, 100.0)), 0.0);
}

#[test]
fn fires_are_saved_and_hashed() {
    let (mut w, _) = field();
    let before = w.state_hash();
    w.add_fire(Vec2::new(105.5, 100.5), 2.0, true);
    w.add_fire(Vec2::new(110.5, 100.5), 1.0, false);
    assert_ne!(w.state_hash(), before);
    let bytes = w.save();
    let l = World::load(&bytes, &w.config).unwrap();
    assert_eq!(l.fires(), w.fires());
    assert_eq!(l.state_hash(), w.state_hash());
}
