//! Building templates: hand-designed, data-driven building layouts (RON files in
//! `assets/data/buildings/`). The town generator stamps them onto the map, rotated and mirrored.
//!
//! A template is drawn as ASCII, one character per tile per level. Walls appear automatically
//! wherever two different rooms meet (or a room meets the outside, `.`), and doors, windows and
//! open archways are placed on edges by tile and side. Local coordinates: x to the right, y
//! downward; **the front of the building is the south side** (bottom row), which faces the street.

use std::collections::BTreeMap;

use glam::IVec2;
use serde::{Deserialize, Serialize};

use crate::map::{Building, Edge, Feature, Floor, OwnedEdge, PlacedFurniture, Room, RoofStyle, Side, TileMap, UpperEdge};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BuildingKind {
    House,
    Office,
    Shop,
    /// A landmark: town hall, courthouse, church, school, library, police station. The town
    /// generator puts these (and shops or offices too big for the main street's lots) on the civic
    /// district's landmark lots, about 42–52 m wide and 48–54 m deep.
    Civic,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomDef {
    pub name: String,
    /// Drives loot tables and what furniture makes sense (bedroom, kitchen, bathroom, ...).
    pub room_type: String,
    pub floor: Floor,
}

/// An edge opening: tile `(x, y)` and which side of it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Opening(pub i32, pub i32, pub Side);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FurnitureDef {
    pub kind: String,
    pub at: (i32, i32),
    #[serde(default = "one_by_one")]
    pub size: (i32, i32),
    #[serde(default = "south")]
    pub facing: Side,
}

fn one_by_one() -> (i32, i32) {
    (1, 1)
}

fn south() -> Side {
    Side::S
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LevelDef {
    /// One string per row; one character per tile. `.` = not part of the building.
    pub layout: Vec<String>,
    #[serde(default)]
    pub doors: Vec<Opening>,
    #[serde(default)]
    pub windows: Vec<Opening>,
    #[serde(default)]
    pub arches: Vec<Opening>,
    #[serde(default)]
    pub furniture: Vec<FurnitureDef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildingTemplate {
    pub id: String,
    pub name: String,
    pub kind: BuildingKind,
    /// Roof style in template-local axes.
    pub roof: RoofStyle,
    /// Layout character → room.
    pub rooms: BTreeMap<char, RoomDef>,
    /// Level 0 first. All levels share the same footprint size.
    pub levels: Vec<LevelDef>,
}

/// How a template is placed: footprint min corner, clockwise quarter turns, mirror along x.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub origin: IVec2,
    pub quarter_turns: u8,
    pub mirror: bool,
}

/// What placing a building produced, for the generator (paths, door states, spawn points).
#[derive(Clone, Debug)]
pub struct Placed {
    pub building: usize,
    /// Doors on the outside of the building: world tile (inside) and world side.
    pub exterior_doors: Vec<(IVec2, Side, bool)>,
    /// Ground-floor room ids by local room character.
    pub rooms: BTreeMap<char, u16>,
}

impl BuildingTemplate {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let t: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        t.validate()?;
        Ok(t)
    }

    /// Footprint size in local tiles.
    pub fn size(&self) -> IVec2 {
        let l = &self.levels[0].layout;
        IVec2::new(l[0].chars().count() as i32, l.len() as i32)
    }

    /// Footprint size after rotation.
    pub fn rotated_size(&self, quarter_turns: u8) -> IVec2 {
        let s = self.size();
        if quarter_turns % 2 == 1 { IVec2::new(s.y, s.x) } else { s }
    }

    fn cell(&self, level: usize, p: IVec2) -> char {
        let s = self.size();
        if p.x < 0 || p.y < 0 || p.x >= s.x || p.y >= s.y {
            return '.';
        }
        self.levels[level].layout[p.y as usize].chars().nth(p.x as usize).unwrap_or('.')
    }

    pub fn validate(&self) -> Result<(), String> {
        let id = &self.id;
        if self.levels.is_empty() {
            return Err(format!("{id}: no levels"));
        }
        let size = self.size();
        for (li, level) in self.levels.iter().enumerate() {
            if level.layout.len() as i32 != size.y || level.layout.iter().any(|r| r.chars().count() as i32 != size.x) {
                return Err(format!("{id}: level {li} layout must be {}×{} like level 0", size.x, size.y));
            }
            for row in &level.layout {
                if let Some(c) = row.chars().find(|c| *c != '.' && !self.rooms.contains_key(c)) {
                    return Err(format!("{id}: level {li} uses room '{c}' which is not in `rooms`"));
                }
            }
            let mut openings: BTreeMap<(i32, i32, u8), &str> = BTreeMap::new();
            for (what, list) in [("door", &level.doors), ("window", &level.windows), ("arch", &level.arches)] {
                for o in list {
                    let a = IVec2::new(o.0, o.1);
                    let b = a + o.2.offset();
                    let (ca, cb) = (self.cell(li, a), self.cell(li, b));
                    if ca == '.' || ca == cb {
                        return Err(format!("{id}: level {li} {what} at ({}, {}) {:?} is not on a wall between two rooms or the outside", o.0, o.1, o.2));
                    }
                    let (owner, which) = o.2.owner(a);
                    if let Some(other) = openings.insert((owner.x, owner.y, matches!(which, OwnedEdge::West) as u8), what) {
                        return Err(format!("{id}: level {li} {what} at ({}, {}) {:?} is on the same edge as a {other}", o.0, o.1, o.2));
                    }
                }
            }
            let mut taken: BTreeMap<(i32, i32), &str> = BTreeMap::new();
            for f in &level.furniture {
                if f.size.0 < 1 || f.size.1 < 1 {
                    return Err(format!("{id}: level {li} furniture '{}' at {:?} has size {:?}", f.kind, f.at, f.size));
                }
                let room = self.cell(li, IVec2::new(f.at.0, f.at.1));
                for dy in 0..f.size.1 {
                    for dx in 0..f.size.0 {
                        let c = self.cell(li, IVec2::new(f.at.0 + dx, f.at.1 + dy));
                        if c == '.' {
                            return Err(format!("{id}: level {li} furniture '{}' at {:?} is outside the building", f.kind, f.at));
                        }
                        if c != room {
                            return Err(format!("{id}: level {li} furniture '{}' at {:?} size {:?} crosses a wall between rooms '{room}' and '{c}'", f.kind, f.at, f.size));
                        }
                        if let Some(other) = taken.insert((f.at.0 + dx, f.at.1 + dy), &f.kind) {
                            return Err(format!("{id}: level {li} furniture '{}' at {:?} overlaps '{other}' at ({}, {})", f.kind, f.at, f.at.0 + dx, f.at.1 + dy));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Stamp this template onto the map.
    pub fn place(&self, map: &mut TileMap, at: Placement, siding: u8, roof_color: u8) -> Placed {
        let size = self.size();
        let xf = |p: IVec2| transform_cell(p, size, at);
        let xs = |s: Side| transform_side(s, at);
        let building = map.buildings.len();
        let world_size = self.rotated_size(at.quarter_turns);
        let mut placed = Placed { building, exterior_doors: Vec::new(), rooms: BTreeMap::new() };
        let mut upper_floors = Vec::new();
        let mut upper_edges = Vec::new();
        let mut furniture = Vec::new();

        for (li, level) in self.levels.iter().enumerate() {
            // Rooms for this level.
            let mut room_ids: BTreeMap<char, u16> = BTreeMap::new();
            for (c, def) in &self.rooms {
                if level.layout.iter().any(|r| r.contains(*c)) {
                    map.rooms.push(Room { building: building as u16, name: def.name.clone(), room_type: def.room_type.clone(), floor: def.floor });
                    room_ids.insert(*c, (map.rooms.len() - 1) as u16);
                }
            }
            let room_of = |c: char| room_ids.get(&c).copied().unwrap_or(0);

            // Floors.
            for y in 0..size.y {
                for x in 0..size.x {
                    let c = self.cell(li, IVec2::new(x, y));
                    if c == '.' {
                        continue;
                    }
                    let w = xf(IVec2::new(x, y));
                    if li == 0 {
                        if let Some(tile) = map.tile_mut(w) {
                            tile.floor = self.rooms[&c].floor;
                            tile.room = room_of(c);
                            tile.feature = Feature::None;
                            tile.north = Edge::None;
                            tile.west = Edge::None;
                        }
                    } else {
                        upper_floors.push((li as i8, w, room_of(c)));
                    }
                }
            }

            // Edges, keyed by their canonical owner in local space so each is set once.
            let mut edges: BTreeMap<(i32, i32, u8), Edge> = BTreeMap::new();
            let key = |p: IVec2, s: Side| {
                let (o, which) = s.owner(p);
                (o.x, o.y, matches!(which, OwnedEdge::West) as u8)
            };
            for y in -1..=size.y {
                for x in -1..=size.x {
                    let p = IVec2::new(x, y);
                    let c = self.cell(li, p);
                    for s in [Side::E, Side::S] {
                        let n = self.cell(li, p + s.offset());
                        if c != n && (c != '.' || n != '.') {
                            edges.insert(key(p, s), Edge::Wall);
                        }
                    }
                }
            }
            for o in &level.windows {
                edges.insert(key(IVec2::new(o.0, o.1), o.2), Edge::Window);
            }
            for o in &level.arches {
                edges.insert(key(IVec2::new(o.0, o.1), o.2), Edge::Arch);
            }
            for o in &level.doors {
                edges.insert(key(IVec2::new(o.0, o.1), o.2), Edge::DoorClosed);
                let a = IVec2::new(o.0, o.1);
                if li == 0 && self.cell(0, a + o.2.offset()) == '.' {
                    placed.exterior_doors.push((xf(a), xs(o.2), false));
                }
            }
            for ((x, y, west), kind) in edges {
                // Express the local edge as (tile, side) then transform both.
                let (tile, side) = if west == 1 { (IVec2::new(x, y), Side::W) } else { (IVec2::new(x, y), Side::N) };
                let neighbour = tile + side.offset();
                let rooms = (room_of(self.cell(li, neighbour)), room_of(self.cell(li, tile)));
                let (wt, ws) = (xf(tile), xs(side));
                if li == 0 {
                    map.set_edge(wt, ws, kind);
                } else {
                    let (owner, which) = ws.owner(wt);
                    // `rooms.0` is the room beyond `side`; after the transform the owner may be the other tile.
                    let rooms = if owner == wt { rooms } else { (rooms.1, rooms.0) };
                    upper_edges.push((li as i8, UpperEdge { tile: owner, edge: which, kind, rooms }));
                }
            }

            // Furniture.
            for f in &level.furniture {
                let a = xf(IVec2::new(f.at.0, f.at.1));
                let b = xf(IVec2::new(f.at.0 + f.size.0 - 1, f.at.1 + f.size.1 - 1));
                let (min, max) = (a.min(b), a.max(b));
                if li == 0 {
                    for y in min.y..=max.y {
                        for x in min.x..=max.x {
                            if let Some(t) = map.tile_mut(IVec2::new(x, y)) {
                                t.feature = Feature::Furniture;
                            }
                        }
                    }
                }
                furniture.push(PlacedFurniture { kind: f.kind.clone(), min, size: max - min + IVec2::ONE, facing: xs(f.facing), level: li as i8 });
            }
            if li == 0 {
                placed.rooms = room_ids;
            }
        }

        let roof = match (self.roof, at.quarter_turns % 2) {
            (RoofStyle::GableX, 1) => RoofStyle::GableY,
            (RoofStyle::GableY, 1) => RoofStyle::GableX,
            (r, _) => r,
        };
        let min = xf(IVec2::ZERO).min(xf(size - IVec2::ONE));
        map.buildings.push(Building {
            template: self.id.clone(),
            min,
            size: world_size,
            levels: self.levels.len() as u8,
            upper_floors,
            upper_edges,
            furniture,
            roof,
            siding,
            roof_color,
        });
        placed
    }
}

/// Local template cell → world tile.
fn transform_cell(p: IVec2, size: IVec2, at: Placement) -> IVec2 {
    let mut q = if at.mirror { IVec2::new(size.x - 1 - p.x, p.y) } else { p };
    let mut dims = size;
    for _ in 0..at.quarter_turns % 4 {
        // Clockwise with +y down: (x, y) → (h - 1 - y, x).
        q = IVec2::new(dims.y - 1 - q.y, q.x);
        dims = IVec2::new(dims.y, dims.x);
    }
    at.origin + q
}

fn transform_side(s: Side, at: Placement) -> Side {
    let s = if at.mirror { s.mirrored_x() } else { s };
    s.rotated(at.quarter_turns % 4)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const BUNGALOW: &str = include_str!("../../../assets/data/buildings/bungalow.ron");
    pub const FAMILY: &str = include_str!("../../../assets/data/buildings/family_two_storey.ron");

    /// Every template in `assets/data/buildings`, sorted by file name like `net::content` loads
    /// them (tests only: the sim itself does no I/O).
    pub(crate) fn shipped_templates() -> Vec<BuildingTemplate> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data/buildings");
        let mut paths: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "ron")).collect();
        paths.sort();
        paths.iter().map(|p| BuildingTemplate::from_ron(&std::fs::read_to_string(p).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).collect()
    }

    /// Furniture kinds that are containers in any room, and kinds that never are.
    const CONTAINER_KINDS: [&str; 12] = ["fridge", "counter", "stove", "sink", "wardrobe", "desk", "coffee_table", "washer", "dryer", "tv", "shelf", "workbench"];
    const PLAIN_KINDS: [&str; 6] = ["bed", "sofa", "table", "toilet", "bathtub", "stairs"];

    /// Walks a template level by level, in its own coordinates, and says what's wrong: a room you
    /// can't get into (on the ground floor from outside through doors and arches, upstairs from the
    /// top of the stairs), a door or arch with furniture in front of it, or furniture with no
    /// reachable free tile beside it (corners count) in its own room.
    fn walk_check(t: &BuildingTemplate) -> Result<(), String> {
        let size = t.size();
        let inside = |li: usize, p: IVec2| t.cell(li, p) != '.';
        let mut stairs: Vec<IVec2> = Vec::new();
        for f in t.levels[0].furniture.iter().filter(|f| f.kind == "stairs") {
            for dy in 0..f.size.1 {
                for dx in 0..f.size.0 {
                    stairs.push(IVec2::new(f.at.0 + dx, f.at.1 + dy));
                }
            }
        }
        if t.levels.len() > 1 && stairs.is_empty() {
            return Err(format!("{}: {} levels but no stairs", t.id, t.levels.len()));
        }
        let edge = |p: IVec2, s: Side| {
            let (o, w) = s.owner(p);
            (o.x, o.y, matches!(w, OwnedEdge::West) as u8)
        };
        let idx = |p: IVec2| (p.y * size.x + p.x) as usize;
        for (li, level) in t.levels.iter().enumerate() {
            let open: BTreeMap<_, ()> = level.doors.iter().chain(&level.arches).map(|o| (edge(IVec2::new(o.0, o.1), o.2), ())).collect();
            let mut blocked = vec![false; (size.x * size.y) as usize];
            for f in &level.furniture {
                for dy in 0..f.size.1 {
                    for dx in 0..f.size.0 {
                        blocked[idx(IVec2::new(f.at.0 + dx, f.at.1 + dy))] = true;
                    }
                }
            }
            let free = |p: IVec2| inside(li, p) && !blocked[idx(p)];
            let step = |a: IVec2, s: Side| {
                let b = a + s.offset();
                free(b) && (t.cell(li, a) == t.cell(li, b) || open.contains_key(&edge(a, s)))
            };
            for o in level.doors.iter().chain(&level.arches) {
                for p in [IVec2::new(o.0, o.1), IVec2::new(o.0, o.1) + o.2.offset()] {
                    if inside(li, p) && !free(p) {
                        return Err(format!("{}: level {li} furniture blocks the opening at ({}, {}) {:?}", t.id, o.0, o.1, o.2));
                    }
                }
            }
            let mut starts = Vec::new();
            if li == 0 {
                for o in level.doors.iter().chain(&level.arches) {
                    let a = IVec2::new(o.0, o.1);
                    if !inside(0, a + o.2.offset()) {
                        starts.push(a);
                    }
                }
                if starts.is_empty() {
                    return Err(format!("{}: no door or arch to the outside", t.id));
                }
            } else {
                for &p in &stairs {
                    if !free(p) {
                        return Err(format!("{}: level {li} has no free floor over the stairs at {p}", t.id));
                    }
                    starts.push(p);
                }
            }
            let mut seen = vec![false; blocked.len()];
            let mut queue = std::collections::VecDeque::new();
            for p in starts {
                seen[idx(p)] = true;
                queue.push_back(p);
            }
            while let Some(p) = queue.pop_front() {
                for s in Side::ALL {
                    let q = p + s.offset();
                    if step(p, s) && !seen[idx(q)] {
                        seen[idx(q)] = true;
                        queue.push_back(q);
                    }
                }
            }
            for (c, room) in &t.rooms {
                let cells: Vec<IVec2> = (0..size.y).flat_map(|y| (0..size.x).map(move |x| IVec2::new(x, y))).filter(|&p| t.cell(li, p) == *c).collect();
                if !cells.is_empty() && !cells.iter().any(|&p| seen[idx(p)]) {
                    return Err(format!("{}: level {li} room '{c}' ({}) can't be reached", t.id, room.name));
                }
            }
            for f in &level.furniture {
                let room = t.cell(li, IVec2::new(f.at.0, f.at.1));
                // A tile touching the footprint, corners too: a container opens from within 1.6 m
                // of its edge (`CONTAINER_REACH`), so a diagonal neighbour reaches it.
                let mut reachable = false;
                for y in f.at.1 - 1..=f.at.1 + f.size.1 {
                    for x in f.at.0 - 1..=f.at.0 + f.size.0 {
                        let q = IVec2::new(x, y);
                        reachable |= free(q) && t.cell(li, q) == room && seen[idx(q)];
                    }
                }
                if !reachable {
                    return Err(format!("{}: level {li} '{}' at {:?} has nowhere to stand next to it", t.id, f.kind, f.at));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn shipped_templates_parse_and_validate() {
        for text in [BUNGALOW, FAMILY] {
            let t = BuildingTemplate::from_ron(text).unwrap();
            assert!(t.size().x > 0);
        }
        let all = shipped_templates();
        let count = |k: fn(&BuildingKind) -> bool| all.iter().filter(|t| k(&t.kind)).count();
        assert_eq!(count(|k| matches!(k, BuildingKind::House)), 6, "six house types");
        assert_eq!(count(|k| matches!(k, BuildingKind::Office)), 2, "two office types");
        assert_eq!(count(|k| matches!(k, BuildingKind::Shop)), 2, "two shops");
        for (i, t) in all.iter().enumerate() {
            assert!(all[..i].iter().all(|o| o.id != t.id), "template id {} used twice", t.id);
        }
    }

    /// Every room of every shipped template can be walked into, nothing stands in a doorway, and
    /// every piece of furniture has somewhere to stand next to it.
    #[test]
    fn every_room_is_reachable_and_nothing_blocks_a_door() {
        for t in shipped_templates() {
            walk_check(&t).unwrap();
            // Houses fit the generator's lots (see `worldgen::place_house`): at most 13 wide, 15 deep.
            if matches!(t.kind, BuildingKind::House) {
                assert!(t.size().x <= 13 && t.size().y <= 15, "{} is {} too big for a lot", t.id, t.size());
            }
            // The front faces the street: some door or arch leads out of the bottom row.
            let s = t.size();
            assert!(t.levels[0].doors.iter().chain(&t.levels[0].arches).any(|o| o.2 == Side::S && o.1 == s.y - 1), "{}: no way in from the front", t.id);
        }
    }

    /// The walk check and validation catch a room with no way in, a blocked door, overlapping
    /// furniture, furniture through a wall and two openings on one edge.
    #[test]
    fn layout_checks_explain_mistakes() {
        let bungalow = || BuildingTemplate::from_ron(BUNGALOW).unwrap();
        let shelf = |at: (i32, i32), size: (i32, i32)| FurnitureDef { kind: "shelf".into(), at, size, facing: Side::S };
        walk_check(&bungalow()).unwrap();
        let mut t = bungalow();
        t.levels[0].doors.retain(|d| (d.0, d.1) != (5, 2));
        assert!(walk_check(&t).unwrap_err().contains("Bathroom"));
        let mut t = bungalow();
        t.levels[0].furniture.push(shelf((2, 7), (1, 1)));
        assert!(walk_check(&t).unwrap_err().contains("blocks the opening"));
        let mut t = bungalow();
        t.levels[0].furniture.push(shelf((2, 1), (1, 1)));
        assert!(t.validate().unwrap_err().contains("overlaps"));
        let mut t = bungalow();
        t.levels[0].furniture.push(shelf((3, 3), (2, 1)));
        assert!(t.validate().unwrap_err().contains("crosses a wall"));
        let mut t = bungalow();
        t.levels[0].windows.push(Opening(2, 7, Side::S));
        assert!(t.validate().unwrap_err().contains("same edge"));
    }

    /// Furniture that should hold things maps to a container in its room (containers.ron), and
    /// shelves and benches in shops, offices, garages and laundries hold that room's own stock
    /// (loot.ron), not the kitchen's or the clutter table.
    #[test]
    fn furniture_maps_to_the_intended_containers() {
        let containers = crate::containers::parse_containers(include_str!("../../../assets/data/containers.ron")).unwrap();
        let loot = crate::loot::parse_loot(include_str!("../../../assets/data/loot.ron")).unwrap();
        let mut room_types = std::collections::BTreeSet::new();
        for t in shipped_templates() {
            for f in &t.levels[0].furniture {
                let room = &t.rooms[&t.cell(0, IVec2::new(f.at.0, f.at.1))];
                room_types.insert(room.room_type.clone());
                let found = containers.lookup(&f.kind, &room.room_type);
                if CONTAINER_KINDS.contains(&f.kind.as_str()) {
                    let def = &containers.containers[found.unwrap_or_else(|| panic!("{}: '{}' in the {} is no container", t.id, f.kind, room.name)) as usize];
                    let table = loot.find(&room.room_type, &def.id).unwrap_or_else(|| panic!("{}: no loot table for {} in {}", t.id, def.id, room.room_type));
                    if ["shelf", "workbench"].contains(&f.kind.as_str()) && ["shop", "pharmacy", "hardware", "office", "garage", "laundry"].contains(&room.room_type.as_str()) {
                        assert_eq!(table.room, room.room_type, "{}: '{}' in the {} fills from {}", t.id, f.kind, room.name, table.key());
                    }
                    // Shops keep no kitchen cupboards: a counter there would fill with groceries from the kitchen table.
                    if ["shop", "pharmacy", "hardware"].contains(&room.room_type.as_str()) {
                        assert_ne!(f.kind, "counter", "{}: a counter in the {} holds kitchen things", t.id, room.name);
                    }
                } else {
                    assert!(PLAIN_KINDS.contains(&f.kind.as_str()), "{}: furniture kind '{}' isn't known to this test", t.id, f.kind);
                    assert_eq!(found, None, "{}: '{}' shouldn't be a container", t.id, f.kind);
                }
            }
        }
        for want in ["garage", "laundry", "office", "shop", "pharmacy", "hardware"] {
            assert!(room_types.contains(want), "no template has a {want}");
        }
    }

    /// Every shipped template stamps onto an empty map whichever way it's turned: sealed but for
    /// its doors, windows and arches, with a way in from the street side, all its furniture placed.
    #[test]
    fn every_template_places_sealed_with_its_front_to_the_street() {
        for t in shipped_templates() {
            for (turns, mirror, street) in [(0, false, Side::S), (2, true, Side::N), (1, false, Side::W), (3, true, Side::E)] {
                let mut map = TileMap::new(4, 4);
                let placed = t.place(&mut map, Placement { origin: IVec2::new(20, 20), quarter_turns: turns, mirror }, 0, 0);
                let b = &map.buildings[placed.building];
                assert_eq!(b.size, t.rotated_size(turns));
                let mut front = false;
                for y in b.min.y..b.min.y + b.size.y {
                    for x in b.min.x..b.min.x + b.size.x {
                        let p = IVec2::new(x, y);
                        for s in Side::ALL {
                            if map.tile(p).room != 0 && map.tile(p + s.offset()).room == 0 {
                                match map.edge(p, s) {
                                    Edge::Wall | Edge::Window => {}
                                    Edge::DoorClosed | Edge::Arch => front |= s == street,
                                    other => panic!("{}: gap in the outer wall at {p} {s:?}: {other:?}", t.id),
                                }
                            }
                        }
                    }
                }
                assert!(front, "{} turned {turns}: no way in from the {street:?}", t.id);
                assert_eq!(b.furniture.len(), t.levels.iter().map(|l| l.furniture.len()).sum::<usize>());
            }
        }
    }

    #[test]
    fn rotated_placement_keeps_the_front_door_facing_the_street() {
        let t = BuildingTemplate::from_ron(BUNGALOW).unwrap();
        for (turns, expect) in [(0, Side::S), (2, Side::N), (1, Side::W), (3, Side::E)] {
            let mut map = TileMap::new(4, 4);
            let placed = t.place(&mut map, Placement { origin: IVec2::new(10, 10), quarter_turns: turns, mirror: turns == 2 }, 0, 0);
            let (door, side, _) = placed.exterior_doors.iter().find(|d| d.1 == expect).copied().expect("front door");
            assert_eq!(map.edge(door, side), Edge::DoorClosed);
            assert_eq!(map.tile(door + side.offset()).room, 0, "front door should lead outside");
            assert_ne!(map.tile(door).room, 0);
        }
    }

    #[test]
    fn building_is_sealed_except_for_openings() {
        let t = BuildingTemplate::from_ron(BUNGALOW).unwrap();
        let mut map = TileMap::new(4, 4);
        t.place(&mut map, Placement { origin: IVec2::new(10, 10), quarter_turns: 0, mirror: false }, 0, 0);
        let size = t.size();
        let mut openings = 0;
        for y in 10..10 + size.y {
            for x in 10..10 + size.x {
                let p = IVec2::new(x, y);
                for s in Side::ALL {
                    if map.tile(p + s.offset()).room == 0 && map.tile(p).room != 0 {
                        match map.edge(p, s) {
                            Edge::Wall | Edge::Window => {}
                            Edge::DoorClosed => openings += 1,
                            other => panic!("gap in the outer wall at {p} {s:?}: {other:?}"),
                        }
                    }
                }
            }
        }
        assert!(openings >= 1);
    }
}
