//! Zombie crowd rendering with vertex animation textures (VAT).
//!
//! Every zombie shares one mesh and one material, so Bevy draws the whole horde as one instanced
//! batch. Animation happens entirely on the GPU: each clip's poses are baked into a float texture
//! (one column per vertex, one row per frame), and the vertex shader reads the right rows from the
//! per-instance [`MeshTag`] (clip, phase, outfit, speed, and the clip it's fading from) plus the
//! global time. Switching between looping clips (idle, walk, attack, thump) crossfades over
//! [`FADE_SECS`]; one-shot clips are authored to start and end near the looping poses instead.
//! Zombies the server no longer lists (corpses it cleared away) are despawned.
//!
//! The character is either a `.zvat` baked offline by `tools/blender/zvat.py` (a textured mesh and its
//! clips, see [`crate::zvat`]), or, when there is none, a grey-box low-poly zombie built and baked
//! here at startup. Which one: `--crowd-vat PATH`, else `ZOMBYE_CROWD_VAT`, else the first
//! `assets/models/characters/*.zvat` by name (where a promoted one goes); `none` (or a file that
//! won't load, with a warning) means the grey-box. A baked file's clips go into [`CLIPS`]' slots by name; the slots it
//! has no clip for get stand-ins made from what it has ([`Source`], [`sources`]), so every anim plays
//! something sensible and nothing T-poses.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshTag, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, TextureDimension,
    TextureFormat, VertexFormat,
};
use bevy::shader::ShaderRef;
use zombye_sim::zombies::GET_UP_SECS;
use zombye_sim::{CombatEventKind, ZombieAnim};

use crate::Options;
use crate::daynight::sky_at;
use crate::fog::FogState;
use crate::rig::{Bone, Pose, box_faces, rx, ry, rz, skin_matrices, smooth};
use crate::server_link::{CombatEventsThisFrame, ServerLink};
use crate::zvat::{self, Zvat};

const SHADER: &str = "shaders/zombie_vat.wesl";

/// x = vertex index into the VAT (its column), y = body-part kind (see [`Kind`]).
const ATTRIBUTE_VAT: MeshVertexAttribute = MeshVertexAttribute::new("ZombieVat", 7_301_552_011, VertexFormat::Float32x2);

pub struct ZombieRenderPlugin;

impl Plugin for ZombieRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<ZombieMaterial>::default())
            .add_systems(Startup, setup_zombie_assets)
            .add_systems(Update, (sync_zombies, light_zombies));
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct ZombieMaterial {
    #[texture(0, sample_type = "float", filterable = false)]
    pub vat: Handle<Image>,
    #[uniform(1)]
    pub settings: VatSettings,
    /// The baked character's base colour (1 × 1 white for the grey-box, which colours by [`Kind`]).
    #[texture(2)]
    #[sampler(3)]
    pub base_color: Handle<Image>,
}

#[derive(ShaderType, Debug, Clone)]
pub struct VatSettings {
    /// Per clip (indexed by [`Clip`]): x = first row, y = frame count, z = loops per second at
    /// fixed rate, w = stride in metres (0 = fixed rate; otherwise the loop rate is the zombie's
    /// speed / stride so feet don't slide; < 0 = a one-shot clip). Room for 16 (4 tag bits).
    pub clips: [Vec4; 16],
    /// xyz = direction toward the sun (or moon), w = its strength.
    pub sun: Vec4,
    /// rgb = sun colour, w unused.
    pub sun_color: Vec4,
    /// rgb = ambient colour × strength.
    pub ambient: Vec4,
}

impl Material for ZombieMaterial {
    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let vertex_layout =
            layout.0.get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0), ATTRIBUTE_VAT.at_shader_location(1), Mesh::ATTRIBUTE_UV_0.at_shader_location(2)])?;
        descriptor.vertex.buffers = vec![vertex_layout];
        Ok(())
    }
}

#[derive(Resource)]
struct ZombieAssets {
    mesh: Handle<Mesh>,
    material: Handle<ZombieMaterial>,
}

/// Sim zombie id → render entity.
#[derive(Resource, Default)]
struct ZombieEntities(HashMap<u32, Tracked>);

// ---------------------------------------------------------------------------------------------
// Procedural grey-box zombie: boxes attached to a 7-bone skeleton. Model faces +Z, feet at y = 0.

#[derive(Clone, Copy)]
enum Kind {
    Skin = 0,
    Shirt = 1,
    Pants = 2,
    Shoes = 3,
    /// A baked character's vertex: coloured by its texture, tinted per outfit.
    Textured = 4,
}

const PELVIS: usize = 0;
const TORSO: usize = 1;
const HEAD: usize = 2;
const ARM_L: usize = 3;
const ARM_R: usize = 4;
const LEG_L: usize = 5;
const LEG_R: usize = 6;

/// (parent, rest pivot in model space)
const BONES: [Bone; 7] = [
    (None, Vec3::new(0.0, 0.9, 0.0)),
    (Some(PELVIS), Vec3::new(0.0, 0.98, 0.0)),
    (Some(TORSO), Vec3::new(0.0, 1.47, 0.0)),
    (Some(TORSO), Vec3::new(0.25, 1.42, 0.0)),
    (Some(TORSO), Vec3::new(-0.25, 1.42, 0.0)),
    (Some(PELVIS), Vec3::new(0.1, 0.9, 0.0)),
    (Some(PELVIS), Vec3::new(-0.1, 0.9, 0.0)),
];

/// (bone, min corner, max corner, kind)
const PARTS: [(usize, Vec3, Vec3, Kind); 13] = [
    (PELVIS, Vec3::new(-0.17, 0.84, -0.1), Vec3::new(0.17, 1.0, 0.1), Kind::Pants),
    (TORSO, Vec3::new(-0.19, 0.98, -0.11), Vec3::new(0.19, 1.46, 0.11), Kind::Shirt),
    (HEAD, Vec3::new(-0.11, 1.48, -0.11), Vec3::new(0.11, 1.72, 0.13), Kind::Skin),
    (ARM_L, Vec3::new(0.19, 1.12, -0.06), Vec3::new(0.31, 1.45, 0.06), Kind::Shirt),
    (ARM_L, Vec3::new(0.2, 0.76, -0.05), Vec3::new(0.3, 1.12, 0.05), Kind::Skin),
    (ARM_R, Vec3::new(-0.31, 1.12, -0.06), Vec3::new(-0.19, 1.45, 0.06), Kind::Shirt),
    (ARM_R, Vec3::new(-0.3, 0.76, -0.05), Vec3::new(-0.2, 1.12, 0.05), Kind::Skin),
    (LEG_L, Vec3::new(0.02, 0.08, -0.08), Vec3::new(0.18, 0.9, 0.08), Kind::Pants),
    (LEG_L, Vec3::new(0.02, 0.0, -0.09), Vec3::new(0.18, 0.08, 0.15), Kind::Shoes),
    (LEG_R, Vec3::new(-0.18, 0.08, -0.08), Vec3::new(-0.02, 0.9, 0.08), Kind::Pants),
    (LEG_R, Vec3::new(-0.18, 0.0, -0.09), Vec3::new(-0.02, 0.08, 0.15), Kind::Shoes),
    // Jaw/neck stub so the head doesn't float.
    (TORSO, Vec3::new(-0.06, 1.44, -0.05), Vec3::new(0.06, 1.5, 0.05), Kind::Skin),
    // Belt line.
    (PELVIS, Vec3::new(-0.175, 0.97, -0.105), Vec3::new(0.175, 1.0, 0.105), Kind::Shoes),
];

type ZPose = Pose<7>;

/// Clips baked into the VAT. More than [`ZombieAnim`]s: a knockdown is a fall then a loop on the
/// ground, a death is one of three falls. The client picks the clip ([`Tracked::clip`]); the shader
/// only sees its index (4 bits of the tag).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Clip {
    Idle = 0,
    Walk = 1,
    Attack = 2,
    Thump = 3,
    Stagger = 4,
    Fall = 5,
    Down = 6,
    GetUp = 7,
    DieBack = 8,
    DieFront = 9,
    DieDown = 10,
}

/// Hip height at rest (the pelvis pivot).
const HIP: f32 = 0.9;

/// Root offset that tips the whole body by `theta` about X around its feet (negative = backward,
/// onto its back), resting `lift` metres higher once flat (the body's thickness).
fn tip(theta: f32, lift: f32) -> Vec3 {
    let hip = rx(theta) * Vec3::new(0.0, HIP, 0.0);
    hip + Vec3::Y * (lift * (theta.abs() / FRAC_PI_2).min(1.0) - HIP)
}

/// Arms half-raised, a little hunched: where one-shot clips start and end so switching to and from
/// the looping clips doesn't jump far.
fn reach() -> ZPose {
    let mut p = ZPose::default();
    p.rot[TORSO] = rx(0.2);
    p.rot[HEAD] = rx(0.2) * rz(0.1);
    p.rot[ARM_L] = rx(-0.9) * rz(0.05);
    p.rot[ARM_R] = rx(-0.8) * rz(-0.05);
    p.offset.y = -0.02;
    p
}

/// Lying on its back, limp: the end of the falls that land face up.
fn limp_on_back() -> ZPose {
    let mut p = ZPose::default();
    p.rot[PELVIS] = rx(-FRAC_PI_2);
    p.rot[HEAD] = ry(0.6) * rx(0.15);
    p.rot[ARM_L] = rz(1.2) * rx(-0.2);
    p.rot[ARM_R] = rz(-1.0) * rx(-0.15);
    p.rot[LEG_L] = rz(0.16);
    p.rot[LEG_R] = rz(-0.12);
    p.offset = tip(-FRAC_PI_2, 0.12);
    p
}

/// The pose of `clip` at normalised time `t` (looping clips: [0, 1); one-shots: [0, 1], ending on
/// their final pose).
fn pose(clip: Clip, t: f32) -> ZPose {
    let ph = t * TAU;
    let s = ph.sin();
    let mut p = ZPose::default();
    // Positive X rotation swings a hanging limb backward and tips the torso forward.
    match clip {
        Clip::Idle => {
            p.rot[TORSO] = rx(0.18 + s * 0.03) * rz(0.05);
            p.rot[HEAD] = rz(s * 0.18) * rx(0.25);
            p.rot[ARM_L] = rx(-0.35 + (ph + 0.6).sin() * 0.06) * rz(0.08);
            p.rot[ARM_R] = rx(-0.25 + s * 0.05) * rz(-0.06);
            p.rot[LEG_L] = rx(-0.05);
            p.rot[LEG_R] = rx(0.08);
            p.offset.y = -0.02 + s * 0.008;
        }
        Clip::Walk => {
            // Shamble: arms reaching forward, one leg dragging.
            p.rot[LEG_L] = rx(s * 0.5);
            p.rot[LEG_R] = rx(-s * 0.32);
            p.rot[PELVIS] = rz(s * 0.07) * ry(s * 0.08);
            p.rot[TORSO] = rx(0.22) * rz(-s * 0.06);
            p.rot[HEAD] = rz(0.15 + s * 0.08) * rx(0.2);
            p.rot[ARM_L] = rx(-1.3 + (ph + 0.5).sin() * 0.12) * rz(-0.05);
            p.rot[ARM_R] = rx(-1.15 + (ph + 2.0).sin() * 0.12) * rz(0.08);
            p.offset.y = -0.03 - s.abs() * 0.035;
        }
        Clip::Attack => {
            // Clawing lunge.
            let c = (ph * 2.0).sin();
            p.rot[TORSO] = rx(0.4 + s * 0.12);
            p.rot[HEAD] = rx(0.1 + c * 0.1);
            p.rot[ARM_L] = rx(-1.55 + c * 0.45) * rz(-0.15);
            p.rot[ARM_R] = rx(-1.55 - c * 0.45) * rz(0.15);
            p.rot[LEG_L] = rx(-0.25);
            p.rot[LEG_R] = rx(0.3);
            p.offset = Vec3::new(0.0, -0.06, 0.04 + s * 0.04);
        }
        Clip::Thump => {
            // Hammering a door with both fists in turn: raised high, brought down hard.
            let raise = |phase: f32| (0.5 + 0.5 * phase.cos()).powf(0.6);
            let (l, r) = (raise(ph), raise(ph + PI));
            p.rot[TORSO] = rx(0.12 + 0.18 * (1.0 - l.max(r))) * ry(0.12 * (l - r));
            p.rot[HEAD] = rx(0.05);
            p.rot[ARM_L] = rx(-1.3 - 1.25 * l) * rz(-0.15);
            p.rot[ARM_R] = rx(-1.3 - 1.25 * r) * rz(0.15);
            p.rot[LEG_L] = rx(-0.15);
            p.rot[LEG_R] = rx(0.2);
            p.offset = Vec3::new(0.0, -0.03, 0.03);
        }
        Clip::Stagger => {
            // Reeling back from a hit: thrown back fast, then recovering.
            let k = if t < 0.18 { smooth(0.0, 0.18, t) } else { 1.0 - smooth(0.18, 1.0, t) };
            let mut peak = ZPose::default();
            peak.rot[TORSO] = rx(-0.4) * rz(0.14);
            peak.rot[HEAD] = rx(-0.55) * rz(-0.2);
            peak.rot[ARM_L] = rx(-1.0) * rz(0.8);
            peak.rot[ARM_R] = rx(-0.3) * rz(-0.9);
            peak.rot[LEG_L] = rx(0.4);
            peak.rot[LEG_R] = rx(-0.15);
            peak.offset = Vec3::new(0.0, -0.06, -0.14);
            p = reach().blend(&peak, k);
        }
        Clip::Fall => {
            // Knocked off its feet onto its back, arms flying up.
            let k = t * t;
            let mut end = pose(Clip::Down, 0.0);
            end.rot[PELVIS] = rx(-FRAC_PI_2);
            p = reach().blend(&end, smooth(0.0, 1.0, t));
            let theta = -FRAC_PI_2 * k;
            p.rot[PELVIS] = rx(theta);
            p.offset = tip(theta, 0.12);
            p.rot[ARM_L] = p.rot[ARM_L] * rx(-0.6 * (t * PI).sin());
            p.rot[ARM_R] = p.rot[ARM_R] * rx(-0.5 * (t * PI).sin());
        }
        Clip::Down => {
            // On its back, clawing at the air and twitching (gets up again).
            p.rot[PELVIS] = rx(-FRAC_PI_2) * ry(s * 0.08);
            p.rot[HEAD] = ry(s * 0.4) * rx(-0.2);
            p.rot[ARM_L] = rx(-1.5 + s * 0.35) * rz(0.2);
            p.rot[ARM_R] = rx(-1.4 - s * 0.35) * rz(-0.2);
            p.rot[LEG_L] = rx(-0.25 + s * 0.12);
            p.rot[LEG_R] = rx(-0.1 - s * 0.12);
            p.offset = tip(-FRAC_PI_2, 0.12);
        }
        Clip::GetUp => {
            // Sits up, then rises stiffly over its feet.
            let a = smooth(0.0, 0.4, t);
            let b = smooth(0.3, 1.0, t);
            let theta = -FRAC_PI_2 * (1.0 - b);
            let idle = pose(Clip::Idle, 0.0);
            let down = pose(Clip::Down, 0.0);
            p.rot[PELVIS] = rx(theta);
            p.offset = tip(theta, 0.12).lerp(idle.offset, b);
            p.rot[TORSO] = rx(1.3 * a * (1.0 - b)).slerp(idle.rot[TORSO], b);
            p.rot[HEAD] = down.rot[HEAD].slerp(rx(0.35), a).slerp(idle.rot[HEAD], b);
            p.rot[ARM_L] = down.rot[ARM_L].slerp(rx(-0.4) * rz(0.35), a).slerp(idle.rot[ARM_L], b);
            p.rot[ARM_R] = down.rot[ARM_R].slerp(rx(-0.3) * rz(-0.35), a).slerp(idle.rot[ARM_R], b);
            p.rot[LEG_L] = down.rot[LEG_L].slerp(idle.rot[LEG_L], a);
            p.rot[LEG_R] = down.rot[LEG_R].slerp(idle.rot[LEG_R], a);
        }
        Clip::DieBack => {
            // Killed from the front: topples backward and lies still.
            let end = limp_on_back();
            p = reach().blend(&end, smooth(0.0, 0.9, t));
            let theta = -FRAC_PI_2 * (t * t).min(1.0);
            p.rot[PELVIS] = rx(theta);
            p.offset = tip(theta, 0.12);
            p.rot[ARM_L] = p.rot[ARM_L] * rx(-0.9 * (t * PI).sin());
            p.rot[ARM_R] = p.rot[ARM_R] * rx(-0.7 * (t * PI).sin());
        }
        Clip::DieFront => {
            // Killed from behind: pitches forward onto its face, arms flung out.
            let mut end = ZPose::default();
            end.rot[PELVIS] = rx(FRAC_PI_2);
            end.rot[HEAD] = ry(1.1);
            end.rot[ARM_L] = rz(1.25) * rx(0.2);
            end.rot[ARM_R] = rz(-1.1) * rx(0.4);
            end.rot[LEG_L] = rz(0.18);
            end.rot[LEG_R] = rz(-0.1) * rx(0.15);
            p = reach().blend(&end, smooth(0.1, 1.0, t));
            let theta = FRAC_PI_2 * t.powf(2.2);
            p.rot[PELVIS] = rx(theta);
            p.offset = tip(theta, 0.13);
        }
        Clip::DieDown => {
            // Finished off on the ground: the arms drop.
            p = pose(Clip::Down, 0.0).blend(&limp_on_back(), smooth(0.0, 1.0, t));
        }
    }
    p
}

/// Clip layout in the VAT: (clip, frames, loops/s at fixed rate, stride metres or 0, one-shot
/// length in seconds or 0 = looping). Looping clips with a stride play at the zombie's speed /
/// stride, so feet don't slide. One-shots are timed by the client from when the anim changed.
const CLIPS: [(Clip, u32, f32, f32, f32); 11] = [
    (Clip::Idle, 24, 0.35, 0.0, 0.0),
    (Clip::Walk, 24, 0.0, 1.1, 0.0),
    (Clip::Attack, 16, 1.3, 0.0, 0.0),
    (Clip::Thump, 16, 0.65, 0.0, 0.0),
    (Clip::Stagger, 16, 0.0, 0.0, STAGGER_SECS),
    (Clip::Fall, 12, 0.0, 0.0, FALL_SECS),
    (Clip::Down, 24, DOWN_RATE, 0.0, 0.0),
    (Clip::GetUp, 24, 0.0, 0.0, GET_UP_SECS),
    (Clip::DieBack, 14, 0.0, 0.0, 0.75),
    (Clip::DieFront, 14, 0.0, 0.0, 0.85),
    (Clip::DieDown, 8, 0.0, 0.0, 0.4),
];

const STAGGER_SECS: f32 = 0.8;
const FALL_SECS: f32 = 0.5;
const DOWN_RATE: f32 = 0.6;

fn clip_secs(clip: Clip) -> f32 {
    CLIPS[clip as usize].4
}

/// Corners of the box every pose of the grey-box fits in (lying down reaches well past the standing
/// body, the thump raises the arms high), added to the mesh as two zero-area triangles so frustum
/// culling uses it. A baked character's box is worked out from its frames instead ([`Baked::bounds`]).
const POSE_BOUNDS: [Vec3; 2] = [Vec3::new(-1.2, -0.2, -2.0), Vec3::new(1.2, 2.6, 2.0)];

/// The largest VAT side: one column per vertex, one row per frame. 8,192 is what every GPU wgpu
/// runs on allows.
const MAX_VAT_SIDE: usize = 8192;

/// A character baked into a VAT: per row (frame), one position per column (vertex).
struct Baked {
    mesh: Mesh,
    texels: Vec<Vec3>,
    width: u32,
    rows: u32,
    clips: [Vec4; 16],
    /// The box every frame fits in (the mesh's culling bounds).
    #[cfg_attr(not(test), allow(dead_code))]
    bounds: [Vec3; 2],
}

impl Baked {
    /// The VAT as RGBA16F (8 bytes a texel, half of RGBA32F; about a millimetre of precision at
    /// 2 m, far below a pixel).
    fn image(&self) -> Image {
        let bytes: Vec<u8> =
            self.texels.iter().flat_map(|p| [p.x, p.y, p.z, 1.0]).flat_map(|f| half::f16::from_f32(f).to_le_bytes()).collect();
        Image::new(
            Extent3d { width: self.width, height: self.rows, depth_or_array_layers: 1 },
            TextureDimension::D2,
            bytes,
            TextureFormat::Rgba16Float,
            RenderAssetUsages::RENDER_WORLD,
        )
    }

    /// One row's positions.
    #[cfg(test)]
    fn row(&self, row: u32) -> &[Vec3] {
        let w = self.width as usize;
        &self.texels[row as usize * w..(row as usize + 1) * w]
    }
}

/// Adds the two bounds corners as degenerate triangles (all three corners at one point: never
/// drawn, but they widen the mesh's bounding box to cover every pose) and builds the mesh.
fn finish_mesh(mut positions: Vec<[f32; 3]>, mut vat: Vec<[f32; 2]>, mut uvs: Vec<[f32; 2]>, mut indices: Vec<u32>, bounds: [Vec3; 2]) -> Mesh {
    for corner in bounds {
        let base = positions.len() as u32;
        for _ in 0..3 {
            vat.push([positions.len() as f32, Kind::Skin as u32 as f32]);
            positions.push(corner.to_array());
            uvs.push([0.0, 0.0]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(ATTRIBUTE_VAT, vat)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices))
}

fn build_zombie() -> Baked {
    // 24 vertices per box (flat-shaded faces), with the bone each vertex follows.
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut vat: Vec<[f32; 2]> = Vec::new();
    let mut bones: Vec<usize> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for (bone, min, max, kind) in PARTS {
        for (quad, _) in box_faces(min, max) {
            let base = positions.len() as u32;
            for v in quad {
                vat.push([positions.len() as f32, kind as u32 as f32]);
                positions.push(v.to_array());
                bones.push(bone);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    let uvs = vec![[0.0, 0.0]; positions.len()];

    // Bake every frame of every clip into rows of the VAT; the bounds corners (the last 6 columns)
    // stay put.
    let width = positions.len() as u32 + 6;
    let rows: u32 = CLIPS.iter().map(|c| c.1).sum();
    let mut texels: Vec<Vec3> = Vec::with_capacity((width * rows) as usize);
    let mut clips = [Vec4::ZERO; 16];
    let mut row = 0u32;
    for (clip, frames, rate, stride, secs) in CLIPS {
        // w < 0 marks a one-shot clip (its last frame is its final pose, no wrapping).
        clips[clip as usize] = Vec4::new(row as f32, frames as f32, rate, if secs > 0.0 { -1.0 } else { stride });
        for f in 0..frames {
            let t = if secs > 0.0 { f as f32 / (frames - 1).max(1) as f32 } else { f as f32 / frames as f32 };
            let m = skin_matrices(&BONES, &pose(clip, t));
            texels.extend(positions.iter().enumerate().map(|(i, p)| m[bones[i]].transform_point3(Vec3::from_array(*p))));
            texels.extend(POSE_BOUNDS.iter().flat_map(|&c| [c; 3]));
        }
        row += frames;
    }
    let mesh = finish_mesh(positions, vat, uvs, indices, POSE_BOUNDS);
    Baked { mesh, texels, width, rows, clips, bounds: POSE_BOUNDS }
}

// ---------------------------------------------------------------------------------------------
// A character baked offline (`.zvat`).

/// The name a `.zvat` clip needs to fill each slot (compared as [`zvat::clip_key`]: case, `_` and
/// `-` don't matter, so `get_up` and `DieBack` work too).
fn clip_name(clip: Clip) -> &'static str {
    match clip {
        Clip::Idle => "idle",
        Clip::Walk => "walk",
        Clip::Attack => "attack",
        Clip::Thump => "thump",
        Clip::Stagger => "stagger",
        Clip::Fall => "fall",
        Clip::Down => "down",
        Clip::GetUp => "getup",
        Clip::DieBack => "dieback",
        Clip::DieFront => "diefront",
        Clip::DieDown => "diedown",
    }
}

/// Where each clip slot of a baked character comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source {
    /// The file's clip of that name (by index in the file).
    File(usize),
    /// Idle and walk stand in for each other: that file clip at its own fixed rate.
    Other(usize),
    /// Attack and thump: that looping file clip (the idle, else the walk) with the whole body leaned
    /// forward by this many radians, at the clip's own rate.
    Leaned(usize, f32),
    /// The one-shots and the ground loop: the base pose (the idle's first frame, else the walk's)
    /// moved as one piece by the grey-box clip's root motion ([`rigid_motion`]: toppling onto its
    /// back or face, getting up, reeling), over the grey-box clip's frames and timing, its lowest
    /// point kept where the base pose's is (on the ground).
    Rigid,
}

/// How far attack and thump stand-ins lean in.
const ATTACK_LEAN: f32 = 0.3;
const THUMP_LEAN: f32 = 0.12;

/// The file clip other clips are made from: the idle, else the walk, else the first.
fn base_clip(z: &Zvat) -> usize {
    z.clip("idle").or(z.clip("walk")).unwrap_or(0)
}

/// The source of every slot for `z`, by [`Clip`] index.
fn sources(z: &Zvat) -> [Source; 11] {
    let base = base_clip(z);
    CLIPS.map(|(clip, ..)| match (z.clip(clip_name(clip)), clip) {
        (Some(i), _) => Source::File(i),
        (None, Clip::Idle | Clip::Walk) => Source::Other(base),
        (None, Clip::Attack) => Source::Leaned(base, ATTACK_LEAN),
        (None, Clip::Thump) => Source::Leaned(base, THUMP_LEAN),
        (None, _) => Source::Rigid,
    })
}

/// The whole-body motion a [`Source::Rigid`] stand-in follows at `t`: the grey-box clip's root
/// (pelvis) transform, which tips the body about its feet; a stagger, which only shifts the
/// grey-box's root, also reels back.
fn rigid_motion(clip: Clip, t: f32) -> Mat4 {
    if clip == Clip::Stagger {
        let k = if t < 0.18 { smooth(0.0, 0.18, t) } else { 1.0 - smooth(0.18, 1.0, t) };
        return Mat4::from_translation(Vec3::new(0.0, 0.0, -0.14 * k)) * Mat4::from_quat(rx(-0.25 * k));
    }
    skin_matrices(&BONES, &pose(clip, t))[PELVIS]
}

/// `frame` moved by `m`, then raised or lowered so its lowest point is at `ground`.
fn moved(frame: &[Vec3], m: Mat4, ground: f32) -> Vec<Vec3> {
    let out: Vec<Vec3> = frame.iter().map(|p| m.transform_point3(*p)).collect();
    let dy = ground - lowest(&out);
    out.into_iter().map(|p| p + Vec3::Y * dy).collect()
}

fn lowest(frame: &[Vec3]) -> f32 {
    frame.iter().map(|p| p.y).fold(f32::MAX, f32::min)
}

/// Bakes a `.zvat`'s clips (and stand-ins for the slots it has no clip for) into a VAT.
fn build_baked(z: &Zvat) -> Result<Baked, String> {
    let v = z.vertices();
    if v + 6 > MAX_VAT_SIDE {
        return Err(format!("{v} vertices: the VAT has a column per vertex, at most {}", MAX_VAT_SIDE - 6));
    }
    let src = sources(z);
    let base = z.frame(base_clip(z), 0);
    let ground = lowest(base);

    let mut frames_out: Vec<Vec3> = Vec::new();
    let mut clips = [Vec4::ZERO; 16];
    // Where each file clip's frames went, so a clip used twice is stored once.
    let mut file_rows: Vec<Option<u32>> = vec![None; z.clips.len()];
    let mut row = 0u32;
    for (slot, (clip, frames, _, _, secs)) in CLIPS.into_iter().enumerate() {
        let one_shot = secs > 0.0;
        let first = row;
        clips[clip as usize] = match src[slot] {
            Source::File(i) | Source::Other(i) => {
                let c = &z.clips[i];
                let first = *file_rows[i].get_or_insert_with(|| {
                    (0..c.frames).for_each(|f| frames_out.extend_from_slice(z.frame(i, f)));
                    row += c.frames;
                    first
                });
                let own = matches!(src[slot], Source::File(_));
                let w = if one_shot { -1.0 } else if own { c.stride } else { 0.0 };
                let rate = if clip == Clip::Down { DOWN_RATE } else if w != 0.0 { 0.0 } else { 1.0 / c.secs };
                Vec4::new(first as f32, c.frames as f32, rate, w)
            }
            Source::Leaned(i, lean) => {
                let c = &z.clips[i];
                let m = Mat4::from_quat(rx(lean));
                for f in 0..c.frames {
                    let frame = z.frame(i, f);
                    frames_out.extend(moved(frame, m, lowest(frame)));
                }
                row += c.frames;
                Vec4::new(first as f32, c.frames as f32, 1.0 / c.secs, 0.0)
            }
            Source::Rigid => {
                for f in 0..frames {
                    let t = if one_shot { f as f32 / (frames - 1).max(1) as f32 } else { f as f32 / frames as f32 };
                    frames_out.extend(moved(base, rigid_motion(clip, t), ground));
                }
                row += frames;
                let rate = if clip == Clip::Down { DOWN_RATE } else { 0.0 };
                Vec4::new(first as f32, frames as f32, rate, if one_shot { -1.0 } else { 0.0 })
            }
        };
    }
    if row as usize > MAX_VAT_SIDE {
        return Err(format!("{row} frames in all: the VAT has a row per frame, at most {MAX_VAT_SIDE}"));
    }

    // The box every frame fits in, a little padded, as the last 6 columns of every row.
    let (lo, hi) = frames_out.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let bounds = [lo - Vec3::splat(0.05), hi + Vec3::splat(0.05)];
    let width = v as u32 + 6;
    let mut texels = Vec::with_capacity(width as usize * row as usize);
    for r in frames_out.chunks_exact(v) {
        texels.extend_from_slice(r);
        texels.extend(bounds.iter().flat_map(|&c| [c; 3]));
    }

    let positions: Vec<[f32; 3]> = base.iter().map(|p| p.to_array()).collect();
    let vat: Vec<[f32; 2]> = (0..v).map(|i| [i as f32, Kind::Textured as u32 as f32]).collect();
    let mesh = finish_mesh(positions, vat, z.uvs.clone(), z.indices.clone(), bounds);
    Ok(Baked { mesh, texels, width, rows: row, clips, bounds })
}

/// The `.zvat` to draw the crowd with, if any: `--crowd-vat PATH`, else `ZOMBYE_CROWD_VAT`, else
/// the first `.zvat` by name in `<data>/../models/characters` (assets/models/characters, where a
/// promoted crowd character goes as `<id>.zvat`). `none` (or an empty value) asks for the grey-box.
fn crowd_vat_path(options: &Options) -> Option<PathBuf> {
    let asked = options.crowd_vat.clone().or_else(|| std::env::var("ZOMBYE_CROWD_VAT").ok());
    match asked {
        Some(p) if p.is_empty() || p.eq_ignore_ascii_case("none") => None,
        Some(p) => Some(p.into()),
        None => first_zvat(&options.data.join("../models/characters")),
    }
}

/// The first `*.zvat` in `dir` by file name.
fn first_zvat(dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> =
        std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "zvat")).collect();
    found.sort();
    found.into_iter().next()
}

/// A baked crowd character, its texture (if one was found) and a line saying what was loaded, or
/// why it couldn't be.
fn load_baked(path: &Path) -> Result<(Baked, Option<Image>, String), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let z = Zvat::parse(&bytes)?;
    let baked = build_baked(&z)?;
    let (texture, note) = match zvat::find_texture(path) {
        Some((from, bytes, ext)) => match zvat::texture_image(&bytes, ext) {
            Ok(image) => (Some(image), format!("texture from {}", from.display())),
            Err(e) => (None, format!("texture {} unreadable ({e}): untextured", from.display())),
        },
        None => (None, "no texture found: untextured".into()),
    };
    let from_file: Vec<&str> =
        CLIPS.iter().zip(sources(&z)).filter(|(_, s)| matches!(s, Source::File(_))).map(|((c, ..), _)| clip_name(*c)).collect();
    let summary = format!(
        "crowd character {}: {} vertices, {} triangles, {} VAT rows ({:.1} MB); clips from the file: {}; stand-ins for the rest; {note}",
        path.display(),
        z.vertices(),
        z.indices.len() / 3,
        baked.rows,
        (baked.width as usize * baked.rows as usize * 8) as f32 / (1 << 20) as f32,
        from_file.join(", "),
    );
    Ok((baked, texture, summary))
}

fn setup_zombie_assets(
    mut commands: Commands,
    options: Res<Options>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ZombieMaterial>>,
) {
    let loaded = crowd_vat_path(&options).and_then(|path| match load_baked(&path) {
        Ok(l) => Some(l),
        Err(e) => {
            warn!("crowd character {} not loaded ({e}): drawing the grey-box", path.display());
            None
        }
    });
    let (baked, texture) = match loaded {
        Some((baked, texture, summary)) => {
            info!("{summary}");
            // Untextured: a plain zombie grey-green, still tinted per outfit.
            (baked, texture.unwrap_or_else(|| zvat::mipmapped(1, 1, &[96, 104, 84, 255])))
        }
        None => (build_zombie(), zvat::mipmapped(1, 1, &[255, 255, 255, 255])),
    };
    let material = materials.add(ZombieMaterial {
        vat: images.add(baked.image()),
        settings: VatSettings { clips: baked.clips, sun: Vec4::new(0.4, 0.8, 0.3, 1.0), sun_color: Vec4::ONE, ambient: Vec4::splat(0.5) },
        base_color: images.add(texture),
    });
    commands.insert_resource(ZombieAssets { mesh: meshes.add(baked.mesh), material });
    commands.init_resource::<ZombieEntities>();
}

/// Looping clips played on the shader's own clock, which can crossfade into each other. They are
/// the first four clips, so the clip faded from fits in 2 tag bits.
fn is_loop(clip: Clip) -> bool {
    (clip as u8) < 4
}

/// How long a switch between looping clips crossfades.
const FADE_SECS: f32 = 0.2;

/// Pack per-instance data for the shader. Must match `vertex` in zombie_vat.wesl:
/// bits 0-3 clip, 4-9 outfit, 10 one-shot/direct, 27 hit flash; then either (looping clips) 11-16
/// phase, 17-24 speed (×1/48 m/s), 25-26 the looping clip it's fading from and 28-31 how much of
/// that clip is still showing (0 = none, 15 = all), or (direct) 11-26 the clip position in 0..1.
fn pack_tag(id: u32, clip: Clip, position: Option<f32>, speed: f32, flash: bool, fade: Option<(Clip, f32)>) -> u32 {
    let h = id.wrapping_mul(0x9E37_79B9);
    let outfit = (h >> 20) & 63;
    let mut tag = (clip as u32 & 15) | (outfit << 4) | ((flash as u32) << 27);
    match position {
        Some(p) => tag |= (1 << 10) | (((p.clamp(0.0, 1.0) * 65535.0).round() as u32) << 11),
        None => {
            let phase = (h >> 8) & 63;
            let speed = ((speed * 48.0).round() as u32).min(255);
            tag |= (phase << 11) | (speed << 17);
            if let Some((from, w)) = fade.filter(|(from, _)| is_loop(*from) && *from != clip) {
                let w = ((w.clamp(0.0, 1.0) * 15.0).round() as u32).min(15);
                tag |= ((from as u32 & 3) << 25) | (w << 28);
            }
        }
    }
    tag
}

/// Render-side state of one zombie: what it's playing since when, and recent blows.
struct Tracked {
    entity: Entity,
    anim: ZombieAnim,
    /// When `anim` started (seconds, `Time` clock).
    since: f32,
    /// The looping clip it was playing before `anim`, if it's fading from one.
    fade_from: Option<Clip>,
    /// The snapshot tick it was last listed in.
    seen: u64,
    /// Which fall a death plays (picked when it dies).
    death: Clip,
    flash_until: f32,
    /// Direction and time of the last blow, for the kickback and which way it falls.
    blow: Vec2,
    blow_at: f32,
}

/// How long a hit zombie flashes, and how far and for how long it's knocked back on screen.
const FLASH_SECS: f32 = 0.09;
const KICK_M: f32 = 0.16;
const KICK_SECS: f32 = 0.22;

impl Tracked {
    /// The clip to play now, and its position for one-shots (and the ground loop, which must start
    /// where the fall ended).
    fn clip(&self, now: f32) -> (Clip, Option<f32>) {
        let e = (now - self.since).max(0.0);
        let once = |c: Clip| (c, Some(e / clip_secs(c)));
        match self.anim {
            ZombieAnim::Idle => (Clip::Idle, None),
            ZombieAnim::Walk => (Clip::Walk, None),
            ZombieAnim::Attack => (Clip::Attack, None),
            ZombieAnim::Thump => (Clip::Thump, None),
            ZombieAnim::Stagger => once(Clip::Stagger),
            ZombieAnim::Down if e < FALL_SECS => once(Clip::Fall),
            ZombieAnim::Down => (Clip::Down, Some(((e - FALL_SECS) * DOWN_RATE).fract())),
            ZombieAnim::GetUp => once(Clip::GetUp),
            ZombieAnim::Dead => once(self.death),
        }
    }

    /// The looping clip it's fading from and how much of it still shows, while a crossfade lasts.
    fn fade(&self, now: f32) -> Option<(Clip, f32)> {
        let e = now - self.since;
        self.fade_from.filter(|_| e < FADE_SECS).map(|c| (c, 1.0 - e / FADE_SECS))
    }

    /// Switch to a new anim from the snapshot.
    fn set_anim(&mut self, anim: ZombieAnim, facing: f32, now: f32) {
        // Fade from one looping clip into another (from the one mostly showing, if mid-fade).
        let (old, _) = self.clip(now);
        let showing = match self.fade(now) {
            Some((from, w)) if w > 0.5 => from,
            _ => old,
        };
        if anim == ZombieAnim::Dead {
            let from_behind = Vec2::from_angle(facing).dot(self.blow) > 0.3 && now - self.blow_at < 0.5;
            self.death = match self.anim {
                ZombieAnim::Down | ZombieAnim::GetUp => Clip::DieDown,
                _ if from_behind => Clip::DieFront,
                _ => Clip::DieBack,
            };
        }
        self.anim = anim;
        self.since = now;
        let (new, _) = self.clip(now);
        self.fade_from = (is_loop(showing) && is_loop(new) && showing != new).then_some(showing);
    }
}

/// Ids tracked but not in the snapshot of tick `tick` (cleared away by the server).
fn stale(entities: &HashMap<u32, Tracked>, tick: u64) -> Vec<u32> {
    entities.iter().filter(|(_, t)| t.seen != tick).map(|(id, _)| *id).collect()
}

/// Mirror the latest snapshots into render entities, interpolating positions between sim ticks.
/// Zombies you can't see (fog of war) aren't drawn. Blows from this frame's combat events make the
/// zombie flash and jolt.
#[allow(clippy::too_many_arguments)]
fn sync_zombies(
    mut commands: Commands,
    link: Res<ServerLink>,
    fog: Res<FogState>,
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    events: Res<CombatEventsThisFrame>,
    assets: Option<Res<ZombieAssets>>,
    entities: Option<ResMut<ZombieEntities>>,
    mut query: Query<(&mut Transform, &mut MeshTag, &mut Visibility)>,
) {
    let (Some(assets), Some(mut entities)) = (assets, entities) else { return };
    let now = time.elapsed_secs();
    for ev in &events.0 {
        if ev.kind == CombatEventKind::PlayerHurt {
            continue;
        }
        if let Some(t) = entities.0.get_mut(&ev.zombie) {
            t.flash_until = now + FLASH_SECS;
            t.blow = ev.dir;
            t.blow_at = now;
        }
    }
    let alpha = fixed.overstep_fraction();
    let tick = link.curr.tick;
    let prev_list = &link.prev.zombies;
    for (i, z) in link.curr.zombies.iter().enumerate() {
        // Both lists are in id order; usually at the same index, unless corpses were cleared away.
        let prev = prev_list.get(i).filter(|p| p.id == z.id).or_else(|| prev_list.binary_search_by_key(&z.id, |p| p.id).ok().map(|k| &prev_list[k])).unwrap_or(z);
        let mut pos = Vec2::from(prev.pos).lerp(Vec2::from(z.pos), alpha);
        let facing = lerp_angle(prev.facing, z.facing, alpha);
        let visibility = if fog.visibility(pos) > 0.35 { Visibility::Inherited } else { Visibility::Hidden };
        // Sim angle 0 = +x, counter-clockwise toward +y (= Bevy +z). The mesh faces +Z.
        let rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - facing);

        let Some(t) = entities.0.get_mut(&z.id) else {
            // First sight. A corpse is already lying still.
            let since = if z.anim == ZombieAnim::Dead { -100.0 } else { now };
            let mut t = Tracked { entity: Entity::PLACEHOLDER, anim: z.anim, since, fade_from: None, seen: tick, death: Clip::DieBack, flash_until: 0.0, blow: Vec2::ZERO, blow_at: -100.0 };
            let (clip, position) = t.clip(now);
            let tag = pack_tag(z.id, clip, position, z.speed, false, None);
            let transform = Transform { translation: Vec3::new(pos.x, 0.0, pos.y), rotation, scale: Vec3::ONE };
            t.entity = commands
                .spawn((
                    Mesh3d(assets.mesh.clone()),
                    MeshMaterial3d(assets.material.clone()),
                    MeshTag::new(tag),
                    transform,
                    visibility,
                    // The default shadow pass would draw the rest pose; animated shadows come later.
                    NotShadowCaster,
                ))
                .id();
            entities.0.insert(z.id, t);
            continue;
        };
        t.seen = tick;
        if z.anim != t.anim {
            t.set_anim(z.anim, z.facing, now);
        }
        let kick = now - t.blow_at;
        if kick < KICK_SECS {
            let k = 1.0 - kick / KICK_SECS;
            pos += t.blow * KICK_M * k * k;
        }
        let (clip, position) = t.clip(now);
        let tag = pack_tag(z.id, clip, position, z.speed, now < t.flash_until, t.fade(now));
        if let Ok((mut tf, mut mt, mut vis)) = query.get_mut(t.entity) {
            *tf = Transform { translation: Vec3::new(pos.x, 0.0, pos.y), rotation, scale: Vec3::ONE };
            if mt.value != tag {
                mt.value = tag;
            }
            if *vis != visibility {
                *vis = visibility;
            }
        }
    }
    // Corpses the server cleared away (ids are never reused, so only removals shrink the list).
    if entities.0.len() > link.curr.zombies.len() {
        for id in stale(&entities.0, tick) {
            if let Some(t) = entities.0.remove(&id) {
                commands.entity(t.entity).despawn();
            }
        }
    }
}

/// Keep the zombie shader's simple lighting in step with the day/night cycle.
fn light_zombies(link: Res<ServerLink>, assets: Option<Res<ZombieAssets>>, mut materials: ResMut<Assets<ZombieMaterial>>) {
    let Some(assets) = assets else { return };
    let (dir, color, lux, day) = sky_at(link.curr.time_of_day);
    let Some(mut m) = materials.get_mut(&assets.material) else { return };
    let c = color.to_linear();
    m.settings.sun = (-dir).extend((lux / 10_000.0).max(0.08));
    m.settings.sun_color = Vec4::new(c.red, c.green, c.blue, 1.0);
    let night = Vec3::new(0.12, 0.15, 0.26);
    let noon = Vec3::new(0.55, 0.58, 0.65);
    m.settings.ambient = night.lerp(noon, day).extend(1.0);
}

pub(crate) fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let mut d = (b - a) % TAU;
    if d > std::f32::consts::PI {
        d -= TAU;
    } else if d < -std::f32::consts::PI {
        d += TAU;
    }
    a + d * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_clip_is_baked_and_fits_the_tag() {
        let baked = build_zombie();
        let (rows, clips) = (baked.rows, baked.clips);
        assert_eq!(baked.image().height(), rows);
        for (clip, frames, _, _, secs) in CLIPS {
            let c = clips[clip as usize];
            assert!(c.y == frames as f32 && c.x + c.y <= rows as f32, "{clip:?}: {c}");
            assert_eq!(c.w < 0.0, secs > 0.0);
            let tag = pack_tag(0xDEAD_BEEF, clip, None, 3.0, true, Some((Clip::Walk, 0.5)));
            assert_eq!(tag & 15, clip as u32);
            assert_eq!((tag >> 17) & 255, 144, "speed survives");
            assert_eq!((tag >> 27) & 1, 1, "flash survives");
            if clip != Clip::Walk {
                assert_eq!(((tag >> 25) & 3, tag >> 28), (Clip::Walk as u32, 8), "fade survives");
            } else {
                assert_eq!(tag >> 28, 0, "no fade into the same clip");
            }
            let tag = pack_tag(7, clip, Some(0.5), 3.0, false, Some((Clip::Walk, 0.5)));
            assert_eq!((tag >> 10) & 1, 1);
            assert_eq!((tag >> 11) & 0xFFFF, 32768);
            assert_eq!(tag >> 27, 0, "one-shots don't fade");
        }
        // The looping clips are the ones the shader can fade between (2 bits).
        for (clip, _, _, _, secs) in CLIPS {
            assert_eq!(is_loop(clip), secs == 0.0 && clip != Clip::Down, "{clip:?}");
        }
    }

    /// Switching between looping clips fades from the old one over FADE_SECS; into or out of a
    /// one-shot it doesn't.
    #[test]
    fn looping_clips_crossfade() {
        let mut t = Tracked { entity: Entity::PLACEHOLDER, anim: ZombieAnim::Walk, since: 0.0, fade_from: None, seen: 0, death: Clip::DieBack, flash_until: 0.0, blow: Vec2::ZERO, blow_at: -1.0 };
        t.set_anim(ZombieAnim::Idle, 0.0, 10.0);
        assert_eq!(t.fade(10.0), Some((Clip::Walk, 1.0)));
        let (_, w) = t.fade(10.0 + FADE_SECS * 0.5).unwrap();
        assert!((w - 0.5).abs() < 1e-4);
        assert_eq!(t.fade(10.0 + FADE_SECS + 0.01), None);
        t.set_anim(ZombieAnim::Thump, 0.0, 11.0);
        assert_eq!(t.fade(11.0).map(|f| f.0), Some(Clip::Idle));
        t.set_anim(ZombieAnim::Stagger, 0.0, 12.0);
        assert_eq!(t.fade(12.0), None, "into a one-shot");
        t.set_anim(ZombieAnim::Walk, 0.0, 13.0);
        assert_eq!(t.fade(13.0), None, "out of a one-shot");
    }

    #[test]
    fn zombies_no_longer_listed_are_stale() {
        let mut m = HashMap::new();
        for (id, seen) in [(1, 5), (2, 4), (3, 5)] {
            m.insert(id, Tracked { entity: Entity::PLACEHOLDER, anim: ZombieAnim::Dead, since: 0.0, fade_from: None, seen, death: Clip::DieBack, flash_until: 0.0, blow: Vec2::ZERO, blow_at: -1.0 });
        }
        assert_eq!(stale(&m, 5), vec![2]);
    }

    /// Every anim maps to a clip, and one-shot clips end where the next clip starts.
    #[test]
    fn clips_join_up() {
        let close = |a: &ZPose, b: &ZPose| {
            let ma = skin_matrices(&BONES, a);
            let mb = skin_matrices(&BONES, b);
            ma.iter().zip(mb.iter()).all(|(x, y)| x.abs_diff_eq(*y, 0.05))
        };
        assert!(close(&pose(Clip::Fall, 1.0), &pose(Clip::Down, 0.0)), "fall ends lying like the ground loop");
        assert!(close(&pose(Clip::GetUp, 0.0), &pose(Clip::Down, 0.0)), "get-up starts from the ground loop");
        assert!(close(&pose(Clip::GetUp, 1.0), &pose(Clip::Idle, 0.0)), "get-up ends standing");
        assert!(close(&pose(Clip::DieDown, 0.0), &pose(Clip::Down, 0.0)));
        let mut t = Tracked { entity: Entity::PLACEHOLDER, anim: ZombieAnim::Idle, since: 0.0, fade_from: None, seen: 0, death: Clip::DieBack, flash_until: 0.0, blow: Vec2::ZERO, blow_at: -1.0 };
        for anim in [ZombieAnim::Idle, ZombieAnim::Walk, ZombieAnim::Attack, ZombieAnim::Thump, ZombieAnim::Stagger, ZombieAnim::Down, ZombieAnim::GetUp, ZombieAnim::Dead] {
            t.set_anim(anim, 0.0, 1.0);
            let (_, position) = t.clip(1.2);
            assert!(position.is_none_or(|p| (0.0..=1.0).contains(&p)));
        }
    }

    /// Lying poses stay above the ground and inside the culling bounds.
    #[test]
    fn lying_poses_rest_on_the_ground() {
        for clip in [Clip::Down, Clip::DieBack, Clip::DieFront, Clip::DieDown] {
            let m = skin_matrices(&BONES, &pose(clip, 1.0));
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let mut lowest_part = 0;
            for (k, (bone, min, max, _)) in PARTS.into_iter().enumerate() {
                for (quad, _) in box_faces(min, max) {
                    for v in quad {
                        let p = m[bone].transform_point3(v);
                        if p.y < lo.y {
                            lowest_part = k;
                        }
                        lo = lo.min(p);
                        hi = hi.max(p);
                    }
                }
            }
            assert!(lo.y > -0.05 && lo.y < 0.12, "{clip:?} lowest point {lo} (part {lowest_part})");
            assert!(lo.cmpge(POSE_BOUNDS[0]).all() && hi.cmple(POSE_BOUNDS[1]).all(), "{clip:?} {lo} {hi}");
        }
    }

    // --- Baked characters (`.zvat`), from small synthetic files written here. ---

    /// A stick figure 1.7 m tall facing +Z, feet at 0: a box per "limb", `walk` (stride 1 m) and
    /// `idle` swinging the arms, plus whatever extra clips are named.
    fn figure(extra: &[(&str, u32, bool)]) -> Zvat {
        let mut uvs = Vec::new();
        let mut indices = Vec::new();
        let mut rest = Vec::new();
        for (min, max) in [
            (Vec3::new(-0.18, 0.0, -0.1), Vec3::new(0.18, 1.45, 0.12)),
            (Vec3::new(-0.11, 1.45, -0.11), Vec3::new(0.11, 1.7, 0.12)),
            (Vec3::new(0.2, 0.8, -0.05), Vec3::new(0.3, 1.42, 0.05)),
            (Vec3::new(-0.3, 0.8, -0.05), Vec3::new(-0.2, 1.42, 0.05)),
        ] {
            for (quad, _) in box_faces(min, max) {
                let b = rest.len() as u32;
                rest.extend(quad);
                uvs.extend([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
                indices.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
            }
        }
        let arms = |t: f32| -> Vec<Vec3> {
            let m = Mat4::from_translation(Vec3::Y * 1.42) * Mat4::from_quat(rx(-0.5 - 0.3 * (t * TAU).sin())) * Mat4::from_translation(-Vec3::Y * 1.42);
            rest.iter().enumerate().map(|(i, p)| if i >= 48 { m.transform_point3(*p) } else { *p }).collect()
        };
        let mut clips = vec![
            zvat::ZvatClip { name: "walk".into(), frames: 6, secs: 1.0, stride: 1.0, one_shot: false },
            zvat::ZvatClip { name: "idle".into(), frames: 4, secs: 3.0, stride: 0.0, one_shot: false },
        ];
        for &(name, frames, one_shot) in extra {
            clips.push(zvat::ZvatClip { name: name.into(), frames, secs: 0.8, stride: 0.0, one_shot });
        }
        let positions = clips.iter().flat_map(|c| (0..c.frames).flat_map(|f| arms(f as f32 / c.frames as f32)).collect::<Vec<_>>()).collect();
        Zvat { uvs, indices, clips, positions }
    }

    fn baked(z: &Zvat) -> Baked {
        // Through the bytes, as a file would come.
        build_baked(&Zvat::parse(&z.to_bytes()).unwrap()).unwrap()
    }

    /// The lowest and highest point of a clip's frame `f` (the bounds columns left out).
    fn extent(b: &Baked, clip: Clip, f: u32) -> (Vec3, Vec3) {
        let c = b.clips[clip as usize];
        let row = &b.row(c.x as u32 + f)[..b.width as usize - 6];
        row.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)))
    }

    /// Every slot gets frames: the file's walk and idle as they are, stand-ins for the rest.
    #[test]
    fn a_baked_character_fills_every_clip() {
        let z = figure(&[]);
        let b = baked(&z);
        assert_eq!(sources(&z)[Clip::Walk as usize], Source::File(0));
        assert_eq!(sources(&z)[Clip::Attack as usize], Source::Leaned(1, ATTACK_LEAN));
        assert_eq!(sources(&z)[Clip::DieBack as usize], Source::Rigid);
        assert_eq!(b.width as usize, z.vertices() + 6);
        assert_eq!(b.texels.len(), (b.width * b.rows) as usize);
        assert_eq!(b.image().height(), b.rows);
        let walk = b.clips[Clip::Walk as usize];
        assert_eq!((walk.y, walk.z, walk.w), (6.0, 0.0, 1.0), "the file's walk, played at speed / stride");
        assert_eq!(b.row(walk.x as u32 + 2)[..z.vertices()], *z.frame(0, 2));
        let idle = b.clips[Clip::Idle as usize];
        assert_eq!((idle.y, idle.z, idle.w), (4.0, 1.0 / 3.0, 0.0), "the idle at its own rate");
        assert_eq!(b.row(idle.x as u32)[..z.vertices()], *z.frame(1, 0));
        for (clip, _, _, _, secs) in CLIPS {
            let c = b.clips[clip as usize];
            assert!(c.y >= 1.0 && c.x + c.y <= b.rows as f32, "{clip:?}: {c}");
            assert_eq!(c.w < 0.0, secs > 0.0, "{clip:?}: one-shots marked as such");
        }
        // Every frame inside the culling bounds.
        assert!(b.texels.iter().all(|p| p.cmpge(b.bounds[0]).all() && p.cmple(b.bounds[1]).all()));
        // The attack stand-in leans in, feet still on the ground.
        let (lo, hi) = extent(&b, Clip::Attack, 0);
        let (_, idle_hi) = extent(&b, Clip::Idle, 0);
        assert!(lo.y.abs() < 1e-4 && hi.z > idle_hi.z + 0.15, "{lo} {hi} vs {idle_hi}");
    }

    /// The rigid stand-ins start standing, end lying on the ground (back or face), and get up again.
    #[test]
    fn baked_stand_ins_fall_lie_and_get_up() {
        let b = baked(&figure(&[]));
        let standing = |clip: Clip, f: u32| {
            let (lo, hi) = extent(&b, clip, f);
            lo.y.abs() < 0.02 && hi.y > 1.6
        };
        let lying = |clip: Clip, f: u32| {
            let (lo, hi) = extent(&b, clip, f);
            lo.y > -0.05 && lo.y < 0.12 && hi.y < 0.6
        };
        let last = |clip: Clip| b.clips[clip as usize].y as u32 - 1;
        for clip in [Clip::Fall, Clip::DieBack, Clip::DieFront] {
            assert!(standing(clip, 0), "{clip:?} starts standing");
            assert!(lying(clip, last(clip)), "{clip:?} ends lying {:?}", extent(&b, clip, last(clip)));
        }
        for f in 0..last(Clip::Down) + 1 {
            assert!(lying(Clip::Down, f), "the ground loop stays down");
        }
        assert!(lying(Clip::DieDown, 0) && lying(Clip::DieDown, last(Clip::DieDown)));
        assert!(lying(Clip::GetUp, 0) && standing(Clip::GetUp, last(Clip::GetUp)));
        assert!(standing(Clip::Stagger, 0) && standing(Clip::Stagger, last(Clip::Stagger)));
        // Backward falls lie toward -Z (behind), forward ones toward +Z.
        assert!(extent(&b, Clip::DieBack, last(Clip::DieBack)).0.z < -1.2);
        assert!(extent(&b, Clip::DieFront, last(Clip::DieFront)).1.z > 1.2);
    }

    /// A clip in the file takes its slot; a file without an idle uses its walk for it.
    #[test]
    fn file_clips_replace_stand_ins() {
        let z = figure(&[("Stagger", 5, true), ("die_back", 3, true), ("attack", 4, false)]);
        let s = sources(&z);
        assert_eq!((s[Clip::Stagger as usize], s[Clip::DieBack as usize], s[Clip::Attack as usize]), (Source::File(2), Source::File(3), Source::File(4)));
        let b = baked(&z);
        assert_eq!(b.clips[Clip::Stagger as usize].y, 5.0);
        assert_eq!(b.clips[Clip::Attack as usize].z, 1.0 / 0.8);

        let mut walk_only = figure(&[]);
        walk_only.clips.truncate(1);
        walk_only.positions.truncate(6 * walk_only.vertices());
        assert_eq!(sources(&walk_only)[Clip::Idle as usize], Source::Other(0));
        let b = baked(&walk_only);
        let (idle, walk) = (b.clips[Clip::Idle as usize], b.clips[Clip::Walk as usize]);
        assert_eq!((idle.x, idle.y, idle.z, idle.w), (walk.x, 6.0, 1.0, 0.0), "the walk's frames, stored once, at a fixed rate");
    }

    #[test]
    fn the_crowd_vat_path() {
        let opts = |args: &str| Options::parse(&args.split_whitespace().map(String::from).collect::<Vec<_>>());
        assert_eq!(crowd_vat_path(&opts("--crowd-vat /x/z.zvat")), Some(PathBuf::from("/x/z.zvat")));
        assert_eq!(crowd_vat_path(&opts("--crowd-vat none")), None);
        assert_eq!(crowd_vat_path(&opts("--crowd-vat none --data /nowhere")), None);
        let dir = std::env::temp_dir().join(format!("zombye-crowd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(first_zvat(&dir), None);
        for name in ["b.zvat", "a.zvat", "a.glb"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        assert_eq!(first_zvat(&dir), Some(dir.join("a.zvat")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
