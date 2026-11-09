//! Saves in the states the M4 screens show (#58), for reviewing them in offscreen pictures
//! (`zombye --render-to shot.png --hud --load PATH`). Ignored: run it by hand with the folder to
//! write them to (without one: `target/review`):
//!
//! `ZOMBYE_REVIEW_DIR=/tmp/review cargo test -p zombye-net --release --test review_saves -- --ignored`
//!
//! Each save has one player in a bedroom of the spawn house: `tags` with most status tags showing,
//! `make` with things to make, one craft under way and one queued, `skills` with XP in several skills,
//! `tired` tired but awake (the narrower view), `asleep` asleep on the bed at night, `bags` a hiking pack
//! full of different things (for picking several and ranges: `--pick-range 1,5`), `panel` that pack with
//! a container of the house open and things on the floor (the inventory panel's three columns), `agony`
//! in too much pain to sleep and `agony_asleep` asleep all the same with a painkiller dose working (#57),
//! `menu` with things on you and on the floor for the right-click menu.

use std::path::PathBuf;

use zombye_net::{content, save_file};
use zombye_sim::skills::Skill;
use zombye_sim::{Dressing, Part, Slot, Treatment, World, WoundKind};

fn world() -> (World, u32) {
    let mut config = content::load_config(&content::default_data_dir(), 1).unwrap();
    config.zombies = Some(0);
    let mut w = World::new(config);
    let id = w.add_player(w.player_spawn);
    (w, id)
}

fn save(w: &World, name: &str) {
    let dir = std::env::var("ZOMBYE_REVIEW_DIR").map_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/review"), PathBuf::from);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.zsave"));
    save_file::save(w, &path).unwrap();
    println!("wrote {}", path.display());
}

fn step(w: &mut World, ticks: u32) {
    for _ in 0..ticks {
        w.step();
    }
}

#[test]
#[ignore]
fn review_saves() {
    // Status tags: hungry, parched, tired, bleeding, hurting, burdened.
    let (mut w, id) = world();
    w.set_needs(id, Some(0.66), Some(0.9), Some(0.62));
    w.inflict_wound(id, Part::LeftLeg, WoundKind::Gash, Some(false));
    w.inflict_wound(id, Part::RightArm, WoundKind::Scratch, Some(false));
    assert!(w.hold_item(id, Slot::Back, "school_backpack"));
    w.give_item(id, "tinned_beans", 6);
    w.give_item(id, "water_bottle", 4);
    w.give_item(id, "crowbar", 1);
    w.give_item(id, "tinned_beans", 6);
    step(&mut w, 2);
    save(&w, "tags");

    // Things to make: rags from a T-shirt and jeans (with a knife in hand), clean rags under way.
    let (mut w, id) = world();
    assert!(w.hold_item(id, Slot::Hand, "kitchen_knife"));
    w.give_item(id, "t_shirt", 2);
    w.give_item(id, "jeans", 1);
    w.give_item(id, "rag", 6);
    w.give_item(id, "water_bottle", 1);
    let recipe = |w: &World, rid: &str| w.config.recipes.iter().position(|r| r.id == rid).unwrap() as u16;
    let (wash, rip) = (recipe(&w, "wash_rags"), recipe(&w, "rip_t_shirt"));
    w.player_craft(id, wash).unwrap();
    w.player_craft(id, rip).unwrap();
    step(&mut w, 40);
    save(&w, "make");

    // Skills with some practice.
    let (mut w, id) = world();
    let pi = w.players.iter().position(|p| p.id == id).unwrap();
    for (skill, xp) in [(Skill::Fitness, 180.0), (Skill::Edged, 270.0), (Skill::Bludgeon, 60.0), (Skill::Stealth, 30.0), (Skill::FirstAid, 120.0), (Skill::Scavenging, 14.0), (Skill::Handiwork, 5.0)] {
        w.award_xp(pi, skill, xp);
    }
    step(&mut w, 2);
    save(&w, "skills");

    // A pack full of different things, one stack each.
    let (mut w, id) = world();
    assert!(w.hold_item(id, Slot::Back, "hiking_pack"));
    for (item, n) in [("tinned_beans", 3), ("water_bottle", 2), ("rag", 4), ("crackers", 1), ("t_shirt", 1), ("tinned_soup", 2), ("duct_tape", 1), ("candle", 2), ("matches", 1), ("plasters", 1)] {
        w.give_item(id, item, n);
    }
    step(&mut w, 2);
    save(&w, "bags");

    // The whole inventory panel (#94): the pack of things, a container of the spawn house open
    // beside it and things on the floor at the player's feet.
    let site = w.containers().iter().find(|s| s.building as usize == w.spawn_building).cloned().expect("a container in the spawn house");
    let around = (-1..=site.size.x).flat_map(|x| (-1..=site.size.y).map(move |y| (site.min + glam::IVec2::new(x, y)).as_vec2() + 0.5));
    let stood = around.into_iter().any(|pos| {
        w.players[0].state.pos = pos;
        !w.map.tile_blocked(pos.floor().as_ivec2()) && w.containers_in_reach(id).contains(&site.id)
    });
    assert!(stood, "nowhere to stand by container {}", site.id);
    w.player_search(id, Some(site.id)).unwrap();
    for _ in 0..400 {
        if w.player(id).unwrap().jobs.is_empty() {
            break;
        }
        w.step();
    }
    let at = w.players[0].state.pos;
    w.give_item(id, "rope", 1);
    for (item, n) in [("rag", 2), ("kitchen_knife", 1), ("tinned_beans", 2), ("nails", 1)] {
        w.spawn_item(item, n, at).unwrap();
    }
    step(&mut w, 2);
    save(&w, "panel");

    // The same with the pack full (#DCM-56): more kinds of thing, several stacks of one item (T-shirts
    // and jeans don't stack, the tins overflow a stack: rows that collapse), something in the hand
    // and on the shoulder, and more on the floor.
    assert!(w.hold_item(id, Slot::Hand, "crowbar"));
    assert!(w.hold_item(id, Slot::Shoulder, "tote_bag"));
    for (item, n) in [("t_shirt", 2), ("jeans", 2), ("tinned_beans", 7), ("peanut_butter", 1), ("antiseptic", 1), ("claw_hammer", 1), ("scissors", 1), ("flashlight", 1), ("nails", 2), ("clean_rag", 3)] {
        assert!(w.give_item(id, item, n), "{item}");
    }
    for (item, n) in [("t_shirt", 2), ("water_bottle", 1), ("hoodie", 1)] {
        w.spawn_item(item, n, at).unwrap();
    }
    step(&mut w, 2);
    save(&w, "panel_full");

    // Tired but awake, then asleep on the bed at night.
    let (mut w, id) = world();
    w.skip_time(14.0);
    w.set_needs(id, Some(0.2), Some(0.2), Some(0.78));
    step(&mut w, 2);
    save(&w, "tired");
    w.player_sleep(id).unwrap();
    step(&mut w, 40);
    save(&w, "asleep");

    // Agony and painkillers (#57): badly bitten (dressed, so it isn't bleeding out) and tired at
    // night, too much pain to sleep; then a dose taken and asleep on the bed all the same.
    let (mut w, id) = world();
    w.skip_time(14.0);
    w.set_needs(id, Some(0.2), Some(0.2), Some(0.85));
    for part in [Part::Head, Part::Head, Part::Torso, Part::LeftLeg, Part::RightLeg] {
        w.inflict_wound(id, part, WoundKind::Bite, Some(false));
    }
    for wound in &mut w.players[0].health.wounds {
        wound.bleeding = false;
        wound.dressing = Dressing::Clean;
    }
    w.players[0].hp = 60.0;
    w.give_item(id, "painkillers", 1);
    step(&mut w, 2);
    assert!(w.player_sleep(id).unwrap_err().contains("painkillers"));
    save(&w, "agony");
    w.player_treat(id, None, Treatment::Painkiller, None).unwrap();
    step(&mut w, 60);
    assert!(w.status_tags(id).iter().any(|t| t.label == "Agony"), "still agony with a dose working");
    w.player_sleep(id).unwrap();
    step(&mut w, 40);
    assert!(w.player(id).unwrap().needs.asleep());
    save(&w, "agony_asleep");

    // The right-click menu (#95): food, a dressing and a bag on you, a knife and beans on the
    // floor, a container searched open if one is in reach.
    let (mut w, id) = world();
    assert!(w.hold_item(id, Slot::Back, "school_backpack"));
    w.give_item(id, "tinned_beans", 4);
    w.give_item(id, "rag", 3);
    w.give_item(id, "t_shirt", 1);
    let at = w.players[0].state.pos;
    w.spawn_item("kitchen_knife", 1, at + glam::Vec2::new(0.4, 0.0));
    w.spawn_item("tinned_beans", 3, at + glam::Vec2::new(-0.4, 0.2));
    if w.player_search(id, None).is_ok() {
        step(&mut w, 200);
    }
    step(&mut w, 2);
    save(&w, "menu");
}
