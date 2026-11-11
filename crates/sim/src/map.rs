//! The world map: 1 m tiles stored in 16 × 16 chunks.
//!
//! Each tile owns its **north** and **west** edges (wall, door, window, fence, opening). The south
//! edge of a tile is the north edge of the tile below it, and so on, so every edge is stored once.
//! Collision, pathfinding and line of sight all read the same edge data, and the client builds
//! its 3D wall meshes from it.
//!
//! Zombies, sight, doors and windows are simulated on the ground level only. Players can also walk
//! up a building's stairs onto its first upper floor ([`Stair`], [`TileMap::resolve_walker`]): the
//! upper floor's tiles and walls come from the per-building data in [`Building`], indexed by
//! [`TileMap::index_storeys`].

use std::collections::{BTreeMap, BTreeSet};

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

pub const CHUNK_SIZE: i32 = 16;
const CHUNK_TILES: usize = (CHUNK_SIZE * CHUNK_SIZE) as usize;
/// Half thickness of a wall for collision, in metres.
pub const WALL_HALF_THICKNESS: f32 = 0.08;
/// Radius of a tree trunk for collision.
pub const TREE_RADIUS: f32 = 0.3;
/// A body whose centre is this close to an open or broken window's line is climbing through it.
/// Just under a body radius plus the wall, so brushing past a window along the wall doesn't count.
pub const CLIMB_ZONE: f32 = 0.35;
/// Floor to floor, in metres (docs/asset-pipeline.md): how high a flight of stairs rises.
pub const STOREY: f32 = 3.0;

/// A flight of stairs from the ground floor up to the first upper floor: the tiles of a piece of
/// `stairs` furniture, a ramp rising toward `up` from the floor at its foot end to [`STOREY`] at
/// its top end. Bodies get on and off at the two ends only; its long sides are walls, and so is
/// its top end to anyone on the ground floor (it is solid underneath). Upstairs it is a well in
/// the floor, open at the top end. Derived from the buildings ([`TileMap::index_storeys`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stair {
    pub building: u16,
    /// Tiles: `min..min+size`.
    pub min: IVec2,
    pub size: IVec2,
    /// The way it rises.
    pub up: Side,
}

impl Stair {
    fn lo(&self) -> Vec2 {
        self.min.as_vec2()
    }

    fn hi(&self) -> Vec2 {
        (self.min + self.size).as_vec2()
    }

    /// How long the flight is over the ground, in metres.
    pub fn length(&self) -> f32 {
        match self.up {
            Side::N | Side::S => self.size.y as f32,
            Side::E | Side::W => self.size.x as f32,
        }
    }

    /// How far up the flight a body at `p` is: 0 at the foot end, 1 at the top end (less and more
    /// beyond them).
    pub fn progress(&self, p: Vec2) -> f32 {
        let (lo, hi) = (self.lo(), self.hi());
        match self.up {
            Side::N => (hi.y - p.y) / self.length(),
            Side::S => (p.y - lo.y) / self.length(),
            Side::E => (p.x - lo.x) / self.length(),
            Side::W => (hi.x - p.x) / self.length(),
        }
    }

    /// Is a body centred at `p` on the flight (between its sides, from the foot end up to, not
    /// including, the top end)?
    pub fn on(&self, p: Vec2) -> bool {
        let (lo, hi) = (self.lo(), self.hi());
        let across = match self.up {
            Side::N | Side::S => p.x >= lo.x && p.x < hi.x,
            Side::E | Side::W => p.y >= lo.y && p.y < hi.y,
        };
        across && (0.0..1.0).contains(&self.progress(p))
    }

    /// The upstairs tile a body steps onto from the top end.
    pub fn landing(&self) -> IVec2 {
        let top = match self.up {
            Side::N | Side::W => self.min,
            Side::S | Side::E => self.min + self.size - IVec2::ONE,
        };
        top + self.up.offset()
    }

    /// The ground-floor tile a body steps onto the foot end from.
    pub fn foot(&self) -> IVec2 {
        let bottom = match self.up {
            Side::N | Side::W => self.min + self.size - IVec2::ONE,
            Side::S | Side::E => self.min,
        };
        bottom - self.up.offset()
    }

    /// The middle of the flight's foot end and of its top end, on the ground.
    pub fn ends(&self) -> (Vec2, Vec2) {
        let (lo, hi) = (self.lo(), self.hi());
        let mid = (lo + hi) * 0.5;
        match self.up {
            Side::N => (Vec2::new(mid.x, hi.y), Vec2::new(mid.x, lo.y)),
            Side::S => (Vec2::new(mid.x, lo.y), Vec2::new(mid.x, hi.y)),
            Side::E => (Vec2::new(lo.x, mid.y), Vec2::new(hi.x, mid.y)),
            Side::W => (Vec2::new(hi.x, mid.y), Vec2::new(lo.x, mid.y)),
        }
    }

    /// Its two long sides, its foot end and its top end, as segments on the ground.
    fn walls(&self) -> ([(Vec2, Vec2); 2], (Vec2, Vec2), (Vec2, Vec2)) {
        let (lo, hi) = (self.lo(), self.hi());
        let (a, b, c, d) = (lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y));
        // a-b is the north end, b-c the east side, c-d the south end, d-a the west side.
        match self.up {
            Side::N => ([(b, c), (d, a)], (c, d), (a, b)),
            Side::S => ([(b, c), (d, a)], (a, b), (c, d)),
            Side::E => ([(a, b), (c, d)], (d, a), (b, c)),
            Side::W => ([(a, b), (c, d)], (b, c), (d, a)),
        }
    }
}

/// One tile of the first upper floor ([`TileMap::index_storeys`]): its floor, and the north and
/// west edges it owns up there. A tile just outside a building has edges and no floor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UpperTile {
    pub floor: bool,
    /// Furniture stands on it.
    pub blocked: bool,
    pub room: u16,
    pub north: Edge,
    pub west: Edge,
}

/// Does an upper floor's edge stop a body? Walls and windows do. Upstairs doors can't be shut
/// yet: they are open doorways.
pub fn upper_edge_blocks(e: Edge) -> bool {
    matches!(e, Edge::Wall | Edge::Window)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Floor {
    #[default]
    Grass,
    Dirt,
    Asphalt,
    RoadLine,
    Sidewalk,
    Path,
    Wood,
    Carpet,
    TileFloor,
    Concrete,
}

impl Floor {
    pub fn is_road(self) -> bool {
        matches!(self, Floor::Asphalt | Floor::RoadLine)
    }
}

/// What sits on an edge between two tiles.
///
/// Closed doors and intact windows can be battered down (their hit points live in the server's
/// `World`, not here); broken ones stay open for good. Open and broken windows can be climbed
/// through, slowly ([`TileMap::in_window_opening`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Edge {
    #[default]
    None,
    Wall,
    /// An intact, shut window: blocks movement, not sight.
    Window,
    DoorClosed,
    DoorOpen,
    /// An open doorway with no door.
    Arch,
    /// Waist-high fence: doesn't block sight; crossing it is a slow climb, as through an open window.
    Fence,
    /// A window slid open: climb through.
    WindowOpen,
    /// Smashed glass: climb through; can't be shut again.
    WindowBroken,
    /// A door battered off its hinges: a permanent opening.
    DoorBroken,
}

impl Edge {
    pub fn blocks_movement(self) -> bool {
        matches!(self, Edge::Wall | Edge::Window | Edge::DoorClosed)
    }

    /// Walls and closed doors. Glass doesn't, and neither does a doorway whose door is gone.
    pub fn blocks_sight(self) -> bool {
        matches!(self, Edge::Wall | Edge::DoorClosed)
    }

    /// A door a player can open or close.
    pub fn is_door(self) -> bool {
        matches!(self, Edge::DoorClosed | Edge::DoorOpen)
    }

    /// Any window, whatever its state.
    pub fn is_window(self) -> bool {
        matches!(self, Edge::Window | Edge::WindowOpen | Edge::WindowBroken)
    }

    /// Shut and in one piece: something zombies can batter (and that has hit points).
    pub fn is_breakable(self) -> bool {
        matches!(self, Edge::DoorClosed | Edge::Window)
    }

    /// An open or broken window, or a fence: passable, but crossing it is a slow climb.
    pub fn is_climbable(self) -> bool {
        matches!(self, Edge::WindowOpen | Edge::WindowBroken | Edge::Fence)
    }

    /// What this edge becomes once battered or smashed in.
    pub fn broken(self) -> Edge {
        match self {
            Edge::DoorClosed | Edge::DoorOpen => Edge::DoorBroken,
            Edge::Window | Edge::WindowOpen => Edge::WindowBroken,
            e => e,
        }
    }
}

/// Things standing on a tile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Feature {
    #[default]
    None,
    Tree,
    PineTree,
    Bush,
    /// Furniture occupying the tile (see the building's furniture list for what it is).
    Furniture,
    /// A street prop that blocks walking: a hydrant, a bin, a mailbox (see
    /// [`TileMap::street_props`] for which, and `crate::street_props`).
    StreetProp,
}

impl Feature {
    pub fn blocks_movement(self) -> bool {
        matches!(self, Feature::Tree | Feature::PineTree | Feature::Furniture | Feature::StreetProp)
    }
}

/// A street prop the town generator placed (`crate::street_props`): its kind (an index into
/// [`TileMap::prop_kinds`]), its tile (it stands in the middle; the least corner of one that covers
/// several) and which way its front faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreetProp {
    pub kind: u16,
    pub tile: IVec2,
    pub facing: Side,
}

/// How far a body is kept from the middle of a blocking street prop whose kind isn't known (a map
/// stored whole whose props aren't the generator's any more).
pub const STREET_PROP_RADIUS: f32 = 0.3;

/// Which owned edge of a tile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum OwnedEdge {
    #[default]
    North,
    West,
}

/// Any of the four sides of a tile. Converts to an owned edge on this tile or a neighbour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    N,
    E,
    S,
    W,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::N, Side::E, Side::S, Side::W];

    /// The tile that owns this side of `t`, and which of its edges it is.
    pub fn owner(self, t: IVec2) -> (IVec2, OwnedEdge) {
        match self {
            Side::N => (t, OwnedEdge::North),
            Side::W => (t, OwnedEdge::West),
            Side::S => (t + IVec2::Y, OwnedEdge::North),
            Side::E => (t + IVec2::X, OwnedEdge::West),
        }
    }

    pub fn offset(self) -> IVec2 {
        match self {
            Side::N => IVec2::NEG_Y,
            Side::E => IVec2::X,
            Side::S => IVec2::Y,
            Side::W => IVec2::NEG_X,
        }
    }

    pub fn opposite(self) -> Side {
        match self {
            Side::N => Side::S,
            Side::E => Side::W,
            Side::S => Side::N,
            Side::W => Side::E,
        }
    }

    /// Rotate clockwise by `quarter_turns` (map +y is "south", so clockwise is N→E→S→W).
    pub fn rotated(self, quarter_turns: u8) -> Side {
        let i = Side::ALL.iter().position(|s| *s == self).unwrap();
        Side::ALL[(i + quarter_turns as usize) % 4]
    }

    pub fn mirrored_x(self) -> Side {
        match self {
            Side::E => Side::W,
            Side::W => Side::E,
            s => s,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    pub floor: Floor,
    pub north: Edge,
    pub west: Edge,
    pub feature: Feature,
    /// Index into [`TileMap::rooms`], or 0 for outdoors.
    pub room: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chunk {
    pub tiles: Vec<Tile>,
}

/// A room: drives floor type, loot (later) and "which building am I in".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Room {
    pub building: u16,
    pub name: String,
    pub room_type: String,
    pub floor: Floor,
}

/// A piece of furniture placed in a building (render + collision; containers later).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlacedFurniture {
    pub kind: String,
    /// World tile of the min corner and size in tiles.
    pub min: IVec2,
    pub size: IVec2,
    /// Which way the front faces.
    pub facing: Side,
    pub level: i8,
}

/// An edge on an upper level (render only in M1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpperEdge {
    pub tile: IVec2,
    pub edge: OwnedEdge,
    pub kind: Edge,
    /// Rooms on either side (0 = outside): (north/west side, south/east side).
    pub rooms: (u16, u16),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum RoofStyle {
    /// Ridge runs along the x axis.
    GableX,
    /// Ridge runs along the y axis.
    GableY,
    Flat,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Building {
    pub template: String,
    /// World tile bounds: `min..min+size`.
    pub min: IVec2,
    pub size: IVec2,
    /// Number of storeys (1 or 2 in M1).
    pub levels: u8,
    /// Upper-floor tiles per level ≥ 1: (tile, room).
    pub upper_floors: Vec<(i8, IVec2, u16)>,
    pub upper_edges: Vec<(i8, UpperEdge)>,
    pub furniture: Vec<PlacedFurniture>,
    pub roof: RoofStyle,
    /// Seeded look: index into the client's siding and roof palettes.
    pub siding: u8,
    pub roof_color: u8,
    /// What the client dresses it with (#59), from its template. Derived: not saved, in no hash.
    #[serde(skip)]
    pub look: Option<crate::template::BuildingLook>,
}

/// Names one edge by the tile that owns it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeRef {
    pub tile: IVec2,
    pub edge: OwnedEdge,
}

impl EdgeRef {
    pub fn new(tile: IVec2, edge: OwnedEdge) -> Self {
        Self { tile, edge }
    }

    /// The edge on `side` of tile `t`.
    pub fn of(t: IVec2, side: Side) -> Self {
        let (tile, edge) = side.owner(t);
        Self { tile, edge }
    }

    /// A totally ordered key, for sorted (deterministic) maps keyed by edge.
    pub fn key(self) -> (i32, i32, u8) {
        (self.tile.x, self.tile.y, self.edge as u8)
    }

    pub fn mid(self) -> Vec2 {
        let (a, b) = TileMap::edge_segment(self.tile, self.edge);
        (a + b) * 0.5
    }

    /// The tile on the other side of this edge from `p`.
    pub fn far_tile(self, p: Vec2) -> IVec2 {
        match self.edge {
            OwnedEdge::North => if p.y < self.tile.y as f32 { self.tile } else { self.tile - IVec2::Y },
            OwnedEdge::West => if p.x < self.tile.x as f32 { self.tile } else { self.tile - IVec2::X },
        }
    }

    /// Distance from `p` to the edge's segment.
    pub fn distance(self, p: Vec2) -> f32 {
        let (a, b) = TileMap::edge_segment(self.tile, self.edge);
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
        p.distance(a + ab * t)
    }
}

/// A change to an edge, replicated to clients in snapshots.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct EdgeChange {
    pub tile: IVec2,
    pub edge: OwnedEdge,
    pub kind: Edge,
}

/// A blind on a window or a glazed door appearing, going or rolled up or down (`blind`: `None`
/// none any more, `Some(true)` down, `Some(false)` rolled up), replicated to clients in snapshots
/// like [`EdgeChange`]s (see [`crate::blinds`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindChange {
    pub tile: IVec2,
    pub edge: OwnedEdge,
    pub blind: Option<bool>,
}

/// A cul-de-sac's turning circle as the generator laid it out: tiles whose middles are within
/// `road` of `centre` are asphalt, then sidewalk out to `walk`. The street into it comes from the
/// `entry` side, its asphalt `stem` either side of the line through the centre and its sidewalks
/// out to `stem_walk`. Only for drawing it round (the tiles are what count).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TurningCircle {
    pub centre: Vec2,
    pub road: f32,
    pub walk: f32,
    pub entry: Side,
    pub stem: f32,
    pub stem_walk: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TileMap {
    /// Size in chunks.
    pub chunks_x: i32,
    pub chunks_y: i32,
    chunks: Vec<Chunk>,
    /// `rooms[0]` is the "outdoors" placeholder.
    pub rooms: Vec<Room>,
    pub buildings: Vec<Building>,
    /// The generator's turning circles, for the client to draw round. Derived: not saved (a map
    /// stored whole, after hand edits, has none) and not part of any hash.
    #[serde(skip)]
    pub turning_circles: Vec<TurningCircle>,
    /// The generator's street props (#61), sorted by tile (row by row), and their kinds (from
    /// `street_props.ron`). Derived like the rest of the generated map: not saved (a save
    /// regenerates its map; one stored whole gets them from the map generated on load) and not
    /// part of the tile data. A blocking one's tile is also a [`Feature::StreetProp`].
    #[serde(skip)]
    pub street_props: Vec<StreetProp>,
    #[serde(skip)]
    pub prop_kinds: Vec<crate::street_props::PropKind>,
    /// Blinds on windows and glazed doors ([`crate::blinds`]), by edge key: `true` down (glass
    /// hidden), `false` rolled up. World state, saved in its own section (`BLND`), not with the
    /// tiles; changes go out as [`BlindChange`]s.
    #[serde(skip)]
    pub blinds: BTreeMap<(i32, i32, u8), bool>,
    /// Doors with glass in them (sight passes a shut one), by edge key. Derived like the street
    /// props: placed with the map (`blinds::place`), not saved, in no hash; a network client gets
    /// them on joining.
    #[serde(skip)]
    pub glazed: BTreeSet<(i32, i32, u8)>,
    /// The flights of stairs players can walk up, in building order, and the first upper floor's
    /// tiles by `(y, x)`. Derived from `buildings` by [`TileMap::index_storeys`]: not saved, in no
    /// hash.
    #[serde(skip)]
    pub stairs: Vec<Stair>,
    #[serde(skip)]
    upper: BTreeMap<(i32, i32), UpperTile>,
    /// How fast a body moves on a flight of stairs, as a share of its speed on the flat
    /// (`StairTuning::speed`; 0 until the world sets it, which reads as 1).
    #[serde(skip)]
    pub stair_speed: f32,
}

impl TileMap {
    pub fn new(chunks_x: i32, chunks_y: i32) -> Self {
        let chunks = (0..chunks_x * chunks_y).map(|_| Chunk { tiles: vec![Tile::default(); CHUNK_TILES] }).collect();
        let outdoors = Room { building: u16::MAX, name: "Outdoors".into(), room_type: "outdoors".into(), floor: Floor::Grass };
        Self { chunks_x, chunks_y, chunks, rooms: vec![outdoors], buildings: Vec::new(), turning_circles: Vec::new(), street_props: Vec::new(), prop_kinds: Vec::new(), blinds: BTreeMap::new(), glazed: BTreeSet::new(), stairs: Vec::new(), upper: BTreeMap::new(), stair_speed: 0.0 }
    }

    pub fn width(&self) -> i32 {
        self.chunks_x * CHUNK_SIZE
    }

    /// Is this map well formed (as one read from a save must be before it's used): every chunk
    /// there with all its tiles, the outdoors room, and every tile's room and building existing?
    pub fn is_consistent(&self) -> bool {
        let n = (self.chunks_x.max(0) as usize) * (self.chunks_y.max(0) as usize);
        let rooms = self.rooms.len();
        self.chunks_x > 0
            && self.chunks_y > 0
            && self.chunks.len() == n
            && rooms > 0
            && self.chunks.iter().all(|c| c.tiles.len() == CHUNK_TILES && c.tiles.iter().all(|t| (t.room as usize) < rooms))
            && self.rooms.iter().skip(1).all(|r| (r.building as usize) < self.buildings.len())
    }

    pub fn height(&self) -> i32 {
        self.chunks_y * CHUNK_SIZE
    }

    pub fn in_bounds(&self, t: IVec2) -> bool {
        t.x >= 0 && t.y >= 0 && t.x < self.width() && t.y < self.height()
    }

    fn locate(&self, t: IVec2) -> (usize, usize) {
        let c = (t.y / CHUNK_SIZE) * self.chunks_x + t.x / CHUNK_SIZE;
        let l = (t.y % CHUNK_SIZE) * CHUNK_SIZE + t.x % CHUNK_SIZE;
        (c as usize, l as usize)
    }

    /// Out-of-bounds reads return a default tile; callers check bounds where it matters.
    pub fn tile(&self, t: IVec2) -> Tile {
        if !self.in_bounds(t) {
            return Tile::default();
        }
        let (c, l) = self.locate(t);
        self.chunks[c].tiles[l]
    }

    pub fn tile_mut(&mut self, t: IVec2) -> Option<&mut Tile> {
        if !self.in_bounds(t) {
            return None;
        }
        let (c, l) = self.locate(t);
        Some(&mut self.chunks[c].tiles[l])
    }

    /// The tiles of chunk `(cx, cy)` in row-major order.
    pub fn chunk(&self, cx: i32, cy: i32) -> Option<&Chunk> {
        (cx >= 0 && cy >= 0 && cx < self.chunks_x && cy < self.chunks_y).then(|| &self.chunks[(cy * self.chunks_x + cx) as usize])
    }

    pub fn edge(&self, t: IVec2, side: Side) -> Edge {
        let (owner, which) = side.owner(t);
        let tile = self.tile(owner);
        match which {
            OwnedEdge::North => tile.north,
            OwnedEdge::West => tile.west,
        }
    }

    pub fn set_edge(&mut self, t: IVec2, side: Side, kind: Edge) {
        let (owner, which) = side.owner(t);
        self.set_owned_edge(owner, which, kind);
    }

    pub fn set_owned_edge(&mut self, owner: IVec2, which: OwnedEdge, kind: Edge) {
        if let Some(tile) = self.tile_mut(owner) {
            match which {
                OwnedEdge::North => tile.north = kind,
                OwnedEdge::West => tile.west = kind,
            }
        }
    }

    /// The blind on edge `e`: `Some(true)` down, `Some(false)` rolled up, `None` none.
    pub fn blind(&self, e: EdgeRef) -> Option<bool> {
        self.blinds.get(&e.key()).copied()
    }

    /// Put up, roll, or (`None`) take down the blind on edge `e`.
    pub fn set_blind(&mut self, e: EdgeRef, blind: Option<bool>) {
        match blind {
            Some(down) => {
                self.blinds.insert(e.key(), down);
            }
            None => {
                self.blinds.remove(&e.key());
            }
        }
    }

    /// Is edge `e` a door with glass in it?
    pub fn glazed(&self, e: EdgeRef) -> bool {
        self.glazed.contains(&e.key())
    }

    /// Can a blind hang on edge `e`? A window (shut, open or broken: it hangs inside the frame) or
    /// a glazed door that is still on its hinges.
    pub fn takes_blind(&self, e: EdgeRef) -> bool {
        let kind = self.owned_edge(e.tile, e.edge);
        kind.is_window() || (kind.is_door() && self.glazed(e))
    }

    /// Is there glass in edge `e` that sight passes (a window, or a glazed door that's shut)?
    pub fn sees_through_glass(&self, e: EdgeRef) -> bool {
        let kind = self.owned_edge(e.tile, e.edge);
        kind.is_window() || (kind == Edge::DoorClosed && self.glazed(e))
    }

    /// Does crossing out of `t` through `side` block sight? Walls and shut doors do, unless the
    /// door is glazed; glass (a window in any state, a shut glazed door) does behind a blind
    /// that's down. An open door's blind swings away with it. The one rule for seeing: the fog of
    /// war, zombies' and players' sight (`crate::vision`). Sound goes by the edges alone.
    pub fn blocks_view(&self, t: IVec2, side: Side) -> bool {
        let kind = self.edge(t, side);
        match kind {
            Edge::DoorClosed | Edge::Window | Edge::WindowOpen | Edge::WindowBroken if !(self.blinds.is_empty() && self.glazed.is_empty()) => {
                let e = EdgeRef::of(t, side);
                if kind == Edge::DoorClosed && !self.glazed(e) {
                    return true;
                }
                self.blind(e) == Some(true)
            }
            _ => kind.blocks_sight(),
        }
    }

    pub fn owned_edge(&self, owner: IVec2, which: OwnedEdge) -> Edge {
        let tile = self.tile(owner);
        match which {
            OwnedEdge::North => tile.north,
            OwnedEdge::West => tile.west,
        }
    }

    pub fn tile_blocked(&self, t: IVec2) -> bool {
        !self.in_bounds(t) || self.tile(t).feature.blocks_movement()
    }

    /// Can something walk from tile `a` to the orthogonal neighbour on `side`?
    pub fn can_step(&self, a: IVec2, side: Side) -> bool {
        let b = a + side.offset();
        !self.tile_blocked(b) && !self.edge(a, side).blocks_movement()
    }

    /// Can something walk diagonally from `a` by `(dx, dy)` (each ±1) without cutting a corner?
    pub fn can_step_diagonal(&self, a: IVec2, dx: i32, dy: i32) -> bool {
        let sx = if dx > 0 { Side::E } else { Side::W };
        let sy = if dy > 0 { Side::S } else { Side::N };
        let ax = a + sx.offset();
        let ay = a + sy.offset();
        self.can_step(a, sx) && self.can_step(ax, sy) && self.can_step(a, sy) && self.can_step(ay, sx)
    }

    /// Is a body centred at `p` in the middle of climbing through an open or broken window (within
    /// [`CLIMB_ZONE`] of one, on either side)? Movement there is slow. Depends only on the map and
    /// the position, so client prediction of a climb is exact.
    pub fn in_window_opening(&self, p: Vec2) -> bool {
        let t = p.floor().as_ivec2();
        let f = p - t.as_vec2();
        let tile = self.tile(t);
        (f.y < CLIMB_ZONE && tile.north.is_climbable())
            || (f.x < CLIMB_ZONE && tile.west.is_climbable())
            || (1.0 - f.y < CLIMB_ZONE && self.tile(t + IVec2::Y).north.is_climbable())
            || (1.0 - f.x < CLIMB_ZONE && self.tile(t + IVec2::X).west.is_climbable())
    }

    /// The open or broken window a body centred at `p` is climbing through, if any (as
    /// [`in_window_opening`](Self::in_window_opening), with which edge it is).
    pub fn window_opening(&self, p: Vec2) -> Option<Edge> {
        let t = p.floor().as_ivec2();
        let f = p - t.as_vec2();
        let tile = self.tile(t);
        [
            (f.y < CLIMB_ZONE, tile.north),
            (f.x < CLIMB_ZONE, tile.west),
            (1.0 - f.y < CLIMB_ZONE, self.tile(t + IVec2::Y).north),
            (1.0 - f.x < CLIMB_ZONE, self.tile(t + IVec2::X).west),
        ]
        .into_iter()
        .find(|(near, e)| *near && e.is_climbable())
        .map(|(_, e)| e)
    }

    /// The street prop on tile `t`, if any.
    pub fn street_prop(&self, t: IVec2) -> Option<&StreetProp> {
        self.street_props.binary_search_by_key(&(t.y, t.x), |p| (p.tile.y, p.tile.x)).ok().map(|i| &self.street_props[i])
    }

    /// How far a body is kept from the middle of the blocking street prop on tile `t`.
    pub fn street_prop_radius(&self, t: IVec2) -> f32 {
        self.street_prop(t).and_then(|p| self.prop_kinds.get(p.kind as usize)).map_or(STREET_PROP_RADIUS, |k| k.radius())
    }

    /// The blocking street prop tile `t` is a whole tile of a prop bigger than one (a parked car):
    /// bodies are kept out of all of it. Such a prop's first tile holds it; its others hold none.
    pub fn street_prop_fills(&self, t: IVec2) -> bool {
        match self.street_prop(t).and_then(|p| self.prop_kinds.get(p.kind as usize)) {
            Some(k) => k.big(),
            None => self.prop_kinds.iter().any(|k| k.big()),
        }
    }

    /// The tiles a street prop covers: its least corner and size.
    pub fn street_prop_rect(&self, p: &StreetProp) -> (IVec2, IVec2) {
        (p.tile, self.prop_kinds.get(p.kind as usize).map_or(IVec2::ONE, |k| k.footprint(p.facing)))
    }

    /// Room id at a world position (0 = outdoors).
    pub fn room_at(&self, p: Vec2) -> u16 {
        self.tile(p.floor().as_ivec2()).room
    }

    /// Building index containing a world position, if indoors.
    pub fn building_at(&self, p: Vec2) -> Option<usize> {
        let room = self.room_at(p);
        (room != 0).then(|| self.rooms[room as usize].building as usize)
    }

    /// Endpoints of an owned edge in world metres.
    pub fn edge_segment(owner: IVec2, which: OwnedEdge) -> (Vec2, Vec2) {
        let o = owner.as_vec2();
        match which {
            OwnedEdge::North => (o, o + Vec2::X),
            OwnedEdge::West => (o, o + Vec2::Y),
        }
    }

    /// Work out the stairs and the first upper floor from the buildings (after the map is
    /// generated, loaded or its buildings edited). A piece of `stairs` furniture on the ground
    /// floor of a building with an upper storey is a [`Stair`] when it is one tile wide, there is
    /// free floor upstairs to step onto from its top end and a free tile at its foot.
    pub fn index_storeys(&mut self) {
        let mut upper: BTreeMap<(i32, i32), UpperTile> = BTreeMap::new();
        for b in &self.buildings {
            for &(level, t, room) in &b.upper_floors {
                if level == 1 {
                    let u = upper.entry((t.y, t.x)).or_default();
                    u.floor = true;
                    u.room = room;
                }
            }
            for (level, e) in &b.upper_edges {
                if *level == 1 {
                    let u = upper.entry((e.tile.y, e.tile.x)).or_default();
                    match e.edge {
                        OwnedEdge::North => u.north = e.kind,
                        OwnedEdge::West => u.west = e.kind,
                    }
                }
            }
            for f in b.furniture.iter().filter(|f| f.level == 1) {
                for y in 0..f.size.y {
                    for x in 0..f.size.x {
                        if let Some(u) = upper.get_mut(&(f.min.y + y, f.min.x + x)) {
                            u.blocked = true;
                        }
                    }
                }
            }
        }
        let mut stairs = Vec::new();
        for (bi, b) in self.buildings.iter().enumerate() {
            if b.levels < 2 {
                continue;
            }
            for f in b.furniture.iter().filter(|f| f.level == 0 && f.kind == "stairs") {
                let s = Stair { building: bi as u16, min: f.min, size: f.size, up: f.facing };
                let wide = match s.up {
                    Side::N | Side::S => f.size.x,
                    Side::E | Side::W => f.size.y,
                };
                let landing = s.landing();
                let lands = upper.get(&(landing.y, landing.x)).is_some_and(|u| u.floor && !u.blocked);
                if wide == 1 && s.length() >= 2.0 && lands && !self.tile_blocked(s.foot()) {
                    stairs.push(s);
                }
            }
        }
        self.upper = upper;
        self.stairs = stairs;
    }

    /// The flight of stairs a body centred at `p` is on, if any ([`Stair::on`]).
    pub fn stair_at(&self, p: Vec2) -> Option<&Stair> {
        self.stairs.iter().find(|s| s.on(p))
    }

    /// The flight of stairs covering tile `t`, if any.
    pub fn stair_on_tile(&self, t: IVec2) -> Option<&Stair> {
        self.stairs.iter().find(|s| t.cmpge(s.min).all() && t.cmplt(s.min + s.size).all())
    }

    /// A tile of the first upper floor (floor, furniture, room, the edges it owns), if there is
    /// anything of it there.
    pub fn upper_tile(&self, t: IVec2) -> Option<UpperTile> {
        self.upper.get(&(t.y, t.x)).copied()
    }

    /// The edge of the first upper floor on `side` of tile `t` (`Edge::None`: nothing there).
    pub fn upper_edge(&self, t: IVec2, side: Side) -> Edge {
        let (owner, which) = side.owner(t);
        self.upper.get(&(owner.y, owner.x)).map_or(Edge::None, |u| match which {
            OwnedEdge::North => u.north,
            OwnedEdge::West => u.west,
        })
    }

    /// Every tile of the first upper floor, row by row.
    pub fn upper_tiles(&self) -> impl Iterator<Item = (IVec2, UpperTile)> + '_ {
        self.upper.iter().map(|(&(y, x), u)| (IVec2::new(x, y), *u))
    }

    /// How high above the ground a body on `level` at `p` stands: on a flight of stairs its
    /// height there, upstairs a storey.
    pub fn height_at(&self, level: u8, p: Vec2) -> f32 {
        if level > 0 {
            return STOREY * level as f32;
        }
        self.stair_at(p).map_or(0.0, |s| s.progress(p).clamp(0.0, 1.0) * STOREY)
    }

    /// Room id at a position on `level` (0 = outdoors, or no floor there).
    pub fn room_on(&self, level: u8, p: Vec2) -> u16 {
        if level == 0 {
            return self.room_at(p);
        }
        self.upper_tile(p.floor().as_ivec2()).filter(|u| u.floor).map_or(0, |u| u.room)
    }

    /// Is the straight line from `a` to `b` on the first upper floor clear of its walls and
    /// windows, and over floor all the way? (For a hand's reach upstairs.)
    pub fn upper_clear(&self, a: Vec2, b: Vec2) -> bool {
        let steps = (a.distance(b) / 0.1).ceil().max(1.0) as i32;
        let mut prev = a.floor().as_ivec2();
        for k in 0..=steps {
            let p = a.lerp(b, k as f32 / steps as f32);
            let t = p.floor().as_ivec2();
            if !self.upper_tile(t).is_some_and(|u| u.floor) {
                return false;
            }
            if t != prev {
                let d = t - prev;
                let crossed = |owner: IVec2, north: bool| self.upper_tile(owner).is_some_and(|u| upper_edge_blocks(if north { u.north } else { u.west }));
                if (d.x != 0 && crossed(IVec2::new(prev.x.max(t.x), prev.y), false))
                    || (d.y != 0 && crossed(IVec2::new(prev.x, prev.y.max(t.y)), true))
                    || (d.x != 0 && d.y != 0 && (crossed(IVec2::new(prev.x.max(t.x), t.y), false) || crossed(IVec2::new(t.x, prev.y.max(t.y)), true)))
                {
                    return false;
                }
                prev = t;
            }
        }
        true
    }

    /// [`resolve_circle`](Self::resolve_circle) for a body that can use the stairs (a player), on
    /// `level` (0 the ground floor and the stairs, 1 upstairs). On the ground a flight of stairs
    /// is not a block of furniture but a ramp between two walls, shut at its top end to anyone not
    /// on it; upstairs the body is kept on the floor, out of the upper walls and furniture, and
    /// out of the stairwell but through its top end.
    pub fn resolve_walker(&self, mut p: Vec2, radius: f32, level: u8) -> Vec2 {
        if self.stairs.is_empty() && level == 0 {
            return self.resolve_circle(p, radius);
        }
        let wall = radius + WALL_HALF_THICKNESS;
        for _ in 0..2 {
            let t = p.floor().as_ivec2();
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let n = t + IVec2::new(dx, dy);
                    if level == 0 {
                        p = self.push_out_of_tile(p, radius, n, true);
                        continue;
                    }
                    let Some(u) = self.upper_tile(n) else {
                        p = push_out_of_box(p, radius, n.as_vec2(), n.as_vec2() + Vec2::ONE);
                        continue;
                    };
                    if !u.floor {
                        p = push_out_of_box(p, radius, n.as_vec2(), n.as_vec2() + Vec2::ONE);
                    } else if u.blocked {
                        p = push_out_of_box(p, radius, n.as_vec2() + Vec2::splat(0.05), n.as_vec2() + Vec2::splat(0.95));
                    }
                    for (which, kind) in [(OwnedEdge::North, u.north), (OwnedEdge::West, u.west)] {
                        if upper_edge_blocks(kind) {
                            let (a, b) = Self::edge_segment(n, which);
                            p = push_out_of_segment(p, wall, a, b);
                        }
                    }
                }
            }
            for s in &self.stairs {
                let (lo, hi) = (s.lo() - Vec2::splat(1.0), s.hi() + Vec2::splat(1.0));
                if p.x < lo.x || p.y < lo.y || p.x > hi.x || p.y > hi.y {
                    continue;
                }
                let (sides, foot, top) = s.walls();
                for (a, b) in sides {
                    p = push_out_of_segment(p, wall, a, b);
                }
                if level > 0 {
                    p = push_out_of_segment(p, wall, foot.0, foot.1);
                } else if !s.on(p) {
                    p = push_out_of_segment(p, wall, top.0, top.1);
                }
            }
        }
        p
    }

    /// One tile's part of [`resolve_circle`](Self::resolve_circle): out of what stands on tile
    /// `n` and of the edges it owns. With `stairs`, a tile of a flight of stairs isn't a block.
    fn push_out_of_tile(&self, mut p: Vec2, radius: f32, n: IVec2, stairs: bool) -> Vec2 {
        if !self.in_bounds(n) {
            return push_out_of_box(p, radius, n.as_vec2(), n.as_vec2() + Vec2::ONE);
        }
        let tile = self.tile(n);
        match tile.feature {
            Feature::Tree | Feature::PineTree => {
                p = push_out_of_circle(p, radius, n.as_vec2() + Vec2::splat(0.5), TREE_RADIUS);
            }
            Feature::Furniture if stairs && self.stair_on_tile(n).is_some() => {}
            Feature::Furniture => {
                p = push_out_of_box(p, radius, n.as_vec2() + Vec2::splat(0.05), n.as_vec2() + Vec2::splat(0.95));
            }
            Feature::StreetProp if self.street_prop_fills(n) => {
                p = push_out_of_box(p, radius, n.as_vec2(), n.as_vec2() + Vec2::ONE);
            }
            Feature::StreetProp => {
                p = push_out_of_circle(p, radius, n.as_vec2() + Vec2::splat(0.5), self.street_prop_radius(n));
            }
            _ => {}
        }
        for (which, kind) in [(OwnedEdge::North, tile.north), (OwnedEdge::West, tile.west)] {
            if kind.blocks_movement() {
                let (a, b) = Self::edge_segment(n, which);
                p = push_out_of_segment(p, radius + WALL_HALF_THICKNESS, a, b);
            }
        }
        p
    }

    /// Push a circle out of blocking edges, trees, furniture, street props and the map border.
    pub fn resolve_circle(&self, mut p: Vec2, radius: f32) -> Vec2 {
        for _ in 0..2 {
            let t = p.floor().as_ivec2();
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let n = t + IVec2::new(dx, dy);
                    if !self.in_bounds(n) {
                        p = push_out_of_box(p, radius, n.as_vec2(), n.as_vec2() + Vec2::ONE);
                        continue;
                    }
                    let tile = self.tile(n);
                    match tile.feature {
                        Feature::Tree | Feature::PineTree => {
                            p = push_out_of_circle(p, radius, n.as_vec2() + Vec2::splat(0.5), TREE_RADIUS);
                        }
                        Feature::Furniture => {
                            p = push_out_of_box(p, radius, n.as_vec2() + Vec2::splat(0.05), n.as_vec2() + Vec2::splat(0.95));
                        }
                        Feature::StreetProp if self.street_prop_fills(n) => {
                            p = push_out_of_box(p, radius, n.as_vec2(), n.as_vec2() + Vec2::ONE);
                        }
                        Feature::StreetProp => {
                            p = push_out_of_circle(p, radius, n.as_vec2() + Vec2::splat(0.5), self.street_prop_radius(n));
                        }
                        _ => {}
                    }
                    for (which, kind) in [(OwnedEdge::North, tile.north), (OwnedEdge::West, tile.west)] {
                        if kind.blocks_movement() {
                            let (a, b) = Self::edge_segment(n, which);
                            p = push_out_of_segment(p, radius + WALL_HALF_THICKNESS, a, b);
                        }
                    }
                }
            }
        }
        p
    }
}

fn push_out_of_segment(p: Vec2, r: f32, a: Vec2, b: Vec2) -> Vec2 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
    let closest = a + ab * t;
    let d = p - closest;
    let len = d.length();
    if len < r {
        if len > 1e-6 {
            return closest + d / len * r;
        }
        // Exactly on the segment: push along its normal.
        return closest + ab.perp().normalize() * r;
    }
    p
}

fn push_out_of_circle(p: Vec2, r: f32, c: Vec2, cr: f32) -> Vec2 {
    let d = p - c;
    let len = d.length();
    let min = r + cr;
    if len < min {
        return if len > 1e-6 { c + d / len * min } else { c + Vec2::X * min };
    }
    p
}

fn push_out_of_box(p: Vec2, r: f32, min: Vec2, max: Vec2) -> Vec2 {
    let closest = p.clamp(min, max);
    let d = p - closest;
    let len2 = d.length_squared();
    if len2 >= r * r {
        return p;
    }
    if len2 > 1e-10 {
        let len = len2.sqrt();
        return closest + d / len * r;
    }
    // Inside the box: leave by the nearest face.
    let c = (min + max) * 0.5;
    let half = (max - min) * 0.5;
    let o = p - c;
    if half.x - o.x.abs() < half.y - o.y.abs() {
        Vec2::new(c.x + o.x.signum() * (half.x + r), p.y)
    } else {
        Vec2::new(p.x, c.y + o.y.signum() * (half.y + r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_are_shared_between_neighbours() {
        let mut m = TileMap::new(2, 2);
        let t = IVec2::new(5, 5);
        m.set_edge(t, Side::S, Edge::Wall);
        assert_eq!(m.edge(t + IVec2::Y, Side::N), Edge::Wall);
        m.set_edge(t, Side::E, Edge::DoorClosed);
        assert_eq!(m.edge(t + IVec2::X, Side::W), Edge::DoorClosed);
        assert!(!m.can_step(t, Side::E));
        m.set_edge(t, Side::E, Edge::DoorOpen);
        assert!(m.can_step(t, Side::E));
    }

    #[test]
    fn circles_cannot_pass_walls() {
        let mut m = TileMap::new(2, 2);
        for y in 0..32 {
            m.set_edge(IVec2::new(10, y), Side::W, Edge::Wall);
        }
        // Walk east into the wall at x = 10.
        let mut p = Vec2::new(9.0, 8.5);
        for _ in 0..100 {
            p = m.resolve_circle(p + Vec2::new(0.05, 0.0), 0.3);
        }
        assert!(p.x < 10.0 - 0.3, "went through the wall: {p}");
    }
}
