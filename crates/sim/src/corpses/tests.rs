//! What zombies wear, and searching a dead one's body: outfits from data, the body as a
//! container, taking its clothes off it, nothing put on it, it going when the corpse is cleared
//! away, saves, and a turned player's clothes found only on its body.

use super::*;
use crate::inventory::{CONTAINER_REACH, Place};
use crate::items::{parse_items, parse_weapons};
use crate::map::{CHUNK_SIZE, TileMap};
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;
use crate::{TICK_DT, ZombieAnim};

fn shipped_outfits() -> Vec<Outfit> {
    crate::population::parse_population(include_str!("../../../../assets/data/population.ron")).unwrap().outfits
}

fn config(seed: u64) -> WorldConfig {
    let templates = vec![BuildingTemplate::from_ron(include_str!("../../../../assets/data/buildings/bungalow.ron")).unwrap()];
    let weapons = parse_weapons(include_str!("../../../../assets/data/items/weapons.ron")).unwrap();
    let items = parse_items(include_str!("../../../../assets/data/items/items.ron"), &weapons).unwrap();
    let containers = crate::containers::parse_containers(include_str!("../../../../assets/data/containers.ron")).unwrap();
    let mut population = crate::population::PopulationDef::default();
    population.outfits = shipped_outfits();
    let mut c = WorldConfig::new(seed, 0, templates)
        .with_weapons(weapons)
        .with_items(items)
        .with_containers(containers)
        .with_loot(crate::loot::tests::shipped())
        .with_population(population)
        .with_map_chunks(8);
    c.zombies = Some(0);
    c.start_hour = 12.0;
    c.needs = crate::needs::NeedsTuning::off();
    c
}

/// Open grass, a player at (60.5, 60.5) facing east and a dead zombie lying 1 m east of it.
fn field() -> (World, u32, u32) {
    let mut w = World::new(config(5));
    w.map = TileMap::new(w.map.width() / CHUNK_SIZE, w.map.height() / CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    w.sites.clear();
    let id = w.add_player(Vec2::new(60.5, 60.5));
    let zi = w.add_zombie(Vec2::new(61.5, 60.5), 0.0, 1.0);
    kill(&mut w, zi);
    let zid = w.zombies.id[zi];
    (w, id, zid)
}

/// A real town (a save needs one): the player where it wakes up, a dead zombie at its feet.
fn town() -> (World, u32, u32) {
    let mut w = World::new(config(5));
    let id = w.add_player(w.player_spawn);
    let zi = w.add_zombie(w.player_spawn + Vec2::new(0.1, 0.0), 0.0, 1.0);
    kill(&mut w, zi);
    let zid = w.zombies.id[zi];
    (w, id, zid)
}

fn kill(w: &mut World, zi: usize) {
    w.zombies.state[zi] = ZombieState::Dead;
    w.zombies.anim[zi] = ZombieAnim::Dead;
    w.zombies.hp[zi] = 0.0;
    w.zombies.timer[zi] = 0.0;
}

fn settle(w: &mut World, id: u32) {
    for _ in 0..400 {
        if w.player(id).unwrap().jobs.is_empty() {
            return;
        }
        w.step();
    }
    panic!("jobs never finished");
}

fn names(w: &World, look: &[u16]) -> Vec<String> {
    look.iter().map(|&i| w.config.items[i as usize].id.clone()).collect()
}

#[test]
fn the_shipped_outfits_are_clothing_that_goes_together() {
    let weapons = parse_weapons(include_str!("../../../../assets/data/items/weapons.ron")).unwrap();
    let items = parse_items(include_str!("../../../../assets/data/items/items.ron"), &weapons).unwrap();
    let outfits = shipped_outfits();
    assert!(outfits.len() >= 4);
    check_outfits(&outfits, &items).unwrap();
}

#[test]
fn outfits_are_checked() {
    let o = |wears: &[&str], weight: f32| Outfit { wears: wears.iter().map(|s| s.to_string()).collect(), weight };
    assert!(validate_outfits(&[o(&["jeans", "jeans"], 1.0)]).unwrap_err().contains("twice"));
    assert!(validate_outfits(&[o(&["jeans"], -1.0)]).unwrap_err().contains("weight"));
    assert!(validate_outfits(&[o(&["jeans"], 0.0)]).unwrap_err().contains("above 0"));
    assert!(validate_outfits(&[o(&["Jeans!"], 1.0)]).is_err());
    validate_outfits(&[]).unwrap();
    let weapons = parse_weapons(include_str!("../../../../assets/data/items/weapons.ron")).unwrap();
    let items = parse_items(include_str!("../../../../assets/data/items/items.ron"), &weapons).unwrap();
    assert!(check_outfits(&[o(&["tuxedo"], 1.0)], &items).unwrap_err().contains("tuxedo"));
    assert!(check_outfits(&[o(&["apple"], 1.0)], &items).unwrap_err().contains("isn't clothing"));
    assert!(check_outfits(&[o(&["trainers", "work_boots"], 1.0)], &items).unwrap_err().contains("together"));
    assert!(crate::population::parse_population("(outfits: [(wears: [\"jeans\"], weight: 1.0, colour: 3)])").is_err(), "unknown fields");
}

#[test]
fn every_zombie_wears_an_outfit_picked_by_weight_from_its_id() {
    let outfits = shipped_outfits();
    let mut count = vec![0u32; outfits.len()];
    for id in 0..6000 {
        let k = outfit_index(11, id, &outfits).unwrap();
        assert_eq!(outfit_index(11, id, &outfits), Some(k), "the same every time");
        count[k] += 1;
    }
    let total: f32 = outfits.iter().map(|o| o.weight).sum();
    for (k, o) in outfits.iter().enumerate() {
        let want = 6000.0 * o.weight / total;
        assert!((count[k] as f32 - want).abs() < want * 0.2 + 30.0, "outfit {k}: {} picks, about {want} expected", count[k]);
    }
    assert_ne!((0..50).map(|id| outfit_index(1, id, &outfits)).collect::<Vec<_>>(), (0..50).map(|id| outfit_index(2, id, &outfits)).collect::<Vec<_>>(), "the seed matters");
    assert_eq!(outfit_index(1, 3, &[]), None, "no outfits: nothing on");
}

#[test]
fn a_dead_zombies_body_holds_what_it_wore_and_taking_it_off_shows() {
    let (mut w, id, zid) = field();
    let wore = w.zombie_look(zid);
    assert!(!wore.is_empty(), "it wears its outfit");
    assert_eq!(wore, default_look(w.config.seed(), zid, &w.config.population.outfits, &w.config.items), "the one a client works out");
    assert!(w.zombie_look_is_default(zid));
    let c = corpse_container(zid);
    assert_eq!(w.containers_in_reach(id), vec![c], "the body is in reach");
    assert_eq!(w.container_in_reach(id), Some(c));
    // E: the body it faces is searched.
    w.player_search(id, None).unwrap();
    settle(&mut w, id);
    assert_eq!(w.player(id).unwrap().open, Some(c), "open to the searcher");
    let held = w.container_contents(c).unwrap().to_vec();
    for i in &wore {
        assert!(held.iter().any(|s| s.item == *i), "{} is on the body", w.config.items[*i as usize].id);
    }
    assert!(!w.zombie_look_is_default(zid), "snapshots send its look from now on");
    assert_eq!(w.zombie_look(zid), wore, "it still wears it all");
    // Take the first thing it wears: it isn't on the body any more.
    let k = held.iter().position(|s| s.item == wore[0]).unwrap();
    w.player_move_item(id, Place::Container(c), k as u32, Place::Carried, 0).unwrap();
    settle(&mut w, id);
    let me = w.player(id).unwrap();
    assert!(me.inventory.carried.iter().any(|s| s.item == wore[0]), "{} taken: {:?}", names(&w, &wore[..1])[0], w.item_notes);
    assert_eq!(w.zombie_look(zid), wore[1..], "and off the body you see");
    // Nothing goes on a body.
    let carried = me.inventory.carried.len() - 1;
    w.player_move_item(id, Place::Carried, carried as u32, Place::Container(c), 0).unwrap_or(());
    settle(&mut w, id);
    assert!(w.item_notes.iter().any(|n| n.player == id && !n.ok && n.text.contains("on a body")), "{:?}", w.item_notes);
    assert!(!w.container_contents(c).unwrap().iter().any(|s| s.item == wore[0]));
}

#[test]
fn only_a_dead_zombie_close_by_is_a_body_to_search() {
    let (mut w, id, zid) = field();
    let alive = w.add_zombie(Vec2::new(60.5, 61.5), 0.0, 1.0);
    let alive_id = w.zombies.id[alive];
    w.zombies.wander_timer[alive] = 1e9;
    assert_eq!(w.containers_in_reach(id), vec![corpse_container(zid)], "a standing zombie isn't a container");
    assert!(w.container(corpse_container(alive_id)).is_none());
    // Too far: out of reach.
    let zi = w.zombies.index_of(zid).unwrap();
    w.zombies.pos[zi] = Vec2::new(60.5 + CONTAINER_REACH + BODY_RADIUS + 0.5, 60.5);
    assert!(w.containers_in_reach(id).is_empty());
    assert!(w.player_search(id, Some(corpse_container(zid))).is_err() || w.item_notes.iter().any(|n| !n.ok && n.text.contains("out of reach")));
    // With no "corpse" kind in the data, bodies are nothing to search.
    let (mut w, id, zid) = field();
    w.config.containers.containers.retain(|c| c.id != CORPSE_KIND);
    assert!(w.containers_in_reach(id).is_empty() && w.container(corpse_container(zid)).is_none());
}

#[test]
fn a_cleared_away_corpse_takes_what_was_left_on_it() {
    let (mut w, id, zid) = town();
    w.player_search(id, None).unwrap();
    settle(&mut w, id);
    let c = corpse_container(zid);
    assert!(w.container_contents(c).is_some());
    let zi = w.zombies.index_of(zid).unwrap();
    w.zombies.timer[zi] = w.config.zombie.corpse_secs + 1.0;
    w.step();
    w.step();
    assert!(w.zombies.index_of(zid).is_none(), "cleared away");
    assert!(w.container_contents(c).is_none(), "with what was on it");
    assert_eq!(w.player(id).unwrap().open, None, "the searcher has nothing open");
    // A save after that loads (no opened container that isn't one).
    let back = World::load(&w.save(), &w.config).unwrap();
    assert_eq!(back.state_hash(), w.state_hash());
}

#[test]
fn a_searched_body_survives_a_save() {
    let (mut w, id, zid) = town();
    w.player_search(id, None).unwrap();
    settle(&mut w, id);
    let c = corpse_container(zid);
    w.player_move_item(id, Place::Container(c), 0, Place::Carried, 0).unwrap();
    settle(&mut w, id);
    let back = World::load(&w.save(), &w.config).unwrap();
    assert_eq!(back.state_hash(), w.state_hash());
    assert_eq!(back.container_contents(c), w.container_contents(c));
    assert_eq!(back.zombie_look(zid), w.zombie_look(zid));
    assert_eq!(back.player(id).unwrap().open, Some(c), "still open to the searcher");
}

#[test]
fn a_turned_players_clothes_are_on_its_body_not_the_floor() {
    let (mut w, _, _) = field();
    let id = w.add_player(Vec2::new(70.5, 70.5));
    let pi = w.players.iter().position(|p| p.id == id).unwrap();
    for item in ["hoodie", "jeans"] {
        let i = w.item_index(item).unwrap();
        w.players[pi].inventory.worn.push(crate::ItemStack::one(i));
    }
    w.inflict_wound(id, crate::health::Part::LeftArm, crate::health::WoundKind::Bite, Some(true)).unwrap();
    w.players[pi].health.infection.as_mut().unwrap().stage = crate::health::InfectionStage::Turning;
    w.players[pi].hp = 1.0;
    while !w.player(id).unwrap().dead {
        w.skip_time(0.01);
    }
    let n = w.zombies.len();
    for _ in 0..(31.0 / TICK_DT) as u32 {
        w.step();
    }
    assert_eq!(w.zombies.len(), n + 1, "the body rose");
    let zid = w.zombies.id[n];
    assert_eq!(names(&w, &w.zombie_look(zid)), ["hoodie", "jeans"], "in what it wore, not an outfit");
    let hoodie = w.item_index("hoodie").unwrap();
    assert!(w.items.iter().all(|it| it.stack.item != hoodie), "not on the floor as well");
    // Put down and searched, its body holds them.
    kill(&mut w, n);
    let looter = w.add_player(w.zombies.pos[n] - Vec2::new(0.9, 0.0));
    w.player_search(looter, Some(corpse_container(zid))).unwrap();
    settle(&mut w, looter);
    assert!(w.container_contents(corpse_container(zid)).unwrap().iter().any(|s| s.item == hoodie), "{:?}", w.item_notes);
}
