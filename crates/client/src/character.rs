//! Modular characters (#62, #63): one base body on one skeleton, and wearables that fit anyone on it.
//! Read from the GLBs `tools/blender/zgarment.py` exports (see its docstring for the format):
//!
//! - **the body** (`<id>_body.glb`, promoted as `assets/models/characters/<id>.glb`): the skeleton, the
//!   skinned body with a `_REGION` vertex attribute (the index of its [`BodyPart`], so what a worn item
//!   covers can be hidden), the clips (idle, walk, run, sneak, ...) and, in the body node's extras, the
//!   `sockets`: named places on a bone (`eyes`, `head_top`, `neck`, `hand_r`, `hand_l`, `back`) as an
//!   offset from the bone's head on Blender's axes in the rest pose;
//! - **a wearable** (`wearables/<model>.glb`): one mesh skinned to the same skeleton, bound to it by
//!   bone name (a rigid one, glasses or a hat, wholly to its socket's bone), with what it `hides` in
//!   its node's extras. Flat palette colours (one per primitive) become vertex colours.
//!
//! Game axes: metres, Y up, facing +Z, the character's left +X, feet at y = 0 (glTF's own, as Blender
//! exports a character facing -Y). [`crate::survivor_model`] draws and poses them.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use zombye_sim::BodyPart;

/// The body regions in `_REGION`'s order (`zgarment.REGIONS`, the order of [`BodyPart`]).
pub const REGIONS: [BodyPart; 6] = [BodyPart::Head, BodyPart::Torso, BodyPart::Arms, BodyPart::Hands, BodyPart::Legs, BodyPart::Feet];

/// Sanity limits, so a broken file is refused rather than eating memory.
const MAX_BONES: usize = 128;
const MAX_VERTICES: usize = 1 << 18;
/// Clips are resampled at this rate.
const CLIP_FPS: f32 = 30.0;

/// One bone: its name, parent and rest transform (local to the parent, or to the model for a root).
#[derive(Clone, Debug)]
pub struct RigBone {
    pub name: String,
    pub parent: Option<usize>,
    pub t: Vec3,
    pub r: Quat,
    pub s: Vec3,
}

/// Every bone's local translation and rotation (scale stays the rest's).
#[derive(Clone, Debug, PartialEq)]
pub struct RigPose {
    pub t: Vec<Vec3>,
    pub r: Vec<Quat>,
}

impl RigPose {
    /// Toward `other` by `k` (0 = self, 1 = other), bone by bone.
    pub fn blend(&self, other: &Self, k: f32) -> Self {
        if k <= 0.0 {
            return self.clone();
        }
        if k >= 1.0 {
            return other.clone();
        }
        let mut out = self.clone();
        out.blend_bones(other, k, None);
        out
    }

    /// Bones in `mask` (all of them for `None`) toward `other` by `k`.
    pub fn blend_bones(&mut self, other: &Self, k: f32, mask: Option<&[usize]>) {
        if k <= 0.0 {
            return;
        }
        let k = k.min(1.0);
        let n = self.t.len();
        let mut one = |b: usize| {
            self.t[b] = self.t[b].lerp(other.t[b], k);
            self.r[b] = self.r[b].slerp(other.r[b], k);
        };
        match mask {
            Some(m) => m.iter().copied().for_each(&mut one),
            None => (0..n).for_each(one),
        }
    }
}

/// A skeleton: bones (parents before children) and the inverse bind matrices of the skin.
#[derive(Clone, Debug)]
pub struct Rig {
    pub bones: Vec<RigBone>,
    pub inverse_bind: Vec<Mat4>,
}

impl Rig {
    pub fn len(&self) -> usize {
        self.bones.len()
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }

    pub fn rest(&self) -> RigPose {
        RigPose { t: self.bones.iter().map(|b| b.t).collect(), r: self.bones.iter().map(|b| b.r).collect() }
    }

    /// Model-space transform of every bone in `pose`.
    pub fn globals(&self, pose: &RigPose) -> Vec<Mat4> {
        let mut g: Vec<Mat4> = Vec::with_capacity(self.len());
        for (b, bone) in self.bones.iter().enumerate() {
            let local = Mat4::from_scale_rotation_translation(bone.s, pose.r[b], pose.t[b]);
            g.push(match bone.parent {
                Some(p) => g[p] * local,
                None => local,
            });
        }
        g
    }

    /// Skinning matrix per bone: posed model space from rest model space.
    pub fn skin_matrices(&self, pose: &RigPose) -> Vec<Mat4> {
        let mut g = self.globals(pose);
        for (m, inv) in g.iter_mut().zip(&self.inverse_bind) {
            *m *= *inv;
        }
        g
    }
}

/// A clip resampled to a fixed rate: one pose per frame, the last the same time as the first for a loop.
#[derive(Clone, Debug)]
pub struct Clip {
    pub name: String,
    pub secs: f32,
    pub frames: Vec<RigPose>,
}

impl Clip {
    /// The pose at `u` of the way through (wrapping), between the two nearest frames.
    pub fn sample(&self, u: f32) -> RigPose {
        let n = self.frames.len();
        if n == 1 {
            return self.frames[0].clone();
        }
        let x = u.rem_euclid(1.0) * (n - 1) as f32;
        let i = (x.floor() as usize).min(n - 2);
        self.frames[i].blend(&self.frames[i + 1], x - i as f32)
    }
}

/// A skinned mesh: joints are bone indices of the [`Rig`] it is drawn with.
#[derive(Clone, Debug, Default)]
pub struct SkinMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<[f32; 2]>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl SkinMesh {
    pub fn vertices(&self) -> usize {
        self.positions.len()
    }

    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// Adds `other`'s triangles.
    pub fn append(&mut self, other: &SkinMesh) {
        self.append_where(other, |_| true);
    }

    /// Adds `other`'s triangles that `keep` (given their vertex indices in `other`).
    pub fn append_where(&mut self, other: &SkinMesh, keep: impl Fn([u32; 3]) -> bool) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&other.positions);
        self.normals.extend_from_slice(&other.normals);
        self.uvs.extend_from_slice(&other.uvs);
        self.joints.extend_from_slice(&other.joints);
        self.weights.extend_from_slice(&other.weights);
        self.colors.extend_from_slice(&other.colors);
        for t in other.indices.as_chunks::<3>().0 {
            if keep(*t) {
                self.indices.extend(t.iter().map(|i| i + base));
            }
        }
    }

    /// A box from `min` to `max` in a frame `frame` (rest-pose model space), all on bone `bone`.
    pub fn add_box(&mut self, frame: Mat4, min: Vec3, max: Vec3, bone: u16, color: [f32; 4]) {
        for (quad, n) in crate::rig::box_faces(min, max) {
            let base = self.positions.len() as u32;
            for p in quad {
                self.positions.push(frame.transform_point3(p));
                self.normals.push(frame.transform_vector3(n).normalize_or_zero());
                self.uvs.push([0.0, 0.0]);
                self.joints.push([bone, 0, 0, 0]);
                self.weights.push([1.0, 0.0, 0.0, 0.0]);
                self.colors.push(color);
            }
            self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }

    pub fn to_mesh(&self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions.iter().map(|p| p.to_array()).collect::<Vec<_>>())
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals.iter().map(|n| n.to_array()).collect::<Vec<_>>())
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(self.joints.clone()))
            .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, self.weights.clone())
            .with_inserted_indices(Indices::U32(self.indices.clone()))
    }
}

/// A named place on the body: on `bone`, `offset` from its head and turned by `rotation`, on the
/// model's axes in the rest pose.
#[derive(Clone, Copy, Debug)]
pub struct Socket {
    pub bone: usize,
    pub offset: Vec3,
    pub rotation: Quat,
}

/// The base body.
#[derive(Clone, Debug)]
pub struct Body {
    pub rig: Rig,
    pub mesh: SkinMesh,
    /// Per vertex, its index in [`REGIONS`].
    pub regions: Vec<u8>,
    /// The base-colour image and its extension (`png` / `jpg`).
    pub texture: Option<(Vec<u8>, &'static str)>,
    pub clips: Vec<Clip>,
    pub sockets: HashMap<String, Socket>,
}

impl Body {
    #[cfg(test)]
    pub fn clip(&self, name: &str) -> Option<&Clip> {
        self.clips.iter().find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// The rest-pose model-space frame of socket `name`: its bone's head plus the offset, turned by
    /// its rotation (not the bone's: sockets are on the model's axes).
    pub fn socket_frame(&self, name: &str) -> Option<(usize, Mat4)> {
        let s = self.sockets.get(name)?;
        let head = self.rig.inverse_bind[s.bone].inverse().transform_point3(Vec3::ZERO);
        Some((s.bone, Mat4::from_rotation_translation(s.rotation, head + s.offset)))
    }
}

/// One wearable, ready to draw on the body it was read against.
#[derive(Clone, Debug)]
pub struct Wearable {
    pub mesh: SkinMesh,
    /// Body regions hidden while it's worn.
    pub hides: Vec<BodyPart>,
    /// Per vertex, the body region it lies over (its `_REGION`, an index in [`REGIONS`]), if it has one:
    /// a part of it another garment worn over it can cut away (the jeans' top under a shirt).
    pub regions: Option<Vec<u8>>,
    /// Its hem's height in the rest pose (`hem_z`): another garment's faces in the regions this one
    /// hides are cut away above it.
    pub hem_z: Option<f32>,
}

type Doc = gltf::Gltf;

fn blob<'a>(doc: &'a Doc) -> impl Fn(gltf::Buffer) -> Option<&'a [u8]> + Clone + 'a {
    move |b: gltf::Buffer| match b.source() {
        gltf::buffer::Source::Bin => doc.blob.as_deref(),
        gltf::buffer::Source::Uri(_) => None,
    }
}

/// Blender's axes (+Z up, -Y forward) to the game's (+Y up, +Z forward): (x, y, z) → (x, z, -y).
fn from_blender(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.z, -v.y)
}

/// Every node's model-space matrix (from the scene's roots).
fn node_worlds(doc: &Doc) -> Vec<Mat4> {
    let mut world = vec![Mat4::IDENTITY; doc.nodes().count()];
    fn walk(n: gltf::Node, parent: Mat4, world: &mut [Mat4]) {
        let m = parent * Mat4::from_cols_array_2d(&n.transform().matrix());
        world[n.index()] = m;
        for c in n.children() {
            walk(c, m, world);
        }
    }
    for scene in doc.scenes() {
        for n in scene.nodes() {
            walk(n, Mat4::IDENTITY, &mut world);
        }
    }
    world
}

/// The skeleton of `skin`: its joints in order, each one's parent among them; a root bone's rest
/// includes whatever its parent nodes do.
fn read_rig(doc: &Doc, skin: &gltf::Skin) -> Result<Rig, String> {
    let joints: Vec<gltf::Node> = skin.joints().collect();
    if joints.is_empty() || joints.len() > MAX_BONES {
        return Err(format!("a skeleton of {} bones", joints.len()));
    }
    let mut parent_node = vec![None; doc.nodes().count()];
    for n in doc.nodes() {
        for c in n.children() {
            parent_node[c.index()] = Some(n.index());
        }
    }
    let worlds = node_worlds(doc);
    let index_of = |node: usize| joints.iter().position(|j| j.index() == node);
    let mut bones: Vec<RigBone> = joints
        .iter()
        .map(|j| {
            let (t, r, s) = j.transform().decomposed();
            let parent = parent_node[j.index()].and_then(index_of);
            let (mut t, mut r, mut s) = (Vec3::from(t), Quat::from_array(r), Vec3::from(s));
            if parent.is_none()
                && let Some(p) = parent_node[j.index()]
            {
                (s, r, t) = (worlds[p] * Mat4::from_scale_rotation_translation(s, r, t)).to_scale_rotation_translation();
            }
            RigBone { name: j.name().unwrap_or_default().to_string(), parent, t, r, s }
        })
        .collect();
    let inverse_bind: Vec<Mat4> = match skin.reader(blob(doc)).read_inverse_bind_matrices() {
        Some(m) => m.map(|c| Mat4::from_cols_array_2d(&c)).collect(),
        None => vec![Mat4::IDENTITY; bones.len()],
    };
    if inverse_bind.len() != bones.len() {
        return Err("inverse bind matrices don't match the joints".into());
    }
    // Parents before children, so globals can be worked out in one pass.
    let mut order: Vec<usize> = Vec::new();
    while order.len() < bones.len() {
        let before = order.len();
        for (b, bone) in bones.iter().enumerate() {
            if !order.contains(&b) && bone.parent.is_none_or(|p| order.contains(&p)) {
                order.push(b);
            }
        }
        if order.len() == before {
            return Err("the skeleton has a loop".into());
        }
    }
    let new_index: Vec<usize> = (0..bones.len()).map(|b| order.iter().position(|&o| o == b).unwrap()).collect();
    for b in &mut bones {
        b.parent = b.parent.map(|p| new_index[p]);
    }
    let bones = order.iter().map(|&o| bones[o].clone()).collect();
    let inverse_bind = order.iter().map(|&o| inverse_bind[o]).collect();
    Ok(Rig { bones, inverse_bind })
}

/// The first skinned mesh node.
fn skinned_node(doc: &Doc) -> Result<gltf::Node<'_>, String> {
    doc.nodes().find(|n| n.mesh().is_some() && n.skin().is_some()).ok_or_else(|| "no skinned mesh".to_string())
}

/// A skinned mesh node's primitives as one mesh, joints mapped through `joint_map` (the node's skin
/// joint index → a bone of the rig it'll be drawn with), and the `_REGION` attribute if it has one.
fn read_skinned(doc: &Doc, node: &gltf::Node, joint_map: &[u16]) -> Result<(SkinMesh, Option<Vec<u8>>), String> {
    let mesh = node.mesh().expect("a mesh node");
    let mut out = SkinMesh::default();
    let mut regions: Option<Vec<u8>> = None;
    for prim in mesh.primitives() {
        let reader = prim.reader(blob(doc));
        let pos: Vec<Vec3> = reader.read_positions().ok_or("a primitive without positions")?.map(Vec3::from).collect();
        let n = pos.len();
        if out.vertices() + n > MAX_VERTICES {
            return Err("too many vertices".into());
        }
        let nrm: Vec<Vec3> = reader.read_normals().ok_or("a primitive without normals")?.map(Vec3::from).collect();
        let uv: Vec<[f32; 2]> = reader.read_tex_coords(0).map_or_else(|| vec![[0.0; 2]; n], |t| t.into_f32().collect());
        let joints: Vec<[u16; 4]> = reader.read_joints(0).ok_or("a primitive without joints")?.into_u16().collect();
        let weights: Vec<[f32; 4]> = reader.read_weights(0).ok_or("a primitive without weights")?.into_f32().collect();
        if [nrm.len(), uv.len(), joints.len(), weights.len()].iter().any(|&l| l != n) {
            return Err("vertex attributes of different lengths".into());
        }
        let material = prim.material();
        let pbr = material.pbr_metallic_roughness();
        let c = pbr.base_color_factor();
        let base = out.vertices() as u32;
        for i in 0..n {
            let mut j = [0u16; 4];
            for k in 0..4 {
                j[k] = *joint_map.get(joints[i][k] as usize).ok_or("a joint index out of range")?;
            }
            let w = weights[i];
            let sum: f32 = w.iter().sum();
            out.positions.push(pos[i]);
            out.normals.push(nrm[i].normalize_or_zero());
            out.uvs.push(uv[i]);
            out.joints.push(j);
            out.weights.push(if sum > 1e-6 { w.map(|x| x / sum) } else { [1.0, 0.0, 0.0, 0.0] });
            out.colors.push([c[0], c[1], c[2], 1.0]);
        }
        match reader.read_indices() {
            Some(idx) => {
                for i in idx.into_u32() {
                    if i as usize >= n {
                        return Err("an index out of range".into());
                    }
                    out.indices.push(base + i);
                }
            }
            None => out.indices.extend(base..base + n as u32),
        }
        // `_REGION`: a float (or small integer) per vertex.
        let attr = prim.attributes().find(|(s, _)| matches!(s, gltf::Semantic::Extras(name) if name.eq_ignore_ascii_case("region")));
        if let Some((_, acc)) = attr {
            let vals: Vec<u8> = match acc.data_type() {
                gltf::accessor::DataType::F32 => gltf::accessor::Iter::<f32>::new(acc, blob(doc)).ok_or("unreadable _REGION")?.map(|r| r.round().clamp(0.0, 255.0) as u8).collect(),
                gltf::accessor::DataType::U8 => gltf::accessor::Iter::<u8>::new(acc, blob(doc)).ok_or("unreadable _REGION")?.collect(),
                gltf::accessor::DataType::U16 => gltf::accessor::Iter::<u16>::new(acc, blob(doc)).ok_or("unreadable _REGION")?.map(|r| r.min(255) as u8).collect(),
                other => return Err(format!("_REGION of type {other:?}")),
            };
            if vals.len() != n {
                return Err("_REGION has the wrong length".into());
            }
            regions.get_or_insert_with(|| vec![u8::MAX; base as usize]).extend(vals);
        } else if let Some(r) = &mut regions {
            r.extend(std::iter::repeat_n(u8::MAX, n));
        }
    }
    if out.triangles() == 0 {
        return Err("no triangles".into());
    }
    if out.positions.iter().chain(&out.normals).any(|v| !v.is_finite()) {
        return Err("non-finite vertex data".into());
    }
    Ok((out, regions))
}

/// The base-colour image of the first textured material, and its extension.
fn read_texture(doc: &Doc) -> Option<(Vec<u8>, &'static str)> {
    let image = doc.materials().find_map(|m| m.pbr_metallic_roughness().base_color_texture()).map(|t| t.texture().source())?;
    match image.source() {
        gltf::image::Source::View { view, mime_type } => {
            let data = doc.blob.as_deref()?.get(view.offset()..view.offset() + view.length())?;
            Some((data.to_vec(), if mime_type == "image/png" { "png" } else { "jpg" }))
        }
        gltf::image::Source::Uri { .. } => None,
    }
}

/// Every animation, resampled at [`CLIP_FPS`]; bones it doesn't key keep their rest.
fn read_clips(doc: &Doc, rig: &Rig, joint_nodes: &[usize]) -> Result<Vec<Clip>, String> {
    let mut clips = Vec::new();
    for anim in doc.animations() {
        let name = anim.name().unwrap_or("clip").to_string();
        // Per bone: (times, translations) and (times, rotations).
        let mut tracks_t: Vec<Option<(Vec<f32>, Vec<Vec3>)>> = vec![None; rig.len()];
        let mut tracks_r: Vec<Option<(Vec<f32>, Vec<Quat>)>> = vec![None; rig.len()];
        let mut end = 0.0f32;
        for ch in anim.channels() {
            let Some(b) = joint_nodes.iter().position(|&n| n == ch.target().node().index()) else { continue };
            let reader = ch.reader(blob(doc));
            let times: Vec<f32> = reader.read_inputs().ok_or("a channel without times")?.collect();
            if times.iter().any(|t| !t.is_finite()) {
                return Err(format!("{name}: bad key times"));
            }
            end = end.max(times.last().copied().unwrap_or(0.0));
            match reader.read_outputs().ok_or("a channel without values")? {
                gltf::animation::util::ReadOutputs::Translations(v) => tracks_t[b] = Some((times, v.map(Vec3::from).collect())),
                gltf::animation::util::ReadOutputs::Rotations(v) => tracks_r[b] = Some((times, v.into_f32().map(|q| Quat::from_array(q).normalize()).collect())),
                _ => {}
            }
        }
        let frames = ((end * CLIP_FPS).round() as usize).max(1) + 1;
        let rest = rig.rest();
        let poses = (0..frames)
            .map(|f| {
                let time = end * f as f32 / (frames - 1).max(1) as f32;
                let mut p = rest.clone();
                for b in 0..rig.len() {
                    if let Some((ts, vs)) = &tracks_t[b] {
                        p.t[b] = key(ts, time, |i, j, k| vs[i].lerp(vs[j], k));
                    }
                    if let Some((ts, vs)) = &tracks_r[b] {
                        p.r[b] = key(ts, time, |i, j, k| vs[i].slerp(vs[j], k));
                    }
                }
                p
            })
            .collect();
        clips.push(Clip { name, secs: end.max(1.0 / CLIP_FPS), frames: poses });
    }
    Ok(clips)
}

/// A linearly keyed value at `time`.
fn key<T>(times: &[f32], time: f32, lerp: impl Fn(usize, usize, f32) -> T) -> T {
    let j = times.partition_point(|&t| t <= time);
    if j == 0 {
        return lerp(0, 0, 0.0);
    }
    if j >= times.len() {
        let last = times.len() - 1;
        return lerp(last, last, 0.0);
    }
    let i = j - 1;
    let span = (times[j] - times[i]).max(1e-6);
    lerp(i, j, ((time - times[i]) / span).clamp(0.0, 1.0))
}

#[derive(serde::Deserialize, Default)]
struct BodyExtras {
    sockets: Option<String>,
}

#[derive(serde::Deserialize)]
struct SocketSpec {
    bone: String,
    offset: [f32; 3],
    #[serde(default)]
    rotation: [f32; 3],
}

#[derive(serde::Deserialize, Default)]
struct WearableExtras {
    hides: Option<String>,
    hem_z: Option<f32>,
}

/// Read a body GLB.
pub fn parse_body(bytes: &[u8]) -> Result<Body, String> {
    let doc = gltf::Gltf::from_slice(bytes).map_err(|e| e.to_string())?;
    let node = skinned_node(&doc)?;
    let skin = node.skin().expect("skinned");
    let rig = read_rig(&doc, &skin)?;
    // The skin's joint order → the rig's (sorted parents first).
    let joint_nodes: Vec<usize> = skin.joints().map(|j| j.index()).collect();
    let by_name: Vec<u16> = skin.joints().map(|j| rig.find(j.name().unwrap_or_default()).unwrap_or(0) as u16).collect();
    let (mesh, regions) = read_skinned(&doc, &node, &by_name)?;
    let regions = regions.unwrap_or_else(|| vec![u8::MAX; mesh.vertices()]);
    let rig_nodes: Vec<usize> = rig.bones.iter().map(|b| joint_nodes[skin.joints().position(|j| j.name().unwrap_or_default() == b.name).unwrap()]).collect();
    let clips = read_clips(&doc, &rig, &rig_nodes)?;
    let extras: BodyExtras = node.extras().as_ref().and_then(|e| serde_json::from_str(e.get()).ok()).unwrap_or_default();
    let mut sockets = HashMap::new();
    if let Some(text) = extras.sockets {
        let specs: HashMap<String, SocketSpec> = serde_json::from_str(&text).map_err(|e| format!("sockets: {e}"))?;
        for (name, s) in specs {
            let Some(bone) = rig.find(&s.bone) else { return Err(format!("socket {name} on unknown bone {}", s.bone)) };
            let [x, y, z] = s.rotation.map(f32::to_radians);
            // Blender's XYZ Euler on Blender's axes, turned onto the game's.
            let blender = Quat::from_rotation_z(z) * Quat::from_rotation_y(y) * Quat::from_rotation_x(x);
            let axes = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
            sockets.insert(name, Socket { bone, offset: from_blender(Vec3::from(s.offset)), rotation: axes * blender * axes.inverse() });
        }
    }
    Ok(Body { texture: read_texture(&doc), rig, mesh, regions, clips, sockets })
}

/// Read a wearable GLB against `rig` (its joints are matched to the rig's bones by name).
pub fn parse_wearable(bytes: &[u8], rig: &Rig) -> Result<Wearable, String> {
    let doc = gltf::Gltf::from_slice(bytes).map_err(|e| e.to_string())?;
    let node = skinned_node(&doc)?;
    let skin = node.skin().expect("skinned");
    let mut map = Vec::new();
    for j in skin.joints() {
        let name = j.name().unwrap_or_default();
        map.push(rig.find(name).ok_or_else(|| format!("bone {name:?} isn't in the body's skeleton"))? as u16);
    }
    let (mesh, regions) = read_skinned(&doc, &node, &map)?;
    let extras: WearableExtras = node.extras().as_ref().and_then(|e| serde_json::from_str(e.get()).ok()).unwrap_or_default();
    let hides = match extras.hides {
        Some(text) => {
            let names: Vec<String> = serde_json::from_str(&text).map_err(|e| format!("hides: {e}"))?;
            names.iter().filter_map(|n| REGIONS.iter().copied().find(|r| format!("{r:?}") == *n)).collect()
        }
        None => Vec::new(),
    };
    Ok(Wearable { mesh, hides, regions, hem_z: extras.hem_z.filter(|h| h.is_finite()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_interpolate_and_clamp() {
        let ts = [0.0, 1.0, 3.0];
        let vs = [0.0f32, 10.0, 30.0];
        let at = |t: f32| key(&ts, t, |i, j, k| vs[i] + (vs[j] - vs[i]) * k);
        assert_eq!(at(-1.0), 0.0);
        assert_eq!(at(0.5), 5.0);
        assert_eq!(at(2.0), 20.0);
        assert_eq!(at(9.0), 30.0);
    }

    #[test]
    fn blender_axes_turn_onto_the_game_s() {
        // Forward in Blender (-Y) is +Z here; up (+Z) is +Y; the left (+X) stays.
        assert_eq!(from_blender(Vec3::NEG_Y), Vec3::Z);
        assert_eq!(from_blender(Vec3::Z), Vec3::Y);
        assert_eq!(from_blender(Vec3::X), Vec3::X);
    }
}
