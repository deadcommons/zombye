//! Hold-ups (#73): who can be tied and how it is called off, what tied hands stop, struggling
//! out, freeing, searching a tied survivor and taking from it, saves and determinism.

use super::*;
use crate::inventory::Slot;
use crate::items::{parse_items, parse_weapons};
use crate::map::{CHUNK_SIZE, TileMap};
use crate::players::{Knock, PlayerInput};
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;
use crate::{TICK_RATE, players};

fn config(pvp: bool) -> WorldConfig {
    let templates = vec![BuildingTemplate::from_ron(include_str!("../../../../assets/data/buildings/bungalow.ron")).unwrap()];
    let weapons = parse_weapons(include_str!("../../../../assets/data/items/weapons.ron")).unwrap();
    let items = parse_items(include_str!("../../../../assets/data/items/items.ron"), &weapons).unwrap();
    let mut c = WorldConfig::new(7, 0, templates).with_weapons(weapons).with_items(items).with_map_chunks(10).with_pvp(pvp);
    c.start_hour = 12.0;
    c.needs = crate::needs::NeedsTuning::off();
    c
}

/// An empty field at noon: a captor at (60.5, 60.5) facing east and another survivor 1 m east.
fn field(pvp: bool) -> (World, u32, u32) {
    let mut w = World::new(config(pvp));
    w.map = TileMap::new(w.map.width() / CHUNK_SIZE, w.map.height() / CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    w.sites.clear();
    w.spawn_building = 0;
    let a = w.add_player(Vec2::new(60.5, 60.5));
    let b = w.add_player(Vec2::new(61.5, 60.5));
    (w, a, b)
}

fn t() -> BondTuning {
    BondTuning::default()
}

fn secs(w: &mut World, s: f32) {
    for _ in 0..(s * TICK_RATE as f32).round() as u32 {
        w.step();
    }
}

fn tied(w: &World, id: u32) -> bool {
    w.player(id).unwrap().restraint.is_some()
}

fn last_note(w: &World, id: u32) -> String {
    w.item_notes.iter().rev().find(|n| n.player == id).map(|n| n.text.clone()).unwrap_or_default()
}

fn count(w: &World, id: u32, item: &str) -> u16 {
    let i = w.item_index(item).unwrap();
    w.player(id).unwrap().inventory.stacks().filter(|s| s.item == i).map(|s| s.count).sum()
}

/// The issue's first half: hands up is a surrender, and a restraint ties whoever holds them up.
#[test]
fn hands_up_then_tied() {
    let (mut w, a, b) = field(true);
    assert!(w.player_restrain(a, None).is_err(), "nothing to tie with");
    w.give_item(a, "cable_tie", 3);
    let err = w.player_restrain(a, None).unwrap_err();
    assert!(err.contains("hands up"), "{err}");
    assert!(w.player_gesture(b, Some(Gesture::HandsUp)));
    assert_eq!(w.faced_player(a), Some(b));
    w.player_restrain(a, None).unwrap();
    assert_eq!(w.player(a).unwrap().hands.map(|h| h.kind.act()), Some(HandsAct::Tie));
    // Asking again while it is under way is neither a second job nor a refusal.
    w.player_restrain(a, Some(b)).unwrap();
    secs(&mut w, 2.0);
    assert!(!tied(&w, b), "it takes three seconds");
    secs(&mut w, 1.1);
    assert!(tied(&w, b) && w.player(a).unwrap().hands.is_none());
    assert_eq!(count(&w, a, "cable_tie"), 2, "one tie used");
    assert!(w.player(b).unwrap().gesture.is_none(), "the hands come down behind the back");
    assert!(last_note(&w, b).contains("Your hands are tied"), "{}", last_note(&w, b));
    assert_eq!(w.player(b).unwrap().restraint.unwrap().by, a);
    assert!(w.player_restrain(a, Some(b)).unwrap_err().contains("already"));
}

#[test]
fn tying_is_called_off_when_they_lower_their_hands_or_step_away_or_we_swing() {
    let (mut w, a, b) = field(true);
    w.give_item(a, "rope", 2);
    // They lower their hands.
    w.player_gesture(b, Some(Gesture::HandsUp));
    w.player_restrain(a, Some(b)).unwrap();
    secs(&mut w, 1.0);
    w.player_gesture(b, None);
    secs(&mut w, 0.1);
    assert!(w.player(a).unwrap().hands.is_none() && !tied(&w, b));
    assert!(last_note(&w, a).starts_with("Couldn't tie their hands"), "{}", last_note(&w, a));
    // They walk off.
    w.player_gesture(b, Some(Gesture::HandsUp));
    w.player_restrain(a, Some(b)).unwrap();
    w.set_player_input(b, 1, PlayerInput { move_dir: Vec2::X, ..Default::default() });
    secs(&mut w, 3.5);
    assert!(!tied(&w, b) && w.player(a).unwrap().hands.is_none(), "walked out of reach");
    assert!(w.player_restrain(a, Some(b)).unwrap_err().contains("too far"));
    w.set_player_input(b, 2, PlayerInput::default());
    // We swing.
    let at = w.player(b).unwrap().state.pos - Vec2::X;
    w.players[0].state.pos = at;
    w.player_gesture(b, None);
    w.player_gesture(b, Some(Gesture::HandsUp));
    w.player_restrain(a, Some(b)).unwrap();
    assert!(w.player_attack(a));
    assert!(w.player(a).unwrap().hands.is_none(), "a swing drops it");
    assert_eq!(count(&w, a, "rope"), 2, "nothing was used");
}

/// With player against player on, someone knocked down or asleep can be tied; in co-op only
/// raised hands will do.
#[test]
fn the_downed_and_the_sleeping_only_with_pvp() {
    for pvp in [true, false] {
        let (mut w, a, b) = field(pvp);
        w.give_item(a, "rope", 1);
        w.players[1].knock = Some(Knock { down: true, timer: 30.0 });
        let r = w.player_restrain(a, Some(b));
        assert_eq!(r.is_ok(), pvp, "{r:?}");
        if pvp {
            secs(&mut w, 3.1);
            assert!(tied(&w, b));
            assert_eq!(count(&w, a, "rope"), 0);
        }
    }
    // A fresh spawn under protection isn't tied.
    let (mut w, a, b) = field(true);
    w.give_item(a, "rope", 1);
    w.player_gesture(b, Some(Gesture::HandsUp));
    w.protect_player(b, 10.0);
    assert!(w.player_restrain(a, Some(b)).is_err());
}

#[test]
fn tied_hands_stop_fighting_handling_and_running() {
    let (mut w, a, b) = field(true);
    w.give_item(b, "tinned_beans", 1);
    w.hold_item(b, Slot::Hand, "baseball_bat");
    let free = w.player(b).unwrap().state.mods;
    assert!(w.restrain_now(b, "rope", a));
    let p = w.player(b).unwrap();
    assert!(!p.state.mods.can_run && !p.state.mods.can_climb);
    assert!((p.state.mods.speed - free.speed * w.config.bonds.speed).abs() < 1e-6);
    w.set_aim(b, true);
    assert!(!w.player_attack(b) && !w.player_shove(b) && !w.player_stomp(b));
    assert!(!w.player_gesture(b, Some(Gesture::Wave)));
    assert!(w.player_drop(b, Place::Carried, 0, 0).unwrap_err().contains("hands are tied"));
    assert!(w.player_unequip(b, Place::Slot(Slot::Hand), 0).is_err());
    w.spawn_item("rag", 1, Vec2::new(61.6, 60.6));
    assert!(w.player_pick_up(b).is_none());
    assert!(w.player_restrain(b, Some(a)).unwrap_err().contains("hands are tied"));
    // It still walks (slowly), and talks.
    let before = w.player(b).unwrap().state.pos;
    w.set_player_input(b, 1, PlayerInput { move_dir: Vec2::Y, run: true, ..Default::default() });
    secs(&mut w, 1.0);
    let moved = w.player(b).unwrap().state.pos.distance(before);
    assert!((moved - players::WALK_SPEED * w.config.bonds.speed).abs() < 0.2, "moved {moved} m in a second");
    assert!(w.player_say(b, "Let me go", crate::speech::Volume::Talk).is_some());
}

#[test]
fn struggling_out_takes_standing_still() {
    let (mut w, a, b) = field(true);
    assert!(!w.player_struggle(b, true), "not tied");
    w.restrain_now(b, "rope", a);
    assert!(w.player_struggle(b, true));
    let need = w.escape_secs(w.player(b).unwrap()).unwrap();
    assert_eq!(need, w.config.bonds.escape_secs);
    // Walking about doesn't count.
    w.set_player_input(b, 1, PlayerInput { move_dir: Vec2::X, ..Default::default() });
    secs(&mut w, 5.0);
    assert_eq!(w.player(b).unwrap().restraint.unwrap().struggled, 0.0);
    w.set_player_input(b, 2, PlayerInput::default());
    secs(&mut w, need - 1.0);
    assert!(tied(&w, b));
    assert!(w.sounds_heard().iter().chain(&w.sounds).count() <= 1);
    secs(&mut w, 1.2);
    assert!(!tied(&w, b), "out after {need} s");
    assert!(w.player(b).unwrap().state.mods.can_run);
    let rope = w.item_index("rope").unwrap();
    assert!(w.items().iter().any(|i| i.stack.item == rope), "the rope lies at its feet");
    // A cable tie takes longer and is gone after.
    w.restrain_now(b, "cable_tie", a);
    assert_eq!(w.escape_secs(w.player(b).unwrap()), Some(w.config.bonds.escape_tie_secs));
    w.player_struggle(b, true);
    secs(&mut w, t().escape_tie_secs + 0.2);
    let tie = w.item_index("cable_tie").unwrap();
    assert!(!tied(&w, b) && !w.items().iter().any(|i| i.stack.item == tie));
}

#[test]
fn freeing_unties_rope_and_cuts_cable_ties() {
    let (mut w, a, b) = field(true);
    let c = w.add_player(Vec2::new(61.5, 61.3));
    assert!(w.player_free(c, Some(b)).unwrap_err().contains("aren't tied"));
    w.restrain_now(b, "rope", a);
    w.player_free(c, Some(b)).unwrap();
    secs(&mut w, t().free_secs + 0.1);
    assert!(!tied(&w, b));
    let rope = w.item_index("rope").unwrap();
    assert_eq!(w.items().iter().filter(|i| i.stack.item == rope).count(), 1);
    w.restrain_now(b, "cable_tie", a);
    let err = w.player_free(c, Some(b)).unwrap_err();
    assert!(err.contains("something that cuts"), "{err}");
    w.give_item(c, "scissors", 1);
    w.player_free(c, Some(b)).unwrap();
    secs(&mut w, t().free_secs + 0.1);
    assert!(!tied(&w, b));
    assert_eq!(count(&w, c, "scissors"), 1, "the tool is kept");
    assert_eq!(last_note(&w, b), "Your hands are free");
}

/// The issue's second half: a tied survivor's things can be gone through and taken.
#[test]
fn searching_a_tied_survivor_and_taking_its_things() {
    let (mut w, a, b) = field(true);
    w.give_item(b, "tinned_beans", 3);
    w.give_item(b, "painkillers", 1);
    w.hold_item(b, Slot::Hand, "kitchen_knife");
    w.hold_item(b, Slot::Back, "school_backpack");
    assert!(w.player_frisk(a, Some(b)).unwrap_err().contains("tied"), "only the tied are searched");
    assert!(w.player_take_from(a, Place::Carried, 0, 0, 0).is_err(), "not searching anyone");
    w.restrain_now(b, "cable_tie", a);
    w.take_private_dirty();
    w.player_frisk(a, None).unwrap();
    secs(&mut w, t().frisk_secs + 0.1);
    assert_eq!(w.player(a).unwrap().frisking, Some(b));
    assert!(w.take_private_dirty().contains(&a), "its client is told what they have");
    assert!(last_note(&w, b).contains("going through your things"));
    // Two of the three tins.
    let beans = w.item_index("tinned_beans").unwrap();
    let at = w.player(b).unwrap().inventory.carried.iter().position(|s| s.item == beans).unwrap() as u32;
    w.player_take_from(a, Place::Carried, at, beans, 2).unwrap();
    secs(&mut w, t().take_secs + 0.1);
    assert_eq!((count(&w, a, "tinned_beans"), count(&w, b, "tinned_beans")), (2, 1));
    assert!(w.take_private_dirty().is_superset(&[a, b].into()), "both clients see it");
    assert!(last_note(&w, b).contains("took your 2 x"), "{}", last_note(&w, b));
    // The knife out of its hand (slower), by a stale index too.
    let knife = w.item_index("kitchen_knife").unwrap();
    w.player_take_from(a, Place::Slot(Slot::Hand), 7, knife, 0).unwrap();
    secs(&mut w, t().take_secs + 0.1);
    assert_eq!(count(&w, a, "kitchen_knife"), 0, "something held takes longer");
    secs(&mut w, t().take_worn_secs);
    assert_eq!((count(&w, a, "kitchen_knife"), count(&w, b, "kitchen_knife")), (1, 0));
    assert!(w.player(b).unwrap().weapon.is_none());
    // A full pack doesn't come off; what doesn't fit our pockets lands at our feet.
    w.give_item(b, "water_bottle", 6);
    let pack = w.item_index("school_backpack").unwrap();
    w.player_take_from(a, Place::Slot(Slot::Back), 0, pack, 0).unwrap();
    secs(&mut w, t().take_worn_secs + 0.1);
    assert!(last_note(&w, a).contains("emptied first"), "{}", last_note(&w, a));
    let water = w.item_index("water_bottle").unwrap();
    let at = w.player(b).unwrap().inventory.carried.iter().position(|s| s.item == water).unwrap() as u32;
    w.player_take_from(a, Place::Carried, at, water, 0).unwrap();
    secs(&mut w, t().take_secs + 0.1);
    assert!(last_note(&w, a).contains("at your feet"), "{}", last_note(&w, a));
    assert!(w.floor_of(a).iter().any(|i| i.stack.item == water));
    // Stepping away shuts the search; so does their getting free.
    w.set_player_input(a, 1, PlayerInput { move_dir: -Vec2::X, ..Default::default() });
    secs(&mut w, 1.0);
    assert_eq!(w.player(a).unwrap().frisking, None);
    assert!(w.player_take_from(a, Place::Carried, 0, beans, 0).is_err());
}

#[test]
fn death_frees_the_hands_and_a_load_keeps_them_tied() {
    let (mut w, a, b) = field(true);
    w.restrain_now(b, "rope", a);
    w.player_struggle(b, true);
    secs(&mut w, 4.0);
    let hash = w.state_hash();
    let l = World::load(&w.save(), &w.config).unwrap();
    assert_eq!(l.state_hash(), hash);
    let r = l.player(b).unwrap().restraint.unwrap();
    assert!(r.struggling && (r.struggled - 4.0).abs() < 0.1 && r.by == a);
    assert!(!l.player(b).unwrap().state.mods.can_run, "still slowed after the load");
    // A world nobody is tied in hashes and saves as it did.
    let (plain, _, _) = field(true);
    let (mut other, _, _) = field(true);
    other.restrain_now(2, "rope", 1);
    assert_ne!(plain.state_hash(), other.state_hash());
    assert!(plain.save().len() < other.save().len());
    // Dying.
    let pi = 1;
    w.hurt_player_by(pi, 1000.0, crate::run::DeathCause::Other);
    assert!(w.player(b).unwrap().dead);
    w.step();
    assert!(!tied(&w, b));
}

#[test]
fn hold_ups_are_deterministic() {
    let run = || {
        let (mut w, a, b) = field(true);
        w.give_item(a, "cable_tie", 1);
        w.give_item(b, "tinned_beans", 2);
        w.player_gesture(b, Some(Gesture::HandsUp));
        w.player_restrain(a, None).unwrap();
        secs(&mut w, 3.2);
        w.player_frisk(a, None).unwrap();
        secs(&mut w, 2.2);
        let beans = w.item_index("tinned_beans").unwrap();
        w.player_take_from(a, Place::Carried, 0, beans, 1).unwrap();
        w.player_struggle(b, true);
        secs(&mut w, 6.0);
        w.state_hash()
    };
    assert_eq!(run(), run());
}
