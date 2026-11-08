//! Saves in the states the M4 screens show (#58), for reviewing them in offscreen pictures
//! (`zombye --render-to shot.png --hud --load PATH`). Ignored: run it by hand with the folder to
//! write them to:
//!
//! `ZOMBYE_REVIEW_DIR=/tmp/review cargo test -p zombye-net --release --test review_saves -- --ignored`
//!
//! Each save has one player in a bedroom of the spawn house: `tags` with most status tags showing,
//! `make` with things to make, one craft under way and one queued, `skills` with XP in several skills,
//! `tired` tired but awake (the narrower view), `asleep` asleep on the bed at night, `bags` a hiking pack
//! full of different things (for picking several and ranges: `--pick-range 1,5`).

use std::path::PathBuf;

use zombye_net::{content, save_file};
use zombye_sim::skills::Skill;
use zombye_sim::{Part, Slot, World, WoundKind};

fn world() -> (World, u32) {
    let mut config = content::load_config(&content::default_data_dir(), 1).unwrap();
    config.zombies = Some(0);
    let mut w = World::new(config);
    let id = w.add_player(w.player_spawn);
    (w, id)
}

fn save(w: &World, name: &str) {
    let dir = PathBuf::from(std::env::var("ZOMBYE_REVIEW_DIR").expect("set ZOMBYE_REVIEW_DIR"));
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

    // Tired but awake, then asleep on the bed at night.
    let (mut w, id) = world();
    w.skip_time(14.0);
    w.set_needs(id, Some(0.2), Some(0.2), Some(0.78));
    step(&mut w, 2);
    save(&w, "tired");
    w.player_sleep(id).unwrap();
    step(&mut w, 40);
    save(&w, "asleep");
}
