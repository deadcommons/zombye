//! Running water, fridges, and the mending and weapon recipes of the loot and crafting pass.

use glam::{IVec2, Vec2};

use crate::inventory::Slot;
use crate::items::{self, ItemStack};
use crate::world::{World, WorldConfig};

const ITEMS: &str = include_str!("../../../assets/data/items/items.ron");
const WEAPONS: &str = include_str!("../../../assets/data/items/weapons.ron");
const RECIPES: &str = include_str!("../../../assets/data/items/recipes.ron");
const CONTAINERS: &str = include_str!("../../../assets/data/containers.ron");
const POWER: &str = include_str!("../../../assets/data/power.ron");

fn world() -> World {
    let weapons = items::parse_weapons(WEAPONS).unwrap();
    let items = items::parse_items(ITEMS, &weapons).unwrap();
    let mut recipes = crate::crafting::parse_recipes_for(RECIPES, &items).unwrap();
    crate::crafting::add_fill_recipes(&mut recipes, &items);
    let containers = crate::containers::parse_containers(CONTAINERS).unwrap();
    let mut c = WorldConfig::new(7, 0, crate::worldgen::tests::templates()).with_weapons(weapons).with_items(items).with_recipes(recipes).with_containers(containers);
    c.power = super::parse_power(POWER).unwrap();
    c.start_hour = 12.0;
    let mut w = World::new(c);
    w.items.clear();
    w
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

/// A free ground-floor spot in the same room as, and next to, a piece of furniture of `kind`.
fn beside(w: &World, kind: &str) -> Vec2 {
    for b in &w.map.buildings {
        for f in b.furniture.iter().filter(|f| f.level == 0 && f.kind == kind) {
            let room = w.map.room_at(f.min.as_vec2() + f.size.as_vec2() * 0.5);
            for y in f.min.y - 1..=f.min.y + f.size.y {
                for x in f.min.x - 1..=f.min.x + f.size.x {
                    let t = IVec2::new(x, y);
                    let at = t.as_vec2() + 0.5;
                    if w.map.room_at(at) == room && !w.map.tile_blocked(t) {
                        return at;
                    }
                }
            }
        }
    }
    panic!("no {kind} with room beside it");
}

#[test]
fn a_running_tap_fills_a_bottle_until_the_water_goes_off() {
    let mut w = world();
    let at = beside(&w, "sink");
    let id = w.add_player(at);
    assert!(w.water_on() && w.water_for(pi(&w, id)));
    assert!(w.give_item(id, "empty_bottle", 2));
    craft(&mut w, id, "fill_empty_bottle").unwrap();
    run(&mut w, 6.0);
    assert_eq!((carried(&w, id, "water_bottle"), carried(&w, id, "empty_bottle")), (1, 1));
    // Away from the tap: no water.
    let p = pi(&w, id);
    w.players[p].state.pos = Vec2::new(5.5, 5.5);
    assert!(craft(&mut w, id, "fill_empty_bottle").unwrap_err().ends_with(super::NEEDS_WATER));
    // The water goes off after a day.
    w.players[p].state.pos = at;
    w.config.power.water.fails_after_hours = Some(24.0);
    assert!(w.water_for(p));
    w.skip_time(25.0);
    assert!(!w.water_on() && !w.water_for(p));
    assert!(craft(&mut w, id, "fill_empty_bottle").unwrap_err().ends_with(super::NEEDS_WATER));
}

#[test]
fn food_keeps_longer_in_a_fridge_with_power() {
    let mut w = world();
    let bread = w.item_index("bread").unwrap();
    let site = w.containers().iter().find(|s| w.container_def(s).id == "fridge" && s.body.is_none()).cloned().expect("a fridge");
    let other = w.containers().iter().find(|s| w.container_def(s).id != "fridge" && s.body.is_none()).cloned().expect("a cupboard");
    let fresh = ItemStack { born: w.game_minute(), ..ItemStack::new(bread, 1) };
    w.opened.insert(site.id, vec![fresh]);
    w.opened.insert(other.id, vec![fresh]);
    let born = |w: &World, c: u32| w.opened[&c][0].born;
    let start = w.game_minute();
    w.skip_time(10.0);
    // Ten hours out of the fridge is ten hours older; in it, two and a half.
    assert_eq!(born(&w, other.id), start);
    let aged = w.game_minute() - born(&w, site.id);
    assert!((140..=160).contains(&aged), "aged {aged} minutes in the fridge");
    // Stepping does the same, a tick at a time.
    let before = born(&w, site.id);
    run(&mut w, 60.0);
    assert!(born(&w, site.id) > before);
    // With the grid down it's just a cupboard.
    w.set_grid(false);
    let (b, m) = (born(&w, site.id), w.game_minute());
    w.skip_time(4.0);
    assert_eq!(born(&w, site.id), b);
    assert!(w.game_minute() - m >= 239);
}

#[test]
fn tape_mends_a_worn_bat_and_nails_make_it_worse_for_them() {
    let mut w = world();
    let id = w.add_player(Vec2::new(5.5, 5.5));
    let p = pi(&w, id);
    assert!(w.give_item(id, "duct_tape", 1));
    assert!(craft(&mut w, id, "tape_wooden_weapon").unwrap_err().ends_with(crate::crafting::NOTHING_TO_MEND));
    assert!(w.hold_item(id, Slot::Hand, "baseball_bat"));
    w.players[p].inventory.hand.as_mut().unwrap().used = 7;
    craft(&mut w, id, "tape_wooden_weapon").unwrap();
    run(&mut w, 12.0);
    assert_eq!(w.players[p].inventory.hand.unwrap().used, 4);
    assert_eq!(carried(&w, id, "duct_tape"), 0);
    // A whole bat, six nails and a hammer: a nailed bat.
    let mut w = world();
    let id = w.add_player(Vec2::new(5.5, 5.5));
    assert!(w.give_item(id, "baseball_bat", 1) && w.give_item(id, "nail", 6));
    assert!(w.hold_item(id, Slot::Hand, "claw_hammer"));
    craft(&mut w, id, "make_nailed_bat").unwrap();
    run(&mut w, 25.0);
    assert_eq!((carried(&w, id, "nailed_bat"), carried(&w, id, "baseball_bat"), carried(&w, id, "nail")), (1, 0, 0));
}
