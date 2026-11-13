//! Seeded town generator (PLAN §4.1a), district by district. A wide main street runs east–west
//! through the town and on out through the forest. Its middle is the town core: the civic district
//! (a town square, and big blocks of landmark lots, about 40 × 50 m, for town halls, courthouses,
//! churches and big offices) with a commercial strip either side of it (medium blocks of shops and
//! offices on narrow lots along the main street, houses behind, sometimes an alley between). The
//! blocks across the main street don't line up with these, so most side streets meet it at a
//! T-junction. The core blocks of each side come in runs of different depths, each with its own back
//! street, so the back streets step; streets run out to the forest between runs and from the ends of
//! the row. The residential districts round the core are split again and again by side streets into
//! smaller blocks of varied sizes and shapes (T-junctions, now and then a crossing, streets that stop
//! at the forest, now and then a long block, dog-legs, L-turns and crescents), some with a cul-de-sac:
//! a dead end with a turning circle and houses round it. Lots that back onto the forest are bigger and
//! sparser. One street leaves north and one south.
//!
//! Templates are chosen by [`BuildingKind`] and footprint: houses on house lots, shops and offices on
//! the strip, civic buildings (and shops or offices too big for the strip) on landmark lots. A
//! landmark lot no civic building fits becomes a green, a parking lot, or the biggest office that
//! fits with parking beside it. With no shop or office templates the strip is lined with houses. The
//! same seed and templates always give the same map, and a bigger map the same town.

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::map::{Edge, Feature, Floor, Side, TileMap, TurningCircle, CHUNK_SIZE};
use crate::population::Zone;
use crate::rng::Rng;
use crate::street_props::{self, Fronts, HouseFront, ShopFront, StreetPropsDef};
use crate::template::{BuildingKind, BuildingTemplate, Placement};

/// Asphalt width of a street, and the sidewalk on each side.
const ROAD: i32 = 7;
const SIDEWALK: i32 = 2;
const CORRIDOR: i32 = ROAD + 2 * SIDEWALK;
/// Side streets in the residential districts are a little narrower.
const SIDE_ROAD: i32 = 6;
const SIDE_CORRIDOR: i32 = SIDE_ROAD + 2 * SIDEWALK;
/// The main street: wider asphalt, wider sidewalks, a solid centre line.
const MAIN_ROAD: i32 = 9;
const MAIN_SIDEWALK: i32 = 3;
const MAIN_CORRIDOR: i32 = MAIN_ROAD + 2 * MAIN_SIDEWALK;
/// A cul-de-sac's narrower asphalt, and its turning circle (asphalt, then the sidewalk round it).
const CUL_ROAD: i32 = 6;
const BULB_RADIUS: f32 = 8.5;
const BULB_OUTER: f32 = 10.5;
/// An alley behind the shops: plain asphalt, no sidewalks.
const ALLEY: i32 = 4;
/// Houses stand this far (plus a little) back from the sidewalk.
const FRONT_SETBACK: i32 = 5;
/// Shops and offices stand 1–2 m back from the main street's sidewalk, paved up to their fronts,
/// with this much yard behind them (bins, the back door, a fence), and 1–3 m alleys between.
const SHOP_BACK: i32 = 4;
/// A house row behind a row of shops is never shallower than this.
const MIN_HOUSE_ROW: i32 = 22;
/// A shop or office bigger than this (width, depth) doesn't fit the main street's lots: it goes on a
/// landmark lot.
const ROW_SHOP_MAX: IVec2 = IVec2::new(24, 16);
/// Client siding colours (`meshgen::siding_color`) for shops, offices and civic buildings: grey, tan, brick.
const SHOP_SIDING: [u8; 3] = [5, 6, 7];
/// Forest kept at least this wide round the town when the map is made bigger to fit it.
const MIN_FOREST: i32 = 24;
/// The narrowest residential strip between the end of the core and the forest.
const WING_MIN: i32 = 36;
/// Block depths: two rows of lots back to back, or one row (the forest, or other lots, behind it).
const TWO_ROWS: i32 = 44;
const ONE_ROW: i32 = 30;
/// A cul-de-sac block: this wide along the street it leaves from (at least, at most) and this deep.
const CUL_WIDTH: (i32, i32) = (56, 92);
const CUL_DEPTH: (i32, i32) = (52, 120);
/// The lot at the end of a cul-de-sac, beyond the turning circle.
const CUL_END_LOT: i32 = 24;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenConfig {
    pub seed: u64,
    /// Map size in chunks (each 16 m). Grown if the town wouldn't fit.
    pub chunks: IVec2,
    /// Town size in nominal blocks of about 78 × 68 m (the default 5 × 4 makes a town of about
    /// 400 × 290 m); the town grows if its core needs more room.
    pub blocks: IVec2,
    /// Commercial blocks on each side of the main street, split between the two ends of the civic
    /// district (both sides of the street). 0: none (the core is the civic district alone).
    #[serde(default = "default_commercial_blocks")]
    pub commercial_blocks: i32,
    /// The height of the ground: low-poly hills outside a flat town (`crate::terrain`).
    #[serde(default)]
    pub terrain: crate::terrain::TerrainDef,
}

fn default_commercial_blocks() -> i32 {
    2
}

impl Default for GenConfig {
    fn default() -> Self {
        // 30 chunks (480 m) holds the default town (at most about 420 × 320 m) with forest round it.
        Self { seed: 1, chunks: IVec2::new(30, 30), blocks: IVec2::new(5, 4), commercial_blocks: default_commercial_blocks(), terrain: crate::terrain::TerrainDef::default() }
    }
}

/// Where the town is (tiles): its bounds and its core, the civic and commercial blocks along the
/// main street: north of it (with the main street) and south of it (`min..max` each).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Town {
    pub min: IVec2,
    pub max: IVec2,
    pub core: [(IVec2, IVec2); 2],
}

/// What a district is for (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum District {
    Civic,
    Commercial,
    Residential,
    /// Residential lots backing onto the forest: bigger and sparser.
    Outer,
}

/// How a block was filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockUse {
    /// The town square.
    Square,
    /// One or two landmark lots.
    Landmarks,
    /// Shops and offices along the main street, houses behind.
    Shops,
    /// Rows of house lots facing the streets round it.
    Rows,
    /// A cul-de-sac with houses round it.
    CulDeSac,
}

#[derive(Clone, Copy, Debug)]
pub struct PlanBlock {
    pub min: IVec2,
    pub max: IVec2,
    pub district: District,
    pub fill: BlockUse,
}

/// The town's layout as the generator drew it (tiles; derived, not saved): for tests, tools and
/// anything that wants to know where the districts are.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub blocks: Vec<PlanBlock>,
    /// Street corridors (sidewalks included) in town, the main street first. Cul-de-sacs and
    /// alleys are counted below, not listed.
    pub streets: Vec<(IVec2, IVec2)>,
    /// Alleys behind rows of shops.
    pub alleys: usize,
    /// The middle of each cul-de-sac's turning circle.
    pub cul_de_sacs: Vec<Vec2>,
    /// The town square.
    pub square: Option<(IVec2, IVec2)>,
    /// Landmark lots.
    pub landmarks: Vec<(IVec2, IVec2)>,
    /// Streets that stop at the forest (the roads out of town aren't counted).
    pub dead_ends: usize,
    /// Bent streets: dog-legs, L-turns and crescents.
    pub bends: usize,
}

pub struct Generated {
    pub map: TileMap,
    /// Town bounds (tiles), used for zombie density and forest.
    pub town_min: IVec2,
    pub town_max: IVec2,
    /// The town core (tiles; see [`Town`]).
    pub core: [(IVec2, IVec2); 2],
    /// Where the player wakes up, and the building it's in.
    pub player_spawn: Vec2,
    pub spawn_building: usize,
    pub plan: Plan,
}

impl Generated {
    pub fn town(&self) -> Town {
        Town { min: self.town_min, max: self.town_max, core: self.core }
    }
}

/// A rectangle of tiles: `min..min + size`.
#[derive(Clone, Copy, Debug, PartialEq)]
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
    fn grow(&self, by: i32) -> Self {
        Self::new(self.min - IVec2::splat(by), self.size + IVec2::splat(2 * by))
    }
}

/// Index of a side in `Side::ALL` order (N, E, S, W).
fn si(s: Side) -> usize {
    match s {
        Side::N => 0,
        Side::E => 1,
        Side::S => 2,
        Side::W => 3,
    }
}

/// A lot (or a row of lots, or a whole block seen from one street): a rectangle and the side of it
/// that faces the street. "Along" runs along the front, "back" away from it.
#[derive(Clone, Copy, Debug)]
struct Lot {
    r: Rect,
    front: Side,
}

impl Lot {
    fn frontage(&self) -> i32 {
        if matches!(self.front, Side::N | Side::S) { self.r.size.x } else { self.r.size.y }
    }
    fn depth(&self) -> i32 {
        if matches!(self.front, Side::N | Side::S) { self.r.size.y } else { self.r.size.x }
    }
    /// The part `a0..a1` along the front and `d0..d1` back from it (either may run outside the lot).
    fn sub(&self, a0: i32, a1: i32, d0: i32, d1: i32) -> Rect {
        let (min, max) = (self.r.min, self.r.max());
        match self.front {
            Side::N => Rect::new(IVec2::new(min.x + a0, min.y + d0), IVec2::new(a1 - a0, d1 - d0)),
            Side::S => Rect::new(IVec2::new(min.x + a0, max.y - d1), IVec2::new(a1 - a0, d1 - d0)),
            Side::W => Rect::new(IVec2::new(min.x + d0, min.y + a0), IVec2::new(d1 - d0, a1 - a0)),
            Side::E => Rect::new(IVec2::new(max.x - d1, min.y + a0), IVec2::new(d1 - d0, a1 - a0)),
        }
    }
    /// Where tile `t` is: (along, back from the front).
    fn local(&self, t: IVec2) -> (i32, i32) {
        let (min, max) = (self.r.min, self.r.max());
        match self.front {
            Side::N => (t.x - min.x, t.y - min.y),
            Side::S => (t.x - min.x, max.y - 1 - t.y),
            Side::W => (t.y - min.y, t.x - min.x),
            Side::E => (t.y - min.y, max.x - 1 - t.x),
        }
    }
    /// Clockwise quarter turns that make a template (front to the south) face this lot's street.
    fn turns(&self) -> u8 {
        match self.front {
            Side::S => 0,
            Side::W => 1,
            Side::N => 2,
            Side::E => 3,
        }
    }
    /// A template's footprint turned to face the street: (along, deep).
    fn dims(&self, t: &BuildingTemplate) -> (i32, i32) {
        let s = t.rotated_size(self.turns());
        if matches!(self.front, Side::N | Side::S) { (s.x, s.y) } else { (s.y, s.x) }
    }
    /// The direction "along" grows in.
    fn along_side(&self) -> Side {
        if matches!(self.front, Side::N | Side::S) { Side::E } else { Side::S }
    }
}

/// A part of town to split into blocks, or a block: the sidewalk width of the street on each side
/// (`Side::ALL` order; `None`: no street, the forest or the backs of other lots), and which sides
/// are the edge of town.
#[derive(Clone, Copy, Debug)]
struct Region {
    r: Rect,
    streets: [Option<i32>; 4],
    edge: [bool; 4],
}

impl Region {
    fn street(&self, s: Side) -> Option<i32> {
        self.streets[si(s)]
    }

    /// Side `s` is the backs of other lots: no street and not the edge of town.
    fn ends_blind(&self, s: Side) -> bool {
        self.street(s).is_none() && !self.edge[si(s)]
    }
}

/// The chance that a region split by a street gets a bent one (where one fits).
const BENT_CHANCE: f32 = 0.6;
/// The shallowest piece a bent split leaves: one row of lots.
const ROW_MIN: i32 = 24;

/// The shapes of bent street [`TownBuilder::bent_split`] draws.
#[derive(Clone, Copy, Debug)]
enum Bend {
    Jog,
    L,
    Crescent,
}

/// A side of a region in a [`Frame`]: the ends across its long side (S0 at s = 0, S1) and its long
/// sides (T0 at t = 0, T1).
#[derive(Clone, Copy, Debug, PartialEq)]
enum FSide {
    S0,
    S1,
    T0,
    T1,
}

/// What a piece of a bent split has on one side: what the region had there, a new street, or the
/// backs of the lots of another piece.
#[derive(Clone, Copy, Debug)]
enum PieceSide {
    Keep,
    Street,
    Back,
}
use PieceSide::{Back, Keep};

/// A region seen along its longer side: `s` runs along it (`0..long`), `t` across (`0..short`),
/// either flipped, so one bent street is drawn every way round.
#[derive(Clone, Copy, Debug)]
struct Frame {
    r: Rect,
    long_x: bool,
    flip_s: bool,
    flip_t: bool,
}

impl Frame {
    /// (long, short).
    fn len(&self) -> (i32, i32) {
        if self.long_x { (self.r.size.x, self.r.size.y) } else { (self.r.size.y, self.r.size.x) }
    }

    /// The tiles `s0..s1` along, `t0..t1` across (either may reach outside the region).
    fn rect(&self, s0: i32, s1: i32, t0: i32, t1: i32) -> Rect {
        let (ll, ls) = self.len();
        let (s0, s1) = if self.flip_s { (ll - s1, ll - s0) } else { (s0, s1) };
        let (t0, t1) = if self.flip_t { (ls - t1, ls - t0) } else { (t0, t1) };
        if self.long_x {
            Rect::new(self.r.min + IVec2::new(s0, t0), IVec2::new(s1 - s0, t1 - t0))
        } else {
            Rect::new(self.r.min + IVec2::new(t0, s0), IVec2::new(t1 - t0, s1 - s0))
        }
    }

    fn side(&self, f: FSide) -> Side {
        match (f, self.long_x) {
            (FSide::S0, true) => if self.flip_s { Side::E } else { Side::W },
            (FSide::S1, true) => if self.flip_s { Side::W } else { Side::E },
            (FSide::T0, true) => if self.flip_t { Side::S } else { Side::N },
            (FSide::T1, true) => if self.flip_t { Side::N } else { Side::S },
            (FSide::S0, false) => if self.flip_s { Side::S } else { Side::N },
            (FSide::S1, false) => if self.flip_s { Side::N } else { Side::S },
            (FSide::T0, false) => if self.flip_t { Side::E } else { Side::W },
            (FSide::T1, false) => if self.flip_t { Side::W } else { Side::E },
        }
    }

    /// The region for `(s0, s1, t0, t1)` with what's on each of its sides.
    fn piece(&self, reg: &Region, (s0, s1, t0, t1): (i32, i32, i32, i32), sides: [(FSide, PieceSide); 4]) -> Region {
        let mut out = Region { r: self.rect(s0, s1, t0, t1), streets: [None; 4], edge: [false; 4] };
        for (f, what) in sides {
            let side = si(self.side(f));
            (out.streets[side], out.edge[side]) = match what {
                Keep => (reg.streets[side], reg.edge[side]),
                PieceSide::Street => (Some(SIDEWALK), false),
                Back => (None, false),
            };
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    None,
    Dashed,
    Solid,
}

/// A street corridor: sidewalks either side of the asphalt, across `r` (`horizontal`: running
/// east–west). Side streets reach across the sidewalk of the street they meet.
#[derive(Clone, Copy, Debug)]
struct Street {
    r: Rect,
    horizontal: bool,
    road: i32,
    sidewalk: i32,
    line: Line,
    /// Ends (north or west first) that stop at the forest rather than at another street.
    open: [bool; 2],
}

/// A block of the core along the main street.
#[derive(Clone, Copy, Debug)]
enum CoreUse {
    Commercial,
    Square,
    /// One landmark lot facing that way.
    Landmark(Side),
    /// Two landmark lots side by side, facing the main street.
    LandmarkPair,
}

fn is_commercial(t: &BuildingTemplate) -> bool {
    matches!(t.kind, BuildingKind::Shop | BuildingKind::Office)
}

fn area(t: &BuildingTemplate) -> i32 {
    t.size().x * t.size().y
}

/// A row of core blocks (west edge, width, use), its cross streets and its two end streets.
type CoreRow = (Vec<(i32, i32, CoreUse)>, Vec<i32>, i32, i32);

/// Lay a row of blocks along the main street outward from the civic district: `west` and `east`
/// commercial block widths (nearest the civic district first), the civic blocks between. Returns
/// each block's west edge, width and use, the cross streets between them (their west edges), and
/// the two long streets at the ends (their west edges), all relative to the civic district's west
/// edge.
fn core_row(west: &[i32], civic: &[(i32, CoreUse)], east: &[i32]) -> CoreRow {
    let (mut blocks, mut cross) = (Vec::new(), Vec::new());
    let mut x = 0;
    for &w in west {
        cross.push(x - CORRIDOR);
        x -= CORRIDOR + w;
        blocks.push((x, w, CoreUse::Commercial));
    }
    let west_end = x - CORRIDOR;
    x = 0;
    for (i, &(w, u)) in civic.iter().enumerate() {
        if i > 0 {
            cross.push(x);
            x += CORRIDOR;
        }
        blocks.push((x, w, u));
        x += w;
    }
    for &w in east {
        cross.push(x);
        x += CORRIDOR;
        blocks.push((x, w, CoreUse::Commercial));
        x += w;
    }
    blocks.sort_by_key(|b| b.0);
    cross.sort();
    (blocks, cross, west_end, x)
}

/// Make the commercial blocks of one side of the main street clearly line up with the other side's
/// or clearly not (no jogs of a few metres where two side streets nearly meet across it), the two
/// rows starting `start_other` and `start_ours` from the civic district's west edge.
fn offset_from(other: &[i32], ours: &mut [i32], start_other: i32, start_ours: i32) {
    let (mut a, mut b) = (start_other, start_ours);
    for (o, w) in other.iter().zip(ours.iter_mut()) {
        a += o;
        b += *w;
        let d = b - a;
        let fix = if d == 0 { 0 } else if d.abs() < 7 { -d } else if d.abs() < 14 { d.signum() * 14 - d } else { 0 };
        *w += fix;
        b += fix;
    }
}

/// A run of core blocks along one side of the main street (`first..=last` of the row's blocks) with
/// one depth and one back street; the cross streets between runs carry on to the forest.
#[derive(Clone, Copy, Debug)]
struct CoreRun {
    first: usize,
    last: usize,
    depth: i32,
}

/// The chance that a cross street between two core blocks carries on to the forest, ending a run.
const THROUGH_CHANCE: f32 = 0.4;
/// Back streets either side of a street to the forest step by at least this.
const MIN_STEP: i32 = 12;

/// Split a row of core blocks into runs and give each a depth: blocks of the civic district deep
/// enough for its landmark lots, neighbouring runs clearly different depths (else one run). No run
/// ends at a cross street in line with one of `avoid` (civic-relative).
fn core_runs(rng: &mut Rng, row: &CoreRow, avoid: &[i32]) -> Vec<CoreRun> {
    let n = row.0.len();
    let mut runs: Vec<CoreRun> = Vec::new();
    let mut first = 0;
    for i in 0..n {
        // (Never in line with a street out to the forest across the main street: no long
        // straight streets from forest to forest.)
        if i + 1 < n && (rng.f32() >= THROUGH_CHANCE || avoid.iter().any(|&x| (x - row.1[i]).abs() <= 2)) {
            continue;
        }
        let civic = row.0[first..=i].iter().any(|b| !matches!(b.2, CoreUse::Commercial));
        let (lo, hi) = if civic { (48, 62) } else { (44, 62) };
        let options: Vec<i32> = (lo..=hi).filter(|&d| runs.last().is_none_or(|p| (d - p.depth).abs() >= MIN_STEP)).collect();
        if options.is_empty() {
            // No depth steps cleanly from the last run's: carry it on instead.
            let prev = runs.last_mut().unwrap();
            prev.last = i;
            prev.depth = prev.depth.max(lo);
        } else {
            let depth = options[rng.below(options.len() as u32) as usize];
            runs.push(CoreRun { first, last: i, depth });
        }
        first = i + 1;
    }
    runs
}

/// The town with no street props (see [`generate_with`]).
pub fn generate(config: &GenConfig, templates: &[BuildingTemplate]) -> Generated {
    generate_with(config, templates, &StreetPropsDef::default())
}

/// The town, with the street props `props` asks for (`crate::street_props`), placed last from their
/// own random streams, so the rest of the town is the same with or without them.
pub fn generate_with(config: &GenConfig, templates: &[BuildingTemplate], props: &StreetPropsDef) -> Generated {
    generate_full(config, templates, props, crate::biomes::shipped())
}

/// The town with its street props, and round it what `biomes` grows (`crate::biomes`): trees,
/// bushes and bare patches by hashes of the seed and the tile, so the town itself is the same
/// whatever the biomes say.
pub fn generate_full(config: &GenConfig, templates: &[BuildingTemplate], props: &StreetPropsDef, biomes: &crate::biomes::BiomeData) -> Generated {
    let mut rng = Rng::new(config.seed ^ 0x70_77_4e_00);
    let blocks = config.blocks.max(IVec2::ONE);
    let fits_row = |t: &BuildingTemplate| t.size().x <= ROW_SHOP_MAX.x && t.size().y <= ROW_SHOP_MAX.y;
    let houses: Vec<&BuildingTemplate> = templates.iter().filter(|t| matches!(t.kind, BuildingKind::House)).collect();
    let shops: Vec<&BuildingTemplate> = templates.iter().filter(|t| is_commercial(t) && fits_row(t)).collect();
    let landmarks: Vec<&BuildingTemplate> = templates.iter().filter(|t| matches!(t.kind, BuildingKind::Civic) || (is_commercial(t) && !fits_row(t))).collect();
    let offices: Vec<&BuildingTemplate> = templates.iter().filter(|t| matches!(t.kind, BuildingKind::Office)).collect();

    // The core, west to east along the main street: commercial blocks, the civic district,
    // commercial blocks, with the civic district anywhere along the row. North of the main street
    // the civic district is the square with a landmark lot beside it (now and then one each side,
    // or none); south of it one wide block of two landmark lots, or a single lot.
    let k = config.commercial_blocks.clamp(0, 16);
    let k_west = rng.below(k as u32 + 1) as i32;
    let k_east = k - k_west;
    let landmark_w = |rng: &mut Rng| 42 + rng.below(5) as i32;
    let square_w = 40 + rng.below(7) as i32;
    let square_east = rng.f32() < 0.5;
    let roll = rng.f32();
    let north_civic: Vec<(i32, CoreUse)> = if roll < 0.2 {
        vec![(landmark_w(&mut rng), CoreUse::Landmark(Side::E)), (square_w, CoreUse::Square), (landmark_w(&mut rng), CoreUse::Landmark(Side::W))]
    } else if roll < 0.85 {
        let l = landmark_w(&mut rng);
        if square_east { vec![(l, CoreUse::Landmark(Side::E)), (square_w, CoreUse::Square)] } else { vec![(square_w, CoreUse::Square), (l, CoreUse::Landmark(Side::W))] }
    } else {
        vec![(square_w, CoreUse::Square)]
    };
    let north_w = north_civic.iter().map(|c| c.0).sum::<i32>() + CORRIDOR * (north_civic.len() as i32 - 1);
    let pair = rng.f32() < 0.65;
    let (min_w, max_w) = if pair { (86, 98) } else { (46, 54) };
    let mut south_w = min_w + rng.below((max_w - min_w + 1) as u32) as i32;
    // The south side's civic district starts in line with the north side's, or clearly along
    // from it; its east end too.
    let south_shift = if rng.f32() < 0.3 { 0 } else { (14 + rng.below(9) as i32) * if rng.f32() < 0.5 { -1 } else { 1 } };
    let east_d = south_shift + south_w - north_w;
    if east_d != 0 && east_d.abs() < 14 {
        let target = north_w - south_shift;
        south_w = [target, target - 14, target + 14].into_iter().find(|w| (min_w..=max_w + 14).contains(w)).unwrap_or(south_w);
    }
    let south_civic = [(south_w, if pair { CoreUse::LandmarkPair } else { CoreUse::Landmark(Side::N) })];
    let mut widths = |n: i32| -> Vec<i32> { (0..n).map(|_| 50 + rng.below(21) as i32).collect() };
    let (n_west, n_east) = (widths(k_west), widths(k_east));
    let (mut s_west, mut s_east) = (widths(k_west), widths(k_east));
    offset_from(&n_west, &mut s_west, 0, -south_shift);
    offset_from(&n_east, &mut s_east, north_w, south_shift + south_w);
    let north = core_row(&n_west, &north_civic, &n_east);
    let mut south = core_row(&s_west, &south_civic, &s_east);
    for b in &mut south.0 {
        b.0 += south_shift;
    }
    for x in &mut south.1 {
        *x += south_shift;
    }
    (south.2, south.3) = (south.2 + south_shift, south.3 + south_shift);
    let north_runs = core_runs(&mut rng, &north, &[]);
    let mut out_north: Vec<i32> = north_runs[..north_runs.len() - 1].iter().map(|r| north.1[r.last]).collect();
    out_north.extend([north.2, north.3]);
    let south_runs = core_runs(&mut rng, &south, &out_north);
    let deepest = |runs: &[CoreRun]| runs.iter().map(|r| r.depth).max().unwrap_or(48);
    let (depth_n, depth_s) = (deepest(&north_runs), deepest(&south_runs));

    // The town round it: at least a strip of houses beyond each end of the core, wider as the
    // nominal size allows, the core off centre by a little.
    let lo = north.2.min(south.2) - WING_MIN;
    let hi = (north.3 + CORRIDOR).max(south.3 + CORRIDOR) + WING_MIN;
    let width = (hi - lo).max(blocks.x * 74 + rng.below(21) as i32);
    let extra = width - (hi - lo);
    let west_extra = extra * (35 + rng.below(31) as i32) / 100;
    let half = |rng: &mut Rng, depth: i32| (blocks.y * 33 - 6 + rng.below(26) as i32).max(depth + CORRIDOR + ONE_ROW + 4);
    let (half_n, half_s) = (half(&mut rng, depth_n), half(&mut rng, depth_s));
    let total = IVec2::new(width, half_n + MAIN_CORRIDOR + half_s);

    // The town sits in the middle of however big the map is (more forest round it on a bigger map),
    // but always fits, with a margin of forest.
    let need = (total + IVec2::splat(2 * MIN_FOREST) + IVec2::splat(CHUNK_SIZE - 1)) / CHUNK_SIZE;
    let map = TileMap::new(config.chunks.x.max(need.x), config.chunks.y.max(need.y));
    let size = IVec2::new(map.width(), map.height());
    let town_min = (size - total) / 2;
    let town_max = town_min + total;
    let civic_x = town_min.x - (lo - west_extra);
    let main_y = town_min.y + half_n;

    let mut town = TownBuilder {
        map,
        rng,
        houses,
        shops,
        landmarks,
        offices,
        homes: Vec::new(),
        streets: Vec::new(),
        plan: Plan::default(),
        town_min,
        town_max,
        last_fallback: None,
        fronts: Fronts::default(),
        elbows: Vec::new(),
    };
    town.add_street(Street {
        r: Rect::new(IVec2::new(town_min.x, main_y), IVec2::new(total.x, MAIN_CORRIDOR)),
        horizontal: true,
        road: MAIN_ROAD,
        sidewalk: MAIN_SIDEWALK,
        line: Line::Solid,
        open: [false; 2],
    });

    // Each side of the main street: its row of core blocks, run by run, each run with its own depth
    // and a back street behind it; streets from the main street out to the forest at the ends of
    // the row and between runs; and the residential parts of the side: beyond each end of the row
    // (from the main street to the forest) and behind each run (from its back street to the
    // forest). Each part stops short of the edge of town by its own few metres, the woods coming up
    // to it, so the edge is ragged.
    let mut core = [(IVec2::ZERO, IVec2::ZERO); 2];
    let mut core_blocks = Vec::new();
    let mut regions = Vec::new();
    let mut wild = Vec::new();
    for (i, (row, runs)) in [(&north, &north_runs), (&south, &south_runs)].into_iter().enumerate() {
        let is_north = i == 0;
        let (half_y0, half_y1) = if is_north { (town_min.y, main_y) } else { (main_y + MAIN_CORRIDOR, town_max.y) };
        let h = half_y1 - half_y0;
        let (west_end, east_end) = (civic_x + row.2, civic_x + row.3);
        let main_side = if is_north { Side::S } else { Side::N };
        // Where a run of depth `d` starts (its main-street side) and its back street.
        let row_y = |d: i32| if is_north { main_y - d } else { main_y + MAIN_CORRIDOR };
        let back_y = |d: i32| if is_north { main_y - d - CORRIDOR } else { main_y + MAIN_CORRIDOR + d };
        let mut through = vec![west_end];
        through.extend(runs[..runs.len() - 1].iter().map(|r| civic_x + row.1[r.last]));
        through.push(east_end);
        let mut parts = vec![(Rect::new(IVec2::new(town_min.x, half_y0), IVec2::new(west_end - town_min.x, h)), vec![(main_side, MAIN_SIDEWALK), (Side::E, SIDEWALK)], TWO_ROWS + SIDE_CORRIDOR + ONE_ROW)];
        for (k, r) in runs.iter().enumerate() {
            let (x0, x1) = (through[k] + CORRIDOR, through[k + 1]);
            let (by0, by1) = if is_north { (half_y0, back_y(r.depth)) } else { (back_y(r.depth) + CORRIDOR, half_y1) };
            parts.push((Rect::new(IVec2::new(x0, by0), IVec2::new(x1 - x0, by1 - by0)), vec![(main_side, SIDEWALK), (Side::E, SIDEWALK), (Side::W, SIDEWALK)], ONE_ROW + 4));
        }
        parts.push((Rect::new(IVec2::new(east_end + CORRIDOR, half_y0), IVec2::new(town_max.x - east_end - CORRIDOR, h)), vec![(main_side, MAIN_SIDEWALK), (Side::W, SIDEWALK)], TWO_ROWS + SIDE_CORRIDOR + ONE_ROW));
        let mut trims = Vec::new();
        for (mut r, streets, keep) in parts {
            let trim = (town.rng.below(17) as i32).min((r.size.y - keep).max(0));
            trims.push(trim);
            r.size.y -= trim;
            if is_north {
                r.min.y += trim;
                wild.push(Rect::new(IVec2::new(r.min.x, half_y0), IVec2::new(r.size.x, trim)));
            } else {
                wild.push(Rect::new(IVec2::new(r.min.x, r.max().y), IVec2::new(r.size.x, trim)));
            }
            let mut reg = Region { r, streets: [None; 4], edge: [false; 4] };
            for (s, w) in streets {
                reg.streets[si(s)] = Some(w);
            }
            // Every side without a street is the edge of town.
            for s in Side::ALL {
                reg.edge[si(s)] = reg.street(s).is_none();
            }
            regions.push(reg);
        }
        for (k, &x) in through.iter().enumerate() {
            let trim = trims[k].min(trims[k + 1]);
            let r = if is_north {
                Rect::new(IVec2::new(x, half_y0 + trim), IVec2::new(CORRIDOR, h - trim + MAIN_SIDEWALK))
            } else {
                Rect::new(IVec2::new(x, half_y0 - MAIN_SIDEWALK), IVec2::new(CORRIDOR, h - trim + MAIN_SIDEWALK))
            };
            town.add_street(Street { r, horizontal: false, road: ROAD, sidewalk: SIDEWALK, line: Line::Dashed, open: [is_north, !is_north] });
        }
        for (k, run) in runs.iter().enumerate() {
            let (x0, x1, d) = (through[k], through[k + 1], run.depth);
            town.add_street(Street {
                r: Rect::new(IVec2::new(x0 + CORRIDOR - SIDEWALK, back_y(d)), IVec2::new(x1 - x0 - CORRIDOR + 2 * SIDEWALK, CORRIDOR)),
                horizontal: true,
                road: ROAD,
                sidewalk: SIDEWALK,
                line: Line::Dashed,
                open: [false; 2],
            });
            // Cross streets between its blocks, from the main street to its back street.
            for &cx in &row.1[run.first..run.last] {
                let r = if is_north {
                    Rect::new(IVec2::new(civic_x + cx, back_y(d) + CORRIDOR - SIDEWALK), IVec2::new(CORRIDOR, d + SIDEWALK + MAIN_SIDEWALK))
                } else {
                    Rect::new(IVec2::new(civic_x + cx, row_y(d) - MAIN_SIDEWALK), IVec2::new(CORRIDOR, d + SIDEWALK + MAIN_SIDEWALK))
                };
                town.add_street(Street { r, horizontal: false, road: ROAD, sidewalk: SIDEWALK, line: Line::Dashed, open: [false; 2] });
            }
            for &(x, w, u) in &row.0[run.first..=run.last] {
                core_blocks.push((Rect::new(IVec2::new(civic_x + x, row_y(d)), IVec2::new(w, d)), u, is_north));
            }
        }
        let d = deepest(runs);
        let (cy0, cy1) = if is_north { (main_y - d, main_y + MAIN_CORRIDOR) } else { (main_y, main_y + MAIN_CORRIDOR + d) };
        core[i] = (IVec2::new(west_end + CORRIDOR, cy0), IVec2::new(east_end, cy1));
    }

    // Split the residential parts into blocks with side streets.
    let mut leaves = Vec::new();
    for reg in regions {
        town.subdivide(reg, &mut leaves);
    }
    // Blocks on the edge of town stop short of it by their own few metres (the street beside them
    // runs on into the trees), so the woods come up to the town raggedly.
    for leaf in &mut leaves {
        for s in Side::ALL {
            if !leaf.edge[si(s)] {
                continue;
            }
            let lot = Lot { r: leaf.r, front: s };
            let keep = if leaf.street(s.opposite()).is_some() { ONE_ROW + 6 } else { TWO_ROWS };
            let trim = (town.rng.below(19) as i32).min(lot.depth() - keep);
            if trim > 0 {
                wild.push(lot.sub(0, lot.frontage(), 0, trim));
                leaf.r = lot.sub(0, lot.frontage(), trim, lot.depth());
            }
        }
    }
    let culs = town.choose_cul_de_sacs(&leaves);
    let rows: Vec<usize> = (0..leaves.len()).filter(|i| !culs.iter().any(|c| c.0 == *i)).collect();
    let park = if rows.is_empty() { None } else { Some(rows[town.rng.below(rows.len() as u32) as usize]) };

    // Streets first, then what's between them.
    for s in town.streets.clone() {
        paint_street(&mut town.map, &s);
    }
    paint_lines(&mut town.map, &town.streets);
    for &(r, outer) in &town.elbows {
        paint_elbow(&mut town.map, r, outer);
    }
    for (r, u, is_north) in core_blocks {
        town.core_block(r, u, is_north);
    }
    for (i, leaf) in leaves.iter().enumerate() {
        if let Some(&(_, entry)) = culs.iter().find(|c| c.0 == i) {
            town.cul_de_sac(leaf, entry);
            town.plan.blocks.push(PlanBlock { min: leaf.r.min, max: leaf.r.max(), district: District::Residential, fill: BlockUse::CulDeSac });
        } else {
            let district = town.rows_block(leaf, park == Some(i));
            town.plan.blocks.push(PlanBlock { min: leaf.r.min, max: leaf.r.max(), district, fill: BlockUse::Rows });
        }
    }
    town.roads_out(size, main_y);
    let TownBuilder { mut map, mut rng, homes, plan, fronts, .. } = town;

    // The hills first: the forest thins out on their steepest faces, so the ground shows there.
    map.terrain = crate::terrain::Terrain::generate(&config.terrain, config.seed, size, town_min, town_max);
    let wild_rects: Vec<(IVec2, IVec2)> = wild.iter().map(|r| (r.min, r.max())).collect();
    map.biomes = crate::biomes::BiomeMap::generate(biomes, config.seed, size, town_min, town_max, &wild_rects, &map.terrain);
    plant_wild(&mut map);

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
    street_props::place(&mut map, props, config.seed, town_min, town_max, &fronts);
    map.index_storeys();
    Generated { map, town_min, town_max, core, player_spawn, spawn_building, plan }
}

/// The map and RNG while the town is laid out, with the templates by use, the houses placed so far
/// and the streets.
struct TownBuilder<'a> {
    map: TileMap,
    rng: Rng,
    houses: Vec<&'a BuildingTemplate>,
    /// Shops and offices that fit the main street's lots.
    shops: Vec<&'a BuildingTemplate>,
    /// Civic buildings, and shops and offices too big for the main street.
    landmarks: Vec<&'a BuildingTemplate>,
    offices: Vec<&'a BuildingTemplate>,
    /// Building indices of the houses placed.
    homes: Vec<usize>,
    streets: Vec<Street>,
    plan: Plan,
    town_min: IVec2,
    town_max: IVec2,
    /// What the last landmark lot nothing civic fitted became (so the next is something else).
    last_fallback: Option<u8>,
    /// Houses' garden paths and shops' fronts, for the street props.
    fronts: Fronts,
    /// The turns of bent streets: their corner squares and the two sides on the outside of each
    /// (see [`paint_elbow`]).
    elbows: Vec<(Rect, [Side; 2])>,
}

impl TownBuilder<'_> {
    fn add_street(&mut self, s: Street) {
        self.plan.streets.push((s.r.min, s.r.max()));
        self.streets.push(s);
    }

    /// Split a part of town into blocks with side streets, until each is small enough.
    fn subdivide(&mut self, reg: Region, leaves: &mut Vec<Region>) {
        let mut stack = vec![reg];
        while let Some(reg) = stack.pop() {
            if reg.r.size.min_element() <= 0 {
                continue;
            }
            match self.split(&reg) {
                Some(pieces) => stack.extend(pieces.into_iter().rev()),
                None => leaves.push(reg),
            }
        }
    }

    /// Split a region with a street across its longer side, if it's too big for a block (or now and
    /// then if it's merely big: some blocks stay long). Each half keeps room for its rows of lots.
    fn split(&mut self, reg: &Region) -> Option<Vec<Region>> {
        let (w, h) = (reg.r.size.x, reg.r.size.y);
        let (long, short) = (w.max(h), w.min(h));
        if !(long > 104 || short > 72 || (long > 80 && self.rng.f32() < 0.5)) {
            return None;
        }
        if self.rng.f32() < BENT_CHANCE
            && let Some(pieces) = self.bent_split(reg)
        {
            return Some(pieces);
        }
        let vertical = w >= h; // a north–south street, splitting it west and east
        let (lo, hi) = if vertical { (Side::W, Side::E) } else { (Side::N, Side::S) };
        let (end_a, end_b) = if vertical { (Side::N, Side::S) } else { (Side::W, Side::E) };
        // A street never stops against the backs of other lots: only at a street or the forest.
        if reg.ends_blind(end_a) || reg.ends_blind(end_b) {
            return None;
        }
        let need = |s: Side| if reg.street(s).is_some() { TWO_ROWS } else { ONE_ROW };
        let (min_lo, max_at) = (need(lo), if vertical { w } else { h } - SIDE_CORRIDOR - need(hi));
        if max_at < min_lo {
            return None;
        }
        let at = self.split_at(reg, vertical, min_lo, max_at);
        let ext = |s: Side| reg.street(s).unwrap_or(0);
        let (mut a, mut b) = (*reg, *reg);
        let r = reg.r;
        let street = if vertical {
            a.r = Rect::new(r.min, IVec2::new(at, h));
            b.r = Rect::new(IVec2::new(r.min.x + at + SIDE_CORRIDOR, r.min.y), IVec2::new(w - at - SIDE_CORRIDOR, h));
            Rect::new(IVec2::new(r.min.x + at, r.min.y - ext(Side::N)), IVec2::new(SIDE_CORRIDOR, h + ext(Side::N) + ext(Side::S)))
        } else {
            a.r = Rect::new(r.min, IVec2::new(w, at));
            b.r = Rect::new(IVec2::new(r.min.x, r.min.y + at + SIDE_CORRIDOR), IVec2::new(w, h - at - SIDE_CORRIDOR));
            Rect::new(IVec2::new(r.min.x - ext(Side::W), r.min.y + at), IVec2::new(w + ext(Side::W) + ext(Side::E), SIDE_CORRIDOR))
        };
        a.streets[si(hi)] = Some(SIDEWALK);
        a.edge[si(hi)] = false;
        b.streets[si(lo)] = Some(SIDEWALK);
        b.edge[si(lo)] = false;
        let open = [reg.street(end_a).is_none(), reg.street(end_b).is_none()];
        self.add_street(Street { r: street, horizontal: !vertical, road: SIDE_ROAD, sidewalk: SIDEWALK, line: Line::Dashed, open });
        Some(vec![a, b])
    }

    /// Split a long region with a bent street instead of a straight one, where it fits: a dog-leg
    /// (across it with a jog part way), an L (in from one end, turning to a long side), or a
    /// crescent (in from a long side and back out to it round a block). The pieces left are
    /// rectangles; where two meet with no street between them, neither has lots facing that way
    /// (it's the backs of other lots), and no street is ever split off against such a side.
    fn bent_split(&mut self, reg: &Region) -> Option<Vec<Region>> {
        let mut forms = [Bend::Jog, Bend::L, Bend::Crescent];
        for i in (1..forms.len()).rev() {
            forms.swap(i, self.rng.below(i as u32 + 1) as usize);
        }
        let first_flip = self.rng.below(4);
        for form in forms {
            for k in 0..4 {
                let f = (first_flip + k) % 4;
                let frame = Frame { r: reg.r, long_x: reg.r.size.x >= reg.r.size.y, flip_s: f & 1 == 1, flip_t: f & 2 == 2 };
                if let Some(pieces) = self.bend(reg, frame, form) {
                    return Some(pieces);
                }
            }
        }
        None
    }

    /// One bent street through `reg`, seen in `frame` (see [`Frame`]): its pieces, or `None` if the
    /// region doesn't fit this form this way round.
    fn bend(&mut self, reg: &Region, fr: Frame, form: Bend) -> Option<Vec<Region>> {
        use FSide::*;
        let (ll, ls) = fr.len();
        let c = SIDE_CORRIDOR;
        let street = |f: FSide| reg.street(fr.side(f));
        let blind = |f: FSide| reg.ends_blind(fr.side(f));
        let pick = |rng: &mut Rng, lo: i32, hi: i32| (hi >= lo).then(|| lo + rng.below((hi - lo + 1) as u32) as i32);
        let mut pieces = Vec::new();
        let mut streets = Vec::new();
        // Each turn's corner square, and the two sides of it on the outside of the turn.
        let mut elbows: Vec<(Rect, [FSide; 2])> = Vec::new();
        match form {
            Bend::Jog => {
                // Across the region from T0 to T1, shifted along it by 14-30 m half way: streets
                // along both, which the pieces either side of the jog face.
                if blind(T0) || blind(T1) || !(60..=100).contains(&ls) {
                    return None;
                }
                let m = pick(&mut self.rng, ROW_MIN, ls - c - ROW_MIN)?;
                let a1 = pick(&mut self.rng, 36, ll - c - 36 - 14)?;
                let a2 = pick(&mut self.rng, a1 + 14, (a1 + 30).min(ll - c - 36))?;
                streets.push((fr.rect(a1, a1 + c, -street(T0).unwrap_or(0), m + c), false));
                streets.push((fr.rect(a1, a2 + c, m, m + c), true));
                streets.push((fr.rect(a2, a2 + c, m, ls + street(T1).unwrap_or(0)), false));
                elbows.push((fr.rect(a1, a1 + c, m, m + c), [T1, S0]));
                elbows.push((fr.rect(a2, a2 + c, m, m + c), [S1, T0]));
                pieces.push(fr.piece(reg, (0, a1, 0, m + c), [(S0, Keep), (S1, PieceSide::Street), (T0, Keep), (T1, Back)]));
                pieces.push(fr.piece(reg, (a1 + c, ll, 0, m), [(S0, PieceSide::Street), (S1, Keep), (T0, Keep), (T1, Back)]));
                pieces.push(fr.piece(reg, (0, a2, m + c, ls), [(S0, Keep), (S1, PieceSide::Street), (T0, Back), (T1, Keep)]));
                pieces.push(fr.piece(reg, (a2 + c, ll, m, ls), [(S0, PieceSide::Street), (S1, Keep), (T0, Back), (T1, Keep)]));
            }
            Bend::L => {
                // In from the street at S0 along the region, then turning to T0 (a street, or the
                // forest, where it stops).
                if street(S0).is_none() || blind(T0) || !(60..=100).contains(&ls) || ll < 110 {
                    return None;
                }
                let m = pick(&mut self.rng, ROW_MIN, ls - c - ROW_MIN)?;
                let a = pick(&mut self.rng, 40, (ll - c - 40).min(70))?;
                streets.push((fr.rect(-street(S0).unwrap_or(0), a + c, m, m + c), true));
                streets.push((fr.rect(a, a + c, -street(T0).unwrap_or(0), m + c), false));
                elbows.push((fr.rect(a, a + c, m, m + c), [S1, T1]));
                pieces.push(fr.piece(reg, (0, a, 0, m), [(S0, Keep), (S1, PieceSide::Street), (T0, Keep), (T1, PieceSide::Street)]));
                pieces.push(fr.piece(reg, (0, a + c, m + c, ls), [(S0, Keep), (S1, Back), (T0, PieceSide::Street), (T1, Keep)]));
                pieces.push(fr.piece(reg, (a + c, ll, 0, ls), [(S0, Back), (S1, Keep), (T0, Keep), (T1, Keep)]));
            }
            Bend::Crescent => {
                // In from the street at T0, round a block and back out to it.
                if street(T0).is_none() || !(60..=96).contains(&ls) || ll < 140 {
                    return None;
                }
                let m = pick(&mut self.rng, ROW_MIN, (ls - c - ROW_MIN).min(50))?;
                let a1 = pick(&mut self.rng, 34, ll - 2 * c - 44 - 34)?;
                let a2 = pick(&mut self.rng, a1 + c + 44, (a1 + c + 64).min(ll - c - 34))?;
                let ext = street(T0).unwrap_or(0);
                streets.push((fr.rect(a1, a1 + c, -ext, m + c), false));
                streets.push((fr.rect(a1, a2 + c, m, m + c), true));
                streets.push((fr.rect(a2, a2 + c, -ext, m + c), false));
                elbows.push((fr.rect(a1, a1 + c, m, m + c), [T1, S0]));
                elbows.push((fr.rect(a2, a2 + c, m, m + c), [T1, S1]));
                pieces.push(fr.piece(reg, (a1 + c, a2, 0, m), [(S0, PieceSide::Street), (S1, PieceSide::Street), (T0, Keep), (T1, PieceSide::Street)]));
                pieces.push(fr.piece(reg, (0, a1, 0, ls), [(S0, Keep), (S1, Back), (T0, Keep), (T1, Keep)]));
                pieces.push(fr.piece(reg, (a1, a2 + c, m + c, ls), [(S0, Back), (S1, Back), (T0, PieceSide::Street), (T1, Keep)]));
                pieces.push(fr.piece(reg, (a2 + c, ll, 0, ls), [(S0, Back), (S1, Keep), (T0, Keep), (T1, Keep)]));
            }
        }
        // Every piece big enough for lots, with a street along it to face (a short piece can make
        // do with one across an end).
        let faces = |p: &Region| {
            let long_x = p.r.size.x >= p.r.size.y;
            let along = if long_x { [Side::N, Side::S] } else { [Side::W, Side::E] };
            p.r.size.max_element() <= 60 || along.iter().any(|&s| p.street(s).is_some())
        };
        if pieces.iter().any(|p| p.r.size.min_element() < ROW_MIN || !faces(p) || Side::ALL.iter().all(|&s| p.street(s).is_none())) {
            return None;
        }
        // The legs are side streets; a leg that ends at the forest (only at T0 or T1) stops there.
        for (r, along_s) in streets {
            // A leg is open at an end that reaches the edge of the region where the forest is.
            let (lo, hi) = if along_s == fr.long_x { (Side::W, Side::E) } else { (Side::N, Side::S) };
            let reaches = |side: Side| match side {
                Side::N => r.min.y <= reg.r.min.y,
                Side::S => r.max().y >= reg.r.max().y,
                Side::W => r.min.x <= reg.r.min.x,
                Side::E => r.max().x >= reg.r.max().x,
            };
            let open = [lo, hi].map(|side| reaches(side) && reg.street(side).is_none() && reg.edge[si(side)]);
            self.add_street(Street { r, horizontal: along_s == fr.long_x, road: SIDE_ROAD, sidewalk: SIDEWALK, line: Line::Dashed, open });
        }
        self.elbows.extend(elbows.into_iter().map(|(r, outer)| (r, outer.map(|f| fr.side(f)))));
        self.plan.bends += 1;
        Some(pieces)
    }

    /// Where to put a splitting street (offset from the region's low side), towards the middle:
    /// straight on from a street that meets the region's edge nearby, or clearly offset from it.
    fn split_at(&mut self, reg: &Region, vertical: bool, min_at: i32, max_at: i32) -> i32 {
        let n = (max_at - min_at + 1) as u32;
        let (start, span) = if vertical { (reg.r.min.x, (reg.r.min.y - 30, reg.r.max().y + 30)) } else { (reg.r.min.y, (reg.r.min.x - 30, reg.r.max().x + 30)) };
        let mut at = min_at;
        for _ in 0..10 {
            at = min_at + ((self.rng.below(n) + self.rng.below(n)) / 2) as i32;
            let mut ok = true;
            for s in &self.streets {
                if s.horizontal == vertical {
                    continue;
                }
                let (s_at, s_lo, s_hi) = if vertical { (s.r.min.x, s.r.min.y, s.r.max().y) } else { (s.r.min.y, s.r.min.x, s.r.max().x) };
                if s_hi < span.0 || s_lo > span.1 {
                    continue;
                }
                let d = start + at - s_at;
                // Now and then straight on across the street (a crossing), else clearly offset
                // (a T): most junctions are Ts.
                if d != 0 && d.abs() < 7 && (min_at..=max_at).contains(&(at - d)) && self.rng.f32() < 0.35 {
                    at -= d;
                    break;
                } else if d.abs() < 16 {
                    ok = false;
                    break;
                }
            }
            if ok {
                break;
            }
        }
        at
    }

    /// Pick blocks to be cul-de-sacs: some of those big enough, at least one in town if any is,
    /// at most three. Returns (block, the side its street leaves from).
    fn choose_cul_de_sacs(&mut self, leaves: &[Region]) -> Vec<(usize, Side)> {
        let mut best: Option<(i32, usize, Side)> = None;
        let mut out = Vec::new();
        for (i, leaf) in leaves.iter().enumerate() {
            let mut pick: Option<(i32, Side)> = None;
            for s in Side::ALL {
                let Some(walk) = leaf.street(s) else { continue };
                let lot = Lot { r: leaf.r, front: s };
                let (w, d) = (lot.frontage(), lot.depth());
                if w < CUL_WIDTH.0 || w > CUL_WIDTH.1 || d < CUL_DEPTH.0 || d > CUL_DEPTH.1 {
                    continue;
                }
                // Deep and not too wide is best; never off the main street if there's another way in.
                let score = d - (w - 64).abs() - if walk == MAIN_SIDEWALK { 100 } else { 0 };
                if pick.is_none_or(|p| score > p.0) {
                    pick = Some((score, s));
                }
            }
            let Some((score, side)) = pick else { continue };
            if best.is_none_or(|b| score > b.0) {
                best = Some((score, i, side));
            }
            if out.len() < 3 && self.rng.f32() < 0.4 {
                out.push((i, side));
            }
        }
        if out.is_empty()
            && let Some((_, i, side)) = best
        {
            out.push((i, side));
        }
        out
    }

    /// A block of the core along the main street.
    fn core_block(&mut self, r: Rect, u: CoreUse, is_north: bool) {
        let toward_main = if is_north { Side::S } else { Side::N };
        let (district, fill) = match u {
            CoreUse::Commercial => (District::Commercial, BlockUse::Shops),
            CoreUse::Square => (District::Civic, BlockUse::Square),
            _ => (District::Civic, BlockUse::Landmarks),
        };
        self.plan.blocks.push(PlanBlock { min: r.min, max: r.max(), district, fill });
        match u {
            CoreUse::Square => self.square(r),
            CoreUse::Landmark(front) => self.landmark(Lot { r, front }),
            CoreUse::LandmarkPair => {
                let a = r.size.x / 2 + self.rng.below(9) as i32 - 4;
                self.landmark(Lot { r: Rect::new(r.min, IVec2::new(a, r.size.y)), front: toward_main });
                self.landmark(Lot { r: Rect::new(r.min + IVec2::new(a, 0), IVec2::new(r.size.x - a, r.size.y)), front: toward_main });
            }
            CoreUse::Commercial => self.commercial_block(r, is_north),
        }
    }

    /// Shops and offices facing the main street, maybe an alley behind them, and a row of houses
    /// facing the back street. With no shop or office templates, houses both sides.
    fn commercial_block(&mut self, r: Rect, is_north: bool) {
        let (toward_main, toward_back) = if is_north { (Side::S, Side::N) } else { (Side::N, Side::S) };
        let block = Lot { r, front: toward_main };
        let depth = block.depth();
        if self.shops.is_empty() {
            let split = depth / 2;
            let front = Lot { r: block.sub(0, block.frontage(), 0, split), front: toward_main };
            let back = Lot { r: block.sub(0, block.frontage(), split, depth), front: toward_back };
            self.row(front, false, false);
            self.row(back, false, false);
            self.fence_backs(front, 0.6);
            return;
        }
        let deepest = self.shops.iter().map(|t| t.size().y).max().unwrap_or(0);
        let shop_depth = (deepest + 2 + SHOP_BACK).clamp(14, (depth - MIN_HOUSE_ROW).max(14));
        let alley = depth - shop_depth - ALLEY >= MIN_HOUSE_ROW + 2 && self.rng.f32() < 0.5;
        let shops = Lot { r: block.sub(0, block.frontage(), 0, shop_depth), front: toward_main };
        self.shop_row(shops.r, toward_main == Side::N);
        let behind = if alley {
            let a = block.sub(0, block.frontage(), shop_depth, shop_depth + ALLEY);
            for t in a.tiles() {
                set_floor(&mut self.map, t, Floor::Asphalt);
            }
            self.plan.alleys += 1;
            shop_depth + ALLEY
        } else {
            shop_depth
        };
        let houses = Lot { r: block.sub(0, block.frontage(), behind, depth), front: toward_back };
        self.row(houses, false, false);
        // A row of shops is always fenced off from the yards behind it (the alley stays open to the shops).
        self.fence_backs(houses, 1.0);
    }

    /// The town square: a paved square in the middle with paths to it from each side and a path
    /// round the edge, a ring of trees, bushes about.
    fn square(&mut self, r: Rect) {
        self.plan.square = Some((r.min, r.max()));
        let c = r.min + r.size / 2;
        let half = (r.size / 6).max(IVec2::splat(4));
        for t in r.tiles() {
            let d = t - c;
            let inset = (t - r.min).min(r.max() - IVec2::ONE - t).min_element();
            let paved = d.x >= -half.x && d.x < half.x && d.y >= -half.y && d.y < half.y;
            let cross = d.x == -1 || d.x == 0 || d.y == -1 || d.y == 0;
            if paved {
                set_floor(&mut self.map, t, Floor::Sidewalk);
            } else if cross || inset == 1 {
                set_floor(&mut self.map, t, Floor::Path);
            }
        }
        for t in r.tiles() {
            let local = t - r.min;
            let inset = local.min(r.max() - IVec2::ONE - t).min_element();
            let d = (t - c).abs();
            let ring = inset == 3 && (local.x + local.y) % 4 == 0;
            let corner_bush = d.x == half.x + 1 && d.y == half.y + 1;
            let tile = self.map.tile(t);
            if tile.floor != Floor::Grass || tile.feature != Feature::None {
                continue;
            }
            let feature = if ring {
                Feature::Tree
            } else if corner_bush || (inset > 4 && (d.x > half.x + 2 || d.y > half.y + 2) && self.rng.f32() < 0.02) {
                Feature::Bush
            } else {
                continue;
            };
            if let Some(tile) = self.map.tile_mut(t) {
                tile.feature = feature;
            }
        }
    }

    /// A landmark lot: the biggest civic building that fits (or a shop or office too big for the
    /// main street); with none, a green, a parking lot, or the biggest office that fits.
    fn landmark(&mut self, lot: Lot) {
        self.plan.landmarks.push((lot.r.min, lot.r.max()));
        let (f, d) = (lot.frontage(), lot.depth());
        let fits = |t: &&BuildingTemplate| {
            let (a, b) = lot.dims(t);
            a + 4 <= f && b + 6 <= d
        };
        fn biggest<'t>(list: &[&'t BuildingTemplate], fits: impl Fn(&&BuildingTemplate) -> bool) -> Option<&'t BuildingTemplate> {
            list.iter().copied().filter(|t| fits(t)).max_by_key(|t| area(t))
        }
        if let Some(t) = biggest(&self.landmarks.clone(), fits) {
            // The biggest, or now and then another that fits nearly as well.
            let big = area(t);
            let near: Vec<&BuildingTemplate> = self.landmarks.iter().copied().filter(|o| fits(o) && area(o) * 10 >= big * 7).collect();
            let t = near[self.rng.below(near.len() as u32) as usize];
            self.landmark_building(lot, t);
            return;
        }
        let office = biggest(&self.offices.clone(), fits);
        let mut pick = if office.is_some() && self.rng.f32() < 0.5 { 0 } else if self.rng.f32() < 0.55 { 1 } else { 2 };
        if Some(pick) == self.last_fallback {
            pick = (pick + 1 + self.rng.below(2) as u8) % 3;
        }
        self.last_fallback = Some(pick);
        match (pick, office) {
            (0, Some(t)) => self.landmark_building(lot, t),
            (2, _) => {
                // A parking lot at the front, a strip of green behind it.
                let depth = (d * 3 / 5).max(16).min(d);
                self.parking_lot(Lot { r: lot.sub(0, f, 0, depth), front: lot.front });
                self.scatter_back(lot, depth + 2, 0.08);
            }
            _ => self.park(lot),
        }
    }

    /// A big building on a landmark lot, a paved forecourt up to its front, parking beside it if
    /// there's room, trees behind.
    fn landmark_building(&mut self, lot: Lot, t: &BuildingTemplate) {
        let (f, d) = (lot.frontage(), lot.depth());
        let (along, deep) = lot.dims(t);
        let setback = 3 + self.rng.below(2) as i32;
        let spare = f - along - 4;
        // Off to one side with a parking lot in the rest, or in the middle.
        let a = if spare >= 14 {
            if self.rng.f32() < 0.5 { 2 } else { f - along - 2 }
        } else {
            2 + spare / 2
        };
        let r = lot.sub(a, a + along, setback, setback + deep);
        if !self.area_free(r.grow(1)) {
            return;
        }
        let mirror = self.rng.f32() < 0.5;
        let siding = SHOP_SIDING[self.rng.below(SHOP_SIDING.len() as u32) as usize];
        let roof = self.rng.below(5) as u8;
        self.place(t, Placement { origin: r.min, quarter_turns: lot.turns(), mirror }, siding, roof, 0.2);
        for p in lot.sub(a, a + along, 0, setback).tiles() {
            set_floor(&mut self.map, p, Floor::Sidewalk);
        }
        if spare >= 14 {
            let (p0, p1) = if a == 2 { (a + along + 2, f) } else { (0, a - 2) };
            self.parking_lot(Lot { r: lot.sub(p0, p1, 0, (deep + setback + 2).min(d)), front: lot.front });
        }
        self.scatter_back(lot, setback + deep + 2, 0.05);
    }

    /// Trees and bushes on the part of a lot from `from` back from the front to its back.
    fn scatter_back(&mut self, lot: Lot, from: i32, density: f32) {
        let seed = self.rng.next_u64();
        for t in lot.r.tiles() {
            let (a, b) = lot.local(t);
            if b < from || a == 0 || a + 1 == lot.frontage() || b + 1 == lot.depth() {
                continue;
            }
            if clear_around(&self.map, t, 1) && hash2(seed, t.x, t.y) < density {
                let tree = hash2(seed ^ 1, t.x, t.y) < 0.7;
                if let Some(tile) = self.map.tile_mut(t) {
                    tile.feature = if tree { Feature::Tree } else { Feature::Bush };
                }
            }
        }
    }

    /// A block of house lots in rows facing the streets round it: two rows back to back where
    /// streets run along both long sides (or both short ones, when that makes better lots), else
    /// one row; rows backing onto the forest have bigger, sparser lots. Returns its district.
    fn rows_block(&mut self, leaf: &Region, park: bool) -> District {
        let r = leaf.r;
        let mut options: Vec<Vec<Lot>> = Vec::new();
        for (a, b) in [(Side::N, Side::S), (Side::W, Side::E)] {
            let whole = Lot { r, front: a };
            let depth = whole.depth();
            if leaf.street(a).is_some() && leaf.street(b).is_some() {
                let split = depth / 2 + self.rng.below(5) as i32 - 2;
                let first = Lot { r: whole.sub(0, whole.frontage(), 0, split), front: a };
                let second = Lot { r: whole.sub(0, whole.frontage(), split, depth), front: b };
                options.push(vec![first, second]);
            }
            for s in [a, b] {
                if leaf.street(s).is_some() {
                    options.push(vec![Lot { r, front: s }]);
                }
            }
        }
        let score = |rows: &Vec<Lot>| {
            let depth = rows[0].depth().min(rows[rows.len() - 1].depth());
            let frontage: i32 = rows.iter().map(|l| l.frontage()).sum();
            rows.len() as i32 * 100 - (depth - 32).max(0) * 3 - (22 - depth).max(0) * 20 + frontage / 2
        };
        let Some(mut rows) = options.into_iter().max_by_key(score) else {
            // No street at all (never from the generator's own splits): leave it wild.
            self.vacant_lot(r);
            return District::Outer;
        };
        // Corner lots: where a street runs past the end of the rows and they're long enough, the
        // last lots face that street instead.
        let (first_end, last_end) = if matches!(rows[0].front, Side::N | Side::S) { (Side::W, Side::E) } else { (Side::N, Side::S) };
        let mut ends = Vec::new();
        for (end, at_start) in [(first_end, true), (last_end, false)] {
            let f = rows[0].frontage();
            if leaf.street(end).is_none() || f < 56 || (ends.len() == 1 && f < 90) || self.rng.f32() < 0.3 {
                continue;
            }
            let d = 20 + self.rng.below(5) as i32;
            let side = Lot { r, front: end };
            ends.push(Lot { r: side.sub(0, side.frontage(), 0, d), front: end });
            for row in &mut rows {
                let (f, depth) = (row.frontage(), row.depth());
                row.r = if at_start { row.sub(d, f, 0, depth) } else { row.sub(0, f - d, 0, depth) };
            }
        }
        for end in ends {
            self.row(end, false, false);
            self.fence_backs(end, 0.5);
        }
        let mut district = District::Residential;
        for (i, row) in rows.iter().enumerate() {
            let outer = leaf.edge[si(row.front.opposite())];
            if outer {
                district = District::Outer;
            }
            self.row(*row, outer, park && i == 0);
            if rows.len() == 2 && i == 0 {
                self.fence_backs(*row, 0.6);
            } else if outer {
                self.fence_backs(*row, 0.3);
            }
        }
        district
    }

    /// A cul-de-sac: a short street from `entry` into the block ending in a turning circle, with
    /// lots facing it down both sides and one at the end.
    fn cul_de_sac(&mut self, leaf: &Region, entry: Side) {
        let frame = Lot { r: leaf.r, front: entry };
        let (w, d) = (frame.frontage(), frame.depth());
        let uc = w / 2 + self.rng.below(5) as i32 - 2;
        let half = CUL_ROAD / 2 + SIDEWALK;
        let bulb = d - CUL_END_LOT - BULB_OUTER.ceil() as i32;
        let ext = leaf.street(entry).unwrap_or(0);
        let stem = frame.sub(uc - half, uc + half, -ext, bulb);
        paint_street(&mut self.map, &Street { r: stem, horizontal: matches!(entry, Side::E | Side::W), road: CUL_ROAD, sidewalk: SIDEWALK, line: Line::None, open: [false; 2] });
        let end = frame.sub(uc - half, uc + half, bulb - half, bulb + half);
        let c = end.min.as_vec2() + end.size.as_vec2() * 0.5;
        for t in Rect::new(c.floor().as_ivec2() - IVec2::splat(12), IVec2::splat(24)).tiles() {
            if !(t.cmpge(leaf.r.min).all() && t.cmplt(leaf.r.max()).all()) {
                continue;
            }
            let dist = (t.as_vec2() + Vec2::splat(0.5)).distance(c);
            if dist <= BULB_RADIUS {
                set_floor(&mut self.map, t, Floor::Asphalt);
            } else if dist <= BULB_OUTER {
                set_ground(&mut self.map, t, Floor::Sidewalk);
            }
        }
        self.plan.cul_de_sacs.push(c);
        let stem = CUL_ROAD as f32 / 2.0;
        self.map.turning_circles.push(TurningCircle { centre: c, road: BULB_RADIUS, walk: BULB_OUTER, entry, stem, stem_walk: stem + SIDEWALK as f32 });
        let end_lot = Lot { r: frame.sub((uc - 14).max(0), (uc + 14).min(w), d - CUL_END_LOT, d), front: entry };
        if let Some(b) = self.place_house(end_lot, false) {
            self.homes.push(b);
        }
        self.yard(end_lot, false);
        let toward = frame.along_side();
        self.row(Lot { r: frame.sub(0, uc - half, 0, d), front: toward }, false, false);
        self.row(Lot { r: frame.sub(uc + half, w, 0, d), front: toward.opposite() }, false, false);
    }

    /// A row of house lots facing its street: mostly houses (a template that fits, at random),
    /// some left as yards or overgrown vacant lots, and maybe the town's pocket park. Lots backing
    /// onto the forest (`outer`) are wider, deeper-set and wilder.
    fn row(&mut self, row: Lot, outer: bool, park: bool) {
        let f = row.frontage();
        let (narrowest, spread) = if outer { (22, 9) } else { (14, 6) };
        let mut lots = Vec::new();
        let mut a = 0;
        while a < f {
            let remaining = f - a;
            let mut w = narrowest + self.rng.below(spread) as i32;
            if remaining - w < narrowest {
                // What's left: one lot, or two if it's wide enough for two.
                w = if remaining >= 2 * narrowest { remaining / 2 } else { remaining };
            }
            lots.push(Lot { r: row.sub(a, a + w, 0, row.depth()), front: row.front });
            a += w;
        }
        let park_lot = if park && !lots.is_empty() { Some(1.min(lots.len() - 1)) } else { None };
        // Inner blocks are nearly all built on; rows by the forest have more gaps.
        let (house_chance, vacant_chance) = if outer { (0.8, 0.1) } else { (0.95, 0.02) };
        for (i, lot) in lots.into_iter().enumerate() {
            if park_lot == Some(i) {
                self.park(lot);
                continue;
            }
            let roll = self.rng.f32();
            if roll < house_chance {
                if let Some(b) = self.place_house(lot, outer) {
                    self.homes.push(b);
                }
            } else if roll < house_chance + vacant_chance {
                self.vacant_lot(lot.r);
            }
            self.yard(lot, outer);
        }
    }

    /// A few trees and bushes in a yard; the back of a deep lot by the forest gets more.
    fn yard(&mut self, lot: Lot, outer: bool) {
        scatter_yard(&mut self.map, &mut self.rng, (lot.r.min, lot.r.size));
        if outer && lot.depth() > 30 {
            self.scatter_back(lot, lot.depth() - 12, 0.1);
        }
    }

    /// Fence the back of each lot-sized stretch of a row with chance `chance`, where it runs between
    /// two outdoor tiles.
    fn fence_backs(&mut self, row: Lot, chance: f32) {
        let back = row.front.opposite();
        let f = row.frontage();
        let mut a = 0;
        while a < f {
            let len = (16 + self.rng.below(7) as i32).min(f - a);
            let fence = self.rng.f32() < chance;
            for i in a..a + len {
                let t = if matches!(row.front, Side::N | Side::S) {
                    IVec2::new(row.r.min.x + i, if back == Side::N { row.r.min.y } else { row.r.max().y - 1 })
                } else {
                    IVec2::new(if back == Side::W { row.r.min.x } else { row.r.max().x - 1 }, row.r.min.y + i)
                };
                if fence {
                    self.fence_edge(t, back);
                }
            }
            a += len;
        }
    }

    /// A fence on side `s` of tile `t`, if both sides are open ground (not a building, a street or a path).
    fn fence_edge(&mut self, t: IVec2, s: Side) {
        let o = t + s.offset();
        let open = |p: IVec2| {
            let tile = self.map.tile(p);
            self.map.in_bounds(p) && tile.room == 0 && matches!(tile.floor, Floor::Grass | Floor::Dirt)
        };
        if open(t) && open(o) && self.map.edge(t, s) == Edge::None {
            self.map.set_edge(t, s, Edge::Fence);
        }
    }

    /// A row of shops and offices facing the main street, packed along it with alleys between,
    /// paved from the sidewalk to their fronts; now and then a parking lot, and a parking or vacant
    /// lot where nothing else fits.
    fn shop_row(&mut self, row: Rect, faces_north: bool) {
        let turns = if faces_north { 2 } else { 0 };
        let front = if faces_north { Side::N } else { Side::S };
        let setback = 1 + self.rng.below(2) as i32;
        let paved = if faces_north { row.min.y } else { row.max().y - setback };
        for t in Rect::new(IVec2::new(row.min.x, paved), IVec2::new(row.size.x, setback)).tiles() {
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
                let r = if faces_north { Rect::new(IVec2::new(x, row.min.y), IVec2::new(w, row.size.y - 1)) } else { Rect::new(IVec2::new(x, row.min.y + 1), IVec2::new(w, row.size.y - 1)) };
                self.parking_lot(Lot { r, front });
                parked = true;
                x += w + 1;
                continue;
            }
            // Not the same as the last one when there's a choice.
            let choice: Vec<&BuildingTemplate> = if fits.len() > 1 { fits.iter().copied().filter(|t| Some(t.id.as_str()) != last).collect() } else { fits };
            // Shops twice as likely as offices: it's the main street.
            let weight = |t: &BuildingTemplate| if matches!(t.kind, BuildingKind::Shop) { 2 } else { 1 };
            let mut roll = self.rng.below(choice.iter().map(|t| weight(t)).sum::<u32>()) as i32;
            let t = choice
                .iter()
                .copied()
                .find(|t| {
                    roll -= weight(t) as i32;
                    roll < 0
                })
                .unwrap_or(choice[0]);
            let s = t.rotated_size(turns);
            let y = if faces_north { row.min.y + setback } else { row.max().y - setback - s.y };
            let mirror = self.rng.f32() < 0.5;
            let siding = SHOP_SIDING[self.rng.below(SHOP_SIDING.len() as u32) as usize];
            let roof = self.rng.below(5) as u8;
            let placed = self.place(t, Placement { origin: IVec2::new(x, y), quarter_turns: turns, mirror }, siding, roof, 0.15);
            let b = &self.map.buildings[placed.building];
            self.fronts.shops.push(ShopFront { min: b.min, max: b.min + b.size, front });
            last = Some(t.id.as_str());
            x += s.x + 1 + self.rng.below(3) as i32;
        }
        // A few bushes and trees out the back.
        let back = if faces_north { Rect::new(IVec2::new(row.min.x, row.max().y - SHOP_BACK), IVec2::new(row.size.x, SHOP_BACK)) } else { Rect::new(row.min, IVec2::new(row.size.x, SHOP_BACK)) };
        scatter_yard(&mut self.map, &mut self.rng, (back.min, back.size));
    }

    /// True if every tile of `r` is open ground: no building, street, path or tree.
    fn area_free(&self, r: Rect) -> bool {
        r.tiles().all(|t| {
            let tile = self.map.tile(t);
            self.map.in_bounds(t) && tile.room == 0 && matches!(tile.floor, Floor::Grass | Floor::Dirt) && tile.feature == Feature::None
        })
    }

    /// A house on a lot, set back from the street, with a garden path to its front door: a random
    /// one of the templates that fit (bigger ones more likely on big lots by the forest), set back
    /// further if something's in the way, else another. Returns its building index.
    fn place_house(&mut self, lot: Lot, outer: bool) -> Option<usize> {
        let (f, d) = (lot.frontage(), lot.depth());
        let mut fitting: Vec<&BuildingTemplate> = self
            .houses
            .iter()
            .copied()
            .filter(|t| {
                let (a, b) = lot.dims(t);
                a + 4 <= f && b + FRONT_SETBACK + 3 <= d
            })
            .collect();
        while !fitting.is_empty() {
            let i = if outer {
                // Weighted by footprint.
                let total: i32 = fitting.iter().map(|t| area(t)).sum();
                let mut roll = self.rng.below(total as u32) as i32;
                fitting.iter().position(|t| {
                    roll -= area(t);
                    roll < 0
                })
                .unwrap_or(0)
            } else {
                self.rng.below(fitting.len() as u32) as usize
            };
            let t = fitting.swap_remove(i);
            let (along, deep) = lot.dims(t);
            let slack = f - along - 4;
            let a = 2 + if slack > 0 { self.rng.below(slack as u32 + 1) as i32 } else { 0 };
            let first = FRONT_SETBACK + self.rng.below(2) as i32 + if outer { 1 + self.rng.below(4) as i32 } else { 0 };
            for setback in first.min(d - deep - 3)..=d - deep - 3 {
                let r = lot.sub(a, a + along, setback, setback + deep);
                if !self.area_free(r.grow(1)) {
                    continue;
                }
                let rng = &mut self.rng;
                let mirror = rng.f32() < 0.5;
                let (siding, roof) = (rng.below(8) as u8, rng.below(5) as u8);
                let placed = self.place(t, Placement { origin: r.min, quarter_turns: lot.turns(), mirror }, siding, roof, 0.25);
                // Garden path from each street-side door to the sidewalk.
                let map = &mut self.map;
                let mut path_end = None;
                for (door, side, _) in placed.exterior_doors {
                    if side != lot.front {
                        continue;
                    }
                    let mut p = door + side.offset();
                    for _ in 0..40 {
                        if !(map.in_bounds(p) && map.tile(p).floor == Floor::Grass && map.tile(p).room == 0) {
                            break;
                        }
                        set_floor(map, p, Floor::Path);
                        p += side.offset();
                    }
                    if path_end.is_none() && p != door + side.offset() {
                        path_end = Some(p - side.offset());
                    }
                }
                if let Some(path_end) = path_end {
                    let b = &self.map.buildings[placed.building];
                    self.fronts.houses.push(HouseFront { path_end, front: lot.front, min: b.min, max: b.min + b.size });
                }
                return Some(placed.building);
            }
        }
        None
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

    /// Asphalt with rows of parking bays (lines every 3 m) and aisles between, from the street side.
    fn parking_lot(&mut self, lot: Lot) {
        let depth = lot.depth();
        for t in lot.r.tiles() {
            let (along, from_street) = lot.local(t);
            let bay = if depth < 14 {
                from_street >= depth - 5
            } else {
                let k = from_street % 16;
                ((1..6).contains(&k) || (10..15).contains(&k)) && from_street < depth - 1 || from_street >= depth - 5
            };
            let line = bay && along % 3 == 0 && along > 0;
            set_floor(&mut self.map, t, if line { Floor::RoadLine } else { Floor::Asphalt });
        }
        // The bays, for what parks in them: two tiles between each pair of lines, five deep.
        let mut rows = Vec::new();
        if depth >= 14 {
            for base in (0..depth).step_by(16) {
                rows.extend([base + 1, base + 10].into_iter().filter(|r| r + 5 < depth - 5));
            }
        }
        rows.push(depth - 5);
        for d0 in rows {
            for m in 0..lot.frontage() / 3 {
                if 3 * m + 3 <= lot.frontage() {
                    let r = lot.sub(3 * m + 1, 3 * m + 3, d0, d0 + 5);
                    self.fronts.bays.push(street_props::Bay { min: r.min, size: r.size, front: lot.front });
                }
            }
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
            if clear_around(&self.map, p, 0)
                && let Some(t) = self.map.tile_mut(p)
            {
                t.feature = if self.rng.f32() < 0.8 { Feature::Bush } else { Feature::Tree };
            }
        }
    }

    /// A park: a path in from the street to the middle and one across, trees round the edge, a few
    /// bushes.
    fn park(&mut self, lot: Lot) {
        let (f, d) = (lot.frontage(), lot.depth());
        let (mid_a, mid_d) = (f / 2, d / 2);
        for t in lot.sub(mid_a, mid_a + 1, 0, mid_d + 1).tiles().chain(lot.sub(2, f - 2, mid_d, mid_d + 1).tiles()) {
            set_floor(&mut self.map, t, Floor::Path);
        }
        for t in lot.r.tiles() {
            let inset = (t - lot.r.min).min(lot.r.max() - IVec2::ONE - t);
            let ring = inset.min_element() == 2 && (inset.x + inset.y) % 3 == 0;
            let roll = self.rng.f32();
            if ((ring && roll < 0.8) || (!ring && inset.min_element() > 2 && roll < 0.03))
                && clear_around(&self.map, t, 1)
                && let Some(tile) = self.map.tile_mut(t)
            {
                tile.feature = if ring { Feature::Tree } else { Feature::Bush };
            }
        }
    }

    /// Where streets reach the edge of town: the main street runs on to both map edges, the street
    /// nearest the middle of town leaves north and one leaves south, and the rest stop at the
    /// forest (some with a dirt track on into the trees).
    fn roads_out(&mut self, size: IVec2, main_y: i32) {
        let (town_min, town_max) = (self.town_min, self.town_max);
        paint_road_only(&mut self.map, Rect::new(IVec2::new(0, main_y + MAIN_SIDEWALK), IVec2::new(town_min.x, MAIN_ROAD)), true, MAIN_ROAD);
        paint_road_only(&mut self.map, Rect::new(IVec2::new(town_max.x, main_y + MAIN_SIDEWALK), IVec2::new(size.x - town_max.x, MAIN_ROAD)), true, MAIN_ROAD);
        let mid = (town_min.x + town_max.x) / 2;
        // Each street end that stops at the forest, and which way it faces.
        let mut ends: Vec<(Street, Side)> = Vec::new();
        for s in &self.streets {
            let (lo, hi) = if s.horizontal { (Side::W, Side::E) } else { (Side::N, Side::S) };
            for (open, side) in s.open.into_iter().zip([lo, hi]) {
                if open {
                    ends.push((*s, side));
                }
            }
        }
        let nearest = |side: Side| ends.iter().filter(|e| e.1 == side).min_by_key(|e| (e.0.r.min.x + e.0.r.size.x / 2 - mid).abs()).map(|e| e.0);
        let (north, south) = (nearest(Side::N), nearest(Side::S));
        // The roads out run on through the forest, with a dog-leg a little way out of town.
        for (s, north) in [(north, true), (south, false)] {
            let Some(s) = s else { continue };
            let (x, w) = (s.r.min.x + s.sidewalk, s.road);
            let (from, edge) = if north { (s.r.min.y, town_min.y) } else { (s.r.max().y, town_max.y) };
            let out = (edge - from).abs() + 14 + self.rng.below(30) as i32;
            let jog = (20 + self.rng.below(40) as i32) * if self.rng.f32() < 0.5 { -1 } else { 1 };
            let x2 = (x + jog).clamp(4, size.x - w - 4);
            let bend = if north { from - out } else { from + out };
            if (north && bend - w < 8) || (!north && bend + w > size.y - 8) {
                let r = if north { Rect::new(IVec2::new(x, 0), IVec2::new(w, from)) } else { Rect::new(IVec2::new(x, from), IVec2::new(w, size.y - from)) };
                paint_road_only(&mut self.map, r, false, w);
                continue;
            }
            let (lo, hi) = (x.min(x2), x.max(x2) + w);
            if north {
                paint_road_only(&mut self.map, Rect::new(IVec2::new(x, bend), IVec2::new(w, from - bend)), false, w);
                paint_road_only(&mut self.map, Rect::new(IVec2::new(lo, bend - w), IVec2::new(hi - lo, w)), true, w);
                paint_road_only(&mut self.map, Rect::new(IVec2::new(x2, 0), IVec2::new(w, bend - w)), false, w);
            } else {
                paint_road_only(&mut self.map, Rect::new(IVec2::new(x, from), IVec2::new(w, bend - from)), false, w);
                paint_road_only(&mut self.map, Rect::new(IVec2::new(lo, bend), IVec2::new(hi - lo, w)), true, w);
                paint_road_only(&mut self.map, Rect::new(IVec2::new(x2, bend + w), IVec2::new(w, size.y - bend - w)), false, w);
            }
        }
        let is = |a: Option<Street>, b: &Street| a.is_some_and(|a| a.r == b.r);
        for (s, side) in ends {
            if (side == Side::N && is(north, &s)) || (side == Side::S && is(south, &s)) {
                continue;
            }
            self.plan.dead_ends += 1;
            if self.rng.f32() < 0.5 {
                continue;
            }
            // A dirt track a little way on into the trees.
            let len = 6 + self.rng.below(12) as i32;
            let (track, inset) = (s.road - 2, s.sidewalk + 1);
            let r = match side {
                Side::N => Rect::new(IVec2::new(s.r.min.x + inset, s.r.min.y - len), IVec2::new(track, len)),
                Side::S => Rect::new(IVec2::new(s.r.min.x + inset, s.r.max().y), IVec2::new(track, len)),
                Side::W => Rect::new(IVec2::new(s.r.min.x - len, s.r.min.y + inset), IVec2::new(len, track)),
                Side::E => Rect::new(IVec2::new(s.r.max().x, s.r.min.y + inset), IVec2::new(len, track)),
            };
            for t in r.tiles() {
                set_floor(&mut self.map, t, Floor::Dirt);
            }
        }
    }
}

/// A street's sidewalks and asphalt (the centre line comes after, see [`paint_lines`]). Asphalt
/// wins where streets meet.
fn paint_street(map: &mut TileMap, s: &Street) {
    for t in s.r.tiles() {
        let across = if s.horizontal { t.y - s.r.min.y } else { t.x - s.r.min.x };
        if across < s.sidewalk || across >= s.sidewalk + s.road {
            set_ground(map, t, Floor::Sidewalk);
        } else {
            set_floor(map, t, Floor::Asphalt);
        }
    }
}

/// Centre lines down the streets that have one (dashed, the main street's solid), stopping where
/// another street crosses or meets it.
fn paint_lines(map: &mut TileMap, streets: &[Street]) {
    for (i, s) in streets.iter().enumerate() {
        if s.line == Line::None {
            continue;
        }
        let across = s.sidewalk + s.road / 2;
        let (lo, hi) = if s.horizontal { (s.r.min.x, s.r.max().x) } else { (s.r.min.y, s.r.max().y) };
        for along in lo..hi {
            let t = if s.horizontal { IVec2::new(along, s.r.min.y + across) } else { IVec2::new(s.r.min.x + across, along) };
            if s.line == Line::Dashed && along.rem_euclid(6) >= 3 {
                continue;
            }
            let crossed = streets.iter().enumerate().any(|(j, o)| j != i && t.cmpge(o.r.min).all() && t.cmplt(o.r.max()).all());
            if !crossed && map.tile(t).floor == Floor::Asphalt {
                set_floor(map, t, Floor::RoadLine);
            }
        }
    }
}

/// The corner square where a bent street turns: the asphalt turns the corner and the sidewalk
/// runs on round the outside of it (`outer`), instead of both legs' asphalt reaching out to its
/// edges; its inside corner is sidewalk.
fn paint_elbow(map: &mut TileMap, r: Rect, outer: [Side; 2]) {
    for t in r.tiles() {
        // How far in from each outer side.
        let d = outer.map(|s| match s {
            Side::N => t.y - r.min.y,
            Side::S => r.max().y - 1 - t.y,
            Side::W => t.x - r.min.x,
            Side::E => r.max().x - 1 - t.x,
        });
        let road = SIDEWALK..SIDEWALK + SIDE_ROAD;
        let floor = if road.contains(&d[0]) && d[1] >= SIDEWALK || road.contains(&d[1]) && d[0] >= SIDEWALK { Floor::Asphalt } else { Floor::Sidewalk };
        set_floor(map, t, floor);
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

/// Set an outdoor tile's floor without touching asphalt that already exists (crossings).
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
    if let Some(tile) = map.tile_mut(t)
        && tile.room == 0
    {
        tile.floor = floor;
        tile.feature = Feature::None;
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

/// What the biomes grow on the wild ground (`crate::biomes`: everywhere outside town and on the
/// parts of town its blocks leave wild), on open grass: trees in their clumps, thinned on steep
/// hillsides, each keeping its biome's spacing; bushes; patches of bare earth. All by hashes of
/// the seed and the tile ([`BiomeMap::plant`](crate::biomes::BiomeMap::plant)), never the
/// generator's stream, so the town draws the same numbers whatever the biomes say.
fn plant_wild(map: &mut TileMap) {
    use crate::biomes::Plant;
    if map.biomes.is_empty() {
        return;
    }
    let flat = map.terrain.is_flat();
    for y in 0..map.height() {
        for x in 0..map.width() {
            let p = IVec2::new(x, y);
            let Some(biome) = map.biomes.at(p) else { continue };
            let spacing = biome.trees.spacing;
            if !clear_around(map, p, 1) {
                continue;
            }
            let grade = if flat { 0.0 } else { map.terrain.broad_grade(p.as_vec2() + Vec2::splat(0.5)) };
            match map.biomes.plant(p, grade) {
                Some(Plant::Tree(feature)) => {
                    if spacing <= 1 || !tree_within(map, p, spacing) {
                        if let Some(t) = map.tile_mut(p) {
                            t.feature = feature;
                        }
                    }
                }
                Some(Plant::Bush) => {
                    if let Some(t) = map.tile_mut(p) {
                        t.feature = Feature::Bush;
                    }
                }
                Some(Plant::Bare) => {
                    if let Some(t) = map.tile_mut(p) {
                        t.floor = Floor::Dirt;
                    }
                }
                None => {}
            }
        }
    }
}

/// Whether a tree stands within `r` tiles of `p` (itself not counted).
fn tree_within(map: &TileMap, p: IVec2, r: i32) -> bool {
    for dy in -r..=r {
        for dx in -r..=r {
            if (dx != 0 || dy != 0) && matches!(map.tile(p + IVec2::new(dx, dy)).feature, Feature::Tree | Feature::PineTree) {
                return true;
            }
        }
    }
    false
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
/// outdoors in town, the town core (the civic and commercial blocks along the main street, and the
/// street; `core_fraction` of each side's length along the street, round its middle), else roads,
/// parking lots and sidewalks are street and the rest yards; outside town, roads are forest roads
/// and the rest forest, edge within `edge_width` metres of the town, deep beyond.
pub fn zone_at(map: &TileMap, town: &Town, t: IVec2, core_fraction: f32, edge_width: f32) -> Zone {
    let tile = map.tile(t);
    if tile.room != 0 {
        return Zone::Indoors;
    }
    let (town_min, town_max) = (town.min, town.max);
    let in_town = t.x >= town_min.x && t.y >= town_min.y && t.x < town_max.x && t.y < town_max.y;
    let road = tile.floor.is_road();
    if in_town {
        let x = t.x as f32 + 0.5;
        let in_core = town.core.iter().any(|&(lo, hi)| {
            let centre = (lo.x + hi.x) as f32 * 0.5;
            let half = (hi.x - lo.x) as f32 * core_fraction * 0.5;
            (x - centre).abs() <= half && t.y >= lo.y && t.y < hi.y
        });
        if in_core {
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

    /// Two shipped houses.
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

    /// A big plain building of `kind`, `w` × `h`: a hall with a lobby across the front.
    fn big(id: &str, kind: &str, w: usize, h: usize) -> BuildingTemplate {
        let mut rows: Vec<String> = (0..h - 6).map(|_| format!("\"{}\"", "M".repeat(w))).collect();
        rows.extend((0..6).map(|_| format!("\"{}\"", "L".repeat(w))));
        let text = format!(
            r#"BuildingTemplate(id: "{id}", name: "{id}", kind: {kind}, roof: Flat,
                rooms: {{ 'M': (name: "Hall", room_type: "hall", floor: Wood), 'L': (name: "Lobby", room_type: "hall", floor: TileFloor) }},
                levels: [(layout: [{}], doors: [({}, {}, S), ({}, {}, N)], windows: [(2, {}, S), ({}, {}, S)])])"#,
            rows.join(", "),
            w / 2,
            h - 1,
            w / 2,
            h - 6,
            h - 1,
            w - 3,
            h - 1
        );
        BuildingTemplate::from_ron(&text).unwrap()
    }

    /// A town hall about as big as a landmark lot takes (test-only: there are no civic templates yet).
    pub fn test_town_hall() -> BuildingTemplate {
        big("test_town_hall", "Civic", 36, 34)
    }

    /// The shipped houses plus the test shops and office.
    pub fn with_shops() -> Vec<BuildingTemplate> {
        let mut t = templates();
        t.extend(TEST_SHOPS.iter().map(|s| BuildingTemplate::from_ron(s).unwrap()));
        t
    }

    fn kind_of<'a>(templates: &'a [BuildingTemplate], b: &crate::map::Building) -> &'a BuildingKind {
        &templates.iter().find(|t| t.id == b.template).unwrap().kind
    }

    fn inside(b: &crate::map::Building, (lo, hi): (IVec2, IVec2)) -> bool {
        b.min.cmpge(lo).all() && (b.min + b.size).cmple(hi).all()
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

    /// Buildings never overlap, and there's open ground between any two.
    fn assert_apart(g: &Generated, what: &str) {
        let b = &g.map.buildings;
        for i in 0..b.len() {
            let (lo, hi) = (b[i].min, b[i].min + b[i].size);
            for (j, o) in b.iter().enumerate().skip(i + 1) {
                let apart = hi.x < o.min.x || o.min.x + o.size.x < lo.x || hi.y < o.min.y || o.min.y + o.size.y < lo.y;
                assert!(apart, "{what}: buildings {i} ({}) and {j} ({}) touch or overlap", b[i].template, o.template);
            }
        }
    }

    /// Outdoor tiles you can walk to from the main street (fences, trees and walls in the way).
    fn reachable_outdoors(g: &Generated) -> Vec<bool> {
        let m = &g.map;
        let w = m.width();
        let mut seen = vec![false; (w * m.height()) as usize];
        let start = g.plan.streets[0].0 + IVec2::new(1, MAIN_SIDEWALK + 1);
        let mut stack = vec![start];
        seen[(start.y * w + start.x) as usize] = true;
        while let Some(p) = stack.pop() {
            for s in Side::ALL {
                let q = p + s.offset();
                if !m.in_bounds(q) || m.tile(q).room != 0 || !m.can_step(p, s) || seen[(q.y * w + q.x) as usize] {
                    continue;
                }
                seen[(q.y * w + q.x) as usize] = true;
                stack.push(q);
            }
        }
        seen
    }

    #[test]
    fn town_is_deterministic_and_populated() {
        for templates in [templates(), with_shops()] {
            let a = generate(&GenConfig::default(), &templates);
            let b = generate(&GenConfig::default(), &templates);
            assert_eq!(map_hash(&a.map), map_hash(&b.map));
            assert_eq!(a.player_spawn, b.player_spawn);
            assert!(a.map.buildings.len() >= 70, "only {} buildings", a.map.buildings.len());
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

    /// Junctions between street corridors: (T-junctions, crossings, corners). A street that ends
    /// against another is a T; two that meet it in line from opposite sides, or one that carries
    /// on through it, make a crossing; two that both end where they meet are a corner (an L).
    fn junctions(plan: &Plan) -> (usize, usize, usize) {
        let streets = &plan.streets;
        let horizontal = |(lo, hi): (IVec2, IVec2)| hi.x - lo.x > hi.y - lo.y;
        let (mut crossings, mut corners) = (0, 0);
        // T-junctions: (the street met, which side of it, where along it).
        let mut tees: Vec<(usize, bool, i32)> = Vec::new();
        for i in 0..streets.len() {
            for j in i + 1..streets.len() {
                let (a, b) = (streets[i], streets[j]);
                if horizontal(a) == horizontal(b) || !(a.0.cmplt(b.1).all() && b.0.cmplt(a.1).all()) {
                    continue;
                }
                let ((h, hi), (v, vi)) = if horizontal(a) { ((a, i), (b, j)) } else { ((b, j), (a, i)) };
                let v_through = v.0.y < h.0.y && v.1.y > h.1.y;
                let h_through = h.0.x < v.0.x && h.1.x > v.1.x;
                match (h_through, v_through) {
                    (true, true) => crossings += 1,
                    (true, false) => tees.push((hi, v.0.y < h.0.y, v.0.x)),
                    (false, true) => tees.push((vi, h.0.x < v.0.x, h.0.y)),
                    (false, false) => corners += 1,
                }
            }
        }
        // Two T-junctions in line from opposite sides of one street are a crossing.
        let mut used = vec![false; tees.len()];
        for k in 0..tees.len() {
            if used[k] {
                continue;
            }
            let (s, side, at) = tees[k];
            if let Some(m) = (k + 1..tees.len()).find(|&m| !used[m] && tees[m].0 == s && tees[m].1 != side && (tees[m].2 - at).abs() <= 2) {
                used[k] = true;
                used[m] = true;
                crossings += 1;
            }
        }
        (used.iter().filter(|u| !**u).count(), crossings, corners)
    }

    /// The town has every district, blocks of different sizes in each, mostly T-junctions, streets
    /// that stop at the forest, a cul-de-sac with houses round it, alleys behind some shops, and a
    /// civic square with landmark lots round it big enough for a 40 × 40 m building.
    #[test]
    fn districts_and_street_shapes() {
        let all = crate::template::tests::shipped_templates();
        let mut alleys = 0;
        let (mut bends, mut steps) = (0, 0);
        let (mut landmarks, mut squares) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
        // Whether the core blocks on one side of the main street are of more than one depth.
        let stepped = |g: &Generated| {
            let main_y = g.plan.streets[0].0.y;
            [true, false].iter().any(|&north| {
                let depths: std::collections::BTreeSet<i32> = g.plan.blocks.iter().filter(|b| matches!(b.district, District::Civic | District::Commercial) && (b.max.y <= main_y) == north).map(|b| (b.max - b.min).y).collect();
                depths.len() > 1
            })
        };
        for seed in 1..=8 {
            let g = generate(&GenConfig { seed, ..Default::default() }, &all);
            let plan = &g.plan;
            let count = |d: District| plan.blocks.iter().filter(|b| b.district == d).count();
            let (civic, commercial, residential, outer) = (count(District::Civic), count(District::Commercial), count(District::Residential), count(District::Outer));
            assert!(civic >= 2 && commercial >= 4 && residential >= 3 && outer >= 3, "seed {seed}: districts {civic} {commercial} {residential} {outer}");
            // Block sizes vary: residential blocks come in many sizes, and the civic district's are
            // bigger than a typical house block is deep.
            let sizes: std::collections::BTreeSet<(i32, i32)> = plan.blocks.iter().filter(|b| b.fill == BlockUse::Rows).map(|b| ((b.max - b.min).x, (b.max - b.min).y)).collect();
            assert!(sizes.len() >= 6, "seed {seed}: only {} block sizes", sizes.len());
            let (tees, crossings, corners) = junctions(plan);
            assert!(tees >= 10 && tees > crossings && crossings >= 1, "seed {seed}: {tees} T-junctions, {crossings} crossings");
            (bends, steps) = (bends + plan.bends, steps + stepped(&g) as usize);
            landmarks.insert(plan.landmarks.len());
            squares.insert((plan.square.unwrap().0.x + plan.square.unwrap().1.x) / 2 - (g.town_min.x + g.town_max.x) / 2);
            assert!(plan.dead_ends >= 3, "seed {seed}: {} streets stop at the forest", plan.dead_ends);
            // A cul-de-sac with houses round its turning circle.
            assert!(!plan.cul_de_sacs.is_empty(), "seed {seed}: no cul-de-sac");
            for &c in &plan.cul_de_sacs {
                let round = g.map.buildings.iter().filter(|b| (b.min.as_vec2() + b.size.as_vec2() * 0.5).distance(c) < 30.0).count();
                assert!(round >= 3, "seed {seed}: only {round} houses round the cul-de-sac at {c}");
                assert!(g.map.tile(c.floor().as_ivec2()).floor.is_road(), "seed {seed}: no turning circle at {c}");
            }
            // The square, and landmark lots that can take 40 × 40 m.
            let (sq_lo, sq_hi) = plan.square.expect("a town square");
            assert!((sq_hi - sq_lo).min_element() >= 40, "seed {seed}: square {}", sq_hi - sq_lo);
            assert!((1..=4).contains(&plan.landmarks.len()), "seed {seed}: {} landmark lots", plan.landmarks.len());
            assert!(plan.landmarks.iter().any(|(lo, hi)| (*hi - *lo).min_element() >= 42), "seed {seed}: no landmark lot of 40 m");
            alleys += plan.alleys;
            // The core is the civic and commercial blocks.
            for b in plan.blocks.iter().filter(|b| matches!(b.district, District::Civic | District::Commercial)) {
                assert!(g.core.iter().any(|&(lo, hi)| b.min.cmpge(lo).all() && b.max.cmple(hi).all()), "seed {seed}: {:?} block outside the core", b.district);
            }
            eprintln!(
                "seed {seed}: town {}, {} buildings; blocks civic {civic}, commercial {commercial}, residential {residential}, outer {outer}; {} sizes; {tees} T, {crossings} +, {corners} L; {} bends; {} landmark lots; {} dead ends; {} cul-de-sacs; {} alleys",
                g.town_max - g.town_min,
                g.map.buildings.len(),
                sizes.len(),
                plan.bends,
                plan.landmarks.len(),
                plan.dead_ends,
                plan.cul_de_sacs.len(),
                plan.alleys
            );
        }
        assert!(alleys >= 2, "only {alleys} alleys in eight towns");
        // #80: bent streets, back streets that step, landmark lots and the square varying.
        assert!(bends >= 8, "only {bends} bent streets in eight towns");
        assert!(steps >= 3, "back streets step in only {steps} of eight towns");
        assert!(landmarks.len() >= 2, "always {landmarks:?} landmark lots");
        assert!(squares.iter().max().unwrap() - squares.iter().min().unwrap() >= 60, "the square always near {squares:?}");
    }

    /// Every building has an outside door you can walk to from the main street, round fences and
    /// trees.
    #[test]
    fn every_building_can_be_walked_to_from_the_main_street() {
        let mut all = crate::template::tests::shipped_templates();
        all.push(test_town_hall());
        for seed in 1..=5 {
            let g = generate(&GenConfig { seed, ..Default::default() }, &all);
            let seen = reachable_outdoors(&g);
            let w = g.map.width();
            for (i, b) in g.map.buildings.iter().enumerate() {
                let reached = Rect::new(b.min, b.size).tiles().any(|t| {
                    g.map.tile(t).room != 0
                        && Side::ALL.iter().any(|&s| {
                            let o = t + s.offset();
                            let e = g.map.edge(t, s);
                            g.map.tile(o).room == 0 && (e.is_door() || e == Edge::Arch) && seen[(o.y * w + o.x) as usize]
                        })
                });
                assert!(reached, "seed {seed}: building {i} ({}) at {} can't be reached from the main street", b.template, b.min);
            }
        }
    }

    /// Shops and offices line the main street in the commercial blocks, fronts to it, inside the
    /// town core; nothing overlaps, and every building has a gap round it.
    #[test]
    fn shops_and_offices_line_the_main_street() {
        for (templates, seed) in [1, 2, 3, 4, 5].map(|s| (with_shops(), s)).into_iter().chain([1, 2, 3].map(|s| (crate::template::tests::shipped_templates(), s))) {
            let g = generate(&GenConfig { seed, ..Default::default() }, &templates);
            let main_mid = g.plan.streets[0].0.y + MAIN_CORRIDOR / 2;
            let (mut shops, mut offices, mut houses) = (0, 0, 0);
            for b in &g.map.buildings {
                let (lo, hi) = (b.min, b.min + b.size);
                match kind_of(&templates, b) {
                    BuildingKind::House => houses += 1,
                    kind => {
                        assert!(g.core.iter().any(|&c| inside(b, c)), "seed {seed}: {kind:?} {} at {lo}..{hi} outside the core {:?}", b.template, g.core);
                        if matches!(kind, BuildingKind::Shop) {
                            shops += 1
                        } else {
                            offices += 1
                        }
                        // On the strip (not a landmark lot), its front door opens onto pavement that
                        // leads to the main street.
                        if g.plan.landmarks.iter().any(|&l| inside(b, l)) {
                            continue;
                        }
                        let toward = if b.min.y < main_mid { Side::S } else { Side::N };
                        let door = Rect::new(b.min, b.size).tiles().find(|&t| g.map.tile(t).room != 0 && g.map.edge(t, toward).is_door() && g.map.tile(t + toward.offset()).room == 0);
                        let door = door.unwrap_or_else(|| panic!("seed {seed}: {} at {lo} has no door toward the main street", b.template));
                        assert_eq!(g.map.tile(door + toward.offset()).floor, Floor::Sidewalk, "seed {seed}: {} front not paved", b.template);
                    }
                }
            }
            assert_apart(&g, &format!("seed {seed}"));
            eprintln!("seed {seed}: {shops} shops, {offices} offices, {houses} houses, map {}", g.map.width());
            assert!(shops >= 4 && offices >= 2 && houses >= 55, "seed {seed}: {shops} shops, {offices} offices, {houses} houses");
        }
    }

    /// A civic building goes on a landmark lot (the biggest that fits), and so does an office too
    /// big for the main street; without them, landmark lots are greens, parking or offices.
    #[test]
    fn landmark_lots_take_civic_buildings() {
        let mut all = crate::template::tests::shipped_templates();
        all.push(test_town_hall());
        all.push(big("test_big_office", "Office", 30, 22));
        for seed in 1..=6 {
            let g = generate(&GenConfig { seed, ..Default::default() }, &all);
            let mut placed = 0;
            for b in &g.map.buildings {
                if b.template.starts_with("test_") {
                    assert!(g.plan.landmarks.iter().any(|&l| inside(b, l)), "seed {seed}: {} at {} not on a landmark lot", b.template, b.min);
                    placed += 1;
                }
            }
            assert!(placed >= g.plan.landmarks.len() - 1, "seed {seed}: {placed} big buildings on {} landmark lots", g.plan.landmarks.len());
            assert!(g.map.buildings.iter().any(|b| b.template == "test_town_hall"), "seed {seed}: no town hall");
            assert_apart(&g, &format!("seed {seed}"));
        }
        // With no civic templates, each landmark lot becomes something: a building, asphalt or a park.
        let shipped = crate::template::tests::shipped_templates();
        let mut uses = std::collections::BTreeSet::new();
        for seed in 1..=6 {
            let g = generate(&GenConfig { seed, ..Default::default() }, &shipped);
            for &(lo, hi) in &g.plan.landmarks {
                let r = Rect::new(lo, hi - lo);
                let built = g.map.buildings.iter().any(|b| inside(b, (lo, hi)));
                let asphalt = r.tiles().filter(|&t| g.map.tile(t).floor.is_road()).count();
                let trees = r.tiles().filter(|&t| matches!(g.map.tile(t).feature, Feature::Tree | Feature::Bush)).count();
                let what = if built { "building" } else if asphalt > 200 { "parking" } else if trees > 5 { "green" } else { "nothing" };
                assert_ne!(what, "nothing", "seed {seed}: landmark lot at {lo} left empty");
                uses.insert(what);
            }
        }
        assert_eq!(uses.len(), 3, "landmark lots only became {uses:?}");
    }

    /// With no shop or office templates the strip is lined with houses instead, and with no house
    /// templates there's still a town (and a spawn point, on the main street).
    #[test]
    fn missing_kinds_fall_back() {
        let only_houses = generate(&GenConfig::default(), &templates());
        let with = generate(&GenConfig::default(), &with_shops());
        let houses_in_core = only_houses.map.buildings.iter().filter(|b| only_houses.core.iter().any(|&c| inside(b, c))).count();
        assert!(houses_in_core >= 6, "only {houses_in_core} houses along the main street");
        assert!(only_houses.map.buildings.len() > with.map.buildings.len() / 2);

        let only_shops: Vec<BuildingTemplate> = with_shops().into_iter().filter(is_commercial).collect();
        let g = generate(&GenConfig::default(), &only_shops);
        assert!(g.map.buildings.iter().all(|b| g.core.iter().any(|&c| inside(b, c))), "a shop off the main street");
        assert!(!g.map.buildings.is_empty());
        assert_eq!(g.map.room_at(g.player_spawn), 0);
        assert!(!g.map.tile_blocked(g.player_spawn.floor().as_ivec2()));

        let none = generate(&GenConfig::default(), &[]);
        assert!(none.map.buildings.is_empty());
        // No commercial blocks: the core is the civic district alone, and the only offices are on its landmark lots.
        let zero = generate(&GenConfig { commercial_blocks: 0, ..Default::default() }, &with_shops());
        assert!(zero.plan.blocks.iter().all(|b| b.district != District::Commercial));
        for b in &zero.map.buildings {
            if !matches!(kind_of(&with_shops(), b), BuildingKind::House) {
                assert!(zero.plan.landmarks.iter().any(|&l| inside(b, l)), "{} off the landmark lots", b.template);
            }
        }
    }

    /// The town core zone is the civic and commercial blocks along the main street, the rest of
    /// town streets and yards, forest round it; and there are parking lots, vacant lots and a park.
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
                // A parking bay line: road paint with no sidewalk next to it (street lines run
                // down the middle of the asphalt).
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
        // The core is along the main street: the square and the shops are in it, a house lot on the edge of town isn't.
        let (sq, _) = g.plan.square.unwrap();
        assert_eq!(zone_at(&g.map, &town, sq + IVec2::splat(3), 1.0, 60.0), Zone::TownCore);
        let main = g.plan.streets[0].0 + IVec2::new(2, 5);
        assert_ne!(zone_at(&g.map, &town, main, 1.0, 60.0), Zone::TownCore, "the main street's far end is not the core");
        assert!(dirt > 20 && paths > 50 && parking > 10, "dirt {dirt}, paths {paths}, parking {parking}");
    }

    /// A bigger map keeps the same town in the middle with more forest round it, and a tiny one is
    /// grown to fit the town.
    #[test]
    fn map_size_is_configurable() {
        let all = crate::template::tests::shipped_templates();
        let small = generate(&GenConfig::default(), &all);
        assert_eq!((small.map.width(), small.map.height()), (480, 480));
        let big = generate(&GenConfig { chunks: IVec2::splat(64), ..Default::default() }, &all);
        assert_eq!((big.map.width(), big.map.height()), (1024, 1024));
        assert_eq!(big.map.buildings.len(), small.map.buildings.len());
        assert_eq!(big.town_max - big.town_min, small.town_max - small.town_min);
        let shift = big.town_min - small.town_min;
        for (a, b) in small.map.buildings.iter().zip(&big.map.buildings) {
            assert_eq!((a.template.as_str(), a.min + shift, a.size), (b.template.as_str(), b.min, b.size), "a different town on the bigger map");
        }
        assert_eq!(small.plan.blocks.len(), big.plan.blocks.len());
        let centre = (big.town_min + big.town_max) / 2;
        assert!((centre - IVec2::splat(512)).abs().max_element() <= 1, "town off centre: {centre}");
        assert!(!big.map.tile_blocked(big.player_spawn.floor().as_ivec2()));
        let tiny = generate(&GenConfig { chunks: IVec2::splat(4), ..Default::default() }, &all);
        assert!(tiny.town_min.x >= MIN_FOREST && tiny.town_max.x <= tiny.map.width() - MIN_FOREST + 1, "town doesn't fit");
        assert_eq!(tiny.map.buildings.len(), small.map.buildings.len());
        // Every seed's town fits the default map with forest round it.
        for seed in 1..20 {
            let g = generate(&GenConfig { seed, ..Default::default() }, &all);
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
        // (Biomes: woods in clumps with glades and meadows between, not one thick forest.)
        assert!(trees_outside > 2500, "only {trees_outside} trees");
    }

    /// With every shipped template, each house type turns up in the town for some seed, every
    /// office and shop type in the core, buildings never overlap, and you still wake up in a
    /// bedroom of a house.
    #[test]
    fn every_house_type_turns_up_in_towns() {
        let all = crate::template::tests::shipped_templates();
        let mut seen = std::collections::BTreeMap::new();
        for seed in 1..=4 {
            let g = generate(&GenConfig { seed, ..Default::default() }, &all);
            let mut by_kind = [0; 4];
            let mut owner = vec![usize::MAX; (g.map.width() * g.map.height()) as usize];
            for (i, b) in g.map.buildings.iter().enumerate() {
                *seen.entry(b.template.clone()).or_insert(0) += 1;
                let kind = kind_of(&all, b);
                by_kind[match kind {
                    BuildingKind::House => 0,
                    BuildingKind::Shop => 1,
                    BuildingKind::Office => 2,
                    BuildingKind::Civic => 3,
                }] += 1;
                if !matches!(kind, BuildingKind::House) {
                    assert!(g.core.iter().any(|&c| inside(b, c)), "seed {seed}: {} off the main street", b.template);
                }
                for t in Rect::new(b.min, b.size).tiles() {
                    let cell = &mut owner[(t.y * g.map.width() + t.x) as usize];
                    assert_eq!(*cell, usize::MAX, "seed {seed}: {} overlaps another building at {t}", b.template);
                    *cell = i;
                }
            }
            eprintln!("seed {seed}: {} houses, {} shops, {} offices", by_kind[0], by_kind[1], by_kind[2]);
            assert!(by_kind[0] >= 55 && by_kind[1] >= 4 && by_kind[2] >= 3, "seed {seed}: houses, shops, offices {by_kind:?}");
            let room = g.map.room_at(g.player_spawn);
            assert_eq!(g.map.rooms[room as usize].room_type, "bedroom", "seed {seed}: spawn outside a bedroom");
            assert!(matches!(kind_of(&all, &g.map.buildings[g.spawn_building]), BuildingKind::House));
        }
        for t in &all {
            assert!(seen.contains_key(&t.id), "{} never placed in four towns: {seen:?}", t.id);
        }
    }

    /// Houses face their streets whichever way those run: some fronts face east or west.
    #[test]
    fn houses_face_streets_every_way() {
        let all = crate::template::tests::shipped_templates();
        let g = generate(&GenConfig::default(), &all);
        let mut facing = [0; 4];
        for b in &g.map.buildings {
            if !matches!(kind_of(&all, b), BuildingKind::House) {
                continue;
            }
            // The side its garden path leaves from.
            for t in Rect::new(b.min, b.size).tiles().filter(|&t| g.map.tile(t).room != 0) {
                for s in Side::ALL {
                    if g.map.edge(t, s).is_door() && g.map.tile(t + s.offset()).floor == Floor::Path {
                        facing[si(s)] += 1;
                    }
                }
            }
        }
        eprintln!("front doors facing N, E, S, W: {facing:?}");
        assert!(facing.iter().all(|&n| n >= 3), "front doors facing N, E, S, W: {facing:?}");
    }

    /// Prints the default town as ASCII, every other row
    /// (`cargo test -p zombye-sim --release print_town -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn print_town() {
        let templates = crate::template::tests::shipped_templates();
        let seed = std::env::var("ZOMBYE_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
        let g = generate(&GenConfig { seed, ..Default::default() }, &templates);
        eprintln!("town {}..{}, square {:?}, cul-de-sacs {:?}, spawn {}", g.town_min, g.town_max, g.plan.square, g.plan.cul_de_sacs, g.player_spawn);
        // A top-down picture too (a PPM file, one pixel a tile), when asked for.
        if let Ok(path) = std::env::var("ZOMBYE_PPM") {
            let (w, h) = (g.map.width(), g.map.height());
            let mut out = format!("P6 {w} {h} 255\n").into_bytes();
            for y in 0..h {
                for x in 0..w {
                    let p = IVec2::new(x, y);
                    let t = g.map.tile(p);
                    let rgb: [u8; 3] = if t.room != 0 {
                        let b = g.map.rooms[t.room as usize].building as usize;
                        match kind_of(&templates, &g.map.buildings[b]) {
                            BuildingKind::House => [200, 120, 90],
                            BuildingKind::Shop => [70, 110, 210],
                            BuildingKind::Office => [150, 90, 200],
                            BuildingKind::Civic => [230, 200, 60],
                        }
                    } else if t.north == Edge::Fence || t.west == Edge::Fence {
                        [150, 120, 70]
                    } else {
                        match (t.feature, t.floor) {
                            (Feature::Tree | Feature::PineTree, _) => [40, 85, 40],
                            (Feature::Bush, _) => [80, 120, 60],
                            (_, Floor::Asphalt) => [55, 55, 60],
                            (_, Floor::RoadLine) => [220, 210, 120],
                            (_, Floor::Sidewalk) => [165, 165, 160],
                            (_, Floor::Path) => [190, 175, 140],
                            (_, Floor::Dirt) => [130, 105, 75],
                            _ => [110, 150, 90],
                        }
                    };
                    out.extend_from_slice(&rgb);
                }
            }
            std::fs::write(path, out).unwrap();
        }
        for b in &g.plan.blocks {
            eprintln!("{:?} {:?} {}..{} ({})", b.district, b.fill, b.min, b.max, b.max - b.min);
        }
        for y in (g.town_min.y - 6..g.town_max.y + 6).step_by(2) {
            let row: String = (g.town_min.x - 6..g.town_max.x + 6)
                .map(|x| {
                    let t = g.map.tile(IVec2::new(x, y));
                    if t.room != 0 {
                        let b = g.map.rooms[t.room as usize].building as usize;
                        match kind_of(&templates, &g.map.buildings[b]) {
                            BuildingKind::House => 'H',
                            BuildingKind::Shop => 'S',
                            BuildingKind::Office => 'O',
                            BuildingKind::Civic => 'C',
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
