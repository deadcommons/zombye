//! Street props (#61): mailboxes, fire hydrants, bins and the like that the town generator puts
//! outside, on lawns and sidewalks.
//!
//! **Data** (`assets/data/street_props.ron`, [`StreetPropsDef`], parsed with [`parse_street_props`]
//! and loaded by `net::content::load_street_props`): the prop kinds (id, the model the client draws,
//! size and colour of the grey-box drawn when there's no model, whether it blocks walking and how
//! big a body it pushes away) and the rules that place them, applied in order: [`Place::GardenPath`]
//! (on the lawn beside the street end of a house's garden path), [`Place::HouseKerb`] (at the kerb
//! in front of a house, beside its path), [`Place::ShopKerb`] (at the kerb in front of a shop or
//! office on the main street), [`Place::Kerb`] (along every straight kerb, `spacing` metres
//! apart), [`Place::Yard`] (on open lawn beside or behind a house), [`Place::Roadside`] and
//! [`Place::ShopRoad`] (parked on the road along the kerb in front of a house, or of a shop) and
//! [`Place::Parking`] (in a parking lot's bay). A data folder without the file has no street props.
//!
//! **Bigger than a tile:** a kind's `tiles` (across, deep; default one) make it cover several: a
//! parked car is 2 x 5, a shed 3 x 3. Its [`StreetProp::tile`] is the least corner of the tiles it
//! covers ([`PropKind::footprint`] turns them with its facing) and it stands in the middle of them.
//! Such kinds go only in yards, on the road and in parking bays. In a yard it keeps a tile of open
//! lawn all round it; parked, at least two tiles of road stay free beside it.
//!
//! **Placing** ([`place`], called at the end of `worldgen::generate`): every rule has its own random
//! stream from the seed, so the same seed and data give the same props, and changing one rule
//! doesn't move the others' props. Props stand in the middle of a tile, turned a quarter at a time
//! to face the street. Never on roads, paths, driveways (the client draws one from each garage door
//! straight out to the road) or a tile beside a door; kerb props only on a straight stretch of kerb
//! (not at corners, junctions, turning circles, or where an alley or parking lot meets the
//! sidewalk), never in front of a garden path; and never two props side by side (each keeps the
//! eight tiles round it free of others), so a blocking prop never closes a way.
//!
//! **On the map** ([`TileMap::street_props`], sorted by tile, with the kinds in
//! [`TileMap::prop_kinds`]): derived from generation, not saved (a save regenerates its map). A
//! blocking prop's tile is a [`Feature::StreetProp`]: the walk grid, flow fields, the navigation
//! graph and straight-line checks treat it like a tree, and [`TileMap::resolve_circle`] pushes a body
//! out of a circle of the kind's `radius` round its middle, the same for the server and the client's
//! prediction. Every tile a bigger blocking prop covers is one, and a body is kept out of the whole
//! of each ([`TileMap::street_prop_fills`]).

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::map::{Edge, Feature, Floor, Side, StreetProp, TileMap};
use crate::rng::Rng;

/// The street-prop file: kinds, then the rules that place them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreetPropsDef {
    #[serde(default)]
    pub kinds: Vec<PropKind>,
    #[serde(default)]
    pub rules: Vec<PropRule>,
}

/// One kind of street prop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropKind {
    pub id: String,
    /// The model the client draws (`<model>.glb` among the furniture models); the id when left out.
    #[serde(default)]
    pub model: Option<String>,
    /// Metres: width (x), depth (y) and height of the grey-box drawn when there's no model, facing
    /// south.
    pub size: (f32, f32, f32),
    /// Grey-box colour (sRGB, 0..1).
    pub color: (f32, f32, f32),
    /// Blocks walking (default yes): its tile is blocked for paths, and bodies are kept `radius`
    /// from its middle.
    #[serde(default = "yes")]
    pub blocks: bool,
    /// Metres from its middle a body is kept (at most 0.5: it stays in its tile); left out, half
    /// the larger of its width and depth.
    #[serde(default)]
    pub radius: Option<f32>,
    /// Tiles it covers, across and deep, facing south (default one tile). A bigger one (a parked
    /// car is 2 x 5) blocks every tile it covers whole, and `radius` isn't read.
    #[serde(default = "one_tile")]
    pub tiles: (u8, u8),
}

fn one_tile() -> (u8, u8) {
    (1, 1)
}

fn yes() -> bool {
    true
}

impl PropKind {
    pub fn model(&self) -> &str {
        self.model.as_deref().unwrap_or(&self.id)
    }

    /// How far from its middle a body is kept.
    pub fn radius(&self) -> f32 {
        self.radius.unwrap_or(0.5 * self.size.0.max(self.size.1)).clamp(0.0, 0.5)
    }

    /// Covers more than one tile.
    pub fn big(&self) -> bool {
        self.tiles != (1, 1)
    }

    /// The tiles it covers (x, y) when its front faces `facing`.
    pub fn footprint(&self, facing: Side) -> IVec2 {
        let (w, d) = (self.tiles.0.max(1) as i32, self.tiles.1.max(1) as i32);
        if matches!(facing, Side::E | Side::W) { IVec2::new(d, w) } else { IVec2::new(w, d) }
    }
}

/// Where a rule puts its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Place {
    /// On the lawn beside the street end of a house's garden path, facing the street.
    GardenPath,
    /// At the kerb in front of a house, two or three metres to one side of its garden path.
    HouseKerb,
    /// At the kerb in front of a shop or office on the main street.
    ShopKerb,
    /// Along every straight kerb in town, `spacing` metres apart.
    Kerb,
    /// In a house's yard, on open lawn beside or behind it (`chance` per house).
    Yard,
    /// On the road at the kerb in front of a house, parked along it (`chance` per house).
    Roadside,
    /// On the road at the kerb in front of a shop or office on the main street (`chance` per building).
    ShopRoad,
    /// In a parking lot's bay (`chance` per bay; kinds of at most 2 x 5 tiles).
    Parking,
}

/// One placing rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropRule {
    /// A kind's id.
    pub kind: String,
    pub place: Place,
    /// Chance per house or shop (default 1). Not for `Kerb`.
    #[serde(default = "one")]
    pub chance: f32,
    /// `Kerb` only: metres from one to the next along a kerb, (min, max).
    #[serde(default)]
    pub spacing: Option<(f32, f32)>,
}

fn one() -> f32 {
    1.0
}

/// Parse and validate a street-prop file.
pub fn parse_street_props(text: &str) -> Result<StreetPropsDef, String> {
    let def: StreetPropsDef = ron::from_str(text).map_err(|e| e.to_string())?;
    def.validate()?;
    Ok(def)
}

impl StreetPropsDef {
    /// Explains the first mistake, if any, naming the kind or rule.
    pub fn validate(&self) -> Result<(), String> {
        if self.kinds.len() > u16::MAX as usize {
            return Err("too many kinds".into());
        }
        for (i, k) in self.kinds.iter().enumerate() {
            let name = if k.id.is_empty() { format!("kind {i}") } else { format!("kind \"{}\"", k.id) };
            if k.id.is_empty() {
                return Err(format!("{name}: id is empty"));
            }
            if self.kinds[..i].iter().any(|o| o.id == k.id) {
                return Err(format!("{name}: id used twice"));
            }
            if k.model.as_deref() == Some("") {
                return Err(format!("{name}: model is empty"));
            }
            let (tw, td) = k.tiles;
            if !((1..=5).contains(&tw) && (1..=10).contains(&td)) {
                return Err(format!("{name}: tiles must be (1..5, 1..10), got ({tw}, {td})"));
            }
            let (w, d, h) = k.size;
            let (mw, md) = ((tw as f32).max(3.0), (td as f32).max(3.0));
            if !([w, d, h].iter().all(|v| v.is_finite() && *v > 0.0) && w <= mw && d <= md && h <= 4.0) {
                return Err(format!("{name}: size must be metres in (0, {mw}], (0, {md}] and (0, 4], got ({w}, {d}, {h})"));
            }
            let (r, g, b) = k.color;
            if ![r, g, b].iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
                return Err(format!("{name}: color must be 0..1 each, got ({r}, {g}, {b})"));
            }
            if let Some(r) = k.radius
                && !(r.is_finite() && (0.0..=0.5).contains(&r))
            {
                return Err(format!("{name}: radius must be in 0..0.5 m (a prop stays in its tile), got {r}"));
            }
        }
        for (i, rule) in self.rules.iter().enumerate() {
            let name = format!("rule {i} ({:?} {})", rule.place, rule.kind);
            if !self.kinds.iter().any(|k| k.id == rule.kind) {
                return Err(format!("{name}: no kind \"{}\"", rule.kind));
            }
            if !(rule.chance.is_finite() && (0.0..=1.0).contains(&rule.chance)) {
                return Err(format!("{name}: chance must be in 0..1, got {}", rule.chance));
            }
            match (rule.place, rule.spacing) {
                (Place::Kerb, None) => return Err(format!("{name}: Kerb needs spacing: (min, max) metres")),
                (Place::Kerb, Some((lo, hi))) => {
                    if !(lo.is_finite() && hi.is_finite() && 4.0 <= lo && lo <= hi && hi <= 1000.0) {
                        return Err(format!("{name}: spacing must be (min, max) metres with 4 <= min <= max <= 1000, got ({lo}, {hi})"));
                    }
                    if rule.chance != 1.0 {
                        return Err(format!("{name}: Kerb takes spacing, not chance"));
                    }
                }
                (_, Some(_)) => return Err(format!("{name}: only Kerb takes spacing")),
                (_, None) => {}
            }
            let kind = self.kinds.iter().find(|k| k.id == rule.kind).expect("checked above");
            match rule.place {
                Place::GardenPath | Place::HouseKerb | Place::ShopKerb | Place::Kerb if kind.big() => {
                    return Err(format!("{name}: a kind of more than one tile goes in a Yard, at the Roadside, on a ShopRoad or in Parking"));
                }
                Place::Parking if kind.tiles.0 > 2 || kind.tiles.1 > 5 => return Err(format!("{name}: a parking bay is 2 x 5 tiles")),
                _ => {}
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// A house the generator placed with a garden path: the path's last tile before the sidewalk, and
/// which way the street is.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HouseFront {
    pub path_end: IVec2,
    pub front: Side,
    /// The house's footprint.
    pub min: IVec2,
    pub max: IVec2,
}

/// A bay of a parking lot: its tiles (2 x 5) and the side its lot's street is on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bay {
    pub min: IVec2,
    pub size: IVec2,
    pub front: Side,
}

/// A shop or office on the main street: its footprint and which way the street is.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ShopFront {
    pub min: IVec2,
    pub max: IVec2,
    pub front: Side,
}

/// What the generator tells [`place`].
#[derive(Default)]
pub(crate) struct Fronts {
    pub houses: Vec<HouseFront>,
    pub shops: Vec<ShopFront>,
    pub bays: Vec<Bay>,
}

/// The direction along a street whose road is on side `s`.
fn along(s: Side) -> IVec2 {
    if matches!(s, Side::N | Side::S) { IVec2::X } else { IVec2::Y }
}

/// How far a kerb must run straight on each side of a kerb prop (clear of the 4 m kerb curves at
/// junctions).
const STRAIGHT: i32 = 5;
/// Kerb props keep this far from a turning circle's middle (its sidewalk ring and the curves into it).
const CIRCLE_CLEAR: f32 = 15.0;

/// Placing state: the town's tiles flagged as kept clear or taken.
struct Placer<'a> {
    map: &'a TileMap,
    min: IVec2,
    size: IVec2,
    /// Driveways and the like: never a prop.
    clear: Vec<bool>,
    /// A prop here or next to here.
    taken: Vec<bool>,
    placed: Vec<StreetProp>,
    kinds: &'a [PropKind],
}

impl Placer<'_> {
    fn index(&self, t: IVec2) -> Option<usize> {
        let d = t - self.min;
        (d.x >= 0 && d.y >= 0 && d.x < self.size.x && d.y < self.size.y).then(|| (d.y * self.size.x + d.x) as usize)
    }

    fn is_road(&self, t: IVec2) -> bool {
        self.map.in_bounds(t) && self.map.tile(t).floor.is_road()
    }

    fn is_walk(&self, t: IVec2) -> bool {
        let tile = self.map.tile(t);
        self.map.in_bounds(t) && tile.room == 0 && tile.floor == Floor::Sidewalk
    }

    /// Free for a prop: in town, outdoors, nothing on it, not kept clear, no prop on it or next to
    /// it, and no door or arch on any of its sides.
    fn free(&self, t: IVec2) -> bool {
        let Some(i) = self.index(t) else { return false };
        let tile = self.map.tile(t);
        !self.clear[i]
            && !self.taken[i]
            && tile.room == 0
            && tile.feature == Feature::None
            && !tile.floor.is_road()
            && tile.floor != Floor::Path
            && Side::ALL.iter().all(|&s| {
                let e = self.map.edge(t, s);
                !(e.is_door() || e == Edge::Arch || e == Edge::DoorBroken)
            })
    }

    /// A sidewalk tile at the kerb of a road on side `s`, on a straight stretch: the kerb runs on
    /// `STRAIGHT` tiles each way, nothing but sidewalk and then no asphalt (an alley, a parking lot)
    /// behind it there, no garden path coming out right behind it, and no turning circle near.
    fn straight_kerb(&self, t: IVec2, s: Side) -> bool {
        let a = along(s);
        let inward = -s.offset();
        for k in -STRAIGHT..=STRAIGHT {
            let q = t + a * k;
            if !self.is_walk(q) || !self.is_road(q + s.offset()) {
                return false;
            }
            // Across the sidewalk to what's behind it.
            let mut b = q + inward;
            let mut n = 0;
            while self.is_walk(b) && n < 8 {
                b += inward;
                n += 1;
            }
            if self.is_road(b) || (k.abs() <= 1 && self.map.tile(b).floor == Floor::Path) {
                return false;
            }
        }
        let c = t.as_vec2() + Vec2::splat(0.5);
        self.map.turning_circles.iter().all(|tc| tc.centre.distance(c) > CIRCLE_CLEAR)
    }

    /// Put a prop with its least corner on `t` (its one tile, for most).
    fn put(&mut self, kind: u16, t: IVec2, facing: Side) {
        let fp = self.kinds[kind as usize].footprint(facing);
        for dy in -1..=fp.y {
            for dx in -1..=fp.x {
                if let Some(i) = self.index(t + IVec2::new(dx, dy)) {
                    self.taken[i] = true;
                }
            }
        }
        self.placed.push(StreetProp { kind, tile: t, facing });
    }

    /// Asphalt with nothing on it and no prop on it or next to it.
    fn free_road(&self, t: IVec2) -> bool {
        self.index(t).is_some_and(|i| !self.taken[i] && !self.clear[i]) && self.is_road(t) && self.map.tile(t).room == 0 && self.map.tile(t).feature == Feature::None
    }

    /// Where a `wide` x `long` vehicle parks on the road beside the kerb tiles from `k` on along
    /// the street (road on side `s` of them): its least corner. The kerb beside it runs straight and
    /// no driveway comes out there, the road under it is free, and two more tiles of road beyond it
    /// are, so the street stays open.
    fn park_at(&self, k: IVec2, s: Side, wide: i32, long: i32) -> Option<IVec2> {
        let a = along(s);
        if !self.straight_kerb(k + a * (long / 2), s) {
            return None;
        }
        for j in 0..long {
            let q = k + a * j;
            if !self.is_walk(q) || self.index(q).is_none_or(|i| self.clear[i]) {
                return None;
            }
            for w in 1..=wide + 2 {
                if !self.free_road(q + s.offset() * w) {
                    return None;
                }
            }
        }
        let (c0, c1) = (k + s.offset(), k + s.offset() * wide + a * (long - 1));
        Some(c0.min(c1))
    }

    /// Open lawn for a prop of `fp` tiles with its least corner on `t`: those tiles and the ring
    /// round them outdoors grass with nothing on it, no prop near, and no wall, fence or door
    /// among them.
    fn open_lawn(&self, t: IVec2, fp: IVec2) -> bool {
        for dy in -1..=fp.y {
            for dx in -1..=fp.x {
                let q = t + IVec2::new(dx, dy);
                let Some(i) = self.index(q) else { return false };
                let tile = self.map.tile(q);
                if self.clear[i] || self.taken[i] || tile.room != 0 || tile.feature != Feature::None || tile.floor != Floor::Grass || tile.north != Edge::None || tile.west != Edge::None {
                    return false;
                }
            }
        }
        true
    }

    /// One of the two ways along a street whose road is on side `s`.
    fn along_side(s: Side, forward: bool) -> Side {
        match (matches!(s, Side::N | Side::S), forward) {
            (true, true) => Side::E,
            (true, false) => Side::W,
            (false, true) => Side::S,
            (false, false) => Side::N,
        }
    }

    /// The kerb tile straight out from `from` toward the street on side `front`, within `reach`.
    fn kerb_from(&self, from: IVec2, front: Side, reach: i32) -> Option<IVec2> {
        let mut q = from;
        for _ in 0..reach {
            q += front.offset();
            if self.is_walk(q) && self.is_road(q + front.offset()) {
                return Some(q);
            }
            if self.is_road(q) {
                return None;
            }
        }
        None
    }
}

/// Mark where driveways run (from each garage opening straight out to the first sidewalk and across
/// it, as the client draws them, a tile either side for the flare) as kept clear.
fn keep_driveways_clear(p: &mut Placer) {
    let map = p.map;
    for (bi, b) in map.buildings.iter().enumerate() {
        for y in b.min.y..b.min.y + b.size.y {
            for x in b.min.x..b.min.x + b.size.x {
                let t = IVec2::new(x, y);
                let room = map.tile(t).room;
                if room == 0 || map.rooms[room as usize].building as usize != bi || map.rooms[room as usize].room_type != "garage" {
                    continue;
                }
                for s in Side::ALL {
                    let e = map.edge(t, s);
                    if map.tile(t + s.offset()).room != 0 || !(e.is_door() || e == Edge::Arch || e == Edge::DoorBroken) {
                        continue;
                    }
                    let a = along(s);
                    let (mut q, mut on_walk) = (t + s.offset(), false);
                    for _ in 0..40 {
                        if !map.in_bounds(q) || map.tile(q).room != 0 || p.is_road(q) || (on_walk && !p.is_walk(q)) {
                            break;
                        }
                        on_walk |= p.is_walk(q);
                        for k in -1..=1 {
                            if let Some(i) = p.index(q + a * k) {
                                p.clear[i] = true;
                            }
                        }
                        q += s.offset();
                    }
                }
            }
        }
    }
}

/// Place the street props `def` asks for on the generated town (`town_min..town_max`), from the
/// houses' and shops' fronts. See the module docs.
pub(crate) fn place(map: &mut TileMap, def: &StreetPropsDef, seed: u64, town_min: IVec2, town_max: IVec2, fronts: &Fronts) {
    map.street_props.clear();
    map.prop_kinds = def.kinds.clone();
    if def.rules.is_empty() {
        return;
    }
    let min = (town_min - IVec2::splat(2)).max(IVec2::ZERO);
    let max = (town_max + IVec2::splat(2)).min(IVec2::new(map.width(), map.height()));
    let size = (max - min).max(IVec2::ZERO);
    let n = (size.x * size.y) as usize;
    let mut p = Placer { map, min, size, clear: vec![false; n], taken: vec![false; n], placed: Vec::new(), kinds: &def.kinds };
    keep_driveways_clear(&mut p);

    for (ri, rule) in def.rules.iter().enumerate() {
        let Some(kind) = def.kinds.iter().position(|k| k.id == rule.kind) else { continue };
        let kind = kind as u16;
        let mut rng = Rng::new(seed ^ 0x5354_5245_4554_0000 ^ (ri as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        match rule.place {
            Place::GardenPath => {
                for h in &fronts.houses {
                    if rng.f32() >= rule.chance {
                        continue;
                    }
                    let a = along(h.front);
                    let sides = if rng.f32() < 0.5 { [a, -a] } else { [-a, a] };
                    // On the lawn beside the path's last tile, the sidewalk in front of it.
                    let spot = sides.into_iter().map(|d| h.path_end + d).find(|&t| {
                        p.free(t) && p.map.tile(t).floor == Floor::Grass && p.is_walk(t + h.front.offset()) && p.map.tile(t).north != Edge::Fence && p.map.tile(t).west != Edge::Fence
                    });
                    if let Some(t) = spot {
                        p.put(kind, t, h.front);
                    }
                }
            }
            Place::HouseKerb => {
                for h in &fronts.houses {
                    if rng.f32() >= rule.chance {
                        continue;
                    }
                    let Some(k) = p.kerb_from(h.path_end, h.front, 6) else { continue };
                    let a = along(h.front);
                    let sign = if rng.f32() < 0.5 { 1 } else { -1 };
                    let spot = [2, 3, -2, -3].into_iter().map(|o| k + a * (o * sign)).find(|&t| p.free(t) && p.straight_kerb(t, h.front));
                    if let Some(t) = spot {
                        p.put(kind, t, h.front);
                    }
                }
            }
            Place::ShopKerb => {
                for s in &fronts.shops {
                    if rng.f32() >= rule.chance {
                        continue;
                    }
                    let a = along(s.front);
                    let (lo, hi) = if a.x != 0 { (s.min.x, s.max.x) } else { (s.min.y, s.max.y) };
                    // A tile just outside the front wall, somewhere along it.
                    let edge = match s.front {
                        Side::N => s.min.y,
                        Side::S => s.max.y - 1,
                        Side::W => s.min.x,
                        Side::E => s.max.x - 1,
                    };
                    for _ in 0..6 {
                        let at = lo + rng.below((hi - lo).max(1) as u32) as i32;
                        let from = if a.x != 0 { IVec2::new(at, edge) } else { IVec2::new(edge, at) };
                        if let Some(t) = p.kerb_from(from, s.front, 12)
                            && p.free(t)
                            && p.straight_kerb(t, s.front)
                        {
                            p.put(kind, t, s.front);
                            break;
                        }
                    }
                }
            }
            Place::Yard => {
                for h in &fronts.houses {
                    if rng.f32() >= rule.chance {
                        continue;
                    }
                    // Any way round, as things stand in a yard.
                    let facing = Side::ALL[rng.below(4) as usize];
                    let fp = def.kinds[kind as usize].footprint(facing);
                    // Beside and behind the house, up to 6 m out, never in front of its front wall.
                    let (lo, hi) = (h.min - IVec2::splat(6) - fp, h.max + IVec2::splat(6));
                    let mut spots = Vec::new();
                    for y in lo.y..hi.y {
                        for x in lo.x..hi.x {
                            let t = IVec2::new(x, y);
                            let in_front = match h.front {
                                Side::N => t.y < h.min.y,
                                Side::S => t.y + fp.y > h.max.y,
                                Side::W => t.x < h.min.x,
                                Side::E => t.x + fp.x > h.max.x,
                            };
                            if !in_front && p.open_lawn(t, fp) {
                                spots.push(t);
                            }
                        }
                    }
                    if !spots.is_empty() {
                        let t = spots[rng.below(spots.len() as u32) as usize];
                        p.put(kind, t, facing);
                    }
                }
            }
            Place::Roadside => {
                let (wide, long) = (def.kinds[kind as usize].tiles.0 as i32, def.kinds[kind as usize].tiles.1 as i32);
                for h in &fronts.houses {
                    if rng.f32() >= rule.chance {
                        continue;
                    }
                    let Some(k) = p.kerb_from(h.path_end, h.front, 6) else { continue };
                    let a = along(h.front);
                    let facing = Placer::along_side(h.front, rng.f32() < 0.5);
                    let offsets = if rng.f32() < 0.5 { [1, -long, 3, -long - 2] } else { [-long, 1, -long - 2, 3] };
                    if let Some(t) = offsets.into_iter().find_map(|o| p.park_at(k + a * o, h.front, wide, long)) {
                        p.put(kind, t, facing);
                    }
                }
            }
            Place::ShopRoad => {
                let (wide, long) = (def.kinds[kind as usize].tiles.0 as i32, def.kinds[kind as usize].tiles.1 as i32);
                for s in &fronts.shops {
                    if rng.f32() >= rule.chance {
                        continue;
                    }
                    let a = along(s.front);
                    let (lo, hi) = if a.x != 0 { (s.min.x, s.max.x) } else { (s.min.y, s.max.y) };
                    let edge = match s.front {
                        Side::N => s.min.y,
                        Side::S => s.max.y - 1,
                        Side::W => s.min.x,
                        Side::E => s.max.x - 1,
                    };
                    let facing = Placer::along_side(s.front, rng.f32() < 0.5);
                    for _ in 0..4 {
                        let at = lo + rng.below((hi - lo).max(1) as u32) as i32;
                        let from = if a.x != 0 { IVec2::new(at, edge) } else { IVec2::new(edge, at) };
                        if let Some(t) = p.kerb_from(from, s.front, 12).and_then(|k| p.park_at(k, s.front, wide, long)) {
                            p.put(kind, t, facing);
                            break;
                        }
                    }
                }
            }
            Place::Parking => {
                for b in &fronts.bays {
                    if rng.f32() >= rule.chance {
                        continue;
                    }
                    // Nose in or nose out.
                    let facing = if rng.f32() < 0.5 { b.front } else { b.front.opposite() };
                    let fp = def.kinds[kind as usize].footprint(facing);
                    if fp.x > b.size.x || fp.y > b.size.y {
                        continue;
                    }
                    if (0..fp.y).all(|dy| (0..fp.x).all(|dx| p.free_road(b.min + IVec2::new(dx, dy)))) {
                        p.put(kind, b.min, facing);
                    }
                }
            }
            Place::Kerb => {
                let (lo, hi) = rule.spacing.unwrap_or((40.0, 60.0));
                // Each run of kerb (sidewalk tiles in a line with road on the same side), in a fixed
                // order: rows for kerbs facing north and south, columns for east and west.
                for s in Side::ALL {
                    let a = along(s);
                    let (outer, inner) = if a.x != 0 { (size.y, size.x) } else { (size.x, size.y) };
                    for o in 0..outer {
                        let mut i = 0;
                        while i < inner {
                            let at = |i: i32| if a.x != 0 { min + IVec2::new(i, o) } else { min + IVec2::new(o, i) };
                            let kerb = |i: i32| p.is_walk(at(i)) && p.is_road(at(i) + s.offset());
                            if !kerb(i) {
                                i += 1;
                                continue;
                            }
                            let start = i;
                            while i < inner && kerb(i) {
                                i += 1;
                            }
                            // Along this run: the first a little way in, then every `spacing`, each at
                            // the first good tile from where it's due.
                            let mut due = start as f32 + rng.range(0.0, lo * 0.5);
                            while (due as i32) < i {
                                let mut j = due as i32;
                                while j < i && j < due as i32 + 8 && !(p.free(at(j)) && p.straight_kerb(at(j), s)) {
                                    j += 1;
                                }
                                if j < i && j < due as i32 + 8 {
                                    p.put(kind, at(j), s);
                                    due = j as f32 + rng.range(lo, hi);
                                } else {
                                    due += 8.0;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut placed = std::mem::take(&mut p.placed);
    placed.sort_by_key(|q| (q.tile.y, q.tile.x));
    for q in &placed {
        let k = &def.kinds[q.kind as usize];
        if !k.blocks {
            continue;
        }
        let fp = k.footprint(q.facing);
        for dy in 0..fp.y {
            for dx in 0..fp.x {
                if let Some(tile) = map.tile_mut(q.tile + IVec2::new(dx, dy)) {
                    tile.feature = Feature::StreetProp;
                }
            }
        }
    }
    map.street_props = placed;
}

#[cfg(test)]
mod tests;
