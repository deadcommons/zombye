//! Procedural low-poly geometry: boxes with per-face vertex colours, merged into one mesh per
//! chunk or building. All of M1's buildings, furniture and props are grey-box geometry built here;
//! generated art replaces pieces later without changing how the world is assembled.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use zombye_sim::map::{Floor, Side};

/// What a piece of geometry is, for the cutaway shader (stored in the second UV channel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CutKind {
    /// Never cut: ground, floors, furniture, trunks, fences.
    Never = 0,
    /// A wall piece: drops to a stub as a whole when it's on the camera side of the player.
    Wall = 1,
    /// A roof: disappears as a whole when it hides the player.
    Roof = 2,
    /// A tree canopy: fades when it hides the player.
    Canopy = 3,
    /// The flat top of a wall stub: only drawn while its wall is cut.
    WallCap = 4,
    /// Other building structure (door leaves): never cut by the shader, but like walls it stays
    /// readable in unseen rooms.
    Structure = 5,
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
}

/// Face order for [`MeshBuilder::add_box`]: +x, -x, +y, -y, +z, -z.
pub type FaceColors = [LinearRgba; 6];

pub fn same(c: LinearRgba) -> FaceColors {
    [c; 6]
}

impl MeshBuilder {
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
        let base = self.positions.len() as u32;
        for c in corners {
            self.push_vertex(c, normal, color);
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    pub fn triangle(&mut self, corners: [Vec3; 3], color: LinearRgba) {
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
    match floor {
        Floor::Grass => {
            let patch = hash01(x / 3, y / 3, 2) * 0.5 + n * 0.5;
            rgb(0.30 + patch * 0.06, 0.38 + patch * 0.07, 0.20 + patch * 0.03)
        }
        Floor::Dirt => rgb(0.36 + n * 0.04, 0.30 + n * 0.03, 0.22),
        Floor::Asphalt => rgb(0.21 + n * 0.02, 0.21 + n * 0.02, 0.22 + n * 0.02),
        Floor::RoadLine => rgb(0.62, 0.55, 0.28),
        Floor::Sidewalk => rgb(0.52 + n * 0.03, 0.51 + n * 0.03, 0.48 + n * 0.03),
        Floor::Path => rgb(0.48 + n * 0.03, 0.45 + n * 0.03, 0.40),
        Floor::Wood => rgb(0.45 + n * 0.04, 0.32 + n * 0.03, 0.20),
        Floor::Carpet => rgb(0.42, 0.36 + n * 0.02, 0.32),
        Floor::TileFloor => {
            if (x + y) % 2 == 0 { rgb(0.72, 0.72, 0.68) } else { rgb(0.62, 0.63, 0.60) }
        }
        Floor::Concrete => rgb(0.50 + n * 0.02, 0.50 + n * 0.02, 0.50),
    }
}

/// Exterior siding colours, picked per building by the generator.
pub fn siding_color(i: u8) -> LinearRgba {
    const C: [(f32, f32, f32); 8] = [
        (0.86, 0.84, 0.78),
        (0.80, 0.74, 0.58),
        (0.84, 0.80, 0.56),
        (0.58, 0.66, 0.72),
        (0.60, 0.66, 0.52),
        (0.62, 0.62, 0.60),
        (0.70, 0.60, 0.46),
        (0.58, 0.32, 0.26),
    ];
    let c = C[i as usize % C.len()];
    rgb(c.0, c.1, c.2)
}

pub fn roof_color(i: u8) -> LinearRgba {
    const C: [(f32, f32, f32); 5] = [(0.24, 0.24, 0.26), (0.36, 0.26, 0.20), (0.42, 0.20, 0.16), (0.28, 0.33, 0.28), (0.26, 0.30, 0.38)];
    let c = C[i as usize % C.len()];
    rgb(c.0, c.1, c.2)
}

pub fn interior_color(room_type: &str) -> LinearRgba {
    match room_type {
        "kitchen" => rgb(0.84, 0.82, 0.70),
        "bathroom" => rgb(0.74, 0.82, 0.84),
        "bedroom" => rgb(0.78, 0.72, 0.68),
        "living" => rgb(0.80, 0.74, 0.62),
        "laundry" => rgb(0.72, 0.72, 0.70),
        _ => rgb(0.82, 0.80, 0.74),
    }
}

/// A piece of furniture as one or two boxes inside its tile rectangle (`min`, `size` in metres).
pub fn add_furniture(mb: &mut MeshBuilder, kind: &str, min: Vec2, size: Vec2, facing: Side, base_y: f32) {
    let inset = 0.08;
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
            block(mb, lo, hi, 0.0, 0.42, rgb(0.42, 0.30, 0.22));
            block(mb, lo + Vec2::splat(0.04), hi - Vec2::splat(0.04), 0.42, 0.58, rgb(0.80, 0.78, 0.74));
        }
        "wardrobe" => {
            let (a, b) = against_wall(0.6);
            block(mb, a, b, 0.0, 2.0, rgb(0.40, 0.28, 0.18));
        }
        "fridge" => block(mb, lo, hi, 0.0, 1.8, rgb(0.86, 0.86, 0.84)),
        "stove" => block(mb, lo, hi, 0.0, 0.9, rgb(0.20, 0.20, 0.21)),
        "counter" | "sink" => block(mb, lo, hi, 0.0, 0.9, rgb(0.62, 0.52, 0.40)),
        "washer" | "dryer" => block(mb, lo, hi, 0.0, 0.9, rgb(0.88, 0.88, 0.86)),
        "table" | "desk" => block(mb, lo, hi, 0.68, 0.76, rgb(0.46, 0.33, 0.22)),
        "coffee_table" => block(mb, lo + Vec2::splat(0.1), hi - Vec2::splat(0.1), 0.0, 0.42, rgb(0.40, 0.28, 0.20)),
        "sofa" => {
            block(mb, lo, hi, 0.0, 0.45, rgb(0.36, 0.30, 0.42));
            let (a, b) = against_wall(0.25);
            block(mb, a, b, 0.45, 0.85, rgb(0.32, 0.26, 0.38));
        }
        "tv" => {
            block(mb, lo + Vec2::splat(0.1), hi - Vec2::splat(0.1), 0.0, 0.5, rgb(0.30, 0.22, 0.16));
            let (a, b) = against_wall(0.18);
            block(mb, a + Vec2::splat(0.05), b - Vec2::splat(0.05), 0.5, 1.05, rgb(0.08, 0.08, 0.09));
        }
        "toilet" => block(mb, lo + Vec2::splat(0.15), hi - Vec2::splat(0.15), 0.0, 0.45, rgb(0.92, 0.92, 0.90)),
        "bathtub" => block(mb, lo, hi, 0.0, 0.55, rgb(0.90, 0.90, 0.88)),
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
                block(mb, a, b, 0.0, 3.0 * t1, rgb(0.44, 0.32, 0.22));
            }
        }
        _ => block(mb, lo, hi, 0.0, 0.8, rgb(0.5, 0.45, 0.4)),
    }
}

/// Vertical n-sided prism (tree trunks, posts).
pub fn add_prism(mb: &mut MeshBuilder, base: Vec3, radius: f32, height: f32, sides: u32, color: LinearRgba) {
    let ring = |i: u32| {
        let a = i as f32 / sides as f32 * std::f32::consts::TAU;
        Vec3::new(a.cos() * radius, 0.0, a.sin() * radius)
    };
    for i in 0..sides {
        let (a, b) = (base + ring(i), base + ring(i + 1));
        let up = Vec3::Y * height;
        let normal = ((a + b) * 0.5 - base).with_y(0.0).normalize_or_zero();
        mb.quad([a, a + up, b + up, b], normal, color);
    }
}

/// Cone from a base ring up to a tip (pine tiers).
pub fn add_cone(mb: &mut MeshBuilder, base: Vec3, radius: f32, height: f32, sides: u32, color: LinearRgba) {
    let tip = base + Vec3::Y * height;
    for i in 0..sides {
        let a0 = i as f32 / sides as f32 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / sides as f32 * std::f32::consts::TAU;
        let p0 = base + Vec3::new(a0.cos(), 0.0, a0.sin()) * radius;
        let p1 = base + Vec3::new(a1.cos(), 0.0, a1.sin()) * radius;
        mb.triangle([p0, tip, p1], if i % 2 == 0 { color } else { shade(color, 0.9) });
    }
}

/// Low-poly blob (icosahedron) for leafy canopies and bushes.
pub fn add_blob(mb: &mut MeshBuilder, centre: Vec3, scale: Vec3, color: LinearRgba, seed: u32) {
    let t = (1.0 + 5.0_f32.sqrt()) / 2.0;
    let v = [
        Vec3::new(-1.0, t, 0.0), Vec3::new(1.0, t, 0.0), Vec3::new(-1.0, -t, 0.0), Vec3::new(1.0, -t, 0.0),
        Vec3::new(0.0, -1.0, t), Vec3::new(0.0, 1.0, t), Vec3::new(0.0, -1.0, -t), Vec3::new(0.0, 1.0, -t),
        Vec3::new(t, 0.0, -1.0), Vec3::new(t, 0.0, 1.0), Vec3::new(-t, 0.0, -1.0), Vec3::new(-t, 0.0, 1.0),
    ];
    const F: [[usize; 3]; 20] = [
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11], [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9], [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    let len = v[0].length();
    // Jitter vertices a little so every tree is lumpy in its own way.
    let pts: Vec<Vec3> = v
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let j = 0.85 + 0.3 * hash01(i as i32, seed as i32, 7);
            centre + (*p / len) * scale * j
        })
        .collect();
    for (i, f) in F.iter().enumerate() {
        let k = 0.85 + 0.25 * hash01(i as i32, seed as i32, 9);
        mb.triangle([pts[f[0]], pts[f[1]], pts[f[2]]], shade(color, k));
    }
}
