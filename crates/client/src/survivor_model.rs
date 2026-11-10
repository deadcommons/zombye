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
//! **Bags in the hand** (#91) hang straight down from the grip, whatever the hand is doing: each hand
//! has a hang joint past the rig's bones (`hang`), at its grip but upright (the T-pose's arm turned
//! down), and a bag held there is skinned to it. Its model is built on the right hand (`hand_r`) and
//! mirrored for the left.
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
/// How much bigger a hat is drawn over a balaclava (see `dress`), about the head's centre, which is
/// this far below the crown (the `head_top` socket).
const OVER_HEAD_TOP: f32 = 1.06;
const HEAD_CENTRE_BELOW_TOP: f32 = 0.105;

/// Each stance's clips, walking, running and sneaking: every direction it has (each the first of
/// these names the body has; a missing one is left out). Which way each travels is measured from its
/// planted feet, not taken from its name.
const STANCE_CLIPS: [&[&[&str]]; 3] = [
    &[&["walk_alt", "walk"], &["walk_left"], &["walk_right"], &["walk_back"]],
    &[&["run"], &["run_back_left"], &["run_back_right"]],
    &[&["sneak"], &["sneak_left"], &["sneak_right"], &["sneak_back"]],
];
/// The cadence (cycles a second, two steps each) each stance's clips are widened to reach
/// ([`amplify`], at most [`MAX_SWING`]): the game's walk (3 m/s) and run (5.5 m/s) are brisker than the
/// clips were made at, and played faster to keep the feet planted they'd step comically quickly
/// (the owner). About 2.9 steps a second walking and running; the sneak as made.
const CADENCE: [f32; 3] = [1.45, 1.45, 0.9];
const MAX_SWING: f32 = 1.6;
const LEG_BONES: [&str; 6] = ["LeftUpLeg", "LeftLeg", "LeftFoot", "RightUpLeg", "RightLeg", "RightFoot"];
/// Clips further apart than this (radians) aren't blended (two different runs' legs would mush):
/// the nearer plays, its hips turned the rest of the way.
const BLEND_SPAN: f32 = 1.75;

/// One locomotion clip: which way it travels relative to where the survivor faces (radians: 0 ahead,
/// +π/2 to its left, ±π back), its phase (the left foot furthest along that way) and its stride
/// (metres a cycle).
#[derive(Clone, Copy, Debug)]
struct Gait {
    clip: usize,
    angle: f32,
    phase: f32,
    stride: f32,
}

/// `a` - `b` wrapped into (-π, π].
fn angle_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    if d > std::f32::consts::PI { d - TAU } else { d }
}

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
    /// The idle clip, and each stance's locomotion clips (walk, run, sneak) by direction.
    idle: Option<usize>,
    gaits: [Vec<Gait>; 3],
    /// The bones the hips' turn is taken back out of (the spine's first, under the hips).
    spine_base: Vec<usize>,
    /// Bones an action takes over (the upper body: everything from the spine up), the arm that holds
    /// a weapon, the bones the flinch turns, all of them.
    upper: Vec<usize>,
    right_arm: Vec<usize>,
    /// The bones a gesture takes (#67): the left arm, and both arms.
    left_arm: Vec<usize>,
    arms: Vec<usize>,
    flinching: Vec<(usize, usize)>,
    all: Vec<usize>,
    /// Body vertices on the feet (for standing them on the ground).
    feet: Vec<usize>,
    /// A light texel of the body's texture, for tinting (see [`light_uv`]).
    light_uv: Option<[f32; 2]>,
    hand_r: usize,
    /// Per hand (right, left): its bone and its grip in the rest pose, for the hang joints.
    hang: [(usize, Vec3); 2],
    weapon_frame: Transform,
    bare: Handle<Mesh>,
    body_material: Handle<StandardMaterial>,
    outfit_material: Handle<StandardMaterial>,
    bindposes: Handle<SkinnedMeshInverseBindposes>,
    ring: Handle<Mesh>,
    ring_me: Handle<StandardMaterial>,
    ring_other: Handle<StandardMaterial>,
    /// Another player's ring on a server that marks agents, when a program plays it (#69).
    ring_agent: Handle<StandardMaterial>,
}

/// The ring of a survivor a program plays, on a server whose rules mark agents (#69): violet,
/// where a person's is teal.
pub const RING_AGENT: Color = Color::srgb(0.66, 0.42, 0.9);

/// A survivor's marker ring: whose, and whether it is drawn as an agent's now.
#[derive(Component)]
pub(crate) struct Ring {
    player: u32,
    agent: bool,
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

/// A looping clip's way of travel, phase and stride: while a foot is planted (in the lowest fifth of
/// its height) it slides against the way the body would travel, at its speed (as `zvat.py` measures
/// a stride). `None` if no foot is planted for long (an idle).
fn gait_of(body: &Body, clip: usize) -> Option<Gait> {
    let rig = &body.rig;
    let c = &body.clips[clip];
    let (l, r) = (rig.find("LeftFoot")?, rig.find("RightFoot")?);
    let feet: Vec<[Vec3; 2]> = c.frames.iter().map(|p| rig.globals(p)).map(|g| [g[l].w_axis.truncate(), g[r].w_axis.truncate()]).collect();
    let n = feet.len();
    let dt = c.secs / (n - 1).max(1) as f32;
    let mut slides: Vec<Vec2> = Vec::new();
    for side in 0..2 {
        let (lo, hi) = feet.iter().fold((f32::MAX, f32::MIN), |(lo, hi), f| (lo.min(f[side].y), hi.max(f[side].y)));
        let low = lo + (hi - lo) * 0.2;
        for w in feet.windows(2) {
            if w[0][side].y < low && w[1][side].y < low {
                slides.push((w[0][side].xz() - w[1][side].xz()) / dt);
            }
        }
    }
    let mean = slides.iter().sum::<Vec2>() / slides.len().max(1) as f32;
    let way = mean.try_normalize()?;
    let mut speeds: Vec<f32> = slides.iter().map(|v| v.dot(way)).collect();
    speeds.sort_by(f32::total_cmp);
    let stride = speeds[speeds.len() / 2].max(0.0) * c.secs;
    if stride < 0.05 {
        return None;
    }
    let along = |f: &[Vec3; 2]| (f[0].xz() - f[1].xz()).dot(way);
    let best = (0..n).max_by(|&a, &b| along(&feet[a]).total_cmp(&along(&feet[b]))).unwrap_or(0);
    Some(Gait { clip, angle: way.x.atan2(way.y), phase: best as f32 / (n - 1).max(1) as f32, stride })
}

/// Turn the idle so the body faces the way the survivor does: take out the root's mean heading over
/// the clip (Meshy's idle stands turned 40° off its forward), keeping its sway within it. Not the
/// moving clips: a diagonal one's hips are turned on purpose.
fn face_forward(body: &mut Body) {
    let Some(root) = (0..body.rig.len()).find(|&b| body.rig.bones[b].parent.is_none()) else { return };
    for clip in body.clips.iter_mut().filter(|c| c.name.eq_ignore_ascii_case("idle")) {
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

/// Swings `bones` of a clip wider (k > 1) or narrower round their mean pose over the clip: each
/// frame's turn away from the mean is scaled by `k` (a longer stride from the legs, a bigger arm swing).
fn amplify(clip: &mut crate::character::Clip, bones: &[usize], k: f32) {
    for &b in bones {
        let first = clip.frames[0].r[b];
        let sum = clip.frames.iter().fold(Vec4::ZERO, |acc, f| {
            let q = f.r[b];
            acc + if q.dot(first) < 0.0 { -Vec4::from(q) } else { Vec4::from(q) }
        });
        let mean = Quat::from_vec4(sum).normalize();
        for f in &mut clip.frames {
            let d = mean.inverse() * f.r[b];
            let (axis, angle) = d.to_axis_angle();
            let angle = if angle > std::f32::consts::PI { angle - TAU } else { angle };
            f.r[b] = (mean * Quat::from_axis_angle(axis, angle * k)).normalize();
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

/// Each stance's clips ([`STANCE_CLIPS`]), their legs widened to reach its [`CADENCE`] at its speed
/// (if they fall short), then measured.
fn gaits(body: &mut Body) -> [Vec<Gait>; 3] {
    use zombye_sim::players::{RUN_SPEED, SNEAK_SPEED, WALK_SPEED};
    let legs: Vec<usize> = LEG_BONES.iter().filter_map(|n| body.rig.find(n)).collect();
    let speeds = [WALK_SPEED, RUN_SPEED, SNEAK_SPEED];
    std::array::from_fn(|s| {
        let want = speeds[s] / CADENCE[s];
        let mut out = Vec::new();
        for names in STANCE_CLIPS[s] {
            let Some(c) = names.iter().find_map(|n| body.clips.iter().position(|x| x.name.eq_ignore_ascii_case(n))) else { continue };
            let Some(g) = gait_of(body, c) else { continue };
            if g.stride < want {
                // Stride grows about in step with the swing: two tries to land near the target.
                let original = body.clips[c].clone();
                let mut k = (want / g.stride).min(MAX_SWING);
                for _ in 0..2 {
                    body.clips[c] = original.clone();
                    amplify(&mut body.clips[c], &legs, k);
                    let got = gait_of(body, c).map_or(g.stride, |g| g.stride);
                    k = (k * want / got).clamp(1.0, MAX_SWING);
                }
                body.clips[c] = original;
                amplify(&mut body.clips[c], &legs, k);
            }
            if let Some(g) = gait_of(body, c) {
                out.push(g);
            }
        }
        out
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
        let idle = body.clips.iter().position(|c| c.name.eq_ignore_ascii_case("idle"));
        let gaits = gaits(&mut body);
        let reference = idle.map_or_else(|| body.rig.rest(), |i| body.clips[i].frames[0].clone());
        let retarget = Retarget::new(&body, reference);
        let rig = &body.rig;
        let spine_base = (0..rig.len()).filter(|&b| rig.bones[b].parent.is_some_and(|p| rig.bones[p].parent.is_none()) && rig.bones[b].name.to_lowercase().starts_with("spine")).collect();
        let upper: Vec<usize> = {
            let mut up = vec![false; rig.len()];
            for b in 0..rig.len() {
                up[b] = rig.bones[b].name.to_lowercase().starts_with("spine") || rig.bones[b].parent.is_some_and(|p| up[p]);
            }
            (0..rig.len()).filter(|&b| up[b]).collect()
        };
        let right_arm = retarget.bones_of(&[UARM_R, FARM_R, HAND_R]);
        let left_arm = retarget.bones_of(&survivor::LEFT_ARM);
        let arms = retarget.bones_of(&survivor::ARMS);
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
        let (ring_me, ring_other, ring_agent) = (ring_material(JACKET_ME), ring_material(JACKET_OTHER), ring_material(RING_AGENT));
        // The hang joints: at each hand's grip in the rest pose, after the rig's bones.
        let hang = [("hand_r", "RightHand"), ("hand_l", "LeftHand")].map(|(socket, bone)| match body.socket_frame(socket) {
            Some((b, frame)) => (b, frame.transform_point3(Vec3::ZERO)),
            None => {
                let b = rig.find(bone).unwrap_or(0);
                (b, rig.inverse_bind[b].inverse().transform_point3(Vec3::ZERO))
            }
        });
        let mut inverse_bind = rig.inverse_bind.clone();
        inverse_bind.extend(hang.map(|(_, grip)| Mat4::from_translation(-grip)));
        let bindposes = bindposes.add(SkinnedMeshInverseBindposes::from(inverse_bind));
        let model = Self {
            idle,
            gaits,
            spine_base,
            upper,
            right_arm,
            left_arm,
            arms,
            flinching,
            all: (0..rig.len()).collect(),
            feet,
            light_uv: light,
            hand_r,
            hang,
            weapon_frame,
            retarget,
            bare: Handle::default(),
            body_material,
            outfit_material,
            bindposes,
            ring: meshes.add(ring_mesh()),
            ring_me,
            ring_other,
            ring_agent,
            wardrobe,
            body,
        };
        let (bare, _) = model.dress(&[], &OutfitView::default());
        let model = Self { bare: meshes.add(bare.to_mesh()), ..model };
        info!(
            "survivor: {} ({} bones, {} triangles), gaits {}, {} wearables",
            path.display(),
            model.body.rig.len(),
            model.body.mesh.triangles(),
            model
                .gaits
                .iter()
                .flatten()
                .map(|g| format!("{} {:+.0}° {:.2} m", model.body.clips[g.clip].name, g.angle.to_degrees(), g.stride))
                .collect::<Vec<_>>()
                .join(", "),
            model.wardrobe.len()
        );
        Some(model)
    }

    fn clip_at(&self, clip: Option<usize>, u: f32) -> RigPose {
        clip.map_or_else(|| self.retarget.reference.clone(), |i| self.body.clips[i].sample(u))
    }

    /// Which of stance `s`'s clips play moving at `angle` (relative to the facing), how much of each,
    /// and how far each one's hips turn to point its legs that way: the two either side of it blended,
    /// or, further apart than [`BLEND_SPAN`], the nearer turned (crossfading across the middle).
    fn mix(&self, s: usize, angle: f32) -> Vec<(Gait, f32, f32)> {
        let gaits = &self.gaits[s];
        match gaits.len() {
            0 => return Vec::new(),
            // A body with only a forward clip: as before directions existed (no turning).
            1 => return vec![(gaits[0], 1.0, 0.0)],
            _ => {}
        }
        // The nearest clip counter-clockwise (at or before `angle`) and the next one round.
        let before = |g: &Gait| angle_diff(angle, g.angle).rem_euclid(TAU);
        let a = *gaits.iter().min_by(|x, y| before(x).total_cmp(&before(y))).unwrap();
        let b = *gaits.iter().filter(|g| g.clip != a.clip).min_by(|x, y| (-before(x)).rem_euclid(TAU).total_cmp(&(-before(y)).rem_euclid(TAU))).unwrap();
        let span = angle_diff(b.angle, a.angle).rem_euclid(TAU).max(1e-3);
        let t = (before(&a) / span).clamp(0.0, 1.0);
        if span <= BLEND_SPAN {
            vec![(a, 1.0 - t, 0.0), (b, t, 0.0)]
        } else {
            let w = smooth(0.4, 0.6, t);
            vec![(a, 1.0 - w, angle_diff(angle, a.angle)), (b, w, angle_diff(angle, b.angle))]
        }
    }

    /// Metres per locomotion cycle for `a` now (its stances' clips at its direction of travel).
    pub fn cycle_metres(&self, a: &SurvivorAnim) -> f32 {
        let grey = [1.5, 2.5, 0.95];
        let weights = [(1.0 - a.run_w) * (1.0 - a.sneak_w), a.run_w * (1.0 - a.sneak_w), a.sneak_w];
        (0..3)
            .map(|s| {
                let m = self.mix(s, a.move_angle);
                let stride = if m.is_empty() { grey[s] } else { m.iter().map(|(g, w, _)| g.stride * w).sum() };
                weights[s] * stride
            })
            .sum::<f32>()
            .max(0.1)
    }

    /// Stance `s` moving at `a`'s direction and cycle: its clips blended, hips turned as [`Self::mix`] says.
    fn moving(&self, s: usize, a: &SurvivorAnim) -> RigPose {
        let mut out: Option<RigPose> = None;
        let mut total = 0.0;
        for (g, w, turn) in self.mix(s, a.move_angle) {
            if w <= 0.0 {
                continue;
            }
            let mut p = self.body.clips[g.clip].sample(a.cycle + g.phase);
            self.turn_hips(&mut p, turn);
            total += w;
            out = Some(match out {
                None => p,
                Some(o) => o.blend(&p, w / total),
            });
        }
        out.unwrap_or_else(|| self.retarget.reference.clone())
    }

    /// Turns the legs (the hips) `turn` radians about the vertical, the chest kept where it was.
    fn turn_hips(&self, pose: &mut RigPose, turn: f32) {
        if turn.abs() < 1e-3 {
            return;
        }
        let root = 0;
        let hips = pose.r[root];
        let yaw = Quat::from_rotation_y(turn);
        pose.r[root] = yaw * hips;
        for &b in &self.spine_base {
            pose.r[b] = hips.inverse() * yaw.inverse() * hips * pose.r[b];
        }
    }

    /// The pose of survivor `a` now: see the module docs.
    /// The hang joints' transforms (right hand, left) for `pose`: at each hand's grip, the arm of the
    /// T-pose turned to point down, so a bag held there hangs straight.
    pub fn hang_joints(&self, pose: &RigPose) -> [Transform; 2] {
        let rig = &self.body.rig;
        let globals = rig.globals(pose);
        [(self.hang[0], std::f32::consts::FRAC_PI_2), (self.hang[1], -std::f32::consts::FRAC_PI_2)]
            .map(|((bone, grip), turn)| Transform::from_translation((globals[bone] * rig.inverse_bind[bone]).transform_point3(grip)).with_rotation(Quat::from_rotation_z(turn)))
    }

    pub fn compose(&self, a: &SurvivorAnim, now: f32, style: Style, armed: bool) -> RigPose {
        let body = &self.body;
        let [walk, run, sneak] = std::array::from_fn(|k| self.moving(k, a));
        let idle_u = self.idle.map_or(0.0, |i| now / body.clips[i].secs + a.player as f32 * 0.37);
        let standing = self.clip_at(self.idle, idle_u).blend(&sneak, a.sneak_w);
        let moving = walk.blend(&run, a.run_w).blend(&sneak, a.sneak_w);
        let mut pose = standing.blend(&moving, a.move_w);
        if armed {
            let held = self.retarget.apply(body, &survivor::locomotion(a, armed));
            pose.blend_bones(&held, 1.0, Some(&self.right_arm));
        }
        if a.aim_w > 0.0 {
            // Aiming: the guard over the upper body.
            pose.blend_bones(&self.retarget.apply(body, &survivor::guard(style, armed)), a.aim_w, Some(&self.upper));
        }
        if let Some((g, both, w)) = survivor::gesture_now(a) {
            // A gesture (#67): the grey-box's arms carried over, the free arm or both.
            pose.blend_bones(&self.retarget.apply(body, &g), w, Some(if both { self.arms.as_slice() } else { self.left_arm.as_slice() }));
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
        if let Some((lie, w)) = survivor::lying(a, now) {
            pose = pose.blend(&self.retarget.apply(body, &lie), w);
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
        // (Not when the outer one is shorter: a t-shirt shows below a leather jacket.)
        let buried = |d: &ItemDef| {
            let Some(c) = d.clothing.as_ref().filter(|c| c.takes.is_empty() && !c.covers.is_empty()) else { return false };
            let inner_hem = model_of(d).and_then(|w| w.hem_z);
            c.covers.iter().all(|r| {
                worn_defs.iter().any(|o| {
                    let low_enough = |w: &Wearable| match (w.hem_z, inner_hem) {
                        (Some(outer), Some(inner)) => outer <= inner + 0.01,
                        _ => true,
                    };
                    o.clothing.as_ref().is_some_and(|oc| oc.layer > c.layer) && model_of(o).is_some_and(|w| w.hides.contains(r) && low_enough(w))
                })
            })
        };
        // Garments drawn as models: each cuts, in the regions it hides, what other garments have
        // there above its hem (the jeans' top under a shirt worn out), whatever the pose.
        let drawn: Vec<&Wearable> = worn_defs.iter().filter(|d| !buried(d)).filter_map(|d| model_of(d)).collect();
        // A hat over a model on a lower layer that takes the head top (a balaclava, #92) is drawn
        // `OVER_HEAD_TOP` bigger about the head's centre, so it sits over the knit, not through it.
        let takes_top = |d: &ItemDef| d.clothing.as_ref().filter(|c| c.takes.contains(&WearSlot::HeadTop)).map(|c| c.layer);
        let under_top = worn_defs.iter().filter(|d| model_of(d).is_some()).filter_map(|d| takes_top(d)).min();
        let head_centre = self.body.socket_frame("head_top").map(|(_, f)| f.transform_point3(Vec3::ZERO) - Vec3::Y * HEAD_CENTRE_BELOW_TOP);
        for &d in &worn_defs {
            if buried(d) {
                continue;
            }
            if let (Some(w), Some(layer), Some(under), Some(c)) = (model_of(d), takes_top(d), under_top, head_centre)
                && layer > under
            {
                let mut big = w.mesh.clone();
                for p in &mut big.positions {
                    *p = c + (*p - c) * OVER_HEAD_TOP;
                }
                worn.append(&big);
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
        // Bags in the hands, on the hang joints (see the module docs): the model mirrored for the left
        // hand, else a box hanging from the grip.
        let rig_len = self.body.rig.len();
        for (k, item) in [outfit.hand, outfit.off_hand].into_iter().enumerate() {
            let Some(d) = item.and_then(|i| items.get(i as usize)).filter(|d| d.bag.is_some()) else { continue };
            let joint = (rig_len + k) as u16;
            if let Some(w) = model_of(d) {
                worn.append(&w.mesh.hung(joint, k == 1));
                continue;
            }
            let s = (d.bag.as_ref().map_or(10.0, |b| b.capacity_l).clamp(5.0, 60.0) / 20.0).cbrt();
            let out = if k == 0 { -1.0 } else { 1.0 };
            let (a, b) = (Vec3::new(out * 0.08, -0.06 * s, -0.15 * s), Vec3::new(out * (0.08 + 0.32 * s), 0.06 * s, 0.15 * s));
            worn.add_box(Mat4::from_translation(self.hang[k].1), a.min(b), a.max(b), joint, colour(d));
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
    // The hang joints for bags in the hands, posed by `hang_joints`.
    for _ in 0..2 {
        joints.push(commands.spawn((Transform::default(), Visibility::Inherited, ChildOf(root))).id());
    }
    let skin = SkinnedMesh { inverse_bindposes: model.bindposes.clone(), joints: joints.clone() };
    // Posed far from the rest pose (lying down), so don't cull by the rest-pose bounds.
    let body = commands.spawn((Mesh3d(model.bare.clone()), MeshMaterial3d(model.body_material.clone()), skin.clone(), NoFrustumCulling, Transform::default(), ChildOf(root))).id();
    let outfit = commands.spawn((MeshMaterial3d(model.outfit_material.clone()), skin, NoFrustumCulling, Transform::default(), Visibility::Hidden, ChildOf(root))).id();
    commands.spawn((HeldWeapon { player }, model.weapon_frame, Visibility::Hidden, ChildOf(joints[model.hand_r])));
    let ring = if local { model.ring_me.clone() } else { model.ring_other.clone() };
    let mut ring = commands.spawn((Mesh3d(model.ring.clone()), MeshMaterial3d(ring), Transform::from_xyz(0.0, 0.02, 0.0), NotShadowCaster, ChildOf(root)));
    if !local {
        // Ours stays ours; another's turns violet if the server says a program plays it (`mark_agents`).
        ring.insert(Ring { player, agent: false });
    }
    commands.entity(root).insert(Dress { player, body, outfit, worn: None });
    joints
}

/// What's worn: the worn items, the back bag, the shoulder bag and what's in each hand.
type OutfitKey = (Vec<u16>, Option<u16>, Option<u16>, Option<u16>, Option<u16>);

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
    for (handle, colour) in [(&model.ring_me, JACKET_ME), (&model.ring_other, JACKET_OTHER), (&model.ring_agent, RING_AGENT)] {
        if let Some(mut m) = materials.get_mut(handle) {
            let c = colour.to_linear();
            m.base_color = Color::LinearRgba(LinearRgba::new(c.red * k, c.green * k, c.blue * k, 0.75));
        }
    }
}

/// Marked agents (#69): the ring of another player the server says a program plays is violet, a
/// person's teal. A handful of rings, and a material swapped only when the word changes.
pub(crate) fn mark_agents(link: Res<ServerLink>, model: Option<Res<SurvivorModel>>, mut rings: Query<(&mut Ring, &mut MeshMaterial3d<StandardMaterial>)>) {
    let Some(model) = model else { return };
    for (mut ring, mut material) in &mut rings {
        let agent = link.curr.players.iter().find(|p| p.id == ring.player).is_some_and(|p| p.agent);
        if agent != ring.agent {
            ring.agent = agent;
            material.0 = if agent { model.ring_agent.clone() } else { model.ring_other.clone() };
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
        let key = (now.worn.clone(), now.back, now.shoulder, now.hand, now.off_hand);
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

    /// The promoted survivor and its wearables' folder (or a work-in-progress export: `ZOMBYE_SURVIVOR`).
    fn shipped_body() -> (Body, PathBuf) {
        if let Some(dir) = std::env::var_os("ZOMBYE_SURVIVOR").map(PathBuf::from) {
            let path = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).find(|p| p.to_string_lossy().ends_with("_body.glb")).unwrap();
            let mut body = parse_body(&std::fs::read(&path).unwrap()).unwrap();
            face_forward(&mut body);
            return (body, dir.join("wearables"));
        }
        let models = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/models");
        let path = models.join(format!("characters/{DEFAULT_BODY}.glb"));
        let mut body = parse_body(&std::fs::read(&path).unwrap()).unwrap();
        face_forward(&mut body);
        (body, models.join("wearables"))
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
        // Every stance's clips, as the game swings them: a believable 2-3.6 steps a second at its speed
        // (the owner: the run was "comically fast"), feet planted; and each travels the way its name says
        // (walk_right is walk_left mirrored).
        let mut body = body;
        let gaits = gaits(&mut body);
        let speeds = [zombye_sim::players::WALK_SPEED, zombye_sim::players::RUN_SPEED, zombye_sim::players::SNEAK_SPEED];
        for (s, gs) in gaits.iter().enumerate() {
            if std::env::var_os("ZOMBYE_SURVIVOR").is_none() && gs.len() == 1 {
                continue; // the promoted body before its directional clips
            }
            assert!(gs.len() >= 3, "stance {s}: {} clips", gs.len());
            for g in gs {
                let name = &body.clips[g.clip].name;
                let steps = 2.0 * speeds[s] / g.stride;
                // Backward at full speed is quicker on the feet (the clip can only stretch so far).
                let most = if g.angle.abs() > 1.75 { 4.3 } else { 3.6 };
                assert!((1.6..most).contains(&steps), "{name}: {steps:.2} steps a second ({:.2} m a cycle)", g.stride);
                let want: f32 = if name.ends_with("back_left") { 135.0 } else if name.ends_with("back_right") { -135.0 } else if name.ends_with("left") { 90.0 }
                    else if name.ends_with("right") { -90.0 } else if name.ends_with("back") { 180.0 } else { 0.0 };
                let off = angle_diff(g.angle, want.to_radians()).to_degrees();
                println!("{name}: {steps:.2} steps a second, {:.2} m a cycle, travels {:.0}°", g.stride, g.angle.to_degrees());
                assert!(off.abs() < 30.0, "{name} travels at {:.0}°, not {want}°", g.angle.to_degrees());
            }
        }
    }
}
