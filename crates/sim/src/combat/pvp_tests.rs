//! Player against player (`WorldConfig::pvp`, docs/PLAN.md §4.8): two players on one world fight
//! with every weapon, wounds and deaths come out right, and with it off they can't hurt each other.

use super::*;
use crate::items::{self, ItemDef};
use crate::map::{Edge, Side, TileMap};
use crate::run::DeathCause;
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;
use glam::IVec2;
use std::f32::consts::PI;

fn weapons() -> Vec<WeaponDef> {
    items::parse_weapons(include_str!("../../../../assets/data/items/weapons.ron")).unwrap()
}

fn item_defs() -> Vec<ItemDef> {
    items::parse_items(include_str!("../../../../assets/data/items/items.ron"), &weapons()).unwrap()
}

fn config(seed: u64, pvp: bool) -> WorldConfig {
    let templates = vec![BuildingTemplate::from_ron(include_str!("../../../../assets/data/buildings/bungalow.ron")).unwrap()];
    let mut c = WorldConfig::new(seed, 0, templates).with_weapons(weapons()).with_items(item_defs()).with_map_chunks(8).with_pvp(pvp);
    c.start_hour = 12.0;
    // Needs stay out of it: these fights are about blows.
    c.needs = crate::needs::NeedsTuning::off();
    c
}

/// A generated town (so it saves as a game does), two players in the bedroom the first wakes in.
fn town(seed: u64) -> (World, u32, u32) {
    let mut w = World::new(config(seed, true));
    let a = w.add_player(w.player_spawn);
    let b = w.add_player(w.player_spawn + Vec2::new(0.9, 0.0));
    (w, a, b)
}

/// An empty field at noon: player `a` at (60.5, 60.5) facing east, player `b` 0.85 m east of it
/// facing west (face to face, in reach of every weapon and of a shove).
fn duel(seed: u64, pvp: bool) -> (World, u32, u32) {
    let mut w = World::new(config(seed, pvp));
    w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    w.sites.clear();
    let a = w.add_player(Vec2::new(60.5, 60.5));
    let b = w.add_player(Vec2::new(61.35, 60.5));
    face(&mut w, b, PI);
    (w, a, b)
}

fn pi_of(w: &World, id: u32) -> usize {
    w.players.iter().position(|p| p.id == id).unwrap()
}

fn face(w: &mut World, id: u32, facing: f32) {
    let pi = pi_of(w, id);
    w.players[pi].state.facing = facing;
    w.players[pi].input.facing = facing;
}

fn place(w: &mut World, id: u32, pos: Vec2) {
    let pi = pi_of(w, id);
    w.players[pi].state.pos = pos;
}

fn equip(w: &mut World, id: u32, weapon: &str) -> u16 {
    assert!(w.hold_item(id, crate::inventory::Slot::Hand, weapon), "{weapon}");
    w.weapon_index(weapon).unwrap()
}

fn wear(w: &mut World, id: u32, item: &str) {
    let k = w.item_index(item).unwrap();
    let pi = pi_of(w, id);
    w.players[pi].inventory.worn.push(crate::ItemStack::one(k));
    w.refresh_carry(pi);
}

fn rest(w: &mut World, id: u32) {
    let pi = pi_of(w, id);
    w.players[pi].state.endurance = 1.0;
}

/// Step until `id`'s action has landed and it can act again. Returns the hits on players.
fn until_ready(w: &mut World, id: u32) -> Vec<PlayerHitEvent> {
    let mut hits = Vec::new();
    for _ in 0..200 {
        w.step();
        hits.extend_from_slice(&w.player_hit_events);
        let p = w.player(id).unwrap();
        if p.action.is_none() && p.ready_in <= 0.0 && p.queued.is_none() {
            return hits;
        }
    }
    panic!("action never finished");
}

fn knock(w: &mut World, id: u32, down: bool, timer: f32) {
    let pi = pi_of(w, id);
    w.players[pi].knock = Some(Knock { down, timer });
    w.refresh_carry(pi);
}

/// With `pvp` off, nothing a player does touches another: no damage, no wound, no push, no event.
#[test]
fn with_pvp_off_players_cannot_hurt_each_other() {
    let (mut w, a, b) = duel(1, false);
    equip(&mut w, a, "fire_axe");
    let before = w.player(b).unwrap().clone();
    for k in 0..12 {
        rest(&mut w, a);
        assert!(match k % 3 {
            0 => w.player_attack(a),
            1 => w.player_shove(a),
            _ => w.player_stomp(a),
        });
        assert!(until_ready(&mut w, a).is_empty());
    }
    let after = w.player(b).unwrap();
    assert_eq!(after.hp, before.hp);
    assert_eq!(after.state.pos, before.state.pos);
    assert!(after.health.wounds.is_empty() && after.knock.is_none());
    assert!(w.combat_events.is_empty(), "{:?}", w.combat_events);
    // Even one lying down (as after a save made with it on) isn't stamped on.
    knock(&mut w, b, true, 10.0);
    rest(&mut w, a);
    w.player_stomp(a);
    assert!(until_ready(&mut w, a).is_empty());
    assert_eq!(w.player(b).unwrap().hp, before.hp);
}

/// Every weapon in the file hits another player: HP comes off through the wound it leaves, on a
/// body part, by how the weapon hits (blades gash, blunt ones bruise or gash, fists bruise).
#[test]
fn every_weapon_wounds_another_player() {
    for def in weapons() {
        let (mut bruises, mut gashes, mut landed) = (0, 0, 0);
        for seed in 1..=30 {
            let (mut w, a, b) = duel(seed, true);
            if def.category != WeaponCategory::Unarmed {
                equip(&mut w, a, &def.id);
            }
            assert!(w.player_attack(a));
            let hits = until_ready(&mut w, a);
            assert_eq!(hits.len(), 1, "{} seed {seed}: {hits:?}", def.id);
            let h = hits[0];
            assert_eq!((h.player, h.victim, h.kind), (a, b, ActionKind::Swing));
            let v = w.player(b).unwrap();
            let (part, wound) = (h.part.unwrap(), h.wound.unwrap());
            assert!(h.damage > 0.0 && (v.hp - (100.0 - h.damage)).abs() < 0.5 || v.dead, "{}: hp {} after {}", def.id, v.hp, h.damage);
            if !v.dead {
                let left = v.health.wounds.last().unwrap();
                assert_eq!((left.part, left.kind), (part, wound));
                assert!(v.health.infection.is_none(), "a survivor's blow carries no infection");
            }
            // The damage is the weapon's, scaled for players (and doubled or so by a crit).
            let c = &w.config.combat;
            let top = def.damage.1 * c.pvp_damage_mult * def.crit_multiplier.max(1.0) * 1.2;
            assert!(h.damage <= top + 1e-3, "{}: {} over {top}", def.id, h.damage);
            landed += 1;
            match wound {
                WoundKind::Bruise => bruises += 1,
                WoundKind::Gash => gashes += 1,
                other => panic!("{}: left a {other:?}", def.id),
            }
            // The fight was heard, and shown: one PlayerHurt on the victim, by the attacker.
            let e: Vec<&CombatEvent> = w.combat_events.iter().filter(|e| e.kind == CombatEventKind::PlayerHurt).collect();
            assert_eq!(e.len(), 1);
            assert_eq!((e[0].player, e[0].by, e[0].zombie, e[0].part, e[0].wound), (b, a, 0, Some(part), Some(wound)));
        }
        assert_eq!(landed, 30);
        match def.category {
            WeaponCategory::Unarmed => assert_eq!(gashes, 0, "fists only bruise"),
            // The victim wears nothing, so nothing stops a blade.
            WeaponCategory::ShortBlade | WeaponCategory::Axe => assert_eq!(bruises, 0, "{} always cuts bare skin", def.id),
            WeaponCategory::ShortBlunt | WeaponCategory::LongBlunt => assert!(bruises > 0 && gashes > 0, "{}: {bruises} bruises, {gashes} gashes", def.id),
        }
    }
}

/// Clothing stops some cuts to a bruise, which takes less; god mode takes nothing at all.
#[test]
fn clothing_protects_and_god_mode_holds() {
    let (mut cut, mut stopped) = (0, 0);
    for seed in 1..=60 {
        let (mut w, a, b) = duel(seed, true);
        equip(&mut w, a, "kitchen_knife");
        for item in ["leather_jacket", "jeans", "work_gloves", "work_boots"] {
            if w.item_index(item).is_some() {
                wear(&mut w, b, item);
            }
        }
        w.player_attack(a);
        let h = until_ready(&mut w, a)[0];
        match h.wound.unwrap() {
            WoundKind::Gash => cut += 1,
            WoundKind::Bruise => {
                stopped += 1;
                // No bleeding from a blow the jacket took.
                assert_eq!(w.player(b).unwrap().health.bleeding(&w.config.health), 0);
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(stopped >= 8 && cut >= 8, "{stopped} stopped, {cut} cut");

    let (mut w, a, b) = duel(3, true);
    equip(&mut w, a, "fire_axe");
    w.set_dev(b, Some(true), None);
    for _ in 0..6 {
        rest(&mut w, a);
        place(&mut w, b, Vec2::new(61.35, 60.5));
        let pi = pi_of(&w, b);
        w.players[pi].knock = None;
        w.player_attack(a);
        let hits = until_ready(&mut w, a);
        assert!(hits.len() == 1 && hits[0].damage > 0.0, "the blow still lands: {hits:?}");
    }
    let v = w.player(b).unwrap();
    assert_eq!(v.hp, 100.0);
    assert!(!v.dead && v.health.wounds.is_empty());
    assert_eq!(w.player(a).unwrap().run.player_kills, 0);
}

/// A shove always staggers a standing player and pushes it back, and now and then floors it. A
/// player reeling or down can't move or act; one on the ground is only hit by a stomp.
#[test]
fn shoves_stagger_and_knock_down_and_stomps_hit_the_downed() {
    let (mut staggered, mut floored) = (0, 0);
    for seed in 1..=60 {
        let (mut w, a, b) = duel(seed, true);
        assert!(w.player_shove(a));
        let hits = until_ready(&mut w, a);
        assert_eq!(hits.len(), 1);
        let h = hits[0];
        assert_eq!((h.kind, h.damage, h.part, h.wound), (ActionKind::Shove, 0.0, None, None));
        match h.effect {
            HitEffect::Staggered => staggered += 1,
            HitEffect::KnockedDown => floored += 1,
            other => panic!("a shove did {other:?}"),
        }
        let v = w.player(b).unwrap();
        assert_eq!(v.hp, 100.0);
        assert!(v.state.pos.x > 61.35 + 0.3, "pushed back: {}", v.state.pos);
        assert!(w.combat_events.iter().any(|e| e.kind == CombatEventKind::PlayerShoved && e.player == b && e.by == a));
    }
    assert!(staggered > floored && floored >= 5, "{staggered} staggered, {floored} floored");

    // Reeling: no walking, no swinging, until it runs out; then both again.
    let (mut w, a, b) = duel(4, true);
    w.player_shove(a);
    let mut t = 0;
    while w.player(b).unwrap().knock.is_none() {
        w.step();
        t += 1;
        assert!(t < 40);
    }
    let from = w.player(b).unwrap().state.pos;
    assert_eq!(w.player(b).unwrap().state.mods.speed, 0.0);
    w.set_player_input(b, 1, PlayerInput { move_dir: Vec2::X, facing: PI, run: true, ..Default::default() });
    assert!(!w.player_attack(b) && !w.player_shove(b));
    let mut ticks = 0;
    while w.player(b).unwrap().knock.is_some() {
        w.step();
        ticks += 1;
        assert!(ticks < 20 * 8, "never recovered");
        if w.player(b).unwrap().knock.is_some() {
            assert_eq!(w.player(b).unwrap().state.pos, from, "moved while knocked back");
        }
    }
    assert_eq!(w.player(b).unwrap().state.pos, from, "the tick it recovers it still hasn't moved");
    w.step();
    assert!(w.player(b).unwrap().state.pos.x > from.x, "walks again");
    assert!(w.player_attack(b));

    // On the ground: swings and shoves pass over, a stomp lands (a bruise) and it stays down.
    let (mut w, a, b) = duel(5, true);
    equip(&mut w, a, "baseball_bat");
    place(&mut w, b, Vec2::new(61.2, 60.5));
    knock(&mut w, b, true, 30.0);
    w.player_attack(a);
    assert!(until_ready(&mut w, a).is_empty(), "a swing hit a player on the ground");
    w.player_shove(a);
    assert!(until_ready(&mut w, a).is_empty(), "a shove hit a player on the ground");
    rest(&mut w, a);
    w.player_stomp(a);
    let hits = until_ready(&mut w, a);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].kind, hits[0].wound, hits[0].effect), (ActionKind::Stomp, Some(WoundKind::Bruise), HitEffect::Hit));
    let v = w.player(b).unwrap();
    assert!(v.hp < 100.0 && v.knock.is_some_and(|k| k.down));
    // And a stomp does nothing to one standing.
    let (mut w, a, b) = duel(5, true);
    place(&mut w, b, Vec2::new(61.2, 60.5));
    w.player_stomp(a);
    assert!(until_ready(&mut w, a).is_empty());
}

/// Reach, arc and walls are as for zombies; a swing that hits several takes zombies and players
/// nearest first in one list.
#[test]
fn players_are_targets_by_the_same_rules_as_zombies() {
    // Behind, beside and out of reach: untouched.
    for at in [Vec2::new(59.4, 60.5), Vec2::new(60.5, 59.2), Vec2::new(63.5, 60.5)] {
        let (mut w, a, b) = duel(2, true);
        equip(&mut w, a, "baseball_bat");
        place(&mut w, b, at);
        w.player_attack(a);
        assert!(until_ready(&mut w, a).is_empty(), "hit a player at {at}");
    }
    // A wall between: no hit.
    let (mut w, a, _) = duel(2, true);
    equip(&mut w, a, "baseball_bat");
    for y in 50..70 {
        w.map.set_edge(IVec2::new(61, y), Side::W, Edge::Wall);
    }
    w.map_changed();
    w.player_attack(a);
    assert!(until_ready(&mut w, a).is_empty(), "hit through a wall");

    // A bat hits up to its `max_targets`, nearest first: the zombie at 0.7 m, then the player at
    // 0.85 m, and not the zombie behind them at 1.5 m when it only takes two.
    let (mut w, a, b) = duel(2, true);
    let bat = equip(&mut w, a, "baseball_bat");
    w.config.weapons[bat as usize].max_targets = 2;
    let near = w.add_zombie(Vec2::new(61.2, 60.4), PI, 1.0);
    let far = w.add_zombie(Vec2::new(62.0, 60.7), PI, 1.0);
    for z in [near, far] {
        w.zombies.wander_timer[z] = 1e9;
    }
    let (near_id, far_hp) = (w.zombies.id[near], w.zombies.hp[far]);
    w.player_attack(a);
    let mut zombie_hits = Vec::new();
    let mut player_hits = Vec::new();
    for _ in 0..40 {
        w.step();
        zombie_hits.extend_from_slice(&w.hit_events);
        player_hits.extend_from_slice(&w.player_hit_events);
        if !w.action_events.is_empty() {
            assert_eq!(w.action_events[0].hits, 2);
            break;
        }
    }
    assert_eq!(zombie_hits.iter().map(|h| h.zombie).collect::<Vec<_>>(), vec![near_id]);
    assert_eq!(player_hits.iter().map(|h| h.victim).collect::<Vec<_>>(), vec![b]);
    assert_eq!(w.zombies.hp[far], far_hp);
    // The second target takes the falloff.
    assert!(player_hits[0].damage <= 37.0 * w.config.combat.pvp_damage_mult * w.config.combat.multi_hit_falloff * 2.0 + 1e-3);
}

/// A player beaten to death: the cause names the killer's weapon and the wound, the killer's run
/// counts a player killed (not a zombie), and everything the victim had falls where it lay.
#[test]
fn a_death_names_the_survivor_and_the_weapon() {
    let (mut w, a, b) = duel(7, true);
    let crowbar = equip(&mut w, a, "crowbar");
    equip(&mut w, b, "kitchen_knife");
    let mut killed = false;
    for _ in 0..30 {
        rest(&mut w, a);
        let pi = pi_of(&w, b);
        w.players[pi].knock = None;
        place(&mut w, b, Vec2::new(61.35, 60.5));
        w.player_attack(a);
        let hits = until_ready(&mut w, a);
        if hits.iter().any(|h| h.effect == HitEffect::Killed) {
            killed = true;
            break;
        }
    }
    assert!(killed, "30 swings of a crowbar didn't kill");
    let v = w.player(b).unwrap();
    assert!(v.dead && v.knock.is_none());
    let death = v.run.death.expect("a cause of death");
    let DeathCause::Survivor { by, weapon, stomp, part, wound } = death.cause else { panic!("{:?}", death.cause) };
    assert_eq!((by, weapon, stomp), (a, crowbar, false));
    let text = death.describe_with(&w.config.weapons);
    assert!(text.starts_with("Killed by a survivor with a crowbar: "), "{text}");
    assert!(text.ends_with(&format!("to the {}", part.name())) && matches!(wound, WoundKind::Bruise | WoundKind::Gash), "{text}");
    assert!(death.describe().starts_with("Killed by another survivor: "), "{}", death.describe());
    assert_eq!(death.cause.id(), "survivor");
    let k = &w.player(a).unwrap().run;
    assert_eq!((k.player_kills, k.kills), (1, 0));
    assert!(v.inventory.hand.is_none(), "the dead drop what they held");
    assert!(w.items().iter().any(|it| w.item_def(it.stack.item).id == "kitchen_knife"));
    // The dead aren't targets.
    rest(&mut w, a);
    w.player_attack(a);
    assert!(until_ready(&mut w, a).is_empty());

    // Fists and boots have their own words.
    let fists = w.unarmed();
    let cause = DeathCause::Survivor { by: a, weapon: fists, stomp: false, part: Part::Head, wound: WoundKind::Bruise };
    assert_eq!(cause.describe_with(false, &w.config.weapons), "Beaten to death by a survivor: a blow to the head");
    let cause = DeathCause::Survivor { by: a, weapon: fists, stomp: true, part: Part::Torso, wound: WoundKind::Bruise };
    assert_eq!(cause.describe_with(false, &w.config.weapons), "Stamped to death by a survivor: a blow to the torso");
    let axe = w.weapon_index("fire_axe").unwrap();
    let cause = DeathCause::Survivor { by: a, weapon: axe, stomp: false, part: Part::Head, wound: WoundKind::Gash };
    assert_eq!(cause.describe_with(false, &w.config.weapons), "Killed by a survivor with a fire axe: a gash to the head");
}

/// A blow wakes a sleeper and stops first aid, as a zombie's does.
#[test]
fn a_blow_wakes_a_sleeper() {
    let (mut w, a, b) = duel(8, true);
    let pi = pi_of(&w, b);
    w.players[pi].needs.tiredness = 0.8;
    w.player_sleep(b).unwrap();
    assert!(w.player(b).unwrap().needs.asleep());
    w.player_attack(a);
    assert_eq!(until_ready(&mut w, a).len(), 1);
    assert!(!w.player(b).unwrap().needs.asleep());
}

/// Two fighters trading blows: the same seed gives the same fight, and a world saved in the middle
/// of it (one of them reeling) carries on in lockstep with the original.
#[test]
fn a_fight_is_deterministic_and_survives_a_save() {
    let fight = |w: &mut World, a: u32, b: u32, t: u32| {
        for (id, other) in [(a, b), (b, a)] {
            let (Some(me), Some(them)) = (w.player(id), w.player(other)) else { continue };
            let to = them.state.pos - me.state.pos;
            let facing = to.to_angle();
            let move_dir = if to.length() > 1.0 { to.normalize() } else { Vec2::ZERO };
            w.set_player_input(id, t, PlayerInput { move_dir, facing, run: false, sneak: false, aim: true });
            if to.length() < 1.3 {
                if (t / 7 + id).is_multiple_of(3) { w.player_shove(id) } else { w.player_attack(id) };
            }
        }
    };
    let start = |seed| {
        let (mut w, a, b) = town(seed);
        equip(&mut w, a, "baseball_bat");
        equip(&mut w, b, "kitchen_knife");
        (w, a, b)
    };
    let (mut w1, a, b) = start(11);
    let (mut w2, ..) = start(11);
    let mut loaded: Option<World> = None;
    let mut saved_knock = false;
    for t in 1..=400 {
        for w in [&mut w1, &mut w2].into_iter().chain(loaded.as_mut()) {
            fight(w, a, b, t);
            w.step();
        }
        assert_eq!(w1.state_hash(), w2.state_hash(), "tick {t}");
        if let Some(l) = &loaded {
            assert_eq!(l.state_hash(), w1.state_hash(), "loaded world diverged at tick {t}");
        } else if w1.players.iter().any(|p| p.knock.is_some()) {
            // Save with someone reeling: the knock comes back, and so does the count of kills.
            let l = World::load(&w1.save(), &w1.config).unwrap();
            assert_eq!(l.state_hash(), w1.state_hash());
            assert_eq!(l.players.iter().map(|p| p.knock).collect::<Vec<_>>(), w1.players.iter().map(|p| p.knock).collect::<Vec<_>>());
            assert!(l.config.combat.pvp, "the switch is saved with the settings");
            loaded = Some(l);
            saved_knock = true;
        }
    }
    assert!(saved_knock, "nobody was ever staggered");
    let hurt = w1.players.iter().filter(|p| p.hp < 100.0).count();
    assert_eq!(hurt, 2, "both took blows");
    // The run stats (players killed included) are saved too.
    let l = World::load(&w1.save(), &w1.config).unwrap();
    for (p, q) in l.players.iter().zip(&w1.players) {
        assert_eq!(p.run, q.run);
    }
    assert!(w1.players.iter().any(|p| p.dead) == w1.players.iter().any(|p| p.run.player_kills > 0));
}

/// A knock only adds to the state hash while there is one, and a save refuses an impossible one.
#[test]
fn knocks_hash_only_when_set_and_are_checked_on_load() {
    let (mut w, _, b) = town(12);
    assert!(World::load(&w.save(), &w.config).is_ok());
    w.step();
    let hash = w.state_hash();
    let pi = pi_of(&w, b);
    w.players[pi].knock = Some(Knock { down: true, timer: 2.0 });
    assert_ne!(w.state_hash(), hash);
    let other = {
        w.players[pi].knock = Some(Knock { down: false, timer: 2.0 });
        w.state_hash()
    };
    w.players[pi].knock = Some(Knock { down: true, timer: 2.0 });
    assert_ne!(w.state_hash(), other);
    for bad in [f32::NAN, -1.0, 1e6] {
        w.players[pi].knock = Some(Knock { down: true, timer: bad });
        assert!(matches!(World::load(&w.save(), &w.config), Err(crate::save::LoadError::Invalid(_))), "timer {bad} loaded");
    }
    w.players[pi].knock = None;
    assert_eq!(w.state_hash(), hash);
}
