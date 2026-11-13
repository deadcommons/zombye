//! What zombies wear, and a dead zombie's body as something to search.
//!
//! **Outfits are data** (`outfits` in `assets/data/population.ron`, [`Outfit`]): a list of what a
//! zombie may wear, as item ids, each with a weight. Every zombie wears one, picked by weight from a
//! hash of the world seed and its id ([`outfit_index`]), so nothing is stored or drawn from the world
//! RNG and a client works out the same outfit for itself ([`default_look`]) to pick the crowd variant
//! it draws. A player who died infected rises in what it wore instead ([`World::risen_look`]).
//!
//! **A body is a container.** A dead zombie lying in the world is searched like furniture (E, or
//! `Intent::Search`), from within [`crate::inventory::CONTAINER_REACH`] of it and from the same room,
//! as the container kind [`CORPSE_KIND`] in `containers.ron` (its search time and noise; a data folder
//! without one has no bodies to search). Its container id is the zombie's id with [`CORPSE_BIT`] set
//! ([`corpse_container`]), so `Place::Container`, `Search`, an open container and the inventory panel
//! all work as they do for a cupboard. The first search fills it ([`World::fill_corpse`]): every item
//! it wears, and what its pockets hold from a loot table for `corpse` if `loot.ron` has one. From then
//! on what it wears is the clothing left in it ([`World::zombie_look`]), so taking a hoodie off a body
//! takes it off the body you see. Nothing can be put on a body. When the world clears a corpse away
//! (`ZombieTuning::corpse_secs`, `max_corpses`) what was left on it goes with it.

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::containers::ContainerSite;
use crate::items::{ItemDef, ItemStack};
use crate::rng::hash01;
use crate::world::World;
use crate::zombies::ZombieState;

/// Set in a container id that is a dead zombie's body (the rest of the bits are its id).
pub const CORPSE_BIT: u32 = 0x8000_0000;
/// The container kind (in `containers.ron`) a body is, and the loot table's container for what its
/// pockets hold.
pub const CORPSE_KIND: &str = "corpse";
/// A body's reach is measured from this far round where it lies.
pub const BODY_RADIUS: f32 = 0.4;
const OUTFIT_SALT: u64 = 0x0F17_0000;

/// One outfit a zombie may wear: item ids (clothing, as `items.ron` has them), and how likely it is
/// against the others.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outfit {
    pub wears: Vec<String>,
    #[serde(default = "one")]
    pub weight: f32,
}

fn one() -> f32 {
    1.0
}

/// Explains the first mistake in a list of outfits (item ids are checked against the items when
/// content is loaded: [`check_outfits`]).
pub fn validate_outfits(outfits: &[Outfit]) -> Result<(), String> {
    if outfits.len() > 256 {
        return Err(format!("outfits: at most 256, got {}", outfits.len()));
    }
    for (k, o) in outfits.iter().enumerate() {
        let n = k + 1;
        if !(o.weight.is_finite() && (0.0..=1000.0).contains(&o.weight)) {
            return Err(format!("outfits {n}: weight must be in 0..1000, got {}", o.weight));
        }
        if o.wears.len() > 16 {
            return Err(format!("outfits {n}: at most 16 things worn, got {}", o.wears.len()));
        }
        for (i, w) in o.wears.iter().enumerate() {
            if w.is_empty() || !w.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                return Err(format!("outfits {n}: {w:?} isn't an item id"));
            }
            if o.wears[..i].contains(w) {
                return Err(format!("outfits {n}: {w} is listed twice"));
            }
        }
    }
    if !outfits.is_empty() && outfits.iter().all(|o| o.weight <= 0.0) {
        return Err("outfits: at least one needs a weight above 0".into());
    }
    Ok(())
}

/// Every outfit's items are clothing in `items`, and no two of one outfit clash (share a body part or
/// an accessory place on one layer). Errors name the outfit.
pub fn check_outfits(outfits: &[Outfit], items: &[ItemDef]) -> Result<(), String> {
    for (k, o) in outfits.iter().enumerate() {
        let mut worn: Vec<&ItemDef> = Vec::new();
        for w in &o.wears {
            let Some(d) = items.iter().find(|d| d.id == *w) else { return Err(format!("outfits {}: no item {w:?}", k + 1)) };
            let Some(cl) = &d.clothing else { return Err(format!("outfits {}: {w} isn't clothing", k + 1)) };
            if let Some(o) = worn.iter().find(|o| o.clothing.as_ref().is_some_and(|oc| oc.clashes(cl))) {
                return Err(format!("outfits {}: {w} and {} can't be worn together", k + 1, o.id));
            }
            worn.push(d);
        }
    }
    Ok(())
}

/// The container id of zombie `zombie`'s body.
pub fn corpse_container(zombie: u32) -> u32 {
    zombie | CORPSE_BIT
}

/// The zombie whose body container `c` is, if it is one.
pub fn corpse_zombie(c: u32) -> Option<u32> {
    (c & CORPSE_BIT != 0).then_some(c & !CORPSE_BIT)
}

/// The body nearest `pos` that a player standing there can search (within
/// [`crate::inventory::CONTAINER_REACH`] of [`BODY_RADIUS`] round it, the same room), as its container
/// id, from `bodies` (dead zombies' ids and where they lie). The server's reach rule, for a client to
/// open the body at its feet when the inventory opens.
pub fn body_in_reach(map: &crate::map::TileMap, pos: Vec2, bodies: impl IntoIterator<Item = (u32, Vec2)>) -> Option<u32> {
    let room = |p: Vec2| if map.in_bounds(p.floor().as_ivec2()) { map.room_at(p) } else { 0 };
    let here = room(pos);
    let reach = crate::inventory::CONTAINER_REACH + BODY_RADIUS;
    bodies
        .into_iter()
        .filter(|&(_, b)| b.distance_squared(pos) <= reach * reach && room(b) == here)
        .min_by(|a, b| a.1.distance_squared(pos).total_cmp(&b.1.distance_squared(pos)).then(a.0.cmp(&b.0)))
        .map(|(z, _)| corpse_container(z))
}

/// Which of `outfits` zombie `id` wears in a world of `seed`: by weight, from a hash of the two.
pub fn outfit_index(seed: u64, id: u32, outfits: &[Outfit]) -> Option<usize> {
    let total: f32 = outfits.iter().map(|o| o.weight.max(0.0)).sum();
    if !(total > 0.0) {
        return None;
    }
    let mut r = hash01(seed ^ OUTFIT_SALT, id as u64, 0, 0) * total;
    let mut last = None;
    for (k, o) in outfits.iter().enumerate() {
        if o.weight <= 0.0 {
            continue;
        }
        if r < o.weight {
            return Some(k);
        }
        r -= o.weight;
        last = Some(k);
    }
    last
}

/// What zombie `id` wears as item indices into `items` unless it rose from a player or its body has
/// been searched (ids `items` doesn't have are left out). The server's rule; clients use it to pick
/// the crowd variant to draw.
pub fn default_look(seed: u64, id: u32, outfits: &[Outfit], items: &[ItemDef]) -> Vec<u16> {
    outfit_index(seed, id, outfits).map_or_else(Vec::new, |k| outfits[k].wears.iter().filter_map(|w| items.iter().position(|d| d.id == *w).map(|i| i as u16)).collect())
}

impl World {
    /// The container kind bodies are (`containers.ron`'s [`CORPSE_KIND`]), if there is one.
    fn corpse_def(&self) -> Option<u16> {
        self.config.containers.containers.iter().position(|c| c.id == CORPSE_KIND).map(|i| i as u16)
    }

    /// Zombie `i`'s body as a container (it must be dead).
    fn body_site(&self, i: usize, def: u16) -> ContainerSite {
        let pos = self.zombies.pos[i];
        let tile = pos.floor().as_ivec2();
        let room = if self.map.in_bounds(tile) { self.map.room_at(pos) } else { 0 };
        ContainerSite { id: corpse_container(self.zombies.id[i]), def, level: 0, building: crate::map::NO_BUILDING, room, min: tile, size: IVec2::ONE, body: Some(pos) }
    }

    /// Container `c` if it is the body of a dead zombie still lying in the world.
    pub(crate) fn corpse_site(&self, c: u32) -> Option<ContainerSite> {
        let def = self.corpse_def()?;
        let i = self.zombies.index_of(corpse_zombie(c)?)?;
        (self.zombies.state[i] == ZombieState::Dead).then(|| self.body_site(i, def))
    }

    /// The bodies lying within `reach` of `pos` (to where they lie, less [`BODY_RADIUS`]), in id
    /// order.
    pub fn corpses_near(&self, pos: Vec2, reach: f32) -> Vec<ContainerSite> {
        let Some(def) = self.corpse_def() else { return Vec::new() };
        let z = &self.zombies;
        let r = reach + BODY_RADIUS;
        (0..z.len()).filter(|&i| z.state[i] == ZombieState::Dead && z.pos[i].distance_squared(pos) <= r * r).map(|i| self.body_site(i, def)).collect()
    }

    /// What zombie `id` wears, as item indices: the clothing left on its body once that has been
    /// searched, else what it wore as a player if it rose from one, else its outfit
    /// ([`default_look`]).
    pub fn zombie_look(&self, id: u32) -> Vec<u16> {
        if let Some(stacks) = self.opened.get(&corpse_container(id)) {
            return stacks.iter().filter(|s| self.config.items[s.item as usize].clothing.is_some()).map(|s| s.item).collect();
        }
        if let Some(wore) = self.risen_look(id) {
            return wore.iter().filter_map(|i| self.item_index(i)).collect();
        }
        default_look(self.config.seed(), id, &self.config.population.outfits, &self.config.items)
    }

    /// Does zombie `id` wear what [`default_look`] says (a client can work that out itself, so
    /// snapshots only send the others' looks)?
    pub fn zombie_look_is_default(&self, id: u32) -> bool {
        !self.opened.contains_key(&corpse_container(id)) && self.risen_look(id).is_none()
    }

    /// What body `c` (of zombie `zombie`) holds when first searched: what it wears, then its
    /// pockets' loot if `loot.ron` has a table for bodies.
    pub(crate) fn fill_corpse(&self, c: u32, zombie: u32, site: &ContainerSite) -> Vec<ItemStack> {
        let mut out: Vec<ItemStack> = self.zombie_look(zombie).into_iter().map(|i| self.fresh_stack(i, 1)).collect();
        let def = self.container_def(site);
        if self.config.loot.tables.iter().any(|t| t.container == CORPSE_KIND) {
            out.extend(crate::loot::fill(self.config.loot_seed(), c, def, crate::containers::ANY_ROOM, &self.config.loot, &self.config.items));
        }
        out
    }
}

#[cfg(test)]
mod tests;
