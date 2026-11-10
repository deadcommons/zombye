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

/// A survivor with every status at about `level` (0: nothing wrong; 4: the worst of each).
fn staged(level: u8) -> (World, u32) {
    let (mut w, id) = world();
    let need = [0.2, 0.4, 0.65, 0.78, 0.92][level as usize];
    w.set_needs(id, Some(need), Some(need), Some(need));
    if level == 0 {
        step(&mut w, 2);
        return (w, id);
    }
    // Wounds: a scratch; a gash; both; a gash and a bite, feverish and queasy.
    let wounds: &[(Part, WoundKind)] = match level {
        1 => &[(Part::RightArm, WoundKind::Scratch)],
        2 => &[(Part::LeftLeg, WoundKind::Gash)],
        3 => &[(Part::LeftLeg, WoundKind::Gash), (Part::RightArm, WoundKind::Scratch)],
        _ => &[(Part::LeftLeg, WoundKind::Gash), (Part::Torso, WoundKind::Bite), (Part::Head, WoundKind::Gash)],
    };
    for &(part, kind) in wounds {
        w.inflict_wound(id, part, kind, Some(false));
    }
    // A load over each burden stage, in crowbars.
    assert!(w.hold_item(id, Slot::Back, "hiking_pack"));
    let over = [0.0, 1.05, 1.35, 1.65, 1.96][level as usize];
    for _ in 0..60 {
        let c = w.carry(&w.players[0]);
        if c.weight_kg / c.limit_kg > over {
            break;
        }
        w.give_item(id, "crowbar", 1);
    }
    let p = &mut w.players[0];
    p.hp = [100.0, 86.0, 60.0, 35.0, 14.0][level as usize];
    if level >= 3 {
        p.state.endurance = 0.3;
    }
    if level == 4 {
        (p.state.endurance, p.state.winded) = (0.02, true);
        p.health.food_poisoning_h = 6.0;
        p.health.infection = Some(zombye_sim::health::Infection { stage: zombye_sim::health::InfectionStage::Feverish, left_h: 10.0, feverish_h: 14.0, wound: 1 });
    }
    step(&mut w, 2);
    (w, id)
}

/// Saves for reviewing the status icons (Linear DCM-57): `icons_0` nothing wrong (no icons),
/// `icons_1` to `icons_4` every status at that severity, `icons_cleared` the worst again after
/// eating, drinking and resting (those icons gone, the wounds' still there), `icons_good` fed
/// and rested (the dimmer good states).
#[test]
#[ignore]
fn status_icon_saves() {
    let labels = |w: &World, id: u32| w.status_tags(id).iter().map(|t| format!("{} {}", t.label, t.severity)).collect::<Vec<_>>().join(", ");
    for level in 0..=4u8 {
        let (w, id) = staged(level);
        println!("icons_{level}: {}", labels(&w, id));
        let tags = w.status_tags(id);
        match level {
            0 => assert!(tags.is_empty(), "{tags:?}"),
            _ => {
                for kind in [zombye_sim::TagKind::Health, zombye_sim::TagKind::Hunger, zombye_sim::TagKind::Thirst, zombye_sim::TagKind::Tiredness, zombye_sim::TagKind::Load, zombye_sim::TagKind::Bleeding] {
                    assert_eq!(tags.iter().find(|t| t.kind == kind).map(|t| t.severity), Some(level), "{kind:?} at level {level}: {}", labels(&w, id));
                }
            }
        }
        save(&w, &format!("icons_{level}"));
    }
    let (mut w, id) = staged(4);
    w.set_needs(id, Some(0.2), Some(0.2), Some(0.2));
    step(&mut w, 2);
    println!("icons_cleared: {}", labels(&w, id));
    assert!(w.status_tags(id).iter().all(|t| !matches!(t.kind, zombye_sim::TagKind::Hunger | zombye_sim::TagKind::Thirst | zombye_sim::TagKind::Tiredness)));
    save(&w, "icons_cleared");
    let (mut w, id) = staged(0);
    w.set_needs(id, Some(0.0), None, Some(0.0));
    step(&mut w, 2);
    assert_eq!(labels(&w, id), "Well fed 0, Rested 0");
    save(&w, "icons_good");
}

/// The middle of the asphalt tile nearest to `from` with open asphalt `clear` tiles east of it.
fn street_near(w: &World, from: glam::Vec2, clear: i32) -> glam::Vec2 {
    use zombye_sim::map::Floor;
    let here = from.floor().as_ivec2();
    let road = |t: glam::IVec2| w.map.in_bounds(t) && w.map.tile(t).floor == Floor::Asphalt && !w.map.tile_blocked(t);
    let mut best: Option<(i32, glam::IVec2)> = None;
    for y in -60..=60 {
        for x in -60..=60 {
            let t = here + glam::IVec2::new(x, y);
            let d = x * x + y * y;
            if best.is_none_or(|b| d < b.0) && (0..=clear).all(|i| road(t + glam::IVec2::new(i, 0))) {
                best = Some((d, t));
            }
        }
    }
    best.expect("a street near the spawn house").1.as_vec2() + 0.5
}

/// Saves for judging the light (#83, docs/art-direction.md "Light and mood"), all at night:
/// `night_dark` on the street by the spawn house with nothing in hand and a few zombies up the
/// street; `night_flashlight` the same with a flashlight, and a second survivor with one across
/// the way (`night_flashlight --player 2` is its view); `night_candle` in the bedroom with a
/// candle; `night_window` out in the yard, back from a lit window with zombies in its pool (the
/// window faces the camera with `--angle 2`). Dev toggles aren't saved: add `--god --ignored`
/// to a windowed run.
#[test]
#[ignore]
fn mood_saves() {
    let night = |w: &mut World| {
        let now = w.time_of_day();
        w.skip_time((23.0 - now).rem_euclid(24.0));
    };
    let street = |light: bool| {
        let (mut w, id) = world();
        let at = street_near(&w, w.player_spawn, 14);
        w.players[0].state.pos = at;
        w.players[0].state.facing = 0.0;
        w.set_dev(id, Some(true), Some(true));
        w.spawn_zombies(5, at + glam::Vec2::new(7.5, 0.0), 2.5);
        if light {
            assert!(w.hold_item(id, Slot::Hand, "flashlight"));
            // Another survivor further up the street, its beam coming back toward us.
            let other = w.add_player(at + glam::Vec2::new(13.0, 1.5));
            let pi = w.players.iter().position(|p| p.id == other).unwrap();
            w.players[pi].state.facing = 2.6;
            w.set_dev(other, Some(true), Some(true));
            assert!(w.hold_item(other, Slot::Hand, "flashlight"));
        }
        night(&mut w);
        step(&mut w, 2);
        w
    };
    save(&street(false), "night_dark");
    save(&street(true), "night_flashlight");

    let (mut w, id) = world();
    assert!(w.hold_item(id, Slot::Hand, "candle"));
    night(&mut w);
    step(&mut w, 2);
    save(&w, "night_candle");

    // Out in the yard, well back from a window of the bedroom we woke in (its light is on), with
    // nothing in hand: the zombies standing in the window's pool are all there is to see.
    let (mut w, id) = world();
    let room = w.map.room_at(w.player_spawn);
    let b = &w.map.buildings[w.spawn_building];
    let mut best: Option<(i32, glam::IVec2, glam::IVec2)> = None;
    for y in b.min.y..b.min.y + b.size.y {
        for x in b.min.x..b.min.x + b.size.x {
            let t = glam::IVec2::new(x, y);
            if w.map.tile(t).room != room {
                continue;
            }
            for side in zombye_sim::map::Side::ALL {
                let out = side.offset();
                if !w.map.edge(t, side).is_window() || w.map.tile(t + out).room != 0 {
                    continue;
                }
                let clear = (1..=14).take_while(|i| !w.map.tile_blocked(t + out * *i) && w.map.tile(t + out * *i).room == 0).count() as i32;
                if best.is_none_or(|b| clear > b.0) {
                    best = Some((clear, t, out));
                }
            }
        }
    }
    let (clear, tile, out) = best.expect("a window of the spawn bedroom onto the yard");
    assert!(clear >= 6, "only {clear} m of open ground outside the bedroom window");
    let window = tile.as_vec2() + 0.5 + out.as_vec2() * 0.5;
    let back = (clear.min(12) as f32) - 0.5;
    w.players[0].state.pos = window + out.as_vec2() * back;
    w.players[0].state.facing = (-out.as_vec2()).to_angle();
    w.set_dev(id, Some(true), Some(true));
    w.spawn_zombies(3, window + out.as_vec2() * 1.6, 1.2);
    night(&mut w);
    step(&mut w, 2);
    println!("night_window: the player {back} m out from the window at {window}");
    save(&w, "night_window");
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

/// A hold-up from the wrong end (#73): `tied`, on the street by the spawn house with our hands in
/// a cable tie, a third of the way out of it, the survivor who tied them standing over us with a
/// kitchen knife, and the two of us in a group with a marker up the street (so the bar, the
/// roster, the name tag and the marker's label all show; `--player 2` is the captor's view).
#[test]
#[ignore]
fn holdup_saves() {
    let mut config = content::load_config(&content::default_data_dir(), 1).unwrap();
    config.zombies = Some(0);
    config.combat.pvp = true;
    config.start_hour = 15.0;
    config.needs = zombye_sim::NeedsTuning::off();
    let mut w = World::new(config);
    let at = street_near(&w, w.player_spawn, 8);
    let me = w.add_player(at);
    let captor = w.add_player(at + glam::Vec2::new(1.3, 0.0));
    w.set_player_input(captor, 1, zombye_sim::PlayerInput { facing: std::f32::consts::PI, ..Default::default() });
    w.hold_item(captor, Slot::Hand, "kitchen_knife");
    w.set_player_name(me, "ada");
    w.set_player_name(captor, "bob");
    step(&mut w, 5);
    w.group_invite(me, Some(captor)).unwrap();
    w.group_accept(captor, None).unwrap();
    w.group_mark(captor, at + glam::Vec2::new(7.0, -1.0)).unwrap();
    assert!(w.restrain_now(me, "cable_tie", captor));
    w.player_struggle(me, true);
    step(&mut w, 30 * 20);
    save(&w, "tied");
}
