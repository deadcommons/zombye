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
//! M0 builds a grey-box low-poly zombie procedurally; the same bake step will later take rigged and
//! animated Meshy GLBs (docs/PLAN.md §6).

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

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

use crate::daynight::sky_at;
use crate::fog::FogState;
use crate::rig::{Bone, Pose, box_faces, rx, ry, rz, skin_matrices, smooth};
use crate::server_link::{CombatEventsThisFrame, ServerLink};

const SHADER: &str = "shaders/zombie_vat.wesl";

/// x = vertex index into the VAT, y = body-part kind (see [`Kind`]).
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
            layout.0.get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0), ATTRIBUTE_VAT.at_shader_location(1)])?;
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

/// Corners of the box every pose fits in (lying down reaches well past the standing body, the
/// thump raises the arms high), added to the mesh as two zero-area triangles so frustum culling
/// uses it.
const POSE_BOUNDS: [Vec3; 2] = [Vec3::new(-1.2, -0.2, -2.0), Vec3::new(1.2, 2.6, 2.0)];

fn build_zombie() -> (Mesh, Image, [Vec4; 16]) {
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
    // Degenerate triangles (all three corners at one point): never drawn, but they widen the
    // mesh's bounding box to cover the lying poses. Baked unskinned so they stay put.
    let first_bound = positions.len();
    for corner in POSE_BOUNDS {
        let base = positions.len() as u32;
        for _ in 0..3 {
            vat.push([positions.len() as f32, Kind::Skin as u32 as f32]);
            positions.push(corner.to_array());
            bones.push(PELVIS);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    // Bake every frame of every clip into rows of the VAT.
    let width = positions.len() as u32;
    let rows: u32 = CLIPS.iter().map(|c| c.1).sum();
    let mut texels: Vec<f32> = Vec::with_capacity((width * rows * 4) as usize);
    let mut clips = [Vec4::ZERO; 16];
    let mut row = 0u32;
    for (clip, frames, rate, stride, secs) in CLIPS {
        // w < 0 marks a one-shot clip (its last frame is its final pose, no wrapping).
        clips[clip as usize] = Vec4::new(row as f32, frames as f32, rate, if secs > 0.0 { -1.0 } else { stride });
        for f in 0..frames {
            let t = if secs > 0.0 { f as f32 / (frames - 1).max(1) as f32 } else { f as f32 / frames as f32 };
            let m = skin_matrices(&BONES, &pose(clip, t));
            for (i, p) in positions.iter().enumerate() {
                let v = if i >= first_bound { Vec3::from_array(*p) } else { m[bones[i]].transform_point3(Vec3::from_array(*p)) };
                texels.extend_from_slice(&[v.x, v.y, v.z, 1.0]);
            }
        }
        row += frames;
    }
    let bytes: Vec<u8> = texels.iter().flat_map(|f| f.to_le_bytes()).collect();
    let image = Image::new(
        Extent3d { width, height: rows, depth_or_array_layers: 1 },
        TextureDimension::D2,
        bytes,
        TextureFormat::Rgba32Float,
        RenderAssetUsages::RENDER_WORLD,
    );

    let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(ATTRIBUTE_VAT, vat)
        .with_inserted_indices(Indices::U32(indices));
    (mesh, image, clips)
}

fn setup_zombie_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ZombieMaterial>>,
) {
    let (mesh, image, clips) = build_zombie();
    let material = materials.add(ZombieMaterial {
        vat: images.add(image),
        settings: VatSettings { clips, sun: Vec4::new(0.4, 0.8, 0.3, 1.0), sun_color: Vec4::ONE, ambient: Vec4::splat(0.5) },
    });
    commands.insert_resource(ZombieAssets { mesh: meshes.add(mesh), material });
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
        let (_, image, clips) = build_zombie();
        let rows = image.height();
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
}
