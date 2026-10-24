//! Survivors drawn as the modular character (#62, #63): the base body ([`crate::character`]) on its
//! own skeleton, with what each one wears drawn on the same skeleton, posed on the CPU every frame
//! (only a few survivors are ever on screen) and skinned on the GPU.
//!
//! **Animation.** Locomotion is the body's own clips (idle, walk, run, sneak), sampled on the
//! locomotion cycle [`crate::survivor`] keeps (it advances with the distance actually moved; each
//! clip's stride is measured from its planted feet, as `zvat.py` does) and blended by stance and
//! speed. The rest is the grey-box's poses, carried over onto this skeleton ([`Retarget`]): the arm
//! holding a weapon at the ready, swings, shoves and stomps (timed to the server as before), the
//! hurt flinch and the fall onto the back when dead or asleep. Each grey-box bone's turn from its
//! rest is put on the matching bones in a reference pose (the idle's first frame, arms down like the
//! grey-box's rest); the spine shares the torso's turn, the neck half the head's.
//!
//! **What's drawn** follows [`Snapshot::outfits`](zombye_net::Snapshot::outfits), for us and for
//! everyone else, changing the moment it does: the body with the regions worn wearables hide cut
//! away (one mesh per outfit, cached), and every worn item's model merged into one more mesh (also
//! cached), so a survivor is three draw calls (body, outfit, held weapon) plus its marker ring.
//! An item whose model isn't there is a grey-box in its colour: clothing tints the regions it
//! covers, an accessory or a bag is a box on its socket (`eyes`, `head_top`, `neck`, `back`).
//!
//! **Telling players apart:** a flat ring on the ground round each survivor, ours mustard, others
//! teal (the grey-box's jacket colours). Unlit, but dimmed with the daylight to [`RING_NIGHT`] of
//! its colour at night (light comes only from sources: a ring mustn't glow in the dark).
//!
//! **Files:** the body is `--survivor PATH` (a body GLB, or a folder holding a `*_body.glb`, such as
//! a character's `export/` folder in the art repo), else `ZOMBYE_SURVIVOR` (the MCP `screenshot`
//! client inherits it, so a work-in-progress character can be reviewed in game), else
//! `<data>/../models/characters/survivor_01.glb`; `none`, or no file, keeps the grey-box. Wearables
//! are every GLB in `<data>/../models/wearables/`, then those in `wearables/` beside the body,
//! replacing any of the same name; an item names its file with `model` in `items.ron`.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::NotShadowCaster;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use zombye_net::OutfitView;
use zombye_sim::{BodyPart, ItemDef, WearSlot};

use crate::character::{Body, REGIONS, RigPose, SkinMesh, Wearable, parse_body, parse_wearable};
use crate::rig::{globals, smooth};
use crate::server_link::{JoinInfo, ServerLink};
use crate::survivor::{
    self, FARM_L, FARM_R, HAND_R, HEAD, HIP, HeldWeapon, JACKET_ME, JACKET_OTHER, PELVIS, SHIN_L, SHIN_R, SPose, Style, SurvivorAnim, THIGH_L,
    THIGH_R, TORSO, UARM_L, UARM_R, UPPER,
};

/// How far above a garment's hem (`hem_z`) the garments under it are cut away.
const CUT_ABOVE_HEM: f32 = 0.03;

/// How bright the marker rings are at night, of their daytime colour.
const RING_NIGHT: f32 = 0.15;

/// The body file, when `--survivor` / `ZOMBYE_SURVIVOR` don't name one.
const DEFAULT_BODY: &str = "survivor_01";

/// Where a skeleton bone takes its turn from in a grey-box pose.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Src {
    /// That grey-box bone's.
    Grey(usize),
    /// Between two grey-box bones' (0 = the first).
    Mix(usize, usize, f32),
    /// None: it keeps its reference pose under its parent (fingers, toes, the head's end).
    Follow,
}

/// Grey-box poses onto the skeleton.
struct Retarget {
    src: Vec<Src>,
    /// Locals of the reference pose, and each bone's model-space rotation in it.
    reference: RigPose,
    ref_global: Vec<Quat>,
    /// Skeleton hip height over the grey-box's.
    hip_scale: f32,
}

impl Retarget {
    fn new(body: &Body, reference: RigPose) -> Self {
        let rig = &body.rig;
        let depth = |mut b: usize| {
            let mut d = 0;
            while let Some(p) = rig.bones[b].parent {
                (b, d) = (p, d + 1);
            }
            d
        };
        let mut spine: Vec<usize> = (0..rig.len()).filter(|&b| rig.bones[b].name.to_lowercase().starts_with("spine")).collect();
        spine.sort_by_key(|&b| depth(b));
        let src = (0..rig.len())
            .map(|b| {
                if let Some(k) = spine.iter().position(|&s| s == b) {
                    return Src::Mix(PELVIS, TORSO, (k + 1) as f32 / spine.len() as f32);
                }
                match rig.bones[b].name.to_lowercase().as_str() {
                    "hips" | "pelvis" => Src::Grey(PELVIS),
                    "leftshoulder" | "rightshoulder" => Src::Grey(TORSO),
                    "neck" => Src::Mix(TORSO, HEAD, 0.5),
                    "head" => Src::Grey(HEAD),
                    "leftarm" => Src::Grey(UARM_L),
                    "leftforearm" | "lefthand" => Src::Grey(FARM_L),
                    "rightarm" => Src::Grey(UARM_R),
                    "rightforearm" => Src::Grey(FARM_R),
                    "righthand" => Src::Grey(HAND_R),
                    "leftupleg" => Src::Grey(THIGH_L),
                    "leftleg" | "leftfoot" => Src::Grey(SHIN_L),
                    "rightupleg" => Src::Grey(THIGH_R),
                    "rightleg" | "rightfoot" => Src::Grey(SHIN_R),
                    _ => Src::Follow,
                }
            })
            .collect();
        let g = rig.globals(&reference);
        let ref_global = g.iter().map(|m| m.to_scale_rotation_translation().1).collect();
        let root_y = (0..rig.len()).find(|&b| rig.bones[b].parent.is_none()).map_or(HIP, |b| g[b].w_axis.y);
        Self { src, reference, ref_global, hip_scale: (root_y / HIP).clamp(0.5, 2.0) }
    }

    /// The grey-box pose `grey` on the skeleton.
    fn apply(&self, body: &Body, grey: &SPose) -> RigPose {
        let gg = globals(&survivor::BONES, grey).map(|m| m.to_scale_rotation_translation().1);
        let rig = &body.rig;
        let mut pose = self.reference.clone();
        let mut global = vec![Quat::IDENTITY; rig.len()];
        for b in 0..rig.len() {
            let parent = rig.bones[b].parent.map_or(Quat::IDENTITY, |p| global[p]);
            global[b] = match self.src[b] {
                Src::Grey(i) => gg[i] * self.ref_global[b],
                Src::Mix(i, j, f) => gg[i].slerp(gg[j], f) * self.ref_global[b],
                Src::Follow => parent * self.reference.r[b],
            };
            pose.r[b] = (parent.inverse() * global[b]).normalize();
            if rig.bones[b].parent.is_none() {
                pose.t[b] = self.reference.t[b] + grey.offset * self.hip_scale;
            }
        }
        pose
    }

    /// The skeleton bones a grey-box bone in `of` drives directly (not through a mix).
    fn bones_of(&self, of: &[usize]) -> Vec<usize> {
        (0..self.src.len()).filter(|&b| matches!(self.src[b], Src::Grey(g) if of.contains(&g))).collect()
    }
}

/// Everything shared by the survivors drawn as the modular character.
#[derive(Resource)]
pub struct SurvivorModel {
    body: Body,
    /// Wearables by model name.
    wardrobe: HashMap<String, Wearable>,
    retarget: Retarget,
    /// The locomotion clips (indices into the body's clips): idle, walk, run, sneak; each loop's
    /// phase where the left foot is furthest forward, and its stride (metres per cycle).
    idle: Option<usize>,
    loops: [Option<usize>; 3],
    phase: [f32; 3],
    stride: [f32; 3],
    /// Bones an action takes over (the upper body: everything from the spine up), the arm that holds
    /// a weapon, the bones the flinch turns, all of them.
    upper: Vec<usize>,
    right_arm: Vec<usize>,
    flinching: Vec<(usize, usize)>,
    all: Vec<usize>,
    /// Body vertices on the feet (for standing them on the ground).
    feet: Vec<usize>,
    /// A light texel of the body's texture, for tinting (see [`light_uv`]).
    light_uv: Option<[f32; 2]>,
    hand_r: usize,
    weapon_frame: Transform,
    bare: Handle<Mesh>,
    body_material: Handle<StandardMaterial>,
    outfit_material: Handle<StandardMaterial>,
    bindposes: Handle<SkinnedMeshInverseBindposes>,
    ring: Handle<Mesh>,
    ring_me: Handle<StandardMaterial>,
    ring_other: Handle<StandardMaterial>,
}

/// The body file to use (see the module docs), or `None` for the grey-box.
fn body_path(options: &crate::Options) -> Option<PathBuf> {
    let asked = options.survivor.clone().or_else(|| std::env::var("ZOMBYE_SURVIVOR").ok());
    let path = match asked {
        Some(p) if p.trim().is_empty() || p.trim().eq_ignore_ascii_case("none") => return None,
        Some(p) => PathBuf::from(p.trim()),
        None => options.data.join(format!("../models/characters/{DEFAULT_BODY}.glb")),
    };
    if path.is_dir() {
        let mut found: Vec<PathBuf> = std::fs::read_dir(&path).ok()?.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with("_body.glb")).collect();
        found.sort();
        return found.into_iter().next();
    }
    path.is_file().then_some(path)
}

/// Every wearable GLB in `dir`, by file name, read against `body`.
fn load_wearables(dir: &Path, body: &Body, into: &mut HashMap<String, Wearable>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "glb")).collect();
    paths.sort();
    for path in paths {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| parse_wearable(&b, &body.rig)) {
            Ok(w) => {
                info!("wearable {name}: {} triangles, hides {:?} ({})", w.mesh.triangles(), w.hides, path.display());
                into.insert(name, w);
            }
            Err(e) => warn!("wearable {}: {e}", path.display()),
        }
    }
}

/// A looping clip's phase (where the left foot is furthest forward) and stride: while a foot is
/// planted (in the lowest fifth of its height) it slides back at the walking speed.
fn phase_and_stride(body: &Body, clip: usize) -> (f32, f32) {
    let rig = &body.rig;
    let c = &body.clips[clip];
    let (Some(l), Some(r)) = (rig.find("LeftFoot"), rig.find("RightFoot")) else { return (0.0, 0.0) };
    let feet: Vec<[Vec3; 2]> = c.frames.iter().map(|p| rig.globals(p)).map(|g| [g[l].w_axis.truncate(), g[r].w_axis.truncate()]).collect();
    let n = feet.len();
    let best = (0..n).max_by(|&a, &b| (feet[a][0].z - feet[a][1].z).total_cmp(&(feet[b][0].z - feet[b][1].z))).unwrap_or(0);
    let phase = best as f32 / (n - 1).max(1) as f32;
    let dt = c.secs / (n - 1).max(1) as f32;
    let mut speeds = Vec::new();
    for side in 0..2 {
        let (lo, hi) = feet.iter().fold((f32::MAX, f32::MIN), |(lo, hi), f| (lo.min(f[side].y), hi.max(f[side].y)));
        let low = lo + (hi - lo) * 0.2;
        for w in feet.windows(2) {
            if w[0][side].y < low && w[1][side].y < low {
                speeds.push((w[0][side].z - w[1][side].z) / dt);
            }
        }
    }
    speeds.sort_by(f32::total_cmp);
    let stride = speeds.get(speeds.len() / 2).copied().unwrap_or(0.0).max(0.0) * c.secs;
    (phase, if stride > 0.05 { stride } else { 0.0 })
}

/// Turn every clip so the body faces the way the survivor does: take out the root's mean heading
/// over the clip (Meshy's idle stands turned 40° off its forward), keeping its sway within the clip.
fn face_forward(body: &mut Body) {
    let Some(root) = (0..body.rig.len()).find(|&b| body.rig.bones[b].parent.is_none()) else { return };
    for clip in &mut body.clips {
        let (sin, cos) = clip.frames.iter().fold((0.0, 0.0), |(s, c), f| {
            let fwd = f.r[root] * Vec3::Z;
            let yaw = fwd.x.atan2(fwd.z);
            (s + yaw.sin(), c + yaw.cos())
        });
        let turn = Quat::from_rotation_y(-f32::atan2(sin, cos));
        for f in &mut clip.frames {
            f.r[root] = turn * f.r[root];
            let t = turn * Vec3::new(f.t[root].x, 0.0, f.t[root].z);
            (f.t[root].x, f.t[root].z) = (t.x, t.z);
        }
    }
}

/// Where model-less clothing's tint samples the body texture: the lightest texel any torso vertex
/// sits on (the base layer's shirt), so a colour multiplied onto it shows as itself.
fn light_uv(body: &Body, texture: &Image) -> Option<[f32; 2]> {
    let torso = REGIONS.iter().position(|r| *r == BodyPart::Torso)? as u8;
    let (w, h) = (texture.width() as f32, texture.height() as f32);
    let data = texture.data.as_deref()?;
    // The first mip level: sRGB RGBA8. Score a texel and its neighbours, so mips keep it light.
    let lum = |x: i32, y: i32| {
        let (x, y) = (x.clamp(0, w as i32 - 1) as usize, y.clamp(0, h as i32 - 1) as usize);
        let p = &data[(y * w as usize + x) * 4..][..3];
        p.iter().map(|&c| c as u32).sum::<u32>()
    };
    (0..body.mesh.vertices())
        .filter(|&v| body.regions[v] == torso)
        .map(|v| body.mesh.uvs[v])
        .max_by_key(|uv| {
            let (x, y) = ((uv[0].rem_euclid(1.0) * w) as i32, (uv[1].rem_euclid(1.0) * h) as i32);
            (-2..=2).flat_map(|dx| (-2..=2).map(move |dy| (dx, dy))).map(|(dx, dy)| lum(x + dx * 2, y + dy * 2)).min().unwrap_or(0)
        })
}

/// A flat ring round a survivor's feet.
fn ring_mesh() -> Mesh {
    let (inner, outer, n) = (0.36, 0.44, 40);
    let mut pos = Vec::new();
    let mut idx = Vec::new();
    for i in 0..n {
        let a = i as f32 / n as f32 * TAU;
        let d = Vec2::from_angle(a);
        pos.push([d.x * inner, 0.0, d.y * inner]);
        pos.push([d.x * outer, 0.0, d.y * outer]);
        let (k, next) = (i as u32 * 2, ((i + 1) % n) as u32 * 2);
        idx.extend_from_slice(&[k, next, k + 1, k + 1, next, next + 1]);
    }
    let normals = vec![[0.0, 1.0, 0.0]; pos.len()];
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_indices(Indices::U32(idx))
}

impl SurvivorModel {
    /// Load the body and the wearables (see the module docs). `None`: draw the grey-box.
    pub fn load(
        options: &crate::Options,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
        bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    ) -> Option<Self> {
        let path = body_path(options)?;
        let mut body = match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| parse_body(&b)) {
            Ok(b) => b,
            Err(e) => {
                warn!("survivor body {}: {e}; drawing the grey-box", path.display());
                return None;
            }
        };
        face_forward(&mut body);
        let mut wardrobe = HashMap::new();
        load_wearables(&options.data.join("../models/wearables"), &body, &mut wardrobe);
        if let Some(dir) = path.parent() {
            load_wearables(&dir.join("wearables"), &body, &mut wardrobe);
        }
        let clip = |name: &str| body.clips.iter().position(|c| c.name.eq_ignore_ascii_case(name));
        let idle = clip("idle");
        let loops = [clip("walk"), clip("run"), clip("sneak")];
        let reference = idle.map_or_else(|| body.rig.rest(), |i| body.clips[i].frames[0].clone());
        let retarget = Retarget::new(&body, reference);
        let mut phase = [0.0; 3];
        let mut stride = [0.0; 3];
        for (k, c) in loops.iter().enumerate() {
            if let Some(c) = c {
                (phase[k], stride[k]) = phase_and_stride(&body, *c);
            }
        }
        let rig = &body.rig;
        let upper: Vec<usize> = {
            let mut up = vec![false; rig.len()];
            for b in 0..rig.len() {
                up[b] = rig.bones[b].name.to_lowercase().starts_with("spine") || rig.bones[b].parent.is_some_and(|p| up[p]);
            }
            (0..rig.len()).filter(|&b| up[b]).collect()
        };
        let right_arm = retarget.bones_of(&[UARM_R, FARM_R, HAND_R]);
        let flinching = (0..rig.len()).filter_map(|b| if let Src::Grey(g) = retarget.src[b] { UPPER.contains(&g).then_some((b, g)) } else { None }).collect();
        let feet_region = REGIONS.iter().position(|r| *r == BodyPart::Feet).unwrap() as u8;
        let feet = (0..body.mesh.vertices()).filter(|&v| body.regions[v] == feet_region).collect();
        // The weapon's grip on the `hand_r` socket, turned so it points as the grey-box's does.
        let hand_r = rig.find("RightHand").or_else(|| body.sockets.get("hand_r").map(|s| s.bone)).unwrap_or(0);
        let rest_rot = rig.inverse_bind[hand_r].inverse().to_scale_rotation_translation().1;
        let grip = body.sockets.get("hand_r").map_or(Vec3::ZERO, |s| rest_rot.inverse() * s.offset);
        let weapon_frame = Transform::from_translation(grip).with_rotation(retarget.ref_global[hand_r].inverse() * Quat::IDENTITY);
        // The texture's own colours times the vertex colours (white, or a tint).
        let texture = body.texture.as_ref().and_then(|(bytes, ext)| match crate::zvat::texture_image(bytes, ext) {
            Ok(i) => Some(i),
            Err(e) => {
                warn!("survivor texture: {e}");
                None
            }
        });
        let light = texture.as_ref().and_then(|t| light_uv(&body, t));
        let texture = texture.map(|t| images.add(t));
        let body_material = materials.add(StandardMaterial { base_color_texture: texture, perceptual_roughness: 0.85, ..default() });
        let outfit_material = materials.add(StandardMaterial { perceptual_roughness: 0.85, cull_mode: None, double_sided: true, ..default() });
        let mut ring_material = |c: Color| materials.add(StandardMaterial { base_color: c.with_alpha(0.75), unlit: true, alpha_mode: AlphaMode::Blend, depth_bias: 50.0, ..default() });
        let (ring_me, ring_other) = (ring_material(JACKET_ME), ring_material(JACKET_OTHER));
        let bindposes = bindposes.add(SkinnedMeshInverseBindposes::from(rig.inverse_bind.clone()));
        let model = Self {
            idle,
            loops,
            phase,
            stride,
            upper,
            right_arm,
            flinching,
            all: (0..rig.len()).collect(),
            feet,
            light_uv: light,
            hand_r,
            weapon_frame,
            retarget,
            bare: Handle::default(),
            body_material,
            outfit_material,
            bindposes,
            ring: meshes.add(ring_mesh()),
            ring_me,
            ring_other,
            wardrobe,
            body,
        };
        let (bare, _) = model.dress(&[], &OutfitView::default());
        let model = Self { bare: meshes.add(bare.to_mesh()), ..model };
        info!(
            "survivor: {} ({} bones, {} triangles, clips {}), strides walk {:.2} run {:.2} sneak {:.2} m, {} wearables",
            path.display(),
            model.body.rig.len(),
            model.body.mesh.triangles(),
            model.body.clips.iter().map(|c| format!("{} {:.2}s", c.name, c.secs)).collect::<Vec<_>>().join(", "),
            model.stride[0],
            model.stride[1],
            model.stride[2],
            model.wardrobe.len()
        );
        Some(model)
    }

    /// Metres per locomotion cycle walking, running and sneaking (the grey-box's where a clip has none).
    pub fn strides(&self) -> [f32; 3] {
        let grey = [1.5, 2.5, 0.95];
        std::array::from_fn(|k| if self.stride[k] > 0.0 { self.stride[k] } else { grey[k] })
    }

    fn clip_at(&self, clip: Option<usize>, u: f32) -> RigPose {
        clip.map_or_else(|| self.retarget.reference.clone(), |i| self.body.clips[i].sample(u))
    }

    /// The pose of survivor `a` now: see the module docs.
    pub fn compose(&self, a: &SurvivorAnim, now: f32, style: Style, armed: bool) -> RigPose {
        let body = &self.body;
        let [walk, run, sneak] = std::array::from_fn(|k| self.clip_at(self.loops[k], a.cycle + self.phase[k]));
        let idle_u = self.idle.map_or(0.0, |i| now / body.clips[i].secs + a.player as f32 * 0.37);
        let standing = self.clip_at(self.idle, idle_u).blend(&sneak, a.sneak_w);
        let moving = walk.blend(&run, a.run_w).blend(&sneak, a.sneak_w);
        let mut pose = standing.blend(&moving, a.move_w);
        if armed {
            let held = self.retarget.apply(body, &survivor::locomotion(a, armed));
            pose.blend_bones(&held, 1.0, Some(&self.right_arm));
        }
        if let Some((act, whole, w)) = survivor::action_now(a, now, style, armed) {
            pose.blend_bones(&self.retarget.apply(body, &act), w, Some(if whole { &self.all } else { &self.upper }));
        }
        // The flinch, on top: each grey-box turn about the axes it has in the reference pose.
        let w = survivor::flinch_weight(a, now);
        if w > 0.0 {
            let f = survivor::flinch();
            for &(b, g) in &self.flinching {
                let t = self.retarget.ref_global[b];
                pose.r[b] *= t.inverse() * Quat::IDENTITY.slerp(f.rot[g], w) * t;
            }
        }
        self.ground(&mut pose);
        let root = 0;
        // Running has a moment in the air each step.
        pose.t[root].y += a.run_w * a.move_w * 0.05 * (a.cycle * TAU * 2.0).sin().max(0.0);
        if let Some(since) = a.dead_since {
            let e = now - since;
            pose = pose.blend(&self.retarget.apply(body, &survivor::dead(e)), smooth(0.0, 0.12, e));
        }
        pose
    }

    /// Push the pose down (or up) so the lowest point of the feet is on the ground.
    fn ground(&self, pose: &mut RigPose) {
        if self.feet.is_empty() {
            return;
        }
        let m = self.body.rig.skin_matrices(pose);
        let mesh = &self.body.mesh;
        let mut lowest = f32::MAX;
        for &v in &self.feet {
            let (j, w, p) = (mesh.joints[v], mesh.weights[v], mesh.positions[v]);
            let y: f32 = (0..4).map(|k| w[k] * m[j[k] as usize].transform_point3(p).y).sum();
            lowest = lowest.min(y);
        }
        pose.t[0].y -= lowest;
    }

    /// The meshes for an outfit: the body with the regions worn wearables hide cut away (and the
    /// regions model-less clothing covers tinted its colour), and everything worn merged into one
    /// mesh (none if nothing is drawn).
    fn dress(&self, items: &[ItemDef], outfit: &OutfitView) -> (SkinMesh, Option<SkinMesh>) {
        let mut hidden = [false; REGIONS.len()];
        let mut tint: [Option<(u8, [f32; 4])>; REGIONS.len()] = [None; REGIONS.len()];
        let mut worn = SkinMesh::default();
        let colour = |d: &ItemDef| Color::srgb(d.color.0, d.color.1, d.color.2).to_linear().to_f32_array();
        let model_of = |d: &ItemDef| d.model.as_ref().and_then(|m| self.wardrobe.get(m));
        let worn_defs: Vec<&ItemDef> = outfit.worn.iter().filter_map(|&i| items.get(i as usize)).collect();
        // A garment entirely under a model on a higher layer that hides what it covers (a t-shirt
        // under a hoodie) isn't drawn: it would only show through the outer one's edges.
        let buried = |d: &ItemDef| {
            let Some(c) = d.clothing.as_ref().filter(|c| c.takes.is_empty() && !c.covers.is_empty()) else { return false };
            c.covers.iter().all(|r| {
                worn_defs.iter().any(|o| {
                    o.clothing.as_ref().is_some_and(|oc| oc.layer > c.layer) && model_of(o).is_some_and(|w| w.hides.contains(r))
                })
            })
        };
        // Garments drawn as models: each cuts, in the regions it hides, what other garments have
        // there above its hem (the jeans' top under a shirt worn out), whatever the pose.
        let drawn: Vec<&Wearable> = worn_defs.iter().filter(|d| !buried(d)).filter_map(|d| model_of(d)).collect();
        for &d in &worn_defs {
            if buried(d) {
                continue;
            }
            if let Some(w) = model_of(d) {
                let cuts: Vec<(u8, f32)> = drawn
                    .iter()
                    .filter(|o| !std::ptr::eq(**o, w))
                    .filter_map(|o| o.hem_z.map(|h| (o, h)))
                    .flat_map(|(o, h)| o.hides.iter().filter_map(move |r| REGIONS.iter().position(|x| x == r).map(|k| (k as u8, h + CUT_ABOVE_HEM))))
                    .collect();
                match (&w.regions, cuts.is_empty()) {
                    (Some(regions), false) => {
                        let cut = |v: u32| cuts.iter().any(|&(r, h)| regions[v as usize] == r && w.mesh.positions[v as usize].y > h);
                        worn.append_where(&w.mesh, |t| !t.iter().all(|&v| cut(v)));
                    }
                    _ => worn.append(&w.mesh),
                }
                for r in &w.hides {
                    hidden[REGIONS.iter().position(|x| x == r).unwrap()] = true;
                }
                continue;
            }
            let Some(c) = &d.clothing else { continue };
            if c.takes.is_empty() {
                // Clothing with no model: the regions it covers in its colour, the outermost layer showing.
                for r in &c.covers {
                    let k = REGIONS.iter().position(|x| x == r).unwrap();
                    if tint[k].is_none_or(|(layer, _)| layer <= c.layer as u8) {
                        tint[k] = Some((c.layer as u8, colour(d)));
                    }
                }
            }
            for slot in &c.takes {
                let (socket, min, max) = match slot {
                    WearSlot::Face => ("eyes", Vec3::new(-0.07, -0.012, 0.0), Vec3::new(0.07, 0.016, 0.012)),
                    WearSlot::HeadTop => ("head_top", Vec3::new(-0.1, -0.075, -0.11), Vec3::new(0.1, 0.012, 0.105)),
                    WearSlot::Neck => ("neck", Vec3::new(-0.085, -0.03, -0.075), Vec3::new(0.085, 0.05, 0.08)),
                };
                self.socket_box(&mut worn, socket, min, max, colour(d));
            }
        }
        // Bags: on the back, or hanging at the left hip from the shoulder.
        for (item, socket) in [(outfit.back, "back"), (outfit.shoulder, "hip_l")] {
            let Some(d) = item.and_then(|i| items.get(i as usize)) else { continue };
            if let Some(w) = model_of(d) {
                worn.append(&w.mesh);
                continue;
            }
            let litres = d.bag.as_ref().map_or(10.0, |b| b.capacity_l).clamp(5.0, 60.0);
            let s = (litres / 25.0).cbrt();
            let (min, max) = if socket == "back" {
                (Vec3::new(-0.15 * s, -0.22 * s, -0.17 * s), Vec3::new(0.15 * s, 0.18 * s, 0.0))
            } else {
                (Vec3::new(0.0, -0.14 * s, -0.12 * s), Vec3::new(0.09 * s, 0.14 * s, 0.12 * s))
            };
            self.socket_box(&mut worn, socket, min, max, colour(d));
        }
        let src = &self.body.mesh;
        let mut body = SkinMesh { indices: Vec::with_capacity(src.indices.len()), ..src.clone() };
        let region = |v: u32| self.body.regions[v as usize] as usize;
        for t in src.indices.as_chunks::<3>().0 {
            if !t.iter().any(|&v| hidden.get(region(v)).copied().unwrap_or(false)) {
                body.indices.extend_from_slice(t);
            }
        }
        for v in 0..body.vertices() {
            let t = tint.get(region(v as u32)).copied().flatten();
            body.colors[v] = t.map_or([1.0; 4], |(_, c)| c);
            if let (Some(_), Some(uv)) = (t, self.light_uv) {
                body.uvs[v] = uv;
            }
        }
        (body, (worn.triangles() > 0).then_some(worn))
    }

    /// A grey-box box from `min` to `max` on a socket (`hip_l`: the left hip, on the hips bone).
    fn socket_box(&self, into: &mut SkinMesh, socket: &str, min: Vec3, max: Vec3, color: [f32; 4]) {
        let frame = if socket == "hip_l" {
            self.body.rig.find("Hips").map(|b| (b, Mat4::from_translation(self.body.rig.inverse_bind[b].inverse().transform_point3(Vec3::ZERO) + Vec3::new(0.17, -0.12, 0.0))))
        } else {
            self.body.socket_frame(socket)
        };
        if let Some((bone, frame)) = frame {
            into.add_box(frame, min, max, bone as u16, color);
        }
    }
}

/// What a survivor drawn as the modular character has on now, and its body and outfit entities.
#[derive(Component)]
pub struct Dress {
    player: u32,
    body: Entity,
    outfit: Entity,
    worn: Option<OutfitView>,
}

/// Build a survivor's skeleton (one joint per bone), body, outfit, held weapon and ring under `root`.
/// Returns the joints.
pub fn attach(commands: &mut Commands, model: &SurvivorModel, root: Entity, player: u32, local: bool) -> Vec<Entity> {
    let rig = &model.body.rig;
    let mut joints: Vec<Entity> = Vec::with_capacity(rig.len());
    for b in &rig.bones {
        let parent = b.parent.map_or(root, |p| joints[p]);
        joints.push(commands.spawn((Transform { translation: b.t, rotation: b.r, scale: b.s }, Visibility::Inherited, ChildOf(parent))).id());
    }
    let skin = SkinnedMesh { inverse_bindposes: model.bindposes.clone(), joints: joints.clone() };
    // Posed far from the rest pose (lying down), so don't cull by the rest-pose bounds.
    let body = commands.spawn((Mesh3d(model.bare.clone()), MeshMaterial3d(model.body_material.clone()), skin.clone(), NoFrustumCulling, Transform::default(), ChildOf(root))).id();
    let outfit = commands.spawn((MeshMaterial3d(model.outfit_material.clone()), skin, NoFrustumCulling, Transform::default(), Visibility::Hidden, ChildOf(root))).id();
    commands.spawn((HeldWeapon { player }, model.weapon_frame, Visibility::Hidden, ChildOf(joints[model.hand_r])));
    let ring = if local { model.ring_me.clone() } else { model.ring_other.clone() };
    commands.spawn((Mesh3d(model.ring.clone()), MeshMaterial3d(ring), Transform::from_xyz(0.0, 0.02, 0.0), NotShadowCaster, ChildOf(root)));
    commands.entity(root).insert(Dress { player, body, outfit, worn: None });
    joints
}

/// What's worn: the worn items, the back bag and the shoulder bag.
type OutfitKey = (Vec<u16>, Option<u16>, Option<u16>);

/// Outfits already built, by what's worn: (body, outfit).
#[derive(Default)]
pub(crate) struct OutfitMeshes(HashMap<OutfitKey, (Handle<Mesh>, Option<Handle<Mesh>>)>);

/// Dim the marker rings with the daylight ([`RING_NIGHT`] at night).
pub(crate) fn dim_rings(daylight: Option<Res<crate::daynight::Daylight>>, model: Option<Res<SurvivorModel>>, mut materials: ResMut<Assets<StandardMaterial>>, mut last: Local<Option<f32>>) {
    let (Some(daylight), Some(model)) = (daylight, model) else { return };
    let k = RING_NIGHT + (1.0 - RING_NIGHT) * daylight.0.clamp(0.0, 1.0);
    if last.is_some_and(|l| (l - k).abs() < 0.005) {
        return;
    }
    *last = Some(k);
    for (handle, colour) in [(&model.ring_me, JACKET_ME), (&model.ring_other, JACKET_OTHER)] {
        if let Some(mut m) = materials.get_mut(handle) {
            let c = colour.to_linear();
            m.base_color = Color::LinearRgba(LinearRgba::new(c.red * k, c.green * k, c.blue * k, 0.75));
        }
    }
}

/// Dress every survivor as the snapshot says, the moment it changes.
pub(crate) fn dress_survivors(
    mut commands: Commands,
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    model: Option<Res<SurvivorModel>>,
    mut built: Local<OutfitMeshes>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut dressed: Query<&mut Dress>,
) {
    let Some(model) = model else { return };
    for mut d in &mut dressed {
        let now = link.curr.outfits.iter().find(|o| o.id == d.player).cloned().unwrap_or(OutfitView { id: d.player, ..default() });
        if d.worn.as_ref() == Some(&now) {
            continue;
        }
        let key = (now.worn.clone(), now.back, now.shoulder);
        let (body, outfit) = built
            .0
            .entry(key)
            .or_insert_with(|| {
                let (body, outfit) = model.dress(&join.items, &now);
                (meshes.add(body.to_mesh()), outfit.map(|o| meshes.add(o.to_mesh())))
            })
            .clone();
        commands.entity(d.body).insert(Mesh3d(body));
        match outfit {
            Some(m) => commands.entity(d.outfit).insert((Mesh3d(m), Visibility::Inherited)),
            None => commands.entity(d.outfit).insert(Visibility::Hidden),
        };
        d.worn = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The promoted survivor and its wearables' folder.
    fn shipped_body() -> (Body, PathBuf) {
        let models = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/models");
        let path = models.join(format!("characters/{DEFAULT_BODY}.glb"));
        (parse_body(&std::fs::read(&path).unwrap()).unwrap(), models.join("wearables"))
    }

    /// The shipped body: clips, sockets, regions; the retarget leaves the reference pose as it is for
    /// the grey-box's rest, and its clips stride like a walk, a run and a sneak.
    #[test]
    fn the_survivor_body_reads_and_retargets() {
        let (body, wearables) = shipped_body();
        assert!(body.mesh.triangles() > 1000 && body.rig.len() >= 20);
        for clip in ["idle", "walk", "run", "sneak"] {
            assert!(body.clip(clip).is_some(), "no {clip}");
        }
        for s in ["eyes", "head_top", "neck", "hand_r", "back"] {
            assert!(body.sockets.contains_key(s), "no socket {s}");
        }
        assert!(body.regions.iter().all(|&r| (r as usize) < REGIONS.len()));
        let reference = body.clip("idle").unwrap().frames[0].clone();
        let rt = Retarget::new(&body, reference.clone());
        let back = rt.apply(&body, &SPose::default());
        for b in 0..body.rig.len() {
            assert!(back.r[b].angle_between(reference.r[b]) < 1e-3, "{} turned", body.rig.bones[b].name);
        }
        // Eyes on the face: about 1.65 m up and a little forward.
        let (_, eyes) = body.socket_frame("eyes").unwrap();
        let p = eyes.w_axis.truncate();
        assert!((1.55..1.75).contains(&p.y) && p.z > 0.05, "eyes at {p}");
        let mut wardrobe = HashMap::new();
        load_wearables(&wearables, &body, &mut wardrobe);
        assert_eq!(wardrobe.get("hoodie").map(|w| w.hides.clone()), Some(vec![BodyPart::Torso, BodyPart::Arms]));
        assert!(wardrobe.get("glasses").is_some_and(|w| w.mesh.triangles() <= 100));
        let walk = body.clips.iter().position(|c| c.name == "walk").unwrap();
        let run = body.clips.iter().position(|c| c.name == "run").unwrap();
        let (_, ws) = phase_and_stride(&body, walk);
        let (_, rs) = phase_and_stride(&body, run);
        assert!((0.8..2.5).contains(&ws) && rs > ws, "walk {ws} run {rs}");
    }
}
