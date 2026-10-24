//! Faceted low-poly trees, built into a chunk's prop mesh: round-crowned broadleaves (a tapering
//! trunk that forks into a few branches, each ending in a clump of foliage), slim birches, and pines
//! and spruces as tiers of drooping skirts. Everything about a tree (kind, size, lean, crown) comes
//! from a hash of its tile, so a tile always grows the same tree and [`tree_at`] can be asked again
//! (the foliage fade in `materials.rs` does) without the mesh.
//!
//! Trunks and branches are tagged [`CutKind::Wood`] and foliage [`CutKind::Canopy`], both anchored
//! at the trunk's foot, so the world shader can fade one tree's foliage, and dim its wood, when it
//! hides the player. Blocking and sight are the sim's (a 0.3 m trunk per tree tile), untouched.

use std::f32::consts::TAU;

use bevy::prelude::*;
use zombye_sim::map::Feature;

use super::land_palette::{self as pal, Swatch};
use super::{CutKind, CutTag, MeshBuilder, hash01, shade};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeKind {
    Broadleaf,
    Birch,
    Pine,
    Spruce,
}

/// One tree, worked out from its tile.
#[derive(Clone, Copy, Debug)]
pub struct TreeSpec {
    pub kind: TreeKind,
    /// Foot of the trunk (also the cutaway anchor of all its pieces).
    pub base: Vec3,
    pub scale: f32,
    /// Horizontal drift of the tree per metre of height: a gentle lean.
    pub lean: Vec2,
    /// Where a broadleaf forks, or a birch's crown starts.
    pub fork: f32,
    /// Overall height.
    pub height: f32,
    /// Centre, horizontal radius and half height of the crown (for the foliage fade).
    pub crown: Vec3,
    pub crown_r: f32,
    pub crown_h: f32,
    x: i32,
    y: i32,
}

impl TreeSpec {
    fn h(&self, k: u32) -> f32 {
        hash01(self.x, self.y, 200 + k)
    }

    /// A point of the unleaned tree, leaned.
    fn p(&self, v: Vec3) -> Vec3 {
        let up = v.y - self.base.y;
        v + Vec3::new(self.lean.x, 0.0, self.lean.y) * up
    }
}

/// The tree a tile with `feature` grows, if any.
pub fn tree_at(x: i32, y: i32, feature: Feature) -> Option<TreeSpec> {
    let h = |k: u32| hash01(x, y, 200 + k);
    let kind = match feature {
        Feature::Tree if h(0) < 0.22 => TreeKind::Birch,
        Feature::Tree => TreeKind::Broadleaf,
        Feature::PineTree if h(0) < 0.4 => TreeKind::Spruce,
        Feature::PineTree => TreeKind::Pine,
        _ => return None,
    };
    // Trunks stay within 0.15 m of the tile centre: the sim's trunk is 0.3 m across there.
    let base = Vec3::new(x as f32 + 0.5 + (h(1) - 0.5) * 0.3, 0.0, y as f32 + 0.5 + (h(2) - 0.5) * 0.3);
    let scale = 0.8 + 0.45 * h(3);
    let a = h(4) * TAU;
    let lean = Vec2::new(a.cos(), a.sin()) * 0.065 * h(5) * h(5);
    let s = scale;
    let (fork, height, crown_y, crown_r, crown_h) = match kind {
        TreeKind::Broadleaf => {
            let fork = (1.5 + 0.7 * h(6)) * s;
            (fork, fork + 2.9 * s, fork + 1.2 * s, 2.0 * s, 1.4 * s)
        }
        TreeKind::Birch => {
            // The top clump's top (see `birch`).
            let fork = (2.0 + 0.4 * h(6)) * s;
            (fork, fork + 2.9 * s, fork + 1.2 * s, 0.9 * s, 1.5 * s)
        }
        TreeKind::Pine => {
            let top = (6.0 + 1.6 * h(6)) * s;
            (1.1 * s, top, top * 0.55, 1.55 * s, top * 0.45)
        }
        TreeKind::Spruce => {
            let top = (6.6 + 1.8 * h(6)) * s;
            (0.9 * s, top, top * 0.55, 1.25 * s, top * 0.47)
        }
    };
    let crown = base + Vec3::new(lean.x * crown_y, crown_y, lean.y * crown_y);
    Some(TreeSpec { kind, base, scale, lean, fork, height, crown, crown_r, crown_h, x, y })
}

/// Add a tree to `mb`. `simple` (deep in the forest, where neighbours hide most of it) uses fewer
/// branches, clumps and tiers.
pub fn add_tree(mb: &mut MeshBuilder, t: &TreeSpec, simple: bool) {
    let wood = CutTag { kind: CutKind::Wood, anchor: t.base.xz(), radius: 0.0 };
    let leaves = CutTag { kind: CutKind::Canopy, anchor: t.base.xz(), radius: t.crown_r };
    match t.kind {
        TreeKind::Broadleaf => broadleaf(mb, t, simple, wood, leaves),
        TreeKind::Birch => birch(mb, t, simple, wood, leaves),
        TreeKind::Pine => conifer(mb, t, if simple { 3 } else { 4 }, 7, pal::PINE, 0.2, wood, leaves),
        TreeKind::Spruce => conifer(mb, t, if simple { 4 } else { 5 }, 6, pal::SPRUCE, 0.12, wood, leaves),
    }
}

fn broadleaf(mb: &mut MeshBuilder, t: &TreeSpec, simple: bool, wood: CutTag, leaves: CutTag) {
    let s = t.scale;
    let fork = t.base + Vec3::Y * t.fork;
    mb.set_tag(wood);
    frustum(mb, t, t.base, fork, 0.17 * s, 0.11 * s, 5, pal::BARK.lin());
    let main = (t.h(10) * 4.0) as usize % 4;
    let second = (main + 1 + (t.h(11) * 2.0) as usize) % 4;
    let greens = [pal::LEAF[main].lin(), pal::LEAF[second].lin()];
    let branches = if simple { 1 } else if t.h(12) < 0.3 { 2 } else { 3 };
    let az0 = t.h(13) * TAU;
    let mut clumps: Vec<(Vec3, f32)> = Vec::with_capacity(4);
    for i in 0..branches {
        let az = az0 + i as f32 * TAU / branches as f32 + (t.h(20 + i) - 0.5) * 0.8;
        let elev = 0.72 + 0.4 * t.h(30 + i);
        let len = (1.15 + 0.55 * t.h(40 + i)) * s;
        let dir = Vec3::new(az.cos() * elev.cos(), elev.sin(), az.sin() * elev.cos());
        let end = fork + dir * len;
        spike(mb, t, fork - dir * 0.1 * s, end, 0.075 * s, 4, pal::BARK_DARK.lin());
        clumps.push((end + Vec3::Y * 0.2 * s, (0.85 + 0.3 * t.h(50 + i)) * s));
    }
    // The leader, carrying the top of the crown.
    let top = fork + Vec3::new((t.h(14) - 0.5) * 0.3, 1.3, (t.h(15) - 0.5) * 0.3) * s;
    spike(mb, t, fork, top, 0.08 * s, 4, pal::BARK.lin());
    clumps.push((top + Vec3::Y * 0.25 * s, (1.0 + 0.25 * t.h(16)) * s));
    mb.set_tag(leaves);
    for (i, (c, r)) in clumps.iter().enumerate() {
        let top_one = i + 1 == clumps.len();
        let colour = if top_one { shade(greens[0], 1.06) } else { greens[i % 2] };
        clump(mb, t, *c, Vec3::new(*r, r * 0.74, *r), colour, i as u32);
    }
}

fn birch(mb: &mut MeshBuilder, t: &TreeSpec, simple: bool, wood: CutTag, leaves: CutTag) {
    let s = t.scale;
    let (pale, mark) = (pal::BIRCH_BARK.lin(), pal::BIRCH_MARK.lin());
    let at = |y: f32| t.base + Vec3::Y * y;
    let r = 0.085 * s;
    let clumps = if simple { 3 } else { 4 };
    let step = 0.6 * s;
    // The trunk ends in the top clump.
    let top = t.fork + 0.25 * s + (clumps - 1) as f32 * step;
    mb.set_tag(wood);
    frustum(mb, t, at(0.0), at(1.5 * s), r, r * 0.92, 4, pale);
    frustum(mb, t, at(1.5 * s), at(1.66 * s), r * 0.92, r * 0.9, 4, mark);
    frustum(mb, t, at(1.66 * s), at(top * 0.8), r * 0.9, r * 0.5, 4, pale);
    spike(mb, t, at(top * 0.8), at(top), r * 0.5, 4, pale);
    let az0 = t.h(13) * TAU;
    let mut centres = Vec::new();
    for i in 0..clumps {
        let y = t.fork + 0.25 * s + i as f32 * step;
        let az = az0 + i as f32 * 2.3;
        let out = Vec3::new(az.cos(), 0.0, az.sin()) * (0.3 - 0.06 * i as f32) * s;
        if i < 2 && !simple {
            spike(mb, t, at(y - 0.5 * s), at(y) + out * 1.6, 0.04 * s, 3, mark);
        }
        centres.push((at(y) + out, (0.62 + 0.12 * t.h(50 + i) - 0.06 * i as f32) * s));
    }
    mb.set_tag(leaves);
    let pick = (t.h(10) * 2.0) as usize % 2;
    for (i, (c, r)) in centres.iter().enumerate() {
        let colour = pal::BIRCH_LEAF[(pick + i) % 2].lin();
        clump(mb, t, *c, Vec3::new(r * 0.85, r * 1.3, r * 0.85), colour, i as u32);
    }
}

#[allow(clippy::too_many_arguments)]
fn conifer(mb: &mut MeshBuilder, t: &TreeSpec, tiers: u32, points: u32, greens: [Swatch; 2], droop: f32, wood: CutTag, leaves: CutTag) {
    let s = t.scale;
    let at = |y: f32| t.base + Vec3::Y * y;
    mb.set_tag(wood);
    frustum(mb, t, at(0.0), at(t.fork + 0.5 * s), 0.15 * s, 0.12 * s, 5, pal::BARK.lin());
    // The trunk runs on up inside the tiers: what's left to see while the foliage is faded.
    spike(mb, t, at(t.fork + 0.5 * s), at(t.height * 0.85), 0.12 * s, 4, pal::BARK_DARK.lin());
    mb.set_tag(leaves);
    let green = greens[(t.h(10) * 2.0) as usize % 2].lin();
    // Tiers from the lowest (widest) up; the top one is a spire.
    let span = t.height - t.fork - 1.9 * s;
    let narrow = if points == 6 { 0.8 } else { 1.0 };
    for k in 0..tiers {
        let f = k as f32 / (tiers - 1) as f32;
        let bottom = t.fork + f * span;
        let top_tier = k + 1 == tiers;
        let tier_h = if top_tier { t.height - bottom } else { 1.75 * s };
        let ro = ((1.6 - 1.0 * f) * narrow) * s * (0.92 + 0.16 * t.h(60 + k));
        let rot = t.h(70 + k) * TAU;
        skirt(mb, t, at(bottom), tier_h, ro, points, droop * s, rot, shade(green, 0.9 + 0.08 * f));
    }
}

/// One tier of a conifer: a fan from the apex down to a jagged rim of drooping points and
/// tucked-in notches.
#[allow(clippy::too_many_arguments)]
fn skirt(mb: &mut MeshBuilder, t: &TreeSpec, bottom: Vec3, height: f32, ro: f32, points: u32, droop: f32, rot: f32, colour: LinearRgba) {
    let apex = bottom + Vec3::Y * height;
    let rim = |i: u32, notch: bool| {
        let a = rot + (i as f32 + if notch { 0.5 } else { 0.0 }) * TAU / points as f32;
        let (r, y) = if notch { (ro * 0.64, height * 0.14) } else { (ro, -droop) };
        bottom + Vec3::new(a.cos() * r, y, a.sin() * r)
    };
    for i in 0..points {
        let (p0, n, p1) = (rim(i, false), rim(i, true), rim(i + 1, false));
        tri_out(mb, t, [apex, p0, n], bottom, colour);
        tri_out(mb, t, [apex, n, p1], bottom, shade(colour, 0.93));
    }
}

/// A tapering n-sided tube from `a` to `b`, open at both ends.
#[allow(clippy::too_many_arguments)]
fn frustum(mb: &mut MeshBuilder, t: &TreeSpec, a: Vec3, b: Vec3, ra: f32, rb: f32, sides: u32, colour: LinearRgba) {
    let (u, v) = basis(b - a);
    let ring = |c: Vec3, r: f32, i: u32| {
        let ang = i as f32 / sides as f32 * TAU;
        c + (u * ang.cos() + v * ang.sin()) * r
    };
    let mid = (a + b) * 0.5;
    for i in 0..sides {
        let (a0, a1, b0, b1) = (ring(a, ra, i), ring(a, ra, i + 1), ring(b, rb, i), ring(b, rb, i + 1));
        tri_out(mb, t, [a0, b0, b1], mid, colour);
        tri_out(mb, t, [a0, b1, a1], mid, colour);
    }
}

/// An n-sided pyramid from a ring round `a` to the point `b` (branches, tips).
fn spike(mb: &mut MeshBuilder, t: &TreeSpec, a: Vec3, b: Vec3, r: f32, sides: u32, colour: LinearRgba) {
    let (u, v) = basis(b - a);
    let ring = |i: u32| {
        let ang = i as f32 / sides as f32 * TAU;
        a + (u * ang.cos() + v * ang.sin()) * r
    };
    let mid = a.lerp(b, 0.3);
    for i in 0..sides {
        tri_out(mb, t, [ring(i), b, ring(i + 1)], mid, colour);
    }
}

/// A lumpy faceted clump of foliage (a jittered icosahedron), a little lighter on top.
fn clump(mb: &mut MeshBuilder, t: &TreeSpec, centre: Vec3, scale: Vec3, colour: LinearRgba, k: u32) {
    let g = (1.0 + 5.0_f32.sqrt()) / 2.0;
    let v = [
        Vec3::new(-1.0, g, 0.0), Vec3::new(1.0, g, 0.0), Vec3::new(-1.0, -g, 0.0), Vec3::new(1.0, -g, 0.0),
        Vec3::new(0.0, -1.0, g), Vec3::new(0.0, 1.0, g), Vec3::new(0.0, -1.0, -g), Vec3::new(0.0, 1.0, -g),
        Vec3::new(g, 0.0, -1.0), Vec3::new(g, 0.0, 1.0), Vec3::new(-g, 0.0, -1.0), Vec3::new(-g, 0.0, 1.0),
    ];
    const F: [[usize; 3]; 20] = [
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11], [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9], [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    let len = v[0].length();
    // A random turn about the vertical, so clumps don't all show the same facets.
    let rot = Quat::from_rotation_y(t.h(80 + k) * TAU);
    let mut pts = [Vec3::ZERO; 12];
    for (i, p) in v.iter().enumerate() {
        let j = 0.84 + 0.3 * t.h(90 + k * 12 + i as u32);
        pts[i] = centre + rot * (*p / len) * scale * j;
    }
    for (i, f) in F.iter().enumerate() {
        let tri = [pts[f[0]], pts[f[1]], pts[f[2]]];
        let up = ((tri[0] + tri[1] + tri[2]) / 3.0 - centre).y / scale.y;
        let k = 0.9 + 0.12 * up + 0.08 * t.h(400 + k * 20 + i as u32);
        tri_out(mb, t, tri, centre, shade(colour, k));
    }
}

/// A triangle (leaned), wound to face away from `inside`.
fn tri_out(mb: &mut MeshBuilder, t: &TreeSpec, [a, b, c]: [Vec3; 3], inside: Vec3, colour: LinearRgba) {
    let (a, b, c, inside) = (t.p(a), t.p(b), t.p(c), t.p(inside));
    let n = (b - a).cross(c - a);
    if n.dot((a + b + c) / 3.0 - inside) >= 0.0 {
        mb.triangle([a, b, c], colour);
    } else {
        mb.triangle([a, c, b], colour);
    }
}

fn basis(axis: Vec3) -> (Vec3, Vec3) {
    let w = axis.normalize_or(Vec3::Y);
    let helper = if w.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let u = w.cross(helper).normalize();
    (u, w.cross(u))
}

/// A low, round bush of two or three clumps (walkable, like the sim's bushes).
pub fn add_bush(mb: &mut MeshBuilder, x: i32, y: i32, centre: Vec3, size: f32) {
    let t = TreeSpec {
        kind: TreeKind::Broadleaf,
        base: centre,
        scale: size,
        lean: Vec2::ZERO,
        fork: 0.0,
        height: 0.8 * size,
        crown: centre,
        crown_r: 0.0,
        crown_h: 0.0,
        x,
        y,
    };
    mb.set_tag(CutTag::default());
    let colour = pal::SHRUB[(t.h(10) * 2.0) as usize % 2].lin();
    let n = if t.h(11) < 0.5 { 2 } else { 3 };
    for i in 0..n {
        let a = t.h(12 + i) * TAU;
        let off = Vec3::new(a.cos(), 0.0, a.sin()) * 0.28 * size * (i > 0) as u32 as f32;
        let r = (0.5 - 0.08 * i as f32) * size;
        clump(mb, &t, centre + off + Vec3::Y * r * 0.62, Vec3::new(r, r * 0.72, r), shade(colour, 1.0 + 0.05 * i as f32), i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tris(feature: Feature, simple: bool) -> (usize, f32) {
        let mut total = 0;
        let mut top: f32 = 0.0;
        for i in 0..40 {
            let t = tree_at(i * 7, i * 3, feature).unwrap();
            let mut mb = MeshBuilder::default();
            add_tree(&mut mb, &t, simple);
            total += mb.tri_count();
            top = top.max(mb.bounds().1.y);
        }
        (total / 40, top)
    }

    #[test]
    fn trees_stay_within_budget_and_size() {
        for (f, simple, most) in [(Feature::Tree, false, 130), (Feature::Tree, true, 70), (Feature::PineTree, false, 80), (Feature::PineTree, true, 60)] {
            let (n, top) = tris(f, simple);
            eprintln!("{f:?} simple {simple}: {n} triangles on average, tallest {top:.1} m");
            assert!(n <= most, "{f:?} {n} triangles");
            assert!(top < 12.0);
        }
    }

    #[test]
    fn a_tile_always_grows_the_same_tree() {
        let a = tree_at(10, 20, Feature::Tree).unwrap();
        let b = tree_at(10, 20, Feature::Tree).unwrap();
        assert_eq!(a.base, b.base);
        assert_eq!(a.kind, b.kind);
        assert!(tree_at(10, 20, Feature::Bush).is_none());
        // The trunk stays where the sim's trunk is.
        assert!((a.base.xz() - Vec2::new(10.5, 20.5)).abs().max_element() <= 0.15);
    }
}
