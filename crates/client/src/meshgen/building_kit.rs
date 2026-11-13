//! The building kit (#59): the details that make a generated building read as a house, a shop or
//! an office, merged into its meshes (`world_render::build_building`), so they cost no draw calls.
//!
//! - **Walls:** a concrete foundation and, on houses, a pale water-table board low on the wall;
//!   siding board lines every 0.3 m (brick courses every 0.25 m on shops and offices); a belt board
//!   between storeys; corner boards on the outside corners.
//! - **Windows and doors:** frames, a sill and a cross of muntins; door casings; steps up to every
//!   outside door that has no porch.
//! - **Roofs:** fascia boards along the eaves and up the gables, a gutter, a ridge cap, a gable
//!   wall reaching the roof with a small window in it, a brick chimney with a cap on some houses and
//!   a TV aerial on others; flat roofs get a parapet with coping and a rooftop unit.
//! - **Porches** (`BuildingTemplate::porch`): a porch room under the main roof gets posts along its
//!   open sides and a railing across its arches but the way in; a porch built onto the front gets a
//!   raised deck, steps, posts, a railing and a shed roof of its own. Render only: they never block
//!   walking.
//! - **Shopfronts:** a dark stall riser under the front windows, a blank sign board along the front
//!   and a striped awning; offices get a canopy over the front door.
//!
//! Every piece carries the cutaway tag of what it belongs to: wall details are tagged with their
//! wall's anchor so they drop with it to the stub (vertical trim that crosses the stub height gets a
//! cap like the wall's); roof details, porch roofs and awnings are roofs (the see-through hole round
//! the player, and hidden with the upper storeys while you're inside); posts, railings, decks and
//! steps are never cut.
//!
//! Colours come from the palette file (`assets/data/palette.ron`, through [`super::palette`]).

use bevy::prelude::*;
use zombye_sim::map::{Building, Edge, Feature, Floor, OwnedEdge, RoofStyle, Side, TileMap};
use zombye_sim::template::{BuildingKind, BuildingLook, Porch};

use super::palette::{self, pal};
use super::{CutKind, CutTag, FaceColors, MeshBuilder, hash01, same, shade};
use crate::world_render::{SILL_LIFT, STOREY, STUB_HEIGHT, WALL_T, WINDOW_HEAD, WINDOW_SILL};

// Sizes, in metres.
/// Top of the foundation and of the water-table board above it (both under the 0.4 m stub).
const FOUNDATION_TOP: f32 = 0.30;
const STRIPE_TOP: f32 = 0.38;
/// Siding boards: a 5 cm shadow line at the foot of each board.
const BOARD: f32 = 0.30;
const BRICK_COURSE: f32 = 0.25;
const LINE: f32 = 0.05;
/// How far trim stands proud of the wall face.
const PROUD: f32 = 0.03;
/// The roof (as `world_render::add_roof` makes it).
const OVERHANG: f32 = 0.4;
const PITCH: f32 = 0.5;
/// Porch built onto the front: deck height, roof height at the wall and its fall per metre.
const DECK_TOP: f32 = 0.22;
const PORCH_ROOF_WALL: f32 = 2.75;
const PORCH_ROOF_FALL: f32 = 0.22;
/// Arch openings are clear up to here (their lintel above).
const ARCH_TOP: f32 = 2.4;

/// Is this room outside, as far as the outside of a wall goes: outdoors, or a porch.
pub fn outside(map: &TileMap, room: u16) -> bool {
    room == 0 || map.rooms[room as usize].room_type == "porch"
}

/// Is this outdoor tile under a porch built onto a house (where no flower bed goes)?
pub fn porch_covers(map: &TileMap, tile: IVec2) -> bool {
    map.buildings.iter().any(|b| {
        b.look.and_then(|l| l.porch).is_some_and(|p| p.room == 0 && tile.cmpge(p.min).all() && tile.cmplt(p.min + p.size).all() && yard_has_room(map, &p))
    })
}

/// A frame on the ground: `origin`, a unit `along` axis and a unit `out` axis (both along world x
/// or z), so boxes given in (along, height, out) are axis-aligned in the world.
#[derive(Clone, Copy)]
struct Frame {
    origin: Vec2,
    along: Vec2,
    out: Vec2,
}

impl Frame {
    fn at(&self, t: f32, o: f32) -> Vec2 {
        self.origin + self.along * t + self.out * o
    }

    fn world(&self, t: f32, y: f32, o: f32) -> Vec3 {
        let p = self.at(t, o);
        Vec3::new(p.x, y, p.y)
    }

    /// A box spanning `t0..t1` along, `y0..y1` up and `o0..o1` out.
    #[allow(clippy::too_many_arguments)]
    fn cube(&self, mb: &mut MeshBuilder, t0: f32, t1: f32, y0: f32, y1: f32, o0: f32, o1: f32, faces: FaceColors) {
        let (a, b) = (self.world(t0, y0, o0), self.world(t1, y1, o1));
        mb.add_box(a.min(b), a.max(b), faces);
    }

    /// A flat quad facing `out` at `o`, spanning `t0..t1` and `y0..y1`.
    #[allow(clippy::too_many_arguments)]
    fn face(&self, mb: &mut MeshBuilder, t0: f32, t1: f32, y0: f32, y1: f32, o: f32, c: LinearRgba) {
        let n = Vec3::new(self.out.x, 0.0, self.out.y);
        mb.quad_facing([self.world(t0, y0, o), self.world(t1, y0, o), self.world(t1, y1, o), self.world(t0, y1, o)], n, c);
    }

    fn normal(&self) -> Vec3 {
        Vec3::new(self.out.x, 0.0, self.out.y)
    }
}

/// A box lit from above: its top a touch lighter.
fn lit_box(c: LinearRgba) -> FaceColors {
    let mut f = same(c);
    f[2] = shade(c, 1.08);
    f
}

fn side_dir(s: Side) -> Vec2 {
    s.offset().as_vec2()
}

/// One ground or upper wall edge seen from one side: its frame (origin at the edge's start, along
/// the edge, out toward that side), base height, kind and cutaway anchor.
#[derive(Clone, Copy)]
struct WallSide {
    frame: Frame,
    base: f32,
    kind: Edge,
    anchor: Vec2,
}

fn wall_sides(owner: IVec2, which: OwnedEdge, kind: Edge, base: f32, rooms: (u16, u16), map: &TileMap) -> Vec<WallSide> {
    let (a, b) = TileMap::edge_segment(owner, which);
    let along = (b - a).normalize();
    let anchor = (a + b) * 0.5;
    // rooms.0 is on the north/west side (-along.perp for a north edge is -z).
    let back = match which {
        OwnedEdge::North => Vec2::NEG_Y,
        OwnedEdge::West => Vec2::NEG_X,
    };
    let mut out = Vec::new();
    for (room, dir) in [(rooms.0, back), (rooms.1, -back)] {
        if outside(map, room) {
            out.push(WallSide { frame: Frame { origin: a, along, out: dir }, base, kind, anchor });
        }
    }
    out
}

/// Height ranges of wall on an edge of this kind (as `world_render::add_wall` builds them).
fn pieces(kind: Edge) -> &'static [(f32, f32)] {
    match kind {
        Edge::Wall => &[(0.0, STOREY)],
        Edge::Window | Edge::WindowOpen | Edge::WindowBroken | Edge::WindowBoarded => &[(0.0, WINDOW_SILL), (WINDOW_HEAD, STOREY)],
        Edge::DoorClosed | Edge::DoorOpen | Edge::DoorBroken | Edge::DoorBoarded => &[(2.1, STOREY)],
        Edge::Arch => &[(ARCH_TOP, STOREY)],
        Edge::None | Edge::Fence => &[],
    }
}

fn is_door(kind: Edge) -> bool {
    matches!(kind, Edge::DoorClosed | Edge::DoorOpen | Edge::DoorBroken | Edge::DoorBoarded)
}

/// The kit for one building.
pub struct Kit<'a> {
    map: &'a TileMap,
    b: &'a Building,
    index: usize,
    look: BuildingLook,
    /// Siding (houses) or brick (shops, offices): the outside of every wall.
    pub wall: LinearRgba,
    trim: LinearRgba,
    line: LinearRgba,
    commercial: bool,
    /// The porch, if the template has one and (built onto the front) the yard has room for it.
    porch: Option<Porch>,
}

impl<'a> Kit<'a> {
    pub fn new(map: &'a TileMap, index: usize, b: &'a Building) -> Self {
        let look = b.look.unwrap_or(BuildingLook { kind: BuildingKind::House, front: Side::S, porch: None });
        let commercial = !matches!(look.kind, BuildingKind::House);
        let wall = if commercial { palette::pick(&pal().brick, b.siding as usize + 1) } else { super::siding_color(b.siding) };
        // Dark trim on the palest sidings (cream, butter), off-white on the rest.
        let trim = if commercial {
            pal().shopfront
        } else if matches!(b.siding % 8, 0 | 2) {
            pal().trim_dark
        } else {
            pal().trim_light
        };
        let line = shade(wall, if commercial { 0.84 } else { 0.78 });
        let porch = look.porch.filter(|p| p.room != 0 || yard_has_room(map, p));
        Kit { map, b, index, look, wall, trim, line, commercial, porch }
    }

    fn belongs(&self, room: u16) -> bool {
        room != 0 && self.map.rooms[room as usize].building as usize == self.index
    }

    /// The outside of one wall edge on one level: foundation and water table, siding lines, window
    /// frames and muntins, door casings. Added after `add_wall`, tagged like it.
    #[allow(clippy::too_many_arguments)]
    pub fn dress_wall(&self, mb: &mut MeshBuilder, owner: IVec2, which: OwnedEdge, kind: Edge, base: f32, rooms: (u16, u16)) {
        let (a, b) = TileMap::edge_segment(owner, which);
        let anchor = (a + b) * 0.5;
        mb.set_tag(CutTag { kind: CutKind::Wall, anchor, radius: 0.0 });
        let face = WALL_T;
        for side in wall_sides(owner, which, kind, base, rooms, self.map) {
            let f = side.frame;
            // A little past the edge's ends, as far as the wall box goes (to the outer corner).
            let (t0, t1) = (-WALL_T, 1.0 + WALL_T);
            let (step, first) = if self.commercial { (BRICK_COURSE, 0.35) } else { (BOARD, STRIPE_TOP + 0.02) };
            for &(h0, h1) in pieces(kind) {
                if base == 0.0 && h0 == 0.0 {
                    f.cube(mb, t0, t1, 0.0, FOUNDATION_TOP, face - 0.01, face + 0.025, lit_box(pal().foundation));
                    if !self.commercial {
                        f.cube(mb, t0, t1, FOUNDATION_TOP, STRIPE_TOP, face - 0.01, face + 0.02, lit_box(self.trim));
                    }
                }
                // Board lines (or brick courses) on this piece, aligned across the whole wall.
                let start = if base == 0.0 { first } else { 0.12 };
                let mut y = start;
                while y + LINE <= STOREY + 1e-3 {
                    if y >= h0 - 1e-3 && y + LINE <= h1 + 1e-3 {
                        f.face(mb, t0, t1, base + y, base + y + LINE, face + 0.006, self.line);
                    }
                    y += step;
                }
                // A belt board where an upper storey starts.
                if base > 0.0 && h0 == 0.0 {
                    f.cube(mb, t0, t1, base - 0.02, base + 0.08, face - 0.01, face + 0.02, lit_box(self.trim));
                }
            }
            if kind.is_window() {
                let (sill, head) = (base + WINDOW_SILL, base + WINDOW_HEAD);
                let c = lit_box(self.trim);
                f.cube(mb, 0.02, 0.12, sill, head, face - 0.01, face + PROUD, c);
                f.cube(mb, 0.88, 0.98, sill, head, face - 0.01, face + PROUD, c);
                f.cube(mb, -0.02, 1.02, head, head + 0.1, face - 0.01, face + PROUD + 0.01, c);
                f.cube(mb, -0.05, 1.05, sill - 0.07, sill + SILL_LIFT, face - 0.04, face + 0.07, c);
                if self.commercial && base == 0.0 && side_dir(self.look.front) == f.out && matches!(self.look.kind, BuildingKind::Shop) {
                    // A shopfront's stall riser under the glass.
                    f.cube(mb, -0.02, 1.02, FOUNDATION_TOP, sill - 0.07, face - 0.01, face + 0.03, lit_box(pal().shopfront));
                }
            } else if is_door(kind) {
                let c = lit_box(self.trim);
                self.vtrim(mb, &f, anchor, 0.0, 0.1, base, base + 2.1, face - 0.01, face + PROUD, c);
                self.vtrim(mb, &f, anchor, 0.9, 1.0, base, base + 2.1, face - 0.01, face + PROUD, c);
                f.cube(mb, -0.04, 1.04, base + 2.1, base + 2.22, face - 0.01, face + PROUD + 0.01, c);
            }
        }
        if kind.is_window() {
            // Muntins: a centre mullion and a meeting rail across, in the wall's middle.
            let f = Frame { origin: a, along: (b - a).normalize(), out: (b - a).normalize().perp() };
            let c = same(self.trim);
            f.cube(mb, 0.47, 0.53, base + WINDOW_SILL + SILL_LIFT, base + WINDOW_HEAD, -0.03, 0.03, c);
            f.cube(mb, 0.04, 0.96, base + 1.47, base + 1.53, -0.03, 0.03, c);
        }
        mb.set_tag(CutTag::default());
    }

    /// Vertical trim on a wall: tagged with the wall, with a cap where it's cut at the stub.
    #[allow(clippy::too_many_arguments)]
    fn vtrim(&self, mb: &mut MeshBuilder, f: &Frame, anchor: Vec2, t0: f32, t1: f32, y0: f32, y1: f32, o0: f32, o1: f32, c: FaceColors) {
        f.cube(mb, t0, t1, y0, y1, o0, o1, c);
        if y0 < STUB_HEIGHT && y1 > STUB_HEIGHT {
            mb.set_tag(CutTag { kind: CutKind::WallCap, anchor, radius: 0.0 });
            let (p, q) = (f.world(t0, STUB_HEIGHT, o0), f.world(t1, STUB_HEIGHT, o1));
            let (lo, hi) = (p.min(q), p.max(q));
            mb.quad_facing([Vec3::new(lo.x, lo.y, lo.z), Vec3::new(lo.x, lo.y, hi.z), Vec3::new(hi.x, lo.y, hi.z), Vec3::new(hi.x, lo.y, lo.z)], Vec3::Y, c[2]);
            mb.set_tag(CutTag { kind: CutKind::Wall, anchor, radius: 0.0 });
        }
    }

    /// Everything that isn't one wall: corner boards, the roof's details, the porch, steps and the
    /// shopfront. `ground` are the ground-floor edges `add_wall` drew (tile, edge, kind, rooms).
    pub fn finish(&self, ground: &[(IVec2, OwnedEdge, Edge, (u16, u16))], lower: &mut MeshBuilder, upper: &mut MeshBuilder, roof: &mut MeshBuilder) {
        let ground_sides: Vec<WallSide> = ground.iter().flat_map(|&(o, w, k, r)| wall_sides(o, w, k, 0.0, r, self.map)).collect();
        self.corners(lower, &ground_sides);
        let mut by_level: Vec<(i8, Vec<WallSide>)> = Vec::new();
        for (level, e) in &self.b.upper_edges {
            let sides = wall_sides(e.tile, e.edge, e.kind, STOREY * *level as f32, e.rooms, self.map);
            match by_level.iter_mut().find(|(l, _)| l == level) {
                Some((_, v)) => v.extend(sides),
                None => by_level.push((*level, sides)),
            }
        }
        for (_, sides) in &by_level {
            self.corners(upper, sides);
        }
        match self.b.roof {
            RoofStyle::Flat => self.flat_roof(roof),
            RoofStyle::GableX | RoofStyle::GableY => self.gable_roof(roof),
        }
        if let Some(p) = self.porch {
            if p.room == 0 {
                self.front_porch(lower, roof, &p);
            } else {
                self.room_porch(lower, &p, ground);
            }
        }
        self.steps(lower, ground);
        match self.look.kind {
            BuildingKind::Shop => self.shopfront(lower, roof, &ground_sides),
            BuildingKind::Office | BuildingKind::Civic => self.door_canopies(roof, ground),
            BuildingKind::House => {}
        }
    }

    /// Corner boards where two outside walls meet at an outside corner.
    fn corners(&self, mb: &mut MeshBuilder, sides: &[WallSide]) {
        let c = lit_box(if self.commercial { shade(self.wall, 0.9) } else { self.trim });
        let w = if self.commercial { 0.2 } else { 0.14 };
        for s in sides.iter().filter(|s| s.kind == Edge::Wall) {
            let f = s.frame;
            for (end, dir) in [(0.0, -1.0), (1.0, 1.0)] {
                let v = f.at(end, 0.0);
                // An outside corner: a wall meets this end whose outside faces the way this one runs on.
                let corner = sides.iter().any(|o| o.base == s.base && o.frame.out == f.along * dir && (o.frame.at(0.0, 0.0) == v || o.frame.at(1.0, 0.0) == v));
                if !corner {
                    continue;
                }
                let (t0, t1) = if dir > 0.0 { (1.0 + WALL_T + 0.025 - w, 1.0 + WALL_T + 0.025) } else { (-WALL_T - 0.025, -WALL_T - 0.025 + w) };
                let y0 = if s.base == 0.0 { STRIPE_TOP } else { s.base };
                mb.set_tag(CutTag { kind: CutKind::Wall, anchor: s.anchor, radius: 0.0 });
                self.vtrim(mb, &f, s.anchor, t0, t1, y0, s.base + STOREY, WALL_T - 0.01, WALL_T + 0.025, c);
            }
        }
        mb.set_tag(CutTag::default());
    }

    fn roof_tag(&self) -> CutTag {
        let b = self.b;
        let centre = b.min.as_vec2() + b.size.as_vec2() * 0.5;
        CutTag { kind: CutKind::Roof, anchor: centre, radius: b.size.max_element() as f32 * 0.5 + OVERHANG }
    }

    /// Gables, fascia, gutters, ridge cap, chimney, aerial and gable windows of a pitched roof.
    fn gable_roof(&self, mb: &mut MeshBuilder) {
        let b = self.b;
        let tag = self.roof_tag();
        mb.set_tag(tag);
        let eave = STOREY * b.levels as f32;
        // Ridge axis u, across it v: (u, y, v) to the world.
        let along_x = matches!(b.roof, RoofStyle::GableX);
        let w = |u: f32, y: f32, v: f32| if along_x { Vec3::new(u, y, v) } else { Vec3::new(v, y, u) };
        let wbox = |mb: &mut MeshBuilder, u0: f32, u1: f32, y0: f32, y1: f32, v0: f32, v1: f32, c: FaceColors| {
            let (p, q) = (w(u0, y0, v0), w(u1, y1, v1));
            mb.add_box(p.min(q), p.max(q), c);
        };
        let (min, size) = (b.min.as_vec2(), b.size.as_vec2());
        let (u0, u1, v0, v1) = if along_x { (min.x, min.x + size.x, min.y, min.y + size.y) } else { (min.y, min.y + size.y, min.x, min.x + size.x) };
        let vm = (v0 + v1) * 0.5;
        let o = OVERHANG;
        let ridge = eave + (vm - v0 + o) * PITCH;
        let roof_at = |v: f32| ridge - (v - vm).abs() * PITCH;
        let roof_c = super::roof_color(b.roof_color);
        let trim = lit_box(if self.commercial { pal().coping } else { self.trim });
        let un = |u: f32| if along_x { Vec3::new(u, 0.0, 0.0) } else { Vec3::new(0.0, 0.0, u) };

        // Gable walls up to the roof, at the wall's outer face, with board lines and a window.
        let wall_top = roof_at(v0 - WALL_T);
        for (uf, sign) in [(u0 - WALL_T, -1.0), (u1 + WALL_T, 1.0)] {
            let n = un(sign);
            let (va, vb) = (v0 - WALL_T, v1 + WALL_T);
            mb.quad_facing([w(uf, eave, va), w(uf, eave, vb), w(uf, wall_top, vb), w(uf, wall_top, va)], n, self.wall);
            mb.triangle_facing([w(uf, wall_top, va), w(uf, wall_top, vb), w(uf, ridge, vm)], n, self.wall);
            let uo = uf + sign * 0.006;
            let mut y = eave + 0.12;
            while y + LINE < ridge - 0.15 {
                let half = ((ridge - (y + LINE)) / PITCH).min(vm - va);
                if half > 0.1 {
                    mb.quad_facing([w(uo, y, vm - half), w(uo, y, vm + half), w(uo, y + LINE, vm + half), w(uo, y + LINE, vm - half)], n, self.line);
                }
                y += BOARD;
            }
            if !self.commercial && ridge - eave > 1.5 {
                // A small window up in the gable: frame, glass (lit at night in some), muntins.
                let (y0, y1, hw) = (eave + 0.5, eave + 1.1, 0.3);
                let u = |d: f32| uf + sign * d;
                wbox(mb, u(-0.01).min(u(PROUD)), u(-0.01).max(u(PROUD)), y0 - 0.08, y1 + 0.08, vm - hw - 0.08, vm + hw + 0.08, trim);
                if attic_lit(self.index, sign) {
                    mb.set_tag(CutTag { kind: CutKind::Lit, ..tag });
                }
                let ug = u(PROUD + 0.004);
                mb.quad_facing([w(ug, y0, vm - hw), w(ug, y0, vm + hw), w(ug, y1, vm + hw), w(ug, y1, vm - hw)], n, pal().gable_glass);
                mb.set_tag(tag);
                let um = (u(PROUD), u(PROUD + 0.02));
                wbox(mb, um.0.min(um.1), um.0.max(um.1), y0, y1, vm - 0.025, vm + 0.025, trim);
                wbox(mb, um.0.min(um.1), um.0.max(um.1), (y0 + y1) * 0.5 - 0.025, (y0 + y1) * 0.5 + 0.025, vm - hw, vm + hw, trim);
            }
        }

        // Fascia boards and gutters along both eaves.
        for (ve, sign) in [(v0 - o, -1.0f32), (v1 + o, 1.0)] {
            let v = |d: f32| ve + sign * d;
            let (a, c) = (v(-0.01), v(0.03));
            wbox(mb, u0 - o - 0.03, u1 + o + 0.03, eave - 0.18, eave + 0.01, a.min(c), a.max(c), trim);
            let (a, c) = (v(0.03), v(0.11));
            wbox(mb, u0 - o, u1 + o, eave - 0.15, eave - 0.05, a.min(c), a.max(c), lit_box(pal().rain_gutter));
        }
        // Fascia up the gables (rakes), hanging under the roof's edge.
        for (ue, sign) in [(u0 - o - 0.01, -1.0), (u1 + o + 0.01, 1.0)] {
            let n = un(sign);
            for (ve, vt) in [(v0 - o, vm), (v1 + o, vm)] {
                let (ye, yt) = (eave, ridge);
                mb.quad_facing([w(ue, ye + 0.02, ve), w(ue, yt + 0.02, vt), w(ue, yt - 0.18, vt), w(ue, ye - 0.18, ve)], n, trim[0]);
            }
        }
        // Ridge cap.
        wbox(mb, u0 - o - 0.03, u1 + o + 0.03, ridge - 0.08, ridge + 0.06, vm - 0.14, vm + 0.14, lit_box(shade(roof_c, 0.7)));

        if self.commercial {
            mb.set_tag(CutTag::default());
            return;
        }
        // A chimney on some houses, near one end, on one slope.
        let (bx, by) = (b.min.x, b.min.y);
        let end = hash01(bx, by, 81) < 0.5;
        let length = u1 - u0;
        if hash01(bx, by, 80) < 0.55 && length >= 5.0 {
            let uc = if end { u0 + 1.3 } else { u1 - 1.3 };
            let side = if hash01(bx, by, 82) < 0.5 { -1.0 } else { 1.0 };
            let vc = vm + side * 0.8;
            let (hu, hv) = (0.36, 0.3);
            let foot = roof_at(vc + side * hv) - 0.05;
            let top = ridge + 0.7;
            wbox(mb, uc - hu, uc + hu, foot, top, vc - hv, vc + hv, lit_box(pal().chimney));
            wbox(mb, uc - hu - 0.06, uc + hu + 0.06, top, top + 0.08, vc - hv - 0.06, vc + hv + 0.06, lit_box(pal().chimney_cap));
            wbox(mb, uc - 0.12, uc + 0.12, top + 0.08, top + 0.22, vc - 0.12, vc + 0.12, lit_box(pal().flue));
        }
        // A TV aerial on a thin mast on others, at the other end of the ridge.
        if hash01(bx, by, 83) < 0.4 && length >= 5.0 {
            let uc = if end { u1 - 1.6 } else { u0 + 1.6 };
            let c = lit_box(pal().aerial);
            let top = ridge + 1.6;
            wbox(mb, uc - 0.03, uc + 0.03, ridge - 0.05, top, vm - 0.03, vm + 0.03, c);
            wbox(mb, uc - 0.55, uc + 0.55, top - 0.1, top - 0.05, vm - 0.025, vm + 0.025, c);
            for (k, du) in [-0.45f32, -0.15, 0.15, 0.45].into_iter().enumerate() {
                let half = 0.45 - 0.08 * k as f32;
                wbox(mb, uc + du - 0.025, uc + du + 0.025, top - 0.1, top - 0.05, vm - half, vm + half, c);
            }
        }
        mb.set_tag(CutTag::default());
    }

    /// A parapet with coping round a flat roof, and a rooftop unit.
    fn flat_roof(&self, mb: &mut MeshBuilder) {
        let b = self.b;
        mb.set_tag(self.roof_tag());
        let top = STOREY * b.levels as f32 + 0.2;
        let (lo, hi) = (b.min.as_vec2() - Vec2::splat(0.1), (b.min + b.size).as_vec2() + Vec2::splat(0.1));
        let (h, t) = (0.55, 0.16);
        let wall = lit_box(self.wall);
        let ring = |mb: &mut MeshBuilder, y0: f32, y1: f32, inset: f32, thick: f32, c: FaceColors| {
            let (a, z) = (lo - Vec2::splat(inset), hi + Vec2::splat(inset));
            mb.add_box(Vec3::new(a.x, y0, a.y), Vec3::new(z.x, y1, a.y + thick), c);
            mb.add_box(Vec3::new(a.x, y0, z.y - thick), Vec3::new(z.x, y1, z.y), c);
            mb.add_box(Vec3::new(a.x, y0, a.y + thick), Vec3::new(a.x + thick, y1, z.y - thick), c);
            mb.add_box(Vec3::new(z.x - thick, y0, a.y + thick), Vec3::new(z.x, y1, z.y - thick), c);
        };
        // 2 cm proud of the slab, so their faces never share a plane.
        ring(mb, top - 0.22, top + h, 0.02, t + 0.02, wall);
        ring(mb, top + h, top + h + 0.07, 0.06, t + 0.1, lit_box(pal().coping));
        // A rooftop unit and a vent pipe toward the back, on roofs big enough.
        let size = b.size.as_vec2();
        if size.min_element() >= 6.0 {
            let back = -side_dir(self.look.front);
            let centre = b.min.as_vec2() + size * 0.5 + back * (size * 0.25);
            let r = hash01(b.min.x, b.min.y, 84) - 0.5;
            let c = centre + back.perp() * r * size.max_element() * 0.4;
            let unit = lit_box(pal().roof_unit);
            mb.add_box(Vec3::new(c.x - 0.7, top, c.y - 0.55), Vec3::new(c.x + 0.7, top + 0.85, c.y + 0.55), unit);
            mb.add_box(Vec3::new(c.x - 0.4, top + 0.85, c.y - 0.4), Vec3::new(c.x + 0.4, top + 0.95, c.y + 0.4), lit_box(pal().rain_gutter));
            let v = c + back.perp() * 1.6;
            mb.add_box(Vec3::new(v.x - 0.1, top, v.y - 0.1), Vec3::new(v.x + 0.1, top + 0.6, v.y + 0.1), lit_box(pal().rain_gutter));
        }
        mb.set_tag(CutTag::default());
    }

    /// Two steps up to every outside door with no porch (one low step on shops and offices).
    fn steps(&self, mb: &mut MeshBuilder, ground: &[(IVec2, OwnedEdge, Edge, (u16, u16))]) {
        let c = lit_box(pal().step);
        for &(owner, which, kind, rooms) in ground {
            if !is_door(kind) {
                continue;
            }
            for side in wall_sides(owner, which, kind, 0.0, rooms, self.map) {
                let f = side.frame;
                // The tile it opens onto: outdoors, not a porch room or under a built-on porch.
                let out_tile = (f.at(0.5, 0.5)).floor().as_ivec2();
                // (A porch room counts as outside for its walls, but its doors get no steps.)
                if self.map.tile(out_tile).room != 0 || self.under_porch(out_tile) {
                    continue;
                }
                if self.commercial {
                    f.cube(mb, -0.1, 1.1, 0.0, 0.1, WALL_T - 0.01, WALL_T + 0.35, c);
                } else {
                    f.cube(mb, -0.15, 1.15, 0.0, 0.1, WALL_T - 0.01, WALL_T + 0.62, c);
                    f.cube(mb, -0.15, 1.15, 0.1, 0.2, WALL_T - 0.01, WALL_T + 0.32, c);
                }
            }
        }
    }

    fn under_porch(&self, t: IVec2) -> bool {
        self.porch.is_some_and(|p| p.room == 0 && t.cmpge(p.min).all() && t.cmplt(p.min + p.size).all())
    }

    /// The porch's frame: along the house wall, out the way it faces; its length and depth.
    fn porch_frame(p: &Porch) -> (Frame, f32, f32) {
        let (min, size) = (p.min.as_vec2(), p.size.as_vec2());
        match p.facing {
            Side::S => (Frame { origin: min, along: Vec2::X, out: Vec2::Y }, size.x, size.y),
            Side::N => (Frame { origin: Vec2::new(min.x, min.y + size.y), along: Vec2::X, out: Vec2::NEG_Y }, size.x, size.y),
            Side::E => (Frame { origin: min, along: Vec2::Y, out: Vec2::X }, size.y, size.x),
            Side::W => (Frame { origin: Vec2::new(min.x + size.x, min.y), along: Vec2::Y, out: Vec2::NEG_X }, size.y, size.x),
        }
    }

    /// A porch built onto the front: deck, steps, posts, railing, and a shed roof.
    fn front_porch(&self, lower: &mut MeshBuilder, roof: &mut MeshBuilder, p: &Porch) {
        let (f, len, depth) = Self::porch_frame(p);
        // The way in: in line with a door in the wall behind, else the middle.
        let mut entry = (len * 0.5).floor() + 0.5;
        for i in 0..len as i32 {
            let porch_tile = f.at(i as f32 + 0.5, 0.5).floor().as_ivec2();
            let house = porch_tile - p.facing.offset();
            if is_door(self.map.edge(house, p.facing)) {
                entry = i as f32 + 0.5;
            }
        }
        lower.set_tag(CutTag { kind: CutKind::Structure, ..default() });
        let deck = pal().deck;
        let mut faces = same(shade(deck, 0.8));
        faces[2] = deck;
        f.cube(lower, 0.0, len, 0.0, DECK_TOP, WALL_T - 0.01, depth, faces);
        // Deck boards: lines across the deck, toward the street.
        let mut t = 0.3;
        while t < len - 0.1 {
            let (a, b) = (f.world(t, DECK_TOP + 0.004, WALL_T), f.world(t + 0.04, DECK_TOP + 0.004, depth));
            let (lo, hi) = (a.min(b), a.max(b));
            lower.quad_facing([Vec3::new(lo.x, lo.y, lo.z), Vec3::new(lo.x, lo.y, hi.z), Vec3::new(hi.x, lo.y, hi.z), Vec3::new(hi.x, lo.y, lo.z)], Vec3::Y, shade(deck, 0.82));
            t += 0.3;
        }
        // Steps down from the way in.
        let step = lit_box(pal().step);
        f.cube(lower, entry - 0.6, entry + 0.6, 0.0, DECK_TOP * 0.5, depth - 0.01, depth + 0.34, step);
        // Posts along the front, either side of the way in and at the corners, at most 2.6 m apart.
        let post_o = depth - 0.12;
        let roof_y = |o: f32| PORCH_ROOF_WALL - (o - WALL_T) * PORCH_ROOF_FALL;
        let mut posts = vec![0.12, len - 0.12];
        for t in [entry - 0.62, entry + 0.62] {
            if t > 0.5 && t < len - 0.5 {
                posts.push(t);
            }
        }
        posts.sort_by(f32::total_cmp);
        let mut i = 0;
        while i + 1 < posts.len() {
            let gap = posts[i + 1] - posts[i];
            if gap > 2.6 {
                let n = (gap / 2.6).ceil();
                for k in 1..n as i32 {
                    posts.push(posts[i] + gap * k as f32 / n);
                }
                posts.sort_by(f32::total_cmp);
            }
            i += 1;
        }
        let pale = lit_box(pal().trim_light);
        let beam_y = roof_y(depth) - 0.06;
        // Each post thins out by itself when it stands over the player (`CutKind::Fade`).
        for &t in &posts {
            lower.set_tag(CutTag { kind: CutKind::Fade, anchor: f.at(t, post_o), radius: 0.1 });
            f.cube(lower, t - 0.07, t + 0.07, DECK_TOP, beam_y, post_o - 0.07, post_o + 0.07, pale);
        }
        lower.set_tag(CutTag { kind: CutKind::Structure, ..default() });
        // Railing: along the front between posts but at the way in, and down both sides.
        for w in posts.windows(2) {
            if !(w[0] < entry && entry < w[1]) {
                self.railing(lower, &f, Vec2::new(w[0] + 0.07, post_o), Vec2::new(w[1] - 0.07, post_o), DECK_TOP);
            }
        }
        for t in [0.12, len - 0.12] {
            self.railing(lower, &f, Vec2::new(t, WALL_T + 0.02), Vec2::new(t, post_o - 0.07), DECK_TOP);
        }
        lower.set_tag(CutTag::default());

        // The roof: a shallow shed from the wall out past the posts, with a beam, fascia and soffit.
        // Built a stretch at a time (about a metre each), every stretch a `CutKind::Fade` piece of
        // its own: the ones standing over the player thin out, the rest of the porch roof stays.
        let (o0, o1) = (WALL_T, depth + 0.25);
        let (t0, t1) = (-0.25, len + 0.25);
        let (y0, y1) = (roof_y(o0), roof_y(o1));
        let up = (Vec3::Y + f.normal() * PORCH_ROOF_FALL).normalize();
        let roof_c = super::roof_color(self.b.roof_color);
        let soffit = shade(pal().trim_light, 0.85);
        let n = Vec3::new(f.along.x, 0.0, f.along.y);
        let stretches = (t1 - t0).round().max(1.0) as i32;
        for k in 0..stretches {
            let (a, b) = (t0 + (t1 - t0) * k as f32 / stretches as f32, t0 + (t1 - t0) * (k + 1) as f32 / stretches as f32);
            roof.set_tag(CutTag { kind: CutKind::Fade, anchor: f.at((a + b) * 0.5, (o0 + o1) * 0.5), radius: (o1 - o0).max(b - a) * 0.5 });
            roof.quad_facing([f.world(a, y0, o0), f.world(b, y0, o0), f.world(b, y1, o1), f.world(a, y1, o1)], up, roof_c);
            roof.quad_facing([f.world(a, y0 - 0.06, o0), f.world(b, y0 - 0.06, o0), f.world(b, y1 - 0.06, o1), f.world(a, y1 - 0.06, o1)], -up, soffit);
            f.cube(roof, a.max(0.0), b.min(len), beam_y - 0.16, beam_y, post_o - 0.08, post_o + 0.08, pale);
            f.cube(roof, a - if k == 0 { 0.02 } else { 0.0 }, b + if k + 1 == stretches { 0.02 } else { 0.0 }, y1 - 0.2, y1 + 0.02, o1 - 0.03, o1, lit_box(self.trim));
            for (t, s, end) in [(a, -1.0, k == 0), (b, 1.0, k + 1 == stretches)] {
                if end {
                    roof.quad_facing([f.world(t, y0 + 0.02, o0), f.world(t, y1 + 0.02, o1), f.world(t, y1 - 0.2, o1), f.world(t, y0 - 0.2, o0)], n * s, self.trim);
                }
            }
            // A ridge flashing where the porch roof meets the wall.
            f.cube(roof, a, b, y0 - 0.02, y0 + 0.06, WALL_T - 0.01, WALL_T + 0.08, lit_box(shade(roof_c, 0.7)));
        }
        roof.set_tag(CutTag::default());
    }

    /// A railing from `a` to `b` (in the frame's (along, out) metres) on a floor at `floor`: top
    /// and bottom rails and square balusters.
    fn railing(&self, mb: &mut MeshBuilder, f: &Frame, a: Vec2, b: Vec2, floor: f32) {
        let len = (b - a).length();
        if len < 0.15 {
            return;
        }
        let dir = (b - a) / len;
        let c = lit_box(pal().trim_light);
        // A bar from `s0` to `s1` along the railing, `half` either side of it.
        let part = |mb: &mut MeshBuilder, s0: f32, s1: f32, y0: f32, y1: f32, half: f32| {
            let side = dir.perp() * half;
            let (p, q) = (a + dir * s0 - side, a + dir * s1 + side);
            f.cube(mb, p.x.min(q.x), p.x.max(q.x), y0, y1, p.y.min(q.y), p.y.max(q.y), c);
        };
        part(mb, 0.0, len, floor + 0.74, floor + 0.81, 0.04);
        part(mb, 0.0, len, floor + 0.08, floor + 0.14, 0.03);
        let n = (len / 0.2).round().max(1.0) as i32;
        for k in 0..n {
            let s = (k as f32 + 0.5) * len / n as f32;
            part(mb, s - 0.025, s + 0.025, floor + 0.14, floor + 0.74, 0.025);
        }
    }

    /// A porch room under the main roof: posts along its open sides, a railing across its arches
    /// but the way in.
    fn room_porch(&self, mb: &mut MeshBuilder, p: &Porch, ground: &[(IVec2, OwnedEdge, Edge, (u16, u16))]) {
        let map = self.map;
        // Its arches to the outside, as (start, end) points on the wall line.
        let mut open: Vec<(Vec2, Vec2, IVec2)> = Vec::new();
        let mut door: Option<Vec2> = None;
        for &(owner, which, kind, rooms) in ground {
            let (a, b) = TileMap::edge_segment(owner, which);
            let porch_side = rooms.0 == p.room || rooms.1 == p.room;
            if !porch_side {
                continue;
            }
            let other = if rooms.0 == p.room { rooms.1 } else { rooms.0 };
            let outer = if rooms.0 == p.room { owner } else { owner - if which == OwnedEdge::North { IVec2::Y } else { IVec2::X } };
            if kind == Edge::Arch && other == 0 {
                open.push((a, b, outer));
            } else if is_door(kind) && self.belongs(other) {
                door = Some((a + b) * 0.5);
            }
        }
        if open.is_empty() {
            return;
        }
        // The way in: an arch onto a garden path, else the one on the porch's open face nearest the door.
        let facing = side_dir(p.facing);
        let on_face = |e: &(Vec2, Vec2, IVec2)| {
            let mid = (e.0 + e.1) * 0.5;
            let centre = p.min.as_vec2() + p.size.as_vec2() * 0.5;
            (mid - centre).dot(facing) > 0.0 && (e.1 - e.0).dot(facing).abs() < 0.5
        };
        let mut entry = open.iter().position(|e| map.tile(e.2).floor == Floor::Path && map.tile(e.2).room == 0);
        if entry.is_none() {
            let target = door.unwrap_or(p.min.as_vec2() + p.size.as_vec2() * 0.5);
            let key = |e: &(Vec2, Vec2, IVec2)| {
                let along = (e.1 - e.0).normalize();
                ((e.0 + e.1) * 0.5 - target).dot(along).abs() + if on_face(e) { 0.0 } else { 100.0 }
            };
            entry = (0..open.len()).min_by(|&i, &j| key(&open[i]).total_cmp(&key(&open[j])));
        }
        mb.set_tag(CutTag { kind: CutKind::Structure, ..default() });
        let pale = lit_box(pal().trim_light);
        // Posts at the ends and corners of the open sides, and every other metre along them.
        let mut verts: Vec<Vec2> = open.iter().flat_map(|e| [e.0, e.1]).collect();
        verts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
        verts.dedup();
        let id = Frame { origin: Vec2::ZERO, along: Vec2::X, out: Vec2::Y };
        for v in verts {
            let at: Vec<Vec2> = open.iter().filter(|e| e.0 == v || e.1 == v).map(|e| e.1 - e.0).collect();
            let inner = at.len() == 2 && at[0].dot(at[1]).abs() > 0.5;
            if inner && (v.x + v.y) as i32 % 2 != 0 {
                continue;
            }
            mb.set_tag(CutTag { kind: CutKind::Fade, anchor: v, radius: 0.1 });
            id.cube(mb, v.x - 0.08, v.x + 0.08, 0.0, ARCH_TOP, v.y - 0.08, v.y + 0.08, pale);
        }
        mb.set_tag(CutTag { kind: CutKind::Structure, ..default() });
        for (i, e) in open.iter().enumerate() {
            if Some(i) == entry {
                continue;
            }
            let dir = (e.1 - e.0).normalize();
            self.railing(mb, &id, e.0 + dir * 0.08, e.1 - dir * 0.08, 0.0);
        }
        mb.set_tag(CutTag::default());
    }

    /// A shopfront: a blank sign board along the front and a striped awning over it.
    fn shopfront(&self, lower: &mut MeshBuilder, roof: &mut MeshBuilder, sides: &[WallSide]) {
        let front = side_dir(self.look.front);
        let sign = pal().sign;
        let pick = hash01(self.b.min.x, self.b.min.y, 85);
        let awning = palette::pick(&pal().awning, (pick * pal().awning.len() as f32) as usize);
        let bone = pal().bone;
        for s in sides.iter().filter(|s| s.frame.out == front) {
            let f = s.frame;
            lower.set_tag(CutTag { kind: CutKind::Wall, anchor: s.anchor, radius: 0.0 });
            f.cube(lower, -WALL_T, 1.0 + WALL_T, 2.32, 2.96, WALL_T - 0.01, WALL_T + 0.05, lit_box(pal().shopfront));
            f.cube(lower, -WALL_T, 1.0 + WALL_T, 2.38, 2.90, WALL_T, WALL_T + 0.07, lit_box(sign));
            lower.set_tag(CutTag::default());
            // The awning: stripes 0.5 m wide by their place along the street, so they line up.
            // Each metre of it thins out by itself when it stands over the player.
            let (y_wall, y_edge, reach) = (2.3, 2.02, 1.25);
            roof.set_tag(CutTag { kind: CutKind::Fade, anchor: f.at(0.5, WALL_T + reach * 0.5), radius: 0.65 });
            // Wall frames run along +x or +z, so the stripe a point is in follows from its place
            // along that axis.
            let start = f.origin.dot(f.along);
            let up = (Vec3::Y + f.normal() * ((y_wall - y_edge) / reach)).normalize();
            let mut t = -WALL_T;
            while t < 1.0 + WALL_T - 1e-3 {
                let k = ((start + t) / 0.5 + 1e-4).floor();
                let t1 = ((k + 1.0) * 0.5 - start).min(1.0 + WALL_T);
                let c = if (k as i32).rem_euclid(2) == 0 { awning } else { bone };
                roof.quad_facing([f.world(t, y_wall, WALL_T), f.world(t1, y_wall, WALL_T), f.world(t1, y_edge, WALL_T + reach), f.world(t, y_edge, WALL_T + reach)], up, c);
                f.face(roof, t, t1, y_edge - 0.22, y_edge, WALL_T + reach, shade(c, 0.92));
                t = t1;
            }
            roof.set_tag(CutTag::default());
        }
    }

    /// A flat canopy over each front door of an office.
    fn door_canopies(&self, roof: &mut MeshBuilder, ground: &[(IVec2, OwnedEdge, Edge, (u16, u16))]) {
        for &(owner, which, kind, rooms) in ground {
            if !is_door(kind) {
                continue;
            }
            for s in wall_sides(owner, which, kind, 0.0, rooms, self.map) {
                if self.map.tile(s.frame.at(0.5, 0.5).floor().as_ivec2()).room == 0 {
                    roof.set_tag(CutTag { kind: CutKind::Fade, anchor: s.frame.at(0.5, 0.6), radius: 0.95 });
                    s.frame.cube(roof, -0.45, 1.45, 2.42, 2.56, WALL_T - 0.01, WALL_T + 1.1, lit_box(pal().coping));
                }
            }
        }
        roof.set_tag(CutTag::default());
    }
}

/// Is the attic lit behind the gable window on this end? About one house in six.
fn attic_lit(index: usize, sign: f32) -> bool {
    hash01(index as i32, sign as i32, 72) < 0.16
}

/// Room in the yard for a porch built onto the front: open ground (no room, tree, bush, rock,
/// fence or wall) under it and a step in front of it.
fn yard_has_room(map: &TileMap, p: &Porch) -> bool {
    let f = p.facing.offset();
    let (lo, hi) = (p.min.min(p.min + f), (p.min + p.size - IVec2::ONE).max(p.min + p.size - IVec2::ONE + f));
    for y in lo.y..=hi.y {
        for x in lo.x..=hi.x {
            let t = IVec2::new(x, y);
            let tile = map.tile(t);
            if !map.in_bounds(t) || tile.room != 0 || tile.feature != Feature::None || matches!(tile.floor, Floor::Asphalt | Floor::RoadLine | Floor::Sidewalk) {
                return false;
            }
            if x < hi.x && map.edge(t, Side::E) != Edge::None || y < hi.y && map.edge(t, Side::S) != Edge::None {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::furniture_models::FurnitureModels;
    use crate::world_render::build_building;

    fn town() -> TileMap {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let templates = zombye_net::content::load_building_templates(&data).expect("templates");
        zombye_sim::worldgen::generate(&zombye_sim::worldgen::GenConfig::default(), &templates).map
    }

    /// Every detail is tagged so the cutaway treats it like what it belongs to: wall details carry
    /// their wall's anchor (the middle of a tile edge) and drop with it, everything in a roof mesh
    /// is roof (or lit glass), and porches get drawn.
    #[test]
    fn details_drop_with_their_walls_and_roofs() {
        let map = town();
        let furniture = FurnitureModels::default();
        let mut porches = 0;
        for (i, b) in map.buildings.iter().enumerate() {
            let m = build_building(&map, i, b, &furniture);
            for k in &m.roof.uv1 {
                let kind = k[0] as u32;
                assert!(kind == CutKind::Roof as u32 || kind == CutKind::Lit as u32 || kind == CutKind::Fade as u32, "{}: a roof piece of kind {kind}", b.template);
            }
            for mb in [&m.lower, &m.upper] {
                for (uv, k) in mb.uv0.iter().zip(&mb.uv1) {
                    if k[0] as u32 == CutKind::Wall as u32 || k[0] as u32 == CutKind::WallCap as u32 {
                        let (fx, fy) = (uv[0].rem_euclid(1.0), uv[1].rem_euclid(1.0));
                        let mid = (fx == 0.5 && fy == 0.0) || (fx == 0.0 && fy == 0.5);
                        assert!(mid, "{}: wall detail anchored at {uv:?}, not an edge's middle", b.template);
                    }
                }
            }
            if b.look.and_then(|l| l.porch).is_some() && m.lower.uv1.iter().any(|k| k[0] as u32 == CutKind::Structure as u32) {
                porches += 1;
            }
        }
        assert!(porches > 10, "only {porches} porches drawn");
    }

    /// Quads and triangles face the normal they're given, whatever order their corners come in.
    #[test]
    fn faces_are_wound_to_their_normals() {
        let mut mb = MeshBuilder::default();
        let c = LinearRgba::WHITE;
        let corners = [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 0.0, 1.0), Vec3::Z];
        mb.quad_facing(corners, Vec3::Y, c);
        mb.quad_facing(corners, Vec3::NEG_Y, c);
        mb.triangle_facing([Vec3::ZERO, Vec3::X, Vec3::Z], Vec3::Y, c);
        for tri in mb.indices.chunks(3) {
            let p = |i: u32| Vec3::from(mb.positions[i as usize]);
            let geo = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
            let n = Vec3::from(mb.normals[tri[0] as usize]);
            assert!(geo.dot(n) > 0.0, "a face wound against its normal {n}");
        }
    }
}
