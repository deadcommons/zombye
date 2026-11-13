//! Guns (#72): the data, where a shot goes and what stops it, heads and pitch, rounds, magazines
//! and reloading, the noise, lag compensation, players shooting players, saves and determinism.

use glam::IVec2;

use super::*;
use crate::combat::{HitEffect, HitEvent};
use crate::health::Part;
use crate::inventory::Slot;
use crate::items;
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;
use crate::zombies::ZombieState;
use crate::{DeathCause, PlayerInput};

fn templates() -> Vec<BuildingTemplate> {
    [include_str!("../../../../assets/data/buildings/bungalow.ron"), include_str!("../../../../assets/data/buildings/family_two_storey.ron")]
        .iter()
        .map(|t| BuildingTemplate::from_ron(t).unwrap())
        .collect()
}

const WEAPONS: &str = include_str!("../../../../assets/data/items/weapons.ron");
const ITEMS: &str = include_str!("../../../../assets/data/items/items.ron");

fn weapons() -> Vec<WeaponDef> {
    items::parse_weapons(WEAPONS).unwrap()
}

fn config(seed: u64) -> WorldConfig {
    let weapons = weapons();
    let items = items::parse_items(ITEMS, &weapons).unwrap();
    let mut config = WorldConfig::new(seed, 0, templates()).with_weapons(weapons).with_items(items);
    config.start_hour = 12.0;
    config
}

/// An empty grass field at noon with one player at (100.5, 100.5) facing east, holding `gun`
/// (loaded), whom zombies neither see nor hear unless a test says otherwise.
fn range(seed: u64, gun: &str) -> (World, u32) {
    let mut w = World::new(config(seed));
    w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    w.spawn_building = 0;
    let id = w.add_player(Vec2::new(100.5, 100.5));
    assert!(w.hold_item(id, Slot::Hand, gun));
    w.set_dev(id, None, Some(true));
    (w, id)
}

const ORIGIN: Vec2 = Vec2::new(100.5, 100.5);

/// A zombie standing still `east` metres east of the shooter and `north` off the line, facing it.
fn zombie(w: &mut World, east: f32, north: f32) -> usize {
    let i = w.add_zombie(ORIGIN + Vec2::new(east, north), std::f32::consts::PI, 0.0);
    w.zombies.wander_timer[i] = 1e9;
    i
}

/// The pitch that puts a level shot `height` up at `dist` metres.
fn pitch_for(height: f32, dist: f32) -> f32 {
    ((height - MUZZLE_HEIGHT) / dist).atan()
}

/// Pull the trigger and step until the shot has gone and the next could. Returns its hits.
fn shoot(w: &mut World, id: u32) -> Vec<HitEvent> {
    assert!(w.player_attack(id), "the trigger does nothing");
    let mut hits = Vec::new();
    for _ in 0..200 {
        w.step();
        hits.extend_from_slice(&w.hit_events);
        let p = w.player(id).unwrap();
        if p.action.is_none() && p.ready_in <= 0.0 && p.queued.is_none() {
            return hits;
        }
    }
    panic!("the shot never finished");
}

fn rounds(w: &World, id: u32) -> u8 {
    w.loaded(id).expect("a gun in hand").0
}

fn carried(w: &World, id: u32, item: &str) -> u32 {
    let i = w.item_index(item).unwrap();
    w.player(id).unwrap().inventory.carried.iter().filter(|s| s.item == i).map(|s| s.count as u32).sum()
}

fn settle_jobs(w: &mut World, id: u32) {
    for _ in 0..2000 {
        if w.player(id).unwrap().jobs.is_empty() {
            return;
        }
        w.step();
    }
    panic!("jobs never finished");
}

/// The shipped guns are firearms with their rounds among the items, and mistakes in a gun's
/// data are refused by name.
#[test]
fn guns_are_data_and_mistakes_are_refused() {
    let w = weapons();
    let guns: Vec<&WeaponDef> = w.iter().filter(|d| d.ranged.is_some()).collect();
    assert_eq!(guns.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["pistol", "hunting_rifle", "shotgun"]);
    let all = items::parse_items(ITEMS, &w).unwrap();
    for g in &guns {
        assert_eq!(g.category, items::WeaponCategory::Firearm);
        assert_eq!(g.spawn_weight, 0.0, "{}: guns come from loot tables, not floors", g.id);
        assert!(g.noise_radius >= 100.0, "{}: a shot is very loud", g.id);
        let item = all.iter().position(|d| d.weapon.as_deref() == Some(g.id.as_str())).unwrap() as u16;
        let h = holds(&all, &w, item).unwrap();
        assert_eq!((h.capacity, h.magazine), (g.ranged.as_ref().unwrap().capacity, false));
        assert!(all[h.ammo as usize].stack > 1, "{}: loose rounds stack", g.id);
    }
    let mag = all.iter().position(|d| d.id == "pistol_magazine").unwrap() as u16;
    assert_eq!(holds(&all, &w, mag).map(|h| (h.capacity, h.magazine)), Some((12, true)));
    assert_eq!(holds(&all, &w, all.iter().position(|d| d.id == "baseball_bat").unwrap() as u16), None);

    let bad = |edit: &dyn Fn(&mut Vec<WeaponDef>), want: &str| {
        let mut defs = weapons();
        edit(&mut defs);
        let err = items::validate_weapons(&defs).expect_err(want);
        assert!(err.contains(want), "{err:?} should say {want:?}");
    };
    let bad_gun = |edit: &dyn Fn(&mut RangedDef), want: &str| bad(&|d| edit(d.iter_mut().find(|x| x.id == "pistol").unwrap().ranged.as_mut().unwrap()), want);
    bad(&|d| d.iter_mut().find(|x| x.id == "pistol").unwrap().ranged = None, "pistol: a Firearm needs");
    bad(&|d| d.iter_mut().find(|x| x.id == "crowbar").unwrap().ranged = weapons().into_iter().find_map(|x| x.ranged), "crowbar: `ranged` is only for Firearm");
    bad_gun(&|r| r.capacity = 0, "capacity");
    bad_gun(&|r| r.range = 0.0, "range");
    bad_gun(&|r| r.spread_degrees = 40.0, "spread");
    bad_gun(&|r| r.pellets = 0, "pellets");
    bad_gun(&|r| r.far_damage = 1.5, "far_damage");
    bad_gun(&|r| r.round_seconds = 0.0, "round_seconds");
    bad_gun(&|r| r.magazine = Some("pistol_rounds".into()), "magazine");
    bad(&|d| d.iter_mut().find(|x| x.id == "crowbar").unwrap().noise_radius = 150.0, "noise_radius");
    assert!(ron::from_str::<Vec<WeaponDef>>(&WEAPONS.replace("pellets: 8,", "pellets: 8, calibre: 12,")).is_err(), "unknown fields are refused");

    // A gun's item without its rounds, or with a magazine that stacks.
    let without = |id: &str| all.iter().filter(|d| d.id != id).cloned().collect::<Vec<_>>();
    assert!(items::validate_items(&without("rifle_rounds"), &w).unwrap_err().contains("hunting_rifle: its ammo"));
    assert!(items::validate_items(&without("pistol_magazine"), &w).unwrap_err().contains("pistol: its magazine"));
    let mut stacked = all.clone();
    stacked.iter_mut().find(|d| d.id == "pistol_magazine").unwrap().stack = 5;
    assert!(items::validate_items(&stacked, &w).unwrap_err().contains("must not stack"));
}

/// What stops a shot on the map: walls and shut doors; a window only outside its opening (and its
/// glass breaks); a fence only what flies low; a tree trunk; the ground.
#[test]
fn the_map_stops_a_shot_as_it_would() {
    let mut m = TileMap::new(4, 4);
    let from = Vec2::new(10.5, 10.5);
    let level = |m: &TileMap| flight(m, from, Vec2::X, 40.0, 0.0);
    assert_eq!(level(&m).end, 40.0, "open ground: it flies its range");
    // The ground stops a shot aimed down: 1.4 m up, falling 0.1 m a metre.
    assert!((flight(&m, from, Vec2::X, 40.0, -0.1).end - 14.0).abs() < 1e-3);

    let at = IVec2::new(20, 10);
    for (edge, stops) in [(Edge::Wall, true), (Edge::DoorClosed, true), (Edge::DoorOpen, false), (Edge::DoorBroken, false), (Edge::Arch, false), (Edge::WindowOpen, false), (Edge::WindowBroken, false), (Edge::Fence, false)] {
        m.set_edge(at, Side::W, edge);
        let f = level(&m);
        assert_eq!(f.end < 40.0, stops, "{edge:?} at muzzle height");
        if stops {
            assert!((f.end - 9.5).abs() < 1e-3, "{edge:?} stops it at the edge: {}", f.end);
        }
        assert!(f.glass().is_empty());
    }
    // A fence stops what flies under a metre; the wall under a sill and over a window's head too.
    m.set_edge(at, Side::W, Edge::Fence);
    assert!((flight(&m, from, Vec2::X, 40.0, -0.06).end - 9.5).abs() < 1e-3, "0.83 m up at the fence");
    m.set_edge(at, Side::W, Edge::WindowOpen);
    assert!((flight(&m, from, Vec2::X, 40.0, -0.08).end - 9.5).abs() < 1e-3, "under the sill");
    assert!((flight(&m, from, Vec2::X, 40.0, 0.1).end - 9.5).abs() < 1e-3, "over the head");
    m.set_edge(at, Side::W, Edge::DoorOpen);
    assert!((flight(&m, from, Vec2::X, 40.0, 0.1).end - 9.5).abs() < 1e-3, "over a doorway");
    // Shut glass lets it through and is listed, nearest first.
    m.set_edge(at, Side::W, Edge::Window);
    m.set_edge(IVec2::new(25, 10), Side::W, Edge::Window);
    let f = level(&m);
    assert_eq!(f.end, 40.0);
    assert_eq!(f.glass().iter().map(|g| (g.0, g.1)).collect::<Vec<_>>(), vec![(9.5, EdgeRef::of(at, Side::W)), (14.5, EdgeRef::of(IVec2::new(25, 10), Side::W))]);
    // A trunk in the way stops it at the bark; one beside the line doesn't.
    let mut m = TileMap::new(4, 4);
    m.tile_mut(IVec2::new(18, 10)).unwrap().feature = Feature::Tree;
    assert!((level(&m).end - (8.0 - TREE_RADIUS)).abs() < 1e-3);
    assert_eq!(flight(&m, from + Vec2::new(0.0, 0.4), Vec2::X, 40.0, 0.0).end, 40.0, "0.4 m off the trunk's middle");
    // Furniture doesn't stop it; the edge of the map does.
    let mut m = TileMap::new(4, 4);
    m.tile_mut(IVec2::new(18, 10)).unwrap().feature = Feature::Furniture;
    assert_eq!(level(&m).end, 40.0);
    assert!((flight(&m, from, -Vec2::X, 40.0, 0.0).end - 10.5).abs() < 1e-3);
}

/// A rifle round kills or floors a zombie 30 m off and uses one round; a zombie a metre off the
/// line isn't touched, nor one behind the shooter.
#[test]
fn a_shot_hits_what_is_on_its_line() {
    let (mut w, id) = range(1, "hunting_rifle");
    let on = zombie(&mut w, 30.0, 0.0);
    let off = zombie(&mut w, 30.0, 1.0);
    let behind = zombie(&mut w, -5.0, 0.0);
    let hp: Vec<f32> = w.zombies.hp.clone();
    assert_eq!(rounds(&w, id), 5);
    // At the chest: level, 1.4 m up, is close under the head.
    w.set_pitch(id, pitch_for(1.1, 30.0));
    let hits = shoot(&mut w, id);
    assert_eq!(rounds(&w, id), 4, "one round a shot");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].zombie, w.zombies.id[on]);
    assert!(hits[0].damage >= 85.0 * 0.85 && !hits[0].crit, "a body shot: {:?}", hits[0]);
    assert!(w.zombies.hp[on] < hp[on]);
    assert_eq!((w.zombies.hp[off], w.zombies.hp[behind]), (hp[off], hp[behind]));
    // The event for the client: the line it flew, from the muzzle to where it stopped.
    let shot = w.combat_events.iter().find(|e| e.kind == CombatEventKind::Shot).expect("a shot event");
    assert_eq!(shot.player, id);
    assert!((shot.pos - shot.dir * shot.damage).distance(ORIGIN) < 1e-3, "{shot:?}");
}

/// The rifle goes through two in a line (the second taking less), a pistol stops in the first.
#[test]
fn a_rifle_round_goes_through_two() {
    for (gun, through) in [("hunting_rifle", 2), ("pistol", 1)] {
        let (mut w, id) = range(2, gun);
        for east in [8.0, 9.0, 10.0] {
            zombie(&mut w, east, 0.0);
        }
        let hits = shoot(&mut w, id);
        assert_eq!(hits.len(), through, "{gun}: {hits:?}");
        assert_eq!(hits[0].zombie, w.zombies.id[0], "nearest first");
        let end = w.combat_events.iter().find(|e| e.kind == CombatEventKind::Shot).unwrap().damage;
        assert!((end - (7.7 + through as f32 - 1.0)).abs() < 0.05, "{gun}: it stops in the last body it can enter: {end}");
    }
}

/// Pitch matters (so a first-person camera needs nothing new): aimed at the head it is a crit,
/// over the head it misses, level it passes over a zombie on the ground and aimed down it hits.
#[test]
fn pitch_decides_head_body_or_miss() {
    let at = |pitch: f32, down: bool| {
        let (mut w, id) = range(3, "hunting_rifle");
        let z = zombie(&mut w, 10.0, 0.0);
        if down {
            w.zombies.state[z] = ZombieState::Down;
            w.zombies.timer[z] = 30.0;
        }
        w.set_pitch(id, pitch);
        shoot(&mut w, id)
    };
    let body = at(0.0, false);
    assert!(body.len() == 1 && !body[0].crit, "level: the chest: {body:?}");
    let head = at(pitch_for(1.65, 10.0), false);
    assert!(head.len() == 1 && head[0].crit, "the head: {head:?}");
    assert!(head[0].damage > body[0].damage * 1.5, "a head shot does far more: {} against {}", head[0].damage, body[0].damage);
    assert!(at(pitch_for(2.3, 10.0), false).is_empty(), "over its head");
    assert!(at(pitch_for(-0.5, 10.0), false).is_empty(), "into the ground in front of it");
    assert!(at(0.0, true).is_empty(), "level passes over one lying down");
    let low = at(pitch_for(0.15, 10.0), true);
    assert!(low.len() == 1 && !low[0].crit, "aimed down at it: {low:?}");
    // What the client sends is clamped, and nonsense is level.
    let (mut w, id) = range(3, "pistol");
    w.set_pitch(id, 9.0);
    assert_eq!(w.player(id).unwrap().pitch, MAX_PITCH);
    w.set_pitch(id, f32::NAN);
    assert_eq!(w.player(id).unwrap().pitch, 0.0);
}

/// Walls stop bullets; a shut window breaks and lets the same bullet on to the zombie behind it.
#[test]
fn walls_stop_bullets_and_glass_breaks() {
    let (mut w, id) = range(4, "hunting_rifle");
    let z = zombie(&mut w, 12.0, 0.0);
    let hp = w.zombies.hp[z];
    let edge = IVec2::new(106, 100);
    w.map.set_edge(edge, Side::W, Edge::Wall);
    w.map_changed();
    assert!(shoot(&mut w, id).is_empty(), "the wall takes it");
    assert_eq!(w.zombies.hp[z], hp);
    w.map.set_edge(edge, Side::W, Edge::Window);
    w.map_changed();
    let hits = shoot(&mut w, id);
    assert_eq!(hits.len(), 1, "through the glass: {hits:?}");
    assert_eq!(w.map.edge(edge, Side::W), Edge::WindowBroken, "and the glass is gone");
    // A wall beyond the zombie doesn't matter; glass beyond where the bullet stopped stays whole.
    let (mut w, id) = range(4, "pistol");
    zombie(&mut w, 4.0, 0.0);
    w.map.set_edge(IVec2::new(108, 100), Side::W, Edge::Window);
    w.map_changed();
    assert_eq!(shoot(&mut w, id).len(), 1);
    assert_eq!(w.map.edge(IVec2::new(108, 100), Side::W), Edge::Window);
}

/// A shotgun's pellets: several land on a zombie close in front, spread over a row of them a
/// little further off, and do little at the end of their range.
#[test]
fn a_shotgun_sprays_pellets() {
    let (mut w, id) = range(5, "shotgun");
    zombie(&mut w, 3.0, 0.0);
    let close = shoot(&mut w, id);
    assert!(close.len() >= 6, "nearly all eight pellets at 3 m: {}", close.len());
    assert!(close.iter().map(|h| h.damage).sum::<f32>() >= 90.0, "which is the end of most zombies");
    let (mut w, id) = range(5, "shotgun");
    for north in [-1.2, -0.6, 0.0, 0.6, 1.2] {
        zombie(&mut w, 12.0, north);
    }
    let mut struck = std::collections::BTreeSet::new();
    let mut far = Vec::new();
    for _ in 0..3 {
        let hits = shoot(&mut w, id);
        struck.extend(hits.iter().map(|h| h.zombie));
        far.extend(hits);
    }
    assert!(struck.len() >= 3, "pellets spread over the row: {struck:?}");
    let mean = |h: &[HitEvent]| h.iter().filter(|h| !h.crit).map(|h| h.damage).sum::<f32>() / h.iter().filter(|h| !h.crit).count().max(1) as f32;
    assert!(mean(&far) < mean(&close) * 0.9, "weaker further off: {} against {}", mean(&far), mean(&close));
}

/// Moving makes a pistol less true: standing, every shot at 12 m is on a zombie; walking, some miss.
#[test]
fn moving_spreads_the_shots() {
    let hits = |moving: bool| {
        let (mut w, id) = range(6, "pistol");
        w.set_dev(id, Some(true), None);
        let mut n = 0;
        for k in 0..12 {
            // A fresh zombie each time, 12 m dead ahead of wherever the shooter has got to.
            let me = w.player(id).unwrap().state.pos;
            let z = w.add_zombie(me + Vec2::new(12.0, 0.0), std::f32::consts::PI, 0.0);
            w.zombies.wander_timer[z] = 1e9;
            let dir = if moving { Vec2::new(0.0, if k % 2 == 0 { 1.0 } else { -1.0 }) } else { Vec2::ZERO };
            w.set_player_input(id, k + 1, PlayerInput { move_dir: dir, facing: 0.0, aim: true, ..Default::default() });
            n += shoot(&mut w, id).len();
            let j = w.zombies.index_of(w.zombies.id[z]).unwrap();
            w.zombies.pos[j] = Vec2::new(20.5, 20.5);
        }
        n
    };
    let (still, moving) = (hits(false), hits(true));
    assert_eq!(still, 12, "standing still a pistol holds a body at 12 m");
    assert!(moving < still, "on the move it doesn't: {moving}");
}

/// Reloading: a carried magazine is swapped in whole (the one taken out keeps what it held); with
/// none, loose rounds go in one at a time until it is full; shooting stops it part way.
#[test]
fn reloading_swaps_a_magazine_or_pushes_rounds_in() {
    let (mut w, id) = range(7, "pistol");
    for _ in 0..3 {
        shoot(&mut w, id);
    }
    assert_eq!(rounds(&w, id), 9);
    // Nothing to reload with.
    assert!(w.player_reload(id).unwrap_err().contains("no pistol round"));
    assert!(w.give_item(id, "pistol_magazine", 1));
    assert_eq!(w.player(id).unwrap().inventory.carried[0].used, 12, "a magazine made now is full");
    w.player_reload(id).unwrap();
    w.player_reload(id).unwrap();
    assert_eq!(w.player(id).unwrap().jobs.len(), 1, "asked twice, reloaded once");
    let before = w.tick;
    settle_jobs(&mut w, id);
    assert!(((w.tick - before) as f32 * TICK_DT - 1.8).abs() < 0.11, "a magazine change takes its 1.8 s");
    assert_eq!(rounds(&w, id), 12);
    assert_eq!(w.player(id).unwrap().inventory.carried[0].used, 9, "the old magazine, with what was left in it");
    assert!(w.item_notes.iter().any(|n| n.ok && n.text == "reloaded the pistol: 12 of 12"), "{:?}", w.item_notes);
    assert!(w.player_reload(id).unwrap_err().contains("full"));

    // Loose rounds: into the gun one by one, each its own step, until it's full.
    for _ in 0..5 {
        shoot(&mut w, id);
    }
    assert!(w.give_item(id, "pistol_rounds", 20));
    w.player_reload(id).unwrap();
    // (The magazine in the pocket holds 9, more than the gun's 7: that goes in first, then rounds.)
    settle_jobs(&mut w, id);
    assert_eq!(rounds(&w, id), 12);
    assert_eq!(carried(&w, id, "pistol_rounds"), 17, "9 from the magazine swapped in, 3 loose rounds on top");
    // The spare magazine (7 in it now) is filled by using it.
    let mag = w.player(id).unwrap().inventory.carried.iter().position(|s| s.item == w.item_index("pistol_magazine").unwrap()).unwrap();
    w.player_use(id, Place::Carried, mag as u32).unwrap();
    settle_jobs(&mut w, id);
    assert_eq!(w.player(id).unwrap().inventory.carried.iter().find(|s| s.item == w.item_index("pistol_magazine").unwrap()).unwrap().used, 12);
    assert_eq!(carried(&w, id, "pistol_rounds"), 12);
    assert!(w.player_fill_magazine(id).unwrap_err().contains("full"));

    // A shotgun has no magazine: shells one at a time, and a shot part way keeps what went in.
    let (mut w, id) = range(7, "shotgun");
    for _ in 0..5 {
        shoot(&mut w, id);
    }
    assert_eq!(rounds(&w, id), 0);
    assert!(w.give_item(id, "shotgun_shells", 3));
    // Using the gun in hand is reloading it.
    w.player_use(id, Place::Slot(Slot::Hand), 0).unwrap();
    for _ in 0..((0.6 * 2.0 / TICK_DT) as usize + 2) {
        w.step();
    }
    assert_eq!(rounds(&w, id), 2, "two shells in after 1.2 s");
    assert_eq!(w.player(id).unwrap().jobs.len(), 1, "the third on its way");
    shoot(&mut w, id);
    assert!(w.player(id).unwrap().jobs.is_empty(), "shooting drops the reload");
    assert_eq!((rounds(&w, id), carried(&w, id, "shotgun_shells")), (1, 1));
    // Three shells for five chambers: it stops when they run out, and says how full it is.
    w.player_reload(id).unwrap();
    settle_jobs(&mut w, id);
    assert_eq!((rounds(&w, id), carried(&w, id, "shotgun_shells")), (2, 0));
    assert!(w.item_notes.iter().any(|n| n.text == "loaded the shotgun: 2 of 5"), "{:?}", w.item_notes);
}

/// The trigger on an empty gun reloads it (and doesn't shoot); with nothing to load, it says so.
#[test]
fn an_empty_gun_reloads_instead_of_shooting() {
    let (mut w, id) = range(8, "hunting_rifle");
    zombie(&mut w, 10.0, 0.0);
    for _ in 0..5 {
        shoot(&mut w, id);
    }
    assert_eq!(rounds(&w, id), 0);
    assert!(!w.player_attack(id), "no shot");
    assert!(w.player(id).unwrap().action.is_none());
    assert!(w.item_notes.iter().any(|n| !n.ok && n.text.contains("no rifle round")), "{:?}", w.item_notes);
    assert!(w.give_item(id, "rifle_rounds", 2));
    assert!(!w.player_attack(id));
    assert!(!w.player_attack(id), "pulled again while reloading: it carries on");
    assert_eq!(w.player(id).unwrap().jobs, vec![crate::inventory::ItemJob { kind: JobKind::Reload { spare: false }, left: 0.9 }]);
    settle_jobs(&mut w, id);
    assert_eq!(rounds(&w, id), 2);
    zombie(&mut w, 12.0, 0.0);
    assert!(!shoot(&mut w, id).is_empty());
    // A melee weapon in hand is not something to reload.
    let (mut w, id) = range(8, "baseball_bat");
    assert!(w.player_reload(id).unwrap_err().contains("isn't something to reload"));
}

/// A shot is heard far off: zombies 100 m away turn to it at once, and it leaves an attractor
/// that draws the far horde (migration); a bat's swing does neither.
#[test]
fn a_gunshot_wakes_the_street_and_draws_the_horde() {
    for (weapon, loud) in [("hunting_rifle", true), ("baseball_bat", false)] {
        let (mut w, id) = range(9, weapon);
        // Heard, for this one.
        w.set_dev(id, None, Some(false));
        let far: Vec<usize> = [(100.0, 0.0), (-90.0, 30.0), (20.0, -95.0)].iter().map(|&(e, n)| {
            let z = w.add_zombie(ORIGIN + Vec2::new(e, n), Vec2::new(e, n).to_angle(), 0.0);
            w.zombies.wander_timer[z] = 1e9;
            z
        }).collect();
        w.step();
        assert!(far.iter().all(|&z| w.zombies.state[z] == ZombieState::Wander), "{weapon}: nobody has noticed anything yet");
        shoot(&mut w, id);
        w.step();
        let coming = far.iter().filter(|&&z| w.zombies.state[z] != ZombieState::Wander && w.zombies.target_pos[z].distance(ORIGIN) < 1.0).count();
        assert_eq!(coming, if loud { 3 } else { 0 }, "{weapon}");
        assert_eq!(w.population().attractors.len(), loud as usize, "{weapon}");
        if loud {
            let a = &w.population().attractors[0];
            assert!(a.pos.distance(ORIGIN) < 1.0 && a.reach > 300.0, "it pulls from far beyond earshot: {a:?}");
        }
    }
}

/// Lag compensation (#23): a client 5 ticks behind shoots at a zombie crossing its line where its
/// screen shows it, and hits; the same shot from a player with no lag misses (the zombie has
/// moved on), and with compensation off it misses too.
#[test]
fn a_shot_lands_where_the_shooter_saw_the_target() {
    const LAG: u64 = 5;
    let run = |reports: bool, cap: f32| {
        let (mut w, id) = range(10, "hunting_rifle");
        w.config.combat.lag_comp_max_secs = cap;
        let z = zombie(&mut w, 15.0, 0.0);
        // It walks north across the line of fire at 2 m/s; `k` is the step about to run.
        let at = |k: i64| ORIGIN + Vec2::new(15.0, 0.1 * k as f32);
        for k in -40..=0 {
            w.zombies.pos[z] = at(k);
            if k == 0 {
                // Aim where the client's screen shows it: where it stood `LAG` ticks ago.
                let seen = at(-(LAG as i64)) - ORIGIN;
                w.set_player_input(id, 1, PlayerInput { facing: seen.to_angle(), aim: true, ..Default::default() });
                assert!(w.player_attack(id));
            }
            if reports {
                w.player_saw(id, w.tick.saturating_sub(LAG));
            }
            w.step();
            if k < 0 {
                assert!(w.hit_events.is_empty());
            } else {
                assert_eq!(w.action_events.len(), 1, "the shot goes in the tick the trigger is pulled");
            }
        }
        w.hit_events.len()
    };
    assert_eq!(run(true, 0.35), 1, "where its client saw the zombie");
    assert_eq!(run(false, 0.35), 0, "a local player sees no lag: the zombie has walked out of the line");
    assert_eq!(run(true, 0.0), 0, "compensation off");
}

/// Two survivors `apart` metres from each other on the range, the first with `gun`.
fn standoff(seed: u64, gun: &str, pvp: bool, apart: f32) -> (World, u32, u32) {
    let (mut w, a) = range(seed, gun);
    w.config.combat.pvp = pvp;
    let b = w.add_player(ORIGIN + Vec2::new(apart, 0.0));
    (w, a, b)
}

/// With PvP on a bullet wounds another survivor on the part at the height it struck, a gash
/// clothing doesn't stop; with it off it passes through them. Spawn protection holds for shots.
#[test]
fn bullets_hit_other_players_only_with_pvp_on() {
    let (mut w, a, b) = standoff(11, "hunting_rifle", false, 10.0);
    let z = zombie(&mut w, 14.0, 0.0);
    let hits = shoot(&mut w, a);
    assert_eq!(hits.len(), 1, "co-op: through the friend, into the zombie behind");
    assert_eq!(hits[0].zombie, w.zombies.id[z]);
    assert_eq!(w.player(b).unwrap().hp, 100.0);

    let shot_at = |pitch: f32, protected: bool, jacket: bool| {
        let (mut w, a, b) = standoff(11, "pistol", true, 10.0);
        if protected {
            w.protect_player(b, 5.0);
        }
        if jacket {
            assert!(w.give_item(b, "leather_jacket", 1));
            w.player_wear(b, 0).unwrap();
            settle_jobs(&mut w, b);
        }
        w.set_pitch(a, pitch);
        w.player_attack(a);
        w.step();
        (w.player_hit_events.clone(), w.player(b).unwrap().clone())
    };
    let (hits, victim) = shot_at(0.0, false, true);
    assert_eq!(hits.len(), 1);
    let h = hits[0];
    assert!(matches!(h.part, Some(Part::Torso | Part::LeftArm | Part::RightArm)), "chest height: {:?}", h.part);
    assert_eq!(h.wound, Some(crate::WoundKind::Gash), "a leather jacket stops no bullet");
    assert!((34.0 * 0.7 * 0.6..=46.0 * 0.7).contains(&h.damage) && !h.crit, "{h:?}");
    assert!((victim.hp - (100.0 - h.damage)).abs() < 0.01 && victim.health.wounds.len() == 1);
    let (hits, _) = shot_at(pitch_for(1.65, 10.0), false, false);
    assert!(hits.len() == 1 && hits[0].part == Some(Part::Head) && hits[0].crit, "{hits:?}");
    let (hits, _) = shot_at(pitch_for(0.4, 10.0), false, false);
    assert!(hits.len() == 1 && matches!(hits[0].part, Some(Part::LeftLeg | Part::RightLeg)), "{hits:?}");
    let (hits, victim) = shot_at(0.0, true, false);
    assert!(hits.is_empty() && victim.hp == 100.0, "spawn protection");
}

/// A survivor shot dead says so, naming the gun.
#[test]
fn a_death_by_gunshot_names_the_gun() {
    let (mut w, a, b) = standoff(12, "hunting_rifle", true, 6.0);
    w.set_pitch(a, pitch_for(1.65, 6.0));
    for _ in 0..5 {
        if w.player(b).unwrap().dead {
            break;
        }
        shoot(&mut w, a);
    }
    let victim = w.player(b).unwrap();
    assert!(victim.dead);
    let death = victim.run.death.as_ref().unwrap();
    assert!(matches!(death.cause, DeathCause::Survivor { by, stomp: false, part: Part::Head, .. } if by == a), "{:?}", death.cause);
    assert_eq!(death.describe_with(&w.config.weapons), "Shot by a survivor with a hunting rifle: hit in the head");
    assert_eq!(w.player(a).unwrap().run.player_kills, 1);
}

/// A gunfight is the same every time, and a world saved in the middle of one (rounds gone, a
/// reload under way, an attractor) loads to the same state and the same future.
#[test]
fn gunfights_are_deterministic_and_survive_a_save() {
    let run = |save_at: Option<u64>| {
        let (mut w, id) = range(13, "pistol");
        w.set_dev(id, Some(true), Some(false));
        assert!(w.give_item(id, "pistol_rounds", 40));
        for k in 0..12 {
            let a = k as f32 * 0.52;
            let z = w.add_zombie(ORIGIN + Vec2::from_angle(a) * (9.0 + k as f32), a + 3.1, 1.0);
            w.zombies.wander_timer[z] = 1e9;
        }
        let mut hashes = Vec::new();
        for t in 0..600u64 {
            if Some(t) == save_at {
                // (The dev toggles aren't saved, and are in the hash while on.)
                let mut loaded = World::load(&w.save(), &w.config).expect("loads");
                loaded.set_dev(id, Some(true), Some(false));
                assert_eq!(loaded.state_hash(), w.state_hash(), "tick {t}");
                w = loaded;
            }
            let nearest = (0..w.zombies.len()).filter(|&j| w.zombies.state[j] != ZombieState::Dead).min_by(|&i, &j| w.zombies.pos[i].distance(ORIGIN).total_cmp(&w.zombies.pos[j].distance(ORIGIN)));
            if let Some(j) = nearest {
                let to = w.zombies.pos[j] - w.player(id).unwrap().state.pos;
                w.set_player_input(id, t as u32 + 1, PlayerInput { facing: to.to_angle(), aim: true, ..Default::default() });
                w.set_pitch(id, pitch_for(1.2, to.length().max(1.0)));
                if t % 9 == 0 {
                    w.player_attack(id);
                }
            }
            w.step();
            hashes.push(w.state_hash());
        }
        let p = w.player(id).unwrap();
        assert!(p.run.kills >= 3, "the pistol did some work: {} kills", p.run.kills);
        assert!(carried(&w, id, "pistol_rounds") < 40, "and was reloaded from the pocket");
        hashes
    };
    let plain = run(None);
    assert_eq!(plain, run(None), "same seed, same fight");
    // 139 ticks in the first magazine is long gone and rounds are going in one by one.
    assert_eq!(plain, run(Some(139)), "saved and loaded mid-reload");
    assert_eq!(plain, run(Some(300)));
}

/// Rounds in guns and magazines are checked on load: more than it takes is refused.
#[test]
fn an_overfull_gun_is_refused_on_load() {
    let (mut w, id) = range(14, "pistol");
    assert!(World::load(&w.save(), &w.config).is_ok());
    let pi = w.players.iter().position(|p| p.id == id).unwrap();
    w.players[pi].inventory.hand.as_mut().unwrap().used = 13;
    match World::load(&w.save(), &w.config) {
        Err(crate::LoadError::Invalid(why)) => assert!(why.contains("goes had"), "{why}"),
        other => panic!("an overfull gun loaded: {:?}", other.map(|_| ())),
    }
}

/// Guns turn up in the town's containers, rarely, with a seeded number of rounds in them (the
/// same whichever order things are opened in), and never lie on a floor at the start.
#[test]
fn guns_are_rare_loot_with_some_rounds_in() {
    let data = |text: &str| text.to_string();
    let weapons = weapons();
    let all = items::parse_items(ITEMS, &weapons).unwrap();
    let containers = crate::containers::parse_containers(&data(include_str!("../../../../assets/data/containers.ron"))).unwrap();
    let loot = crate::loot::parse_loot(&data(include_str!("../../../../assets/data/loot.ron"))).unwrap();
    loot.check(&all, &containers).unwrap();
    let templates: Vec<BuildingTemplate> = [
        include_str!("../../../../assets/data/buildings/bungalow.ron"),
        include_str!("../../../../assets/data/buildings/family_two_storey.ron"),
        include_str!("../../../../assets/data/buildings/ranch_garage.ron"),
        include_str!("../../../../assets/data/buildings/office_small.ron"),
    ]
    .iter()
    .map(|t| BuildingTemplate::from_ron(t).unwrap())
    .collect();
    let w = World::new(WorldConfig::new(21, 0, templates).with_weapons(weapons).with_items(all).with_containers(containers).with_loot(loot));
    assert!(w.items().iter().all(|it| w.holds(it.stack.item).is_none()), "no gun lies on a floor");
    let (mut guns, mut stacks, mut loads) = (0, 0, std::collections::BTreeSet::new());
    for site in w.containers() {
        let def = w.container_def(site);
        let room = w.map.rooms.get(site.room as usize).map_or("", |r| r.room_type.as_str());
        let mut contents = crate::loot::fill(w.config.loot_seed(), site.id, def, room, &w.config.loot, &w.config.items);
        w.load_found(site.id, &mut contents);
        let mut again = contents.clone();
        w.load_found(site.id, &mut again);
        assert_eq!(contents, again, "the same however often it is asked");
        for s in &contents {
            stacks += 1;
            if let Some(h) = w.holds(s.item) {
                guns += 1;
                assert!(s.used <= h.capacity && s.count == 1);
                loads.insert((s.item, s.used));
            } else {
                assert!(s.used == 0 || w.config.items[s.item as usize].fuel_l.is_some());
            }
        }
    }
    assert!(guns >= 3, "a town has a few guns and magazines: {guns} in {stacks} stacks");
    assert!(guns * 60 < stacks, "but they are rare: {guns} in {stacks} stacks");
    assert!(loads.len() >= 3, "with different numbers of rounds in them: {loads:?}");
    let _ = HitEffect::Hit;
}
