//! Furniture models built in Blender (`assets/models/furniture/<kind>.glb`, made by
//! `tools/blender`; see `.claude/skills/blender-assets/SKILL.md`). Read once at startup and merged
//! into each building's mesh in place of the grey-box shapes, so they get the world material (fog
//! of war, cutaways, lighting) and cost no extra draw calls. A kind with no model stays grey-box.
//!
//! A model faces south (+z in the game) with its origin at the centre of its footprint on the
//! floor; its footprint in tiles is in the glTF node's extras (`"footprint": [w, d]`). Placed
//! furniture is turned to its `facing`, a quarter more if that lines the model's long side up with
//! a differently shaped placement (a table's), and scaled to the placement's tiles when they differ
//! from the footprint.
//!
//! `ZOMBYE_EXTRA_MODELS` (folders separated by `:`) adds GLBs that aren't in the game yet, replacing
//! any of the same kind: a work-in-progress asset's `export/` folder in the art repo, to check it in
//! game before it's promoted. The MCP server passes its environment on to the `screenshot` client.
//!
//! A material's base colour is a triangle's colour, times the mean of its corners' vertex colours
//! (`COLOR_0`) where the model has them. Textures aren't read. Release builds put models from the
//! private art repo over these (`release_models.rs`).
//!
//! `<kind>.2.glb`, `<kind>.3.glb`, ... beside a kind's model are variants of it (same footprint): each
//! placement draws one of them, picked by where it stands, so not every sofa in town is the same sofa.

use std::collections::HashMap;
use std::path::Path;

use bevy::prelude::*;
use zombye_sim::map::Side;

use crate::meshgen::{Detail, MeshBuilder};

/// A triangle of a model: corners, their normals, its colour, and whether it is foliage.
type Tri = ([Vec3; 3], [Vec3; 3], LinearRgba, bool);

/// One model: triangles (position, normal) with a colour each, in the model's own frame.
pub struct FurnitureModel {
    tris: Vec<Tri>,
    /// A lighter stand-in for when the camera is zoomed far out ([`simplify`]), for a model heavy
    /// enough to want one: what a chunk's far props mesh holds in its place (`MeshBuilder::with_far`).
    far: Option<Vec<Tri>>,
    /// Footprint in tiles (x, z) when facing south.
    footprint: Vec2,
    /// How tall it stands.
    height: f32,
    /// Its main colour, if a saturated one covers a good part of it ([`body_colour`]).
    body: Option<Vec3>,
}

#[derive(Default)]
pub struct FurnitureModels(HashMap<String, FurnitureModel>);

impl FurnitureModels {
    /// Every `*.glb` in `dir`, by file name. Unreadable ones are skipped with a warning.
    pub fn load(dir: &Path) -> Self {
        let mut models = HashMap::new();
        let Ok(entries) = std::fs::read_dir(dir) else { return Self(models) };
        for path in entries.flatten().map(|e| e.path()) {
            if path.extension().is_none_or(|e| e != "glb") {
                continue;
            }
            let kind = path.file_stem().unwrap().to_string_lossy().into_owned();
            match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| parse_glb(&b)) {
                Ok(m) => {
                    models.insert(kind, m);
                }
                Err(e) => warn!("furniture model {}: {e}", path.display()),
            }
        }
        Self(models)
    }

    /// Adds `other`'s models, replacing any of the same kind.
    pub fn extend(&mut self, other: FurnitureModels) {
        self.0.extend(other.0);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// How many triangles the model for `kind` has, if there is one.
    #[cfg(test)]
    pub fn triangles(&self, kind: &str) -> Option<usize> {
        self.0.get(kind).map(|m| m.tris.len())
    }

    /// For checking a folder of models: each one's name, triangles, how many of them are foliage,
    /// its size (x, y up, z) and the mean of its colours.
    #[cfg(test)]
    pub fn report(&self) -> Vec<(String, usize, usize, Vec3, Vec3)> {
        let mut out: Vec<_> = self
            .0
            .iter()
            .map(|(k, m)| {
                let (mut lo, mut hi, mut c) = (Vec3::MAX, Vec3::MIN, Vec3::ZERO);
                for (p, _, col, _) in &m.tris {
                    for p in p {
                        lo = lo.min(*p);
                        hi = hi.max(*p);
                    }
                    c += Vec3::new(col.red, col.green, col.blue);
                }
                (k.clone(), m.tris.len(), m.tris.iter().filter(|t| t.3).count(), hi - lo, c / m.tris.len().max(1) as f32)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// The model for `kind` as a mesh of its own (positions, normals, a colour a vertex), for things
    /// drawn as entities and not merged into a chunk: items on the floor.
    pub fn mesh(&self, kind: &str) -> Option<Mesh> {
        let m = self.0.get(kind)?;
        let (mut pos, mut nrm, mut col) = (Vec::new(), Vec::new(), Vec::new());
        for (p, n, c, _) in &m.tris {
            for k in 0..3 {
                pos.push(p[k].to_array());
                nrm.push(n[k].to_array());
                col.push([c.red, c.green, c.blue, 1.0]);
            }
        }
        Some(
            Mesh::new(bevy::mesh::PrimitiveTopology::TriangleList, bevy::asset::RenderAssetUsages::RENDER_WORLD)
                .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
                .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, nrm)
                .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, col),
        )
    }

    /// The triangles of the model named `name` as they are: corners, colour, and whether the material
    /// is foliage (`leaves...`). For the instanced ground cover, which builds its own meshes.
    pub fn raw(&self, name: &str) -> Option<Vec<([Vec3; 3], LinearRgba, bool)>> {
        Some(self.0.get(name)?.tris.iter().map(|(p, _, c, leaf)| (*p, *c, *leaf)).collect())
    }

    /// The model drawn for `kind` at a place: the kind's own or one of its variants (`<kind>.2.glb`,
    /// `<kind>.3.glb`, ... beside it, numbered without a gap), the same one every time for the same `salt`.
    fn pick(&self, kind: &str, salt: u32) -> Option<&FurnitureModel> {
        let first = self.0.get(kind)?;
        let mut n = 1;
        while self.0.contains_key(&format!("{kind}.{}", n + 1)) {
            n += 1;
        }
        let k = (salt.wrapping_mul(0x9E37_79B9) >> 16) % n;
        if k == 0 { Some(first) } else { self.0.get(&format!("{kind}.{}", k + 1)) }
    }

    /// How many models `kind` has to pick from (its own and its variants).
    pub fn variants(&self, kind: &str) -> u32 {
        if !self.0.contains_key(kind) {
            return 0;
        }
        (2..).take_while(|n| self.0.contains_key(&format!("{kind}.{n}"))).count() as u32 + 1
    }

    /// How tall variant `k` (from 0) of `kind` is, if there is one.
    pub fn height(&self, kind: &str, k: u32) -> Option<f32> {
        let key = if k == 0 { kind.to_string() } else { format!("{kind}.{}", k + 1) };
        self.0.get(&key).map(|m| m.height)
    }

    /// Adds variant `k` (from 0) of `kind` at its own proportions: its foot at `origin`, turned `yaw`
    /// about the vertical, scaled so it stands `height` tall. Triangles of a material named `leaves...`
    /// take the tag `leaves`, the rest `wood` (trees: the foliage fades, the trunk doesn't). False when
    /// there's no such model.
    pub fn add_standing(&self, mb: &mut MeshBuilder, kind: &str, k: u32, origin: Vec3, yaw: f32, height: f32, wood: crate::meshgen::CutTag, leaves: crate::meshgen::CutTag) -> bool {
        let key = if k == 0 { kind.to_string() } else { format!("{kind}.{}", k + 1) };
        let Some(m) = self.0.get(&key) else { return false };
        let rot = Quat::from_rotation_y(yaw);
        let s = height / m.height.max(0.01);
        for (detail, tris) in m.details() {
            mb.only(detail);
            for pass in [false, true] {
                mb.set_tag(if pass { leaves } else { wood });
                for (p, n, c, leaf) in tris {
                    if *leaf == pass {
                        mb.triangle_with_normals(p.map(|p| origin + rot * p * s), n.map(|n| rot * n), *c);
                    }
                }
            }
        }
        mb.only(Detail::Both);
        true
    }

    /// Adds the model for `kind` filling the tile rectangle (`min`, `size` in metres) and turned to
    /// `facing`. False when there's no model for it.
    pub fn add(&self, mb: &mut MeshBuilder, kind: &str, min: Vec2, size: Vec2, facing: Side, base_y: f32) -> bool {
        let Some(m) = self.pick(kind, (min.x as i32 as u32).wrapping_mul(73_856_093) ^ (min.y as i32 as u32).wrapping_mul(19_349_663)) else { return false };
        let mut turn = match facing {
            Side::S => 0,
            Side::E => 1,
            Side::N => 2,
            Side::W => 3,
        };
        let turned = |turn: i32| if turn % 2 == 0 { m.footprint } else { m.footprint.yx() };
        let long = |v: Vec2| (v.x - v.y).signum();
        if long(size) != 0.0 && long(turned(turn)) == -long(size) {
            turn += 1;
        }
        let fp = turned(turn);
        let scale = Vec3::new(size.x / fp.x, 1.0, size.y / fp.y);
        let origin = Vec3::new(min.x + size.x * 0.5, base_y, min.y + size.y * 0.5);
        m.emit(mb, origin, turn, scale);
        true
    }

    /// Adds the model for `kind` at its own size (never stretched to a placement: small street props
    /// stand in a tile without filling it), the middle of its footprint at `centre`, turned to
    /// `facing`. False when there's no model for it.
    pub fn add_at(&self, mb: &mut MeshBuilder, kind: &str, centre: Vec2, facing: Side, base_y: f32) -> bool {
        self.add_at_worn(mb, kind, centre, facing, base_y, None)
    }

    /// The main colour of the model [`Self::add_at`] draws for `kind` at `centre`: the saturated
    /// colour that covers most of it (a car's paintwork), if one covers a good part.
    pub fn body_colour(&self, kind: &str, centre: Vec2) -> Option<Vec3> {
        self.pick(kind, Self::salt_at(centre))?.body
    }

    fn salt_at(centre: Vec2) -> u32 {
        (centre.x as i32 as u32).wrapping_mul(73_856_093) ^ (centre.y as i32 as u32).wrapping_mul(19_349_663)
    }

    /// [`Self::add_at`], each triangle's colour worn as `wear` says (`meshgen::decay`).
    pub fn add_at_worn(&self, mb: &mut MeshBuilder, kind: &str, centre: Vec2, facing: Side, base_y: f32, wear: Option<&crate::meshgen::decay::Wear>) -> bool {
        let Some(m) = self.pick(kind, Self::salt_at(centre)) else { return false };
        let turn = match facing {
            Side::S => 0,
            Side::E => 1,
            Side::N => 2,
            Side::W => 3,
        };
        m.emit_worn(mb, Vec3::new(centre.x, base_y, centre.y), turn, Vec3::ONE, wear);
        true
    }
}

impl FurnitureModel {
    /// Every triangle, scaled, turned `turn` quarters and moved to `origin`.
    fn emit(&self, mb: &mut MeshBuilder, origin: Vec3, turn: i32, scale: Vec3) {
        self.emit_worn(mb, origin, turn, scale, None);
    }

    fn emit_worn(&self, mb: &mut MeshBuilder, origin: Vec3, turn: i32, scale: Vec3, wear: Option<&crate::meshgen::decay::Wear>) {
        // Positive turns about +y take the model's front (+z) toward +x (east).
        let rot = Quat::from_rotation_y(turn as f32 * std::f32::consts::FRAC_PI_2);
        for (detail, tris) in self.details() {
            mb.only(detail);
            for (p, n, c, _) in tris {
                let c = match wear {
                    Some(w) => w.colour((p[0] + p[1] + p[2]) / 3.0, (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero(), *c),
                    None => *c,
                };
                let p = p.map(|p| origin + rot * p * scale);
                let n = n.map(|n| ((rot * n) / scale).normalize_or_zero());
                mb.triangle_with_normals(p, n, c);
            }
        }
        mb.only(Detail::Both);
    }

    /// What to add to which copy of a mesh: everything to both, or the model to the full mesh and
    /// its stand-in to the far copy.
    fn details(&self) -> impl Iterator<Item = (Detail, &[Tri])> {
        let both = self.far.is_none().then_some((Detail::Both, &self.tris[..]));
        let far = self.far.as_deref().map(|far| [(Detail::Near, &self.tris[..]), (Detail::Far, far)]);
        both.into_iter().chain(far.into_iter().flatten())
    }
}

/// A model's main colour: the saturated colour covering the most of its surface, if that is at
/// least a tenth of it (a car's paintwork; not its tyres, glass or lights).
fn body_colour(tris: &[Tri]) -> Option<Vec3> {
    let mut by_colour: Vec<(Vec3, f32)> = Vec::new();
    let mut total = 0.0;
    for (p, _, c, _) in tris {
        let area = (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5;
        total += area;
        let v = Vec3::new(c.red, c.green, c.blue);
        let (hi, lo) = (v.max_element(), v.min_element());
        if hi < 0.08 || (hi - lo) / hi < 0.45 {
            continue;
        }
        match by_colour.iter_mut().find(|(k, _)| (*k - v).abs().max_element() < 0.02) {
            Some(slot) => slot.1 += area,
            None => by_colour.push((v, area)),
        }
    }
    by_colour.into_iter().max_by(|a, b| a.1.total_cmp(&b.1)).filter(|(_, area)| *area > total * 0.1).map(|(v, _)| v)
}

fn parse_glb(bytes: &[u8]) -> Result<FurnitureModel, String> {
    let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| e.to_string())?;
    let blob = gltf.blob.as_deref();
    let mut tris = Vec::new();
    let mut footprint = None;
    for node in gltf.nodes() {
        let Some(mesh) = node.mesh() else { continue };
        let tf = Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(extras) = node.extras() {
            #[derive(serde::Deserialize)]
            struct Extras {
                footprint: Option<[f32; 2]>,
            }
            if let Ok(Extras { footprint: Some([w, d]) }) = serde_json::from_str(extras.get()) {
                footprint = Some(Vec2::new(w, d));
            }
        }
        for prim in mesh.primitives() {
            let c = prim.material().pbr_metallic_roughness().base_color_factor();
            let leaf = prim.material().name().is_some_and(|n| n.starts_with("leaves"));
            let color = LinearRgba::new(c[0], c[1], c[2], 1.0);
            let reader = prim.reader(|b| match b.source() {
                gltf::buffer::Source::Bin => blob,
                gltf::buffer::Source::Uri(_) => None,
            });
            let pos: Vec<Vec3> = reader.read_positions().ok_or("a primitive without positions")?.map(|p| tf.transform_point3(Vec3::from(p))).collect();
            let nrm: Vec<Vec3> = match reader.read_normals() {
                Some(n) => n.map(|n| tf.transform_vector3(Vec3::from(n)).normalize_or_zero()).collect(),
                None => return Err("a primitive without normals".into()),
            };
            // Vertex colours, where a model has them (one painted from a texture atlas): a triangle
            // takes the mean of its corners', times the material's colour.
            let tint: Option<Vec<[f32; 4]>> = reader.read_colors(0).map(|c| c.into_rgba_f32().collect());
            let idx: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..pos.len() as u32).collect(),
            };
            for t in idx.chunks_exact(3) {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| i as usize);
                if a.max(b).max(c) >= pos.len() {
                    return Err("index out of range".into());
                }
                let color = match &tint {
                    Some(t) if a.max(b).max(c) < t.len() => {
                        let m = (Vec4::from(t[a]) + Vec4::from(t[b]) + Vec4::from(t[c])) / 3.0;
                        LinearRgba::new(color.red * m.x, color.green * m.y, color.blue * m.z, 1.0)
                    }
                    _ => color,
                };
                tris.push(([pos[a], pos[b], pos[c]], [nrm[a], nrm[b], nrm[c]], color, leaf));
            }
        }
    }
    if tris.is_empty() {
        return Err("no triangles".into());
    }
    // Without extras, the tiles its bounds cover.
    let footprint = footprint.unwrap_or_else(|| {
        let (lo, hi) = tris.iter().flat_map(|t| t.0).fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(p), hi.max(p)));
        Vec2::new((hi.x - lo.x).ceil().max(1.0), (hi.z - lo.z).ceil().max(1.0))
    });
    let height = tris.iter().flat_map(|t| t.0).fold(0.0f32, |h, p| h.max(p.y));
    let far = simplify(&tris);
    let body = body_colour(&tris);
    Ok(FurnitureModel { tris, far, footprint, height, body })
}

/// A part of a model (its foliage, or the rest) of at most this many triangles is drawn as it is
/// at any zoom.
const FAR_KEEP: usize = 200;
/// A stand-in's part has about this share of the part's triangles (and needn't go under [`FAR_KEEP`]).
const FAR_SHARE: f32 = 0.3;
/// No corner of a stand-in is moved where that would take it further than this (metres) from the
/// faces it was a corner of: at the zooms a stand-in is drawn at a metre is under 27 pixels, so
/// this is about two of them. A model that is all sharp detail keeps more than [`FAR_SHARE`].
const FAR_ERROR: f32 = 0.07;
/// Foliage is thinned (some of its leaves dropped, the rest grown to cover as much) only when it is
/// made of at least this many separate pieces, none of them more than [`FAR_PIECE`] of it: a crown
/// of loose leaves or strands, not a few solid masses.
/// A stand-in's surface is at least this share of its part's (and at most one over it).
const FAR_COVER: f32 = 0.88;
const FAR_PIECES: usize = 24;
const FAR_PIECE: f32 = 0.1;

/// A lighter stand-in for a heavy model, for when the camera is zoomed far out; none for a model
/// light enough already, or that can't be made much lighter without showing. Foliage and the rest
/// are done apart, each only when heavy: a crown of many loose leaves is [`thin`]ned, anything else
/// has its edges [`collapse`]d. Colours and which triangles are foliage stay.
fn simplify(tris: &[Tri]) -> Option<Vec<Tri>> {
    let (leaves, rest): (Vec<Tri>, Vec<Tri>) = tris.iter().partition(|t| t.3);
    let mut out = Vec::with_capacity(tris.len() / 2);
    for part in [rest, leaves] {
        let want = ((part.len() as f32 * FAR_SHARE) as usize).max(FAR_KEEP);
        // Collapsing can fold thin, layered shapes (a conifer's skirts, a washing line) away
        // without moving any point far: what covers much less than the part did is tried again
        // more carefully, then left as it was.
        let fair = |t: &Vec<Tri>| (FAR_COVER..=1.0 / FAR_COVER).contains(&(area(t) / area(&part).max(1e-6)));
        let lighter = if part.len() <= FAR_KEEP {
            None
        } else {
            part[0].3.then(|| thin(&part, want)).flatten().or_else(|| [1.0, 0.3, 0.1].into_iter().map(|e| collapse(&part, want, FAR_ERROR * e)).find(fair))
        };
        out.extend(lighter.unwrap_or(part));
    }
    (out.len() * 10 < tris.len() * 8).then_some(out)
}

fn area(tris: &[Tri]) -> f32 {
    tris.iter().map(|(p, ..)| (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5).sum()
}

/// A plane's squared distance as a quadric (the ten numbers of a symmetric 4 x 4), to add up.
#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    /// Of the plane through `p` with unit normal `n`, weighted.
    fn plane(n: Vec3, p: Vec3, weight: f64) -> Self {
        let (a, b, c) = (n.x as f64, n.y as f64, n.z as f64);
        let d = -(a * p.x as f64 + b * p.y as f64 + c * p.z as f64);
        Quadric([a * a, a * b, a * c, a * d, b * b, b * c, b * d, c * c, c * d, d * d].map(|v| v * weight))
    }

    fn add(&mut self, o: &Quadric) {
        for (a, b) in self.0.iter_mut().zip(o.0) {
            *a += b;
        }
    }

    /// The summed squared distance of `p` from the planes added up.
    fn at(&self, p: Vec3) -> f64 {
        let (x, y, z) = (p.x as f64, p.y as f64, p.z as f64);
        let q = &self.0;
        q[0] * x * x + 2.0 * q[1] * x * y + 2.0 * q[2] * x * z + 2.0 * q[3] * x + q[4] * y * y + 2.0 * q[5] * y * z + 2.0 * q[6] * y + q[7] * z * z + 2.0 * q[8] * z + q[9]
    }
}

/// `tris` with edges collapsed, cheapest first, until about `want` are left or the next would move
/// a corner more than `max_error` metres off the faces it belonged to (quadric error simplification,
/// a corner always moving onto its neighbour, so no point of the stand-in is a new one). Edges with
/// a face on one side only (the rim of a leaf, of a sheet) are held in their line, so open shapes
/// don't shrink; a collapse that would turn a face over is skipped. A flat-shaded face is flat-shaded
/// still; a smooth one keeps its corners' normals.
fn collapse(tris: &[Tri], want: usize, max_error: f32) -> Vec<Tri> {
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashSet};

    // Corners at the same point are one vertex.
    let mut ids: HashMap<IVec3, u32> = HashMap::new();
    let mut verts: Vec<Vec3> = Vec::new();
    let mut faces: Vec<[u32; 3]> = Vec::with_capacity(tris.len());
    for (p, ..) in tris {
        faces.push(p.map(|p| {
            *ids.entry((p * 2000.0).round().as_ivec3()).or_insert_with(|| {
                verts.push(p);
                verts.len() as u32 - 1
            })
        }));
    }
    let normal = |f: [u32; 3], verts: &[Vec3]| (verts[f[1] as usize] - verts[f[0] as usize]).cross(verts[f[2] as usize] - verts[f[0] as usize]);
    let flat = |f: [u32; 3]| f[0] == f[1] || f[1] == f[2] || f[0] == f[2];
    let mut alive: Vec<bool> = faces.iter().map(|&f| !flat(f) && normal(f, &verts).length_squared() > 1e-14).collect();
    let mut left = alive.iter().filter(|a| **a).count();

    // Each vertex's faces, and the planes it lies on.
    let mut of: Vec<Vec<u32>> = vec![Vec::new(); verts.len()];
    let mut quadrics = vec![Quadric::default(); verts.len()];
    // How many faces each edge has, the two sides of a sheet counting once.
    let mut sides: HashMap<(u32, u32), u32> = HashMap::new();
    let mut counted: HashSet<[u32; 3]> = HashSet::new();
    let edge = |a: u32, b: u32| (a.min(b), a.max(b));
    for (i, &f) in faces.iter().enumerate() {
        if !alive[i] {
            continue;
        }
        let n = normal(f, &verts).normalize();
        let q = Quadric::plane(n, verts[f[0] as usize], 1.0);
        let mut sorted = f;
        sorted.sort_unstable();
        let first = counted.insert(sorted);
        for k in 0..3 {
            of[f[k] as usize].push(i as u32);
            quadrics[f[k] as usize].add(&q);
            if first {
                *sides.entry(edge(f[k], f[(k + 1) % 3])).or_default() += 1;
            }
        }
    }
    for (i, &f) in faces.iter().enumerate() {
        if !alive[i] {
            continue;
        }
        let n = normal(f, &verts).normalize();
        for k in 0..3 {
            let (a, b) = (f[k], f[(k + 1) % 3]);
            if sides[&edge(a, b)] == 1 {
                // A rim: a plane through the edge, square to the face, holds it in its line.
                let across = (verts[b as usize] - verts[a as usize]).cross(n).normalize_or_zero();
                let q = Quadric::plane(across, verts[a as usize], 8.0);
                quadrics[a as usize].add(&q);
                quadrics[b as usize].add(&q);
            }
        }
    }

    // Moving `from` onto `to`: what it costs.
    let cost = |from: u32, to: u32, quadrics: &[Quadric], verts: &[Vec3]| {
        let mut q = quadrics[from as usize];
        q.add(&quadrics[to as usize]);
        q.at(verts[to as usize]).max(0.0)
    };
    let key = |c: f64| Reverse((c * 1e12) as u64);
    let mut stamp = vec![0u32; verts.len()];
    let mut heap = BinaryHeap::new();
    for &(a, b) in sides.keys() {
        heap.push((key(cost(a, b, &quadrics, &verts)), a, b, 0u32, 0u32));
        heap.push((key(cost(b, a, &quadrics, &verts)), b, a, 0u32, 0u32));
    }
    let limit = (max_error as f64) * (max_error as f64);
    while left > want {
        let Some((Reverse(c), from, to, s_from, s_to)) = heap.pop() else { break };
        if stamp[from as usize] != s_from || stamp[to as usize] != s_to {
            continue;
        }
        if c as f64 / 1e12 > limit {
            break;
        }
        // Would a face that stays be turned over, or flattened to nothing?
        let turned = of[from as usize].iter().any(|&i| {
            let f = faces[i as usize];
            if !alive[i as usize] || f.contains(&to) {
                return false;
            }
            let before = normal(f, &verts);
            let moved = f.map(|v| if v == from { to } else { v });
            let after = normal(moved, &verts);
            after.length_squared() < 1e-14 || before.dot(after) <= 0.2 * before.length() * after.length()
        });
        if turned {
            continue;
        }
        let moved = std::mem::take(&mut of[from as usize]);
        for i in moved {
            if !alive[i as usize] {
                continue;
            }
            let f = &mut faces[i as usize];
            if f.contains(&to) {
                alive[i as usize] = false;
                left -= 1;
            } else {
                *f = f.map(|v| if v == from { to } else { v });
                of[to as usize].push(i);
            }
        }
        let q = quadrics[from as usize];
        quadrics[to as usize].add(&q);
        stamp[from as usize] = u32::MAX;
        stamp[to as usize] += 1;
        of[to as usize].retain(|&i| alive[i as usize]);
        // The edges round the vertex that stayed cost something else now.
        let mut round: Vec<u32> = of[to as usize].iter().flat_map(|&i| faces[i as usize]).filter(|&v| v != to).collect();
        round.sort_unstable();
        round.dedup();
        for v in round {
            heap.push((key(cost(to, v, &quadrics, &verts)), to, v, stamp[to as usize], stamp[v as usize]));
            heap.push((key(cost(v, to, &quadrics, &verts)), v, to, stamp[v as usize], stamp[to as usize]));
        }
    }

    let mut out = Vec::with_capacity(left);
    for (i, (p, n, colour, leaf)) in tris.iter().enumerate() {
        if !alive[i] {
            continue;
        }
        let corners = faces[i].map(|v| verts[v as usize]);
        let was_flat = n[0].dot(n[1]) > 0.999 && n[0].dot(n[2]) > 0.999;
        let facet = (corners[1] - corners[0]).cross(corners[2] - corners[0]).normalize_or_zero();
        let was = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
        // Flat faces follow their new plane (keeping the side their normal was on); smooth ones
        // keep their corners' normals.
        let normals = if was_flat { [facet * if was.dot(n[0]) < 0.0 { -1.0 } else { 1.0 }; 3] } else { *n };
        out.push((corners, normals, *colour, *leaf));
    }
    out
}

/// The separate pieces of `tris` (triangles joined by shared corners), as lists of indices.
fn pieces(tris: &[Tri]) -> Vec<Vec<usize>> {
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut parent: Vec<usize> = (0..tris.len()).collect();
    let mut at: HashMap<IVec3, usize> = HashMap::new();
    for (i, (p, ..)) in tris.iter().enumerate() {
        for p in p {
            let other = *at.entry((*p * 2000.0).round().as_ivec3()).or_insert(i);
            let (a, b) = (find(&mut parent, i), find(&mut parent, other));
            parent[a] = b;
        }
    }
    let mut by_root: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..tris.len() {
        let root = find(&mut parent, i);
        by_root.entry(root).or_default().push(i);
    }
    let mut out: Vec<Vec<usize>> = by_root.into_values().collect();
    out.sort_by_key(|piece| piece[0]);
    out
}

/// Foliage of many loose leaves with only some of them kept, about `want` triangles' worth: every
/// few-th leaf going up the tree, each grown about its own middle so that together they cover as
/// much as all of them did. None when the foliage isn't that kind (see [`FAR_PIECES`]).
fn thin(tris: &[Tri], want: usize) -> Option<Vec<Tri>> {
    let mut pieces = pieces(tris);
    let biggest = pieces.iter().map(Vec::len).max()?;
    if pieces.len() < FAR_PIECES || biggest as f32 > tris.len() as f32 * FAR_PIECE {
        return None;
    }
    let middle = |piece: &Vec<usize>| piece.iter().flat_map(|&i| tris[i].0).sum::<Vec3>() / (piece.len() * 3) as f32;
    pieces.sort_by(|a, b| middle(a).y.total_cmp(&middle(b).y));
    let every = (tris.len() as f32 / want as f32).max(1.0);
    let mut out = Vec::with_capacity(want + biggest);
    let mut due = 0.0;
    for (k, piece) in pieces.iter().enumerate() {
        if (k as f32) < due {
            continue;
        }
        due += every;
        let (centre, grow) = (middle(piece), every.sqrt());
        out.extend(piece.iter().map(|&i| {
            let (p, n, c, leaf) = tris[i];
            (p.map(|p| centre + (p - centre) * grow), n, c, leaf)
        }));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> FurnitureModels {
        FurnitureModels::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models/furniture"))
    }

    /// One triangle with a colour on each corner, as a GLB.
    fn painted_triangle(base: [f32; 4], corners: [[f32; 4]; 3]) -> Vec<u8> {
        let mut bin: Vec<u8> = Vec::new();
        let f = |bin: &mut Vec<u8>, v: &[f32]| v.iter().for_each(|x| bin.extend_from_slice(&x.to_le_bytes()));
        f(&mut bin, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
        f(&mut bin, &[0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0]);
        corners.iter().for_each(|c| f(&mut bin, c));
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"COLOR_0":2}},"material":0}}]}}],
            "materials":[{{"pbrMetallicRoughness":{{"baseColorFactor":{base:?}}}}}],
            "buffers":[{{"byteLength":120}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":36}},{{"buffer":0,"byteOffset":72,"byteLength":48}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,0,1]}},
            {{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}},{{"bufferView":2,"componentType":5126,"count":3,"type":"VEC4"}}]}}"#
        );
        let mut json = json.into_bytes();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&bin);
        glb
    }

    #[test]
    fn vertex_colours_tint_the_material() {
        let glb = painted_triangle([1.0, 0.5, 1.0, 1.0], [[0.9, 0.2, 0.0, 1.0], [0.3, 0.2, 0.0, 1.0], [0.0, 0.2, 0.6, 1.0]]);
        let m = parse_glb(&glb).unwrap();
        assert_eq!(m.tris.len(), 1);
        let c = m.tris[0].2;
        assert!((c.red - 0.4).abs() < 1e-5 && (c.green - 0.1).abs() < 1e-5 && (c.blue - 0.2).abs() < 1e-5, "{c:?}");
    }

    /// Prints what the stand-ins of a folder of models come to (`ZOMBYE_MODELS_DIR`).
    #[test]
    #[ignore]
    fn stand_in_report() {
        let dir = std::path::PathBuf::from(std::env::var_os("ZOMBYE_MODELS_DIR").expect("set ZOMBYE_MODELS_DIR"));
        let models = FurnitureModels::load(&dir);
        let mut names: Vec<_> = models.0.keys().cloned().collect();
        names.sort();
        for name in names {
            let m = &models.0[&name];
            let leaves: Vec<Tri> = m.tris.iter().filter(|t| t.3).copied().collect();
            let p = pieces(&leaves);
            println!("{name}: {} ({} foliage in {} pieces, biggest {}) -> {:?}", m.tris.len(), leaves.len(), p.len(), p.iter().map(Vec::len).max().unwrap_or(0), m.far.as_ref().map(|f| (f.len(), area(f) / area(&m.tris))));
        }
    }

    /// A flat sheet `n` x `n` squares of `step` metres, standing in the x-y plane, both sides, the
    /// top half foliage: a leaf card cut finely, as the release trees' are.
    fn fine_sheet(n: i32, step: f32) -> Vec<Tri> {
        let mut tris = Vec::new();
        let at = |i: i32, j: i32| Vec3::new(i as f32 * step, j as f32 * step, 0.0);
        for j in 0..n {
            for i in 0..n {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                let leaf = j >= n / 2;
                for (corners, normal) in [([a, b, c], Vec3::Z), ([a, c, d], Vec3::Z), ([c, b, a], Vec3::NEG_Z), ([d, c, a], Vec3::NEG_Z)] {
                    tris.push((corners, [normal; 3], LinearRgba::GREEN, leaf));
                }
            }
        }
        tris
    }

    fn bounds(tris: &[Tri]) -> (Vec3, Vec3) {
        tris.iter().flat_map(|t| t.0).fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(p), hi.max(p)))
    }

    /// A heavy model gets a stand-in for far zooms with a fraction of its triangles that keeps its
    /// outline (to a grid cell), both of its sides, its colours, normals and which part is foliage;
    /// a light one is drawn as it is.
    #[test]
    fn heavy_models_get_a_lighter_stand_in() {
        let sheet = fine_sheet(40, 0.025);
        assert_eq!(sheet.len(), 6400);
        let far = simplify(&sheet).expect("a stand-in");
        assert!(far.len() <= 6400 / 3 && far.len() >= 100, "{} triangles", far.len());
        let ((lo, hi), (flo, fhi)) = (bounds(&sheet), bounds(&far));
        assert!((lo - flo).abs().max_element() < 0.03 && (hi - fhi).abs().max_element() < 0.03, "{flo} {fhi}: the rim holds");
        assert!((area(&far) / area(&sheet) - 1.0).abs() < 0.05, "it covers as much");
        let facing = |z: f32| far.iter().filter(|(p, ..)| (p[1] - p[0]).cross(p[2] - p[0]).z * z > 0.0).count();
        assert_eq!(facing(1.0), facing(-1.0), "both sides of the card");
        assert!(far.iter().all(|(p, n, c, leaf)| *c == LinearRgba::GREEN && n[0].z.abs() > 0.999 && *leaf == (p[0].y + p[1].y + p[2].y > 1.5)));
        assert!(far.iter().all(|(p, ..)| (p[1] - p[0]).cross(p[2] - p[0]).length() > 1e-6), "no collapsed triangles");
        // Light already: left alone. And a shape that is all corners keeps them: a zigzag whose
        // every edge is a fold has nothing to give within the error allowed.
        assert!(simplify(&fine_sheet(7, 0.025)).is_none(), "196 triangles");
        let zigzag: Vec<Tri> = (0..300).map(|i| { let x = i as f32 * 0.5; ([Vec3::new(x, 0.0, 0.0), Vec3::new(x + 0.5, 0.0, 0.0), Vec3::new(x + 0.25, 1.0, if i % 2 == 0 { 0.5 } else { -0.5 })], [Vec3::Z; 3], LinearRgba::GREEN, false) }).collect();
        assert!(simplify(&zigzag).is_none());
    }

    /// The stand-in goes to a mesh's far copy and the model to the full mesh, in the same place;
    /// a mesh with no far copy, and a model with no stand-in, are as before.
    #[test]
    fn stand_ins_go_to_the_far_copy_of_a_mesh() {
        let tris = fine_sheet(40, 0.025);
        let heavy = FurnitureModel { far: simplify(&tris), tris, footprint: Vec2::ONE, height: 1.0, body: None };
        let light = FurnitureModel { far: None, tris: fine_sheet(4, 0.25), footprint: Vec2::ONE, height: 1.0, body: None };
        let far_tris = heavy.far.as_ref().unwrap().len();
        let models = FurnitureModels(HashMap::from([("tree".to_string(), heavy), ("post".to_string(), light)]));
        let tag = crate::meshgen::CutTag::default();

        let mut mb = MeshBuilder::default().with_far();
        assert!(models.add_at(&mut mb, "post", Vec2::new(3.5, 3.5), Side::S, 0.0));
        mb.add_box(Vec3::ZERO, Vec3::ONE, crate::meshgen::same(LinearRgba::WHITE));
        assert!(mb.take_far().is_none(), "nothing with a stand-in: no far copy");

        let mut mb = MeshBuilder::default().with_far();
        mb.add_box(Vec3::ZERO, Vec3::ONE, crate::meshgen::same(LinearRgba::WHITE));
        assert!(models.add_standing(&mut mb, "tree", 0, Vec3::new(5.0, 0.0, 9.0), 1.0, 4.0, tag, tag));
        assert!(models.add_at(&mut mb, "tree", Vec2::new(12.5, 3.5), Side::E, 0.0));
        mb.add_box(Vec3::ZERO, Vec3::ONE, crate::meshgen::same(LinearRgba::WHITE));
        let far = mb.take_far().expect("a far copy");
        assert_eq!(mb.tri_count(), 24 + 2 * 6400);
        assert_eq!(far.tri_count(), 24 + 2 * far_tris);
        let ((lo, hi), (flo, fhi)) = (mb.bounds(), far.bounds());
        assert!((lo - flo).abs().max_element() < 0.5 && (hi - fhi).abs().max_element() < 0.5, "{lo} {hi} / {flo} {fhi}");

        // No far copy asked for (a building's furniture): the model, once.
        let mut mb = MeshBuilder::default();
        assert!(models.add_at(&mut mb, "tree", Vec2::new(12.5, 3.5), Side::E, 0.0));
        assert_eq!(mb.tri_count(), 6400);
        assert!(mb.take_far().is_none());
    }

    #[test]
    fn every_template_furniture_kind_but_stairs_has_a_model() {
        let models = models();
        let templates = zombye_net::content::load_building_templates(&zombye_net::content::default_data_dir()).unwrap();
        for t in &templates {
            for level in &t.levels {
                for f in &level.furniture {
                    assert!(f.kind == "stairs" || models.0.contains_key(&f.kind), "no model for {}", f.kind);
                }
            }
        }
    }

    #[test]
    fn models_fill_their_placement_whichever_way_they_face() {
        let models = models();
        for (kind, m) in &models.0 {
            for facing in [Side::N, Side::E, Side::S, Side::W] {
                let size = if facing == Side::E || facing == Side::W { m.footprint.yx() } else { m.footprint };
                let mut mb = MeshBuilder::default();
                assert!(models.add(&mut mb, kind, Vec2::new(10.0, 20.0), size, facing, 3.0));
                let (lo, hi) = mb.bounds();
                // Inside the tiles (walls take 8 cm off each side) and standing on the floor.
                assert!(lo.x >= 10.0 + 0.07 && lo.z >= 20.0 + 0.07, "{kind} {facing:?}: {lo}");
                assert!(hi.x <= 10.0 + size.x - 0.07 && hi.z <= 20.0 + size.y - 0.07, "{kind} {facing:?}: {hi}");
                assert!((lo.y - 3.0).abs() < 1e-3, "{kind} {facing:?}: floor at {}", lo.y);
            }
        }
    }

    #[test]
    fn wall_backed_models_turn_their_back_to_the_wall() {
        let models = models();
        let sofa = &models.0["sofa"];
        // Facing east: the back is against the west side of the placement.
        let size = sofa.footprint.yx();
        let mut mb = MeshBuilder::default();
        models.add(&mut mb, "sofa", Vec2::ZERO, size, Side::E, 0.0);
        let (lo, _) = mb.bounds();
        assert!(lo.x < 0.1, "sofa back at x = {}", lo.x);
        // A table placed 1 × 2 turns its long side along z.
        let mut mb = MeshBuilder::default();
        models.add(&mut mb, "table", Vec2::ZERO, Vec2::new(1.0, 2.0), Side::S, 0.0);
        let (lo, hi) = mb.bounds();
        assert!(hi.z - lo.z > 1.5 && hi.x - lo.x < 1.0, "table {lo} {hi}");
    }
}
