//! Decay on street props: the models are clean and all of one colour (every parked car the same
//! blue), so as a chunk's props mesh is built each prop's triangles are repainted a little: a
//! vehicle takes one of a handful of paints, paint fades toward grey, dust settles on what faces
//! up and mud on what is near the ground, and rust comes through in soft patches. All by hashes of
//! where the prop stands, so a town looks the same every time. The amounts and colours are data
//! (`assets/data/decay.ron`). Baked into the vertex colours: nothing is added to a frame.

use std::collections::BTreeMap;

use bevy::prelude::*;

/// `decay.ron`: how worn each kind of street prop is drawn.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DecayLook {
    /// 0 as made .. 1 a wreck, by kind; kinds not listed take `other`.
    pub wear: BTreeMap<String, f32>,
    pub other: f32,
    /// One thing of a kind differs from the next by up to this much either way.
    pub spread: f32,
    /// What settles on things and what they rust to (sRGB).
    pub dust: (f32, f32, f32),
    pub rust: (f32, f32, f32),
    /// Kinds that take one of `paints` (sRGB) in place of their model's main colour.
    pub repaint: Vec<String>,
    pub paints: Vec<(f32, f32, f32)>,
}

impl Default for DecayLook {
    fn default() -> Self {
        shipped()
    }
}

const SHIPPED: &str = include_str!("../../../../assets/data/decay.ron");
static LOOK: std::sync::OnceLock<DecayLook> = std::sync::OnceLock::new();

/// The file's fields, each optional.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    wear: Option<BTreeMap<String, f32>>,
    other: Option<f32>,
    spread: Option<f32>,
    dust: Option<(f32, f32, f32)>,
    rust: Option<(f32, f32, f32)>,
    repaint: Option<Vec<String>>,
    paints: Option<Vec<(f32, f32, f32)>>,
}

impl DecayLook {
    /// No decay at all: every model as it was made.
    pub fn none() -> DecayLook {
        DecayLook { wear: BTreeMap::new(), other: 0.0, spread: 0.0, dust: (0.0, 0.0, 0.0), rust: (0.0, 0.0, 0.0), repaint: Vec::new(), paints: Vec::new() }
    }

    pub fn parse(text: &str) -> Result<DecayLook, String> {
        Self::parse_over(text, shipped())
    }

    fn parse_over(text: &str, base: DecayLook) -> Result<DecayLook, String> {
        let f: File = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME).from_str(text).map_err(|e| e.to_string())?;
        let look = DecayLook {
            wear: f.wear.unwrap_or(base.wear),
            other: f.other.unwrap_or(base.other),
            spread: f.spread.unwrap_or(base.spread),
            dust: f.dust.unwrap_or(base.dust),
            rust: f.rust.unwrap_or(base.rust),
            repaint: f.repaint.unwrap_or(base.repaint),
            paints: f.paints.unwrap_or(base.paints),
        };
        let unit = |name: &str, v: f32| if (0.0..=1.0).contains(&v) { Ok(()) } else { Err(format!("{name} must be from 0 to 1, not {v}")) };
        for (kind, w) in &look.wear {
            unit(&format!("wear of {kind}"), *w)?;
        }
        unit("other", look.other)?;
        unit("spread", look.spread)?;
        for (name, c) in [("dust", look.dust), ("rust", look.rust)].into_iter().chain(look.paints.iter().map(|c| ("a paint", *c))) {
            for v in [c.0, c.1, c.2] {
                unit(name, v)?;
            }
        }
        Ok(look)
    }
}

fn shipped() -> DecayLook {
    DecayLook::parse_over(SHIPPED, DecayLook::none()).expect("the shipped decay.ron is good (a test checks)")
}

/// Use `decay.ron` from the data folder `dir` (the shipped one if it has none). Call before any
/// chunk is built; later calls change nothing.
pub fn load(dir: &std::path::Path) -> Result<(), String> {
    let path = dir.join("decay.ron");
    let look = match std::fs::read_to_string(&path) {
        Ok(text) => DecayLook::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(_) => shipped(),
    };
    let _ = LOOK.set(look);
    Ok(())
}

pub fn look() -> &'static DecayLook {
    LOOK.get_or_init(shipped)
}

fn hash(a: i32, b: i32, c: i32, salt: u32) -> f32 {
    let mut h = (a as u32).wrapping_mul(0x9E37_79B1) ^ (b as u32).wrapping_mul(0x85EB_CA77) ^ (c as u32).wrapping_mul(0xC2B2_AE3D) ^ salt.wrapping_mul(0x27D4_EB2F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

fn linear((r, g, b): (f32, f32, f32)) -> Vec3 {
    let c = Color::srgb(r, g, b).to_linear();
    Vec3::new(c.red, c.green, c.blue)
}

/// How one prop is repainted: made per prop by [`Wear::of`], asked per corner.
#[derive(Clone, Copy, Debug)]
pub struct Wear {
    /// 0 as made .. 1 a wreck.
    pub amount: f32,
    /// 0 none .. 1 all over: dark scrapes and dents' shadows from crashes (a driven car's body
    /// damage, `vehicles_render`); a parked prop has none.
    pub scuffs: f32,
    seed: u32,
    /// The model's main colour and the paint this one has instead.
    paint: Option<(Vec3, Vec3)>,
    dust: Vec3,
    rust: Vec3,
}

impl Wear {
    /// The wear of the prop of `kind` standing at `at`, whose model's main colour is `body`
    /// (`FurnitureModels::body_colour`). `None` when it is drawn as made.
    pub fn of(look: &DecayLook, kind: &str, at: Vec2, body: Option<Vec3>) -> Option<Wear> {
        let (x, y) = ((at.x * 2.0) as i32, (at.y * 2.0) as i32);
        let base = look.wear.get(kind).copied().unwrap_or(look.other);
        let amount = if base > 0.0 { (base + (hash(x, y, 0, 1) * 2.0 - 1.0) * look.spread).clamp(0.05, 1.0) } else { 0.0 };
        let paint = match body {
            Some(from) if !look.paints.is_empty() && look.repaint.iter().any(|k| k == kind) => {
                let i = (hash(x, y, 0, 2) * look.paints.len() as f32) as usize;
                Some((from, linear(look.paints[i.min(look.paints.len() - 1)])))
            }
            _ => None,
        };
        (amount > 0.0 || paint.is_some()).then(|| Wear { amount, scuffs: 0.0, seed: (x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663), paint, dust: linear(look.dust), rust: linear(look.rust) })
    }

    /// The colour of a corner of the model: where it is, `p`, and its normal `n` in the model's
    /// own frame (metres, y up from the ground), `c` as the model has it. Everything here varies
    /// smoothly from place to place and is asked per corner, so it blends across a panel: asked
    /// once a triangle in hard patches, a car's big panels came out as shards of rust and dust.
    pub fn colour(&self, p: Vec3, n: Vec3, c: LinearRgba) -> LinearRgba {
        let mut v = Vec3::new(c.red, c.green, c.blue);
        if let Some((from, to)) = self.paint
            && (v - from).abs().max_element() < 0.02
        {
            v = to;
        }
        let w = self.amount;
        if w <= 0.0 {
            let v = self.scuffed(p, v);
            return LinearRgba::new(v.x, v.y, v.z, c.alpha);
        }
        let lum = v.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        // Paint fades in the sun: toward its own grey, a little paler.
        v = v.lerp(Vec3::splat(lum * 1.15 + 0.01), w * 0.35);
        // Dust on what faces up, mud on what is near the ground, in broad soft drifts.
        let up = ((n.y - 0.35) / 0.5).clamp(0.0, 1.0);
        let low = 1.0 - ((p.y - 0.12) / 0.55).clamp(0.0, 1.0);
        let dust = w * (0.12 + 0.38 * up + 0.5 * low) * (0.6 + 0.6 * self.drift(p, 1.1, 11));
        v = v.lerp(self.dust * (0.35 + lum.sqrt() * 0.9).min(1.0), dust.clamp(0.0, 0.8));
        // Rust creeping up from the sills and blooming here and there, soft at its edges, the
        // more of it the more worn the thing is.
        let rusty = self.drift(p, 0.7, 23) + 0.3 * low;
        let from = 1.0 - w * w * 0.45;
        let rust = ((rusty - from) / 0.22).clamp(0.0, 1.0);
        v = v.lerp(self.rust * (0.7 + 0.3 * self.drift(p, 0.5, 31)), rust * rust * (3.0 - 2.0 * rust) * 0.6);
        let v = self.scuffed(p, v);
        LinearRgba::new(v.x, v.y, v.z, c.alpha)
    }

    /// `v` with the crash scrapes over it: dark patches, small and few at first, joining up as
    /// `scuffs` nears 1, with bare primer grey in the middle of the biggest.
    fn scuffed(&self, p: Vec3, v: Vec3) -> Vec3 {
        if self.scuffs <= 0.0 {
            return v;
        }
        let n = 0.65 * self.drift(p, 0.42, 47) + 0.35 * self.drift(p, 0.13, 53);
        let from = 0.82 - 0.5 * self.scuffs.min(1.0);
        let dark = ((n - from) / 0.07).clamp(0.0, 1.0);
        let bare = ((n - from - 0.12) / 0.05).clamp(0.0, 1.0);
        v.lerp(Vec3::splat(0.02), dark * 0.85).lerp(Vec3::new(0.2, 0.2, 0.21), bare * 0.7)
    }

    /// Smooth noise over the model, 0..1, its features `size` metres across.
    fn drift(&self, p: Vec3, size: f32, salt: u32) -> f32 {
        let q = p / size;
        let (i, f) = (q.floor(), q - q.floor());
        let f = f * f * (Vec3::splat(3.0) - 2.0 * f);
        let at = |dx: i32, dy: i32, dz: i32| hash(i.x as i32 + dx, i.y as i32 + dy, i.z as i32 + dz, self.seed ^ salt);
        let along = |dy: i32, dz: i32| at(0, dy, dz) + (at(1, dy, dz) - at(0, dy, dz)) * f.x;
        let plane = |dz: i32| along(0, dz) + (along(1, dz) - along(0, dz)) * f.y;
        plane(0) + (plane(1) - plane(0)) * f.z
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_file_reads_and_bad_ones_are_refused() {
        let look = shipped();
        assert!(look.wear["car"] > look.other && !look.paints.is_empty());
        assert_eq!(DecayLook::parse("()").unwrap(), look);
        assert_eq!(DecayLook::parse("(other: 0.0)").unwrap().other, 0.0);
        assert!(DecayLook::parse("(wear: {\"car\": 1.5})").is_err());
        assert!(DecayLook::parse("(mud: 1.0)").is_err());
    }

    /// The same prop is worn the same every time, cars differ from one another, and a thing with
    /// no wear keeps its colours.
    #[test]
    fn wear_is_steady_and_varied() {
        let look = shipped();
        let blue = Vec3::new(0.06, 0.13, 0.4);
        let c = LinearRgba::new(blue.x, blue.y, blue.z, 1.0);
        let at = |x: f32| Wear::of(&look, "car", Vec2::new(x, 40.0), Some(blue)).unwrap().colour(Vec3::new(0.2, 1.0, 0.3), Vec3::X, c);
        assert_eq!(at(10.0), at(10.0));
        let mut seen: Vec<[u8; 3]> = (0..40).map(|i| at(i as f32 * 3.0)).map(|c| [c.red, c.green, c.blue].map(|v| (v * 40.0) as u8)).collect();
        seen.sort();
        seen.dedup();
        assert!(seen.len() >= 6, "{} paints", seen.len());
        assert!(Wear::of(&DecayLook::none(), "car", Vec2::ZERO, Some(blue)).is_none());
        // Windows and tyres are not repainted.
        let glass = LinearRgba::new(0.33, 0.57, 0.69, 1.0);
        let w = Wear { amount: 0.0, ..Wear::of(&look, "car", Vec2::new(4.0, 4.0), Some(blue)).unwrap() };
        assert_eq!(w.colour(Vec3::Y, Vec3::X, glass), glass);
    }
}
