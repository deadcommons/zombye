use super::*;
use crate::health::{Part, WoundKind};
use crate::inventory::Place;
use crate::items;
use crate::map::TileMap;
use crate::players::{Player, PlayerInput};
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;
use glam::IVec2;

fn templates() -> Vec<BuildingTemplate> {
    [include_str!("../../../../assets/data/buildings/bungalow.ron"), include_str!("../../../../assets/data/buildings/family_two_storey.ron")]
        .iter()
        .map(|t| BuildingTemplate::from_ron(t).unwrap())
        .collect()
}

fn config(seed: u64) -> WorldConfig {
    let weapons = items::parse_weapons(include_str!("../../../../assets/data/items/weapons.ron")).unwrap();
    let items = items::parse_items(include_str!("../../../../assets/data/items/items.ron"), &weapons).unwrap();
    let containers = crate::containers::parse_containers(include_str!("../../../../assets/data/containers.ron")).unwrap();
    let mut c = WorldConfig::new(seed, 0, templates()).with_weapons(weapons).with_items(items).with_containers(containers).with_map_chunks(8);
    c.start_hour = 12.0;
    c
}

/// An empty grass field at noon with one player at (60.5, 60.5).
fn arena(seed: u64) -> (World, u32) {
    let mut w = World::new(config(seed));
    w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    let id = w.add_player(Vec2::new(60.5, 60.5));
    (w, id)
}

fn me(w: &World, id: u32) -> &Player {
    w.player(id).unwrap()
}

fn ticks_per_hour(w: &World) -> u32 {
    (1.0 / (w.game_hours_per_sec() * TICK_DT)).round() as u32
}

/// Eat or drink the carried stack of `item` and let the job finish; the note it left.
fn use_item(w: &mut World, id: u32, item: &str) -> String {
    let k = me(w, id).inventory.carried.iter().position(|s| w.item_def(s.item).id == item).unwrap_or_else(|| panic!("no {item}"));
    w.item_notes.clear();
    if let Err(why) = w.player_use(id, Place::Carried, k as u32) {
        return format!("refused: {why}");
    }
    for _ in 0..200 {
        w.step();
        if me(w, id).jobs.is_empty() {
            break;
        }
    }
    w.item_notes.iter().map(|n| n.text.clone()).collect::<Vec<_>>().join("; ")
}

/// A player beside a bed in the house it woke up in, and the bed.
fn by_a_bed(seed: u64) -> (World, u32) {
    let mut w = World::new(config(seed));
    let id = w.add_player(w.player_spawn);
    let b = &w.map.buildings[w.spawn_building];
    let bed = b.furniture.iter().find(|f| f.kind == "bed" && f.level == 0).expect("a bed downstairs").clone();
    let room = w.map.room_at(bed.min.as_vec2() + Vec2::splat(0.5));
    let around = (-1..=bed.size.y).flat_map(|dy| (-1..=bed.size.x).map(move |dx| bed.min + IVec2::new(dx, dy)));
    let t = around.filter(|&t| w.map.in_bounds(t) && !w.map.tile_blocked(t) && w.map.tile(t).room == room).min_by_key(|t| (t.x, t.y)).expect("room by the bed");
    w.players[0].state.pos = t.as_vec2() + Vec2::splat(0.5);
    assert_eq!(w.sleep_spot(w.players[0].state.pos), SleepSpot::Bed);
    (w, id)
}

/// Needs rise by the game hour: the same by ticking as by skipping time, faster for thirst,
/// and a day without food and water leaves you starving and parched.
#[test]
fn needs_rise_over_a_game_day_and_with_skip_time() {
    let (mut a, id) = arena(1);
    let (mut b, _) = arena(1);
    let t = a.config.needs;
    for _ in 0..ticks_per_hour(&a) {
        a.step();
    }
    b.skip_time(1.0);
    for w in [&a, &b] {
        let n = me(w, id).needs;
        assert!((n.hunger - t.hunger_per_h).abs() < 1e-3, "hunger {}", n.hunger);
        assert!((n.thirst - t.thirst_per_h).abs() < 1e-3, "thirst {}", n.thirst);
        assert!((n.tiredness - t.tired_per_h).abs() < 1e-3, "tiredness {}", n.tiredness);
    }
    // Running makes you thirstier.
    let (mut r, _) = arena(1);
    for k in 0..ticks_per_hour(&r) {
        // Back and forth, never winded for long.
        let input = PlayerInput { move_dir: Vec2::from_angle(k as f32 * 0.01), facing: 0.0, run: true, sneak: false };
        r.set_player_input(id, k + 1, input);
        r.step();
    }
    assert!(me(&r, id).needs.thirst > me(&a, id).needs.thirst + 0.002, "running: {} vs {}", me(&r, id).needs.thirst, me(&a, id).needs.thirst);
    // A day: from fresh, dry after about 6 hours, peckish after about 9; a day in, starving is
    // close and parched has come.
    b.skip_time(5.0);
    assert_eq!(me(&b, id).needs.thirst_level(&t), Level::Mild);
    assert_eq!(me(&b, id).needs.hunger_level(&t), Level::Fine);
    b.skip_time(3.0);
    assert_eq!(me(&b, id).needs.hunger_level(&t), Level::Mild);
    b.skip_time(11.0);
    let n = me(&b, id).needs;
    assert_eq!((n.hunger_level(&t), n.thirst_level(&t), n.tiredness_level(&t)), (Level::Strong, Level::Severe, Level::Severe), "{n:?}");
    let tags = b.status_tags(id);
    let labels: Vec<&str> = tags.iter().map(|t| t.label.as_str()).collect();
    assert!(labels.contains(&"Hungry") && labels.contains(&"Parched") && labels.contains(&"Exhausted"), "{labels:?}");
    // Starving after about 21 hours.
    b.skip_time(1.0);
    assert_eq!(me(&b, id).needs.hunger_level(&t), Level::Strong);
    b.skip_time(1.5);
    assert_eq!(me(&b, id).needs.hunger_level(&t), Level::Severe);
    assert!(tags.iter().all(|t| !t.tip.is_empty()));
}

/// Eating and drinking take the item's amounts off, one go at a time; crackers make you
/// thirsty; food is wasted when you aren't hungry; food poisoning stops you eating.
#[test]
fn eating_and_drinking_lower_needs() {
    let (mut w, id) = arena(2);
    w.set_needs(id, Some(0.5), Some(0.5), None);
    w.give_item(id, "water_bottle", 1);
    let note = use_item(&mut w, id, "water_bottle");
    let n = me(&w, id).needs;
    assert!((n.thirst - 0.0).abs() < 0.01, "a go of water: {n:?} ({note})");
    assert!(note.contains("1 of 2 goes left") && note.contains("thirst 50% -> 0%"), "{note}");
    assert_eq!(me(&w, id).inventory.carried[0].used, 1, "half a bottle left");
    // Tinned food takes longer than a chocolate bar, both 2-5 s.
    let (beans, choc) = (w.item_index("tinned_beans").unwrap(), w.item_index("chocolate_bar").unwrap());
    assert!(w.use_secs(beans) > w.use_secs(choc) && w.use_secs(choc) >= 2.0 && w.use_secs(beans) <= 5.0);
    w.give_item(id, "tinned_beans", 1);
    let before = me(&w, id).needs.hunger;
    use_item(&mut w, id, "tinned_beans");
    let c = w.item_def(beans).food.clone().unwrap();
    assert!((before - me(&w, id).needs.hunger - c.hunger).abs() < 0.01);
    // Crackers: less hungry, more thirsty.
    if w.item_index("crackers").is_some() {
        w.set_needs(id, Some(0.5), Some(0.2), None);
        w.give_item(id, "crackers", 1);
        use_item(&mut w, id, "crackers");
        let n = me(&w, id).needs;
        assert!(n.hunger < 0.5 && n.thirst > 0.2, "{n:?}");
    }
    // Not hungry: the food is gone, the hunger it would have taken isn't.
    w.set_needs(id, Some(0.05), None, None);
    w.give_item(id, "chocolate_bar", 1);
    let note = use_item(&mut w, id, "chocolate_bar");
    assert!(note.contains("weren't hungry") && me(&w, id).needs.hunger >= 0.05, "{note}");
    assert!(me(&w, id).inventory.carried.iter().all(|s| s.item != choc));
    // Food poisoning: no eating (drinking is fine).
    w.poison_player(id, 1.0);
    w.give_item(id, "chocolate_bar", 1);
    let note = use_item(&mut w, id, "chocolate_bar");
    assert!(note.contains("can't keep food down"), "{note}");
    assert!(me(&w, id).inventory.carried.iter().any(|s| s.item == choc), "still have it");
}

/// Starving and parched cost HP (god mode keeps it), hunger stops HP coming back, and the
/// effects reach movement and swings.
#[test]
fn ignoring_needs_hurts_and_slows() {
    let (mut w, id) = arena(3);
    w.set_needs(id, Some(0.9), Some(0.9), Some(0.9));
    let mods = me(&w, id).state.mods;
    let t = w.config.needs;
    assert!(!mods.can_run && (mods.speed - t.exhausted_walk).abs() < 1e-5, "{mods:?}");
    let fit = w.config.skills.move_mods(1, MoveMods::default()).regen;
    assert!((mods.regen - t.hungry_regen * t.thirsty_regen * fit).abs() < 1e-5);
    let (power, time) = w.needs_swing(0);
    assert!((power - t.starving_swing_power).abs() < 1e-5 && (time - t.exhausted_swing_time).abs() < 1e-5);
    // Five HP an hour.
    w.players[0].hp = 80.0;
    w.skip_time(2.0);
    let hp = me(&w, id).hp;
    assert!((hp - (80.0 - 2.0 * (t.starving_hp_per_h + t.parched_hp_per_h))).abs() < 0.2, "{hp}");
    // God mode: no loss.
    w.set_dev(id, Some(true), None);
    w.skip_time(2.0);
    assert_eq!(me(&w, id).hp, hp);
    // Fed and watered but hungry: no HP back. Fed: it comes back.
    w.set_dev(id, Some(false), None);
    w.set_needs(id, Some(0.65), Some(0.0), Some(0.0));
    let hp = me(&w, id).hp;
    w.skip_time(1.0);
    assert!((me(&w, id).hp - hp).abs() < 1e-4, "hungry: no healing");
    w.set_needs(id, Some(0.0), None, None);
    w.skip_time(1.0);
    assert!(me(&w, id).hp > hp + 1.0);
    assert_eq!(me(&w, id).state.mods, w.config.skills.move_mods(1, MoveMods::default()));
    // Starving to death, god mode off.
    w.set_needs(id, Some(1.0), Some(1.0), None);
    w.skip_time(40.0);
    assert!(me(&w, id).dead);
}

/// Food goes stale at its `spoils_hours`, mouldy at twice that; going off gives less and can
/// poison; stacks of different ages don't merge; food found in furniture is already aging.
#[test]
fn food_spoils_over_time_and_can_poison() {
    let (mut w, id) = arena(4);
    // Days go by: don't starve meanwhile.
    w.set_dev(id, Some(true), None);
    let bread = w.item_index("bread").unwrap();
    let h = w.item_def(bread).food.as_ref().unwrap().spoils_hours.unwrap();
    w.give_item(id, "bread", 1);
    let fresh = me(&w, id).inventory.carried[0];
    assert_eq!(w.freshness_of(&fresh), Some(Freshness::Fresh));
    assert_eq!(w.freshness_of(&ItemStack::new(w.item_index("tinned_beans").unwrap(), 1)), None, "tins keep");
    w.skip_time(h + 0.5);
    assert_eq!(w.freshness_of(&fresh), Some(Freshness::Stale));
    // A new loaf doesn't join the stale one.
    w.give_item(id, "bread", 1);
    assert_eq!(me(&w, id).inventory.carried.iter().filter(|s| s.item == bread).count(), 2);
    w.skip_time(h);
    assert_eq!(w.freshness_of(&fresh), Some(Freshness::Mouldy));
    // Mouldy: 40% of the hunger, usually poisoned. Over a few eaters, some are, some aren't.
    let mut poisoned = 0;
    for seed in 0..10 {
        let (mut w, id) = arena(100 + seed);
        w.config.health.poison_hours = (6.0, 6.0);
        w.set_dev(id, Some(true), None);
        w.give_item(id, "bread", 1);
        w.skip_time(2.0 * h + 1.0);
        w.set_needs(id, Some(0.8), Some(0.0), Some(0.0));
        let note = use_item(&mut w, id, "bread");
        let gain = 0.8 - me(&w, id).needs.hunger;
        let per_go = w.item_def(bread).food.as_ref().unwrap().hunger;
        assert!((gain - per_go * w.config.needs.mouldy_food).abs() < 0.01, "{gain} ({note})");
        assert!(note.contains("mouldy"), "{note}");
        poisoned += me(&w, id).health.food_poisoned() as u32;
    }
    assert!((4..=10).contains(&poisoned), "{poisoned} of 10 poisoned");
    // Found food: the fridge's contents were made before the world began, the same whenever it's opened.
    let found = |skip: f32| {
        let mut w = World::new(config(5));
        let id = w.add_player(w.player_spawn);
        w.skip_time(skip);
        let site = w.containers().iter().find(|s| w.container_def(s).id == "fridge").expect("a fridge").clone();
        let spots = (-1..=site.size.y).flat_map(|dy| (-1..=site.size.x).map(move |dx| site.min + IVec2::new(dx, dy)));
        let t = spots.filter(|&t| w.map.in_bounds(t) && !w.map.tile_blocked(t) && w.map.tile(t).room == site.room).min_by_key(|t| (t.x, t.y)).unwrap();
        w.players[0].state.pos = t.as_vec2() + Vec2::splat(0.5);
        w.player_search(id, Some(site.id)).unwrap();
        for _ in 0..100 {
            w.step();
        }
        w.container_contents(site.id).unwrap().to_vec()
    };
    let (a, b) = (found(0.0), found(30.0));
    assert_eq!(a, b);
    assert!(a.iter().all(|s| s.born <= 0), "{a:?}");
}

/// Sleep: refused when not tired; on a bed tiredness falls, health comes back faster, you don't
/// move, and single-player time runs 30x; rested, you wake by yourself. Two players: only once both sleep.
#[test]
fn sleep_rests_and_speeds_time_up() {
    let (mut w, id) = by_a_bed(6);
    assert!(w.player_sleep(id).unwrap_err().contains("not tired enough"));
    w.set_needs(id, Some(0.0), Some(0.0), Some(0.8));
    w.players[0].hp = 60.0;
    let text = w.player_sleep(id).unwrap();
    assert!(text.contains("bed"), "{text}");
    assert_eq!(w.time_scale(), 30.0);
    let mods = me(&w, id).state.mods;
    assert!(mods.speed == 0.0 && !mods.can_run);
    // Walking asleep goes nowhere.
    let at = me(&w, id).state.pos;
    w.set_player_input(id, 1, PlayerInput { move_dir: Vec2::X, ..Default::default() });
    let clock = w.game_hours();
    let ticks = ticks_per_hour(&w) / 30;
    for _ in 0..ticks {
        w.step();
    }
    assert_eq!(me(&w, id).state.pos, at);
    let hours = w.game_hours() - clock;
    assert!((hours - 1.0).abs() < 0.01, "an hour in {ticks} ticks: {hours}");
    let t = w.config.needs;
    let n = me(&w, id).needs;
    assert!((n.tiredness - (0.8 - t.rest_bed_per_h)).abs() < 0.005, "{n:?}");
    assert!((n.hunger - t.hunger_asleep_per_h).abs() < 0.005);
    assert!((me(&w, id).hp - (60.0 + w.config.health.regen_asleep_per_h)).abs() < 0.1, "{}", me(&w, id).hp);
    assert!(w.status_tags(id).iter().any(|t| t.kind == TagKind::Asleep && t.label == "Asleep x30"));
    // Rested: wakes by itself, time back to normal.
    let mut woke = None;
    for _ in 0..ticks * 10 {
        w.step();
        if let Some(&(p, r)) = w.woke.first() {
            woke = Some((p, r));
            break;
        }
    }
    assert_eq!(woke, Some((id, WakeReason::Rested)));
    assert!(me(&w, id).needs.tiredness < t.rested_below && w.time_scale() == 1.0);
    assert_eq!(me(&w, id).state.mods, w.config.skills.move_mods(1, MoveMods::default()));
    // Two players: one asleep isn't enough.
    let other = w.add_player(w.player_spawn);
    w.set_needs(id, None, None, Some(0.8));
    w.player_sleep(id).unwrap();
    assert_eq!(w.time_scale(), 1.0);
    w.set_needs(other, None, None, Some(0.8));
    w.player_sleep(other).unwrap();
    assert_eq!(w.time_scale(), 30.0);
    assert!(w.wake_player(other));
    assert_eq!(w.time_scale(), 1.0);
}

/// What wakes a sleeper: a noise, a zombie's strike (god mode too), agony; the floor leaves you sore.
#[test]
fn sleepers_wake_on_noise_strikes_and_pain() {
    let asleep = || {
        let (mut w, id) = arena(7);
        w.set_needs(id, None, None, Some(0.9));
        w.player_sleep(id).unwrap();
        (w, id)
    };
    // The floor.
    let (mut w, id) = asleep();
    assert_eq!(me(&w, id).needs.sleep.unwrap().spot, SleepSpot::Floor);
    for _ in 0..3 {
        w.step();
    }
    assert_eq!(w.woke, vec![], "sleeps on in the quiet");
    w.emit_sound(Vec2::new(70.5, 60.5), 15.0);
    let woke_by_noise = (0..3).any(|_| {
        w.step();
        w.woke.contains(&(id, WakeReason::Noise))
    });
    assert!(woke_by_noise, "a noise 10 m off woke us");
    assert!(me(&w, id).needs.sore_h > 0.0 && w.pain_of(0) >= w.config.needs.floor_sore_pain, "a night on the floor");
    // Too far: sleeps on.
    let (mut w, id) = asleep();
    w.emit_sound(Vec2::new(90.5, 60.5), 40.0);
    for _ in 0..3 {
        w.step();
    }
    assert!(me(&w, id).needs.asleep());
    // A zombie's strike, god mode or not.
    let (mut w, id) = asleep();
    w.set_dev(id, Some(true), None);
    let z = w.add_zombie(Vec2::new(61.2, 60.5), std::f32::consts::PI, 1.0);
    w.zombies.wander_timer[z] = 1e9;
    let mut why = None;
    for _ in 0..20 * 20 {
        w.step();
        if let Some(&(_, r)) = w.woke.first() {
            why = Some(r);
            break;
        }
    }
    assert!(matches!(why, Some(WakeReason::Hit | WakeReason::Seen | WakeReason::Noise)), "{why:?}");
    assert_eq!(me(&w, id).hp, 100.0);
    // Agony: no sleeping, and it wakes you.
    let (mut w, id) = asleep();
    for part in [Part::Head, Part::Torso, Part::LeftLeg] {
        w.inflict_wound(id, part, WoundKind::Bite, Some(false));
    }
    w.step();
    assert!(w.woke.contains(&(id, WakeReason::Pain)), "{:?}", w.woke);
    assert!(w.player_sleep(id).unwrap_err().contains("pain"));
}

/// Needs, sleep and food ages are saved: a loaded world steps exactly like the original, asleep
/// and sped up; impossible values are refused.
#[test]
fn needs_save_and_load_in_lockstep() {
    let (mut w, id) = by_a_bed(8);
    w.skip_time(1.0);
    w.give_item(id, "bread", 2);
    w.give_item(id, "water_bottle", 1);
    w.set_needs(id, Some(0.5), Some(0.5), Some(0.7));
    use_item(&mut w, id, "water_bottle");
    use_item(&mut w, id, "bread");
    w.skip_time(3.0);
    w.player_sleep(id).unwrap();
    for _ in 0..50 {
        w.step();
    }
    let mut loaded = World::load(&w.save(), &w.config).unwrap();
    assert_eq!(loaded.players, w.players);
    assert!(loaded.players[0].inventory.carried.iter().any(|s| s.used > 0 && s.born > 0), "{:?}", loaded.players[0].inventory.carried);
    assert_eq!(loaded.time_scale(), 30.0);
    assert_eq!(loaded.state_hash(), w.state_hash());
    for t in 0..400u32 {
        if t == 300 {
            w.wake_player(id);
            loaded.wake_player(id);
        }
        let input = PlayerInput { move_dir: Vec2::from_angle(t as f32 * 0.05) * (t % 3) as f32 * 0.5, facing: t as f32 * 0.05, run: t % 50 < 20, sneak: false };
        for world in [&mut w, &mut loaded] {
            world.set_player_input(id, t + 1, input);
            world.step();
        }
        assert_eq!(loaded.state_hash(), w.state_hash(), "tick {t}");
    }
    assert_eq!(loaded.time_of_day(), w.time_of_day());
    // Crafted values are refused.
    for (what, f) in [
        ("needs.hunger", (|w: &mut World| w.players[0].needs.hunger = 1.5) as fn(&mut World)),
        ("sleep hours", |w| w.players[0].needs.sleep = Some(Sleep { spot: SleepSpot::Bed, hours: -1.0 })),
        ("goes had", |w| w.players[0].inventory.carried[0].used = 9),
        ("made at game minute", |w| w.players[0].inventory.carried[0].born = i32::MAX),
    ] {
        let mut bad = World::load(&w.save(), &w.config).unwrap();
        f(&mut bad);
        match World::load(&bad.save(), &bad.config) {
            Err(crate::save::LoadError::Invalid(why)) => assert!(why.contains(what), "{what}: {why}"),
            other => panic!("{what}: {:?}", other.map(|_| ())),
        }
    }
    let mut bad = w.config.clone();
    bad.needs.severe = 0.1;
    assert!(bad.needs.check().unwrap_err().contains("thresholds"));
}

/// The same run twice gives the same needs, sleep and hash; needs change the hash.
#[test]
fn needs_are_deterministic_and_hashed() {
    let run = || {
        let (mut w, id) = by_a_bed(9);
        w.give_item(id, "apple", 3);
        w.set_needs(id, Some(0.4), Some(0.3), Some(0.6));
        use_item(&mut w, id, "apple");
        w.player_sleep(id).unwrap();
        for _ in 0..600 {
            w.step();
        }
        w.skip_time(2.0);
        (w.state_hash(), me(&w, id).needs, w.game_minute())
    };
    let a = run();
    assert_eq!(a, run());
    let (mut w, id) = arena(9);
    let h = w.state_hash();
    w.set_needs(id, Some(0.2), None, None);
    assert_ne!(w.state_hash(), h);
}

#[test]
fn tuning_checks_and_status_tags() {
    assert!(NeedsTuning::default().check().is_ok());
    let t = NeedsTuning { sleep_time_scale: 0.5, ..Default::default() };
    assert!(t.check().unwrap_err().contains("sleep_time_scale"));
    let t = NeedsTuning::default();
    let s = StatusInput {
        needs: NeedsView { hunger: 0.9, thirst: 0.4, tiredness: 0.65, asleep: None, sore: 0.0 },
        pain: 0.5,
        bleeding: 2,
        food_poisoned: true,
        festering: false,
        infection: None,
        endurance: 0.3,
        winded: false,
        tired_below: 0.5,
        burden: Burden::Heavy,
        time_scale: 1.0,
    };
    let tags = status_tags(&t, &s);
    assert_eq!(tags_line(&tags), "Starving, Dry, Tired, Short of breath, Heavy, Hurting, Bleeding x2, Queasy");
    assert_eq!(tags.iter().map(|t| t.kind).collect::<Vec<_>>(), [TagKind::Hunger, TagKind::Thirst, TagKind::Tiredness, TagKind::Endurance, TagKind::Load, TagKind::Pain, TagKind::Bleeding, TagKind::Sick]);
    assert_eq!(tags[0].severity, 3);
    assert!(tags[0].tip.contains("2 HP"));
}
