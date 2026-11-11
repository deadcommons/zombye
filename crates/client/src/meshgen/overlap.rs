//! Finds z-fighting before it's on a screen (tests only): faces of one mesh that lie in the same
//! plane, cover the same area and differ in colour. The GPU can't tell which of the two is in
//! front, and which wins changes from pixel to pixel: dashes and shimmer along the overlap.

use bevy::prelude::*;

use super::{CutKind, MeshBuilder};

/// Two faces fighting over the same plane.
#[derive(Debug)]
pub struct Fight {
    /// The middle of what they share.
    pub at: Vec3,
    pub normal: Vec3,
    /// How much they share, in square metres.
    pub area: f32,
    pub colours: [[f32; 4]; 2],
    pub kinds: [u32; 2],
}

/// Planes nearer than this are one plane to a depth buffer (the camera's 400 m of depth in 24
/// bits of float is about 0.05 mm, and interpolation across big triangles loses more).
const SAME_PLANE: f32 = 0.0005;
/// Overlaps smaller than this (1 cm by 1 cm) never show.
const MIN_AREA: f32 = 0.0001;

struct Tri {
    /// Corners in the plane's own two axes.
    p: [Vec2; 3],
    lo: Vec2,
    hi: Vec2,
    colour: [f32; 4],
    kind: u32,
    d: f32,
    normal: Vec3,
    u: Vec3,
    v: Vec3,
}

fn differ(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).any(|(x, y)| (x - y).abs() > 0.004)
}

/// A convex polygon cut to the inside of triangle `t` (either winding).
fn clip(mut poly: Vec<Vec2>, t: &[Vec2; 3]) -> Vec<Vec2> {
    let sign = (t[1] - t[0]).perp_dot(t[2] - t[0]).signum();
    for i in 0..3 {
        let (a, b) = (t[i], t[(i + 1) % 3]);
        let side = |p: Vec2| (b - a).perp_dot(p - a) * sign;
        let mut out = Vec::with_capacity(poly.len() + 2);
        for k in 0..poly.len() {
            let (p, q) = (poly[k], poly[(k + 1) % poly.len()]);
            let (sp, sq) = (side(p), side(q));
            if sp >= 0.0 {
                out.push(p);
            }
            if (sp > 0.0 && sq < 0.0) || (sp < 0.0 && sq > 0.0) {
                out.push(p + (q - p) * (sp / (sp - sq)));
            }
        }
        poly = out;
        if poly.len() < 3 {
            return Vec::new();
        }
    }
    poly
}

fn area(poly: &[Vec2]) -> (f32, Vec2) {
    let mut a = 0.0;
    let mut c = Vec2::ZERO;
    for i in 1..poly.len().saturating_sub(1) {
        let t = (poly[i] - poly[0]).perp_dot(poly[i + 1] - poly[0]) * 0.5;
        a += t;
        c += (poly[0] + poly[i] + poly[i + 1]) / 3.0 * t;
    }
    if a.abs() < 1e-12 { (0.0, Vec2::ZERO) } else { (a.abs(), c / a) }
}

impl MeshBuilder {
    /// Every pair of faces in this mesh that fight (see the module docs). Faces of one colour
    /// don't count (nobody can see which wins), nor do the tops of wall stubs against anything
    /// (drawn only while their wall is down).
    pub fn fights(&self) -> Vec<Fight> {
        let mut planes: std::collections::BTreeMap<(i32, i32, i32), Vec<Tri>> = Default::default();
        for t in self.indices.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|i| Vec3::from(self.positions[t[i] as usize]));
            let mut n = (b - a).cross(c - a);
            if n.length() < 1e-7 {
                continue;
            }
            n = n.normalize();
            // One name for a plane whichever way its faces look (materials are double-sided).
            let lead = [n.x, n.y, n.z].into_iter().find(|v| v.abs() > 1e-3).unwrap_or(1.0);
            if lead < 0.0 {
                n = -n;
            }
            let u = n.any_orthonormal_vector();
            let v = n.cross(u);
            // Axis-aligned planes (nearly all of them) get exact axes, so their keys agree.
            let key = ((n.x * 500.0).round() as i32, (n.y * 500.0).round() as i32, (n.z * 500.0).round() as i32);
            let n = Vec3::new(key.0 as f32, key.1 as f32, key.2 as f32).normalize();
            let (u, v) = if key.0.abs() + key.1.abs() + key.2.abs() == 500 { (n.any_orthonormal_vector(), n.cross(n.any_orthonormal_vector())) } else { (u, v) };
            let p = [a, b, c].map(|q| Vec2::new(q.dot(u), q.dot(v)));
            planes.entry(key).or_default().push(Tri {
                p,
                lo: p[0].min(p[1]).min(p[2]),
                hi: p[0].max(p[1]).max(p[2]),
                colour: self.colors[t[0] as usize],
                kind: self.uv1[t[0] as usize][0] as u32,
                d: (a.dot(n) + b.dot(n) + c.dot(n)) / 3.0,
                normal: n,
                u,
                v,
            });
        }
        let mut out = Vec::new();
        for tris in planes.values_mut() {
            tris.sort_by(|a, b| a.d.total_cmp(&b.d));
            for i in 0..tris.len() {
                let a = &tris[i];
                for b in &tris[i + 1..] {
                    if b.d - a.d > SAME_PLANE {
                        break;
                    }
                    if a.hi.x <= b.lo.x || b.hi.x <= a.lo.x || a.hi.y <= b.lo.y || b.hi.y <= a.lo.y {
                        continue;
                    }
                    if !differ(a.colour, b.colour) || a.kind == CutKind::WallCap as u32 || b.kind == CutKind::WallCap as u32 {
                        continue;
                    }
                    let (shared, mid) = area(&clip(a.p.to_vec(), &b.p));
                    if shared >= MIN_AREA {
                        out.push(Fight { at: a.u * mid.x + a.v * mid.y + a.normal * a.d, normal: a.normal, area: shared, colours: [a.colour, b.colour], kinds: [a.kind, b.kind] });
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshgen::same;

    #[test]
    fn boxes_flush_with_each_other_fight_and_inset_ones_do_not() {
        let (red, blue) = (LinearRgba::RED, LinearRgba::BLUE);
        let mut mb = MeshBuilder::default();
        mb.add_box(Vec3::ZERO, Vec3::ONE, same(red));
        mb.add_box(Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 1.0), same(red));
        assert!(mb.fights().is_empty(), "one colour never shows");
        // A blue box whose end is flush with the red one's side.
        mb.add_box(Vec3::new(0.25, 0.0, -1.0), Vec3::new(0.75, 1.0, 1.0), same(blue));
        let f = mb.fights();
        assert!(!f.is_empty() && f.iter().any(|f| f.normal.z.abs() > 0.9 && (f.at.z - 1.0).abs() < 1e-3), "{f:?}");
        let mut mb = MeshBuilder::default();
        mb.add_box(Vec3::ZERO, Vec3::ONE, same(red));
        mb.add_box(Vec3::new(0.25, 0.1, -1.0), Vec3::new(0.75, 0.9, 0.997), same(blue));
        assert!(mb.fights().is_empty(), "{:?}", mb.fights());
    }
}
