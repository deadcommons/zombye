use super::*;
use crate::combat::{HitEffect, HitEvent};
use crate::firstaid::Treatment;
use crate::health::{Part, WoundKind};
use crate::inventory::Slot;
use crate::items;
use crate::map::TileMap;
use crate::players::{Player, PlayerInput};
use crate::save::LoadError;
use crate::senses::SoundSource;
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;
use crate::zombies::ZombieAnim;
use glam::{IVec2, Vec2};
use std::f32::consts::PI;

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
    let mut c = WorldConfig::new(seed, 0, templates()).with_weapons(weapons).with_items(items).with_containers(containers).with_map_chunks(14);
    c.start_hour = 12.0;
    c
}

/// An empty grass field at noon with one player at (100.5, 100.5) facing east.
fn arena(seed: u64) -> (World, u32) {
    let mut w = World::new(config(seed));
    w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    let id = w.add_player(Vec2::new(100.5, 100.5));
    (w, id)
}

fn me(w: &World, id: u32) -> &Player {
    w.player(id).unwrap()
}

fn xp(w: &World, id: u32, s: Skill) -> f32 {
    me(w, id).skills.xp(s)
}

/// A zombie standing still (it can't wander off), facing `facing`, with `hp`.
fn zombie(w: &mut World, pos: Vec2, facing: f32, hp: f32) -> usize {
    let i = w.add_zombie(pos, facing, 1.0);
    w.zombies.wander_timer[i] = 1e9;
    w.zombies.hp[i] = hp;
    i
}

/// Step until the player's action has landed and it can act again. Returns the hits.
fn until_ready(w: &mut World, id: u32) -> Vec<HitEvent> {
    let mut hits = Vec::new();
    for _ in 0..200 {
        w.step();
        hits.extend_from_slice(&w.hit_events);
        let p = me(w, id);
        if p.action.is_none() && p.ready_in <= 0.0 && p.queued.is_none() {
            return hits;
        }
    }
    panic!("action never finished");
}

/// Move with `input` for `ticks` ticks.
fn hold(w: &mut World, id: u32, input: PlayerInput, ticks: u32) {
    for _ in 0..ticks {
        let seq = me(w, id).last_input_seq + 1;
        w.set_player_input(id, seq, input);
        w.step();
    }
}

fn run_east() -> PlayerInput {
    PlayerInput { move_dir: Vec2::X, facing: 0.0, run: true, sneak: false }
}

/// Levels come from cumulative XP; a new survivor has Fitness and Strength 1; XP stops at the top.
#[test]
fn levels_from_xp_and_a_new_survivor() {
    let t = SkillTuning::default();
    assert!(t.check().is_ok());
    assert_eq!([0.0, 99.9, 100.0, 249.0, 250.0, 500.0, 899.0, 900.0, 1500.0, 9999.0].map(|x| t.level(x)), [0, 0, 1, 1, 2, 3, 3, 4, 5, 5]);
    assert_eq!((t.xp_for(0), t.xp_for(3), t.xp_for(9)), (0.0, 500.0, 1500.0));
    let s = Skills::start(&t);
    for k in Skill::ALL {
        let want = if matches!(k, Skill::Fitness | Skill::Strength) { 1 } else { 0 };
        assert_eq!(s.level(k, &t), want, "{k:?}");
        assert_eq!(Skill::from_id(k.id()), Some(k));
    }
    assert_eq!(Skill::FirstAid.name(), "First Aid");
    let v = s.view(&t);
    assert_eq!(v.len(), SKILL_COUNT);
    assert_eq!((v[0].level, v[0].xp, v[0].next_at, v[0].progress), (1, 100.0, Some(250.0), 0.0));
    let (mut w, id) = arena(1);
    assert_eq!(me(&w, id).skills, s);
    // Award past the top: it stops at 1,500, with one level-up per level crossed (here two at once: 0 -> 2).
    assert_eq!(w.award_xp(0, Skill::Edged, 260.0), Some(2));
    assert_eq!(w.award_xp(0, Skill::Edged, 5000.0), Some(5));
    assert_eq!(xp(&w, id, Skill::Edged), 1500.0);
    assert_eq!(w.award_xp(0, Skill::Edged, 10.0), None);
    let ups = w.take_level_ups();
    assert_eq!(ups, [LevelUp { player: id, skill: Skill::Edged, level: 2 }, LevelUp { player: id, skill: Skill::Edged, level: 5 }]);
    assert_eq!(ups[0].text(), "Edged 2");
    assert!(w.take_level_ups().is_empty());
    assert_eq!(w.award_xp(0, Skill::Edged, f32::NAN), None);
    assert_eq!(w.award_xp(9, Skill::Edged, 1.0), None, "no such player");
    let mut bad = SkillTuning::default();
    bad.levels[2] = 200.0;
    assert!(bad.check().unwrap_err().contains("levels must rise"));
    let bad = SkillTuning { weapon_time: 0.9, ..Default::default() };
    assert!(bad.check().unwrap_err().contains("weapon_time"));
}

/// Running trains Fitness (1 XP per 10 s); a Fitness level-up changes the movement modifiers at the
/// end of the tick, and running then drains endurance slower and it comes back faster.
#[test]
fn running_trains_fitness_and_fitness_changes_endurance() {
    let (mut w, id) = arena(2);
    let t = w.config.skills.clone();
    assert_eq!(me(&w, id).state.mods, t.move_mods(1, MoveMods::default()), "Fitness 1 from the start");
    hold(&mut w, id, run_east(), 200);
    assert!((xp(&w, id, Skill::Fitness) - 101.0).abs() < 0.01, "{}", xp(&w, id, Skill::Fitness));
    // Walking gives nothing.
    hold(&mut w, id, PlayerInput { run: false, ..run_east() }, 100);
    assert!((xp(&w, id, Skill::Fitness) - 101.0).abs() < 0.01);
    let mods = me(&w, id).state.mods;
    let to = 250.0 - xp(&w, id, Skill::Fitness);
    assert_eq!(w.award_xp(0, Skill::Fitness, to), Some(2));
    assert_eq!(me(&w, id).state.mods, mods, "not before the tick ends");
    w.step();
    let m = me(&w, id).state.mods;
    assert!((m.run_drain - (1.0 - 2.0 * t.fitness_drain)).abs() < 1e-5 && (m.regen - (1.0 + 2.0 * t.fitness_regen)).abs() < 1e-5, "{m:?}");
    // Five seconds' run drains 12% less than at level 0.
    w.players[0].state.endurance = 1.0;
    hold(&mut w, id, PlayerInput { move_dir: -Vec2::X, ..run_east() }, 100);
    let drained = 1.0 - me(&w, id).state.endurance;
    assert!((drained - 5.0 * crate::players::RUN_DRAIN * m.run_drain).abs() < 0.01, "{drained}");
}

/// Strength: a swing of a weapon over 1.5 kg (not a lighter one), and moving over the carry limit
/// (not standing still), each give XP; a level adds 1.5 kg to the limit, knockback and shove knockdowns.
#[test]
fn strength_from_heavy_swings_and_loads() {
    let (mut w, id) = arena(3);
    let t = w.config.skills.clone();
    assert!(w.hold_item(id, Slot::Hand, "baseball_bat"));
    assert!(w.player_attack(id));
    until_ready(&mut w, id);
    assert_eq!(xp(&w, id, Skill::Strength), 100.0, "a 1 kg bat isn't heavy");
    assert!(w.hold_item(id, Slot::Hand, "fire_axe"));
    assert!(w.player_attack(id));
    until_ready(&mut w, id);
    assert_eq!(xp(&w, id, Skill::Strength), 100.0 + t.heavy_swing_xp);
    // 18 kg of planks against a 15.5 kg limit.
    w.players[0].inventory = crate::inventory::Inventory::default();
    let plank = w.item_index("wooden_plank").unwrap();
    w.players[0].inventory.carried = (0..9).map(|_| crate::items::ItemStack::one(plank)).collect();
    w.refresh_carry(0);
    assert_eq!(w.carry(me(&w, id)).limit_kg, 15.5);
    assert!(w.carry(me(&w, id)).burden > Burden::None);
    let before = xp(&w, id, Skill::Strength);
    hold(&mut w, id, PlayerInput::default(), 100);
    assert_eq!(xp(&w, id, Skill::Strength), before, "standing still trains nothing");
    let ticks = 500;
    hold(&mut w, id, PlayerInput { run: false, ..run_east() }, ticks);
    let minutes = ticks as f32 * TICK_DT * w.game_hours_per_sec() * 60.0;
    assert!((xp(&w, id, Skill::Strength) - before - minutes * t.overload_xp_per_min).abs() < 0.01, "{minutes} game minutes");
    // Strength 2: a bigger limit, at the end of the tick.
    let to = 250.0 - xp(&w, id, Skill::Strength);
    assert_eq!(w.award_xp(0, Skill::Strength, to), Some(2));
    w.step();
    assert_eq!(w.carry(me(&w, id)).limit_kg, 17.0);
    assert_eq!(w.carry(me(&w, id)).burden, Burden::Burdened, "18 kg of 17");
    assert!((t.knockback_mult(2) - 1.1).abs() < 1e-6 && (t.shove_knockdown_bonus(2, 1) - 0.1).abs() < 1e-6);
}

/// Hits train the weapon's skill (2 a hit, 5 a kill): blunt Bludgeon, knives and axes Edged, fists
/// Brawl; shoves train Brawl per zombie shoved, and a stomp that kills 5.
#[test]
fn hits_train_the_weapons_skill() {
    let (mut w, id) = arena(4);
    let t = w.config.skills.clone();
    let z = zombie(&mut w, Vec2::new(101.3, 100.5), PI, 1e4);
    let swing = |w: &mut World, weapon: Option<&str>, hp: f32| {
        match weapon {
            Some(it) => assert!(w.hold_item(id, Slot::Hand, it)),
            None => w.players[0].inventory.hand = None,
        }
        w.refresh_carry(0);
        (w.zombies.pos[z], w.zombies.hp[z], w.zombies.state[z], w.zombies.anim[z]) = (Vec2::new(101.3, 100.5), hp, ZombieState::Wander, ZombieAnim::Idle);
        w.players[0].state.endurance = 1.0;
        assert!(w.player_attack(id));
        let hits = until_ready(w, id);
        assert_eq!(hits.len(), 1, "{weapon:?}");
        hits[0].effect
    };
    swing(&mut w, Some("baseball_bat"), 1e4);
    assert_eq!(xp(&w, id, Skill::Bludgeon), t.hit_xp);
    swing(&mut w, Some("kitchen_knife"), 1e4);
    swing(&mut w, Some("fire_axe"), 1e4);
    assert_eq!(xp(&w, id, Skill::Edged), 2.0 * t.hit_xp);
    swing(&mut w, None, 1e4);
    assert_eq!(xp(&w, id, Skill::Brawl), t.hit_xp, "a punch");
    assert_eq!(swing(&mut w, Some("claw_hammer"), 1.0), HitEffect::Killed);
    assert_eq!(xp(&w, id, Skill::Bludgeon), t.hit_xp + t.kill_xp, "a kill gives the kill's XP");
    // A shove into two zombies.
    let z2 = zombie(&mut w, Vec2::new(101.2, 101.0), PI, 1e4);
    (w.zombies.state[z], w.zombies.anim[z], w.zombies.hp[z]) = (ZombieState::Wander, ZombieAnim::Idle, 1e4);
    (w.zombies.pos[z], w.zombies.vel[z]) = (Vec2::new(101.2, 100.0), Vec2::ZERO);
    w.players[0].state.endurance = 1.0;
    assert!(w.player_shove(id));
    assert_eq!(until_ready(&mut w, id).len(), 2);
    assert_eq!(xp(&w, id, Skill::Brawl), t.hit_xp + 2.0 * t.shove_xp);
    // A stomp on a downed one: only a kill counts.
    for (hp, gain) in [(1e4, 0.0), (1.0, t.stomp_kill_xp)] {
        let before = xp(&w, id, Skill::Brawl);
        (w.zombies.pos[z2], w.zombies.state[z2], w.zombies.timer[z2], w.zombies.hp[z2]) = (Vec2::new(101.1, 100.5), ZombieState::Down, 10.0, hp);
        w.players[0].state.endurance = 1.0;
        assert!(w.player_stomp(id));
        assert_eq!(until_ready(&mut w, id).len(), 1);
        assert_eq!(xp(&w, id, Skill::Brawl) - before, gain);
    }
}

/// Bludgeon (and Edged) levels make swings stronger, cheaper and quicker; Brawl makes punches and
/// stomps stronger.
#[test]
fn weapon_levels_change_the_swing() {
    let swing = |bludgeon: f32, strength: u32| {
        let (mut w, id) = arena(5);
        assert!(w.hold_item(id, Slot::Hand, "baseball_bat"));
        let bat = w.weapon_index("baseball_bat").unwrap() as usize;
        (w.config.weapons[bat].crit_chance, w.config.weapons[bat].knockdown_chance, w.config.weapons[bat].stagger_chance) = (0.0, 0.0, 0.0);
        w.players[0].skills.xp[Skill::Bludgeon as usize] = bludgeon;
        w.players[0].skills.xp[Skill::Strength as usize] = w.config.skills.xp_for(strength);
        zombie(&mut w, Vec2::new(101.6, 100.5), PI, 1e4);
        assert!(w.player_attack(id));
        let (cost, ready) = (1.0 - me(&w, id).state.endurance, me(&w, id).ready_in);
        let mut hit = None;
        for _ in 0..40 {
            w.step();
            if let Some(h) = w.hit_events.first() {
                hit = Some(h.damage);
                break;
            }
        }
        (cost, ready, hit.expect("hit"), w.config.weapons[bat].clone())
    };
    let (cost0, ready0, d0, bat) = swing(0.0, 0);
    let (cost3, ready3, d3, _) = swing(500.0, 3);
    let t = SkillTuning::default();
    assert!((cost3 / cost0 - (1.0 - 3.0 * t.weapon_cost)).abs() < 1e-4, "{cost0} {cost3}");
    assert!((ready3 / ready0 - (1.0 - 3.0 * t.weapon_time)).abs() < 1e-4, "{ready0} {ready3}");
    let m = 1.0 + 3.0 * t.weapon_damage;
    assert!(d0 >= bat.damage.0 - 1e-3 && d0 <= bat.damage.1 + 1e-3);
    assert!(d3 >= bat.damage.0 * m - 1e-3 && d3 <= bat.damage.1 * m + 1e-3 && d3 > d0, "{d0} -> {d3}");
    assert!((t.damage_mult(Skill::Brawl, 2) - 1.2).abs() < 1e-6 && t.cost_mult(Skill::Brawl, 5) == 1.0 && t.time_mult(Skill::Stealth, 5) == 1.0);
}

/// Sneaking (and moving) near a zombie that isn't after you trains Stealth; Stealth quietens
/// sneaking footsteps.
#[test]
fn sneaking_near_unaware_zombies_trains_stealth() {
    let sneak = PlayerInput { move_dir: Vec2::Y, facing: PI / 2.0, run: false, sneak: true };
    let (mut w, id) = arena(6);
    hold(&mut w, id, sneak, 100);
    assert_eq!(xp(&w, id, Skill::Stealth), 0.0, "no zombie about");
    // Facing away, 8 m off our path.
    let at = me(&w, id).state.pos + Vec2::new(8.0, 4.0);
    let z = zombie(&mut w, at, 0.0, 100.0);
    hold(&mut w, id, sneak, 100);
    assert!((xp(&w, id, Skill::Stealth) - 100.0 * TICK_DT * w.config.skills.stealth_xp_per_sec).abs() < 1e-3, "{}", xp(&w, id, Skill::Stealth));
    // Not walking, and not with it after you.
    let before = xp(&w, id, Skill::Stealth);
    hold(&mut w, id, PlayerInput { sneak: false, ..sneak }, 50);
    (w.zombies.state[z], w.zombies.target_player[z], w.zombies.timer[z]) = (ZombieState::Chase, id, 100.0);
    w.zombies.pos[z] = me(&w, id).state.pos + Vec2::new(8.0, 0.0);
    hold(&mut w, id, sneak, 3);
    assert_eq!(xp(&w, id, Skill::Stealth), before);
    // Quieter footsteps at Stealth 3.
    let step = |w: &World| w.heard.iter().find(|s| s.source == SoundSource::Player(id)).map(|s| s.radius).unwrap();
    let (mut w, id) = arena(6);
    hold(&mut w, id, sneak, 1);
    let quiet0 = step(&w);
    w.players[0].skills.xp[Skill::Stealth as usize] = 500.0;
    hold(&mut w, id, sneak, 1);
    assert!((step(&w) / quiet0 - w.config.skills.footstep_mult(3)).abs() < 1e-5);
    assert!((w.targets[0].sight_mult - w.config.skills.sight_mult(3)).abs() < 1e-6);
}

/// Each treatment is First Aid XP; levels make it faster, and from 3 stitching hurts half as much.
#[test]
fn first_aid_trains_and_helps() {
    let stitch = |first_aid: f32| {
        let (mut w, id) = arena(7);
        w.players[0].skills.xp[Skill::FirstAid as usize] = first_aid;
        w.give_item(id, "sewing_kit", 1);
        w.inflict_wound(id, Part::LeftArm, WoundKind::Gash, Some(false)).unwrap();
        let wound = me(&w, id).health.wounds[0].id;
        let kit = w.item_index("sewing_kit").unwrap();
        w.player_treat(id, Some(wound), Treatment::Stitch, Some(kit)).unwrap();
        let secs = me(&w, id).jobs[0].left;
        for _ in 0..400 {
            w.step();
            if me(&w, id).jobs.is_empty() {
                break;
            }
        }
        assert!(me(&w, id).health.wounds[0].stitched);
        (secs, me(&w, id).health.aid.sting_pain(), xp(&w, id, Skill::FirstAid) - first_aid)
    };
    let (secs0, pain0, gain) = stitch(0.0);
    assert_eq!(gain, SkillTuning::default().treat_xp);
    let (secs3, pain3, _) = stitch(500.0);
    let aid = crate::firstaid::FirstAidTuning::default();
    assert!((secs3 / secs0 - (1.0 - 3.0 * aid.skill_faster_per_level)).abs() < 1e-4, "{secs0} {secs3}");
    assert!((pain3 - pain0 * 0.5).abs() < 1e-5 && pain0 > 0.0, "{pain0} {pain3}");
}

/// Searching a container for the first time trains Scavenging (reopening doesn't); levels make
/// first searches faster and quieter.
#[test]
fn searching_trains_scavenging() {
    let search = |scavenging: f32| {
        let mut w = World::new(config(8));
        let id = w.add_player(w.player_spawn);
        w.players[0].skills.xp[Skill::Scavenging as usize] = scavenging;
        // Beside the spawn house's nearest container.
        let spawn = w.player_spawn;
        let mut sites: Vec<_> = w.containers().iter().filter(|s| s.building as usize == w.spawn_building).cloned().collect();
        sites.sort_by(|a, b| a.distance(spawn).total_cmp(&b.distance(spawn)).then(a.id.cmp(&b.id)));
        let site = sites.into_iter().find(|s| w.container_def(s).noise_m > 0.0).expect("a noisy container");
        let spot = (-1..=site.size.y)
            .flat_map(|dy| (-1..=site.size.x).map(move |dx| site.min + IVec2::new(dx, dy)))
            .map(|t| t.as_vec2() + Vec2::splat(0.5))
            .find(|&c| w.map.in_bounds(c.floor().as_ivec2()) && !w.map.tile_blocked(c.floor().as_ivec2()) && w.map.room_at(c) == site.room && site.distance(c) <= 1.0)
            .expect("a spot by it");
        w.players[0].state.pos = spot;
        w.player_search(id, Some(site.id)).unwrap();
        let secs = me(&w, id).jobs[0].left;
        let mut noise = None;
        for _ in 0..400 {
            w.step();
            // Standing still, the only sound we make is the search.
            noise = noise.or(w.sounds.iter().find(|s| s.source == SoundSource::Player(id)).map(|s| s.radius));
            if me(&w, id).jobs.is_empty() {
                break;
            }
        }
        let gained = xp(&w, id, Skill::Scavenging) - scavenging;
        // Again: a reopening, no XP.
        w.player_search(id, Some(site.id)).unwrap();
        for _ in 0..40 {
            w.step();
        }
        assert_eq!(xp(&w, id, Skill::Scavenging) - scavenging, gained, "reopening trains nothing");
        (secs, noise.expect("searching makes a noise"), gained)
    };
    let (secs0, noise0, gain) = search(0.0);
    let t = SkillTuning::default();
    assert_eq!(gain, t.search_xp);
    let (secs2, noise2, _) = search(250.0);
    assert!((secs2 / secs0 - t.search_time_mult(2)).abs() < 1e-4, "{secs0} {secs2}");
    assert!((noise2 / noise0 - t.search_noise_mult(2)).abs() < 1e-4, "{noise0} {noise2}");
}

/// Handiwork for crafting (#12, built elsewhere): the level and XP hooks, and the time multiplier.
#[test]
fn handiwork_hooks_for_crafting() {
    let (mut w, id) = arena(9);
    assert_eq!(w.skill_level(0, Skill::Handiwork), 0);
    for _ in 0..20 {
        w.award_xp(0, Skill::Handiwork, w.config.skills.craft_xp);
    }
    assert_eq!(xp(&w, id, Skill::Handiwork), 100.0);
    assert_eq!(w.skill_level(0, Skill::Handiwork), 1);
    assert_eq!(w.take_level_ups(), [LevelUp { player: id, skill: Skill::Handiwork, level: 1 }]);
    assert!((w.config.skills.craft_time_mult(2) - 0.8).abs() < 1e-6 && w.config.skills.craft_time_mult(5) == 0.5);
    assert_eq!(w.skill_level(7, Skill::Handiwork), 0, "no such player");
    // The dead learn nothing; a respawn is a new survivor.
    w.players[0].hp = 1.0;
    w.inflict_wound(id, Part::Head, WoundKind::Bite, Some(false));
    assert!(me(&w, id).dead);
    assert_eq!(w.award_xp(0, Skill::Handiwork, 10.0), None);
    assert!(w.respawn_player(id));
    assert_eq!(me(&w, id).skills, Skills::start(&w.config.skills));
    assert!(SkillTuning::default().gives(Skill::Handiwork, 1, &w.config.carry, &w.config.health.first_aid).contains("10% faster"));
    assert!(SkillTuning::default().gives(Skill::Strength, 2, &w.config.carry, &w.config.health.first_aid).contains("17.0 kg"));
}

/// Skills are saved: a loaded world steps exactly like the original while XP is earned, and
/// impossible XP is refused.
#[test]
fn skills_save_and_load_in_lockstep() {
    let mut w = World::new(config(10));
    let id = w.add_player(w.player_spawn);
    let out = w.map.resolve_circle(w.player_spawn + Vec2::new(6.0, 3.0), 0.3);
    w.add_zombie(out, 0.0, 1.0);
    // Knocked down for the whole test, so it's never after the player whatever it could see from
    // where it lies (sneaking past it still earns Stealth).
    (w.zombies.state[0], w.zombies.timer[0]) = (ZombieState::Down, 600.0);
    for (k, s) in Skill::ALL.into_iter().enumerate() {
        w.award_xp(0, s, 37.5 * k as f32 + 0.25);
    }
    w.step();
    let mut loaded = World::load(&w.save(), &w.config).unwrap();
    assert_eq!(loaded.players, w.players);
    assert_eq!(loaded.state_hash(), w.state_hash());
    for t in 0..400u32 {
        let input = PlayerInput { move_dir: Vec2::from_angle(t as f32 * 0.03), facing: t as f32 * 0.03, run: t % 120 < 50, sneak: t % 120 > 80 };
        for world in [&mut w, &mut loaded] {
            world.set_player_input(id, t + 1, input);
            world.step();
        }
        assert_eq!(loaded.state_hash(), w.state_hash(), "tick {t}");
    }
    assert!(xp(&w, id, Skill::Fitness) > 100.25 && xp(&w, id, Skill::Stealth) > 187.75, "{:?}", me(&w, id).skills);
    assert_eq!(loaded.players, w.players);
    for (what, f) in [
        ("fitness XP is NaN", (|w: &mut World| w.players[0].skills.xp[0] = f32::NAN) as fn(&mut World)),
        ("edged XP is -1", |w| w.players[0].skills.xp[Skill::Edged as usize] = -1.0),
        ("handiwork XP is 2000", |w| w.players[0].skills.xp[Skill::Handiwork as usize] = 2000.0),
        ("skills: levels must rise", |w| w.config.skills.levels = [100.0, 50.0, 500.0, 900.0, 1500.0]),
    ] {
        let mut bad = World::load(&w.save(), &w.config).unwrap();
        f(&mut bad);
        match World::load(&bad.save(), &w.config) {
            Err(LoadError::Invalid(why)) => assert!(why.contains(what), "{what}: {why}"),
            other => panic!("{what}: {:?}", other.map(|_| ())),
        }
    }
}

/// The same run twice gives the same skills and hash; XP changes the hash.
#[test]
fn skills_are_deterministic_and_hashed() {
    let run = || {
        let (mut w, id) = arena(11);
        assert!(w.hold_item(id, Slot::Hand, "fire_axe"));
        w.spawn_zombies(6, Vec2::new(104.0, 100.5), 2.0);
        for t in 0..600u32 {
            let input = PlayerInput { move_dir: Vec2::from_angle(t as f32 * 0.02) * 0.5, facing: 0.0, run: t % 200 < 60, sneak: t % 200 > 150 };
            w.set_player_input(id, t + 1, input);
            if t % 15 == 0 {
                w.player_attack(id);
            }
            w.step();
        }
        (w.state_hash(), me(&w, id).skills)
    };
    let a = run();
    assert_eq!(a, run());
    assert!(a.1.xp(Skill::Edged) > 0.0 || a.1.xp(Skill::Strength) > 100.0, "{:?}", a.1);
    let (mut w, _) = arena(11);
    let h = w.state_hash();
    w.award_xp(0, Skill::Stealth, 0.5);
    assert_ne!(w.state_hash(), h);
}
