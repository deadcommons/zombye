//! Procedural low-poly geometry: boxes with per-face vertex colours, merged into one mesh per
//! chunk or building. All of M1's buildings, furniture and props are grey-box geometry built here;
//! generated art replaces pieces later without changing how the world is assembled.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use zombye_sim::map::{Floor, Side};
use zombye_sim::terrain::{Facet, Terrain};

use palette::{between, pal};

pub mod building_kit;
pub mod curves;
pub mod decay;
pub mod landscape;
#[cfg(test)]
pub mod overlap;
pub mod palette;
pub mod street_props;
pub mod trees;

/// Is `q` inside facet `f` (seen from above), its edges included?
fn in_facet(f: &Facet, q: Vec2) -> bool {
    let (a, b, c) = (f[0].xz(), f[1].xz(), f[2].xz());
    let s = (b - a).perp_dot(c - a).signum();
    [(a, b), (b, c), (c, a)].iter().all(|(u, v)| (*v - *u).perp_dot(q - *u) * s >= -1e-4)
}

/// A facet's normal, facing up.
fn facet_normal(f: &Facet) -> Vec3 {
    let n = (f[1] - f[0]).cross(f[2] - f[0]).normalize_or(Vec3::Y);
    if n.y < 0.0 { -n } else { n }
}

/// The convex polygon `poly` (seen from above) cut to what lies over facet `f`.
fn clip_to_facet(mut poly: Vec<Vec2>, f: &Facet) -> Vec<Vec2> {
    let (a, b, c) = (f[0].xz(), f[1].xz(), f[2].xz());
    let s = (b - a).perp_dot(c - a).signum();
    for (u, v) in [(a, b), (b, c), (c, a)] {
        let side = |q: Vec2| (v - u).perp_dot(q - u) * s;
        let mut out = Vec::with_capacity(poly.len() + 2);
        for k in 0..poly.len() {
            let (p, q) = (poly[k], poly[(k + 1) % poly.len()]);
            let (sp, sq) = (side(p), side(q));
            if sp >= 0.0 {
                out.push(p);
            }
            if (sp >= 0.0) != (sq >= 0.0) {
                out.push(p + (q - p) * (sp / (sp - sq)));
            }
        }
        poly = out;
        if poly.len() < 3 {
            break;
        }
    }
    // Drop corners that came out (nearly) on top of each other.
    poly.dedup_by(|q, p| q.distance_squared(*p) < 1e-10);
    if poly.len() > 1 && poly[0].distance_squared(poly[poly.len() - 1]) < 1e-10 {
        poly.pop();
    }
    poly
}

/// Barycentric weights over a triangle seen from above.
struct Bary {
    a: Vec2,
    e1: Vec2,
    e2: Vec2,
    inv: f32,
}

impl Bary {
    fn new(p: [Vec2; 3]) -> Self {
        let (e1, e2) = (p[1] - p[0], p[2] - p[0]);
        Self { a: p[0], e1, e2, inv: 1.0 / e1.perp_dot(e2) }
    }

    fn at(&self, q: Vec2) -> [f32; 3] {
        let d = q - self.a;
        let v = d.perp_dot(self.e2) * self.inv;
        let w = self.e1.perp_dot(d) * self.inv;
        [1.0 - v - w, v, w]
    }
}

/// The ground's colour on a facet: on a slope it drifts toward bare earth, and on the steepest
/// toward pale rock, as grass gives way on a bank.
fn slope_tint(c: LinearRgba, normal: Vec3) -> LinearRgba {
    let grade = normal.xz().length() / normal.y.max(1e-3);
    let earth = smooth(0.06, 0.12, grade);
    let rock = smooth(0.12, 0.20, grade);
    let p = pal();
    c.mix(&p.dirt, 0.45 * earth).mix(&p.rock, 0.55 * rock)
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What a piece of geometry is, for the cutaway shader (stored in the second UV channel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CutKind {
    /// Never cut: ground, floors, furniture, trunks, fences.
    Never = 0,
    /// A wall piece: drops to a stub as a whole when it's on the camera side of the player.
    Wall = 1,
    /// A roof: faded out whole with its building's upper storeys while the player is inside
    /// (`world_render::hide_upper_floors`); the shader only counts it as outdoors for the fog.
    Roof = 2,
    /// A tree's foliage: fades out when it hides the player.
    Canopy = 3,
    /// The flat top of a wall stub: only drawn while its wall is cut.
    WallCap = 4,
    /// Other building structure (door leaves): never cut by the shader, but like walls it stays
    /// readable in unseen rooms.
    Structure = 5,
    /// A tree's trunk and branches: dimmed and thinned (not removed) while its foliage fades.
    Wood = 6,
    /// Window glass of a room with a light on (`lights::LitRooms`): like `Structure`, and it
    /// glows warm amber at night (world.wesl).
    Lit = 7,
    /// A building detail that can stand over the player from outside (a porch roof, an awning, a
    /// door canopy, a porch post): thinned out whole, in a stipple, while it stands between the
    /// camera and the player (`cutaway::object_fade`; its anchor is its middle on the ground, its
    /// radius its half-width). Outdoors for the fog, like a roof.
    Fade = 8,
}

/// Cutaway data shared by all vertices of one piece: its kind, an anchor point on the ground
/// (wall midpoint, roof or canopy centre) and a radius for roofs and canopies.
#[derive(Clone, Copy, Debug)]
pub struct CutTag {
    pub kind: CutKind,
    pub anchor: Vec2,
    pub radius: f32,
}

impl Default for CutTag {
    fn default() -> Self {
        Self { kind: CutKind::Never, anchor: Vec2::ZERO, radius: 0.0 }
    }
}

#[derive(Default)]
pub struct MeshBuilder {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    /// UV0 = cut anchor (x, z); UV1 = (cut kind, radius).
    uv0: Vec<[f32; 2]>,
    uv1: Vec<[f32; 2]>,
    indices: Vec<u32>,
    tag: CutTag,
    /// The lighter copy drawn when zoomed far out ([`MeshBuilder::with_far`]), if one is kept.
    far: Option<Box<MeshBuilder>>,
    /// Which of the two copies geometry added now goes to.
    detail: Detail,
    /// Whether anything went to one copy only: without that the far copy is the same mesh.
    far_differs: bool,
}

/// Which copy of a mesh geometry goes to (see [`MeshBuilder::with_far`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Detail {
    #[default]
    Both,
    /// Only the full mesh: a model at full detail.
    Near,
    /// Only the far copy: the model's lighter stand-in.
    Far,
}

/// Face order for [`MeshBuilder::add_box`]: +x, -x, +y, -y, +z, -z.
pub type FaceColors = [LinearRgba; 6];

pub fn same(c: LinearRgba) -> FaceColors {
    [c; 6]
}

impl MeshBuilder {
    /// A builder with room for about this many triangles.
    pub fn with_capacity(triangles: usize) -> Self {
        let v = triangles * 3;
        Self {
            positions: Vec::with_capacity(v),
            normals: Vec::with_capacity(v),
            colors: Vec::with_capacity(v),
            uv0: Vec::with_capacity(v),
            uv1: Vec::with_capacity(v),
            indices: Vec::with_capacity(v),
            tag: CutTag::default(),
            far: None,
            detail: Detail::Both,
            far_differs: false,
        }
    }

    /// Also keep a far copy of this mesh: everything added goes to both, but what's added between
    /// [`MeshBuilder::only`]`(Detail::Near)` and `only(Detail::Far)` to one of them, so a heavy model
    /// can have a lighter stand-in drawn when the camera is zoomed far out
    /// (`world_render::swap_detail`). A builder without one ignores `Detail::Far` geometry.
    pub fn with_far(mut self) -> Self {
        self.far = Some(Box::default());
        self
    }

    /// Geometry added from now on goes to this copy (`Detail::Both` again afterwards).
    pub fn only(&mut self, detail: Detail) {
        self.detail = detail;
        if detail != Detail::Both && self.far.is_some() {
            self.far_differs = true;
        }
    }

    /// The far copy, if one was kept and it differs from the full mesh.
    pub fn take_far(&mut self) -> Option<MeshBuilder> {
        let far = self.far.take()?;
        self.far_differs.then_some(*far)
    }

    /// Passes what's being added on to the far copy; true when the full mesh is to take it too.
    fn mirror(&mut self, add: impl FnOnce(&mut MeshBuilder)) -> bool {
        if self.detail != Detail::Near
            && let Some(far) = &mut self.far
        {
            far.tag = self.tag;
            add(far);
        }
        self.detail != Detail::Far
    }

    #[cfg(test)]
    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Geometry added from now on carries this cutaway tag.
    pub fn set_tag(&mut self, tag: CutTag) {
        self.tag = tag;
    }

    fn push_vertex(&mut self, p: Vec3, n: Vec3, c: LinearRgba) {
        self.positions.push(p.to_array());
        self.normals.push(n.to_array());
        self.colors.push(c.to_f32_array());
        self.uv0.push(self.tag.anchor.to_array());
        self.uv1.push([self.tag.kind as u32 as f32, self.tag.radius]);
    }

    pub fn quad(&mut self, corners: [Vec3; 4], normal: Vec3, color: LinearRgba) {
        if !self.mirror(|far| far.quad(corners, normal, color)) {
            return;
        }
        let base = self.positions.len() as u32;
        for c in corners {
            self.push_vertex(c, normal, color);
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// A quad wound to face `normal`, whichever order its corners come in. The building material
    /// is double-sided and lights a face seen from behind with its normal flipped, so a face
    /// wound against its normal would look unlit.
    pub fn quad_facing(&mut self, mut corners: [Vec3; 4], normal: Vec3, color: LinearRgba) {
        if (corners[1] - corners[0]).cross(corners[2] - corners[0]).dot(normal) < 0.0 {
            corners.reverse();
        }
        self.quad(corners, normal, color);
    }

    /// A triangle wound to face `normal` (see [`MeshBuilder::quad_facing`]).
    pub fn triangle_facing(&mut self, mut corners: [Vec3; 3], normal: Vec3, color: LinearRgba) {
        if (corners[1] - corners[0]).cross(corners[2] - corners[0]).dot(normal) < 0.0 {
            corners.reverse();
        }
        self.triangle(corners, color);
    }

    /// A triangle with its own vertex normals (imported models).
    pub fn triangle_with_normals(&mut self, corners: [Vec3; 3], normals: [Vec3; 3], color: LinearRgba) {
        self.triangle_with_normals_shaded(corners, normals, [color; 3]);
    }

    /// The same with a colour at each corner, blended across it.
    pub fn triangle_with_normals_shaded(&mut self, corners: [Vec3; 3], normals: [Vec3; 3], colors: [LinearRgba; 3]) {
        if !self.mirror(|far| far.triangle_with_normals_shaded(corners, normals, colors)) {
            return;
        }
        let base = self.positions.len() as u32;
        for ((c, n), col) in corners.into_iter().zip(normals).zip(colors) {
            self.push_vertex(c, n, col);
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    /// A triangle with a colour at each corner, blended across it.
    pub fn triangle_shaded(&mut self, corners: [Vec3; 3], colors: [LinearRgba; 3]) {
        if !self.mirror(|far| far.triangle_shaded(corners, colors)) {
            return;
        }
        let normal = (corners[1] - corners[0]).cross(corners[2] - corners[0]).normalize_or_zero();
        let base = self.positions.len() as u32;
        for (c, col) in corners.into_iter().zip(colors) {
            self.push_vertex(c, normal, col);
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    /// Smallest and largest corner of everything added so far.
    #[cfg(test)]
    pub fn bounds(&self) -> (Vec3, Vec3) {
        self.positions.iter().map(|p| Vec3::from(*p)).fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(p), hi.max(p)))
    }

    pub fn triangle(&mut self, corners: [Vec3; 3], color: LinearRgba) {
        if !self.mirror(|far| far.triangle(corners, color)) {
            return;
        }
        let normal = (corners[1] - corners[0]).cross(corners[2] - corners[0]).normalize_or_zero();
        let base = self.positions.len() as u32;
        for c in corners {
            self.push_vertex(c, normal, color);
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    /// Axis-aligned box with one colour per face.
    pub fn add_box(&mut self, min: Vec3, max: Vec3, colors: FaceColors) {
        let c = |x: bool, y: bool, z: bool| Vec3::new(if x { max.x } else { min.x }, if y { max.y } else { min.y }, if z { max.z } else { min.z });
        // Counter-clockwise when seen from outside.
        self.quad([c(true, false, false), c(true, true, false), c(true, true, true), c(true, false, true)], Vec3::X, colors[0]);
        self.quad([c(false, false, true), c(false, true, true), c(false, true, false), c(false, false, false)], Vec3::NEG_X, colors[1]);
        self.quad([c(false, true, false), c(false, true, true), c(true, true, true), c(true, true, false)], Vec3::Y, colors[2]);
        self.quad([c(false, false, true), c(false, false, false), c(true, false, false), c(true, false, true)], Vec3::NEG_Y, colors[3]);
        self.quad([c(true, false, true), c(true, true, true), c(false, true, true), c(false, false, true)], Vec3::Z, colors[4]);
        self.quad([c(false, false, false), c(false, true, false), c(true, true, false), c(true, false, false)], Vec3::NEG_Z, colors[5]);
    }

    /// Lay what was built on flat ground over the terrain (`zombye_sim::terrain`): every vertex
    /// rises by the ground's height under it, and every triangle that would cross a fold between
    /// two facets is cut along it, so the ground is faceted exactly as the sim's [`Terrain::height`]
    /// and paint and clutter lying on it stay on it. Faces lying flat take their facet's normal (the
    /// low-poly look) and, on a slope, its colour drifts to bare earth and rock (`slope_tint`).
    /// Nothing changes where the ground is level (all of town).
    pub fn drape(&mut self, t: &Terrain) {
        if let Some(far) = &mut self.far {
            far.drape(t);
        }
        if t.is_flat() {
            // Level everywhere: the ground's height (the first UVs, below) is 0 all over.
            self.uv0.iter_mut().for_each(|uv| *uv = [0.0, 0.0]);
            return;
        }
        let n = self.positions.len();
        let lifted: Vec<[f32; 3]> = self.positions.iter().map(|p| [p[0], p[1] + t.height(Vec2::new(p[0], p[2])), p[2]]).collect();
        let old_pos = std::mem::replace(&mut self.positions, lifted);
        let indices = std::mem::take(&mut self.indices);
        let mut keep = Vec::with_capacity(indices.len());
        // A vertex's facet normal and tint, once a flat face on one facet has given it one.
        let mut facet_of: Vec<Option<Vec3>> = vec![None; n];
        let mut cut: Vec<[u32; 3]> = Vec::new();
        // The colours as built, before any tint: a piece cut across a fold is tinted from these.
        let built = self.colors.clone();
        for tri in indices.chunks_exact(3) {
            let ids = [tri[0], tri[1], tri[2]];
            let p = ids.map(|i| Vec3::from(old_pos[i as usize]));
            let (min, max) = (p[0].xz().min(p[1].xz()).min(p[2].xz()), p[0].xz().max(p[1].xz()).max(p[2].xz()));
            let area = (p[1].xz() - p[0].xz()).perp_dot(p[2].xz() - p[0].xz());
            let flat = Vec3::from(self.normals[ids[0] as usize]).y > 0.7;
            if area.abs() < 1e-7 || t.level_over(min, max).is_some() {
                // Standing up (a kerb's face, a post), or on level ground: each corner simply rises.
                keep.extend_from_slice(&ids);
                continue;
            }
            let f = t.facet_at((p[0].xz() + p[1].xz() + p[2].xz()) / 3.0);
            if p.iter().all(|q| in_facet(&f, q.xz())) {
                let mut ids = ids;
                if flat {
                    let normal = facet_normal(&f);
                    for i in ids.iter_mut() {
                        match facet_of[*i as usize] {
                            // A quad whose diagonal is the fold has its two halves on two facets:
                            // the corners they share are split, so each half keeps its own normal.
                            Some(other) if (other - normal).length_squared() > 1e-8 => {
                                let k = *i as usize;
                                self.positions.push(self.positions[k]);
                                self.normals.push(self.normals[k]);
                                self.colors.push(built[k]);
                                self.uv0.push(self.uv0[k]);
                                self.uv1.push(self.uv1[k]);
                                facet_of.push(Some(normal));
                                *i = self.positions.len() as u32 - 1;
                            }
                            _ => facet_of[*i as usize] = Some(normal),
                        }
                    }
                }
                keep.extend_from_slice(&ids);
            } else {
                cut.push(ids);
            }
        }
        for (i, normal) in facet_of.iter().enumerate() {
            if let Some(normal) = normal {
                self.normals[i] = normal.to_array();
                self.colors[i] = slope_tint(LinearRgba::from_f32_array(self.colors[i]), *normal).to_f32_array();
            }
        }
        self.indices = keep;
        // The ground material has no cutaway, so its first UVs are free: they carry the height of
        // the ground under each vertex, for the shader to tell height above the ground (world.wesl).
        for (k, p) in self.positions.iter().enumerate() {
            self.uv0[k] = [t.height(Vec2::new(p[0], p[2])), 0.0];
        }
        // The triangles across a fold, cut into a piece on each facet they cross.
        for ids in cut {
            let p = ids.map(|i| Vec3::from(old_pos[i as usize]));
            let flat = Vec3::from(self.normals[ids[0] as usize]).y > 0.7;
            let (min, max) = (p[0].xz().min(p[1].xz()).min(p[2].xz()), p[0].xz().max(p[1].xz()).max(p[2].xz()));
            let (lo, hi) = t.squares(min, max);
            let bary = Bary::new(p.map(|q| q.xz()));
            for j in lo.y..=hi.y {
                for i in lo.x..=hi.x {
                    for f in t.facets(i, j) {
                        let poly = clip_to_facet(vec![p[0].xz(), p[1].xz(), p[2].xz()], &f);
                        if poly.len() < 3 {
                            continue;
                        }
                        let normal = facet_normal(&f);
                        let base = self.positions.len() as u32;
                        for q in &poly {
                            let w = bary.at(*q);
                            let y = p[0].y * w[0] + p[1].y * w[1] + p[2].y * w[2] + t.height(*q);
                            let mut c = [0.0f32; 4];
                            for (k, id) in ids.iter().enumerate() {
                                let col = built[*id as usize];
                                for (ch, v) in c.iter_mut().zip(col) {
                                    *ch += v * w[k];
                                }
                            }
                            let (nrm, col) = if flat { (normal, slope_tint(LinearRgba::from_f32_array(c), normal).to_f32_array()) } else { (Vec3::from(self.normals[ids[0] as usize]), c) };
                            self.positions.push([q.x, y, q.y]);
                            self.normals.push(nrm.to_array());
                            self.colors.push(col);
                            self.uv0.push([t.height(*q), 0.0]);
                            self.uv1.push(self.uv1[ids[0] as usize]);
                        }
                        for k in 1..poly.len() as u32 - 1 {
                            self.indices.extend_from_slice(&[base, base + k, base + k + 1]);
                        }
                    }
                }
            }
        }
    }

    /// Stand what was built on flat ground on the terrain without bending it: each piece rises by
    /// the ground's height at its cutaway anchor (a tree, a wall) where it has one, and its lowest
    /// few centimetres follow the ground under them, so a trunk or a post meets a slope. Pieces
    /// without an anchor (fences, hedges, clutter) follow the ground vertex by vertex.
    pub fn lift(&mut self, t: &Terrain) {
        if t.is_flat() {
            return;
        }
        if let Some(far) = &mut self.far {
            far.lift(t);
        }
        for (k, p) in self.positions.iter_mut().enumerate() {
            let here = t.height(Vec2::new(p[0], p[2]));
            let anchor = Vec2::from(self.uv0[k]);
            let anchored = self.uv1[k][0] as u32 != CutKind::Never as u32 || anchor != Vec2::ZERO;
            p[1] += if anchored && p[1] > 0.05 { t.height(anchor) } else { here };
        }
    }

    pub fn build(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uv0)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, self.uv1)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

pub fn rgb(r: f32, g: f32, b: f32) -> LinearRgba {
    Color::srgb(r, g, b).to_linear()
}

pub fn shade(c: LinearRgba, k: f32) -> LinearRgba {
    LinearRgba::new(c.red * k, c.green * k, c.blue * k, 1.0)
}

/// Cheap stable hash → [0, 1).
pub fn hash01(x: i32, y: i32, salt: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ salt.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65536.0
}

pub fn floor_color(floor: Floor, x: i32, y: i32) -> LinearRgba {
    let n = hash01(x, y, 1);
    let p = pal();
    match floor {
        Floor::Grass => between(&p.floor_grass, hash01(x / 3, y / 3, 2) * 0.5 + n * 0.5),
        Floor::Dirt => between(&p.floor_dirt, n),
        Floor::Asphalt => between(&p.floor_asphalt, n),
        Floor::RoadLine => p.floor_road_line,
        Floor::Sidewalk => between(&p.floor_sidewalk, n),
        Floor::Path => between(&p.floor_path, n),
        Floor::Wood => between(&p.floor_wood, n),
        Floor::Carpet => between(&p.floor_carpet, n),
        Floor::TileFloor => palette::pick(&p.floor_tile, (x + y).rem_euclid(2) as usize),
        Floor::Concrete => between(&p.floor_concrete, n),
    }
}

/// Exterior siding colours, picked per building by the generator.
pub fn siding_color(i: u8) -> LinearRgba {
    palette::pick(&pal().siding, i as usize)
}

pub fn roof_color(i: u8) -> LinearRgba {
    palette::pick(&pal().roof, i as usize)
}

pub fn interior_color(room_type: &str) -> LinearRgba {
    pal().interior(room_type)
}

/// A piece of furniture as one or two boxes inside its tile rectangle (`min`, `size` in metres).
pub fn add_furniture(mb: &mut MeshBuilder, kind: &str, min: Vec2, size: Vec2, facing: Side, base_y: f32) {
    let inset = 0.08;
    let p = pal();
    let lo = min + Vec2::splat(inset);
    let hi = min + size - Vec2::splat(inset);
    let block = |mb: &mut MeshBuilder, lo: Vec2, hi: Vec2, h0: f32, h1: f32, c: LinearRgba| {
        let mut faces = same(c);
        faces[2] = shade(c, 1.1);
        mb.add_box(Vec3::new(lo.x, base_y + h0, lo.y), Vec3::new(hi.x, base_y + h1, hi.y), faces);
    };
    // Shrink toward the back (opposite `facing`) for things that sit against a wall.
    let against_wall = |depth: f32| -> (Vec2, Vec2) {
        match facing {
            Side::S => (lo, Vec2::new(hi.x, lo.y + depth)),
            Side::N => (Vec2::new(lo.x, hi.y - depth), hi),
            Side::E => (lo, Vec2::new(lo.x + depth, hi.y)),
            Side::W => (Vec2::new(hi.x - depth, lo.y), hi),
        }
    };
    match kind {
        "bed" => {
            block(mb, lo, hi, 0.0, 0.42, p.greybox_bed);
            block(mb, lo + Vec2::splat(0.04), hi - Vec2::splat(0.04), 0.42, 0.58, p.greybox_bed_sheet);
        }
        "wardrobe" => {
            let (a, b) = against_wall(0.6);
            block(mb, a, b, 0.0, 2.0, p.greybox_wardrobe);
        }
        "fridge" => block(mb, lo, hi, 0.0, 1.8, p.greybox_fridge),
        "stove" => block(mb, lo, hi, 0.0, 0.9, p.greybox_stove),
        "counter" | "sink" => block(mb, lo, hi, 0.0, 0.9, p.greybox_counter),
        "washer" | "dryer" => block(mb, lo, hi, 0.0, 0.9, p.greybox_washer),
        "table" | "desk" => block(mb, lo, hi, 0.68, 0.76, p.greybox_table),
        "coffee_table" => block(mb, lo + Vec2::splat(0.1), hi - Vec2::splat(0.1), 0.0, 0.42, p.greybox_coffee_table),
        "sofa" => {
            block(mb, lo, hi, 0.0, 0.45, p.greybox_sofa);
            let (a, b) = against_wall(0.25);
            block(mb, a, b, 0.45, 0.85, p.greybox_sofa_back);
        }
        "tv" => {
            block(mb, lo + Vec2::splat(0.1), hi - Vec2::splat(0.1), 0.0, 0.5, p.greybox_tv_stand);
            let (a, b) = against_wall(0.18);
            block(mb, a + Vec2::splat(0.05), b - Vec2::splat(0.05), 0.5, 1.05, p.greybox_tv_screen);
        }
        "toilet" => block(mb, lo + Vec2::splat(0.15), hi - Vec2::splat(0.15), 0.0, 0.45, p.greybox_toilet),
        "shelf" => {
            // Uprights and four boards against the wall.
            let (a, b) = against_wall(0.4);
            let c = p.greybox_shelf;
            for k in 0..4 {
                let h = 0.05 + k as f32 * 0.55;
                block(mb, a, b, h, h + 0.04, c);
            }
            let side = |mb: &mut MeshBuilder, x0: Vec2, x1: Vec2| block(mb, x0, x1, 0.0, 1.8, c);
            match facing {
                Side::N | Side::S => {
                    side(mb, a, Vec2::new(a.x + 0.05, b.y));
                    side(mb, Vec2::new(b.x - 0.05, a.y), b);
                }
                Side::E | Side::W => {
                    side(mb, a, Vec2::new(b.x, a.y + 0.05));
                    side(mb, Vec2::new(a.x, b.y - 0.05), b);
                }
            }
        }
        "workbench" => {
            block(mb, lo, hi, 0.0, 0.85, p.greybox_workbench);
            block(mb, lo, hi, 0.85, 0.92, p.greybox_workbench_top);
        }
        "bathtub" => block(mb, lo, hi, 0.0, 0.55, p.greybox_bathtub),
        "stairs" => {
            // Steps rising toward `facing`, up to the next floor.
            let steps = 10;
            for i in 0..steps {
                let t0 = i as f32 / steps as f32;
                let t1 = (i + 1) as f32 / steps as f32;
                let (a, b) = match facing {
                    Side::N => (Vec2::new(lo.x, hi.y - (hi.y - lo.y) * t1), Vec2::new(hi.x, hi.y - (hi.y - lo.y) * t0)),
                    Side::S => (Vec2::new(lo.x, lo.y + (hi.y - lo.y) * t0), Vec2::new(hi.x, lo.y + (hi.y - lo.y) * t1)),
                    Side::E => (Vec2::new(lo.x + (hi.x - lo.x) * t0, lo.y), Vec2::new(lo.x + (hi.x - lo.x) * t1, hi.y)),
                    Side::W => (Vec2::new(hi.x - (hi.x - lo.x) * t1, lo.y), Vec2::new(hi.x - (hi.x - lo.x) * t0, hi.y)),
                };
                block(mb, a, b, 0.0, 3.0 * t1, p.greybox_stairs);
            }
        }
        _ => block(mb, lo, hi, 0.0, 0.8, p.greybox_other),
    }
}

#[cfg(test)]
mod drape_tests {
    use super::*;
    use zombye_sim::terrain::TerrainDef;

    fn hills() -> Terrain {
        Terrain::generate(&TerrainDef::default(), 7, IVec2::splat(240), IVec2::splat(100), IVec2::splat(130))
    }

    #[test]
    fn draped_ground_lies_on_the_facets() {
        let t = hills();
        assert!(!t.is_flat());
        let mut m = MeshBuilder::default();
        // A lawn of one-metre tiles out in the hills, as the landscape builds it, and a kerb face.
        for z in 170..186 {
            for x in 170..186 {
                let (x, z) = (x as f32, z as f32);
                m.quad([Vec3::new(x, 0.0, z), Vec3::new(x, 0.0, z + 1.0), Vec3::new(x + 1.0, 0.0, z + 1.0), Vec3::new(x + 1.0, 0.0, z)], Vec3::Y, rgb(0.2, 0.5, 0.2));
            }
        }
        m.quad([Vec3::new(170.0, 0.0, 190.5), Vec3::new(170.0, 0.15, 190.5), Vec3::new(186.0, 0.15, 190.5), Vec3::new(186.0, 0.0, 190.5)], Vec3::Z, rgb(0.5, 0.5, 0.5));
        m.drape(&t);
        let mut flat = 0;
        for tri in m.indices.chunks_exact(3) {
            let p = [0, 1, 2].map(|k| Vec3::from(m.positions[tri[k] as usize]));
            let n = Vec3::from(m.normals[tri[0] as usize]);
            if n.y < 0.7 {
                continue;
            }
            flat += 1;
            for q in p {
                assert!((q.y - t.height(q.xz())).abs() < 1e-3, "a corner off the ground at {q}");
            }
            // Wholly on one facet: its middle is on the ground too, not cutting across a fold.
            let mid = (p[0] + p[1] + p[2]) / 3.0;
            assert!((mid.y - t.height(mid.xz())).abs() < 1e-3, "a triangle across a fold at {mid}");
            // Its normal is its facet's (the low-poly look).
            let f = t.facet_at(mid.xz());
            assert!((n - facet_normal(&f)).length() < 1e-3);
        }
        assert!(flat >= 16 * 16 * 2);
        // The ground's height rides in the first UVs, for the shader.
        for (k, q) in m.positions.iter().enumerate() {
            assert!((m.uv0[k][0] - t.height(Vec2::new(q[0], q[2]))).abs() < 1e-3);
        }
    }

    #[test]
    fn draping_level_ground_changes_nothing() {
        let t = Terrain::default();
        let mut m = MeshBuilder::default();
        m.quad([Vec3::ZERO, Vec3::Z, Vec3::new(1.0, 0.0, 1.0), Vec3::X], Vec3::Y, rgb(0.2, 0.5, 0.2));
        let before = m.positions.clone();
        m.drape(&t);
        assert_eq!(m.positions, before);
        assert!(m.uv0.iter().all(|uv| *uv == [0.0, 0.0]));
    }
}
