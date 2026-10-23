//! Seeded town generator: a few street blocks of houses stamped from templates, ringed by forest.
//! The same seed and templates always give the same map.

use glam::{IVec2, Vec2};

use crate::map::{Edge, Feature, Floor, Side, TileMap, CHUNK_SIZE};
use crate::rng::Rng;
use crate::template::{BuildingTemplate, Placement};

/// Asphalt width of a street, and the sidewalk on each side.
const ROAD: i32 = 7;
const SIDEWALK: i32 = 2;
const CORRIDOR: i32 = ROAD + 2 * SIDEWALK;
const FRONT_SETBACK: i32 = 5;
/// Forest kept at least this wide round the town when the map is made bigger to fit it.
const MIN_FOREST: i32 = 24;

#[derive(Clone, Debug)]
pub struct GenConfig {
    pub seed: u64,
    /// Map size in chunks (each 16 m). Grown if the town wouldn't fit.
    pub chunks: IVec2,
    pub blocks: IVec2,
}

impl Default for GenConfig {
    fn default() -> Self {
        Self { seed: 1, chunks: IVec2::new(20, 20), blocks: IVec2::new(3, 2) }
    }
}

pub struct Generated {
    pub map: TileMap,
    /// Town bounds (tiles), used for zombie density and forest.
    pub town_min: IVec2,
    pub town_max: IVec2,
    /// Where the player wakes up, and the building it's in.
    pub player_spawn: Vec2,
    pub spawn_building: usize,
}

pub fn generate(config: &GenConfig, templates: &[BuildingTemplate]) -> Generated {
    let mut rng = Rng::new(config.seed ^ 0x70_77_4e_00);

    // Street grid: block sizes vary a little per seed.
    let widths: Vec<i32> = (0..config.blocks.x).map(|_| 56 + rng.below(14) as i32).collect();
    let depths: Vec<i32> = (0..config.blocks.y).map(|_| 46 + rng.below(8) as i32).collect();
    let total = IVec2::new(widths.iter().sum::<i32>() + CORRIDOR * (config.blocks.x + 1), depths.iter().sum::<i32>() + CORRIDOR * (config.blocks.y + 1));
    // The town sits in the middle of however big the map is (more forest round it on a bigger map),
    // but always fits, with a margin of forest.
    let need = (total + IVec2::splat(2 * MIN_FOREST) + IVec2::splat(CHUNK_SIZE - 1)) / CHUNK_SIZE;
    let mut map = TileMap::new(config.chunks.x.max(need.x), config.chunks.y.max(need.y));
    let size = IVec2::new(map.width(), map.height());
    let town_min = (size - total) / 2;
    let town_max = town_min + total;
    let mut xs = vec![town_min.x];
    for w in &widths {
        xs.push(xs.last().unwrap() + CORRIDOR + w);
    }
    let mut ys = vec![town_min.y];
    for d in &depths {
        ys.push(ys.last().unwrap() + CORRIDOR + d);
    }

    for &y in &ys {
        paint_street(&mut map, IVec2::new(town_min.x, y), IVec2::new(total.x, CORRIDOR), true);
    }
    for &x in &xs {
        paint_street(&mut map, IVec2::new(x, town_min.y), IVec2::new(CORRIDOR, total.y), false);
    }
    // Roads out of town: the middle east–west street runs to both map edges, one street runs north.
    let main_y = ys[ys.len() / 2];
    paint_road_only(&mut map, IVec2::new(0, main_y + SIDEWALK), IVec2::new(town_min.x, ROAD), true);
    paint_road_only(&mut map, IVec2::new(town_max.x, main_y + SIDEWALK), IVec2::new(size.x - town_max.x, ROAD), true);
    let north_x = xs[xs.len() / 2];
    paint_road_only(&mut map, IVec2::new(north_x + SIDEWALK, 0), IVec2::new(ROAD, town_min.y), false);

    // Houses on lots: each block has a row facing its north street and a row facing its south street.
    let mut houses: Vec<usize> = Vec::new();
    let mut bungalows: Vec<usize> = Vec::new();
    for bx in 0..config.blocks.x as usize {
        for by in 0..config.blocks.y as usize {
            let block_min = IVec2::new(xs[bx] + CORRIDOR, ys[by] + CORRIDOR);
            let block_size = IVec2::new(widths[bx], depths[by]);
            let lot_depth = block_size.y / 2;
            for faces_north in [true, false] {
                let lot_y = if faces_north { block_min.y } else { block_min.y + lot_depth };
                let mut x = block_min.x;
                while x < block_min.x + block_size.x {
                    let remaining = block_min.x + block_size.x - x;
                    let mut w = 17 + rng.below(6) as i32;
                    if remaining - w < 17 {
                        w = remaining;
                    }
                    let lot = (IVec2::new(x, lot_y), IVec2::new(w, if faces_north { lot_depth } else { block_size.y - lot_depth }));
                    x += w;
                    if rng.f32() < 0.82 {
                        if let Some((b, is_bungalow)) = place_house(&mut map, &mut rng, templates, lot, faces_north) {
                            houses.push(b);
                            if is_bungalow {
                                bungalows.push(b);
                            }
                        }
                    }
                    // Back fence between back-to-back lots.
                    if faces_north && rng.f32() < 0.6 {
                        for fx in lot.0.x..lot.0.x + lot.1.x {
                            let t = IVec2::new(fx, lot.0.y + lot.1.y);
                            if map.tile(t).room == 0 && map.tile(t - IVec2::Y).room == 0 {
                                map.set_edge(t, Side::N, Edge::Fence);
                            }
                        }
                    }
                    scatter_yard(&mut map, &mut rng, lot);
                }
            }
        }
    }

    plant_forest(&mut map, &mut rng, town_min - IVec2::splat(4), town_max + IVec2::splat(4), config.seed);

    // Wake up in a ground-floor bedroom.
    let candidates = if bungalows.is_empty() { &houses } else { &bungalows };
    let spawn_building = candidates[rng.below(candidates.len().max(1) as u32) as usize % candidates.len().max(1)];
    let player_spawn = find_bedroom_tile(&map, spawn_building, &mut rng).unwrap_or(Vec2::new(size.x as f32 / 2.0, main_y as f32 + 5.5));
    Generated { map, town_min, town_max, player_spawn, spawn_building }
}

fn paint_street(map: &mut TileMap, min: IVec2, size: IVec2, horizontal: bool) {
    for y in min.y..min.y + size.y {
        for x in min.x..min.x + size.x {
            let across = if horizontal { y - min.y } else { x - min.x };
            let along = if horizontal { x } else { y };
            let floor = if across < SIDEWALK || across >= SIDEWALK + ROAD {
                Floor::Sidewalk
            } else if across == SIDEWALK + ROAD / 2 && along % 6 < 3 {
                Floor::RoadLine
            } else {
                Floor::Asphalt
            };
            set_ground(map, IVec2::new(x, y), floor);
        }
    }
}

fn paint_road_only(map: &mut TileMap, min: IVec2, size: IVec2, horizontal: bool) {
    for y in min.y..min.y + size.y {
        for x in min.x..min.x + size.x {
            let across = if horizontal { y - min.y } else { x - min.x };
            let along = if horizontal { x } else { y };
            let floor = if across == ROAD / 2 && along % 6 < 3 { Floor::RoadLine } else { Floor::Asphalt };
            set_ground(map, IVec2::new(x, y), floor);
        }
    }
}

/// Set an outdoor tile's floor without touching sidewalks that already exist (crossings).
fn set_ground(map: &mut TileMap, t: IVec2, floor: Floor) {
    if let Some(tile) = map.tile_mut(t) {
        if tile.floor == Floor::Asphalt || tile.floor == Floor::RoadLine {
            return; // keep crossing asphalt
        }
        tile.floor = floor;
        tile.feature = Feature::None;
    }
}

/// Place a random fitting house on a lot. Returns its building index and whether it's a bungalow.
fn place_house(map: &mut TileMap, rng: &mut Rng, templates: &[BuildingTemplate], lot: (IVec2, IVec2), faces_north: bool) -> Option<(usize, bool)> {
    let turns = if faces_north { 2 } else { 0 };
    let fitting: Vec<&BuildingTemplate> = templates
        .iter()
        .filter(|t| {
            let s = t.rotated_size(turns);
            s.x + 4 <= lot.1.x && s.y + FRONT_SETBACK + 3 <= lot.1.y
        })
        .collect();
    if fitting.is_empty() {
        return None;
    }
    let t = fitting[rng.below(fitting.len() as u32) as usize];
    let s = t.rotated_size(turns);
    let slack = lot.1.x - s.x - 4;
    let x = lot.0.x + 2 + if slack > 0 { rng.below(slack as u32 + 1) as i32 } else { 0 };
    let setback = FRONT_SETBACK + rng.below(2) as i32;
    let y = if faces_north { lot.0.y + setback } else { lot.0.y + lot.1.y - setback - s.y };
    let mirror = rng.f32() < 0.5;
    let placed = t.place(map, Placement { origin: IVec2::new(x, y), quarter_turns: turns, mirror }, rng.below(8) as u8, rng.below(5) as u8);

    // Door states: a few front doors left open, interior doors often open.
    let building = &map.buildings[placed.building];
    let (min, size) = (building.min, building.size);
    for yy in min.y..min.y + size.y {
        for xx in min.x..min.x + size.x {
            let p = IVec2::new(xx, yy);
            for s in [Side::N, Side::W] {
                if map.edge(p, s) == Edge::DoorClosed {
                    let exterior = map.tile(p).room == 0 || map.tile(p + s.offset()).room == 0;
                    let open = if exterior { rng.f32() < 0.25 } else { rng.f32() < 0.6 };
                    if open {
                        map.set_edge(p, s, Edge::DoorOpen);
                    }
                }
            }
        }
    }
    // Garden path from each street-side door to the sidewalk.
    let street = if faces_north { Side::N } else { Side::S };
    for (door, side, _) in placed.exterior_doors.clone() {
        if side != street {
            continue;
        }
        let mut p = door + side.offset();
        while map.in_bounds(p) && map.tile(p).floor == Floor::Grass && map.tile(p).room == 0 {
            if let Some(tile) = map.tile_mut(p) {
                tile.floor = Floor::Path;
                tile.feature = Feature::None;
            }
            p += side.offset();
        }
    }
    Some((placed.building, t.id.contains("bungalow")))
}

fn scatter_yard(map: &mut TileMap, rng: &mut Rng, lot: (IVec2, IVec2)) {
    let count = rng.below(4);
    for _ in 0..count {
        let p = lot.0 + IVec2::new(1 + rng.below((lot.1.x - 2).max(1) as u32) as i32, 1 + rng.below((lot.1.y - 2).max(1) as u32) as i32);
        if clear_around(map, p, 1) {
            if let Some(t) = map.tile_mut(p) {
                t.feature = if rng.f32() < 0.6 { Feature::Tree } else { Feature::Bush };
            }
        }
    }
}

/// True if every tile within `r` of `p` is outdoor grass with nothing on it.
fn clear_around(map: &TileMap, p: IVec2, r: i32) -> bool {
    for dy in -r..=r {
        for dx in -r..=r {
            let t = map.tile(p + IVec2::new(dx, dy));
            if !map.in_bounds(p + IVec2::new(dx, dy)) || t.room != 0 || t.floor != Floor::Grass || t.feature != Feature::None {
                return false;
            }
        }
    }
    true
}

fn plant_forest(map: &mut TileMap, rng: &mut Rng, keep_min: IVec2, keep_max: IVec2, seed: u64) {
    for y in 0..map.height() {
        for x in 0..map.width() {
            let p = IVec2::new(x, y);
            if p.x >= keep_min.x && p.y >= keep_min.y && p.x < keep_max.x && p.y < keep_max.y {
                continue;
            }
            if !clear_around(map, p, 1) {
                continue;
            }
            // Distance outside the town edge, and patchy noise for clearings and thickets.
            let outside = IVec2::new((keep_min.x - p.x).max(p.x - keep_max.x).max(0), (keep_min.y - p.y).max(p.y - keep_max.y).max(0));
            let dist = outside.max_element() as f32;
            let n = value_noise(seed, p.as_vec2() / 18.0) * 0.65 + value_noise(seed ^ 0xabc, p.as_vec2() / 6.0) * 0.35;
            let density = (0.04 + 0.5 * smoothstep(0.35, 0.75, n) + 0.2 * (dist / 40.0).min(1.0)).min(0.55);
            let roll = rng.f32();
            if roll < density {
                let pine = rng.f32() < 0.35 + 0.4 * (dist / 60.0).min(1.0);
                if let Some(t) = map.tile_mut(p) {
                    t.feature = if pine { Feature::PineTree } else { Feature::Tree };
                }
            } else if roll < density + 0.03 {
                if let Some(t) = map.tile_mut(p) {
                    t.feature = Feature::Bush;
                }
            } else if roll > 0.985 {
                if let Some(t) = map.tile_mut(p) {
                    t.floor = Floor::Dirt;
                }
            }
        }
    }
}

fn find_bedroom_tile(map: &TileMap, building: usize, rng: &mut Rng) -> Option<Vec2> {
    let b = &map.buildings[building];
    let mut tiles = Vec::new();
    for y in b.min.y..b.min.y + b.size.y {
        for x in b.min.x..b.min.x + b.size.x {
            let p = IVec2::new(x, y);
            let t = map.tile(p);
            if t.room != 0 && map.rooms[t.room as usize].room_type == "bedroom" && t.feature == Feature::None {
                tiles.push(p);
            }
        }
    }
    (!tiles.is_empty()).then(|| tiles[rng.below(tiles.len() as u32) as usize].as_vec2() + Vec2::splat(0.5))
}

/// Zombie spawn weight for a tile: streets busy, yards and houses quieter, forest sparse.
pub fn spawn_weight(map: &TileMap, t: IVec2, town_min: IVec2, town_max: IVec2) -> f32 {
    let tile = map.tile(t);
    if !map.in_bounds(t) || map.tile_blocked(t) {
        return 0.0;
    }
    let in_town = t.x >= town_min.x && t.y >= town_min.y && t.x < town_max.x && t.y < town_max.y;
    match (in_town, tile.room != 0, tile.floor) {
        (_, true, _) => 0.3,
        (true, false, f) if f.is_road() || f == Floor::Sidewalk => 1.0,
        (true, false, _) => 0.6,
        (false, false, f) if f.is_road() => 0.25,
        _ => 0.04,
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn hash2(seed: u64, x: i32, y: i32) -> f32 {
    let mut h = seed ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Smooth value noise in [0, 1].
fn value_noise(seed: u64, p: Vec2) -> f32 {
    let i = p.floor();
    let f = p - i;
    let (x, y) = (i.x as i32, i.y as i32);
    let u = f * f * (Vec2::splat(3.0) - 2.0 * f);
    let a = hash2(seed, x, y);
    let b = hash2(seed, x + 1, y);
    let c = hash2(seed, x, y + 1);
    let d = hash2(seed, x + 1, y + 1);
    (a + (b - a) * u.x) + ((c + (d - c) * u.x) - (a + (b - a) * u.x)) * u.y
}

/// Chunk coordinates that contain tile `t`.
pub fn chunk_of(t: IVec2) -> IVec2 {
    IVec2::new(t.x.div_euclid(CHUNK_SIZE), t.y.div_euclid(CHUNK_SIZE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::BuildingTemplate;

    pub fn templates() -> Vec<BuildingTemplate> {
        [include_str!("../../../assets/data/buildings/bungalow.ron"), include_str!("../../../assets/data/buildings/family_two_storey.ron")]
            .iter()
            .map(|t| BuildingTemplate::from_ron(t).unwrap())
            .collect()
    }

    #[test]
    fn town_is_deterministic_and_populated() {
        let a = generate(&GenConfig::default(), &templates());
        let b = generate(&GenConfig::default(), &templates());
        assert_eq!(a.map.buildings.len(), b.map.buildings.len());
        assert!(a.map.buildings.len() >= 12, "only {} houses", a.map.buildings.len());
        assert!(a.map.buildings.iter().any(|b| b.levels == 2), "expected a two-storey house");
        assert_eq!(a.player_spawn, b.player_spawn);
        let c = generate(&GenConfig { seed: 2, ..Default::default() }, &templates());
        assert_ne!(a.player_spawn, c.player_spawn);
    }

    #[test]
    fn player_wakes_up_in_a_bedroom() {
        for seed in 1..6 {
            let g = generate(&GenConfig { seed, ..Default::default() }, &templates());
            let room = g.map.room_at(g.player_spawn);
            assert_ne!(room, 0, "seed {seed}: spawn outdoors");
            assert_eq!(g.map.rooms[room as usize].room_type, "bedroom");
            assert!(!g.map.tile_blocked(g.player_spawn.floor().as_ivec2()));
        }
    }

    /// A bigger map keeps the same town in the middle with more forest round it, and a tiny one is
    /// grown to fit the town.
    #[test]
    fn map_size_is_configurable() {
        let small = generate(&GenConfig::default(), &templates());
        let big = generate(&GenConfig { chunks: IVec2::splat(64), ..Default::default() }, &templates());
        assert_eq!((big.map.width(), big.map.height()), (1024, 1024));
        assert_eq!(big.map.buildings.len(), small.map.buildings.len());
        assert_eq!(big.town_max - big.town_min, small.town_max - small.town_min);
        let centre = (big.town_min + big.town_max) / 2;
        assert!((centre - IVec2::splat(512)).abs().max_element() <= 1, "town off centre: {centre}");
        assert!(!big.map.tile_blocked(big.player_spawn.floor().as_ivec2()));
        let tiny = generate(&GenConfig { chunks: IVec2::splat(4), ..Default::default() }, &templates());
        assert!(tiny.town_min.x >= MIN_FOREST && tiny.town_max.x <= tiny.map.width() - MIN_FOREST + 1, "town doesn't fit");
        assert_eq!(tiny.map.buildings.len(), small.map.buildings.len());
    }

    #[test]
    fn forest_surrounds_town() {
        let g = generate(&GenConfig::default(), &templates());
        let mut trees_outside = 0;
        for y in 0..g.map.height() {
            for x in 0..g.map.width() {
                let p = IVec2::new(x, y);
                let outside = p.x < g.town_min.x || p.y < g.town_min.y || p.x >= g.town_max.x || p.y >= g.town_max.y;
                if outside && matches!(g.map.tile(p).feature, Feature::Tree | Feature::PineTree) {
                    trees_outside += 1;
                }
            }
        }
        assert!(trees_outside > 3000, "only {trees_outside} trees");
    }
}
