//! Long-range paths: a coarse navigation graph over the whole map, searched with A* (the
//! hierarchical A* of docs/PLAN.md §4.3), for zombies heading somewhere no flow field reaches.
//!
//! **The graph** ([`NavGraph`]). Each 16 m chunk is split into its connected open areas: tiles
//! joined by walkable steps (from the [`WalkGrid`], so trees, furniture, walls, fences, shut doors
//! and windows separate them) without leaving the chunk. Each area is a node, with the tile nearest
//! its middle as its centre. Nodes are joined by portals: runs of crossings over a chunk border
//! (open ground, open doors, archways, broken doors; open or broken windows as a climb), and single
//! shut doors and intact windows, on chunk borders or inside a chunk (a shut house is its own node),
//! as "breach" portals. A link costs the octile distance from one centre through the portal to the
//! other centre, plus the zombie flow fields' extra cost of the crossing ([`CLIMB_COST`],
//! [`WINDOW_BREACH_COST`], [`DOOR_BREACH_COST`]). Node ids are `chunk << 8 | area`, areas numbered in
//! tile order and links kept sorted, so rebuilding the chunks around an edit (a door opened, shut
//! or broken: [`Nav::edge_changed`]) gives exactly the graph a fresh build would. The graph is
//! derived from the map and never saved.
//!
//! **Plans** ([`Nav`]). A* over the graph from a zombie's node to the node of its target. When the
//! target can be walked to, breach portals are left out (a horde goes round a row of shut houses,
//! not through them); only when it can't (the target is in a shut house, or the zombie is) do plans
//! lead through doors and windows. Plans are cached and shared: every node along a plan is indexed
//! with it (any part of a shortest path is a shortest path), so every zombie on the way, or that
//! joins it later, follows the same plan without searching. A miss queues a request; each tick
//! [`Nav::update`] makes at most [`PLANS_PER_TICK`] plans (and stops after [`EXPANSIONS_PER_TICK`]
//! node expansions), and the zombie heads straight for its target meanwhile. An edit drops the
//! plans through the chunks it touched (they are made again when asked for). Which plans exist
//! changes what zombies do, so they (and the queued requests) are saved; the index over them is
//! rebuilt on load.
//!
//! **Moving along a plan.** Within a node a zombie heads for the next portal (or, in its target's
//! node, the target) along a small field over the chunk's tiles: [`Nav::way`] for zombies that
//! walk with collision (near and active), [`Nav::trace`] for the virtual tier's bulk moves, which
//! follow it tile by tile and so never cross anything that blocks walking. Those fields are a pure
//! function of the map, so they are memoised and never saved.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::flow::{CLIMB_BITS, CLIMB_COST, DIAGONAL, DOOR_BITS, DOOR_BREACH_COST, NO_STEP, OFFSETS, STEPS, STRAIGHT, WINDOW_BITS, WINDOW_BREACH_COST, WalkGrid};
use crate::map::{CHUNK_SIZE, EdgeRef, Side};

/// A node: `chunk index << 8 | area index within the chunk`.
pub type NodeId = u32;

const C: i32 = CHUNK_SIZE;
const CHUNK_TILES: usize = (CHUNK_SIZE * CHUNK_SIZE) as usize;
/// "No node" in the per-tile area map (blocked tiles, and any beyond the per-chunk cap).
const NONE: u8 = u8::MAX;
/// Areas per chunk at most (a 16 x 16 chunk of single tiles boxed in by walls could have 256).
const MAX_AREAS: usize = 254;

/// Plans made per tick at most, and node expansions after which no new search starts that tick.
pub const PLANS_PER_TICK: u32 = 16;
pub const EXPANSIONS_PER_TICK: u32 = 40_000;
/// Plans kept; when full, the ones nobody used for [`PLAN_IDLE_TICKS`] go (then the older half).
pub const PLAN_CAPACITY: usize = 1024;
const PLAN_IDLE_TICKS: u64 = 600;
/// Queued plan requests at most (the rest ask again next time).
const MAX_REQUESTS: usize = 512;
/// Memoised in-chunk fields at most (each 256 bytes); when full they are all dropped.
const FIELD_CAPACITY: usize = 4096;
/// Tiles a single [`Nav::trace`] follows at most.
const MAX_TRACE: usize = 48;
/// A zombie this close to a shut door or window its plan leads through batters it (the flow
/// fields' crowd reach).
const BREACH_NEAR: f32 = 1.6;
const NO_HOP: u16 = u16::MAX;

/// How a portal is crossed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PortalKind {
    /// Open ground, an open or broken door, an archway.
    Open,
    /// Through an open or broken window.
    Climb,
    /// An intact window: battered through.
    Window,
    /// A shut door: battered through.
    Door,
}

impl PortalKind {
    /// Extra path cost of the crossing (the flow fields' fixed point: tenths of a metre).
    pub fn extra(self) -> u32 {
        match self {
            PortalKind::Open => 0,
            PortalKind::Climb => CLIMB_COST,
            PortalKind::Window => WINDOW_BREACH_COST,
            PortalKind::Door => DOOR_BREACH_COST,
        }
    }

    /// Can it be walked (or climbed) through, without battering anything?
    pub fn passable(self) -> bool {
        matches!(self, PortalKind::Open | PortalKind::Climb)
    }
}

/// A way from one node to a neighbouring one: step from tile `via` (in this node) to `to_tile`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub to: NodeId,
    pub cost: u32,
    pub via: IVec2,
    pub to_tile: IVec2,
    pub kind: PortalKind,
}

/// One connected open area of a chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// The area's tile nearest its middle.
    pub center: IVec2,
    pub tiles: u16,
    /// Sorted by (to, via, to_tile, kind).
    pub links: Vec<Link>,
}

/// The chunk-area graph (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NavGraph {
    width: i32,
    height: i32,
    cols: i32,
    rows: i32,
    /// Per tile, its area within its chunk, or [`NONE`].
    area: Vec<u8>,
    /// Per chunk, its nodes.
    chunks: Vec<Vec<Node>>,
    /// Dense numbering for searches: each chunk's first node, and every node's id.
    offsets: Vec<u32>,
    ids: Vec<NodeId>,
}

fn octile(a: IVec2, b: IVec2) -> u32 {
    let d = (a - b).abs();
    let (lo, hi) = (d.x.min(d.y) as u32, d.x.max(d.y) as u32);
    DIAGONAL * lo + STRAIGHT * (hi - lo)
}

/// How straight step `k` (0 east, 2 south, ...) out of `a` crosses, if it can.
fn crossing(walk: &WalkGrid, a: IVec2, k: usize) -> Option<PortalKind> {
    let bit = (k / 2) as u32;
    let extra = walk.extra(a);
    if extra & (1 << (CLIMB_BITS + bit)) != 0 {
        Some(PortalKind::Climb)
    } else if walk.steps(a) & (1 << k) != 0 {
        Some(PortalKind::Open)
    } else if extra & (1 << (WINDOW_BITS + bit)) != 0 {
        Some(PortalKind::Window)
    } else if extra & (1 << (DOOR_BITS + bit)) != 0 {
        Some(PortalKind::Door)
    } else {
        None
    }
}

/// The edge between orthogonal neighbours `a` and `b`.
fn edge_between(a: IVec2, b: IVec2) -> EdgeRef {
    let side = match b - a {
        IVec2 { x: 1, .. } => Side::E,
        IVec2 { x: -1, .. } => Side::W,
        IVec2 { y: 1, .. } => Side::S,
        _ => Side::N,
    };
    EdgeRef::of(a, side)
}

impl NavGraph {
    /// Build the graph for the whole map.
    pub fn new(walk: &WalkGrid) -> Self {
        let (width, height) = (walk.width(), walk.height());
        let (cols, rows) = (width / C, height / C);
        let mut g = Self { width, height, cols, rows, area: vec![NONE; (width * height).max(0) as usize], chunks: vec![Vec::new(); (cols * rows).max(0) as usize], offsets: Vec::new(), ids: Vec::new() };
        let all: Vec<usize> = (0..g.chunks.len()).collect();
        g.rebuild(walk, &all);
        g
    }

    /// Nodes in all.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Links in all (each way counted).
    pub fn link_count(&self) -> usize {
        self.chunks.iter().flatten().map(|n| n.links.len()).sum()
    }

    fn in_bounds(&self, t: IVec2) -> bool {
        t.x >= 0 && t.y >= 0 && t.x < self.width && t.y < self.height
    }

    fn tile_index(&self, t: IVec2) -> usize {
        (t.y * self.width + t.x) as usize
    }

    fn chunk_of_tile(&self, t: IVec2) -> usize {
        ((t.y / C) * self.cols + t.x / C) as usize
    }

    fn origin(&self, c: usize) -> IVec2 {
        IVec2::new(c as i32 % self.cols, c as i32 / self.cols) * C
    }

    /// The node tile `t` belongs to (none for blocked tiles).
    pub fn node_at(&self, t: IVec2) -> Option<NodeId> {
        if !self.in_bounds(t) {
            return None;
        }
        let a = self.area[self.tile_index(t)];
        (a != NONE).then(|| ((self.chunk_of_tile(t) as u32) << 8) | a as u32)
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.chunks.get((id >> 8) as usize)?.get((id & 0xff) as usize)
    }

    fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.chunks[(id >> 8) as usize][(id & 0xff) as usize]
    }

    fn dense(&self, id: NodeId) -> usize {
        self.offsets[(id >> 8) as usize] as usize + (id & 0xff) as usize
    }

    fn dense_node(&self, d: usize) -> &Node {
        let id = self.ids[d];
        &self.chunks[(id >> 8) as usize][(id & 0xff) as usize]
    }

    fn neighbours(&self, c: usize) -> impl Iterator<Item = usize> + use<> {
        let (cx, cy, cols, rows) = (c as i32 % self.cols, c as i32 / self.cols, self.cols, self.rows);
        [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().filter_map(move |(dx, dy)| {
            let (x, y) = (cx + dx, cy + dy);
            (x >= 0 && y >= 0 && x < cols && y < rows).then_some((y * cols + x) as usize)
        })
    }

    /// The chunks whose walk-grid tiles an edit at tile `t` can change ([`WalkGrid::update_around`]
    /// refreshes `t` ± 2), in index order.
    pub fn chunks_around(&self, t: IVec2) -> Vec<usize> {
        let max = IVec2::new(self.width - 1, self.height - 1);
        let (lo, hi) = ((t - IVec2::splat(2)).clamp(IVec2::ZERO, max) / C, (t + IVec2::splat(2)).clamp(IVec2::ZERO, max) / C);
        let mut out = Vec::new();
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                out.push((y * self.cols + x) as usize);
            }
        }
        out
    }

    /// Split chunk `c` into its areas (setting the per-tile area map), with no links yet.
    fn split_chunk(&mut self, walk: &WalkGrid, c: usize) -> Vec<Node> {
        let o = self.origin(c);
        for ly in 0..C {
            for lx in 0..C {
                let i = self.tile_index(o + IVec2::new(lx, ly));
                self.area[i] = NONE;
            }
        }
        let inside = |t: IVec2| t.x >= o.x && t.y >= o.y && t.x < o.x + C && t.y < o.y + C;
        let mut nodes = Vec::new();
        let (mut stack, mut tiles) = (Vec::new(), Vec::new());
        for ly in 0..C {
            for lx in 0..C {
                let t = o + IVec2::new(lx, ly);
                if nodes.len() >= MAX_AREAS || walk.blocked(t) || self.area[self.tile_index(t)] != NONE {
                    continue;
                }
                let k = nodes.len() as u8;
                let i = self.tile_index(t);
                self.area[i] = k;
                stack.push(t);
                tiles.clear();
                while let Some(u) = stack.pop() {
                    tiles.push(u);
                    let steps = walk.steps(u);
                    for (j, d) in OFFSETS.iter().enumerate() {
                        let n = u + *d;
                        if steps & (1 << j) == 0 || !inside(n) || walk.blocked(n) {
                            continue;
                        }
                        let ni = self.tile_index(n);
                        if self.area[ni] == NONE {
                            self.area[ni] = k;
                            stack.push(n);
                        }
                    }
                }
                // The tile nearest the mean (in integers, ties to the first in tile order).
                let n = tiles.len() as i64;
                let (sx, sy) = tiles.iter().fold((0i64, 0i64), |(x, y), u| (x + u.x as i64, y + u.y as i64));
                let center = *tiles.iter().min_by_key(|u| ((u.x as i64 * n - sx).pow(2) + (u.y as i64 * n - sy).pow(2), u.y, u.x)).expect("at least the seed");
                nodes.push(Node { center, tiles: tiles.len() as u16, links: Vec::new() });
            }
        }
        nodes
    }

    fn link_pair(&mut self, a: IVec2, b: IVec2, kind: PortalKind) {
        let (Some(ia), Some(ib)) = (self.node_at(a), self.node_at(b)) else { return };
        let (ca, cb) = (self.node(ia).expect("exists").center, self.node(ib).expect("exists").center);
        let cost = octile(ca, a) + STRAIGHT + kind.extra() + octile(b, cb);
        self.node_mut(ia).links.push(Link { to: ib, cost, via: a, to_tile: b, kind });
        self.node_mut(ib).links.push(Link { to: ia, cost, via: b, to_tile: a, kind });
    }

    /// Portals over the border east of chunk `c` (`east`) or south of it: a run of crossings
    /// joining the same two areas the same way is a portal, at the run's middle tile, and of the
    /// runs joining two areas the same way only the longest (the first of equals) is kept: within an
    /// area the in-chunk fields find the way to it.
    fn border(&mut self, walk: &WalkGrid, c: usize, east: bool) {
        let o = self.origin(c);
        let (k, step) = if east { (0, IVec2::X) } else { (2, IVec2::Y) };
        let at = |j: i32| if east { o + IVec2::new(C - 1, j) } else { o + IVec2::new(j, C - 1) };
        let mut run: Option<(NodeId, NodeId, PortalKind, i32)> = None;
        // (from, to, kind, middle, length) of the best run per (from, to, kind), in order found.
        let mut best: Vec<(NodeId, NodeId, PortalKind, i32, i32)> = Vec::new();
        for j in 0..=C {
            let here = if j < C {
                let a = at(j);
                match (self.node_at(a), self.node_at(a + step)) {
                    (Some(x), Some(y)) => crossing(walk, a, k).map(|kind| (x, y, kind)),
                    _ => None,
                }
            } else {
                None
            };
            if run.map(|(x, y, kind, _)| (x, y, kind)) != here {
                if let Some((x, y, kind, start)) = run {
                    let (mid, len) = (start + (j - start - 1) / 2, j - start);
                    match best.iter_mut().find(|b| (b.0, b.1, b.2) == (x, y, kind)) {
                        Some(b) if b.4 < len => (b.3, b.4) = (mid, len),
                        Some(_) => {}
                        None => best.push((x, y, kind, mid, len)),
                    }
                }
                run = here.map(|(x, y, kind)| (x, y, kind, j));
            }
        }
        for (_, _, kind, mid, _) in best {
            let a = at(mid);
            self.link_pair(a, a + step, kind);
        }
    }

    /// Shut doors and windows between two areas of chunk `c`: one portal per pair of areas and
    /// kind (the first in tile order).
    fn internal(&mut self, walk: &WalkGrid, c: usize) {
        let o = self.origin(c);
        let mut seen: Vec<(NodeId, NodeId, PortalKind)> = Vec::new();
        for ly in 0..C {
            for lx in 0..C {
                let a = o + IVec2::new(lx, ly);
                // Only shut doors and intact windows can join two areas of one chunk.
                if walk.extra(a) & (0xF << WINDOW_BITS | 0xF << DOOR_BITS) == 0 {
                    continue;
                }
                for (k, d, ok) in [(0, IVec2::X, lx < C - 1), (2, IVec2::Y, ly < C - 1)] {
                    let b = a + d;
                    if !ok {
                        continue;
                    }
                    let (Some(x), Some(y)) = (self.node_at(a), self.node_at(b)) else { continue };
                    if x != y
                        && let Some(kind) = crossing(walk, a, k)
                        && !seen.contains(&(x.min(y), x.max(y), kind))
                    {
                        seen.push((x.min(y), x.max(y), kind));
                        self.link_pair(a, b, kind);
                    }
                }
            }
        }
    }

    /// Rebuild chunks `chunks` (ascending): their areas, every link of theirs, and the links into
    /// them from their neighbours.
    pub fn rebuild(&mut self, walk: &WalkGrid, chunks: &[usize]) {
        let in_set = |c: usize| chunks.binary_search(&c).is_ok();
        for &c in chunks {
            self.chunks[c] = self.split_chunk(walk, c);
        }
        let mut touched = chunks.to_vec();
        for &c in chunks {
            for nb in self.neighbours(c) {
                if !in_set(nb) {
                    touched.push(nb);
                    for n in &mut self.chunks[nb] {
                        n.links.retain(|l| !in_set((l.to >> 8) as usize));
                    }
                }
            }
        }
        touched.sort_unstable();
        touched.dedup();
        for &c in chunks {
            let (cx, cy) = (c as i32 % self.cols, c as i32 / self.cols);
            // Each border once: east and south of every rebuilt chunk, and west and north of it
            // when that neighbour isn't being rebuilt too.
            if cx + 1 < self.cols {
                self.border(walk, c, true);
            }
            if cy + 1 < self.rows {
                self.border(walk, c, false);
            }
            if cx > 0 && !in_set(c - 1) {
                self.border(walk, c - 1, true);
            }
            if cy > 0 && !in_set(c - self.cols as usize) {
                self.border(walk, c - self.cols as usize, false);
            }
            self.internal(walk, c);
        }
        for &c in &touched {
            for n in &mut self.chunks[c] {
                n.links.sort_unstable_by_key(|l| (l.to, l.via.y, l.via.x, l.to_tile.y, l.to_tile.x, l.kind, l.cost));
            }
        }
        self.offsets.clear();
        self.ids.clear();
        for (c, nodes) in self.chunks.iter().enumerate() {
            self.offsets.push(self.ids.len() as u32);
            self.ids.extend((0..nodes.len()).map(|k| ((c as u32) << 8) | k as u32));
        }
        self.offsets.push(self.ids.len() as u32);
    }

    /// Per tile of `src`'s chunk, the step (into [`OFFSETS`]) toward `src` by the shortest walk
    /// within `src`'s area, or [`NO_STEP`].
    fn local_field(&self, walk: &WalkGrid, src: IVec2) -> Box<[u8; CHUNK_TILES]> {
        let mut dir = Box::new([NO_STEP; CHUNK_TILES]);
        let Some(area) = self.node_at(src).map(|id| (id & 0xff) as u8) else { return dir };
        let o = self.origin(self.chunk_of_tile(src));
        let local = |t: IVec2| {
            let l = t - o;
            (l.x >= 0 && l.y >= 0 && l.x < C && l.y < C).then_some((l.y * C + l.x) as usize)
        };
        let mut cost = [u32::MAX; CHUNK_TILES];
        let mut heap = BinaryHeap::new();
        let s = local(src).expect("in its own chunk");
        cost[s] = 0;
        heap.push(Reverse((0u32, s as u16)));
        while let Some(Reverse((c, i))) = heap.pop() {
            let i = i as usize;
            if c > cost[i] {
                continue;
            }
            let t = o + IVec2::new(i as i32 % C, i as i32 / C);
            let (steps, extra) = (walk.steps(t), walk.extra(t));
            for (k, d) in OFFSETS.iter().enumerate() {
                if steps & (1 << k) == 0 {
                    continue;
                }
                let n = t + *d;
                let Some(ni) = local(n) else { continue };
                if self.area[self.tile_index(n)] != area {
                    continue;
                }
                let step = if k % 2 == 0 { STRAIGHT + if extra & (1 << (CLIMB_BITS + (k / 2) as u32)) != 0 { CLIMB_COST } else { 0 } } else { DIAGONAL };
                let nc = c + step;
                if nc < cost[ni] {
                    cost[ni] = nc;
                    dir[ni] = ((k + 4) % 8) as u8;
                    heap.push(Reverse((nc, ni as u16)));
                }
            }
        }
        dir
    }
}

/// A step of a plan: from node `node`, cross from tile `via` (in it) to tile `to` (in the next).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hop {
    pub node: NodeId,
    pub via: IVec2,
    pub to: IVec2,
    pub kind: PortalKind,
}

/// A path over the graph from `start` to `goal` (no hops: there's no way).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Plan {
    start: NodeId,
    goal: NodeId,
    reachable: bool,
    hops: Vec<Hop>,
    used_tick: u64,
}

/// What a zombie in some node heading for a goal node does next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Route {
    /// Already in the goal's node.
    Here,
    /// Take this hop.
    Hop(Hop),
    /// No way there at all.
    Unreachable,
    /// No plan yet: one has been asked for.
    Pending,
}

/// What [`Nav::way`] says a walking zombie should do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Way {
    /// Walk this way (unit vector).
    Go(Vec2),
    /// Its plan leads through this shut door or window, right here.
    Breach(EdgeRef),
}

/// The navigation graph plus the shared plan cache (see the module docs).
#[derive(Clone, Debug, Default)]
pub struct Nav {
    graph: NavGraph,
    /// Plans in the order they were made; saved.
    plans: Vec<Plan>,
    /// (node, goal) -> (plan, hop) for every node along every plan (the first plan to pass a node
    /// wins). Derived from `plans`.
    index: BTreeMap<(NodeId, NodeId), (u32, u16)>,
    /// Plans asked for and not made yet, in order; saved.
    requests: Vec<(NodeId, NodeId)>,
    /// In-chunk fields toward a tile, by tile index (memoised; a pure function of the map).
    fields: BTreeMap<u32, Box<[u8; CHUNK_TILES]>>,
    /// Per dense node, which part of the graph it is in counting every link, and counting only
    /// passable ones (empty when stale).
    reach_all: Vec<u32>,
    reach_walk: Vec<u32>,
    cost: Vec<u32>,
    came: Vec<(u32, u16)>,
    stamp: Vec<u32>,
    generation: u32,
    heap: BinaryHeap<Reverse<(u32, u32)>>,
    /// Plans made and nodes expanded by the last [`update`](Self::update), and searches in all
    /// (statistics; not saved).
    pub made: u32,
    pub expanded: u32,
    pub searches: u64,
}

/// The plan cache as saved: the plans in order, and the queued requests.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct SavedNav {
    plans: Vec<Plan>,
    requests: Vec<(NodeId, NodeId)>,
}

impl Nav {
    pub fn new(walk: &WalkGrid) -> Self {
        Self { graph: NavGraph::new(walk), ..Default::default() }
    }

    pub fn graph(&self) -> &NavGraph {
        &self.graph
    }

    /// Plans cached.
    pub fn plan_count(&self) -> usize {
        self.plans.len()
    }

    /// Plan requests waiting.
    pub fn pending(&self) -> usize {
        self.requests.len()
    }

    /// The map changed everywhere ([`crate::world::World::map_changed`]): build afresh and forget
    /// every plan.
    pub fn rebuild_all(&mut self, walk: &WalkGrid) {
        *self = Self { searches: self.searches, ..Self::new(walk) };
    }

    /// An edge at tile `t` changed (the walk grid is already updated): rebuild the chunks around it
    /// and drop the plans and requests through them.
    pub fn edge_changed(&mut self, walk: &WalkGrid, t: IVec2) {
        let chunks = self.graph.chunks_around(t);
        self.graph.rebuild(walk, &chunks);
        let hit = |id: NodeId| chunks.binary_search(&((id >> 8) as usize)).is_ok();
        let before = self.plans.len();
        self.plans.retain(|p| !hit(p.start) && !hit(p.goal) && !p.hops.iter().any(|h| hit(h.node)));
        if self.plans.len() != before {
            self.reindex();
        }
        self.requests.retain(|&(s, g)| !hit(s) && !hit(g));
        let cols = self.graph.cols;
        let width = self.graph.width;
        self.fields.retain(|&k, _| {
            let t = IVec2::new(k as i32 % width, k as i32 / width);
            chunks.binary_search(&(((t.y / C) * cols + t.x / C) as usize)).is_err()
        });
        self.reach_all.clear();
        self.reach_walk.clear();
    }

    fn add(&mut self, plan: Plan) {
        let p = self.plans.len() as u32;
        if plan.reachable {
            for (k, h) in plan.hops.iter().enumerate() {
                self.index.entry((h.node, plan.goal)).or_insert((p, k as u16));
            }
        } else {
            self.index.entry((plan.start, plan.goal)).or_insert((p, NO_HOP));
        }
        self.plans.push(plan);
    }

    fn reindex(&mut self) {
        self.index.clear();
        for plan in std::mem::take(&mut self.plans) {
            self.add(plan);
        }
    }

    /// What a zombie in node `from` heading for node `goal` does next. A miss asks for a plan.
    pub(crate) fn route(&mut self, from: NodeId, goal: NodeId, tick: u64) -> Route {
        if from == goal {
            return Route::Here;
        }
        match self.index.get(&(from, goal)) {
            Some(&(p, k)) => {
                let plan = &mut self.plans[p as usize];
                plan.used_tick = tick;
                if k == NO_HOP { Route::Unreachable } else { Route::Hop(plan.hops[k as usize]) }
            }
            None => {
                if self.requests.len() < MAX_REQUESTS && !self.requests.contains(&(from, goal)) {
                    self.requests.push((from, goal));
                }
                Route::Pending
            }
        }
    }

    /// Spend this tick's planning budget on the queued requests, in order.
    pub(crate) fn update(&mut self, tick: u64) {
        self.made = 0;
        self.expanded = 0;
        let mut taken = 0;
        while taken < self.requests.len() && self.made < PLANS_PER_TICK && self.expanded < EXPANSIONS_PER_TICK {
            let (s, g) = self.requests[taken];
            taken += 1;
            if self.index.contains_key(&(s, g)) || self.graph.node(s).is_none() || self.graph.node(g).is_none() {
                continue;
            }
            if self.plans.len() >= PLAN_CAPACITY {
                self.prune(tick);
            }
            let plan = self.search(s, g, tick);
            self.made += 1;
            self.searches += 1;
            self.add(plan);
        }
        self.requests.drain(..taken);
    }

    /// Make room: drop plans nobody used lately, and if that isn't enough the less recently used
    /// half (ties: the earlier plan goes).
    fn prune(&mut self, tick: u64) {
        self.plans.retain(|p| p.used_tick + PLAN_IDLE_TICKS >= tick);
        let n = self.plans.len();
        if n > PLAN_CAPACITY / 2 {
            let mut order: Vec<(u64, usize)> = self.plans.iter().enumerate().map(|(i, p)| (p.used_tick, i)).collect();
            order.sort_unstable();
            let mut keep = vec![true; n];
            for &(_, i) in &order[..n / 2] {
                keep[i] = false;
            }
            let mut k = keep.iter();
            self.plans.retain(|_| *k.next().unwrap_or(&true));
        }
        self.reindex();
    }

    /// Label which nodes can reach which: over every link, and over passable links only.
    fn ensure_reach(&mut self) {
        let n = self.graph.len();
        if self.reach_all.len() == n && n > 0 {
            return;
        }
        for (walk_only, out) in [(false, &mut self.reach_all), (true, &mut self.reach_walk)] {
            out.clear();
            out.resize(n, u32::MAX);
            let mut stack = Vec::new();
            for start in 0..n {
                if out[start] != u32::MAX {
                    continue;
                }
                out[start] = start as u32;
                stack.push(start);
                while let Some(d) = stack.pop() {
                    for l in &self.graph.dense_node(d).links {
                        let nd = self.graph.dense(l.to);
                        if (!walk_only || l.kind.passable()) && out[nd] == u32::MAX {
                            out[nd] = start as u32;
                            stack.push(nd);
                        }
                    }
                }
            }
        }
    }

    /// A* from `s` to `g`. Breach portals are only used when `g` can't be walked to.
    fn search(&mut self, s: NodeId, g: NodeId, tick: u64) -> Plan {
        let mut plan = Plan { start: s, goal: g, reachable: false, hops: Vec::new(), used_tick: tick };
        self.ensure_reach();
        let (ds, dg) = (self.graph.dense(s), self.graph.dense(g));
        if self.reach_all[ds] != self.reach_all[dg] {
            return plan;
        }
        let walk_only = self.reach_walk[ds] == self.reach_walk[dg];
        let n = self.graph.len();
        let Nav { graph, cost, came, stamp, generation, heap, expanded, .. } = self;
        if stamp.len() != n {
            stamp.clear();
            stamp.resize(n, 0);
            cost.resize(n, 0);
            came.resize(n, (0, 0));
            *generation = 0;
        }
        *generation = generation.wrapping_add(1);
        if *generation == 0 {
            stamp.iter_mut().for_each(|s| *s = 0);
            *generation = 1;
        }
        let gen_ = *generation;
        let goal_c = graph.dense_node(dg).center;
        let h = |d: usize| octile(graph.dense_node(d).center, goal_c);
        heap.clear();
        stamp[ds] = gen_;
        cost[ds] = 0;
        heap.push(Reverse((h(ds), ds as u32)));
        while let Some(Reverse((f, d))) = heap.pop() {
            let d = d as usize;
            if d == dg {
                break;
            }
            let c = cost[d];
            if c + h(d) < f {
                continue;
            }
            *expanded += 1;
            for (li, l) in graph.dense_node(d).links.iter().enumerate() {
                if walk_only && !l.kind.passable() {
                    continue;
                }
                let nd = graph.dense(l.to);
                let nc = c + l.cost;
                if stamp[nd] != gen_ || nc < cost[nd] {
                    stamp[nd] = gen_;
                    cost[nd] = nc;
                    came[nd] = (d as u32, li as u16);
                    heap.push(Reverse((nc + h(nd), nd as u32)));
                }
            }
        }
        if stamp[dg] != gen_ {
            return plan;
        }
        let mut d = dg;
        while d != ds {
            let (from, li) = came[d];
            let node_id = graph.ids[from as usize];
            let l = graph.dense_node(from as usize).links[li as usize];
            plan.hops.push(Hop { node: node_id, via: l.via, to: l.to_tile, kind: l.kind });
            d = from as usize;
        }
        plan.hops.reverse();
        plan.reachable = plan.hops.len() < NO_HOP as usize;
        if !plan.reachable {
            plan.hops.clear();
        }
        plan
    }

    /// The step (into [`OFFSETS`]) from tile `t` toward tile `src` within their node.
    fn step_toward(&mut self, walk: &WalkGrid, src: IVec2, t: IVec2) -> Option<usize> {
        if src == t || !self.graph.in_bounds(src) {
            return None;
        }
        let key = self.graph.tile_index(src) as u32;
        if !self.fields.contains_key(&key) {
            if self.fields.len() >= FIELD_CAPACITY {
                self.fields.clear();
            }
            let f = self.graph.local_field(walk, src);
            self.fields.insert(key, f);
        }
        let o = self.graph.origin(self.graph.chunk_of_tile(src));
        let l = t - o;
        if l.x < 0 || l.y < 0 || l.x >= C || l.y >= C {
            return None;
        }
        let k = self.fields[&key][(l.y * C + l.x) as usize];
        (k != NO_STEP).then_some(k as usize)
    }

    /// The node and tile to steer for when heading to `target`: its own tile, or the nearest tile
    /// around it that has a node (a target can be on a tree or furniture).
    pub(crate) fn goal_of(&self, target: Vec2) -> Option<(NodeId, IVec2)> {
        let c = target.floor().as_ivec2();
        for r in 0..=2i32 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    let t = c + IVec2::new(dx, dy);
                    if let Some(n) = self.graph.node_at(t) {
                        return Some((n, t));
                    }
                }
            }
        }
        None
    }

    /// Which way a zombie walking with collision at `p` (near and active zombies) goes for
    /// `target` along the graph: toward the next portal of its plan, or in the target's node
    /// toward the target; [`Way::Breach`] when its plan leads through a shut door or window within
    /// reach. `None` when the graph can't help (no node here, a plan not made yet, no way there,
    /// on the target's tile): go straight.
    pub(crate) fn way(&mut self, walk: &WalkGrid, p: Vec2, target: Vec2, tick: u64) -> Option<Way> {
        let tile = p.floor().as_ivec2();
        let from = self.graph.node_at(tile)?;
        let (goal, goal_tile) = self.goal_of(target)?;
        let (dest, hop) = match self.route(from, goal, tick) {
            Route::Here => (goal_tile, None),
            Route::Hop(h) => (h.via, Some(h)),
            Route::Unreachable | Route::Pending => return None,
        };
        if let Some(h) = hop
            && !h.kind.passable()
        {
            let edge = edge_between(h.via, h.to);
            if tile == dest || edge.distance(p) <= BREACH_NEAR {
                return Some(Way::Breach(edge));
            }
        }
        if tile == dest {
            let h = hop?;
            return Some(Way::Go((h.to.as_vec2() + Vec2::splat(0.5) - p).normalize_or_zero()));
        }
        let k = self.step_toward(walk, dest, tile)?;
        Some(Way::Go(STEPS[k]))
    }

    /// Where a virtual zombie at `p` gets to walking `dist` metres toward `target` along the graph,
    /// tile by tile (to points `jitter` off each tile's centre, |jitter| < 0.5 per axis), so it never
    /// crosses anything that blocks walking. It waits at a shut door or window its plan leads
    /// through. `None` when the graph can't help from here (no node, no plan yet, no way).
    pub(crate) fn trace(&mut self, walk: &WalkGrid, p: Vec2, target: Vec2, dist: f32, jitter: Vec2, tick: u64) -> Option<Vec2> {
        let (goal, goal_tile) = self.goal_of(target)?;
        let mut tile = p.floor().as_ivec2();
        let (mut pos, mut left) = (p, dist);
        for k in 0..MAX_TRACE {
            if left <= 1e-4 {
                break;
            }
            let Some(n) = self.graph.node_at(tile) else { break };
            let (dest, hop) = match self.route(n, goal, tick) {
                Route::Here => (goal_tile, None),
                Route::Hop(h) => (h.via, Some(h)),
                _ if k == 0 => return None,
                _ => break,
            };
            let next = if tile == dest {
                match hop {
                    Some(h) if h.kind.passable() => h.to,
                    _ => break,
                }
            } else {
                match self.step_toward(walk, dest, tile) {
                    Some(s) => tile + OFFSETS[s],
                    None => break,
                }
            };
            let aim = next.as_vec2() + Vec2::splat(0.5) + jitter;
            let seg = aim - pos;
            let len = seg.length();
            if len >= left {
                pos += seg * (left / len);
                break;
            }
            pos = aim;
            left -= len;
            tile = next;
        }
        Some(pos)
    }

    pub(crate) fn to_saved(&self) -> SavedNav {
        SavedNav { plans: self.plans.clone(), requests: self.requests.clone() }
    }

    /// Restore saved plans and requests into a nav built for the same map.
    pub(crate) fn restore(&mut self, saved: SavedNav) -> Result<(), String> {
        let ok = |id: NodeId| self.graph.node(id).is_some();
        if saved.plans.len() > PLAN_CAPACITY || saved.requests.len() > MAX_REQUESTS {
            return Err(format!("nav: {} plans, {} requests", saved.plans.len(), saved.requests.len()));
        }
        for p in &saved.plans {
            if !ok(p.start) || !ok(p.goal) || p.hops.iter().any(|h| !ok(h.node)) || p.reachable == p.hops.is_empty() || p.hops.len() >= NO_HOP as usize {
                return Err("nav: a plan names a node the map doesn't have".into());
            }
        }
        if saved.requests.iter().any(|&(s, g)| !ok(s) || !ok(g)) {
            return Err("nav: a request names a node the map doesn't have".into());
        }
        self.plans = saved.plans;
        self.requests = saved.requests;
        self.reindex();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{Edge, TileMap};

    /// Walk a plan's hops and check every crossing is one the walk grid allows (walkable, or a
    /// breach of a shut door or window), so a path never goes through a wall.
    fn check_plan(map: &TileMap, plan: &Plan) {
        for h in &plan.hops {
            let e = edge_between(h.via, h.to);
            let kind = map.owned_edge(e.tile, e.edge);
            match h.kind {
                PortalKind::Open => assert!(!kind.blocks_movement() && !kind.is_climbable(), "open hop through {kind:?} at {}", h.via),
                PortalKind::Climb => assert!(kind.is_climbable(), "climb through {kind:?}"),
                PortalKind::Window => assert_eq!(kind, Edge::Window),
                PortalKind::Door => assert_eq!(kind, Edge::DoorClosed),
            }
            assert!(!map.tile_blocked(h.via) && !map.tile_blocked(h.to));
        }
    }

    fn plan_between(nav: &mut Nav, a: IVec2, b: IVec2) -> Plan {
        let (s, g) = (nav.graph.node_at(a).unwrap(), nav.graph.node_at(b).unwrap());
        assert_eq!(nav.route(s, g, 0), Route::Pending);
        nav.update(1);
        let (p, _) = nav.index[&(s, g)];
        nav.plans[p as usize].clone()
    }

    /// A long wall with one gap and one shut door: the plan goes through the gap (walking is
    /// possible), never through the wall; with the gap closed it goes through the door.
    #[test]
    fn plans_go_through_gaps_and_doors_never_walls() {
        let mut map = TileMap::new(8, 8);
        // Wall along x = 64 from y = 0 to 128, gap (an archway) at y = 100, a shut door at y = 20.
        for y in 0..128 {
            map.set_edge(IVec2::new(64, y), Side::W, Edge::Wall);
        }
        map.set_edge(IVec2::new(64, 100), Side::W, Edge::Arch);
        map.set_edge(IVec2::new(64, 20), Side::W, Edge::DoorClosed);
        let walk = WalkGrid::new(&map);
        let mut nav = Nav::new(&walk);
        let (a, b) = (IVec2::new(30, 30), IVec2::new(100, 30));
        let plan = plan_between(&mut nav, a, b);
        assert!(plan.reachable);
        check_plan(&map, &plan);
        let crossing: Vec<&Hop> = plan.hops.iter().filter(|h| (h.via.x < 64) != (h.to.x < 64)).collect();
        assert_eq!(crossing.len(), 1, "crosses the wall line once: {:?}", plan.hops);
        assert_eq!((crossing[0].via.y, crossing[0].kind), (100, PortalKind::Open), "through the arch, not the door: {:?}", crossing[0]);
        // Close the arch: the only way is the door.
        map.set_edge(IVec2::new(64, 100), Side::W, Edge::Wall);
        let mut walk = walk;
        walk.update_around(&map, IVec2::new(64, 100));
        nav.edge_changed(&walk, IVec2::new(64, 100));
        assert_eq!(nav.graph, NavGraph::new(&walk), "a local update must match a fresh build");
        assert_eq!(nav.plan_count(), 0, "the plan through the closed arch is dropped");
        let plan = plan_between(&mut nav, a, b);
        check_plan(&map, &plan);
        let crossing: Vec<&Hop> = plan.hops.iter().filter(|h| (h.via.x < 64) != (h.to.x < 64)).collect();
        assert_eq!((crossing.len(), crossing[0].via.y, crossing[0].kind), (1, 20, PortalKind::Door), "{:?}", plan.hops);
        // Wall the door up too: no way.
        map.set_edge(IVec2::new(64, 20), Side::W, Edge::Wall);
        walk.update_around(&map, IVec2::new(64, 20));
        nav.edge_changed(&walk, IVec2::new(64, 20));
        let plan = plan_between(&mut nav, a, b);
        assert!(!plan.reachable);
        let (s, g) = (nav.graph.node_at(a).unwrap(), nav.graph.node_at(b).unwrap());
        assert_eq!(nav.route(s, g, 2), Route::Unreachable);
    }

    /// Following a traced path tile by tile from one side of a maze-like wall to the other never
    /// crosses a wall, and gets there.
    #[test]
    fn traces_follow_the_plan_and_never_cross_walls() {
        let mut map = TileMap::new(6, 6);
        // Two staggered walls: gap at the south end of the first and the north end of the second.
        for y in 0..90 {
            map.set_edge(IVec2::new(30, y), Side::W, Edge::Wall);
            map.set_edge(IVec2::new(60, 96 - 1 - y), Side::W, Edge::Wall);
        }
        // Trees scattered about.
        for k in 0..200 {
            let t = IVec2::new((k * 37) % 96, (k * 53) % 96);
            if t.x != 30 && t.x != 60 && t.x != 29 && t.x != 59 {
                map.tile_mut(t).unwrap().feature = crate::map::Feature::Tree;
            }
        }
        for t in [IVec2::new(5, 10), IVec2::new(85, 10)] {
            map.tile_mut(t).unwrap().feature = crate::map::Feature::None;
        }
        let walk = WalkGrid::new(&map);
        let mut nav = Nav::new(&walk);
        let target = Vec2::new(85.5, 10.5);
        let mut p = Vec2::new(5.5, 10.5);
        for tick in 0..4000 {
            nav.update(tick);
            let q = nav.trace(&walk, p, target, 0.25, Vec2::new(0.1, -0.2), tick).unwrap_or(p);
            assert!(!map.tile_blocked(q.floor().as_ivec2()), "on a blocked tile at {q}");
            // Every move is along walkable steps (short moves, so the straight line between is the path).
            let n = ((q - p).length() * 8.0).ceil() as usize + 1;
            for s in 0..n {
                let (a, b) = (p.lerp(q, s as f32 / n as f32), p.lerp(q, (s + 1) as f32 / n as f32));
                let (ta, tb) = (a.floor().as_ivec2(), b.floor().as_ivec2());
                if ta != tb {
                    let d = tb - ta;
                    assert!(d.x.abs() + d.y.abs() <= 2, "jumped from {ta} to {tb}");
                    if d.x != 0 && d.y != 0 {
                        assert!(map.can_step_diagonal(ta, d.x, d.y), "cut a corner at {ta}");
                    } else {
                        let side = match d {
                            IVec2 { x: 1, .. } => Side::E,
                            IVec2 { x: -1, .. } => Side::W,
                            IVec2 { y: 1, .. } => Side::S,
                            _ => Side::N,
                        };
                        assert!(map.can_step(ta, side), "crossed a wall from {ta} to {tb} moving {p} -> {q}: blocked {} {}, edge {:?}", map.tile_blocked(ta), map.tile_blocked(tb), map.edge(ta, side));
                    }
                }
            }
            p = q;
            if p.distance(target) < 1.5 {
                break;
            }
        }
        assert!(p.distance(target) < 1.5, "never got there: {p}");
    }

    /// Plans are shared: a crowd spread over many nodes heading to one goal needs few searches,
    /// and the graph rebuilt around a changed door matches a fresh build.
    #[test]
    fn plans_are_shared_along_the_way() {
        let map = TileMap::new(16, 16);
        let walk = WalkGrid::new(&map);
        let mut nav = Nav::new(&walk);
        let g = nav.graph.node_at(IVec2::new(20, 20)).unwrap();
        // One zombie far off asks first...
        let far = nav.graph.node_at(IVec2::new(240, 240)).unwrap();
        assert_eq!(nav.route(far, g, 0), Route::Pending);
        nav.update(1);
        assert_eq!(nav.searches, 1);
        let plan = nav.plans[0].clone();
        // ...then every node on its way already has the answer.
        for h in &plan.hops {
            assert!(matches!(nav.route(h.node, g, 2), Route::Hop(_)));
        }
        nav.update(2);
        assert_eq!(nav.searches, 1, "nodes along a plan reuse it");
        // Asking twice queues once.
        let other = nav.graph.node_at(IVec2::new(240, 10)).unwrap();
        nav.route(other, g, 3);
        nav.route(other, g, 3);
        assert_eq!(nav.pending(), 1);
    }
}
