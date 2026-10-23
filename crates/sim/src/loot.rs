//! What furniture holds: loot tables by room type and container kind (`assets/data/loot.ron`,
//! parsed with [`parse_loot`] and checked against the items and containers with
//! [`LootData::check`]).
//!
//! A [`LootTable`] is keyed by `(room, container)`: a room type from the building templates
//! (`kitchen`, `shed`, ...) and a container kind from `containers.ron` (`fridge`, `shelves`, ...),
//! either of which may be `any`. A container uses the first table of ([`LootData::find`]):
//! `(room, container)`, `(any, container)`, `(room, any)`, `(any, any)`; with none it holds
//! nothing. So a fridge is a fridge wherever it stands unless a room says otherwise, and furniture
//! whose kind has no `any` table (shelves, a workbench) holds what its room holds.
//!
//! A table is: `empty` (the chance the container holds nothing at all), `rolls` (how many picks,
//! a range), `entries` (each an item id, a `weight` among the table's entries, a `count` range per
//! pick and whether it is `rare`), and `groups`, extra lists rolled after the picks, each with a
//! `chance` and its own `rolls` ("one in three cupboards has a cutlery drawer: one of these"). The
//! same item picked twice adds up (in stacks of its stack size); a pick that doesn't fit in the
//! container's room is cut to what fits, or skipped.
//!
//! The sandbox knobs are at the top of the file: `scale` multiplies every table's and group's
//! rolls (0.25 scarce ... 2 plenty; a fractional result rounds up or down by chance), and
//! `rare_scale` multiplies the weight of `rare` entries.
//!
//! [`fill`] rolls a table for one container from a hash of the world seed and the container id
//! alone, never the world RNG: a container holds the same whenever, and in whatever order, the
//! houses are searched, and an unopened container costs nothing to save. Changing the tables only
//! changes containers nobody has opened yet (`save_check` loads such a save with a note).

use serde::{Deserialize, Serialize};

use crate::containers::{ANY_ROOM, ContainerData, ContainerDef};
use crate::items::{ItemDef, ItemStack};
use crate::rng::Rng;

/// The key that matches any room type or any container kind.
pub const ANY: &str = ANY_ROOM;

/// Most picks one table or group can make, after `scale`.
const MAX_ROLLS: u32 = 64;

/// One item a table can give.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LootEntry {
    /// An item id (`items.ron`).
    pub item: String,
    /// Relative chance among the list's entries (positive).
    #[serde(default = "one_f32")]
    pub weight: f32,
    /// How many one pick gives, (min, max), uniform.
    #[serde(default = "one_one")]
    pub count: (u16, u16),
    /// Its weight is multiplied by [`LootData::rare_scale`].
    #[serde(default)]
    pub rare: bool,
}

/// An extra list rolled after a table's picks: with `chance`, `rolls` picks from `entries`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LootGroup {
    /// What it is, for people reading the file and the MCP preview ("cutlery drawer").
    #[serde(default)]
    pub name: String,
    #[serde(default = "one_f32")]
    pub chance: f32,
    #[serde(default = "one_one_u8")]
    pub rolls: (u8, u8),
    pub entries: Vec<LootEntry>,
}

/// What one kind of container holds in one room type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LootTable {
    /// A room type (`kitchen`, ...) or `any`.
    pub room: String,
    /// A container kind (`fridge`, ...) or `any`.
    pub container: String,
    /// Chance (0..1) it holds nothing at all.
    #[serde(default)]
    pub empty: f32,
    /// How many picks from `entries`, (min, max), uniform, times [`LootData::scale`].
    #[serde(default = "one_one_u8")]
    pub rolls: (u8, u8),
    #[serde(default)]
    pub entries: Vec<LootEntry>,
    #[serde(default)]
    pub groups: Vec<LootGroup>,
}

/// The whole of `loot.ron`. The default has no tables: every container holds nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LootData {
    /// Sandbox loot abundance: multiplies every table's and group's rolls.
    #[serde(default = "one_f32")]
    pub scale: f32,
    /// Multiplies the weight of `rare` entries (0: never).
    #[serde(default = "one_f32")]
    pub rare_scale: f32,
    #[serde(default)]
    pub tables: Vec<LootTable>,
}

impl Default for LootData {
    fn default() -> Self {
        Self { scale: 1.0, rare_scale: 1.0, tables: Vec::new() }
    }
}

fn one_f32() -> f32 {
    1.0
}

fn one_one() -> (u16, u16) {
    (1, 1)
}

fn one_one_u8() -> (u8, u8) {
    (1, 1)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl LootTable {
    /// "(kitchen, fridge)", for messages.
    pub fn key(&self) -> String {
        format!("({}, {})", self.room, self.container)
    }
}

fn check_entries(what: &str, entries: &[LootEntry]) -> Result<(), String> {
    for (i, e) in entries.iter().enumerate() {
        if !valid_id(&e.item) {
            return Err(format!("{what}: item {:?} must be an item id (lowercase letters, digits and _)", e.item));
        }
        if !(e.weight > 0.0 && e.weight <= 1000.0) {
            return Err(format!("{what}: {} has weight {}; weights must be in (0, 1000]", e.item, e.weight));
        }
        if e.count.0 == 0 || e.count.0 > e.count.1 || e.count.1 > 100 {
            return Err(format!("{what}: {} has count {:?}; it must be (min, max) with 1 <= min <= max <= 100", e.item, e.count));
        }
        if entries[..i].iter().any(|o| o.item == e.item) {
            return Err(format!("{what}: {} is listed twice (give it one entry with the weight of both)", e.item));
        }
    }
    Ok(())
}

fn check_rolls(what: &str, rolls: (u8, u8)) -> Result<(), String> {
    if rolls.0 > rolls.1 || rolls.1 as u32 > MAX_ROLLS {
        return Err(format!("{what}: rolls {rolls:?} must be (min, max) with min <= max <= {MAX_ROLLS}"));
    }
    Ok(())
}

impl LootData {
    /// Check the file on its own: the numbers are sane, every table has something to give (or says
    /// it holds nothing with `empty: 1.0`), and no `(room, container)` has two tables.
    pub fn validate(&self) -> Result<(), String> {
        if !(self.scale.is_finite() && (0.0..=10.0).contains(&self.scale)) {
            return Err(format!("scale must be in 0..10, got {}", self.scale));
        }
        if !(self.rare_scale.is_finite() && (0.0..=100.0).contains(&self.rare_scale)) {
            return Err(format!("rare_scale must be in 0..100, got {}", self.rare_scale));
        }
        for (i, t) in self.tables.iter().enumerate() {
            let what = format!("loot table {}", t.key());
            if !valid_id(&t.room) || !valid_id(&t.container) {
                return Err(format!("{what}: room and container must be lowercase ids (or \"any\")"));
            }
            if self.tables[..i].iter().any(|o| o.room == t.room && o.container == t.container) {
                return Err(format!("{what} is there twice"));
            }
            if !(0.0..=1.0).contains(&t.empty) {
                return Err(format!("{what}: empty must be a chance in 0..1, got {}", t.empty));
            }
            check_rolls(&what, t.rolls)?;
            if t.entries.is_empty() && t.groups.is_empty() {
                if t.empty < 1.0 {
                    return Err(format!("{what} is empty: give it entries, or `empty: 1.0` for a container that holds nothing"));
                }
            } else if t.entries.is_empty() && t.rolls.1 > 0 {
                return Err(format!("{what}: rolls is {:?} but there are no entries to pick from (rolls: (0, 0) for a table of groups only)", t.rolls));
            }
            check_entries(&what, &t.entries)?;
            for (k, g) in t.groups.iter().enumerate() {
                let what = format!("{what}, group {}", if g.name.is_empty() { (k + 1).to_string() } else { format!("{:?}", g.name) });
                if !(0.0..=1.0).contains(&g.chance) {
                    return Err(format!("{what}: chance must be in 0..1, got {}", g.chance));
                }
                check_rolls(&what, g.rolls)?;
                if g.entries.is_empty() {
                    return Err(format!("{what} has no entries"));
                }
                check_entries(&what, &g.entries)?;
            }
        }
        Ok(())
    }

    /// Check the tables against the content they name: every item is in `items` and every
    /// container kind in `containers` (or `any`).
    pub fn check(&self, items: &[ItemDef], containers: &ContainerData) -> Result<(), String> {
        for t in &self.tables {
            let what = format!("loot table {}", t.key());
            if t.container != ANY && !containers.containers.iter().any(|c| c.id == t.container) {
                return Err(format!("{what}: container {:?} isn't one of the containers in containers.ron", t.container));
            }
            let lists = std::iter::once(&t.entries).chain(t.groups.iter().map(|g| &g.entries));
            for e in lists.flatten() {
                if !items.iter().any(|d| d.id == e.item) {
                    return Err(format!("{what}: item {:?} isn't in items.ron", e.item));
                }
            }
        }
        Ok(())
    }

    /// The table a container of kind `container` uses in a room of type `room`: the first of
    /// `(room, container)`, `(any, container)`, `(room, any)`, `(any, any)`.
    pub fn find(&self, room: &str, container: &str) -> Option<&LootTable> {
        let get = |r: &str, c: &str| self.tables.iter().find(|t| t.room == r && t.container == c);
        get(room, container).or_else(|| get(ANY, container)).or_else(|| get(room, ANY)).or_else(|| get(ANY, ANY))
    }

    /// Parse and validate `loot.ron`, and check it against `items` and `containers`.
    pub fn parse_for(text: &str, items: &[ItemDef], containers: &ContainerData) -> Result<Self, String> {
        let data = parse_loot(text)?;
        data.check(items, containers)?;
        Ok(data)
    }
}

/// Parse and validate `loot.ron` on its own (see [`LootData::check`] for the item and container
/// names).
pub fn parse_loot(text: &str) -> Result<LootData, String> {
    let data: LootData = ron::from_str(text).map_err(|e| e.to_string())?;
    data.validate()?;
    Ok(data)
}

/// `n` picks times `scale`, the fraction rounded up with that chance.
fn scaled(rng: &mut Rng, rolls: (u8, u8), scale: f32) -> u32 {
    let (lo, hi) = (rolls.0 as u32, rolls.1 as u32);
    let base = lo + rng.below(hi - lo + 1);
    let x = base as f32 * scale;
    let n = x.floor() as u32 + (rng.f32() < x.fract()) as u32;
    n.min(MAX_ROLLS)
}

/// One weighted pick from `entries` (rare ones times `rare_scale`); `None` if every weight is 0.
fn pick<'a>(rng: &mut Rng, entries: &'a [LootEntry], rare_scale: f32) -> Option<&'a LootEntry> {
    let weight = |e: &LootEntry| e.weight * if e.rare { rare_scale } else { 1.0 };
    let total: f32 = entries.iter().map(weight).sum();
    let roll = rng.f32() * total;
    if total <= 0.0 {
        return None;
    }
    let mut left = roll;
    let mut last = None;
    for e in entries {
        let w = weight(e);
        if w <= 0.0 {
            continue;
        }
        last = Some(e);
        if left < w {
            break;
        }
        left -= w;
    }
    last
}

/// What's in a container as it fills: stacks, and the room they take.
struct Filling<'a> {
    items: &'a [ItemDef],
    room_l: f32,
    used_l: f32,
    out: Vec<ItemStack>,
}

impl Filling<'_> {
    /// Add `count` of `item`, as many as fit, topping up a stack of it before starting another.
    fn add(&mut self, item: &str, count: u16) {
        let Some(i) = self.items.iter().position(|d| d.id == item) else { return };
        let d = &self.items[i];
        let fit = ((self.room_l - self.used_l) / d.volume_l + 1e-4).floor().max(0.0);
        let mut left = (count as f32).min(fit) as u16;
        self.used_l += left as f32 * d.volume_l;
        let stack = d.stack.max(1);
        let item = i as u16;
        while left > 0 {
            match self.out.iter_mut().find(|s| s.item == item && s.count < stack) {
                Some(s) => {
                    let n = left.min(stack - s.count);
                    s.count += n;
                    left -= n;
                }
                None => {
                    let n = left.min(stack);
                    self.out.push(ItemStack::new(item, n));
                    left -= n;
                }
            }
        }
    }
}

fn roll_list(rng: &mut Rng, entries: &[LootEntry], n: u32, rare_scale: f32, into: &mut Filling) {
    for _ in 0..n {
        let Some(e) = pick(rng, entries, rare_scale) else { return };
        let (lo, hi) = e.count;
        let count = lo + rng.below((hi - lo) as u32 + 1) as u16;
        into.add(&e.item, count);
    }
}

/// Roll `table` for a container with `room_l` litres of room, from `rng`.
pub fn roll(rng: &mut Rng, table: &LootTable, loot: &LootData, room_l: f32, items: &[ItemDef]) -> Vec<ItemStack> {
    let mut f = Filling { items, room_l, used_l: 0.0, out: Vec::new() };
    if rng.f32() < table.empty {
        return f.out;
    }
    let n = scaled(rng, table.rolls, loot.scale);
    roll_list(rng, &table.entries, n, loot.rare_scale, &mut f);
    for g in &table.groups {
        if rng.f32() < g.chance {
            let n = scaled(rng, g.rolls, loot.scale);
            roll_list(rng, &g.entries, n, loot.rare_scale, &mut f);
        }
    }
    f.out
}

/// The random stream container `id` fills from: a hash of the world seed and the id alone.
pub fn container_rng(seed: u64, id: u32) -> Rng {
    Rng::new(seed ^ 0x100_7AB1_E5ED ^ (id as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// What container `id` (of kind `def`, in a room of type `room_type`) holds when it is first
/// opened: its loot table rolled from [`container_rng`]. Deterministic from the seed, the id and
/// the data, so containers fill the same in any order. Never more than the container's room.
pub fn fill(seed: u64, id: u32, def: &ContainerDef, room_type: &str, loot: &LootData, items: &[ItemDef]) -> Vec<ItemStack> {
    match loot.find(room_type, &def.id) {
        Some(table) => roll(&mut container_rng(seed, id), table, loot, def.room_l, items),
        None => Vec::new(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::containers::parse_containers;
    use crate::items::{ItemCategory, complete_items, parse_items, parse_weapons};
    use crate::template::BuildingTemplate;
    use crate::world::{World, WorldConfig};
    use glam::Vec2;

    pub(crate) fn shipped() -> LootData {
        parse_loot(include_str!("../../../assets/data/loot.ron")).unwrap()
    }

    fn items() -> Vec<ItemDef> {
        let weapons = parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap()
    }

    fn containers() -> ContainerData {
        parse_containers(include_str!("../../../assets/data/containers.ron")).unwrap()
    }

    fn templates() -> Vec<BuildingTemplate> {
        [include_str!("../../../assets/data/buildings/bungalow.ron"), include_str!("../../../assets/data/buildings/family_two_storey.ron")]
            .iter()
            .map(|t| BuildingTemplate::from_ron(t).unwrap())
            .collect()
    }

    fn content(seed: u64) -> WorldConfig {
        let weapons = parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        WorldConfig::new(seed, 0, templates()).with_weapons(weapons).with_items(items()).with_containers(containers()).with_loot(shipped()).with_map_chunks(8)
    }

    fn def<'a>(c: &'a ContainerData, id: &str) -> &'a ContainerDef {
        c.containers.iter().find(|d| d.id == id).unwrap()
    }

    #[test]
    fn loot_file_is_valid_against_the_content() {
        let mut it = items();
        let weapons = parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        complete_items(&mut it, &weapons);
        LootData::parse_for(include_str!("../../../assets/data/loot.ron"), &it, &containers()).unwrap();
        let l = shipped();
        assert_eq!((l.scale, l.rare_scale), (1.0, 1.0));
        for room in ["kitchen", "bathroom", "bedroom", "living", "dining", "hall", "laundry", "shed", "garage", "office", "shop"] {
            assert!(l.tables.iter().any(|t| t.room == room), "no table for {room} rooms");
        }
        // Every container kind has somewhere to fill from.
        for c in &containers().containers {
            assert!(l.find("nowhere_in_particular", &c.id).is_some(), "{} has no fallback", c.id);
        }
    }

    #[test]
    fn validation_names_mistakes() {
        let it = items();
        let c = containers();
        let bad = |edit: &dyn Fn(&mut LootData), want: &str| {
            let mut l = shipped();
            edit(&mut l);
            let err = l.validate().and_then(|_| l.check(&it, &c)).unwrap_err();
            assert!(err.contains(want), "{err:?} should mention {want:?}");
        };
        bad(&|l| l.tables[0].entries[0].item = "milkk".into(), "\"milkk\" isn't in items.ron");
        bad(&|l| l.tables[0].container = "safe".into(), "\"safe\" isn't one of the containers");
        bad(&|l| l.tables[0].entries[0].count = (3, 1), "count (3, 1)");
        bad(&|l| l.tables[0].entries[0].count = (0, 2), "count (0, 2)");
        bad(&|l| l.tables[0].entries[0].weight = 0.0, "weight 0");
        bad(&|l| l.tables[0].rolls = (5, 2), "rolls (5, 2)");
        bad(&|l| l.tables[0].empty = 1.5, "empty must be");
        bad(&|l| l.tables[0].entries.clear(), "is empty");
        bad(&|l| l.tables.iter_mut().find(|t| !t.groups.is_empty()).unwrap().entries.clear(), "no entries to pick from");
        bad(&|l| l.tables.iter_mut().find(|t| !t.groups.is_empty()).unwrap().groups[0].entries.clear(), "has no entries");
        bad(&|l| { let t = l.tables[0].clone(); l.tables.push(t) }, "there twice");
        bad(&|l| { let e = l.tables[0].entries[0].clone(); l.tables[0].entries.push(e) }, "listed twice");
        bad(&|l| l.scale = -1.0, "scale");
        bad(&|l| l.tables.iter_mut().find(|t| !t.groups.is_empty()).unwrap().groups[0].chance = 2.0, "chance");
        // The key is in the message, so the mistake can be found.
        let mut l = shipped();
        l.tables[0].entries[0].item = "milkk".into();
        assert!(l.check(&it, &c).unwrap_err().contains(&l.tables[0].key()));
        assert!(parse_loot("(tables: [], extra: 1)").unwrap_err().contains("extra"), "unknown fields are refused");
        assert!(parse_loot("(tables: [(room: \"shed\", container: \"any\", empty: 1.0)])").is_ok(), "a table that holds nothing on purpose");
        assert_eq!(parse_loot("()").unwrap(), LootData::default());
    }

    #[test]
    fn tables_fall_back_room_then_container_then_any() {
        let l = shipped();
        let key = |r: &str, c: &str| l.find(r, c).map(|t| t.key());
        assert_eq!(key("kitchen", "fridge").as_deref(), Some("(kitchen, fridge)"));
        assert_eq!(key("garage", "fridge").as_deref(), Some("(any, fridge)"), "a fridge is a fridge");
        assert_eq!(key("shed", "shelves").as_deref(), Some("(shed, any)"), "shelves hold what the room holds");
        assert_eq!(key("attic", "shelves").as_deref(), Some("(any, any)"));
        assert_eq!(LootData::default().find("kitchen", "fridge"), None);
    }

    /// Every container in the shipped templates' rooms has a table, and every room type has some.
    #[test]
    fn every_room_in_the_templates_has_a_table() {
        let l = shipped();
        let c = containers();
        for t in templates() {
            for level in &t.levels {
                for f in &level.furniture {
                    let Some(room) = level.layout.iter().nth(f.at.1 as usize).and_then(|row| row.chars().nth(f.at.0 as usize)).and_then(|ch| t.rooms.get(&ch)) else { continue };
                    if let Some(k) = c.lookup(&f.kind, &room.room_type) {
                        let kind = &c.containers[k as usize].id;
                        let table = l.find(&room.room_type, kind).unwrap_or_else(|| panic!("{}: a {} ({kind}) in a {} has no table", t.id, f.kind, room.room_type));
                        assert!(table.room == room.room_type || table.room == ANY);
                    }
                }
            }
            for room in t.rooms.values() {
                assert!(l.tables.iter().any(|tb| tb.room == room.room_type), "{}: no table names {} rooms", t.id, room.room_type);
            }
        }
        // And on a generated map, every container site fills from some table.
        let w = World::new(content(4));
        for s in w.containers() {
            let room = &w.map.rooms[s.room as usize].room_type;
            assert!(l.find(room, &w.container_def(s).id).is_some(), "container {} in a {room}", s.id);
        }
    }

    /// Fills are the same for the same seed and container, never overfill, and stacks stay within
    /// their stack size.
    #[test]
    fn fills_are_deterministic_and_respect_room() {
        let (it, c, l) = (items(), containers(), shipped());
        let mut tight = l.clone();
        tight.scale = 10.0;
        for kind in &c.containers {
            for room in ["kitchen", "bedroom", "shed", "shop", "pharmacy", "hardware", "garage"] {
                for id in 0..60 {
                    for loot in [&l, &tight] {
                        let a = fill(11, id, kind, room, loot, &it);
                        assert_eq!(a, fill(11, id, kind, room, loot, &it));
                        let volume: f32 = a.iter().map(|s| it[s.item as usize].volume_l * s.count as f32).sum();
                        assert!(volume <= kind.room_l + 1e-3, "{} in a {room}: {volume} L in {} L", kind.id, kind.room_l);
                        assert!(a.iter().all(|s| s.count >= 1 && s.count <= it[s.item as usize].stack), "{a:?}");
                    }
                }
            }
        }
        let fridge = def(&c, "fridge");
        assert!((0..50).any(|id| fill(7, id, fridge, "kitchen", &l, &it) != fill(8, id, fridge, "kitchen", &l, &it)), "the seed matters");
        assert!((0..50).any(|id| fill(7, id, fridge, "kitchen", &l, &it) != fill(7, id + 50, fridge, "kitchen", &l, &it)), "the id matters");
    }

    fn count_where(stacks: &[ItemStack], it: &[ItemDef], cat: ItemCategory) -> u32 {
        stacks.iter().filter(|s| it[s.item as usize].category == cat).map(|s| s.count as u32).sum()
    }

    /// The sandbox scale changes how much there is.
    #[test]
    fn scale_changes_amounts() {
        let (it, c) = (items(), containers());
        let total = |scale: f32| {
            let mut l = shipped();
            l.scale = scale;
            let mut n = 0;
            for kind in ["fridge", "kitchen_cupboard", "wardrobe", "bathroom_cabinet"] {
                let room = if kind == "wardrobe" { "bedroom" } else if kind == "bathroom_cabinet" { "bathroom" } else { "kitchen" };
                for id in 0..400 {
                    n += fill(3, id, def(&c, kind), room, &l, &it).iter().map(|s| s.count as u32).sum::<u32>();
                }
            }
            n
        };
        let (scarce, normal, plenty) = (total(0.25), total(1.0), total(2.0));
        assert!(scarce * 2 < normal && normal * 3 < plenty * 2, "scale 0.25 / 1 / 2: {scarce} / {normal} / {plenty} items");
        assert_eq!(total(0.0), 0);
        // rare_scale 0: no rare entries at all.
        let mut l = shipped();
        l.rare_scale = 0.0;
        let rare: Vec<&str> = l.tables.iter().flat_map(|t| t.entries.iter().chain(t.groups.iter().flat_map(|g| &g.entries))).filter(|e| e.rare).map(|e| e.item.as_str()).collect();
        assert!(!rare.is_empty());
        let cab = def(&c, "bathroom_cabinet");
        assert!((0..2000).all(|id| fill(5, id, cab, "bathroom", &l, &it).iter().all(|s| it[s.item as usize].id != "antibiotics")));
        assert!((0..2000).any(|id| fill(5, id, cab, "bathroom", &shipped(), &it).iter().any(|s| it[s.item as usize].id == "antibiotics")));
    }

    /// Kitchens are where the food is: per container, far more than in bedrooms.
    #[test]
    fn kitchens_give_food_far_more_than_bedrooms() {
        let (it, c, l) = (items(), containers(), shipped());
        let food = |room: &str, kinds: &[&str]| {
            let mut n = 0;
            for kind in kinds {
                for id in 0..1000 {
                    let s = fill(21, id, def(&c, kind), room, &l, &it);
                    n += count_where(&s, &it, ItemCategory::Food) + count_where(&s, &it, ItemCategory::Drink);
                }
            }
            n as f32 / (1000 * kinds.len()) as f32
        };
        let kitchen = food("kitchen", &["fridge", "kitchen_cupboard", "oven", "under_sink"]);
        let bedroom = food("bedroom", &["wardrobe", "desk_drawers"]);
        assert!(kitchen > 1.5 && kitchen > bedroom * 6.0, "food and drink per container: kitchen {kitchen:.2}, bedroom {bedroom:.2}");
        // And bathrooms are where the medicine is, wardrobes the clothes, sheds the tools.
        let per = |room: &str, kind: &str, cat: ItemCategory| (0..1000).map(|id| count_where(&fill(22, id, def(&c, kind), room, &l, &it), &it, cat)).sum::<u32>() as f32 / 1000.0;
        assert!(per("bathroom", "bathroom_cabinet", ItemCategory::Medical) > 4.0 * per("kitchen", "kitchen_cupboard", ItemCategory::Medical).max(0.05));
        assert!(per("bedroom", "wardrobe", ItemCategory::Clothing) > 2.0);
        let shed = per("shed", "shelves", ItemCategory::Tool) + per("shed", "shelves", ItemCategory::Material) + per("shed", "shelves", ItemCategory::Weapon);
        assert!(shed > 2.0 && shed > 4.0 * per("bedroom", "wardrobe", ItemCategory::Tool).max(0.05), "tools, materials and tool-weapons per shed shelf: {shed:.2}");
    }

    /// Step until player `id` has nothing queued.
    fn settle(w: &mut World, id: u32) {
        for _ in 0..400 {
            if w.player(id).unwrap().jobs.is_empty() {
                return;
            }
            w.step();
        }
        panic!("jobs never finished");
    }

    /// Stand beside container `c` (in its room) and search it.
    fn search(w: &mut World, id: u32, c: u32) {
        let site = w.container(c).unwrap().clone();
        let spots = (-1..=site.size.y).flat_map(|dy| (-1..=site.size.x).map(move |dx| site.min + glam::IVec2::new(dx, dy)));
        let Some(t) = spots.filter(|&t| w.map.in_bounds(t) && !w.map.tile_blocked(t) && w.map.tile(t).room == site.room).min_by_key(|t| (t.x, t.y)) else { return };
        let pi = w.players.iter().position(|p| p.id == id).unwrap();
        w.players[pi].state.pos = t.as_vec2() + Vec2::splat(0.5);
        if w.player_search(id, Some(c)).is_ok() {
            settle(w, id);
        }
    }

    /// Two houses opened in two orders, one of them saved and loaded half way: the same contents.
    #[test]
    fn the_same_seed_fills_the_same_in_any_order_and_across_a_save() {
        let w = World::new(content(9));
        let mut ids: Vec<u32> = w.containers().iter().filter(|s| s.building as usize == w.spawn_building).map(|s| s.id).collect();
        let other = w.containers().iter().map(|s| s.building).find(|&b| b as usize != w.spawn_building).unwrap();
        ids.extend(w.containers().iter().filter(|s| s.building == other).map(|s| s.id));
        assert!(ids.len() >= 8, "{ids:?}");
        let mut a = World::new(content(9));
        let pa = a.add_player(a.player_spawn);
        for &c in &ids {
            search(&mut a, pa, c);
        }
        let mut b = World::new(content(9));
        let pb = b.add_player(b.player_spawn);
        let rev: Vec<u32> = ids.iter().rev().copied().collect();
        for &c in &rev[..rev.len() / 2] {
            search(&mut b, pb, c);
        }
        let mut b = World::load(&b.save(), &content(9)).unwrap();
        for &c in &rev[rev.len() / 2..] {
            search(&mut b, pb, c);
        }
        let got = |w: &World| ids.iter().map(|&c| (c, w.container_contents(c).map(<[ItemStack]>::to_vec))).collect::<Vec<_>>();
        assert_eq!(got(&a), got(&b));
        assert!(got(&a).iter().all(|(_, s)| s.is_some()), "every one opened");
        assert!(got(&a).iter().filter(|(_, s)| s.as_ref().is_some_and(|s| !s.is_empty())).count() >= 4);
    }

    /// Wherever the player wakes up, there's something to fight with in the house: a weapon on the
    /// floor or in its furniture.
    #[test]
    fn the_spawn_house_always_offers_a_weapon() {
        for seed in 1..=12 {
            let w = World::new(content(seed));
            let house = w.spawn_building;
            let floor = w.items().iter().any(|g| w.map.building_at(g.pos) == Some(house) && w.item_weapon(g.stack.item).is_some());
            let furniture = w.containers().iter().filter(|s| s.building as usize == house).any(|s| {
                let room = &w.map.rooms[s.room as usize].room_type;
                fill(w.config.seed(), s.id, w.container_def(s), room, &w.config.loot, &w.config.items).iter().any(|st| w.item_weapon(st.item).is_some())
            });
            assert!(floor || furniture, "seed {seed}: nothing to fight with in the spawn house");
        }
        // Weapons are loot too: a kitchen often has a knife in a drawer.
        let (it, c, l) = (items(), containers(), shipped());
        let knives = (0..1000).filter(|&id| fill(1, id, def(&c, "kitchen_cupboard"), "kitchen", &l, &it).iter().any(|s| it[s.item as usize].id == "kitchen_knife")).count();
        assert!((80..400).contains(&knives), "a kitchen knife in {knives} of 1000 kitchen cupboards");
    }

    /// The food in every container of the default town is enough for one player for about two to
    /// four weeks at the default scale (a design target of #8; ours to tune). How much hunger a day
    /// takes is our assumption until needs (#11) set it: 2.0, about 2,000 kcal at a tin of beans'
    /// 0.30 for some 350 kcal.
    #[test]
    fn the_default_town_feeds_one_player_for_weeks() {
        const HUNGER_PER_DAY: f32 = 2.0;
        let mut days = Vec::new();
        for seed in 1..=3 {
            // The town is the same size on any map; a bigger map only adds forest.
            let w = World::new(content(seed));
            let mut hunger = 0.0;
            for s in w.containers() {
                let room = &w.map.rooms[s.room as usize].room_type;
                for st in fill(w.config.seed(), s.id, w.container_def(s), room, &w.config.loot, &w.config.items) {
                    if let Some(f) = &w.config.items[st.item as usize].food {
                        hunger += f.hunger.max(0.0) * f.uses as f32 * st.count as f32;
                    }
                }
            }
            days.push(hunger / HUNGER_PER_DAY);
        }
        let mean = days.iter().sum::<f32>() / days.len() as f32;
        eprintln!("days of food for one player in the default town, seeds 1-3: {days:.1?}");
        assert!((14.0..=28.0).contains(&mean), "days of food for one player in the default town, seeds 1-3: {days:.1?}");
    }
}
