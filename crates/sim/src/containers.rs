//! Furniture that holds things: fridges, cupboards, wardrobes, drawers. Which furniture is which
//! container is data (`assets/data/containers.ron`, parsed with [`parse_containers`]): a map from
//! (furniture kind, room type) to a [`ContainerDef`] (room in litres, search time, noise), with an
//! `any` room as the fallback.
//!
//! Every ground-floor piece of furniture that maps to a container is a [`ContainerSite`], found
//! from the map when the world is made ([`find_sites`]); its id is the furniture's place in the
//! map (buildings in order, each building's furniture in order), so it stays the same for the same
//! map whatever `containers.ron` says. Contents are filled the first time anyone opens a container
//! ([`fill`], from a hash of the world seed and the container id, never the world RNG), so an
//! unopened house costs nothing to save and fills the same whenever and in whatever order it is
//! searched. [`fill`] is the hook loot tables (#8) replace: for now it picks a few items from the
//! container's `holds` categories.

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::items::{ItemCategory, ItemDef, ItemStack};
use crate::map::TileMap;
use crate::rng::Rng;

/// The room type that matches any room in `containers.ron`'s furniture map.
pub const ANY_ROOM: &str = "any";

/// One kind of container.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerDef {
    pub id: String,
    pub name: String,
    /// Room inside, in litres.
    pub room_l: f32,
    /// Seconds the first search takes (×`CarryTuning::sneak_search_mult` when sneaking).
    pub search_secs: f32,
    /// Radius of the noise opening it makes (halved when sneaking).
    pub noise_m: f32,
    /// Placeholder filling until loot tables (#8): item categories it may hold...
    #[serde(default)]
    pub holds: Vec<ItemCategory>,
    /// ...and how many items, (min, max).
    #[serde(default)]
    pub fill: (u8, u8),
}

/// One line of the furniture map: furniture `furniture` in a room of type `room` (or `any`) is a
/// `container`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FurnitureContainer {
    pub furniture: String,
    pub room: String,
    pub container: String,
}

/// The whole of `containers.ron`. Empty by default: no furniture holds anything.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerData {
    pub containers: Vec<ContainerDef>,
    pub furniture: Vec<FurnitureContainer>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl ContainerData {
    pub fn validate(&self) -> Result<(), String> {
        for (i, c) in self.containers.iter().enumerate() {
            let id = &c.id;
            if !valid_id(id) {
                return Err(format!("container id {id:?} must be lowercase letters, digits and _"));
            }
            if self.containers[..i].iter().any(|o| &o.id == id) {
                return Err(format!("container id {id:?} is used twice"));
            }
            if c.name.trim().is_empty() {
                return Err(format!("{id}: name is empty"));
            }
            if !(c.room_l > 0.0 && c.room_l <= 1000.0) {
                return Err(format!("{id}: room_l must be in (0, 1000], got {}", c.room_l));
            }
            if !(0.0..=30.0).contains(&c.search_secs) {
                return Err(format!("{id}: search_secs must be in 0..30, got {}", c.search_secs));
            }
            if !(0.0..=50.0).contains(&c.noise_m) {
                return Err(format!("{id}: noise_m must be in 0..50, got {}", c.noise_m));
            }
            if c.fill.0 > c.fill.1 || c.fill.1 > 20 {
                return Err(format!("{id}: fill must be (min, max) with min <= max <= 20, got {:?}", c.fill));
            }
            if c.fill.1 > 0 && c.holds.is_empty() {
                return Err(format!("{id}: fill is {:?} but `holds` names no item categories", c.fill));
            }
        }
        for (i, f) in self.furniture.iter().enumerate() {
            if !valid_id(&f.furniture) || !valid_id(&f.room) {
                return Err(format!("furniture map line {}: furniture {:?} and room {:?} must be lowercase ids", i + 1, f.furniture, f.room));
            }
            if !self.containers.iter().any(|c| c.id == f.container) {
                return Err(format!("furniture map line {}: container {:?} isn't one of the containers", i + 1, f.container));
            }
            if self.furniture[..i].iter().any(|o| o.furniture == f.furniture && o.room == f.room) {
                return Err(format!("furniture map line {}: ({}, {}) is mapped twice", i + 1, f.furniture, f.room));
            }
        }
        Ok(())
    }

    /// The container furniture `kind` makes in a room of type `room`: that room's line, else the
    /// `any` line, else none. An index into `containers`.
    pub fn lookup(&self, kind: &str, room: &str) -> Option<u16> {
        let line = self.furniture.iter().find(|f| f.furniture == kind && f.room == room).or_else(|| self.furniture.iter().find(|f| f.furniture == kind && f.room == ANY_ROOM))?;
        self.containers.iter().position(|c| c.id == line.container).map(|i| i as u16)
    }
}

/// Parse and validate `containers.ron`.
pub fn parse_containers(text: &str) -> Result<ContainerData, String> {
    let data: ContainerData = ron::from_str(text).map_err(|e| e.to_string())?;
    data.validate()?;
    Ok(data)
}

/// A piece of furniture that is a container, where it stands. Derived from the map and the
/// container data (not saved).
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerSite {
    /// The furniture's place in the map (see the module docs).
    pub id: u32,
    /// Index into `ContainerData::containers`.
    pub def: u16,
    pub building: u16,
    pub room: u16,
    /// Footprint in tiles: `min..min+size`.
    pub min: IVec2,
    pub size: IVec2,
}

impl ContainerSite {
    pub fn centre(&self) -> Vec2 {
        self.min.as_vec2() + self.size.as_vec2() * 0.5
    }

    /// Distance from `p` to the edge of the footprint (0 inside it).
    pub fn distance(&self, p: Vec2) -> f32 {
        let lo = self.min.as_vec2();
        let hi = lo + self.size.as_vec2();
        p.clamp(lo, hi).distance(p)
    }
}

/// Every container in the map, in id order: ground-floor furniture whose (kind, room type) maps to
/// a container.
pub fn find_sites(map: &TileMap, data: &ContainerData) -> Vec<ContainerSite> {
    let mut out = Vec::new();
    let mut id = 0u32;
    for (bi, b) in map.buildings.iter().enumerate() {
        for f in &b.furniture {
            let this = id;
            id += 1;
            if f.level != 0 || !map.in_bounds(f.min) {
                continue;
            }
            let room = map.tile(f.min).room;
            let room_type = map.rooms.get(room as usize).map_or("", |r| r.room_type.as_str());
            if let Some(def) = data.lookup(&f.kind, room_type) {
                out.push(ContainerSite { id: this, def, building: bi as u16, room, min: f.min, size: f.size });
            }
        }
    }
    out
}

/// What a container holds when it is first opened. Deterministic from `(seed, container id)` and
/// the data alone, so containers fill the same in any order. **The loot hook (#8):** replace the
/// body with a roll of the loot table for `(room_type, def.id)`. For now: `def.fill` items, each
/// picked evenly from the items of `def.holds` categories, skipping picks that don't fit its room;
/// the same item twice adds to one stack (up to its stack size).
pub fn fill(seed: u64, id: u32, def: &ContainerDef, _room_type: &str, items: &[ItemDef]) -> Vec<ItemStack> {
    let mut out: Vec<ItemStack> = Vec::new();
    let pool: Vec<u16> = (0..items.len()).filter(|&i| def.holds.contains(&items[i].category)).map(|i| i as u16).collect();
    if pool.is_empty() || def.fill.1 == 0 {
        return out;
    }
    let mut rng = Rng::new(seed ^ 0xC0_47A1_4E25 ^ (id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let (lo, hi) = (def.fill.0 as u32, def.fill.1 as u32);
    let n = lo + rng.below(hi - lo + 1);
    let mut used = 0.0;
    for _ in 0..n {
        let item = pool[rng.below(pool.len() as u32) as usize];
        let d = &items[item as usize];
        if used + d.volume_l > def.room_l {
            continue;
        }
        used += d.volume_l;
        match out.iter_mut().find(|s| s.item == item && s.count < d.stack) {
            Some(s) => s.count += 1,
            None => out.push(ItemStack::one(item)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{parse_items, parse_weapons};

    fn data() -> ContainerData {
        parse_containers(include_str!("../../../assets/data/containers.ron")).unwrap()
    }

    #[test]
    fn containers_file_is_valid_and_rooms_pick_the_kind() {
        let d = data();
        let name = |k: &str, r: &str| d.lookup(k, r).map(|i| d.containers[i as usize].id.as_str());
        assert_eq!(name("sink", "bathroom"), Some("bathroom_cabinet"));
        assert_eq!(name("sink", "kitchen"), Some("under_sink"));
        assert_eq!(name("wardrobe", "hall"), Some("coat_cupboard"));
        assert_eq!(name("wardrobe", "bedroom"), Some("wardrobe"));
        assert_eq!(name("bed", "bedroom"), None);
        assert!(name("shelf", "laundry").is_some() && name("workbench", "shed").is_some());
    }

    #[test]
    fn validation_explains_mistakes() {
        let mut d = data();
        d.furniture[0].container = "safe".into();
        assert!(d.validate().unwrap_err().contains("safe"));
        let mut d = data();
        d.containers[1].id = d.containers[0].id.clone();
        assert!(d.validate().unwrap_err().contains("twice"));
        let mut d = data();
        d.containers[0].room_l = 0.0;
        assert!(d.validate().unwrap_err().contains("room_l"));
        let mut d = data();
        d.furniture.push(d.furniture[0].clone());
        assert!(d.validate().unwrap_err().contains("mapped twice"));
        assert!(parse_containers("(containers: [], furniture: [], extra: 1)").is_err());
    }

    #[test]
    fn filling_is_deterministic_and_fits() {
        let weapons = parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        let items = parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap();
        let d = data();
        let fridge = &d.containers[d.lookup("fridge", "kitchen").unwrap() as usize];
        let mut filled = 0;
        for id in 0..50 {
            let a = fill(7, id, fridge, "kitchen", &items);
            assert_eq!(a, fill(7, id, fridge, "kitchen", &items));
            let volume: f32 = a.iter().map(|s| items[s.item as usize].volume_l * s.count as f32).sum();
            assert!(volume <= fridge.room_l);
            assert!(a.iter().all(|s| matches!(items[s.item as usize].category, ItemCategory::Food | ItemCategory::Drink)));
            filled += !a.is_empty() as u32;
        }
        assert!(filled > 40, "fridges are mostly not empty: {filled} of 50");
        assert!((0..50).any(|id| fill(7, id, fridge, "kitchen", &items) != fill(8, id, fridge, "kitchen", &items)), "the seed matters");
    }
}
