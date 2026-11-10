//! Wear: the data, knocks on hits only, breaking, a battered weapon's damage, practised hands,
//! found weapons, cramped blows, one-handed swings, saves and determinism.

use glam::Vec2;

use super::*;
use crate::inventory::Slot;
use crate::items;
use crate::map::TileMap;
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;
use crate::{PlayerInput, Skill};

const WEAPONS: &str = include_str!("../../../../assets/data/items/weapons.ron");
const ITEMS: &str = include_str!("../../../../assets/data/items/items.ron");
const ORIGIN: Vec2 = Vec2::new(100.5, 100.5);

fn config(seed: u64) -> WorldConfig {
    let templates = vec![BuildingTemplate::from_ron(include_str!("../../../../assets/data/buildings/bungalow.ron")).unwrap()];
    let weapons = items::parse_weapons(WEAPONS).unwrap();
    let items = items::parse_items(ITEMS, &weapons).unwrap();
    let mut config = WorldConfig::new(seed, 0, templates).with_weapons(weapons).with_items(items);
    config.start_hour = 12.0;
    config
}

/// An empty field with one player at `ORIGIN` facing east, holding `weapon`, ignored by zombies.
fn field(config: WorldConfig, weapon: &str) -> (World, u32) {
    let mut w = World::new(config);
    w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    w.spawn_building = 0;
    let id = w.add_player(ORIGIN);
    assert!(w.hold_item(id, Slot::Hand, weapon));
    w.set_dev(id, None, Some(true));
    (w, id)
}

/// A zombie that doesn't die or walk, `east` metres east of the player.
fn dummy(w: &mut World, east: f32) -> usize {
    let i = w.add_zombie(ORIGIN + Vec2::new(east, 0.0), std::f32::consts::PI, 0.0);
    w.zombies.wander_timer[i] = 1e9;
    w.zombies.hp[i] = 1e5;
    i
}

/// One swing, stepped until the player can act again, rested after. The zombie, if there is
/// one, is first stood back up `east` of the player.
fn swing(w: &mut World, id: u32, east: f32) {
    if !w.zombies.is_empty() {
        w.zombies.state[0] = crate::zombies::ZombieState::Wander;
        w.zombies.pos[0] = ORIGIN + Vec2::new(east, 0.0);
        w.zombies.vel[0] = Vec2::ZERO;
    }
    let seq = w.tick as u32 + 1;
    w.set_player_input(id, seq, PlayerInput { facing: 0.0, aim: true, ..Default::default() });
    assert!(w.player_attack(id));
    for _ in 0..200 {
        w.step();
        let p = w.player(id).unwrap();
        if p.action.is_none() && p.ready_in <= 0.0 {
            break;
        }
    }
    let pi = w.players.iter().position(|p| p.id == id).unwrap();
    w.players[pi].state.endurance = 1.0;
    w.players[pi].state.winded = false;
}

fn hand(w: &World, id: u32) -> Option<ItemStack> {
    w.player(id).unwrap().inventory.hand
}

#[test]
fn shipped_weapons_wear_and_bare_hands_and_guns_dont() {
    let c = config(1);
    for w in &c.weapons {
        let wears = w.ranged.is_none() && w.category != crate::WeaponCategory::Unarmed;
        assert_eq!(w.durability > 0, wears, "{}", w.id);
        assert_eq!(w.wear_chance > 0.0, wears, "{}", w.id);
    }
    let bad = |edit: &dyn Fn(&mut WeaponDef), want: &str| {
        let mut ws = c.weapons.clone();
        edit(ws.iter_mut().find(|w| w.id == "baseball_bat").unwrap());
        let e = items::validate_weapons(&ws).unwrap_err();
        assert!(e.contains(want), "{e}");
    };
    bad(&|w| w.wear_chance = 0.0, "go together");
    bad(&|w| w.durability = 0, "go together");
    bad(&|w| w.wear_chance = 1.5, "wear_chance");
    bad(&|w| w.close_range = w.range, "close_range");
    let mut ws = c.weapons.clone();
    let gun = ws.iter_mut().find(|w| w.id == "pistol").unwrap();
    (gun.durability, gun.wear_chance) = (5, 0.1);
    assert!(items::validate_weapons(&ws).unwrap_err().contains("don't wear"));
}

#[test]
fn a_weapon_wears_only_on_hits_and_breaks_at_its_last_knock() {
    let mut c = config(3);
    c.weapons.iter_mut().find(|w| w.id == "wooden_plank").unwrap().wear_chance = 1.0;
    let of = c.weapons.iter().find(|w| w.id == "wooden_plank").unwrap().durability;
    let (mut w, id) = field(c, "wooden_plank");
    // Swings at the air cost it nothing.
    for _ in 0..3 {
        swing(&mut w, id, 1.2);
    }
    assert_eq!(hand(&w, id).unwrap().used, 0);
    dummy(&mut w, 1.2);
    for k in 1..of {
        swing(&mut w, id, 1.2);
        assert_eq!(hand(&w, id).unwrap().used, k, "a knock a hit at wear_chance 1");
    }
    assert_eq!(w.condition_of(&hand(&w, id).unwrap()).unwrap().2, Condition::Battered);
    w.item_notes.clear();
    swing(&mut w, id, 1.2);
    assert!(hand(&w, id).is_none(), "the last knock breaks it");
    assert_eq!(w.player(id).unwrap().weapon, None, "bare hands now");
    assert!(w.item_notes.iter().any(|n| n.player == id && n.text.contains("plank broke")), "{:?}", w.item_notes);
}

#[test]
fn a_battered_weapon_hits_weaker() {
    let mean = |used: u8| {
        // (So little chance of a knock that it never breaks here.)
        let mut c = config(5);
        c.weapons.iter_mut().find(|w| w.id == "crowbar").unwrap().wear_chance = 1e-9;
        let (mut w, id) = field(c, "crowbar");
        let pi = 0;
        w.players[pi].inventory.hand.as_mut().unwrap().used = used;
        let z = dummy(&mut w, 1.0);
        let before = w.zombies.hp[z];
        for _ in 0..40 {
            swing(&mut w, id, 1.0);
            // Keep it as worn as it was.
            w.players[pi].inventory.hand.as_mut().unwrap().used = used;
        }
        (before - w.zombies.hp[z]) / 40.0
    };
    let c = config(5);
    let of = c.weapons.iter().find(|w| w.id == "crowbar").unwrap().durability;
    let (sound, battered) = (mean(0), mean(of - 1));
    let ratio = battered / sound;
    assert!((ratio - c.combat.battered_damage).abs() < 0.08, "sound {sound}, battered {battered}");
}

#[test]
fn practised_hands_wear_a_weapon_slower() {
    let knocks = |level: u32| {
        let mut c = config(7);
        let bat = c.weapons.iter_mut().find(|w| w.id == "baseball_bat").unwrap();
        (bat.durability, bat.wear_chance) = (250, 0.5);
        let (mut w, id) = field(c, "baseball_bat");
        let xp = if level == 0 { 0.0 } else { w.config.skills.levels[level as usize - 1] };
        w.players[0].skills.xp[Skill::Bludgeon as usize] = xp;
        dummy(&mut w, 1.2);
        for _ in 0..120 {
            swing(&mut w, id, 1.2);
            w.players[0].skills.xp[Skill::Bludgeon as usize] = xp;
        }
        hand(&w, id).unwrap().used as f32
    };
    let (new, old) = (knocks(0), knocks(5));
    assert!((new / 120.0 - 0.5).abs() < 0.12, "{new} knocks in 120 hits at 0.5");
    assert!(old < new * 0.8, "level 5: {old} against {new}");
}

#[test]
fn found_weapons_have_seen_some_use() {
    let w = World::new(config(11));
    let bat = w.config.items.iter().position(|d| d.id == "baseball_bat").unwrap() as u16;
    let tin = w.config.items.iter().position(|d| d.category == crate::ItemCategory::Food).unwrap() as u16;
    let of = durability(&w.config.items, &w.config.weapons, bat);
    let mut worn = 0;
    for c in 0..200 {
        let mut stacks = [ItemStack::one(bat), ItemStack::one(tin)];
        w.wear_found(c, &mut stacks);
        let mut again = [ItemStack::one(bat), ItemStack::one(tin)];
        w.wear_found(c, &mut again);
        assert_eq!(stacks, again, "the same whenever it's found");
        assert!(stacks[0].used < of, "never found broken");
        assert_eq!(stacks[1].used, 0, "only weapons");
        worn += (stacks[0].used > 0) as u32;
    }
    assert!((40..190).contains(&worn), "{worn} of 200 bats found worn");
}

#[test]
fn a_blow_too_close_is_cramped() {
    let mean = |east: f32| {
        let mut c = config(13);
        c.weapons.iter_mut().find(|w| w.id == "baseball_bat").unwrap().wear_chance = 1e-9;
        let (mut w, id) = field(c, "baseball_bat");
        let z = dummy(&mut w, east);
        let before = w.zombies.hp[z];
        for _ in 0..40 {
            swing(&mut w, id, east);
        }
        (before - w.zombies.hp[z]) / 40.0
    };
    let c = config(13);
    // The bat's close_range is to the body's edge: 0.65 m off its middle is inside it, 1.2 m isn't.
    let (close, far) = (mean(0.65), mean(1.2));
    assert!((close / far - c.combat.cramped_damage).abs() < 0.08, "close {close}, far {far}");
}

#[test]
fn some_two_handed_weapons_swing_in_one_hand_weaker_and_slower() {
    let (mut w, id) = field(config(17), "baseball_bat");
    assert!(w.hold_item(id, Slot::OffHand, "flashlight"));
    let inv = &w.player(id).unwrap().inventory;
    assert!(inv.hand.is_some() && inv.off_hand.is_some(), "a bat leaves a hand for the flashlight");
    let c = w.config.combat.clone();
    w.set_player_input(id, 1, PlayerInput { facing: 0.0, aim: true, ..Default::default() });
    assert!(w.player_attack(id));
    let (one_power, one_ready) = (w.player(id).unwrap().action.unwrap().power, w.player(id).unwrap().ready_in);

    let (mut w, id) = field(config(17), "baseball_bat");
    w.set_player_input(id, 1, PlayerInput { facing: 0.0, aim: true, ..Default::default() });
    assert!(w.player_attack(id));
    let (two_power, two_ready) = (w.player(id).unwrap().action.unwrap().power, w.player(id).unwrap().ready_in);
    assert!((one_power / two_power - c.one_hand_power).abs() < 1e-4);
    assert!((one_ready / two_ready - c.one_hand_slow).abs() < 1e-4);

    // An axe takes both hands whatever the other holds.
    let (mut w, id) = field(config(17), "flashlight");
    let axe = w.config.items.iter().position(|d| d.id == "fire_axe").unwrap() as u16;
    w.players[0].inventory.off_hand = w.players[0].inventory.hand.take();
    w.players[0].inventory.carried.push(ItemStack::one(axe));
    w.player_equip(id, 0).unwrap();
    for _ in 0..40 {
        w.step();
    }
    let inv = &w.player(id).unwrap().inventory;
    assert_eq!(inv.hand.map(|s| s.item), Some(axe));
    assert!(inv.off_hand.is_none(), "the flashlight went back in the bags");
}

#[test]
fn wear_is_saved_checked_and_deterministic() {
    let run = || {
        let mut c = config(19);
        c.weapons.iter_mut().find(|w| w.id == "golf_club").unwrap().wear_chance = 0.5;
        let (mut w, id) = field(c, "golf_club");
        dummy(&mut w, 1.2);
        for _ in 0..4 {
            swing(&mut w, id, 1.2);
        }
        // (The dev flag isn't saved, and is in the hash while on.)
        w.set_dev(id, None, Some(false));
        (w, id)
    };
    let (a, id) = run();
    let (b, _) = run();
    let used = hand(&a, id).unwrap().used;
    assert!(used > 0, "four hits at 0.5 left a knock");
    assert_eq!(a.state_hash(), b.state_hash());
    let loaded = World::load(&a.save(), &a.config).unwrap();
    assert_eq!(loaded.state_hash(), a.state_hash());
    assert_eq!(hand(&loaded, id).unwrap().used, used);
    // More knocks than it can take is not a save a game made.
    let (mut bad, _) = run();
    let of = durability(&bad.config.items, &bad.config.weapons, hand(&bad, id).unwrap().item);
    bad.players[0].inventory.hand.as_mut().unwrap().used = of;
    assert!(World::load(&bad.save(), &bad.config).is_err());
}
