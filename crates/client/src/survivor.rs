//! Survivors (players): a procedural low-poly person built like the zombies ([`crate::rig`]: boxes on
//! a skeleton), but as a Bevy skinned mesh posed on the CPU every frame, since there are only a
//! few of them. That buys blending between clips, real lighting and shadows, and joints the held
//! weapon can hang from.
//!
//! Twelve bones (knees and elbows, a right hand for the grip). Upright, clean clothes, a mustard
//! jacket, jeans and a backpack: nothing like the hunched, grubby zombies.
//!
//! Animation is layered:
//! - **locomotion**: idle / walk / run / sneak (crouched walk), blended by stance; the cycle advances
//!   with the distance actually moved, so feet keep pace with movement at any speed;
//! - **actions**: swing (punch, one-handed or two-handed by weapon category), shove, stomp, timed
//!   from the snapshot (`action_left` until the blow lands, then `ready_in` to recover), so the
//!   blow lands on screen when the server lands it;
//! - **hurt**: a short additive flinch on every wound;
//! - **dead**: a fall onto the back.
//!
//! Every pose is then pushed down so the lowest foot touches the ground.

use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use zombye_sim::{ActionKind, CombatEventKind, Stance, TICK_DT, WeaponCategory};

use crate::items_render::WeaponLooks;
use crate::player::LocalPlayer;
use crate::rig::{Bone, Pose, box_faces, mix, rx, ry, rz, skin_matrices, smooth};
use crate::server_link::{CombatEventsThisFrame, JoinInfo, ServerLink};

const N: usize = 12;
type SPose = Pose<N>;

const PELVIS: usize = 0;
const TORSO: usize = 1;
const HEAD: usize = 2;
const UARM_L: usize = 3;
const FARM_L: usize = 4;
const UARM_R: usize = 5;
const FARM_R: usize = 6;
const HAND_R: usize = 7;
const THIGH_L: usize = 8;
const SHIN_L: usize = 9;
const THIGH_R: usize = 10;
const SHIN_R: usize = 11;

/// Hip height at rest.
const HIP: f32 = 0.95;

/// (parent, rest pivot). The model faces +Z, its left is +X, feet at y = 0.
const BONES: [Bone; N] = [
    (None, Vec3::new(0.0, HIP, 0.0)),
    (Some(PELVIS), Vec3::new(0.0, 1.0, 0.0)),
    (Some(TORSO), Vec3::new(0.0, 1.5, 0.0)),
    (Some(TORSO), Vec3::new(0.25, 1.43, 0.0)),
    (Some(UARM_L), Vec3::new(0.25, 1.15, 0.0)),
    (Some(TORSO), Vec3::new(-0.25, 1.43, 0.0)),
    (Some(UARM_R), Vec3::new(-0.25, 1.15, 0.0)),
    (Some(FARM_R), Vec3::new(-0.25, 0.9, 0.0)),
    (Some(PELVIS), Vec3::new(0.1, HIP, 0.0)),
    (Some(THIGH_L), Vec3::new(0.1, 0.5, 0.0)),
    (Some(PELVIS), Vec3::new(-0.1, HIP, 0.0)),
    (Some(THIGH_R), Vec3::new(-0.1, 0.5, 0.0)),
];

#[derive(Clone, Copy)]
enum Kind {
    Skin,
    Hair,
    Jacket,
    Shirt,
    Pants,
    Boots,
    Belt,
    Pack,
}

const fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

/// (bone, min, max, kind). Left side; the right side is mirrored in [`parts`].
const LEFT_PARTS: [(usize, Vec3, Vec3, Kind); 6] = [
    (UARM_L, v(0.19, 1.15, -0.065), v(0.31, 1.46, 0.065), Kind::Jacket),
    (FARM_L, v(0.2, 0.9, -0.055), v(0.3, 1.16, 0.055), Kind::Jacket),
    (FARM_L, v(0.21, 0.78, -0.045), v(0.29, 0.9, 0.05), Kind::Skin),
    (THIGH_L, v(0.02, 0.5, -0.08), v(0.18, 0.96, 0.08), Kind::Pants),
    (SHIN_L, v(0.03, 0.1, -0.075), v(0.17, 0.51, 0.075), Kind::Pants),
    (SHIN_L, v(0.025, 0.0, -0.09), v(0.175, 0.12, 0.16), Kind::Boots),
];

const CENTRE_PARTS: [(usize, Vec3, Vec3, Kind); 9] = [
    (PELVIS, v(-0.165, 0.86, -0.1), v(0.165, 1.02, 0.1), Kind::Pants),
    (PELVIS, v(-0.17, 0.96, -0.105), v(0.17, 1.0, 0.105), Kind::Belt),
    (TORSO, v(-0.19, 1.0, -0.115), v(0.19, 1.46, 0.115), Kind::Jacket),
    // The shirt showing between the open jacket's fronts.
    (TORSO, v(-0.065, 1.05, 0.115), v(0.065, 1.43, 0.128), Kind::Shirt),
    (TORSO, v(-0.15, 1.08, -0.215), v(0.15, 1.4, -0.115), Kind::Pack),
    (TORSO, v(-0.05, 1.44, -0.04), v(0.05, 1.52, 0.05), Kind::Skin),
    (HEAD, v(-0.105, 1.5, -0.1), v(0.105, 1.75, 0.12), Kind::Skin),
    (HEAD, v(-0.112, 1.69, -0.112), v(0.112, 1.785, 0.115), Kind::Hair),
    (HEAD, v(-0.112, 1.53, -0.112), v(0.112, 1.69, -0.07), Kind::Hair),
];

/// The bone on the other side of the body.
fn mirror_bone(b: usize) -> usize {
    match b {
        UARM_L => UARM_R,
        FARM_L => FARM_R,
        THIGH_L => THIGH_R,
        SHIN_L => SHIN_R,
        other => other,
    }
}

fn parts() -> Vec<(usize, Vec3, Vec3, Kind)> {
    let mut out: Vec<_> = CENTRE_PARTS.to_vec();
    for (bone, min, max, kind) in LEFT_PARTS {
        out.push((bone, min, max, kind));
        let mut b = mirror_bone(bone);
        // The right hand has its own bone (the weapon grip).
        if bone == FARM_L && matches!(kind, Kind::Skin) {
            b = HAND_R;
        }
        out.push((b, Vec3::new(-max.x, min.y, min.z), Vec3::new(-min.x, max.y, max.z), kind));
    }
    out
}

fn color(kind: Kind, jacket: Color) -> LinearRgba {
    let c = match kind {
        Kind::Skin => Color::srgb(0.86, 0.66, 0.52),
        Kind::Hair => Color::srgb(0.22, 0.14, 0.08),
        Kind::Jacket => jacket,
        Kind::Shirt => Color::srgb(0.88, 0.87, 0.82),
        Kind::Pants => Color::srgb(0.2, 0.3, 0.5),
        Kind::Boots => Color::srgb(0.3, 0.2, 0.11),
        Kind::Belt => Color::srgb(0.12, 0.08, 0.05),
        Kind::Pack => Color::srgb(0.3, 0.35, 0.2),
    };
    c.to_linear()
}

/// The local player's jacket, and everyone else's.
pub const JACKET_ME: Color = Color::srgb(0.85, 0.6, 0.15);
pub const JACKET_OTHER: Color = Color::srgb(0.15, 0.48, 0.55);

fn build_mesh(jacket: Color) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut joints: Vec<[u16; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for (bone, min, max, kind) in parts() {
        let c = color(kind, jacket).to_f32_array();
        for (quad, n) in box_faces(min, max) {
            let base = positions.len() as u32;
            for p in quad {
                positions.push(p.to_array());
                normals.push(n.to_array());
                colors.push(c);
                joints.push([bone as u16, 0, 0, 0]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    let weights = vec![[1.0f32, 0.0, 0.0, 0.0]; positions.len()];
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(joints))
        .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights)
        .with_inserted_indices(Indices::U32(indices))
}

/// Meshes, material and bind poses shared by every survivor.
#[derive(Resource)]
pub struct SurvivorAssets {
    me: Handle<Mesh>,
    other: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    bindposes: Handle<SkinnedMeshInverseBindposes>,
}

impl SurvivorAssets {
    pub fn new(meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>, bindposes: &mut Assets<SkinnedMeshInverseBindposes>) -> Self {
        let inverse: Vec<Mat4> = BONES.iter().map(|(_, pivot)| Mat4::from_translation(-*pivot)).collect();
        Self {
            me: meshes.add(build_mesh(JACKET_ME)),
            other: meshes.add(build_mesh(JACKET_OTHER)),
            material: materials.add(StandardMaterial { base_color: Color::WHITE, perceptual_roughness: 0.85, ..default() }),
            bindposes: bindposes.add(SkinnedMeshInverseBindposes::from(inverse)),
        }
    }
}

/// The weapon in a survivor's right hand: a bar in the weapon's colour, child of the hand joint.
#[derive(Component)]
pub struct HeldWeapon {
    player: u32,
}

/// Build a survivor's joints, skinned body and held weapon under `root` (which carries the
/// position and facing). `local` picks the local player's look.
pub fn attach(commands: &mut Commands, assets: &SurvivorAssets, root: Entity, player: u32, local: bool) {
    let mut joints = [Entity::PLACEHOLDER; N];
    for (b, (parent, pivot)) in BONES.iter().enumerate() {
        let local_pivot = match parent {
            Some(pa) => *pivot - BONES[*pa].1,
            None => *pivot,
        };
        let parent_entity = parent.map_or(root, |pa| joints[pa]);
        joints[b] = commands.spawn((Transform::from_translation(local_pivot), Visibility::Inherited, ChildOf(parent_entity))).id();
    }
    // The grip sits in the fist; the bar points along the hand's +Z (forward when the arm hangs).
    commands.spawn((HeldWeapon { player }, Transform::from_xyz(0.0, -0.07, 0.0), Visibility::Hidden, ChildOf(joints[HAND_R])));
    commands.spawn((
        Mesh3d(if local { assets.me.clone() } else { assets.other.clone() }),
        MeshMaterial3d(assets.material.clone()),
        SkinnedMesh { inverse_bindposes: assets.bindposes.clone(), joints: joints.to_vec() },
        // Posed far from the rest pose (lying down), so don't cull by the rest-pose bounds.
        NoFrustumCulling,
        Transform::default(),
        ChildOf(root),
    ));
    commands.entity(root).insert(SurvivorAnim::new(player, joints));
}

/// How a weapon is swung.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    Punch,
    OneHand,
    TwoHand,
}

fn style(category: WeaponCategory) -> Style {
    match category {
        WeaponCategory::Unarmed => Style::Punch,
        WeaponCategory::ShortBlade | WeaponCategory::ShortBlunt => Style::OneHand,
        WeaponCategory::LongBlunt | WeaponCategory::Axe => Style::TwoHand,
    }
}

/// One action on the local clock: started, lands, recovered.
#[derive(Clone, Copy, Debug)]
struct ActionClock {
    kind: ActionKind,
    start: f32,
    land: f32,
    /// Set once the server says it landed.
    end: Option<f32>,
}

/// Where in an action clip the blow lands: the windup plays before it, the recovery after.
const U_HIT: f32 = 0.45;

impl ActionClock {
    /// Normalised clip time now.
    fn u(&self, now: f32) -> f32 {
        if now < self.land {
            U_HIT * ((now - self.start) / (self.land - self.start).max(1e-3)).clamp(0.0, 1.0)
        } else {
            match self.end {
                Some(end) => U_HIT + (1.0 - U_HIT) * ((now - self.land) / (end - self.land).max(1e-3)).clamp(0.0, 1.0),
                None => U_HIT,
            }
        }
    }
}

/// Animation state of one survivor.
#[derive(Component)]
pub struct SurvivorAnim {
    player: u32,
    joints: [Entity; N],
    last_pos: Option<Vec2>,
    /// Smoothed ground speed (m/s), stance weights and the moving/standing weight.
    speed: f32,
    run_w: f32,
    sneak_w: f32,
    move_w: f32,
    /// Locomotion cycle in [0, 1) and the idle breathing clock.
    cycle: f32,
    idle: f32,
    action: Option<ActionClock>,
    last_tick: u64,
    hurt_at: f32,
    dead_since: Option<f32>,
}

impl SurvivorAnim {
    fn new(player: u32, joints: [Entity; N]) -> Self {
        Self {
            player,
            joints,
            last_pos: None,
            speed: 0.0,
            run_w: 0.0,
            sneak_w: 0.0,
            move_w: 0.0,
            cycle: 0.0,
            idle: 0.0,
            action: None,
            last_tick: u64::MAX,
            hurt_at: -100.0,
            dead_since: None,
        }
    }
}

/// Distance covered by one full locomotion cycle (two steps), per stance.
const WALK_CYCLE_M: f32 = 1.5;
const RUN_CYCLE_M: f32 = 2.5;
const SNEAK_CYCLE_M: f32 = 0.95;
const DEAD_FALL_SECS: f32 = 0.8;
const FLINCH_SECS: f32 = 0.35;

pub struct SurvivorPlugin;

impl Plugin for SurvivorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (animate_survivors, show_held_weapons));
    }
}

#[allow(clippy::too_many_arguments)]
fn animate_survivors(
    time: Res<Time>,
    link: Res<ServerLink>,
    me: Option<Res<LocalPlayer>>,
    join: Res<JoinInfo>,
    events: Res<CombatEventsThisFrame>,
    mut survivors: Query<(&mut SurvivorAnim, &Transform)>,
    mut joints: Query<&mut Transform, Without<SurvivorAnim>>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs().max(1e-4);
    for (mut a, tf) in &mut survivors {
        let a = &mut *a;
        let Some(view) = link.curr.players.iter().find(|p| p.id == a.player) else { continue };
        let local = me.as_ref().filter(|m| m.id == a.player);
        let stance = local.map_or(view.stance, |m| m.curr.stance);

        // Speed from how far the body actually moved this frame.
        let pos = tf.translation.xz();
        let fresh = a.last_pos.is_none();
        let moved = a.last_pos.map_or(0.0, |p| p.distance(pos));
        a.last_pos = Some(pos);
        let inst = if moved > 2.0 { 0.0 } else { (moved / dt).min(8.0) };
        let k = |rate: f32| 1.0 - (-rate * dt).exp();
        a.speed += (inst - a.speed) * k(12.0);
        a.run_w += ((stance == Stance::Run) as u8 as f32 - a.run_w) * k(8.0);
        a.sneak_w += ((stance == Stance::Sneak) as u8 as f32 - a.sneak_w) * k(8.0);
        a.move_w += (smooth(0.15, 0.8, a.speed) - a.move_w) * k(10.0);
        let cycle_m = mix(mix(WALK_CYCLE_M, RUN_CYCLE_M, a.run_w), SNEAK_CYCLE_M, a.sneak_w);
        a.cycle = (a.cycle + a.speed * dt / cycle_m).fract();
        a.idle = (a.idle + dt * 0.3).fract();

        // The action under way, timed from snapshots.
        if link.curr.tick != a.last_tick {
            a.last_tick = link.curr.tick;
            match view.action {
                Some(kind) => match &mut a.action {
                    Some(c) if c.kind == kind && c.end.is_none() => c.land = now + view.action_left,
                    _ => a.action = Some(ActionClock { kind, start: now - TICK_DT, land: now + view.action_left, end: None }),
                },
                None => {
                    if let Some(c) = &mut a.action
                        && c.end.is_none()
                    {
                        c.land = c.land.min(now);
                        c.end = Some(now + view.ready_in.max(0.15));
                    }
                }
            }
        }
        if a.action.is_some_and(|c| c.end.is_some_and(|e| now > e)) {
            a.action = None;
        }
        if events.0.iter().any(|e| e.kind == CombatEventKind::PlayerHurt && e.player == a.player) {
            a.hurt_at = now;
        }
        // Asleep, a survivor lies on its back too (#11), and gets straight up when it wakes.
        match (view.dead || view.asleep, a.dead_since) {
            // Seen dead (or asleep) from the start: already lying there.
            (true, None) => a.dead_since = Some(if fresh { now - 10.0 } else { now }),
            (false, Some(_)) => {
                a.dead_since = None;
                a.action = None;
            }
            _ => {}
        }

        let category = join.weapons.get(view.weapon.map_or(usize::MAX, |w| w as usize)).map_or(WeaponCategory::Unarmed, |w| w.category);
        let armed = view.weapon.is_some();
        let pose = compose(a, now, style(category), armed);
        for (b, (parent, pivot)) in BONES.iter().enumerate() {
            if let Ok(mut jt) = joints.get_mut(a.joints[b]) {
                let local_pivot = match parent {
                    Some(pa) => *pivot - BONES[*pa].1,
                    None => *pivot + pose.offset,
                };
                jt.translation = local_pivot;
                jt.rotation = pose.rot[b];
            }
        }
    }
}

/// Upper-body bones an action takes over.
const UPPER: [usize; 7] = [TORSO, HEAD, UARM_L, FARM_L, UARM_R, FARM_R, HAND_R];
const ALL: [usize; N] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

/// The full pose of a survivor now.
fn compose(a: &SurvivorAnim, now: f32, style: Style, armed: bool) -> SPose {
    let standing = idle(a.idle, armed).blend(&crouch(0.0, armed, 0.0), a.sneak_w);
    let moving = walk(a.cycle, armed).blend(&run(a.cycle, armed), a.run_w).blend(&crouch(a.cycle, armed, 1.0), a.sneak_w);
    let mut pose = standing.blend(&moving, a.move_w);
    if let Some(c) = a.action {
        let u = c.u(now);
        let w = smooth(0.0, 0.06, u) * (1.0 - smooth(0.88, 1.0, u));
        let act = action(c.kind, style, armed, u);
        if c.kind == ActionKind::Stomp {
            pose.blend_bones(&act, w, &ALL, false);
        } else {
            pose.blend_bones(&act, w, &UPPER, false);
        }
    }
    // A flinch on top of whatever it's doing.
    let e = now - a.hurt_at;
    if (0.0..FLINCH_SECS).contains(&e) {
        let w = if e < 0.05 { e / 0.05 } else { (1.0 - (e - 0.05) / (FLINCH_SECS - 0.05)).powi(2) };
        let f = flinch();
        for b in UPPER {
            pose.rot[b] *= Quat::IDENTITY.slerp(f.rot[b], w);
        }
    }
    ground(&mut pose);
    // Running has a moment in the air each step.
    pose.offset.y += a.run_w * a.move_w * 0.05 * (a.cycle * TAU * 2.0).sin().max(0.0);
    if let Some(since) = a.dead_since {
        let e = now - since;
        pose = pose.blend(&dead(e), smooth(0.0, 0.12, e));
    }
    pose
}

/// Push the pose down (or up) so the lowest point of either boot is on the ground.
fn ground(pose: &mut SPose) {
    let m = skin_matrices(&BONES, pose);
    let mut lowest = f32::MAX;
    for (shin, x) in [(SHIN_L, 0.1), (SHIN_R, -0.1)] {
        for z in [-0.09, 0.16] {
            lowest = lowest.min(m[shin].transform_point3(Vec3::new(x, 0.0, z)).y);
        }
    }
    pose.offset.y -= lowest;
}

/// The right arm when it isn't doing anything: a weapon held at the ready (pointing up and
/// forward), or a loose hand.
fn carry(p: &mut SPose, armed: bool, swing: f32) {
    if armed {
        p.rot[UARM_R] = rx(-0.2 - 0.1 * swing) * rz(-0.08);
        p.rot[FARM_R] = rx(-0.55);
        p.rot[HAND_R] = rx(0.15);
    } else {
        p.rot[UARM_R] = rx(-0.38 * swing) * rz(-0.1);
        p.rot[FARM_R] = rx(-0.3);
    }
}

fn idle(t: f32, armed: bool) -> SPose {
    let s = (t * TAU).sin();
    let mut p = SPose::default();
    p.rot[TORSO] = rx(0.04 + 0.012 * s);
    p.rot[HEAD] = rx(-0.03 + 0.01 * s);
    p.rot[UARM_L] = rx(-0.05 + 0.02 * s) * rz(0.12);
    p.rot[FARM_L] = rx(-0.3);
    carry(&mut p, armed, 0.0);
    p.rot[THIGH_L] = rz(0.04) * rx(-0.03);
    p.rot[THIGH_R] = rz(-0.04) * rx(0.02);
    p.rot[SHIN_L] = rx(0.06);
    p.rot[SHIN_R] = rx(0.04);
    p
}

fn walk(t: f32, armed: bool) -> SPose {
    let ph = t * TAU;
    let (s, c) = (ph.sin(), ph.cos());
    let mut p = SPose::default();
    // Left leg forward while s > 0; a leg's knee bends as it swings through.
    p.rot[THIGH_L] = rx(-0.42 * s);
    p.rot[THIGH_R] = rx(0.42 * s);
    p.rot[SHIN_L] = rx(0.1 + 0.6 * c.max(0.0));
    p.rot[SHIN_R] = rx(0.1 + 0.6 * (-c).max(0.0));
    p.rot[PELVIS] = ry(-0.07 * s);
    p.rot[TORSO] = rx(0.06) * ry(0.12 * s);
    p.rot[HEAD] = rx(-0.04) * ry(-0.05 * s);
    p.rot[UARM_L] = rx(0.38 * s) * rz(0.08);
    p.rot[FARM_L] = rx(-0.3);
    carry(&mut p, armed, s);
    p
}

fn run(t: f32, armed: bool) -> SPose {
    let ph = t * TAU;
    let (s, c) = (ph.sin(), ph.cos());
    let mut p = SPose::default();
    p.rot[THIGH_L] = rx(-0.85 * s);
    p.rot[THIGH_R] = rx(0.85 * s);
    p.rot[SHIN_L] = rx(0.3 + 1.3 * c.max(0.0));
    p.rot[SHIN_R] = rx(0.3 + 1.3 * (-c).max(0.0));
    p.rot[PELVIS] = ry(-0.1 * s);
    p.rot[TORSO] = rx(0.28) * ry(0.15 * s);
    p.rot[HEAD] = rx(-0.2);
    p.rot[UARM_L] = rx(0.7 * s) * rz(0.1);
    p.rot[FARM_L] = rx(-1.4);
    if armed {
        p.rot[UARM_R] = rx(-0.35 - 0.3 * s) * rz(-0.1);
        p.rot[FARM_R] = rx(-1.2);
        p.rot[HAND_R] = rx(0.6);
    } else {
        p.rot[UARM_R] = rx(-0.7 * s) * rz(-0.1);
        p.rot[FARM_R] = rx(-1.4);
    }
    p
}

/// Crouched: knees bent, leaning forward, hands up. `amp` 0 = standing still, 1 = sneaking along.
fn crouch(t: f32, armed: bool, amp: f32) -> SPose {
    let ph = t * TAU;
    let (s, c) = (ph.sin() * amp, ph.cos() * amp);
    let mut p = SPose::default();
    p.rot[THIGH_L] = rx(-0.9 - 0.3 * s);
    p.rot[THIGH_R] = rx(-0.9 + 0.3 * s);
    p.rot[SHIN_L] = rx(1.4 + 0.3 * c.max(0.0));
    p.rot[SHIN_R] = rx(1.4 + 0.3 * (-c).max(0.0));
    p.rot[PELVIS] = ry(-0.05 * s);
    p.rot[TORSO] = rx(0.5) * ry(0.06 * s);
    p.rot[HEAD] = rx(-0.4);
    p.rot[UARM_L] = rx(-0.45 + 0.1 * s) * rz(0.1);
    p.rot[FARM_L] = rx(-0.9);
    if armed {
        p.rot[UARM_R] = rx(-0.5) * rz(-0.08);
        p.rot[FARM_R] = rx(-0.7);
        p.rot[HAND_R] = rx(0.35);
    } else {
        p.rot[UARM_R] = rx(-0.45 - 0.1 * s) * rz(-0.1);
        p.rot[FARM_R] = rx(-0.9);
    }
    p
}

/// Keyframes: (time, pose), interpolated with an ease between neighbours.
fn keyed(keys: &[(f32, SPose)], u: f32) -> SPose {
    let i = keys.iter().rposition(|(t, _)| *t <= u).unwrap_or(0);
    if i + 1 >= keys.len() {
        return keys[i].1.clone();
    }
    let (t0, a) = &keys[i];
    let (t1, b) = &keys[i + 1];
    a.blend(b, smooth(*t0, *t1, u))
}

/// Upper-body pose builder.
#[allow(clippy::too_many_arguments)]
fn upper(torso: Quat, head: Quat, uarm_l: Quat, farm_l: Quat, uarm_r: Quat, farm_r: Quat, hand_r: Quat) -> SPose {
    let mut p = SPose::default();
    p.rot[TORSO] = torso;
    p.rot[HEAD] = head;
    p.rot[UARM_L] = uarm_l;
    p.rot[FARM_L] = farm_l;
    p.rot[UARM_R] = uarm_r;
    p.rot[FARM_R] = farm_r;
    p.rot[HAND_R] = hand_r;
    p
}

/// An action's pose at normalised time `u` (the blow lands at [`U_HIT`]).
fn action(kind: ActionKind, style: Style, armed: bool, u: f32) -> SPose {
    let ready = idle(0.0, armed);
    let q = Quat::IDENTITY;
    match (kind, style) {
        (ActionKind::Swing, Style::Punch) => {
            let windup = upper(ry(-0.3), ry(0.2), rz(0.1) * rx(-0.6), rx(-1.9), rz(-0.15) * rx(-0.5), rx(-1.9), q);
            let hit = upper(ry(0.4) * rx(0.12), ry(-0.3), rx(-0.5), rx(-1.9), rz(0.15) * rx(-1.55), rx(-0.1), q);
            let back = upper(ry(0.25) * rx(0.08), ry(-0.15), rx(-0.5), rx(-1.8), rz(0.1) * rx(-1.2), rx(-0.8), q);
            keyed(&[(0.0, ready.clone()), (0.3, windup), (U_HIT, hit), (0.62, back), (1.0, ready)], u)
        }
        (ActionKind::Swing, Style::OneHand) => {
            // Overhand chop: raised up and back, brought down across the body.
            let windup = upper(ry(-0.45) * rx(-0.04), ry(0.25), rz(0.25) * rx(-0.7), rx(-0.6), rz(-0.4) * rx(-2.5), rx(-1.1), rx(0.5));
            let hit = upper(ry(0.35) * rx(0.15), ry(-0.2), rz(0.3) * rx(-0.3), rx(-0.4), rz(0.25) * rx(-1.0), rx(-0.15), rx(0.9));
            let follow = upper(ry(0.55) * rx(0.25), ry(-0.3), rz(0.3) * rx(-0.2), rx(-0.4), rz(0.45) * rx(-0.35), rx(-0.2), rx(0.9));
            keyed(&[(0.0, ready.clone()), (0.32, windup), (U_HIT, hit), (0.62, follow), (1.0, ready)], u)
        }
        (ActionKind::Swing, Style::TwoHand) => {
            // Wound up over the right shoulder, swept flat across to the left.
            let windup = upper(ry(-0.75) * rx(-0.05), ry(0.5), rz(-0.5) * rx(-1.4), rx(-1.3), rz(-0.55) * rx(-1.7), rx(-1.4), rx(0.3));
            let hit = upper(ry(0.4) * rx(0.1), ry(-0.3), rz(-0.45) * rx(-1.35), rx(-0.2), rz(0.1) * rx(-1.45), rx(-0.1), rx(1.35));
            let follow = upper(ry(0.85) * rx(0.15), ry(-0.5), rz(0.1) * rx(-1.0), rx(-0.5), rz(0.5) * rx(-1.2), rx(-0.2), rx(1.2));
            keyed(&[(0.0, ready.clone()), (0.32, windup), (U_HIT, hit), (0.62, follow), (1.0, ready)], u)
        }
        (ActionKind::Shove, _) => {
            let hand = if armed { rx(0.3) } else { q };
            let gather = upper(rx(-0.05), q, rz(-0.1) * rx(-0.6), rx(-1.8), rz(0.1) * rx(-0.6), rx(-1.8), hand);
            let push = upper(rx(0.3), rx(-0.15), rz(-0.12) * rx(-1.45), rx(-0.1), rz(0.12) * rx(-1.45), rx(-0.1), hand);
            let hold = upper(rx(0.22), rx(-0.1), rz(-0.1) * rx(-1.35), rx(-0.3), rz(0.1) * rx(-1.35), rx(-0.3), hand);
            keyed(&[(0.0, ready.clone()), (0.25, gather), (U_HIT, push.clone()), (0.62, hold), (1.0, ready)], u)
        }
        (ActionKind::Stomp, _) => {
            let mut lift = ready.clone();
            lift.rot[THIGH_R] = rx(-1.3);
            lift.rot[SHIN_R] = rx(1.6);
            lift.rot[THIGH_L] = rx(0.05);
            lift.rot[SHIN_L] = rx(0.15);
            lift.rot[TORSO] = rx(0.35);
            lift.rot[HEAD] = rx(0.35);
            lift.rot[UARM_L] = rz(0.35) * rx(-0.3);
            let mut stamp = lift.clone();
            stamp.rot[THIGH_R] = rx(-0.5);
            stamp.rot[SHIN_R] = rx(0.1);
            stamp.rot[THIGH_L] = rx(0.2);
            stamp.rot[SHIN_L] = rx(0.35);
            stamp.rot[TORSO] = rx(0.45);
            stamp.rot[HEAD] = rx(0.4);
            keyed(&[(0.0, ready.clone()), (0.3, lift), (U_HIT, stamp.clone()), (0.65, stamp), (1.0, ready)], u)
        }
    }
}

/// Recoiling from a blow (applied on top of the current pose).
fn flinch() -> SPose {
    let q = Quat::IDENTITY;
    upper(rx(-0.28) * ry(0.12), rx(-0.35), rz(0.25) * rx(-0.45), q, rz(-0.2) * rx(-0.35), q, q)
}

/// Dead: falls onto its back over [`DEAD_FALL_SECS`] (pivoting about the feet), then lies still.
fn dead(e: f32) -> SPose {
    let mut end = SPose::default();
    end.rot[HEAD] = ry(0.8) * rx(-0.1);
    end.rot[UARM_L] = rz(1.2) * rx(-0.2);
    end.rot[FARM_L] = rx(-0.4);
    end.rot[UARM_R] = rz(-1.0);
    end.rot[FARM_R] = rx(-0.6);
    end.rot[THIGH_L] = rz(0.12) * rx(-0.1);
    end.rot[SHIN_L] = rx(0.2);
    end.rot[THIGH_R] = rx(-0.35) * rz(-0.1);
    end.rot[SHIN_R] = rx(0.7);
    let mut p = idle(0.0, false).blend(&end, smooth(0.0, DEAD_FALL_SECS, e));
    let k = (e / DEAD_FALL_SECS).clamp(0.0, 1.0);
    let theta = -FRAC_PI_2 * k * k;
    p.rot[PELVIS] = rx(theta);
    // The hip swings back over the feet and comes to rest a body's thickness off the ground.
    let hip = rx(theta) * Vec3::new(0.0, HIP, 0.0);
    p.offset = hip + Vec3::Y * (0.12 * k - HIP);
    // Arms thrown up as it goes over.
    let flail = (k * std::f32::consts::PI).sin();
    p.rot[UARM_L] *= rx(-flail);
    p.rot[UARM_R] *= rx(-0.8 * flail);
    p
}

/// Show the weapon each survivor holds (from snapshots), as a bar in the weapon's colour.
fn show_held_weapons(
    link: Res<ServerLink>,
    looks: Option<Res<WeaponLooks>>,
    mut commands: Commands,
    mut held: Query<(Entity, &HeldWeapon, &mut Visibility, Option<&Children>)>,
    mut bars: Query<(&mut Transform, &mut MeshMaterial3d<StandardMaterial>), Without<HeldWeapon>>,
) {
    let Some(looks) = looks else { return };
    for (e, h, mut vis, children) in &mut held {
        let view = link.curr.players.iter().find(|p| p.id == h.player);
        let weapon = view.filter(|v| !v.dead).and_then(|v| v.weapon);
        let len = weapon.map_or(0.0, |w| looks.length(w));
        let want = if len > 0.0 { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        let Some(w) = weapon.filter(|_| len > 0.0) else { continue };
        let material = looks.materials.get(w as usize).cloned().unwrap_or_default();
        // The handle sticks out a little behind the fist.
        let bar = Transform::from_xyz(0.0, 0.0, len * 0.5 - 0.1).with_scale(Vec3::new(0.05, 0.05, len));
        match children.and_then(|c| c.first().copied()) {
            Some(child) => {
                if let Ok((mut tf, mut m)) = bars.get_mut(child) {
                    if *tf != bar {
                        *tf = bar;
                    }
                    if m.0 != material {
                        m.0 = material;
                    }
                }
            }
            None => {
                commands.spawn((Mesh3d(looks.cube.clone()), MeshMaterial3d(material), bar, ChildOf(e)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::globals;

    /// Lowest point of each boot's sole.
    fn foot_heights(p: &SPose) -> (f32, f32) {
        let m = skin_matrices(&BONES, p);
        let f = |shin: usize, x: f32| [-0.09, 0.16].map(|z| m[shin].transform_point3(Vec3::new(x, 0.0, z)).y).into_iter().fold(f32::MAX, f32::min);
        (f(SHIN_L, 0.1), f(SHIN_R, -0.1))
    }

    /// Grounded poses put a boot on the floor, and nothing sinks below it.
    #[test]
    fn locomotion_keeps_a_foot_on_the_ground() {
        for i in 0..16 {
            let t = i as f32 / 16.0;
            for mut p in [idle(t, true), walk(t, true), run(t, false), crouch(t, true, 1.0)] {
                ground(&mut p);
                let (l, r) = foot_heights(&p);
                assert!(l.min(r).abs() < 1e-4, "t {t}: feet {l} {r}");
            }
        }
    }

    /// The crouch really is lower, and the head stays above the hips.
    #[test]
    fn sneaking_crouches() {
        let mut stand = idle(0.0, false);
        let mut low = crouch(0.0, false, 0.0);
        ground(&mut stand);
        ground(&mut low);
        let head = |p: &SPose| globals(&BONES, p)[HEAD].transform_point3(Vec3::ZERO).y;
        assert!(head(&low) < head(&stand) - 0.15, "{} vs {}", head(&low), head(&stand));
        assert!(head(&low) > 1.0);
    }

    /// The weapon points backward in the windup and forward when the blow lands.
    #[test]
    fn swings_wind_up_and_land_in_front() {
        for style in [Style::OneHand, Style::TwoHand] {
            let dir = |u: f32| {
                let p = action(ActionKind::Swing, style, true, u);
                (globals(&BONES, &p)[HAND_R].transform_vector3(Vec3::Z), globals(&BONES, &p)[HAND_R].transform_point3(Vec3::ZERO))
            };
            let (wind, _) = dir(0.32);
            let (hit, hand) = dir(U_HIT);
            assert!(wind.z < 0.3, "{style:?} windup points {wind}");
            assert!(hit.z > 0.6 && hit.y.abs() < 0.6, "{style:?} hit points {hit}");
            assert!(hand.z > 0.2 && hand.y > 0.9, "{style:?} hand at {hand}");
        }
    }

    #[test]
    fn the_dead_lie_on_their_backs() {
        let p = dead(5.0);
        let g = globals(&BONES, &p);
        let head = g[HEAD].transform_point3(Vec3::new(0.0, 0.1, 0.0));
        assert!(head.y < 0.35 && head.z < -1.0, "head at {head}");
        let (l, r) = foot_heights(&p);
        assert!(l > -0.05 && r > -0.05);
    }

    #[test]
    fn action_clock_spans_windup_and_recovery() {
        let c = ActionClock { kind: ActionKind::Swing, start: 1.0, land: 1.5, end: Some(2.0) };
        assert_eq!(c.u(1.0), 0.0);
        assert!((c.u(1.5) - U_HIT).abs() < 1e-6);
        assert_eq!(c.u(3.0), 1.0);
    }
}
