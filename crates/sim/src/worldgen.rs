//! Seeded town generator (PLAN §4.1a). A grid of street blocks round a wide main street that runs
//! east–west through the middle of town and on out through the forest: shops and offices on narrow,
//! shallow lots along the main street in the middle of town (the town core), houses on the
//! residential streets round it (bigger lots on the edge of town), a few parking lots, vacant lots
//! and a pocket park, back fences and yard trees, and forest all round. Templates are chosen by
//! [`BuildingKind`] and size; with no shop or office templates the main street is lined with houses
//! too, and a gap nothing fits becomes a parking or vacant lot. The same seed and templates always
//! give the same map.

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::map::{Edge, Feature, Floor, Side, TileMap, CHUNK_SIZE};
use crate::population::Zone;
use crate::rng::Rng;
use crate::template::{BuildingKind, BuildingTemplate, Placement};

/// Asphalt width of a street, and the sidewalk on each side.
const ROAD: i32 = 7;
const SIDEWALK: i32 = 2;
const CORRIDOR: i32 = ROAD + 2 * SIDEWALK;
/// The main street: wider asphalt, wider sidewalks, a solid centre line.
const MAIN_ROAD: i32 = 9;
const MAIN_SIDEWALK: i32 = 3;
const MAIN_CORRIDOR: i32 = MAIN_ROAD + 2 * MAIN_SIDEWALK;
/// Houses stand this far (plus up to 1 m) back from the sidewalk.
const FRONT_SETBACK: i32 = 5;
/// Shops and offices stand 1–2 m back from the main street's sidewalk, paved up to their fronts,
/// with this much yard behind them (bins, the back door, a fence), and 1–3 m alleys between.
const SHOP_BACK: i32 = 4;
/// A house row behind a row of shops is never shallower than this.
const MIN_HOUSE_ROW: i32 = 18;
/// Client siding colours (`meshgen::siding_color`) for shops and offices: grey, tan, brick.
const SHOP_SIDING: [u8; 3] = [5, 6, 7];
/// Forest kept at least this wide round the town when the map is made bigger to fit it.
const MIN_FOREST: i32 = 24;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenConfig {
    pub seed: u64,
    /// Map size in chunks (each 16 m). Grown if the town wouldn't fit.
    pub chunks: IVec2,
    /// Street blocks across and down. The main street is the middle east–west street.
    pub blocks: IVec2,
    /// How many block columns in the middle of the main street are shops and offices (both sides
    /// of the street; the rest of it is houses). 0: none.
    #[serde(default = "default_commercial_blocks")]
    pub commercial_blocks: i32,
}

fn default_commercial_blocks() -> i32 {
    3
}

impl Default for GenConfig {
    fn default() -> Self {
        // 30 chunks (480 m) holds the default town (about 380 × 260 m) with forest round it.
        Self { seed: 1, chunks: IVec2::new(30, 30), blocks: IVec2::new(5, 4), commercial_blocks: default_commercial_blocks() }
    }
}

/// Where the town is (tiles): its bounds and its core, the main street's shops and offices (from
/// the far side of the shops north of it to the far side of those south of it).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Town {
    pub min: IVec2,
    pub max: IVec2,
    pub core_min: IVec2,
    pub core_max: IVec2,
}

pub struct Generated {
    pub map: TileMap,
    /// Town bounds (tiles), used for zombie density and forest.
    pub town_min: IVec2,
    pub town_max: IVec2,
    /// The town core (tiles): the main street's commercial blocks (see [`Town`]).
    pub core_min: IVec2,
    pub core_max: IVec2,
    /// Where the player wakes up, and the building it's in.
    pub player_spawn: Vec2,
    pub spawn_building: usize,
}

impl Generated {
    pub fn town(&self) -> Town {
        Town { min: self.town_min, max: self.town_max, core_min: self.core_min, core_max: self.core_max }
    }
}

/// A rectangle of tiles: `min..min + size`.
#[derive(Clone, Copy, Debug)]
struct Rect {
    min: IVec2,
    size: IVec2,
}

impl Rect {
    fn new(min: IVec2, size: IVec2) -> Self {
        Self { min, size }
    }
    fn max(&self) -> IVec2 {
        self.min + self.size
    }
    fn tiles(&self) -> impl Iterator<Item = IVec2> {
        let (min, max) = (self.min, self.max());
        (min.y..max.y).flat_map(move |y| (min.x..max.x).map(move |x| IVec2::new(x, y)))
    }
}

fn is_commercial(t: &BuildingTemplate) -> bool {
    matches!(t.kind, BuildingKind::Shop | BuildingKind::Office)
}

pub fn generate(config: &GenConfig, templates: &[BuildingTemplate]) -> Generated {
    let mut rng = Rng::new(config.seed ^ 0x70_77_4e_00);
    let blocks = config.blocks.max(IVec2::ONE);
    let (cols, rows) = (blocks.x as usize, blocks.y as usize);
    // The main street is the middle east–west street (`ys[main]`).
    let main = rows / 2;
    let houses: Vec<&BuildingTemplate> = templates.iter().filter(|t| matches!(t.kind, BuildingKind::House)).collect();
    let shops: Vec<&BuildingTemplate> = templates.iter().filter(|t| is_commercial(t)).collect();
    let n_commercial = config.commercial_blocks.clamp(0, blocks.x);
    let first_commercial = (blocks.x - n_commercial) / 2;
    let commercial_col = |bx: usize| (first_commercial..first_commercial + n_commercial).contains(&(bx as i32));

    // Street grid: block sizes vary a little per seed.
    let widths: Vec<i32> = (0..cols).map(|_| 56 + rng.below(14) as i32).collect();
    let depths: Vec<i32> = (0..rows).map(|_| 46 + rng.below(8) as i32).collect();
    let street = |i: usize| if i == main { MAIN_CORRIDOR } else { CORRIDOR };
    let total = IVec2::new(widths.iter().sum::<i32>() + CORRIDOR * (cols as i32 + 1), depths.iter().sum::<i32>() + (0..=rows).map(street).sum::<i32>());
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
    for (i, d) in depths.iter().enumerate() {
        ys.push(ys.last().unwrap() + street(i) + d);
    }

    for (i, &y) in ys.iter().enumerate() {
        if i == main {
            paint_street(&mut map, Rect::new(IVec2::new(town_min.x, y), IVec2::new(total.x, MAIN_CORRIDOR)), true, MAIN_ROAD, MAIN_SIDEWALK, true);
        } else {
            paint_street(&mut map, Rect::new(IVec2::new(town_min.x, y), IVec2::new(total.x, CORRIDOR)), true, ROAD, SIDEWALK, false);
        }
    }
    for &x in &xs {
        paint_street(&mut map, Rect::new(IVec2::new(x, town_min.y), IVec2::new(CORRIDOR, total.y)), false, ROAD, SIDEWALK, false);
    }
    // Roads out of town: the main street runs to both map edges, one street runs north and one south.
    let main_y = ys[main];
    paint_road_only(&mut map, Rect::new(IVec2::new(0, main_y + MAIN_SIDEWALK), IVec2::new(town_min.x, MAIN_ROAD)), true, MAIN_ROAD);
    paint_road_only(&mut map, Rect::new(IVec2::new(town_max.x, main_y + MAIN_SIDEWALK), IVec2::new(size.x - town_max.x, MAIN_ROAD)), true, MAIN_ROAD);
    let north_x = xs[xs.len() / 2];
    paint_road_only(&mut map, Rect::new(IVec2::new(north_x + SIDEWALK, 0), IVec2::new(ROAD, town_min.y)), false, ROAD);
    let south_x = xs[(xs.len() - 1) / 2];
    paint_road_only(&mut map, Rect::new(IVec2::new(south_x + SIDEWALK, town_max.y), IVec2::new(ROAD, size.y - town_max.y)), false, ROAD);

    // How deep the rows of shops along the main street are: deep enough for the deepest shop or
    // office, leaving a house row behind.
    let min_depth = depths.iter().copied().min().unwrap_or(46);
    let deepest = shops.iter().map(|t| t.size().y).max().unwrap_or(0);
    let shop_depth = (deepest + 2 + SHOP_BACK).clamp(12, (min_depth - MIN_HOUSE_ROW).max(12));
    let downtown = !shops.is_empty();
    let core_depth = if downtown { shop_depth } else { min_depth / 2 };
    let (core_min, core_max) = if n_commercial > 0 {
        let lo = IVec2::new(xs[first_commercial as usize], (main_y - core_depth).max(town_min.y));
        let hi = IVec2::new(xs[(first_commercial + n_commercial) as usize] + CORRIDOR, (main_y + MAIN_CORRIDOR + core_depth).min(town_max.y));
        (lo, hi)
    } else {
        let c = (town_min + town_max) / 2;
        (c, c)
    };

    // One pocket park, on a residential row.
    let mut residential_rows = Vec::new();
    for bx in 0..cols {
        for by in 0..rows {
            for faces_north in [true, false] {
                let shop_row = downtown && commercial_col(bx) && if faces_north { by == main } else { by + 1 == main };
                if !shop_row {
                    residential_rows.push((bx, by, faces_north));
                }
            }
        }
    }
    let park_row = if residential_rows.is_empty() { None } else { Some(residential_rows[rng.below(residential_rows.len() as u32) as usize]) };

    let mut town = TownBuilder { map, rng, houses, shops, homes: Vec::new() };
    for bx in 0..cols {
        for by in 0..rows {
            let block = Rect::new(IVec2::new(xs[bx] + CORRIDOR, ys[by] + street(by)), IVec2::new(widths[bx], depths[by]));
            // The north row faces the main street (the block is just south of it), or the south row does.
            let north_shops = downtown && commercial_col(bx) && by == main;
            let south_shops = downtown && commercial_col(bx) && by + 1 == main;
            let north_depth = if north_shops {
                shop_depth
            } else if south_shops {
                block.size.y - shop_depth
            } else {
                block.size.y / 2
            };
            let edge_block = bx == 0 || by == 0 || bx + 1 == cols || by + 1 == rows;
            for faces_north in [true, false] {
                let row = if faces_north {
                    Rect::new(block.min, IVec2::new(block.size.x, north_depth))
                } else {
                    Rect::new(block.min + IVec2::new(0, north_depth), IVec2::new(block.size.x, block.size.y - north_depth))
                };
                if (faces_north && north_shops) || (!faces_north && south_shops) {
                    town.shop_row(row, faces_north);
                } else {
                    let park = park_row == Some((bx, by, faces_north));
                    town.house_row(row, faces_north, edge_block, park, !(north_shops || south_shops));
                }
            }
            // A row of shops is always fenced off from the yards behind it.
            if north_shops || south_shops {
                town.fence(block.min + IVec2::new(0, north_depth), block.size.x);
            }
        }
    }
    let TownBuilder { mut map, mut rng, homes, .. } = town;

    plant_forest(&mut map, &mut rng, town_min - IVec2::splat(4), town_max + IVec2::splat(4), config.seed);

    // Wake up in a ground-floor bedroom of a house.
    let candidates: Vec<usize> = homes.iter().copied().filter(|&b| !bedroom_tiles(&map, b).is_empty()).collect();
    let fallback = || Vec2::new(size.x as f32 / 2.0, main_y as f32 + MAIN_SIDEWALK as f32 + 1.5);
    let (spawn_building, player_spawn) = if candidates.is_empty() {
        (homes.first().copied().unwrap_or(0), fallback())
    } else {
        let b = candidates[rng.below(candidates.len() as u32) as usize];
        let tiles = bedroom_tiles(&map, b);
        (b, tiles[rng.below(tiles.len() as u32) as usize].as_vec2() + Vec2::splat(0.5))
    };
    Generated { map, town_min, town_max, core_min, core_max, player_spawn, spawn_building }
}

/// The map and RNG while lots are filled, with the templates by kind and the houses placed so far.
struct TownBuilder<'a> {
    map: TileMap,
    rng: Rng,
    houses: Vec<&'a BuildingTemplate>,
    shops: Vec<&'a BuildingTemplate>,
    /// Building indices of the houses placed.
    homes: Vec<usize>,
}

impl TownBuilder<'_> {
    /// A row of house lots facing its street: mostly houses (a template that fits, at random),
    /// some left as yards or overgrown vacant lots, and maybe the town's pocket park. Lots on the
    /// edge of town are wider. Back fences between back-to-back lots when `fences`.
    fn house_row(&mut self, row: Rect, faces_north: bool, edge_block: bool, park: bool, fences: bool) {
        let (narrowest, spread) = if edge_block { (19, 7) } else { (17, 6) };
        let mut lots = Vec::new();
        let mut x = row.min.x;
        while x < row.max().x {
            let remaining = row.max().x - x;
            let mut w = narrowest + self.rng.below(spread) as i32;
            if remaining - w < narrowest {
                w = remaining;
            }
            lots.push(Rect::new(IVec2::new(x, row.min.y), IVec2::new(w, row.size.y)));
            x += w;
        }
        let park_lot = if park { Some(1.min(lots.len() - 1)) } else { None };
        for (i, lot) in lots.into_iter().enumerate() {
            if park_lot == Some(i) {
                self.park(lot, faces_north);
                continue;
            }
            let roll = self.rng.f32();
            if roll < 0.82 {
                if let Some(b) = self.place_house(lot, faces_north) {
                    self.homes.push(b);
                }
            } else if roll < 0.91 {
                self.vacant_lot(lot);
            }
            // Back fence between back-to-back lots.
            if fences && faces_north && self.rng.f32() < 0.6 {
                self.fence(IVec2::new(lot.min.x, lot.max().y), lot.size.x);
            }
            scatter_yard(&mut self.map, &mut self.rng, (lot.min, lot.size));
        }
    }

    /// A row of shops and offices facing the main street, packed along it with alleys between,
    /// paved from the sidewalk to their fronts; now and then a parking lot, and a parking or vacant
    /// lot where nothing else fits.
    fn shop_row(&mut self, row: Rect, faces_north: bool) {
        let turns = if faces_north { 2 } else { 0 };
        let setback = 1 + self.rng.below(2) as i32;
        let front = if faces_north { row.min.y } else { row.max().y - setback };
        for t in Rect::new(IVec2::new(row.min.x, front), IVec2::new(row.size.x, setback)).tiles() {
            set_floor(&mut self.map, t, Floor::Sidewalk);
        }
        let end = row.max().x - 1;
        let mut x = row.min.x + 1;
        let mut last: Option<&str> = None;
        let mut parked = false;
        loop {
            let room = end - x;
            if room < 6 {
                break;
            }
            let fits: Vec<&BuildingTemplate> = self
                .shops
                .iter()
                .copied()
                .filter(|t| {
                    let s = t.rotated_size(turns);
                    s.x <= room && s.y + setback + 2 <= row.size.y
                })
                .collect();
            let park_here = !parked && room >= 12 && self.rng.f32() < 0.15;
            if park_here || fits.is_empty() {
                if room < 10 {
                    self.vacant_lot(Rect::new(IVec2::new(x, row.min.y), IVec2::new(room, row.size.y)));
                    break;
                }
                let w = if fits.is_empty() { room } else { (12 + self.rng.below(7) as i32).min(room) };
                self.parking_lot(Rect::new(IVec2::new(x, row.min.y), IVec2::new(w, row.size.y - 1)), faces_north);
                parked = true;
                x += w + 1;
                continue;
            }
            // Not the same as the last one when there's a choice.
            let choice: Vec<&BuildingTemplate> = if fits.len() > 1 { fits.iter().copied().filter(|t| Some(t.id.as_str()) != last).collect() } else { fits };
            let t = choice[self.rng.below(choice.len() as u32) as usize];
            let s = t.rotated_size(turns);
            let y = if faces_north { row.min.y + setback } else { row.max().y - setback - s.y };
            let mirror = self.rng.f32() < 0.5;
            let siding = SHOP_SIDING[self.rng.below(SHOP_SIDING.len() as u32) as usize];
            let roof = self.rng.below(5) as u8;
            self.place(t, Placement { origin: IVec2::new(x, y), quarter_turns: turns, mirror }, siding, roof, 0.15);
            last = Some(t.id.as_str());
            x += s.x + 1 + self.rng.below(3) as i32;
        }
        // A few bushes and trees out the back.
        let back = if faces_north { Rect::new(IVec2::new(row.min.x, row.max().y - SHOP_BACK), IVec2::new(row.size.x, SHOP_BACK)) } else { Rect::new(row.min, IVec2::new(row.size.x, SHOP_BACK)) };
        scatter_yard(&mut self.map, &mut self.rng, (back.min, back.size));
    }

    /// A random fitting house on a lot, set back from the street, with a garden path to its front
    /// door. Returns its building index.
    fn place_house(&mut self, lot: Rect, faces_north: bool) -> Option<usize> {
        let turns = if faces_north { 2 } else { 0 };
        let fitting: Vec<&BuildingTemplate> = self
            .houses
            .iter()
            .copied()
            .filter(|t| {
                let s = t.rotated_size(turns);
                s.x + 4 <= lot.size.x && s.y + FRONT_SETBACK + 3 <= lot.size.y
            })
            .collect();
        if fitting.is_empty() {
            return None;
        }
        let rng = &mut self.rng;
        let t = fitting[rng.below(fitting.len() as u32) as usize];
        let s = t.rotated_size(turns);
        let slack = lot.size.x - s.x - 4;
        let x = lot.min.x + 2 + if slack > 0 { rng.below(slack as u32 + 1) as i32 } else { 0 };
        let setback = FRONT_SETBACK + rng.below(2) as i32;
        let y = if faces_north { lot.min.y + setback } else { lot.max().y - setback - s.y };
        let mirror = rng.f32() < 0.5;
        let (siding, roof) = (rng.below(8) as u8, rng.below(5) as u8);
        let placed = self.place(t, Placement { origin: IVec2::new(x, y), quarter_turns: turns, mirror }, siding, roof, 0.25);
        // Garden path from each street-side door to the sidewalk.
        let street = if faces_north { Side::N } else { Side::S };
        let map = &mut self.map;
        for (door, side, _) in placed.exterior_doors {
            if side != street {
                continue;
            }
            let mut p = door + side.offset();
            while map.in_bounds(p) && map.tile(p).floor == Floor::Grass && map.tile(p).room == 0 {
                set_floor(map, p, Floor::Path);
                p += side.offset();
            }
        }
        Some(placed.building)
    }

    /// Stamp a template and set its doors: outside doors open with chance `outside_open`, inside
    /// ones often open.
    fn place(&mut self, t: &BuildingTemplate, at: Placement, siding: u8, roof: u8, outside_open: f32) -> crate::template::Placed {
        let placed = t.place(&mut self.map, at, siding, roof);
        let building = &self.map.buildings[placed.building];
        for p in Rect::new(building.min, building.size).tiles() {
            for s in [Side::N, Side::W] {
                if self.map.edge(p, s) == Edge::DoorClosed {
                    let exterior = self.map.tile(p).room == 0 || self.map.tile(p + s.offset()).room == 0;
                    let open = if exterior { self.rng.f32() < outside_open } else { self.rng.f32() < 0.6 };
                    if open {
                        self.map.set_edge(p, s, Edge::DoorOpen);
                    }
                }
            }
        }
        placed
    }

    /// A fence along the north edges of `width` tiles from `start` eastward, where it runs between
    /// two outdoor tiles.
    fn fence(&mut self, start: IVec2, width: i32) {
        for x in start.x..start.x + width {
            let t = IVec2::new(x, start.y);
            if self.map.tile(t).room == 0 && self.map.tile(t - IVec2::Y).room == 0 {
                self.map.set_edge(t, Side::N, Edge::Fence);
            }
        }
    }

    /// Asphalt with a row of parking bays at the back (and one at the front too when deep enough).
    fn parking_lot(&mut self, lot: Rect, faces_north: bool) {
        let depth = lot.size.y;
        for t in lot.tiles() {
            // Rows counted from the street side.
            let from_street = if faces_north { t.y - lot.min.y } else { lot.max().y - 1 - t.y };
            let bay = from_street >= depth - 5 || (depth >= 14 && (1..5).contains(&from_street));
            let line = bay && (t.x - lot.min.x) % 3 == 0 && t.x > lot.min.x;
            set_floor(&mut self.map, t, if line { Floor::RoadLine } else { Floor::Asphalt });
        }
    }

    /// Overgrown ground: patches of bare dirt, bushes and a sapling or two.
    fn vacant_lot(&mut self, lot: Rect) {
        let seed = self.rng.next_u64();
        for t in lot.tiles() {
            let tile = self.map.tile(t);
            if tile.room == 0 && tile.floor == Floor::Grass && value_noise(seed, (t - lot.min).as_vec2() / 4.0) > 0.62 {
                set_floor(&mut self.map, t, Floor::Dirt);
            }
        }
        for _ in 0..6 + self.rng.below(6) {
            let p = lot.min + IVec2::new(1 + self.rng.below((lot.size.x - 2).max(1) as u32) as i32, 1 + self.rng.below((lot.size.y - 2).max(1) as u32) as i32);
            if clear_around(&self.map, p, 0) {
                if let Some(t) = self.map.tile_mut(p) {
                    t.feature = if self.rng.f32() < 0.8 { Feature::Bush } else { Feature::Tree };
                }
            }
        }
    }

    /// A pocket park: paths across it from the street, trees round the edge, a few bushes.
    fn park(&mut self, lot: Rect, faces_north: bool) {
        let mid = lot.min + lot.size / 2;
        for t in lot.tiles() {
            let into = if faces_north { t.y < mid.y + 1 } else { t.y >= mid.y };
            if (t.x == mid.x && into) || (t.y == mid.y && t.x > lot.min.x + 1 && t.x < lot.max().x - 2) {
                set_floor(&mut self.map, t, Floor::Path);
            }
        }
        for t in lot.tiles() {
            let inset = (t - lot.min).min(lot.max() - IVec2::ONE - t);
            let ring = inset.min_element() == 2 && (inset.x + inset.y) % 3 == 0;
            let roll = self.rng.f32();
            if ((ring && roll < 0.8) || (!ring && inset.min_element() > 2 && roll < 0.03)) && clear_around(&self.map, t, 1) {
                if let Some(tile) = self.map.tile_mut(t) {
                    tile.feature = if ring { Feature::Tree } else { Feature::Bush };
                }
            }
        }
    }
}

fn paint_street(map: &mut TileMap, r: Rect, horizontal: bool, road: i32, sidewalk: i32, solid_line: bool) {
    for t in r.tiles() {
        let across = if horizontal { t.y - r.min.y } else { t.x - r.min.x };
        let along = if horizontal { t.x } else { t.y };
        let floor = if across < sidewalk || across >= sidewalk + road {
            Floor::Sidewalk
        } else if across == sidewalk + road / 2 && (solid_line || along % 6 < 3) {
            Floor::RoadLine
        } else {
            Floor::Asphalt
        };
        set_ground(map, t, floor);
    }
}

fn paint_road_only(map: &mut TileMap, r: Rect, horizontal: bool, road: i32) {
    for t in r.tiles() {
        let across = if horizontal { t.y - r.min.y } else { t.x - r.min.x };
        let along = if horizontal { t.x } else { t.y };
        let floor = if across == road / 2 && along % 6 < 3 { Floor::RoadLine } else { Floor::Asphalt };
        set_ground(map, t, floor);
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

/// Set an outdoor tile's floor, clearing anything on it.
fn set_floor(map: &mut TileMap, t: IVec2, floor: Floor) {
    if let Some(tile) = map.tile_mut(t) {
        if tile.room == 0 {
            tile.floor = floor;
            tile.feature = Feature::None;
        }
    }
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

/// The free ground-floor bedroom tiles of a building.
fn bedroom_tiles(map: &TileMap, building: usize) -> Vec<IVec2> {
    let b = &map.buildings[building];
    Rect::new(b.min, b.size)
        .tiles()
        .filter(|&p| {
            let t = map.tile(p);
            t.room != 0 && map.rooms[t.room as usize].room_type == "bedroom" && t.feature == Feature::None
        })
        .collect()
}

/// The population zone of tile `t` (see [`crate::population::Zone`]) in `town`: indoors anywhere;
/// outdoors in town, the town core (the main street's shops and offices; `core_fraction` of its
/// length along the street, round its middle), else roads, parking lots and sidewalks are street
/// and the rest yards; outside town, roads are forest roads and the rest forest, edge within
/// `edge_width` metres of the town, deep beyond.
pub fn zone_at(map: &TileMap, town: &Town, t: IVec2, core_fraction: f32, edge_width: f32) -> Zone {
    let tile = map.tile(t);
    if tile.room != 0 {
        return Zone::Indoors;
    }
    let (town_min, town_max) = (town.min, town.max);
    let in_town = t.x >= town_min.x && t.y >= town_min.y && t.x < town_max.x && t.y < town_max.y;
    let road = tile.floor.is_road();
    if in_town {
        let centre = (town.core_min.x + town.core_max.x) as f32 * 0.5;
        let half = (town.core_max.x - town.core_min.x) as f32 * core_fraction * 0.5;
        let x = t.x as f32 + 0.5;
        if (x - centre).abs() <= half && t.y >= town.core_min.y && t.y < town.core_max.y {
            Zone::TownCore
        } else if road || tile.floor == Floor::Sidewalk {
            Zone::Street
        } else {
            Zone::Yard
        }
    } else if road {
        Zone::ForestRoad
    } else {
        let outside = (town_min - t).max(t - (town_max - IVec2::ONE)).max(IVec2::ZERO);
        if outside.max_element() as f32 <= edge_width { Zone::ForestEdge } else { Zone::DeepForest }
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
pub(crate) mod tests {
    use super::*;
    use crate::template::BuildingTemplate;

    /// The shipped templates (houses only, for now).
    pub fn templates() -> Vec<BuildingTemplate> {
        [include_str!("../../../assets/data/buildings/bungalow.ron"), include_str!("../../../assets/data/buildings/family_two_storey.ron")]
            .iter()
            .map(|t| BuildingTemplate::from_ron(t).unwrap())
            .collect()
    }

    /// A small shop, a wide one and an office, made up for these tests (the real ones are data).
    const TEST_SHOPS: [&str; 3] = [
        r#"BuildingTemplate(id: "test_store", name: "Corner store", kind: Shop, roof: Flat,
            rooms: { 'S': (name: "Shop", room_type: "shop", floor: TileFloor), 'R': (name: "Storeroom", room_type: "shop", floor: Concrete) },
            levels: [(
                layout: ["RRRRRRRRR", "RRRRRRRRR", "SSSSSSSSS", "SSSSSSSSS", "SSSSSSSSS", "SSSSSSSSS", "SSSSSSSSS"],
                doors: [(4, 6, S), (1, 0, N), (2, 1, S)],
                windows: [(1, 6, S), (2, 6, S), (6, 6, S), (7, 6, S)],
                furniture: [(kind: "shelf", at: (1, 3), size: (4, 1))],
            )])"#,
        r#"BuildingTemplate(id: "test_hardware", name: "Hardware store", kind: Shop, roof: Flat,
            rooms: { 'S': (name: "Shop", room_type: "hardware", floor: Concrete) },
            levels: [(
                layout: ["SSSSSSSSSSSSSSSS", "SSSSSSSSSSSSSSSS", "SSSSSSSSSSSSSSSS", "SSSSSSSSSSSSSSSS", "SSSSSSSSSSSSSSSS", "SSSSSSSSSSSSSSSS", "SSSSSSSSSSSSSSSS", "SSSSSSSSSSSSSSSS", "SSSSSSSSSSSSSSSS", "SSSSSSSSSSSSSSSS", "SSSSSSSSSSSSSSSS", "SSSSSSSSSSSSSSSS"],
                doors: [(7, 11, S), (14, 0, N)],
                windows: [(2, 11, S), (12, 11, S)],
            )])"#,
        r#"BuildingTemplate(id: "test_office", name: "Office", kind: Office, roof: Flat,
            rooms: { 'O': (name: "Office", room_type: "office", floor: Carpet), 'H': (name: "Lobby", room_type: "hall", floor: TileFloor) },
            levels: [(
                layout: ["OOOOOOOOOO", "OOOOOOOOOO", "OOOOOOOOOO", "OOOOOOOOOO", "HHHHHHHHHH", "HHHHHHHHHH"],
                doors: [(5, 5, S), (5, 4, N)],
                windows: [(1, 5, S), (8, 5, S), (2, 0, N)],
            )])"#,
    ];

    /// The shipped houses plus the test shops and office.
    pub fn with_shops() -> Vec<BuildingTemplate> {
        let mut t = templates();
        t.extend(TEST_SHOPS.iter().map(|s| BuildingTemplate::from_ron(s).unwrap()));
        t
    }

    fn kind_of<'a>(templates: &'a [BuildingTemplate], b: &crate::map::Building) -> &'a BuildingKind {
        &templates.iter().find(|t| t.id == b.template).unwrap().kind
    }

    /// Everything that makes the map, hashed: floors, features, edges, rooms and buildings.
    fn map_hash(m: &TileMap) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |v: u64| h = (h ^ v).wrapping_mul(0x100_0000_01b3);
        for y in 0..m.height() {
            for x in 0..m.width() {
                let t = m.tile(IVec2::new(x, y));
                eat(t.floor as u64 | (t.feature as u64) << 8 | (t.north as u64) << 16 | (t.west as u64) << 24 | (t.room as u64) << 32);
            }
        }
        for b in &m.buildings {
            eat(b.min.x as u64 ^ (b.min.y as u64) << 20 ^ (b.size.x as u64) << 40 ^ (b.siding as u64) << 56);
        }
        h
    }

    #[test]
    fn town_is_deterministic_and_populated() {
        for templates in [templates(), with_shops()] {
            let a = generate(&GenConfig::default(), &templates);
            let b = generate(&GenConfig::default(), &templates);
            assert_eq!(map_hash(&a.map), map_hash(&b.map));
            assert_eq!(a.player_spawn, b.player_spawn);
            assert!(a.map.buildings.len() >= 60, "only {} buildings", a.map.buildings.len());
            assert!(a.map.buildings.iter().any(|b| b.levels == 2), "expected a two-storey house");
            let c = generate(&GenConfig { seed: 2, ..Default::default() }, &templates);
            assert_ne!(a.player_spawn, c.player_spawn);
            assert_ne!(map_hash(&a.map), map_hash(&c.map));
        }
    }

    #[test]
    fn player_wakes_up_in_a_bedroom() {
        for templates in [templates(), with_shops()] {
            for seed in 1..6 {
                let g = generate(&GenConfig { seed, ..Default::default() }, &templates);
                let room = g.map.room_at(g.player_spawn);
                assert_ne!(room, 0, "seed {seed}: spawn outdoors");
                assert_eq!(g.map.rooms[room as usize].room_type, "bedroom");
                assert!(!g.map.tile_blocked(g.player_spawn.floor().as_ivec2()));
                assert!(matches!(kind_of(&templates, &g.map.buildings[g.spawn_building]), BuildingKind::House));
            }
        }
    }

    /// Shops and offices line the main street in the middle of town, fronts to it, inside the town
    /// core; houses fill the rest; nothing overlaps, and every building has a gap round it.
    #[test]
    fn shops_and_offices_line_the_main_street() {
        let templates = with_shops();
        for seed in 1..6 {
            let g = generate(&GenConfig { seed, ..Default::default() }, &templates);
            let town = g.town();
            let (mut shops, mut offices, mut houses) = (0, 0, 0);
            for (i, b) in g.map.buildings.iter().enumerate() {
                let (lo, hi) = (b.min, b.min + b.size);
                let inside = lo.cmpge(town.core_min).all() && hi.cmple(town.core_max).all();
                match kind_of(&templates, b) {
                    BuildingKind::House => houses += 1,
                    kind => {
                        assert!(inside, "seed {seed}: {kind:?} {} at {lo}..{hi} outside the core {}..{}", b.template, town.core_min, town.core_max);
                        if matches!(kind, BuildingKind::Shop) {
                            shops += 1
                        } else {
                            offices += 1
                        }
                        // Its front door opens onto pavement that leads to the main street.
                        let main_mid = (town.core_min.y + town.core_max.y) / 2;
                        let toward = if b.min.y < main_mid { Side::S } else { Side::N };
                        let door = Rect::new(b.min, b.size).tiles().find(|&t| g.map.tile(t).room != 0 && g.map.edge(t, toward).is_door() && g.map.tile(t + toward.offset()).room == 0);
                        let door = door.unwrap_or_else(|| panic!("seed {seed}: {} at {lo} has no door toward the main street", b.template));
                        assert_eq!(g.map.tile(door + toward.offset()).floor, Floor::Sidewalk, "seed {seed}: {} front not paved", b.template);
                    }
                }
                for (j, o) in g.map.buildings.iter().enumerate().skip(i + 1) {
                    let apart = hi.x < o.min.x || o.min.x + o.size.x < lo.x || hi.y < o.min.y || o.min.y + o.size.y < lo.y;
                    assert!(apart, "seed {seed}: buildings {i} and {j} touch or overlap");
                }
            }
            eprintln!("seed {seed}: {shops} shops, {offices} offices, {houses} houses, map {}", g.map.width());
            assert!(shops >= 6 && offices >= 2 && houses >= 50, "seed {seed}: {shops} shops, {offices} offices, {houses} houses");
        }
    }

    /// With no shop or office templates the main street is lined with houses instead, and with no
    /// house templates there's still a town (and a spawn point, on the main street).
    #[test]
    fn missing_kinds_fall_back() {
        let only_houses = generate(&GenConfig::default(), &templates());
        let with = generate(&GenConfig::default(), &with_shops());
        let core = only_houses.town();
        let houses_in_core = only_houses.map.buildings.iter().filter(|b| b.min.cmpge(core.core_min).all() && (b.min + b.size).cmple(core.core_max).all()).count();
        assert!(houses_in_core >= 6, "only {houses_in_core} houses along the main street");
        assert!(only_houses.map.buildings.len() > with.map.buildings.len() / 2);

        let only_shops: Vec<BuildingTemplate> = with_shops().into_iter().filter(is_commercial).collect();
        let g = generate(&GenConfig::default(), &only_shops);
        assert!(g.map.buildings.iter().all(|b| b.min.cmpge(g.core_min).all()), "a shop off the main street");
        assert!(!g.map.buildings.is_empty());
        assert_eq!(g.map.room_at(g.player_spawn), 0);
        assert!(!g.map.tile_blocked(g.player_spawn.floor().as_ivec2()));

        let none = generate(&GenConfig::default(), &[]);
        assert!(none.map.buildings.is_empty());
        let zero = generate(&GenConfig { commercial_blocks: 0, ..Default::default() }, &with_shops());
        assert!(zero.map.buildings.iter().all(|b| matches!(kind_of(&with_shops(), b), BuildingKind::House)));
        assert_eq!(zero.core_min, zero.core_max, "no shops, no core");
    }

    /// The town core zone is the main street's shops, the rest of town streets and yards, forest
    /// round it; and there are parking lots, vacant lots and a park about.
    #[test]
    fn zones_and_open_lots() {
        let g = generate(&GenConfig::default(), &with_shops());
        let town = g.town();
        let mut zones = [0u32; 7];
        let (mut parking, mut dirt, mut paths) = (0, 0, 0);
        for y in 0..g.map.height() {
            for x in 0..g.map.width() {
                let t = IVec2::new(x, y);
                zones[zone_at(&g.map, &town, t, 1.0, 60.0) as usize] += 1;
                let tile = g.map.tile(t);
                let in_town = t.cmpge(town.min).all() && t.cmplt(town.max).all();
                // A parking bay line: road paint with grass, dirt or a fence-free yard next to it,
                // never beside a sidewalk (street lines run down the middle of the asphalt).
                if in_town && tile.floor == Floor::RoadLine {
                    let street = Side::ALL.iter().any(|s| g.map.tile(t + s.offset()).floor == Floor::Sidewalk);
                    parking += !street as u32;
                }
                dirt += (in_town && tile.floor == Floor::Dirt) as u32;
                paths += (in_town && tile.floor == Floor::Path) as u32;
            }
        }
        eprintln!("zones {zones:?}, road paint off the sidewalks {parking}, dirt {dirt}, paths {paths}");
        for z in Zone::ALL {
            assert!(zones[z as usize] > 0, "no {z:?}");
        }
        // The core is along the main street: its middle column crosses the main street's asphalt.
        let mid = (town.core_min.x + town.core_max.x) / 2;
        let main_rows = (town.core_min.y..town.core_max.y).filter(|&y| g.map.tile(IVec2::new(mid, y)).floor.is_road()).count();
        assert!(main_rows >= MAIN_ROAD as usize, "core misses the main street ({main_rows} rows of road)");
        assert!(dirt > 20 && paths > 50, "dirt {dirt}, paths {paths}");
    }

    /// A bigger map keeps the same town in the middle with more forest round it, and a tiny one is
    /// grown to fit the town.
    #[test]
    fn map_size_is_configurable() {
        let small = generate(&GenConfig::default(), &templates());
        assert_eq!((small.map.width(), small.map.height()), (480, 480));
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
        // Every seed's town fits the default map with forest round it.
        for seed in 1..20 {
            let g = generate(&GenConfig { seed, ..Default::default() }, &templates());
            assert_eq!(g.map.width(), 480, "seed {seed} grew the map");
            assert!(g.town_min.min_element() >= 30, "seed {seed}: forest only {} m", g.town_min.min_element());
        }
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
        assert!(trees_outside > 6000, "only {trees_outside} trees");
    }

    /// Prints the default town as ASCII, every other row
    /// (`cargo test -p zombye-sim --release print_town -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn print_town() {
        let templates = with_shops();
        let g = generate(&GenConfig::default(), &templates);
        for y in (g.town_min.y - 2..g.town_max.y + 2).step_by(2) {
            let row: String = (g.town_min.x - 2..g.town_max.x + 2)
                .map(|x| {
                    let t = g.map.tile(IVec2::new(x, y));
                    if t.room != 0 {
                        let b = g.map.rooms[t.room as usize].building as usize;
                        match kind_of(&templates, &g.map.buildings[b]) {
                            BuildingKind::House => 'H',
                            BuildingKind::Shop => 'S',
                            BuildingKind::Office => 'O',
                        }
                    } else {
                        match (t.feature, t.floor) {
                            (Feature::Tree | Feature::PineTree, _) => 't',
                            (Feature::Bush, _) => '*',
                            (_, Floor::Asphalt) => ' ',
                            (_, Floor::RoadLine) => '=',
                            (_, Floor::Sidewalk) => ':',
                            (_, Floor::Path) => ',',
                            (_, Floor::Dirt) => '~',
                            _ => '.',
                        }
                    }
                })
                .collect();
            println!("{row}");
        }
    }
}
