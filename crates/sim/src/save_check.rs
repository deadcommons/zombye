//! Checks made while loading a save ([`crate::save`]): what a change in the game data means for it,
//! and whether the values it holds are ones the simulation can run.
//!
//! **Content changes** (`plan_content`). A save records a fingerprint of the data its world was
//! made from ([`ContentPrint`]). Loading against other data sorts each change:
//!
//! - *Compatible*, loaded with a note: a weapon's numbers changed (the new numbers apply), weapons
//!   added, removed or reordered when nothing in the save uses the removed ones (the save's weapon
//!   indices are matched up by id), the same for items (`items.ron`: weights, sizes and other numbers
//!   apply from now on; a load that no longer fits just slows you down), `containers.ron` changed
//!   while every container the save has opened is still a container (new room sizes and search
//!   times apply; unopened ones fill by the new rules), `population.ron` changed (its respawn and migration settings
//!   apply from now on; each region keeps the target it was saved with), and building templates that
//!   changed without changing the map they generate (or a save that stores its map whole).
//! - *Incompatible*, refused with [`LoadError::ContentChanged`]: templates that change the
//!   regenerated map (the save's door and window edits, positions and paths would be on another
//!   town), a weapon the save refers to (mid-swing) or an item it holds anywhere (on the floor, in an
//!   inventory, in a container, in a queued job) that is gone, and an opened container that isn't a
//!   container any more (its contents would be lost).
//!
//! [`LoadOptions::force`](crate::save::LoadOptions::force) ("load anyway") loads those too, doing its best and saying what it did:
//! stacks of a missing item are dropped (and jobs moving or using one), a swing with a missing
//! weapon is called off, the contents of containers that are gone are dropped; on a changed map, only the door and window edits that still fit are applied, damage
//! on edges that are no longer doors or windows is dropped, and players and zombies standing where
//! something is now built are moved to the nearest free ground. A map stored whole is kept as it is.
//!
//! **Validation** (`validate`, `check_settings`, `zombie_region`). A save with good checksums
//! can still hold values no tick could have produced (a buggy writer, or a crafted file): NaN or
//! infinite positions and timers, positions off the map, repeated or zero ids, indices past the end
//! of what they index (weapons, rooms, buildings, players, regions), negative hit points, or states
//! that can't go together (a living zombie with no hit points, a lunge on a cheaper tier). Each is
//! refused with [`LoadError::Invalid`] naming the field, so a loaded world never breaks a tick.

use glam::{IVec2, Vec2};

use crate::inventory::JobKind;
use crate::items::{GroundItem, ItemStack};
use crate::map::{Edge, TileMap};
use crate::players::Player;
use crate::save::{ContentPrint, LoadError};
use crate::tiers::{RegionIndex, settle};
use crate::world::{World, WorldConfig, edge_of_key};
use crate::zombies::{Tier, ZombieState};

/// What a change of content means for one save.
pub(crate) struct ContentPlan {
    /// Compatible changes, in words (warnings for whoever loads).
    pub notes: Vec<String>,
    /// The templates changed: this says how, for an error if the map they make differs.
    pub templates: Option<String>,
    /// The weapon ids the save's indices refer to.
    pub saved_weapons: Vec<String>,
    /// Saved weapon index → index in the content now (`None`: gone). `None` when nothing moved.
    pub weapon_map: Option<Vec<Option<u16>>>,
    /// The same for items.
    pub saved_items: Vec<String>,
    pub item_map: Option<Vec<Option<u16>>>,
    /// `containers.ron` changed.
    pub containers_changed: bool,
}

/// Compare the content a save was made with to the content now.
pub(crate) fn plan_content(saved: &ContentPrint, now: &ContentPrint) -> ContentPlan {
    let list = |ids: &[&String]| if ids.is_empty() { "none".to_string() } else { ids.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ") };
    let mut notes = Vec::new();
    let mut weapon_map = None;
    if saved.weapons != now.weapons {
        if saved.weapon_ids == now.weapon_ids {
            notes.push("weapons.ron changed (the same weapons, other numbers): the new numbers apply".to_string());
        } else {
            let added: Vec<&String> = now.weapon_ids.iter().filter(|id| !saved.weapon_ids.contains(id)).collect();
            let removed: Vec<&String> = saved.weapon_ids.iter().filter(|id| !now.weapon_ids.contains(id)).collect();
            notes.push(format!(
                "weapons.ron changed (added: {}; removed: {}): the save's weapons are matched up by id, and the numbers in the file apply",
                list(&added),
                list(&removed)
            ));
            weapon_map = Some(saved.weapon_ids.iter().map(|id| now.weapon_ids.iter().position(|n| n == id).map(|i| i as u16)).collect());
        }
    }
    let mut item_map = None;
    if saved.items != now.items {
        if saved.item_ids == now.item_ids {
            notes.push("items.ron changed (the same items, other numbers): the new numbers apply".to_string());
        } else {
            let added: Vec<&String> = now.item_ids.iter().filter(|id| !saved.item_ids.contains(id)).collect();
            let removed: Vec<&String> = saved.item_ids.iter().filter(|id| !now.item_ids.contains(id)).collect();
            notes.push(format!("items.ron changed (added: {}; removed: {}): the save's items are matched up by id, and the numbers in the file apply", list(&added), list(&removed)));
            item_map = Some(saved.item_ids.iter().map(|id| now.item_ids.iter().position(|n| n == id).map(|i| i as u16)).collect());
        }
    }
    let containers_changed = saved.containers != now.containers;
    if containers_changed {
        notes.push("containers.ron changed: opened containers keep what they hold under the new rules; unopened ones fill by them".to_string());
    }
    if saved.population != now.population {
        notes.push("population.ron changed: its respawn and migration settings apply from now on; each region keeps the zombie target it was saved with".to_string());
    }
    let templates = (saved.templates != now.templates).then(|| {
        let ids = |v: &[String]| list(&v.iter().collect::<Vec<_>>());
        if saved.template_ids == now.template_ids {
            format!("building templates changed (the same ones: {}), so the map would differ", ids(&now.template_ids))
        } else {
            format!("building templates changed (saved with {}; now {}), so the map would differ", ids(&saved.template_ids), ids(&now.template_ids))
        }
    });
    ContentPlan { notes, templates, saved_weapons: saved.weapon_ids.clone(), weapon_map, saved_items: saved.item_ids.clone(), item_map, containers_changed }
}

/// Every weapon index in the saved players' actions must name a weapon of the save's own list;
/// then they are moved to the content's list. (The weapon in hand follows from the item held and is
/// worked out again after loading.) A weapon that's gone is refused (the returned `blocking` line)
/// unless `force`, which calls off those swings (said in `notes`).
pub(crate) fn remap_weapons(plan: &ContentPlan, players: &mut [Player], force: bool, notes: &mut Vec<String>) -> Result<Option<String>, LoadError> {
    let n = plan.saved_weapons.len();
    for p in players.iter() {
        if let Some(a) = p.action
            && a.weapon as usize >= n
        {
            return Err(invalid(format!("player {}'s action: weapon index {} is out of range (the save lists {n} weapons)", p.id, a.weapon)));
        }
    }
    let Some(map) = &plan.weapon_map else { return Ok(None) };
    let gone = |i: u16| map[i as usize].is_none();
    let swinging: Vec<&Player> = players.iter().filter(|p| p.action.is_some_and(|a| gone(a.weapon))).collect();
    if !swinging.is_empty() {
        let mut missing: Vec<&str> = Vec::new();
        for p in &swinging {
            let id = plan.saved_weapons[p.action.expect("swinging").weapon as usize].as_str();
            if !missing.contains(&id) {
                missing.push(id);
            }
        }
        let ids = swinging.iter().map(|p| p.id.to_string()).collect::<Vec<_>>().join(", ");
        let what = format!("weapon {} is gone from weapons.ron but the save uses it (swung by player {ids})", missing.join(", "));
        if !force {
            return Ok(Some(what));
        }
        notes.push(format!("loaded anyway: {what}; called off those swings"));
    }
    for p in players.iter_mut() {
        if p.action.is_some_and(|a| gone(a.weapon)) {
            p.action = None;
        }
        if let Some(a) = &mut p.action {
            a.weapon = map[a.weapon as usize].expect("kept");
        }
    }
    Ok(None)
}

/// The item a job names, if any.
fn job_item(kind: &JobKind) -> Option<u16> {
    match *kind {
        JobKind::Move { item, .. } | JobKind::Use { item, .. } => Some(item),
        JobKind::Search { .. } => None,
    }
}

/// Every item index in the save (ground items, inventories, jobs, opened containers) must name an
/// item of the save's own list; then they are moved to the content's. An item that's gone is
/// refused (the returned `blocking` line) unless `force`, which drops those stacks and jobs.
pub(crate) fn remap_items(plan: &ContentPlan, ground: &mut Vec<GroundItem>, players: &mut [Player], opened: &mut [(u32, Vec<ItemStack>)], force: bool, notes: &mut Vec<String>) -> Result<Option<String>, LoadError> {
    let n = plan.saved_items.len();
    let range = |what: &dyn Fn() -> String, i: u16| if (i as usize) < n { Ok(()) } else { Err(invalid(format!("{} item index {i} is out of range (the save lists {n} items)", what()))) };
    for it in ground.iter() {
        range(&|| format!("ground item {}:", it.id), it.stack.item)?;
    }
    for p in players.iter() {
        for s in p.inventory.stacks() {
            range(&|| format!("player {}'s inventory:", p.id), s.item)?;
        }
        for j in &p.jobs {
            if let Some(i) = job_item(&j.kind) {
                range(&|| format!("player {}'s job:", p.id), i)?;
            }
        }
    }
    for (c, stacks) in opened.iter() {
        for s in stacks {
            range(&|| format!("container {c}:"), s.item)?;
        }
    }
    let Some(map) = &plan.item_map else { return Ok(None) };
    let gone = |i: u16| map[i as usize].is_none();
    let mut missing: Vec<&str> = Vec::new();
    let mut note = |i: u16| {
        if gone(i) && !missing.contains(&plan.saved_items[i as usize].as_str()) {
            missing.push(plan.saved_items[i as usize].as_str());
        }
    };
    ground.iter().for_each(|it| note(it.stack.item));
    for p in players.iter() {
        p.inventory.stacks().for_each(|s| note(s.item));
        p.jobs.iter().filter_map(|j| job_item(&j.kind)).for_each(&mut note);
    }
    opened.iter().flat_map(|(_, v)| v).for_each(|s| note(s.item));
    if !missing.is_empty() {
        let what = format!("item {} is gone from items.ron but the save has some (on the floor, carried or in a container)", missing.join(", "));
        if !force {
            return Ok(Some(what));
        }
        notes.push(format!("loaded anyway: {what}; dropped them"));
    }
    let to = |i: u16| map[i as usize];
    ground.retain(|it| !gone(it.stack.item));
    ground.iter_mut().for_each(|it| it.stack.item = to(it.stack.item).expect("kept"));
    for p in players.iter_mut() {
        let inv = &mut p.inventory;
        for slot in [&mut inv.hand, &mut inv.off_hand, &mut inv.back, &mut inv.shoulder] {
            *slot = slot.and_then(|s| to(s.item).map(|item| ItemStack { item, ..s }));
        }
        for list in [&mut inv.worn, &mut inv.carried] {
            list.retain(|s| !gone(s.item));
            list.iter_mut().for_each(|s| s.item = to(s.item).expect("kept"));
        }
        p.jobs.retain(|j| job_item(&j.kind).is_none_or(|i| !gone(i)));
        for j in &mut p.jobs {
            match &mut j.kind {
                JobKind::Move { item, .. } | JobKind::Use { item, .. } => *item = to(*item).expect("kept"),
                JobKind::Search { .. } => {}
            }
        }
    }
    for (_, list) in opened.iter_mut() {
        list.retain(|s| !gone(s.item));
        list.iter_mut().for_each(|s| s.item = to(s.item).expect("kept"));
    }
    Ok(None)
}

/// Every opened container must be a container of the map as loaded. One that isn't (the map or
/// `containers.ron` changed) is refused (the returned line) unless `force`, which drops it and what it
/// held; with the same content and map that can only be a damaged save. Players with a container
/// open that's gone have it closed.
pub(crate) fn fit_containers(w: &mut World, plan: &ContentPlan, new_map: bool, force: bool, notes: &mut Vec<String>) -> Result<Option<String>, LoadError> {
    let stale: Vec<u32> = w.opened.keys().copied().filter(|&c| w.container(c).is_none()).collect();
    if !stale.is_empty() {
        if !plan.containers_changed && !new_map {
            return Err(invalid(format!("opened container {} isn't a container of this map", stale[0])));
        }
        let held: usize = stale.iter().map(|c| w.opened[c].len()).sum();
        let what = format!("{} opened containers are no longer containers ({held} stacks in them)", stale.len());
        if !force {
            return Ok(Some(what));
        }
        notes.push(format!("loaded anyway: {what}; dropped them"));
        for c in stale {
            w.opened.remove(&c);
        }
    }
    for pi in 0..w.players.len() {
        if w.players[pi].open.is_some_and(|c| w.container(c).is_none()) {
            w.players[pi].open = None;
        }
    }
    Ok(None)
}

/// Can a saved map edit (`saved`, a door or window opened, shut or broken) stand on the edge as
/// generated (`generated`)? Doors stay doors and windows windows; nothing else is ever edited.
pub(crate) fn edit_fits(generated: Edge, saved: Edge) -> bool {
    let family = |e: Edge| match e {
        Edge::DoorClosed | Edge::DoorOpen | Edge::DoorBroken => 1,
        Edge::Window | Edge::WindowOpen | Edge::WindowBroken => 2,
        _ => 0,
    };
    family(generated) != 0 && family(generated) == family(saved)
}

/// After a forced load onto a map that isn't the one saved: drop damage on edges that aren't doors
/// or windows any more, and move players and zombies off ground that's now built on.
pub(crate) fn fit_to_new_map(w: &mut World, notes: &mut Vec<String>) {
    let before = w.edge_hp.len();
    let keys: Vec<_> = w.edge_hp.keys().copied().collect();
    for key in keys {
        let e = edge_of_key(key);
        if !w.map.in_bounds(e.tile) || w.max_hp(w.map.owned_edge(e.tile, e.edge)) <= 0.0 {
            w.edge_hp.remove(&key);
        }
    }
    let dropped_hp = before - w.edge_hp.len();
    let mut scratch = Vec::new();
    let blocked = |map: &TileMap, p: Vec2| p.is_finite() && map.in_bounds(p.floor().as_ivec2()) && map.tile_blocked(p.floor().as_ivec2());
    let mut moved_players = 0;
    for p in &mut w.players {
        if blocked(&w.map, p.state.pos) {
            p.state.pos = settle(&w.map, p.state.pos, &mut scratch);
            moved_players += 1;
        }
    }
    let mut moved_zombies = 0;
    for i in 0..w.zombies.len() {
        if blocked(&w.map, w.zombies.pos[i]) {
            w.zombies.pos[i] = settle(&w.map, w.zombies.pos[i], &mut scratch);
            w.zombies.vel[i] = Vec2::ZERO;
            moved_zombies += 1;
        }
    }
    let mut did = Vec::new();
    if dropped_hp > 0 {
        did.push(format!("dropped the damage of {dropped_hp} doors and windows that are gone"));
    }
    if moved_players + moved_zombies > 0 {
        did.push(format!("moved {moved_players} players and {moved_zombies} zombies off ground that is now built on"));
    }
    if !did.is_empty() {
        notes.push(format!("loaded anyway onto the new map: {}", did.join("; ")));
    }
}

/// The settings read from the save (as RON text, then as the config they make): every number
/// finite, and the few that size or divide things in range.
pub(crate) fn check_settings(text: &str, config: &WorldConfig) -> Result<(), LoadError> {
    if let Ok(value) = ron::from_str::<ron::Value>(text) {
        let mut path = Vec::new();
        if let Some(bad) = non_finite(&value, &mut path) {
            return Err(invalid(format!("settings: {bad}")));
        }
    }
    let g = &config.generation;
    if g.chunks.x < 1 || g.chunks.y < 1 || g.chunks.x > 256 || g.chunks.y > 256 {
        return Err(invalid(format!("settings: generation.chunks {} (1 to 256 a side)", g.chunks)));
    }
    if g.blocks.x < 1 || g.blocks.y < 1 || g.blocks.x > 32 || g.blocks.y > 32 {
        return Err(invalid(format!("settings: generation.blocks {} (1 to 32 a side)", g.blocks)));
    }
    if !config.day_length_secs.is_finite() || config.day_length_secs <= 0.0 {
        return Err(invalid(format!("settings: day_length_secs {} must be positive", config.day_length_secs)));
    }
    config.health.check().map_err(|e| invalid(format!("settings: {e}")))?;
    config.infection.check().map_err(|e| invalid(format!("settings: {e}")))?;
    config.needs.check().map_err(|e| invalid(format!("settings: {e}")))?;
    Ok(())
}

/// Food made this long before the world began is past any spoiling (game minutes, 10 years).
const OLDEST_FOOD_MIN: i32 = -10 * 365 * 24 * 60;

/// A stack's age and goes had (#11): goes below the item's uses, made no later than now and not
/// absurdly long before the world began. Run after the item index is known to be in range.
fn check_stack_state(w: &World, s: &ItemStack, what: &dyn Fn() -> String) -> Result<(), LoadError> {
    let uses = w.config.items[s.item as usize].consumable().map_or(1, |c| c.uses.max(1));
    if s.used >= uses {
        return Err(invalid(format!("{}: {} goes had of an item with {uses}", what(), s.used)));
    }
    if s.born > w.game_minute() + 1 || s.born < OLDEST_FOOD_MIN {
        return Err(invalid(format!("{}: made at game minute {}, now {}", what(), s.born, w.game_minute())));
    }
    Ok(())
}

/// After items.ron changed (a compatible change): goes had of a stack's top item stay below the
/// item's `uses` as it is now.
pub(crate) fn fit_goes(w: &mut World) {
    let items = &w.config.items;
    let fit = |s: &mut ItemStack| {
        let uses = items.get(s.item as usize).and_then(|d| d.consumable()).map_or(1, |c| c.uses.max(1));
        s.used = s.used.min(uses - 1);
    };
    for p in &mut w.players {
        p.inventory.stacks_mut().for_each(&fit);
    }
    w.items.iter_mut().for_each(|it| fit(&mut it.stack));
    w.opened.values_mut().flatten().for_each(&fit);
}

/// A player's needs and sleep: values a tick could have made.
fn check_needs(p: &Player) -> Result<(), LoadError> {
    let n = &p.needs;
    for (field, v) in [("hunger", n.hunger), ("thirst", n.thirst), ("tiredness", n.tiredness)] {
        if !(0.0..=1.0).contains(&v) {
            return Err(invalid(format!("player {}: needs.{field} is {v} (must be 0 to 1)", p.id)));
        }
    }
    non_negative(n.sore_h, || format!("player {}: needs.sore_h", p.id))?;
    if let Some(s) = n.sleep {
        non_negative(s.hours, || format!("player {}: sleep hours", p.id))?;
        if p.dead {
            return Err(invalid(format!("player {}: dead and asleep", p.id)));
        }
    }
    Ok(())
}

/// No tuning number in a save is bigger than this (a speed of a million m/s would carry a zombie
/// off the map, and beyond what positions can count, in a few minutes).
const MAX_SETTING: f64 = 1e6;
/// Fastest a saved zombie may walk, or be moving (m/s): far beyond anything the tunings give
/// (knockback is capped at `MAX_KNOCKBACK_SPEED`, 5 m/s).
const MAX_ZOMBIE_SPEED: f32 = 50.0;

/// The first non-finite (or absurdly big) float in a RON value, with its field path
/// ("zombie.sight_day is NaN").
fn non_finite(v: &ron::Value, path: &mut Vec<String>) -> Option<String> {
    match v {
        ron::Value::Number(n @ (ron::value::Number::F32(_) | ron::value::Number::F64(_))) => {
            let f = n.into_f64();
            let name = || if path.is_empty() { "a value".to_string() } else { path.join(".") };
            if !f.is_finite() {
                Some(format!("{} is {f}", name()))
            } else {
                (f.abs() > MAX_SETTING).then(|| format!("{} is {f} (at most {MAX_SETTING:e} either way)", name()))
            }
        }
        ron::Value::Map(m) => m.iter().find_map(|(k, v)| {
            path.push(match k {
                ron::Value::String(s) => s.clone(),
                other => format!("{other:?}"),
            });
            let found = non_finite(v, path);
            path.pop();
            found
        }),
        ron::Value::Seq(items) => items.iter().enumerate().find_map(|(i, v)| {
            path.push(i.to_string());
            let found = non_finite(v, path);
            path.pop();
            found
        }),
        ron::Value::Option(Some(v)) => non_finite(v, path),
        _ => None,
    }
}

/// One region's block of zombies: the region exists and each zombie stands in it, on the map.
pub(crate) fn zombie_region(region: u32, pos: &[Vec2], first_id: u32, regions: &RegionIndex, map: &TileMap) -> Result<(), LoadError> {
    if region as usize >= regions.regions() {
        return Err(invalid(format!("zombies: region {region} is out of range (the map has {})", regions.regions())));
    }
    for &p in pos {
        on_map(map, p, || format!("a zombie of region {region} (ids from {first_id}): pos"))?;
        if regions.region_of(p) != region as usize {
            return Err(invalid(format!("a zombie of region {region} (ids from {first_id}) stands in region {}", regions.region_of(p))));
        }
    }
    Ok(())
}

pub(crate) fn invalid(why: impl Into<String>) -> LoadError {
    LoadError::Invalid(why.into())
}

fn finite(v: f32, field: impl FnOnce() -> String) -> Result<(), LoadError> {
    if v.is_finite() { Ok(()) } else { Err(invalid(format!("{} is {v}", field()))) }
}

fn finite2(v: Vec2, field: impl FnOnce() -> String) -> Result<(), LoadError> {
    if v.is_finite() { Ok(()) } else { Err(invalid(format!("{} is ({}, {})", field(), v.x, v.y))) }
}

fn non_negative(v: f32, field: impl FnOnce() -> String) -> Result<(), LoadError> {
    if v.is_finite() && v >= 0.0 { Ok(()) } else { Err(invalid(format!("{} is {v} (must be 0 or more)", field()))) }
}

fn at_most(v: f32, max: f32, field: impl FnOnce() -> String) -> Result<(), LoadError> {
    if v <= max { Ok(()) } else { Err(invalid(format!("{} is {v} (at most {max})", field()))) }
}

fn on_map(map: &TileMap, p: Vec2, field: impl FnOnce() -> String) -> Result<(), LoadError> {
    if p.is_finite() && p.x >= 0.0 && p.y >= 0.0 && p.x < map.width() as f32 && p.y < map.height() as f32 {
        Ok(())
    } else {
        Err(invalid(format!("{} ({}, {}) is off the map ({} x {} m)", field(), p.x, p.y, map.width(), map.height())))
    }
}

fn tile_on_map(map: &TileMap, t: IVec2, field: impl FnOnce() -> String) -> Result<(), LoadError> {
    if map.in_bounds(t) { Ok(()) } else { Err(invalid(format!("{} {t} is off the map ({} x {} tiles)", field(), map.width(), map.height()))) }
}

/// A player's wounds and illnesses: numbers a tick could have made, ids in order.
fn check_health(p: &Player) -> Result<(), LoadError> {
    let (id, h) = (p.id, &p.health);
    let name = |field: &str| format!("player {id}: {field}");
    if h.wounds.len() > crate::health::MAX_WOUNDS {
        return Err(invalid(format!("player {id}: {} wounds (at most {})", h.wounds.len(), crate::health::MAX_WOUNDS)));
    }
    for (k, w) in h.wounds.iter().enumerate() {
        if w.id >= h.next_wound || (k > 0 && h.wounds[k - 1].id >= w.id) {
            return Err(invalid(format!("player {id}: wound id {} is repeated, out of order or past the next id {}", w.id, h.next_wound)));
        }
        let what = |f: &str| name(&format!("wound {}: {f}", w.id));
        non_negative(w.age_h, || what("age_h"))?;
        non_negative(w.healed_h, || what("healed_h"))?;
        non_negative(w.dressing_age_h, || what("dressing_age_h"))?;
        if !(w.heal_h.is_finite() && w.heal_h > 0.0) || w.healed_h >= w.heal_h {
            return Err(invalid(format!("{} {} (healed {}; must be above 0 and above what has healed)", what("heal_h"), w.heal_h, w.healed_h)));
        }
    }
    if let Some(i) = h.infection {
        non_negative(i.feverish_h, || name("infection.feverish_h"))?;
        finite(i.left_h, || name("infection.left_h"))?;
        if i.wound >= h.next_wound {
            return Err(invalid(format!("player {id}: infection from wound {} past the next id {}", i.wound, h.next_wound)));
        }
    }
    non_negative(h.food_poisoning_h, || name("food_poisoning_h"))?;
    if let Some(r) = h.rise_in {
        finite(r, || name("rise_in"))?;
        if !p.dead {
            return Err(invalid(name("alive and about to rise")));
        }
    }
    Ok(())
}

/// Everything a tick reads from a freshly loaded world (before its derived state is rebuilt).
pub(crate) fn validate(w: &World) -> Result<(), LoadError> {
    let map = &w.map;
    finite(w.clock_offset_hours, || "clock offset".into())?;
    // The map's own indices (a map stored whole is checked for tiles and rooms by `is_consistent`).
    let rooms = map.rooms.len();
    for (k, b) in map.buildings.iter().enumerate() {
        let upper = b.upper_floors.iter().map(|f| f.2).chain(b.upper_edges.iter().flat_map(|(_, e)| [e.rooms.0, e.rooms.1]));
        if let Some(room) = upper.into_iter().find(|&r| r as usize >= rooms) {
            return Err(invalid(format!("building {k}: an upper floor's room {room} is out of range ({rooms} rooms)")));
        }
    }
    if w.spawn_building >= map.buildings.len() && !(map.buildings.is_empty() && w.spawn_building == 0) {
        return Err(invalid(format!("spawn building {} is out of range ({} buildings)", w.spawn_building, map.buildings.len())));
    }
    on_map(map, w.player_spawn, || "player spawn".into())?;
    let size = IVec2::new(map.width(), map.height());
    if w.town_min.cmplt(IVec2::ZERO).any() || w.town_max.cmpgt(size).any() || w.town_min.cmpgt(w.town_max).any() {
        return Err(invalid(format!("town bounds {}..{} don't fit the map ({size})", w.town_min, w.town_max)));
    }
    if let Some(goal) = w.goal {
        tile_on_map(map, goal, || "goal".into())?;
    }

    // Players.
    let weapons = w.config.weapons.len();
    let items = w.config.items.len();
    let weapon = |what: &dyn Fn() -> String, i: u16| {
        if (i as usize) < weapons { Ok(()) } else { Err(invalid(format!("{}weapon index {i} is out of range ({weapons} weapons)", what()))) }
    };
    for (k, p) in w.players.iter().enumerate() {
        let id = p.id;
        let name = |field: &str| format!("player {id}: {field}");
        if id == 0 || id >= w.next_player_id || w.players[..k].iter().any(|o| o.id == id) {
            return Err(invalid(format!("player id {id} is zero, repeated or past the next id {}", w.next_player_id)));
        }
        on_map(map, p.state.pos, || name("pos"))?;
        finite(p.state.facing, || name("facing"))?;
        if !(0.0..=1.0).contains(&p.state.endurance) {
            return Err(invalid(format!("{} is {} (must be 0 to 1)", name("endurance"), p.state.endurance)));
        }
        non_negative(p.hp, || name("hp"))?;
        if !p.dead && p.hp <= 0.0 {
            return Err(invalid(name("alive with no hit points")));
        }
        if p.dead && (p.action.is_some() || p.queued.is_some()) {
            return Err(invalid(name("dead with an action under way")));
        }
        finite(p.ready_in, || name("ready_in"))?;
        finite2(p.input.move_dir, || name("input.move_dir"))?;
        finite(p.input.facing, || name("input.facing"))?;
        if let Some(i) = p.weapon {
            weapon(&|| name(""), i)?;
        }
        let stack = |what: &str, s: &ItemStack| {
            if (s.item as usize) >= items || s.count == 0 {
                Err(invalid(format!("player {id}: {what}: item {} x {} (items 0..{items}, at least one)", s.item, s.count)))
            } else {
                check_stack_state(w, s, &|| format!("player {id}: {what}"))
            }
        };
        let inv = &p.inventory;
        for (what, slot) in [("hand", inv.hand), ("off hand", inv.off_hand), ("back", inv.back), ("shoulder", inv.shoulder)] {
            if let Some(s) = slot {
                stack(what, &s)?;
                if s.count != 1 {
                    return Err(invalid(format!("player {id}: {what} holds {} items (one at a time)", s.count)));
                }
            }
        }
        inv.worn.iter().try_for_each(|s| stack("worn", s))?;
        inv.carried.iter().try_for_each(|s| stack("carried", s))?;
        if p.jobs.len() > 256 {
            return Err(invalid(format!("player {id}: {} jobs queued", p.jobs.len())));
        }
        for j in &p.jobs {
            if !(j.left.is_finite() && j.left >= 0.0 && j.left < 3600.0) {
                return Err(invalid(format!("player {id}: a job's time left is {}", j.left)));
            }
            if job_item(&j.kind).is_some_and(|i| i as usize >= items) {
                return Err(invalid(format!("player {id}: a job's item is out of range ({items} items)")));
            }
        }
        if p.dead && (!p.jobs.is_empty() || p.open.is_some()) {
            return Err(invalid(name("dead with things to do")));
        }
        if let Some(a) = p.action {
            finite(a.timer, || name("action.timer"))?;
            finite(a.power, || name("action.power"))?;
            weapon(&|| name("action: "), a.weapon)?;
        }
        check_health(p)?;
        check_needs(p)?;
    }

    // Items, in id order.
    for (k, it) in w.items.iter().enumerate() {
        let name = |field: &str| format!("item {}: {field}", it.id);
        if it.id == 0 || it.id >= w.next_item_id || (k > 0 && w.items[k - 1].id >= it.id) {
            return Err(invalid(format!("item id {} is zero, repeated, out of order or past the next id {}", it.id, w.next_item_id)));
        }
        if it.stack.item as usize >= items || it.stack.count == 0 {
            return Err(invalid(format!("{}item {} x {} (items 0..{items}, at least one)", name(""), it.stack.item, it.stack.count)));
        }
        check_stack_state(w, &it.stack, &|| format!("item {}", it.id))?;
        on_map(map, it.pos, || name("pos"))?;
        finite(it.angle, || name("angle"))?;
    }

    // Opened containers (each one a container of this map, checked by `fit_containers`).
    for (c, stacks) in &w.opened {
        if let Some(s) = stacks.iter().find(|s| s.item as usize >= items || s.count == 0) {
            return Err(invalid(format!("container {c}: item {} x {} (items 0..{items}, at least one)", s.item, s.count)));
        }
        stacks.iter().try_for_each(|s| check_stack_state(w, s, &|| format!("container {c}")))?;
    }

    // Door and window damage: only on doors and windows that can still be battered.
    for (&key, &hp) in &w.edge_hp {
        let e = edge_of_key(key);
        let name = || format!("edge damage at {} {}", e.tile, if key.2 == 0 { "north" } else { "west" });
        if key.2 > 1 {
            return Err(invalid(format!("{}: edge {} is neither north nor west", name(), key.2)));
        }
        tile_on_map(map, e.tile, name)?;
        let max = w.max_hp(map.owned_edge(e.tile, e.edge));
        if max <= 0.0 {
            return Err(invalid(format!("{}: a {:?} has no hit points", name(), map.owned_edge(e.tile, e.edge))));
        }
        if !(hp > 0.0 && hp <= max) {
            return Err(invalid(format!("{}: hp {hp} (must be above 0, at most {max})", name())));
        }
    }

    // Sounds waiting to be heard and those just heard.
    for (k, s) in w.sounds.iter().chain(&w.heard).enumerate() {
        let name = |field: &str| format!("sound {k}: {field}");
        finite2(s.pos, || name("pos"))?;
        finite2(s.lure, || name("lure"))?;
        non_negative(s.radius, || name("radius"))?;
    }

    // Population.
    let pop = &w.population;
    for (r, &t) in pop.target.iter().enumerate() {
        non_negative(t, || format!("population: region {r} target"))?;
    }
    for (k, a) in pop.attractors.iter().enumerate() {
        let name = |field: &str| format!("attractor {}: {field}", a.id);
        if a.id == 0 || a.id > pop.next_attractor || (k > 0 && pop.attractors[k - 1].id >= a.id) {
            return Err(invalid(format!("attractor id {} is zero, repeated, out of order or past the last id {}", a.id, pop.next_attractor)));
        }
        finite2(a.pos, || name("pos"))?;
        non_negative(a.reach, || name("reach"))?;
        if !(a.life_secs > 0.0 && a.life_secs.is_finite()) {
            return Err(invalid(format!("{} is {} (must be positive)", name("life_secs"), a.life_secs)));
        }
    }

    // Zombies (already in id order).
    let z = &w.zombies;
    for i in 0..z.len() {
        let id = z.id[i];
        let name = |field: &str| format!("zombie {id}: {field}");
        if id == 0 || id >= w.next_id || (i > 0 && z.id[i - 1] >= id) {
            return Err(invalid(format!("zombie id {id} is zero, repeated or past the next id {}", w.next_id)));
        }
        on_map(map, z.pos[i], || name("pos"))?;
        finite2(z.vel[i], || name("vel"))?;
        finite(z.facing[i], || name("facing"))?;
        non_negative(z.speed[i], || name("speed"))?;
        at_most(z.speed[i], MAX_ZOMBIE_SPEED, || name("speed"))?;
        at_most(z.vel[i].length(), MAX_ZOMBIE_SPEED, || name("vel"))?;
        finite2(z.wander[i], || name("wander"))?;
        at_most(z.wander[i].length(), MAX_ZOMBIE_SPEED, || name("wander"))?;
        finite(z.wander_timer[i], || name("wander_timer"))?;
        finite2(z.target_pos[i], || name("target_pos"))?;
        finite(z.timer[i], || name("timer"))?;
        finite(z.cooldown[i], || name("cooldown"))?;
        finite(z.groan_timer[i], || name("groan_timer"))?;
        non_negative(z.hp[i], || name("hp"))?;
        finite2(z.anchor[i], || name("anchor"))?;
        finite(z.stuck[i], || name("stuck"))?;
        finite(z.bored[i], || name("bored"))?;
        let state = z.state[i];
        let target = z.target_player[i];
        if target != 0 && target >= w.next_player_id && state != ZombieState::Dead {
            return Err(invalid(format!("{} {target} never existed (next player id {})", name("target_player"), w.next_player_id)));
        }
        if state == ZombieState::Dead && z.hp[i] > 0.0 {
            return Err(invalid(name("dead with hit points left")));
        }
        if state != ZombieState::Dead && z.hp[i] <= 0.0 {
            return Err(invalid(format!("{} {state:?} with no hit points", name("alive"))));
        }
        if matches!(state, ZombieState::Lunge | ZombieState::Attack) && z.tier[i] != Tier::Active {
            return Err(invalid(format!("{} {state:?} on the {:?} tier (only active zombies lunge or strike)", name("state"), z.tier[i])));
        }
        if state == ZombieState::Thump {
            tile_on_map(map, z.thump_edge[i].tile, || name("thump_edge"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{WeaponDef, parse_weapons};
    use crate::map::{EdgeRef, OwnedEdge};
    use crate::save::LoadOptions;
    use crate::senses::Sound;
    use crate::template::BuildingTemplate;

    fn templates() -> Vec<BuildingTemplate> {
        [include_str!("../../../assets/data/buildings/bungalow.ron"), include_str!("../../../assets/data/buildings/family_two_storey.ron")]
            .iter()
            .map(|t| BuildingTemplate::from_ron(t).unwrap())
            .collect()
    }

    fn weapons() -> Vec<WeaponDef> {
        parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap()
    }

    fn items() -> Vec<crate::items::ItemDef> {
        crate::items::parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons()).unwrap()
    }

    fn content() -> WorldConfig {
        let containers = crate::containers::parse_containers(include_str!("../../../assets/data/containers.ron")).unwrap();
        WorldConfig::new(5, 0, templates()).with_weapons(weapons()).with_items(items()).with_containers(containers).with_map_chunks(8)
    }

    fn first_edge(w: &World, kind: impl Fn(Edge) -> bool) -> EdgeRef {
        let b = &w.map.buildings[w.spawn_building];
        for y in b.min.y..b.min.y + b.size.y + 1 {
            for x in b.min.x..b.min.x + b.size.x + 1 {
                for edge in [OwnedEdge::North, OwnedEdge::West] {
                    let t = IVec2::new(x, y);
                    if kind(w.map.owned_edge(t, edge)) {
                        return EdgeRef::new(t, edge);
                    }
                }
            }
        }
        panic!("no such edge in the spawn house");
    }

    /// A small world with a bit of everything: two players (one armed), weapons on the floor, a
    /// fight, a damaged door, a loud noise's attractor, and zombies on every tier.
    fn busy() -> World {
        let mut config = content();
        config.zombies = Some(120);
        let mut w = World::new(config);
        let spawn = w.player_spawn;
        let a = w.add_player(spawn);
        w.add_player(spawn + Vec2::new(1.0, 0.0));
        for p in &mut w.players {
            p.hp = 1e5;
        }
        w.spawn_item("baseball_bat", 1, spawn + Vec2::new(0.4, 0.0));
        w.player_pick_up(a);
        w.spawn_item("baseball_bat", 1, spawn + Vec2::new(-0.4, 0.0));
        // The second player searched a container and carries a few things.
        let b = w.players[1].id;
        crate::save::tests::open_nearest_container(&mut w, b);
        w.give_item(b, "tinned_beans", 2);
        w.spawn_zombies(12, spawn + Vec2::new(8.0, 0.0), 4.0);
        let door = first_edge(&w, |e| e == Edge::DoorClosed);
        w.edge_hp.insert(door.key(), 5.0);
        w.make_noise(spawn, 120.0, true);
        for t in 0..60u32 {
            let seq = t + 1;
            w.set_player_input(a, seq, crate::players::PlayerInput { move_dir: Vec2::from_angle(t as f32 * 0.1), facing: 0.0, run: false, sneak: false });
            if t % 20 == 0 {
                w.player_attack(a);
            }
            w.step();
        }
        w
    }

    fn load(bytes: &[u8]) -> Result<World, LoadError> {
        World::load(bytes, &content())
    }

    // --- Content changes -------------------------------------------------------------------

    #[test]
    fn compatible_content_changes_load_with_a_note() {
        let w = busy();
        let bytes = w.save();
        let hash = w.state_hash();
        let ids = |w: &World| -> Vec<String> { w.items.iter().map(|it| w.config.items[it.stack.item as usize].id.clone()).collect() };
        let held = |w: &World| w.players[0].weapon.map(|i| w.config.weapons[i as usize].id.clone());

        // A weapon's numbers changed: the new ones apply, nothing else moves.
        let mut c = content();
        c.weapons[0].damage.1 += 1.0;
        let l = World::load_with(&bytes, &c, LoadOptions::default()).unwrap();
        assert_eq!(l.notes.len(), 1);
        assert!(l.notes[0].contains("other numbers"), "{:?}", l.notes);
        assert_eq!((l.world.state_hash(), l.world.config.weapons[0].damage), (hash, c.weapons[0].damage));

        // A weapon added at the front: every index moves, and is matched up by id.
        let mut c = content();
        let mut new = c.weapons[0].clone();
        new.id = "crowbar_x".into();
        c.weapons.insert(0, new);
        let l = World::load_with(&bytes, &c, LoadOptions::default()).unwrap();
        assert!(l.notes[0].contains("added: crowbar_x; removed: none"), "{:?}", l.notes);
        assert_eq!((ids(&l.world), held(&l.world)), (ids(&w), held(&w)));
        assert!(held(&w).is_some());

        // A weapon (and its item) nothing in the save uses, removed.
        let used: Vec<String> = ids(&w).into_iter().chain(held(&w)).chain(w.players.iter().flat_map(|p| p.inventory.stacks().map(|s| w.config.items[s.item as usize].id.clone()))).collect();
        let mut c = content();
        let spare = c.weapons.iter().position(|d| !used.contains(&d.id) && d.category != crate::items::WeaponCategory::Unarmed).expect("an unused weapon");
        let gone = c.weapons.remove(spare).id;
        c.items.retain(|d| d.id != gone);
        let l = World::load_with(&bytes, &c, LoadOptions::default()).unwrap();
        assert!(l.notes[0].contains(&format!("removed: {gone}")), "{:?}", l.notes);
        assert!(l.notes[1].contains("items.ron") && l.notes[1].contains(&format!("removed: {gone}")), "{:?}", l.notes);
        assert_eq!((ids(&l.world), held(&l.world)), (ids(&w), held(&w)));

        // An item's weight changed: it loads, and the new number counts.
        let mut c = content();
        c.items.iter_mut().find(|d| d.id == "tinned_beans").unwrap().weight_kg = 5.0;
        let l = World::load_with(&bytes, &c, LoadOptions::default()).unwrap();
        assert!(l.notes.len() == 1 && l.notes[0].contains("items.ron changed (the same items"), "{:?}", l.notes);
        let b = &l.world.players[1];
        assert!(l.world.carry(b).weight_kg > w.carry(&w.players[1]).weight_kg + 8.0);

        // A container got bigger: the opened one keeps its contents.
        let mut c = content();
        c.containers.containers.iter_mut().for_each(|d| d.room_l *= 2.0);
        let l = World::load_with(&bytes, &c, LoadOptions::default()).unwrap();
        assert!(l.notes.len() == 1 && l.notes[0].contains("containers.ron"), "{:?}", l.notes);
        assert_eq!(l.world.opened, w.opened);
        assert_eq!(l.world.state_hash(), hash);

        // Population settings.
        let mut c = content();
        c.population.respawn.enabled = !c.population.respawn.enabled;
        let l = World::load_with(&bytes, &c, LoadOptions::default()).unwrap();
        assert!(l.notes.len() == 1 && l.notes[0].contains("population.ron"), "{:?}", l.notes);
        assert_eq!(l.world.state_hash(), hash);
        assert_eq!(l.world.config.population.respawn.enabled, c.population.respawn.enabled);

        // Unchanged content: no notes, the same world.
        let l = World::load_with(&bytes, &content(), LoadOptions::default()).unwrap();
        assert!(l.notes.is_empty());
        assert_eq!(l.world.save(), bytes);
    }

    #[test]
    fn a_missing_item_the_save_uses_needs_load_anyway() {
        let w = busy();
        let bytes = w.save();
        let bat_weapon = w.config.weapons.iter().position(|d| d.id == "baseball_bat").unwrap() as u16;
        let bat = w.item_index("baseball_bat").unwrap();
        let bats = w.items.iter().filter(|it| it.stack.item == bat).count();
        assert!(bats >= 1 && w.players[0].weapon == Some(bat_weapon));
        // The weapon alone gone: the bat is still an item (it just doesn't swing like one).
        let mut c = content();
        c.weapons.retain(|d| d.id != "baseball_bat");
        let l = World::load_with(&bytes, &c, LoadOptions::default()).unwrap();
        assert_eq!(l.world.players[0].weapon, None);
        assert_eq!(l.world.players[0].inventory.hand.map(|s| l.world.item_def(s.item).id.clone()).as_deref(), Some("baseball_bat"));
        // The item gone too: refused, unless forced.
        c.items.retain(|d| d.id != "baseball_bat");
        let err = World::load_with(&bytes, &c, LoadOptions::default()).err().unwrap();
        let LoadError::ContentChanged(what) = &err else { panic!("{err}") };
        assert!(what.len() == 1 && what[0].contains("item baseball_bat is gone"), "{err}");
        assert!(err.forceable());

        let l = World::load_with(&bytes, &c, LoadOptions { force: true }).unwrap();
        assert!(l.notes.iter().any(|n| n.contains("loaded anyway") && n.contains("baseball_bat")), "{:?}", l.notes);
        let mut world = l.world;
        assert_eq!(world.items.len(), w.items.len() - bats);
        assert_eq!((world.players[0].weapon, world.players[0].inventory.hand), (None, None));
        assert!(world.items.iter().all(|it| world.config.items[it.stack.item as usize].id != "baseball_bat"));
        for _ in 0..40 {
            world.step();
        }
        // The forced world saves and loads against the new content without complaint.
        let again = World::load_with(&world.save(), &c, LoadOptions::default()).unwrap();
        assert!(again.notes.is_empty(), "{:?}", again.notes);
    }

    #[test]
    fn templates_that_change_the_map_need_load_anyway() {
        let mut w = busy();
        // Open a door, so there's a map edit to carry over.
        let door = first_edge(&w, |e| e == Edge::DoorClosed);
        w.toggle_edge(door, crate::senses::SoundSource::World);
        let bytes = w.save();
        let mut c = content();
        c.templates.truncate(1);
        let err = World::load_with(&bytes, &c, LoadOptions::default()).err().unwrap();
        assert!(matches!(&err, LoadError::ContentChanged(what) if what[0].contains("templates") && what[0].contains("map would differ")), "{err}");

        let l = World::load_with(&bytes, &c, LoadOptions { force: true }).unwrap();
        assert!(l.notes.iter().any(|n| n.contains("regenerated")), "{:?}", l.notes);
        let mut world = l.world;
        assert_eq!(world.zombies.len(), w.zombies.len());
        assert_eq!(world.players.len(), 2);
        for p in &world.players {
            assert!(!world.map.tile_blocked(p.state.pos.floor().as_ivec2()), "player {} in something", p.id);
        }
        for _ in 0..60 {
            world.step();
        }
        let again = World::load_with(&world.save(), &c, LoadOptions::default()).unwrap();
        assert!(again.notes.is_empty(), "{:?}", again.notes);

        // A save with its map stored whole doesn't care about templates.
        let mut w = busy();
        w.map_changed();
        let l = World::load_with(&w.save(), &c, LoadOptions::default()).unwrap();
        assert!(l.notes.iter().any(|n| n.contains("stores its map whole")), "{:?}", l.notes);
        assert_eq!(l.world.state_hash(), w.state_hash());
    }

    #[test]
    fn a_changed_generator_is_forceable_too() {
        let w = busy();
        let mut bytes = w.save();
        // Pretend the map was made by another generator: change the MAPD base hash.
        let mut s = sections(&bytes);
        let mapd = s.iter_mut().find(|(t, _)| t == b"MAPD").unwrap();
        mapd.1[0] ^= 1;
        bytes = assemble(&s);
        assert_eq!(load(&bytes).err(), Some(LoadError::MapChanged));
        let l = World::load_with(&bytes, &content(), LoadOptions { force: true }).unwrap();
        assert!(l.notes.iter().any(|n| n.contains("generator")), "{:?}", l.notes);
    }

    // --- Validation --------------------------------------------------------------------------

    /// A save's sections (tag, payload), and back into a file with good checksums.
    fn sections(bytes: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        let mut out = Vec::new();
        let mut at = 12;
        while at < bytes.len() {
            let tag: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
            let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            out.push((tag, bytes[at + 8..at + 8 + len].to_vec()));
            at += 8 + len + 8;
        }
        out
    }

    fn assemble(sections: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let fnv = |h: u64, bytes: &[u8]| bytes.iter().fold(h, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
        let mut out = b"ZOMBYSAV".to_vec();
        out.extend(crate::save::FORMAT_VERSION.to_le_bytes());
        for (tag, payload) in sections {
            out.extend(tag);
            out.extend((payload.len() as u32).to_le_bytes());
            out.extend(payload);
            out.extend(fnv(fnv(0xcbf2_9ce4_8422_2325, tag), payload).to_le_bytes());
        }
        out
    }

    /// Indices of a living zombie and a corpse.
    fn alive(w: &World) -> usize {
        w.zombies.state.iter().position(|s| *s != ZombieState::Dead).unwrap()
    }

    type Mutation = (&'static str, &'static str, fn(&mut World));

    /// Each mutation of a valid world, saved (so every checksum is right), is refused on load with
    /// `LoadError::Invalid` naming the field, and nothing panics.
    #[test]
    fn impossible_values_are_refused_by_name() {
        let base = busy().save();
        assert!(load(&base).is_ok());
        let cases: &[Mutation] = &[
            ("zombie pos NaN", "pos", |w| w.zombies.pos[0].x = f32::NAN),
            ("zombie off the map", "off the map", |w| w.zombies.pos[0] = Vec2::new(-4.0, 3.0)),
            ("zombie far off the map", "off the map", |w| w.zombies.pos[0] = Vec2::new(1e9, 3.0)),
            ("zombie vel inf", "vel", |w| w.zombies.vel[0].y = f32::INFINITY),
            ("zombie facing NaN", "facing", |w| w.zombies.facing[0] = f32::NAN),
            ("zombie speed negative", "speed", |w| w.zombies.speed[0] = -1.0),
            ("zombie speed absurd", "speed is 1000000 (at most 50)", |w| w.zombies.speed[0] = 1e6),
            ("zombie flung", "vel", |w| w.zombies.vel[0] = Vec2::new(0.0, 4e9)),
            ("zombie timer inf", "timer", |w| w.zombies.timer[0] = f32::INFINITY),
            ("zombie cooldown NaN", "cooldown", |w| w.zombies.cooldown[0] = f32::NAN),
            ("zombie groan NaN", "groan_timer", |w| w.zombies.groan_timer[0] = f32::NAN),
            ("zombie wander timer NaN", "wander_timer", |w| w.zombies.wander_timer[0] = f32::NAN),
            ("zombie wander NaN", "wander", |w| w.zombies.wander[0] = Vec2::NAN),
            ("zombie target pos NaN", "target_pos", |w| w.zombies.target_pos[0] = Vec2::NAN),
            ("zombie anchor NaN", "anchor", |w| w.zombies.anchor[0] = Vec2::NAN),
            ("zombie stuck NaN", "stuck", |w| w.zombies.stuck[0] = f32::NAN),
            ("zombie bored NaN", "bored", |w| w.zombies.bored[0] = f32::NAN),
            ("zombie hp negative", "hp", |w| w.zombies.hp[0] = -3.0),
            ("zombie hp NaN", "hp", |w| w.zombies.hp[0] = f32::NAN),
            ("living zombie without hp", "with no hit points", |w| {
                let i = alive(w);
                w.zombies.hp[i] = 0.0;
            }),
            ("corpse with hp", "dead with hit points", |w| {
                w.zombies.state[0] = ZombieState::Dead;
                w.zombies.hp[0] = 10.0;
            }),
            ("repeated zombie id", "zombie id", |w| w.zombies.id[1] = w.zombies.id[0]),
            ("zero zombie id", "zombie id 0", |w| w.zombies.id[0] = 0),
            ("zombie id past the next", "past the next id", |w| w.next_id = w.zombies.id[w.zombies.len() - 1]),
            ("zombie hunting a player that never was", "target_player", |w| {
                let i = alive(w);
                (w.zombies.state[i], w.zombies.target_player[i]) = (ZombieState::Chase, 77);
            }),
            ("lunge on the virtual tier", "Virtual tier", |w| {
                let i = alive(w);
                (w.zombies.state[i], w.zombies.tier[i]) = (ZombieState::Lunge, Tier::Virtual);
            }),
            ("thumping an edge off the map", "thump_edge", |w| {
                let i = alive(w);
                w.zombies.state[i] = ZombieState::Thump;
                w.zombies.thump_edge[i] = EdgeRef::new(IVec2::new(-3, 900), OwnedEdge::North);
            }),
            ("player pos NaN", "player 1: pos", |w| w.players[0].state.pos.y = f32::NAN),
            ("player off the map", "off the map", |w| w.players[0].state.pos = Vec2::new(5.0, -0.5)),
            ("player facing inf", "facing", |w| w.players[0].state.facing = f32::NEG_INFINITY),
            ("player endurance", "endurance", |w| w.players[0].state.endurance = 7.0),
            ("player hp negative", "hp", |w| w.players[0].hp = -1.0),
            ("player hp NaN", "hp", |w| w.players[0].hp = f32::NAN),
            ("living player without hp", "alive with no hit points", |w| w.players[0].hp = 0.0),
            ("dead player mid-swing", "dead with an action", |w| {
                w.players[1].dead = true;
                w.players[1].queued = Some(crate::players::ActionKind::Swing);
            }),
            ("repeated player id", "player id", |w| w.players[1].id = w.players[0].id),
            ("zero player id", "player id 0", |w| w.players[0].id = 0),
            ("player ready_in NaN", "ready_in", |w| w.players[0].ready_in = f32::NAN),
            ("wound age NaN", "age_h", |w| {
                w.inflict_wound(1, crate::health::Part::Torso, crate::health::WoundKind::Gash, Some(false));
                w.players[0].health.wounds[0].age_h = f32::NAN;
            }),
            ("wound healed past its time", "heal_h", |w| {
                w.inflict_wound(1, crate::health::Part::LeftLeg, crate::health::WoundKind::Scratch, Some(false));
                w.players[0].health.wounds[0].healed_h = 1e3;
            }),
            ("repeated wound id", "wound id", |w| {
                w.inflict_wound(1, crate::health::Part::Head, crate::health::WoundKind::Bruise, Some(false));
                w.inflict_wound(1, crate::health::Part::Head, crate::health::WoundKind::Bruise, Some(false));
                w.players[0].health.wounds[1].id = 0;
            }),
            ("infection timer inf", "infection.left_h", |w| {
                w.inflict_wound(1, crate::health::Part::Torso, crate::health::WoundKind::Bite, Some(true));
                w.players[0].health.infection.as_mut().unwrap().left_h = f32::INFINITY;
            }),
            ("food poisoning negative", "food_poisoning_h", |w| w.players[0].health.food_poisoning_h = -2.0),
            ("rising while alive", "about to rise", |w| w.players[0].health.rise_in = Some(3.0)),
            ("infection stage length absurd", "stage_mult", |w| w.config.infection.stage_mult = 40.0),
            ("wound that never heals", "health.gash", |w| w.config.health.gash.heal_h = 0.0),
            ("player input NaN", "input.move_dir", |w| w.players[0].input.move_dir = Vec2::NAN),
            ("player hand item out of range", "item index 3000", |w| w.players[0].inventory.hand = Some(ItemStack::one(3000))),
            ("carried item out of range", "item index 3001", |w| w.players[1].inventory.carried.push(ItemStack::one(3001))),
            ("a carried stack of none", "at least one", |w| w.players[1].inventory.carried[0].count = 0),
            ("two things in one hand", "one at a time", |w| w.players[0].inventory.hand.as_mut().unwrap().count = 2),
            ("job time NaN", "time left", |w| {
                w.players[1].jobs.push(crate::inventory::ItemJob { kind: crate::inventory::JobKind::Search { container: 0 }, left: f32::NAN })
            }),
            ("opened container that isn't one", "isn't a container", |w| {
                w.opened.insert(9_999_999, Vec::new());
            }),
            ("container item out of range", "item index 4000", |w| {
                w.opened.values_mut().next().unwrap().push(ItemStack::one(4000));
            }),
            ("player action weapon out of range", "action: weapon index 301", |w| {
                w.players[0].action = Some(crate::players::Action { kind: crate::players::ActionKind::Swing, timer: 0.1, power: 1.0, weapon: 301 })
            }),
            ("player action timer NaN", "action.timer", |w| {
                w.players[0].action = Some(crate::players::Action { kind: crate::players::ActionKind::Swing, timer: f32::NAN, power: 1.0, weapon: 0 })
            }),
            ("ground item out of range", "item index 999", |w| w.items[0].stack.item = 999),
            ("ground stack of none", "at least one", |w| w.items[0].stack.count = 0),
            ("item pos inf", "pos", |w| w.items[0].pos.x = f32::INFINITY),
            ("item off the map", "off the map", |w| w.items[0].pos = Vec2::new(-1.0, -1.0)),
            ("item angle NaN", "angle", |w| w.items[0].angle = f32::NAN),
            ("repeated item id", "item id", |w| w.items[1].id = w.items[0].id),
            ("item id past the next", "item id", |w| w.next_item_id = 1),
            ("damage on a wall", "has no hit points", |w| {
                let wall = first_edge(w, |e| e == Edge::Wall);
                w.edge_hp.insert(wall.key(), 3.0);
            }),
            ("door damage NaN", "hp NaN", |w| {
                let door = first_edge(w, |e| e == Edge::DoorClosed);
                w.edge_hp.insert(door.key(), f32::NAN);
            }),
            ("door damage negative", "hp -2", |w| {
                let door = first_edge(w, |e| e == Edge::DoorClosed);
                w.edge_hp.insert(door.key(), -2.0);
            }),
            ("damage off the map", "off the map", |w| {
                w.edge_hp.insert((-5, 2, 0), 3.0);
            }),
            ("damage on a third side", "neither north nor west", |w| {
                let door = first_edge(w, |e| e == Edge::DoorClosed);
                w.edge_hp.insert((door.tile.x, door.tile.y, 7), 3.0);
            }),
            ("sound NaN", "sound 0: pos", |w| w.sounds.insert(0, Sound::new(Vec2::NAN, 10.0, crate::senses::SoundSource::World))),
            ("sound radius negative", "radius", |w| w.sounds.insert(0, Sound::new(Vec2::ONE, -10.0, crate::senses::SoundSource::World))),
            ("goal off the map", "goal", |w| w.goal = Some(IVec2::new(-1, 4))),
            ("spawn building out of range", "spawn building", |w| w.spawn_building = 9999),
            ("player spawn NaN", "player spawn", |w| w.player_spawn = Vec2::NAN),
            ("town past the map", "town bounds", |w| w.town_max = IVec2::new(100_000, 10)),
            ("attractor with no life", "life_secs", |w| w.population.attractors[0].life_secs = 0.0),
            ("attractor pos NaN", "attractor", |w| w.population.attractors[0].pos = Vec2::NAN),
            ("attractor id past the last", "attractor id", |w| w.population.attractors[0].id = 99),
            ("population target NaN", "target", |w| w.population.target[0] = f32::NAN),
            ("clock NaN", "clock offset", |w| w.clock_offset_hours = f32::NAN),
            ("an edit that turns a wall into a door", "turns a Wall into a DoorOpen", |w| {
                let wall = first_edge(w, |e| e == Edge::Wall);
                w.map.set_owned_edge(wall.tile, wall.edge, Edge::DoorOpen);
                w.map_edits.insert(wall.key());
            }),
            ("an edit off the map", "map edit", |w| {
                w.map_edits.insert((-2, -2, 0));
            }),
            ("a stored map with an upper room out of range", "upper floor", |w| {
                w.map_changed();
                let b = w.map.buildings.iter_mut().find(|b| !b.upper_floors.is_empty()).expect("a two-storey house");
                b.upper_floors[0].2 = 9999;
            }),
            ("settings NaN", "zombie.sight_day is NaN", |w| w.config.zombie.sight_day = f32::NAN),
            ("settings inf", "combat", |w| w.config.combat.zombie_hit_chance = f32::INFINITY),
            ("settings absurd", "zombie.wander_speed is 100000000", |w| w.config.zombie.wander_speed = 1e8),
            ("day of no length", "day_length_secs", |w| w.config.day_length_secs = 0.0),
            ("no town", "generation.blocks", |w| w.config.generation.blocks = IVec2::new(0, 2)),
            ("a huge town", "generation.blocks", |w| w.config.generation.blocks = IVec2::new(2, 10_000)),
            ("a huge map", "generation.chunks", |w| w.config.generation.chunks = IVec2::new(4000, 8)),
        ];
        for (what, field, mutate) in cases {
            let mut w = load(&base).unwrap();
            mutate(&mut w);
            match load(&w.save()) {
                Err(LoadError::Invalid(why)) => assert!(why.contains(field), "{what}: the message should name {field:?}: {why}"),
                other => panic!("{what}: expected Invalid, got {:?}", other.map(|_| "a world")),
            }
        }
    }

    /// Damage inside a section that a checksum can't catch (the checksum was recomputed): a region
    /// that doesn't exist, zombies filed under the wrong region, a mangled settings text.
    #[test]
    fn forged_sections_are_refused() {
        let base = busy();
        let s = sections(&base.save());
        let zomb = s.iter().position(|(t, _)| t == b"ZOMB").unwrap();
        // The block's region is its first field, a postcard varint.
        let varint = |p: &[u8]| {
            let len = p.iter().position(|b| b & 0x80 == 0).unwrap() + 1;
            (p[..len].iter().rev().fold(0u32, |v, b| v << 7 | (b & 0x7f) as u32), len)
        };
        let with_region = |region: u32| {
            let mut s = s.clone();
            let (_, len) = varint(&s[zomb].1);
            let mut bytes = Vec::new();
            let mut v = region;
            while v >= 0x80 {
                bytes.push((v as u8 & 0x7f) | 0x80);
                v >>= 7;
            }
            bytes.push(v as u8);
            s[zomb].1.splice(0..len, bytes);
            assemble(&s)
        };
        let err = load(&with_region(100_000)).err().unwrap();
        assert!(matches!(&err, LoadError::Invalid(why) if why.contains("region 100000 is out of range")), "{err}");
        let regions = base.regions().regions() as u32;
        let (first, _) = varint(&s[zomb].1);
        let err = load(&with_region((first + 1) % regions)).err().unwrap();
        assert!(matches!(&err, LoadError::Invalid(why) if why.contains("stands in region")), "{err}");

        let conf = s.iter().position(|(t, _)| t == b"CONF").unwrap();
        let mut bad = s.clone();
        bad[conf].1 = String::from_utf8(bad[conf].1.clone()).unwrap().replacen("door_hp:", "door_hp:inf,_was:", 1).into_bytes();
        assert!(load(&assemble(&bad)).is_err());
    }

    /// Random bytes changed inside sections (checksums fixed up): every load either fails cleanly
    /// or gives a world that steps without panicking.
    #[test]
    fn fuzzed_sections_load_or_fail_cleanly() {
        let base = busy();
        let s = sections(&base.save());
        let mut rng = crate::rng::Rng::new(45);
        let (mut loaded, mut refused) = (0, 0);
        for round in 0..300 {
            let mut s = s.clone();
            let k = 1 + rng.below(s.len() as u32 - 2) as usize; // anything but INFO and END
            let p = &mut s[k].1;
            if p.is_empty() {
                continue;
            }
            for _ in 0..1 + rng.below(3) {
                let at = rng.below(p.len() as u32) as usize;
                p[at] = match rng.below(4) {
                    0 => p[at] ^ (1 << rng.below(8)),
                    1 => 0xff,
                    2 => 0,
                    _ => rng.next_u64() as u8,
                };
            }
            match load(&assemble(&s)) {
                Ok(mut w) => {
                    for _ in 0..10 {
                        w.step();
                    }
                    let _ = w.save();
                    loaded += 1;
                }
                Err(LoadError::Invalid(_) | LoadError::Corrupt(_) | LoadError::Truncated | LoadError::MapChanged) => refused += 1,
                Err(e) => panic!("round {round}: unexpected {e:?}"),
            }
        }
        assert!(loaded > 10 && refused > 10, "{loaded} loaded, {refused} refused");
    }
}
