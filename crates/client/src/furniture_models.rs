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

use std::collections::HashMap;
use std::path::Path;

use bevy::prelude::*;
use zombye_sim::map::Side;

use crate::meshgen::MeshBuilder;

/// One model: triangles (position, normal) with a colour each, in the model's own frame.
pub struct FurnitureModel {
    tris: Vec<([Vec3; 3], [Vec3; 3], LinearRgba)>,
    /// Footprint in tiles (x, z) when facing south.
    footprint: Vec2,
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

    /// Adds the model for `kind` filling the tile rectangle (`min`, `size` in metres) and turned to
    /// `facing`. False when there's no model for it.
    pub fn add(&self, mb: &mut MeshBuilder, kind: &str, min: Vec2, size: Vec2, facing: Side, base_y: f32) -> bool {
        let Some(m) = self.0.get(kind) else { return false };
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
        // Positive turns about +y take the model's front (+z) toward +x (east).
        let rot = Quat::from_rotation_y(turn as f32 * std::f32::consts::FRAC_PI_2);
        let fp = turned(turn);
        let scale = Vec3::new(size.x / fp.x, 1.0, size.y / fp.y);
        let origin = Vec3::new(min.x + size.x * 0.5, base_y, min.y + size.y * 0.5);
        for (p, n, c) in &m.tris {
            let p = p.map(|p| origin + rot * p * scale);
            let n = n.map(|n| ((rot * n) / scale).normalize_or_zero());
            mb.triangle_with_normals(p, n, *c);
        }
        true
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
            let idx: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..pos.len() as u32).collect(),
            };
            for t in idx.chunks_exact(3) {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| i as usize);
                if a.max(b).max(c) >= pos.len() {
                    return Err("index out of range".into());
                }
                tris.push(([pos[a], pos[b], pos[c]], [nrm[a], nrm[b], nrm[c]], color));
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
    Ok(FurnitureModel { tris, footprint })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> FurnitureModels {
        FurnitureModels::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models/furniture"))
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
