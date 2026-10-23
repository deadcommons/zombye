//! The world map: 1 m tiles stored in 16 × 16 chunks.
//!
//! Each tile owns its **north** and **west** edges (wall, door, window, fence, opening). The south
//! edge of a tile is the north edge of the tile below it, and so on, so every edge is stored once.
//! Collision, pathfinding and line of sight all read the same edge data, and the client builds
//! its 3D wall meshes from it.
//!
//! M1 simulates the ground level only. Upper floors and roofs exist as per-building render data in
//! [`Building`]; walking upstairs comes later.

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
    /// Waist-high fence: blocks movement, not sight.
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
        matches!(self, Edge::Wall | Edge::Window | Edge::DoorClosed | Edge::Fence)
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

    /// An open or broken window: passable, but crossing it is a slow climb.
    pub fn is_climbable(self) -> bool {
        matches!(self, Edge::WindowOpen | Edge::WindowBroken)
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
}

impl Feature {
    pub fn blocks_movement(self) -> bool {
        matches!(self, Feature::Tree | Feature::PineTree | Feature::Furniture)
    }
}

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TileMap {
    /// Size in chunks.
    pub chunks_x: i32,
    pub chunks_y: i32,
    chunks: Vec<Chunk>,
    /// `rooms[0]` is the "outdoors" placeholder.
    pub rooms: Vec<Room>,
    pub buildings: Vec<Building>,
}

impl TileMap {
    pub fn new(chunks_x: i32, chunks_y: i32) -> Self {
        let chunks = (0..chunks_x * chunks_y).map(|_| Chunk { tiles: vec![Tile::default(); CHUNK_TILES] }).collect();
        let outdoors = Room { building: u16::MAX, name: "Outdoors".into(), room_type: "outdoors".into(), floor: Floor::Grass };
        Self { chunks_x, chunks_y, chunks, rooms: vec![outdoors], buildings: Vec::new() }
    }

    pub fn width(&self) -> i32 {
        self.chunks_x * CHUNK_SIZE
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

    /// Push a circle out of blocking edges, trees, furniture and the map border.
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
