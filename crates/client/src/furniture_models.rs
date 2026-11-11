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

use crate::meshgen::MeshBuilder;

/// One model: triangles (position, normal) with a colour each, in the model's own frame.
pub struct FurnitureModel {
    tris: Vec<([Vec3; 3], [Vec3; 3], LinearRgba, bool)>,
    /// Footprint in tiles (x, z) when facing south.
    footprint: Vec2,
    /// How tall it stands.
    height: f32,
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

    /// Adds variant `k` (from 0) of `kind` at its own proportions: its foot at `origin`, turned `yaw`
    /// about the vertical, scaled so it stands `height` tall. Triangles of a material named `leaves...`
    /// take the tag `leaves`, the rest `wood` (trees: the foliage fades, the trunk doesn't). False when
    /// there's no such model.
    pub fn add_standing(&self, mb: &mut MeshBuilder, kind: &str, k: u32, origin: Vec3, yaw: f32, height: f32, wood: crate::meshgen::CutTag, leaves: crate::meshgen::CutTag) -> bool {
        let key = if k == 0 { kind.to_string() } else { format!("{kind}.{}", k + 1) };
        let Some(m) = self.0.get(&key) else { return false };
        let rot = Quat::from_rotation_y(yaw);
        let s = height / m.height.max(0.01);
        for pass in [false, true] {
            mb.set_tag(if pass { leaves } else { wood });
            for (p, n, c, leaf) in &m.tris {
                if *leaf == pass {
                    mb.triangle_with_normals(p.map(|p| origin + rot * p * s), n.map(|n| rot * n), *c);
                }
            }
        }
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
        let Some(m) = self.pick(kind, (centre.x as i32 as u32).wrapping_mul(73_856_093) ^ (centre.y as i32 as u32).wrapping_mul(19_349_663)) else { return false };
        let turn = match facing {
            Side::S => 0,
            Side::E => 1,
            Side::N => 2,
            Side::W => 3,
        };
        m.emit(mb, Vec3::new(centre.x, base_y, centre.y), turn, Vec3::ONE);
        true
    }
}

impl FurnitureModel {
    /// Every triangle, scaled, turned `turn` quarters and moved to `origin`.
    fn emit(&self, mb: &mut MeshBuilder, origin: Vec3, turn: i32, scale: Vec3) {
        // Positive turns about +y take the model's front (+z) toward +x (east).
        let rot = Quat::from_rotation_y(turn as f32 * std::f32::consts::FRAC_PI_2);
        for (p, n, c, _) in &self.tris {
            let p = p.map(|p| origin + rot * p * scale);
            let n = n.map(|n| ((rot * n) / scale).normalize_or_zero());
            mb.triangle_with_normals(p, n, *c);
        }
    }
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
    Ok(FurnitureModel { tris, footprint, height })
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
