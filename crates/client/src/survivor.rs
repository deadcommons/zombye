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
//!
//! This grey-box is the fallback. With a modular character's files (#62, #63) a survivor is drawn as
//! that body with what it wears ([`crate::survivor_model`]): its own clips for locomotion, and the
//! action, flinch and death poses here carried over onto its skeleton.

use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use zombye_sim::{ActionKind, CombatEventKind, Gesture, ItemCategory, JobKind, Stance, TICK_DT, WeaponCategory};

use crate::items_render::WeaponLooks;
use crate::player::LocalPlayer;
use crate::rig::{Bone, Pose, box_faces, mix, rx, ry, rz, skin_matrices, smooth};
use crate::server_link::{ClientMap, CombatEventsThisFrame, JoinInfo, ServerLink};
use crate::survivor_model::SurvivorModel;

pub(crate) const N: usize = 12;
pub(crate) type SPose = Pose<N>;

pub(crate) const PELVIS: usize = 0;
pub(crate) const TORSO: usize = 1;
pub(crate) const HEAD: usize = 2;
pub(crate) const UARM_L: usize = 3;
pub(crate) const FARM_L: usize = 4;
pub(crate) const UARM_R: usize = 5;
pub(crate) const FARM_R: usize = 6;
pub(crate) const HAND_R: usize = 7;
pub(crate) const THIGH_L: usize = 8;
pub(crate) const SHIN_L: usize = 9;
pub(crate) const THIGH_R: usize = 10;
pub(crate) const SHIN_R: usize = 11;

/// Hip height at rest.
pub(crate) const HIP: f32 = 0.95;

/// (parent, rest pivot). The model faces +Z, its left is +X, feet at y = 0.
pub(crate) const BONES: [Bone; N] = [
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

/// What a survivor holds in one hand (#35): a child of that hand's grip frame (the grip at the
/// origin, pointing along +Z), drawn as `WeaponLooks::in_hand` says; hidden while the hand is empty.
#[derive(Component)]
pub struct HeldItem {
    pub(crate) player: u32,
    /// 0 the right hand (`Hand`), 1 the left (`OffHand`).
    pub(crate) hand: u8,
}

/// Build a survivor's joints, skinned body and held weapon under `root` (which carries the
/// position and facing): the modular character when there is one, else the grey-box. `local` picks
/// the local player's look.
pub fn attach(commands: &mut Commands, assets: &SurvivorAssets, model: Option<&SurvivorModel>, root: Entity, player: u32, local: bool) {
    if let Some(model) = model {
        let joints = crate::survivor_model::attach(commands, model, root, player, local);
        commands.entity(root).insert(SurvivorAnim::new(player, Look::Model(joints), GREY_STRIDES));
        return;
    }
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
    // The grey-box has a right hand only, so whatever is in the off hand isn't drawn on it.
    commands.spawn((HeldItem { player, hand: 0 }, Transform::from_xyz(0.0, -0.07, 0.0), Visibility::Hidden, ChildOf(joints[HAND_R])));
    let body = commands
        .spawn((
            Mesh3d(if local { assets.me.clone() } else { assets.other.clone() }),
            MeshMaterial3d(assets.material.clone()),
            SkinnedMesh { inverse_bindposes: assets.bindposes.clone(), joints: joints.to_vec() },
            // Posed far from the rest pose (lying down), so don't cull by the rest-pose bounds.
            NoFrustumCulling,
            Transform::default(),
            ChildOf(root),
        ))
        .id();
    if local {
        // Ours shows through whatever still covers it (`silhouette.rs`).
        commands.entity(body).insert(crate::silhouette::Silhouetted);
    }
    commands.entity(root).insert(SurvivorAnim::new(player, Look::Grey(joints), GREY_STRIDES));
}

/// How a weapon is swung.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    Punch,
    OneHand,
    TwoHand,
    /// A gun (#72): held out level along the aim, and it kicks.
    Gun,
}

pub(crate) fn style(category: WeaponCategory) -> Style {
    match category {
        WeaponCategory::Unarmed => Style::Punch,
        WeaponCategory::ShortBlade | WeaponCategory::ShortBlunt => Style::OneHand,
        WeaponCategory::LongBlunt | WeaponCategory::Axe => Style::TwoHand,
        WeaponCategory::Firearm => Style::Gun,
    }
}

/// One action on the local clock: started, lands, recovered.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ActionClock {
    pub(crate) kind: ActionKind,
    /// The how-manyth action this survivor began on this client: picks which of a move's variants
    /// plays (only how it looks, so clients needn't agree).
    pub(crate) n: u32,
    start: f32,
    land: f32,
    /// Set once the server says it landed.
    end: Option<f32>,
}

/// Where in an action clip the blow lands: the windup plays before it, the recovery after.
pub(crate) const U_HIT: f32 = 0.45;

impl ActionClock {
    /// Normalised clip time now.
    pub(crate) fn u(&self, now: f32) -> f32 {
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

/// What a survivor is drawn as: its joints.
pub(crate) enum Look {
    Grey([Entity; N]),
    /// The modular character ([`crate::survivor_model`]), one joint per bone of its skeleton.
    Model(Vec<Entity>),
}

/// Animation state of one survivor.
#[derive(Component)]
pub struct SurvivorAnim {
    pub(crate) player: u32,
    look: Look,
    /// Metres covered by one locomotion cycle (two steps) walking, running and sneaking.
    strides: [f32; 3],
    last_pos: Option<Vec2>,
    /// Smoothed ground speed (m/s), stance weights and the moving/standing weight.
    pub(crate) speed: f32,
    pub(crate) run_w: f32,
    pub(crate) sneak_w: f32,
    pub(crate) move_w: f32,
    /// Which way it moves relative to where it faces (radians: 0 ahead, +π/2 to its left), smoothed.
    pub(crate) move_angle: f32,
    /// Aiming (the right mouse held), eased in and out: the ready stance.
    pub(crate) aim_w: f32,
    /// Locomotion cycle in [0, 1) and the idle breathing clock.
    pub(crate) cycle: f32,
    pub(crate) idle: f32,
    pub(crate) action: Option<ActionClock>,
    /// Actions begun so far ([`ActionClock::n`]).
    begun: u32,
    last_tick: u64,
    /// Our own survivor on a multiplayer server (#23): the action begun on the click that
    /// `action` is playing (the input it went with, `player::OwnSwing::seq`) until the snapshots
    /// have caught up with it, whether one has confirmed it, and the last one taken up.
    own: Option<u32>,
    own_confirmed: bool,
    own_taken: u32,
    pub(crate) hurt_at: f32,
    pub(crate) dead_since: Option<f32>,
    /// Lying asleep rather than dead (#93), and when it woke (it gets up over `GET_UP_SECS`).
    pub(crate) sleeping: bool,
    pub(crate) woke_at: Option<f32>,
    /// The gesture it makes or last made (#67), eased in and out, and its own clock (seconds).
    pub(crate) gesture: Option<Gesture>,
    pub(crate) gesture_w: f32,
    pub(crate) gesture_t: f32,
    /// Hands tied behind the back (#73), and hands at work on another survivor (tying, freeing,
    /// searching): each eased in and out. `struggling`: working at the ties now.
    pub(crate) tied_w: f32,
    pub(crate) reach_w: f32,
    pub(crate) struggling: bool,
    /// What its hands are busy with, where we are told (its item job under way: eating, drinking,
    /// searching, reloading), eased in and out, and how long it has been at it (seconds).
    pub(crate) task: Option<Task>,
    pub(crate) task_w: f32,
    pub(crate) task_t: f32,
    /// Lying dead (not knocked down or asleep): a body's own death clip plays.
    pub(crate) died: bool,
    /// Getting through an open or broken window or over a fence: which, eased in and out, how far
    /// across (0 where it came into the climb zone, 1 where it leaves on the far side), and the
    /// side of the edge it came from.
    pub(crate) cross: Option<Cross>,
    pub(crate) cross_w: f32,
    pub(crate) cross_u: f32,
    cross_side: f32,
    /// On a flight of stairs and moving: going up (or down), eased in and out, and where in the
    /// stair loop it is (a body's `stairs_up` / `stairs_down` clip; `STAIR_CYCLE_M` a loop).
    pub(crate) stair: Option<bool>,
    pub(crate) stair_w: f32,
    pub(crate) stair_u: f32,
    stair_p: Option<f32>,
    /// Sitting down (`zombye_sim::sit`): on what, eased in and out (the whole body; moved onto
    /// the seat by `player::sit_on_seat`).
    pub(crate) seat: Option<zombye_sim::SeatKind>,
    pub(crate) sit_w: f32,
}

/// Metres over the ground a stair loop (two steps) covers: what the clip it was made from rose in
/// a loop (0.92 m), a flight rising a metre a metre, so a planted foot stays where it was put.
pub(crate) const STAIR_CYCLE_M: f32 = 0.92;

/// What a survivor is getting across (its clip in [`crate::survivor_model`]'s `CROSS_CLIPS`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Cross {
    /// Through an open or broken window.
    Climb,
    /// Over a fence.
    Vault,
}

/// The edge a body at `p` is climbing across, if any (`TileMap::window_opening`'s rule), and how
/// far `p` is from its line (signed: which side).
pub(crate) fn crossing(map: &zombye_sim::map::TileMap, p: Vec2) -> Option<(Cross, f32)> {
    use zombye_sim::map::CLIMB_ZONE;
    let t = p.floor().as_ivec2();
    let f = p - t.as_vec2();
    let tile = map.tile(t);
    [
        (f.y, tile.north),
        (f.x, tile.west),
        (f.y - 1.0, map.tile(t + IVec2::Y).north),
        (f.x - 1.0, map.tile(t + IVec2::X).west),
    ]
    .into_iter()
    .find(|(d, e)| d.abs() < CLIMB_ZONE && e.is_climbable())
    .map(|(d, e)| (if e == zombye_sim::map::Edge::Fence { Cross::Vault } else { Cross::Climb }, d))
}

/// What a survivor's hands are seen to be busy with (a body's `eat`, `drink`, `loot` and `reload`
/// clips; the grey-box has no pose for them).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Task {
    Eat,
    Drink,
    Loot,
    Reload,
}

impl Task {
    fn of(job: &JobKind, join: &JoinInfo) -> Option<Self> {
        match job {
            JobKind::Search { .. } => Some(Self::Loot),
            JobKind::Reload { .. } => Some(Self::Reload),
            JobKind::Use { item, .. } => match join.items.get(*item as usize)?.category {
                ItemCategory::Drink => Some(Self::Drink),
                ItemCategory::Food => Some(Self::Eat),
                _ => None,
            },
            _ => None,
        }
    }
}

impl SurvivorAnim {
    fn new(player: u32, look: Look, strides: [f32; 3]) -> Self {
        Self {
            player,
            look,
            strides,
            last_pos: None,
            speed: 0.0,
            run_w: 0.0,
            sneak_w: 0.0,
            move_w: 0.0,
            move_angle: 0.0,
            aim_w: 0.0,
            cycle: 0.0,
            idle: 0.0,
            action: None,
            begun: 0,
            last_tick: u64::MAX,
            own: None,
            own_confirmed: false,
            own_taken: 0,
            hurt_at: -100.0,
            dead_since: None,
            sleeping: false,
            woke_at: None,
            task: None,
            task_w: 0.0,
            task_t: 0.0,
            died: false,
            cross: None,
            cross_w: 0.0,
            cross_u: 0.0,
            cross_side: 1.0,
            stair: None,
            stair_w: 0.0,
            stair_u: 0.0,
            stair_p: None,
            gesture: None,
            gesture_w: 0.0,
            gesture_t: 0.0,
            tied_w: 0.0,
            reach_w: 0.0,
            struggling: false,
            seat: None,
            sit_w: 0.0,
        }
    }
}

/// Distance covered by one full locomotion cycle (two steps) of the grey-box, per stance: walking,
/// running, sneaking.
const GREY_STRIDES: [f32; 3] = [1.5, 2.5, 0.95];
const DEAD_FALL_SECS: f32 = 0.8;
pub(crate) const FLINCH_SECS: f32 = 0.35;

pub struct SurvivorPlugin;

impl Plugin for SurvivorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (animate_survivors, show_held_items, crate::survivor_model::dress_survivors, crate::survivor_model::dim_rings, crate::survivor_model::mark_agents));
    }
}

#[allow(clippy::too_many_arguments)]
fn animate_survivors(
    time: Res<Time>,
    link: Res<ServerLink>,
    me: Option<Res<LocalPlayer>>,
    join: Res<JoinInfo>,
    map: Res<ClientMap>,
    events: Res<CombatEventsThisFrame>,
    model: Option<Res<SurvivorModel>>,
    options: Res<crate::Options>,
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
        if let Some(prev) = a.last_pos.filter(|_| moved > 1e-4 && moved < 2.0) {
            // The way it moved, in its own frame (it faces +Z, its left is +X).
            let d = tf.rotation.inverse() * Vec3::new(pos.x - prev.x, 0.0, pos.y - prev.y);
            let target = d.x.atan2(d.z);
            let turn = (target - a.move_angle + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI;
            a.move_angle += turn * (1.0 - (-10.0 * dt).exp());
        }
        a.last_pos = Some(pos);
        let inst = if moved > 2.0 { 0.0 } else { (moved / dt).min(8.0) };
        let k = |rate: f32| 1.0 - (-rate * dt).exp();
        a.speed += (inst - a.speed) * k(12.0);
        a.run_w += ((stance == Stance::Run) as u8 as f32 - a.run_w) * k(8.0);
        a.sneak_w += ((stance == Stance::Sneak) as u8 as f32 - a.sneak_w) * k(8.0);
        a.move_w += (smooth(0.15, 0.8, a.speed) - a.move_w) * k(10.0);
        let aiming = local.map_or(view.aiming, |m| m.aiming) && !view.dead && !view.asleep;
        a.aim_w += (aiming as u8 as f32 - a.aim_w) * k(12.0);
        // A gesture (#67): eased in while the server says so, and out after.
        let gesturing = view.gesture.filter(|_| !view.dead && !view.asleep);
        if let Some(g) = gesturing {
            if a.gesture != Some(g) || a.gesture_w < 0.05 {
                a.gesture_t = 0.0;
            }
            a.gesture = Some(g);
        }
        a.gesture_w += (gesturing.is_some() as u8 as f32 - a.gesture_w) * k(9.0);
        a.gesture_t += dt;
        // Tied hands, and hands at work on another survivor (#73).
        let up = !view.dead && !view.asleep;
        a.tied_w += ((view.restrained && up) as u8 as f32 - a.tied_w) * k(9.0);
        a.reach_w += ((view.hands.is_some() && up) as u8 as f32 - a.reach_w) * k(9.0);
        a.struggling = view.struggling && up;
        let seat = view.seated.filter(|_| up);
        if seat.is_some() {
            a.seat = seat;
        }
        a.sit_w += (seat.is_some() as u8 as f32 - a.sit_w) * k(7.0);
        // Its item job under way, where we know which (our own survivor's; single-player: anyone's).
        let task = link.own(a.player).and_then(|v| v.jobs.first()).filter(|_| up && view.busy > 0.0).and_then(|j| Task::of(j, &join));
        if let Some(t) = task {
            if a.task != Some(t) || a.task_w < 0.05 {
                a.task_t = 0.0;
            }
            a.task = Some(t);
        }
        a.task_w += (task.is_some() as u8 as f32 - a.task_w) * k(9.0);
        a.task_t += dt;
        a.died = view.dead;
        // Through a window or over a fence: how far across goes by where the body is, so the clip
        // keeps pace with the slow climb (and runs back if it backs out).
        let cross = crossing(&map.0, pos).filter(|_| up);
        if let Some((kind, d)) = cross {
            if a.cross != Some(kind) || a.cross_w < 0.05 {
                a.cross_side = if d < 0.0 { -1.0 } else { 1.0 };
            }
            a.cross = Some(kind);
            a.cross_u = (0.5 - a.cross_side * d / (2.0 * zombye_sim::map::CLIMB_ZONE)).clamp(0.0, 1.0);
        }
        a.cross_w += (cross.is_some() as u8 as f32 - a.cross_w) * k(14.0);
        // On a flight of stairs: which way goes by how far up it the body is and was.
        let flight = map.0.stair_at(pos).filter(|_| up && view.level == 0).map(|s| s.progress(pos));
        if let Some(p) = flight {
            match a.stair_p {
                // Just come onto it: from the foot going up, from the top coming down.
                None => a.stair = Some(p < 0.5),
                Some(was) if (p - was).abs() > 1e-4 => a.stair = Some(p > was),
                _ => {}
            }
        }
        a.stair_p = flight;
        a.stair_w += ((flight.is_some() && a.speed > 0.3) as u8 as f32 - a.stair_w) * k(10.0);
        a.stair_u = (a.stair_u + a.speed * dt / STAIR_CYCLE_M).fract();
        let cycle_m = match (&a.look, &model) {
            (Look::Model(_), Some(m)) => m.cycle_metres(a),
            _ => {
                let [walk_m, run_m, sneak_m] = a.strides;
                mix(mix(walk_m, run_m, a.run_w), sneak_m, a.sneak_w)
            }
        };
        a.cycle = (a.cycle + a.speed * dt / cycle_m).fract();
        a.idle = (a.idle + dt * 0.3).fract();

        // On a multiplayer server our own action begins on the click (#23, `LocalPlayer::act`):
        // our survivor is predicted, so the blow lands on screen when and where the server lands
        // it, and the server tests the targets where this picture has them (lag compensation).
        // Everything the snapshot says of our own timing is `lag` old by our survivor's clock.
        let lag = local.map_or(0.0, |m| m.view_lag(&link, view));
        if let Some(s) = local.and_then(|m| m.swing).filter(|s| link.net.is_some() && s.seq != a.own_taken) {
            a.own_taken = s.seq;
            a.own = Some(s.seq);
            a.own_confirmed = false;
            a.begun += 1;
            a.action = Some(ActionClock { kind: s.kind, n: a.begun, start: s.at, land: s.at + s.windup, end: Some(s.at + s.total) });
        }
        // The action under way, timed from snapshots.
        if link.curr.tick != a.last_tick {
            a.last_tick = link.curr.tick;
            match a.own {
                // A snapshot from before the click says nothing about this action.
                Some(seq) if view.last_input_seq < seq => {}
                Some(_) => match (view.action, &mut a.action) {
                    // The server has it under way: its timing, moved to our survivor's clock.
                    (Some(kind), Some(c)) if c.kind == kind => {
                        a.own_confirmed = true;
                        c.land = now + view.action_left - lag;
                        c.end = Some((now + view.ready_in - lag).max(c.land + 0.1));
                    }
                    _ => {
                        // Landed there, or never taken (we were hit first, or too soon after the
                        // last one): from here the snapshots say what we do.
                        if !a.own_confirmed && view.action.is_none() && view.ready_in <= 0.0 {
                            a.action = None;
                        }
                        a.own = None;
                    }
                },
                None => match view.action {
                    Some(kind) => match &mut a.action {
                        Some(c) if c.kind == kind && c.end.is_none() => c.land = now + view.action_left - lag,
                        _ => {
                            a.begun += 1;
                            a.action = Some(ActionClock { kind, n: a.begun, start: now - TICK_DT, land: now + view.action_left - lag, end: None });
                        }
                    },
                    None => {
                        if let Some(c) = &mut a.action
                            && c.end.is_none()
                        {
                            c.land = c.land.min(now);
                            c.end = Some(now + (view.ready_in - lag).max(0.15));
                        }
                    }
                },
            }
        }
        if a.action.is_none() {
            a.own = None;
        }
        if a.action.is_some_and(|c| c.end.is_some_and(|e| now > e)) {
            a.action = None;
        }
        if events.0.iter().any(|e| matches!(e.kind, CombatEventKind::PlayerHurt | CombatEventKind::PlayerShoved) && e.player == a.player) {
            a.hurt_at = now;
        }
        // Knocked down by another player's blow (PvP): it falls as the dead do and lies there, and
        // gets up over the last moment of it (as a sleeper does on waking).
        let floored = view.knock.filter(|k| k.down);
        // Asleep, a survivor lies still on its back (#93): its own short lie-down and get-up.
        if view.dead {
            if a.sleeping || a.woke_at.is_some() {
                a.dead_since = None;
            }
            a.sleeping = false;
            a.woke_at = None;
            if a.dead_since.is_none() {
                a.dead_since = Some(if fresh { now - 10.0 } else { now });
            }
        } else if view.asleep {
            a.woke_at = None;
            if a.dead_since.is_none() || !a.sleeping {
                a.dead_since = Some(if fresh { now - 10.0 } else { now });
            }
            a.sleeping = true;
        } else if floored.is_some_and(|k| k.timer > SLEEP_GET_UP_SECS) && a.woke_at.is_none() {
            if a.dead_since.is_none() || a.sleeping {
                a.dead_since = Some(if fresh { now - 10.0 } else { now });
            }
            a.sleeping = false;
            a.action = None;
        } else if a.dead_since.is_some() {
            if floored.is_some() && !a.sleeping {
                // Nearly over: up it gets.
                a.sleeping = true;
                a.woke_at = Some(now);
            }
            if a.sleeping && a.woke_at.is_none() {
                a.woke_at = Some(now);
                a.action = None;
            }
            if !a.sleeping || a.woke_at.is_some_and(|w| now - w > SLEEP_GET_UP_SECS) {
                a.dead_since = None;
                a.sleeping = false;
                a.woke_at = None;
                a.action = None;
            }
        }

        if let Some(pose) = options.pose.as_deref() {
            // Every survivor, each the next step through the cycle (a strip from players in a row).
            let players = &link.curr.players;
            let k = players.iter().position(|p| p.id == a.player).unwrap_or(0);
            hold_pose(a, pose, k as f32 / players.len().max(1) as f32, now);
        }
        let category = join.weapons.get(view.weapon.map_or(usize::MAX, |w| w as usize)).map_or(WeaponCategory::Unarmed, |w| w.category);
        let armed = view.weapon.is_some();
        match &a.look {
            Look::Grey(js) => {
                let pose = compose(a, now, style(category), armed);
                for (b, (parent, pivot)) in BONES.iter().enumerate() {
                    if let Ok(mut jt) = joints.get_mut(js[b]) {
                        let local_pivot = match parent {
                            Some(pa) => *pivot - BONES[*pa].1,
                            None => *pivot + pose.offset,
                        };
                        jt.translation = local_pivot;
                        jt.rotation = pose.rot[b];
                    }
                }
            }
            Look::Model(js) => {
                let Some(model) = &model else { continue };
                let pose = model.compose(a, now, category, armed);
                for (b, e) in js.iter().take(pose.t.len()).enumerate() {
                    if let Ok(mut jt) = joints.get_mut(*e) {
                        jt.translation = pose.t[b];
                        jt.rotation = pose.r[b];
                    }
                }
                for (e, t) in js.iter().skip(pose.t.len()).zip(model.hang_joints(&pose)) {
                    if let Ok(mut jt) = joints.get_mut(*e) {
                        *jt = t;
                    }
                }
            }
        }
    }
}

/// `--pose NAME[:U]`: hold survivors in one pose, to review animation in offscreen pictures (where
/// nothing moves). `walk`, `run` and `sneak` at `U` of the way through the locomotion cycle (0..1);
/// `swing`, `shove` and `stomp` at `U` of the way through the action (the blow lands at 0.45);
/// `idle`, `dead`. Each survivor is `step` further along (`U` wraps).
fn hold_pose(a: &mut SurvivorAnim, pose: &str, step: f32, now: f32) {
    // `+aim` anywhere: aiming (the guard).
    a.aim_w = pose.contains("+aim") as u8 as f32;
    let pose = pose.replace("+aim", "");
    // `NAME@DEG[:U]`: moving DEG degrees off where it faces (90 to its left, -90 its right, 180 back).
    let (pose, deg) = match pose.split_once('@') {
        Some((n, rest)) => {
            let (d, u) = rest.split_once(':').map_or((rest, None), |(d, u)| (d, Some(u)));
            (u.map_or(n.to_string(), |u| format!("{n}:{u}")), d.parse::<f32>().unwrap_or(0.0))
        }
        None => (pose.to_string(), 0.0),
    };
    a.move_angle = deg.to_radians();
    let pose = pose.as_str();
    let (name, u) = pose.split_once(':').map_or((pose, 0.25), |(n, u)| (n, u.parse::<f32>().unwrap_or(0.25).clamp(0.0, 1.0)));
    let u = (u + step).fract();
    let locomotion = |a: &mut SurvivorAnim, run: f32, sneak: f32, moving: f32| {
        (a.run_w, a.sneak_w, a.move_w, a.cycle) = (run, sneak, moving, u);
        a.gesture_w = 0.0;
        (a.tied_w, a.reach_w, a.struggling) = (0.0, 0.0, false);
        a.action = None;
        a.dead_since = None;
        a.sleeping = false;
        a.woke_at = None;
        (a.task, a.task_w, a.died) = (None, 0.0, false);
        (a.cross, a.cross_w) = (None, 0.0);
        (a.seat, a.sit_w) = (None, 0.0);
        a.hurt_at = -100.0;
    };
    let act = |a: &mut SurvivorAnim, kind: ActionKind| {
        locomotion(a, 0.0, 0.0, 0.0);
        // `swing_b`: the how-manyth action that plays a swing's second variant.
        let n = if name == "swing_b" { (0..).find(|&n| crate::survivor_model::variant(n)).unwrap_or(0) } else { a.begun };
        // Started, landing and recovering so that the clip is at `u` now.
        a.action = Some(if u <= U_HIT {
            ActionClock { kind, n, start: now - u, land: now + (U_HIT - u), end: None }
        } else {
            ActionClock { kind, n, start: now - 1.0, land: now - (u - U_HIT), end: Some(now + (1.0 - u)) }
        });
    };
    match name {
        "walk" => locomotion(a, 0.0, 0.0, 1.0),
        "run" => locomotion(a, 1.0, 0.0, 1.0),
        "sneak" => locomotion(a, 0.0, 1.0, 1.0),
        "swing" | "swing_b" => act(a, ActionKind::Swing),
        "shove" => act(a, ActionKind::Shove),
        "stomp" => act(a, ActionKind::Stomp),
        "dead" => {
            locomotion(a, 0.0, 0.0, 0.0);
            a.dead_since = Some(now - 10.0);
            a.died = true;
        }
        // Dying, `U` of a second into it; hurt, `U` of a second after the blow.
        "die" => {
            locomotion(a, 0.0, 0.0, 0.0);
            a.dead_since = Some(now - u);
            a.died = true;
        }
        "hurt" => {
            locomotion(a, 0.0, 0.0, 0.0);
            a.hurt_at = now - u;
        }
        // Hands busy (a body's own clips), `U` x 4 s into it.
        "eat" | "drink" | "loot" | "reload" => {
            locomotion(a, 0.0, 0.0, 0.0);
            let task = match name {
                "eat" => Task::Eat,
                "drink" => Task::Drink,
                "loot" => Task::Loot,
                _ => Task::Reload,
            };
            (a.task, a.task_w, a.task_t) = (Some(task), 1.0, u * 4.0);
        }
        // Through a window, over a fence: `U` of the way across.
        "climb" | "vault" => {
            locomotion(a, 0.0, 0.0, 0.0);
            (a.cross, a.cross_w, a.cross_u) = (Some(if name == "climb" { Cross::Climb } else { Cross::Vault }), 1.0, u);
        }
        // On a flight of stairs: `U` of the loop.
        "stairs" | "stairs_down" => {
            locomotion(a, 0.0, 0.0, 1.0);
            (a.stair, a.stair_w, a.stair_u) = (Some(name == "stairs"), 1.0, u);
        }
        // Sitting down: on a seat (a sofa's height) or on the ground.
        "sit" | "sit_ground" => {
            locomotion(a, 0.0, 0.0, 0.0);
            let kind = if name == "sit" { zombye_sim::SeatKind::Sofa } else { zombye_sim::SeatKind::Ground };
            (a.seat, a.sit_w) = (Some(kind), 1.0);
        }
        "sleep" => {
            locomotion(a, 0.0, 0.0, 0.0);
            a.dead_since = Some(now - 10.0);
            a.sleeping = true;
        }
        // The gestures (#67), `U` of a second into them (the wave swings once a second).
        "wave" | "handsup" | "hands_up" | "point" => {
            locomotion(a, 0.0, 0.0, 0.0);
            (a.gesture, a.gesture_w, a.gesture_t) = (Gesture::parse(name), 1.0, u);
        }
        // Hands tied behind the back, and hands at work on another survivor (#73).
        "tied" => {
            locomotion(a, 0.0, 0.0, 0.0);
            a.tied_w = 1.0;
        }
        "tying" => {
            locomotion(a, 0.0, 0.0, 0.0);
            (a.reach_w, a.gesture_t) = (1.0, u);
        }
        _ => locomotion(a, 0.0, 0.0, 0.0),
    }
}

/// Upper-body bones an action takes over.
pub(crate) const UPPER: [usize; 7] = [TORSO, HEAD, UARM_L, FARM_L, UARM_R, FARM_R, HAND_R];
const ALL: [usize; N] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

/// Standing and moving blended by stance and speed.
pub(crate) fn locomotion(a: &SurvivorAnim, armed: bool) -> SPose {
    let standing = idle(a.idle, armed).blend(&crouch(0.0, armed, 0.0), a.sneak_w);
    let moving = walk(a.cycle, armed).blend(&run(a.cycle, armed), a.run_w).blend(&crouch(a.cycle, armed, 1.0), a.sneak_w);
    standing.blend(&moving, a.move_w)
}

/// The action under way: its pose, how much of the body it takes (the stomp all of it, the rest the
/// upper body) and its weight now.
/// The action under way: its kind, how far through it is (0 to 1, the blow at [`U_HIT`]) and how much
/// it shows (eased in and out).
pub(crate) fn action_clock(a: &SurvivorAnim, now: f32) -> Option<(ActionKind, u32, f32, f32)> {
    let c = a.action?;
    let u = c.u(now);
    Some((c.kind, c.n, u, smooth(0.0, 0.06, u) * (1.0 - smooth(0.88, 1.0, u))))
}

pub(crate) fn action_now(a: &SurvivorAnim, now: f32, style: Style, armed: bool) -> Option<(SPose, bool, f32)> {
    let c = a.action?;
    let u = c.u(now);
    let w = smooth(0.0, 0.06, u) * (1.0 - smooth(0.88, 1.0, u));
    // From the stance it's in: the guard when aiming.
    let ready = idle(0.0, armed).blend(&guard(style, armed), a.aim_w);
    Some((action(c.kind, style, armed, u, &ready), c.kind == ActionKind::Stomp, w))
}

/// Aiming (the right mouse held): at the ready, leaning in. Bare-handed, fists up; with a one-handed
/// weapon, it raised beside the head and the free hand up to fend; with a two-handed one, both hands on
/// it, held up across the body.
pub(crate) fn guard(style: Style, armed: bool) -> SPose {
    let mut p = idle(0.0, armed);
    p.rot[TORSO] = rx(0.08);
    p.rot[HEAD] = rx(-0.22);
    match (style, armed) {
        (Style::Gun, true) => {
            // Both arms out along the aim, the gun level in front of the chest.
            p.rot[TORSO] = rx(0.04);
            p.rot[HEAD] = rx(-0.08);
            p.rot[UARM_R] = rz(0.14) * rx(-1.42);
            p.rot[FARM_R] = rx(-0.08);
            p.rot[HAND_R] = rx(1.5);
            p.rot[UARM_L] = rz(-0.3) * rx(-1.36);
            p.rot[FARM_L] = rx(-0.22);
        }
        (Style::TwoHand, true) => {
            p.rot[UARM_R] = rz(-0.25) * rx(-0.75);
            p.rot[FARM_R] = rx(-1.45);
            p.rot[HAND_R] = rx(0.75);
            p.rot[UARM_L] = rz(-0.35) * rx(-0.85);
            p.rot[FARM_L] = rx(-1.35);
        }
        (Style::OneHand, true) => {
            p.rot[UARM_R] = rz(-0.25) * rx(-0.85);
            p.rot[FARM_R] = rx(-1.4);
            p.rot[HAND_R] = rx(0.35);
            p.rot[UARM_L] = rz(0.12) * rx(-0.6);
            p.rot[FARM_L] = rx(-1.7);
        }
        _ => {
            // Elbows in, forearms up in front of the chest, fists at chin height.
            p.rot[UARM_L] = rz(-0.12) * rx(-0.3);
            p.rot[FARM_L] = ry(-0.5) * rx(-1.75);
            p.rot[UARM_R] = rz(0.12) * rx(-0.3);
            p.rot[FARM_R] = ry(0.5) * rx(-1.75);
        }
    }
    p
}

/// The bones a gesture takes: the free (left) arm for a wave or a point, both arms for hands up.
pub(crate) const LEFT_ARM: [usize; 2] = [UARM_L, FARM_L];
pub(crate) const ARMS: [usize; 5] = [UARM_L, FARM_L, UARM_R, FARM_R, HAND_R];

/// The gesture under way (#67): its pose, whether it takes both arms (else the left), and its
/// weight now. The left hand is the free one (the right holds the weapon), so that one waves and
/// points; hands up raises both, whatever is in them.
pub(crate) fn gesture_now(a: &SurvivorAnim) -> Option<(SPose, bool, f32)> {
    let g = a.gesture.filter(|_| a.gesture_w > 0.01)?;
    let mut p = SPose::default();
    match g {
        Gesture::Wave => {
            // The arm up beside the head, the forearm swinging from side to side.
            let s = (a.gesture_t * TAU * 1.6).sin();
            p.rot[UARM_L] = rz(0.75) * rx(-2.55);
            p.rot[FARM_L] = rz(0.2 + 0.5 * s) * rx(-0.35);
        }
        Gesture::HandsUp => {
            // Both arms up and a little out, open, elbows soft.
            p.rot[UARM_L] = rz(0.85) * rx(-2.5);
            p.rot[FARM_L] = rx(-0.5);
            p.rot[UARM_R] = rz(-0.85) * rx(-2.5);
            p.rot[FARM_R] = rx(-0.5);
            p.rot[HAND_R] = rx(-0.1);
        }
        Gesture::Point => {
            // The arm straight out the way it faces, jabbed once as it comes up.
            let jab = 0.12 * (1.0 - smooth(0.0, 0.5, a.gesture_t));
            p.rot[UARM_L] = rz(-0.05) * rx(-1.5 + jab);
            p.rot[FARM_L] = rx(-0.06);
        }
    }
    Some((p, g == Gesture::HandsUp, a.gesture_w))
}

/// Hands tied behind the back, or at work on another survivor (#73): the pose of both arms and
/// its weight now. Tied wins: a tied survivor does nothing else with its hands.
pub(crate) fn bonds_now(a: &SurvivorAnim) -> Option<(SPose, f32)> {
    let mut p = SPose::default();
    if a.tied_w > 0.01 {
        // Both arms drawn back and in, wrists crossed at the small of the back; struggling, the
        // shoulders work from side to side.
        let s = if a.struggling { (a.gesture_t * TAU * 2.2).sin() * 0.12 } else { 0.0 };
        p.rot[UARM_L] = rz(-0.22 + s) * rx(0.62);
        p.rot[FARM_L] = rz(-0.75) * rx(0.25);
        p.rot[UARM_R] = rz(0.22 + s) * rx(0.62);
        p.rot[FARM_R] = rz(0.75) * rx(0.25);
        return Some((p, a.tied_w));
    }
    if a.reach_w > 0.01 {
        // Both hands out in front at waist height, working.
        let s = (a.gesture_t * TAU * 1.8).sin() * 0.1;
        p.rot[UARM_L] = rz(-0.1) * rx(-0.95 + s);
        p.rot[FARM_L] = rx(-0.55 - s);
        p.rot[UARM_R] = rz(0.1) * rx(-0.95 - s);
        p.rot[FARM_R] = rx(-0.55 + s);
        return Some((p, a.reach_w));
    }
    None
}

/// How far into the hurt flinch it is now (0 = none).
pub(crate) fn flinch_weight(a: &SurvivorAnim, now: f32) -> f32 {
    let e = now - a.hurt_at;
    if !(0.0..FLINCH_SECS).contains(&e) {
        return 0.0;
    }
    if e < 0.05 { e / 0.05 } else { (1.0 - (e - 0.05) / (FLINCH_SECS - 0.05)).powi(2) }
}

/// The full pose of a survivor now.
fn compose(a: &SurvivorAnim, now: f32, style: Style, armed: bool) -> SPose {
    let mut pose = locomotion(a, armed);
    pose.blend_bones(&guard(style, armed), a.aim_w, &UPPER, false);
    if let Some((g, both, w)) = gesture_now(a) {
        pose.blend_bones(&g, w, if both { &ARMS[..] } else { &LEFT_ARM[..] }, false);
    }
    if let Some((b, w)) = bonds_now(a) {
        pose.blend_bones(&b, w, &ARMS[..], false);
    }
    if let Some((act, whole, w)) = action_now(a, now, style, armed) {
        pose.blend_bones(&act, w, if whole { &ALL } else { &UPPER }, false);
    }
    // A flinch on top of whatever it's doing.
    let w = flinch_weight(a, now);
    if w > 0.0 {
        let f = flinch();
        for b in UPPER {
            pose.rot[b] *= Quat::IDENTITY.slerp(f.rot[b], w);
        }
    }
    ground(&mut pose);
    // Running has a moment in the air each step.
    pose.offset.y += a.run_w * a.move_w * 0.05 * (a.cycle * TAU * 2.0).sin().max(0.0);
    if let Some((sit, w)) = sitting_now(a) {
        pose = pose.blend(&sit, w);
    }
    if let Some((lie, w)) = lying(a, now) {
        pose = pose.blend(&lie, w);
    }
    pose
}

/// The seated pose and how much of it to blend over the rest, while sitting or getting up.
pub(crate) fn sitting_now(a: &SurvivorAnim) -> Option<(SPose, f32)> {
    let kind = a.seat.filter(|_| a.sit_w > 0.01)?;
    let mut p = seated(kind == zombye_sim::SeatKind::Ground);
    ground(&mut p);
    Some((p, a.sit_w))
}

/// Sitting: on a seat, thighs level and shins down to the floor, hands on the knees; on the
/// ground, legs out in front and leaning back a little on straight arms. Grounded by the feet
/// (on a seat that puts the hips at about a chair's height).
pub(crate) fn seated(on_ground: bool) -> SPose {
    let mut p = SPose::default();
    if on_ground {
        p.rot[THIGH_L] = rx(-1.45) * rz(0.12);
        p.rot[THIGH_R] = rx(-1.45) * rz(-0.12);
        p.rot[SHIN_L] = rx(0.25);
        p.rot[SHIN_R] = rx(0.25);
        p.rot[TORSO] = rx(0.12);
        p.rot[UARM_L] = rx(0.45) * rz(0.2);
        p.rot[UARM_R] = rx(0.45) * rz(-0.2);
        p.offset = Vec3::new(0.0, 0.0, 0.08);
    } else {
        p.rot[THIGH_L] = rx(-FRAC_PI_2) * rz(0.08);
        p.rot[THIGH_R] = rx(-FRAC_PI_2) * rz(-0.08);
        p.rot[SHIN_L] = rx(FRAC_PI_2 - 0.1);
        p.rot[SHIN_R] = rx(FRAC_PI_2 - 0.1);
        p.rot[TORSO] = rx(-0.06);
        p.rot[UARM_L] = rx(-0.35) * rz(0.1);
        p.rot[UARM_R] = rx(-0.35) * rz(-0.1);
        p.rot[FARM_L] = rx(-0.9);
        p.rot[FARM_R] = rx(-0.9);
    }
    p.rot[HEAD] = rx(-0.05);
    p
}

/// How long a sleeper takes to get up after waking.
pub(crate) const SLEEP_GET_UP_SECS: f32 = 0.8;
/// How long a sleeper takes to lie down.
const SLEEP_LIE_SECS: f32 = 1.2;

/// The pose a dead or sleeping survivor is in and how much of it to blend over its normal one.
pub(crate) fn lying(a: &SurvivorAnim, now: f32) -> Option<(SPose, f32)> {
    let since = a.dead_since?;
    let e = now - since;
    if !a.sleeping {
        return Some((dead(e), smooth(0.0, 0.12, e)));
    }
    let w = match a.woke_at {
        Some(wk) => 1.0 - smooth(0.0, SLEEP_GET_UP_SECS, now - wk),
        None => smooth(0.0, 0.3, e),
    };
    Some((sleeping(e), w))
}

/// Lying still on the back, arms down by the sides, breathing slowly; lies down over a moment.
pub(crate) fn sleeping(e: f32) -> SPose {
    let k = smooth(0.0, SLEEP_LIE_SECS, e);
    let mut end = SPose::default();
    end.rot[HEAD] = rx(-0.05);
    end.rot[UARM_L] = rz(0.12);
    end.rot[UARM_R] = rz(-0.12);
    end.rot[FARM_L] = rx(-0.1);
    end.rot[FARM_R] = rx(-0.1);
    end.rot[THIGH_L] = rz(0.06);
    end.rot[THIGH_R] = rz(-0.06);
    let mut p = idle(0.0, false).blend(&end, k);
    let theta = -FRAC_PI_2 * k;
    p.rot[PELVIS] = rx(theta);
    let hip = rx(theta) * Vec3::new(0.0, HIP, 0.0);
    p.offset = hip + Vec3::Y * (0.12 * k - HIP);
    // A slow breath: the chest rises a little.
    let b = (e * 1.3).sin() * k;
    p.rot[TORSO] *= rx(-0.03 * b);
    p.offset.y += 0.006 * b;
    p
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

/// The right arm when it isn't doing anything: a weapon held at the ready (pointing forward and
/// down), or a loose hand.
fn carry(p: &mut SPose, armed: bool, swing: f32) {
    if armed {
        p.rot[UARM_R] = rx(-0.2 - 0.1 * swing) * rz(-0.08);
        p.rot[FARM_R] = rx(-0.55);
        // Forward and down about 25 degrees at rest: tilted up, it lined up with the camera's
        // line of sight and hid end-on behind the fist of a survivor facing the camera (#35).
        p.rot[HAND_R] = rx(1.25);
    } else {
        p.rot[UARM_R] = rx(-0.38 * swing) * rz(-0.1);
        p.rot[FARM_R] = rx(-0.3);
    }
}

pub(crate) fn idle(t: f32, armed: bool) -> SPose {
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
pub(crate) fn action(kind: ActionKind, style: Style, armed: bool, u: f32, ready: &SPose) -> SPose {
    let ready = ready.clone();
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
        (ActionKind::Swing, Style::Gun) => {
            // Levelled, the shot (at `U_HIT`), the muzzle kicking up and coming back down.
            let level = guard(Style::Gun, true);
            let mut kick = level.clone();
            kick.rot[UARM_R] = rz(0.14) * rx(-1.62);
            kick.rot[FARM_R] = rx(-0.3);
            kick.rot[HAND_R] = rx(1.25);
            kick.rot[UARM_L] = rz(-0.3) * rx(-1.5);
            kick.rot[TORSO] = rx(-0.03);
            keyed(&[(0.0, ready.clone()), (U_HIT * 0.6, level.clone()), (U_HIT, level), (0.56, kick), (1.0, ready)], u)
        }
        (ActionKind::Shove, style) if armed && style != Style::Gun => {
            // A melee weapon in hand: held across the chest in both hands like a bar and driven
            // straight out from the body. The bar points along the hand's +Z; a quarter turn about
            // the hand's Y lays it level across to his left hand, whatever the arm's lift (the
            // arm's lift is about X), and a little more takes out the arm's inward turn.
            let bar = ry(FRAC_PI_2 + 0.18);
            let gather = upper(rx(-0.05), q, rz(-0.18) * rx(-0.55), rx(-1.75), rz(0.18) * rx(-0.55), rx(-1.75), bar);
            // The arms' lift is under the torso's lean, which swings them back as much: so they're
            // raised by it more, to reach out level at the shoulders with the bar at chest height.
            let push = upper(rx(0.2), rx(-0.12), rz(-0.2) * rx(-1.72), rx(-0.08), rz(0.2) * rx(-1.72), rx(-0.08), bar);
            let hold = upper(rx(0.15), rx(-0.08), rz(-0.18) * rx(-1.55), rx(-0.25), rz(0.18) * rx(-1.55), rx(-0.25), bar);
            keyed(&[(0.0, ready.clone()), (0.25, gather), (U_HIT, push.clone()), (0.62, hold), (1.0, ready)], u)
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
pub(crate) fn flinch() -> SPose {
    let q = Quat::IDENTITY;
    upper(rx(-0.28) * ry(0.12), rx(-0.35), rz(0.25) * rx(-0.45), q, rz(-0.2) * rx(-0.35), q, q)
}

/// Dead: falls onto its back over [`DEAD_FALL_SECS`] (pivoting about the feet), then lies still.
pub(crate) fn dead(e: f32) -> SPose {
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

/// What a survivor's left hand holds while an item job is under way: the food or drink being
/// had, the pill or dressing being used, the magazine or round going into the gun (#35). Known
/// where we know its jobs (our own survivor's; single-player: anyone's), as the hands' clips are.
fn busy_with(link: &ServerLink, join: &JoinInfo, player: u32) -> Option<u16> {
    let view = link.curr.players.iter().find(|p| p.id == player)?;
    if view.dead || view.asleep || view.busy <= 0.0 {
        return None;
    }
    let by_id = |id: &str| join.items.iter().position(|d| d.id == id).map(|i| i as u16);
    match link.own(player)?.jobs.first()? {
        JobKind::Use { item, .. } => Some(*item),
        JobKind::Treat { item, .. } => *item,
        JobKind::Reload { spare } => {
            let ranged = join.weapons.get(view.weapon? as usize)?.ranged.as_ref()?;
            match (&ranged.magazine, spare) {
                (Some(mag), false) => by_id(mag),
                _ => by_id(&ranged.ammo),
            }
        }
        _ => None,
    }
}

/// Show what each survivor holds in either hand (from snapshots; while eating, drinking, taking a
/// pill or reloading, the thing in use in the left hand): see `WeaponLooks::in_hand`.
fn show_held_items(
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    looks: Option<Res<WeaponLooks>>,
    mut commands: Commands,
    mut held: Query<(Entity, &HeldItem, &mut Visibility, Option<&Children>)>,
    mut shapes: Query<(&mut Transform, &mut MeshMaterial3d<StandardMaterial>, &mut Mesh3d), Without<HeldItem>>,
) {
    let Some(looks) = looks else { return };
    for (e, h, mut vis, children) in &mut held {
        let alive = link.curr.players.iter().any(|p| p.id == h.player && !p.dead);
        let outfit = link.curr.outfits.iter().find(|o| o.id == h.player).filter(|_| alive);
        let busy = if h.hand == 1 { busy_with(&link, &join, h.player) } else { None };
        let item = busy.or_else(|| outfit.and_then(|o| if h.hand == 0 { o.hand } else { o.off_hand }));
        let look = item.and_then(|i| looks.in_hand(i));
        let want = if look.is_some() { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        let Some((mesh, material, mut place)) = look else { continue };
        // Food at the mouth sits in front of it (`held::EATING`), unless its grip says otherwise.
        if let Some(def) = busy.and_then(|i| join.items.get(i as usize))
            && def.category == ItemCategory::Food
            && !crate::held::has(&def.id)
        {
            place = Transform::from_matrix(crate::held::EATING.transform().to_matrix() * place.to_matrix());
        }
        match children.and_then(|c| c.first().copied()) {
            Some(child) => {
                if let Ok((mut tf, mut m, mut shape)) = shapes.get_mut(child) {
                    if *tf != place {
                        *tf = place;
                    }
                    if m.0 != material {
                        m.0 = material;
                    }
                    if shape.0 != mesh {
                        shape.0 = mesh;
                    }
                }
            }
            None => {
                commands.spawn((Mesh3d(mesh), MeshMaterial3d(material), place, ChildOf(e)));
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

    /// A gun is held out level along the aim, and kicks up after the shot (#72).
    #[test]
    fn a_gun_is_levelled_and_kicks() {
        let barrel = |p: &SPose| (globals(&BONES, p)[HAND_R].transform_vector3(Vec3::Z), globals(&BONES, p)[HAND_R].transform_point3(Vec3::ZERO));
        let (dir, hand) = barrel(&guard(Style::Gun, true));
        assert!(dir.z > 0.95 && dir.y.abs() < 0.2, "aiming, the barrel points {dir}");
        assert!(hand.z > 0.4 && (1.2..1.6).contains(&hand.y), "held out at shoulder height: {hand}");
        let ready = guard(Style::Gun, true);
        let (shot, _) = barrel(&action(ActionKind::Swing, Style::Gun, true, U_HIT, &ready));
        let (kick, _) = barrel(&action(ActionKind::Swing, Style::Gun, true, 0.56, &ready));
        assert!(shot.z > 0.95, "level as the shot goes: {shot}");
        assert!(kick.y > shot.y + 0.2 && kick.z > 0.5, "then the muzzle rises: {kick}");
        assert_eq!(style(WeaponCategory::Firearm), Style::Gun);
    }

    /// A shove with a weapon in hand holds it level across the body like a bar and drives it out.
    #[test]
    fn a_weapon_shove_pushes_with_it_across_the_body() {
        for style in [Style::OneHand, Style::TwoHand] {
            let at = |u: f32| {
                let p = action(ActionKind::Shove, style, true, u, &idle(0.0, true));
                let g = globals(&BONES, &p);
                (g[HAND_R].transform_vector3(Vec3::Z), g[HAND_R].transform_point3(Vec3::ZERO))
            };
            let (gather_dir, gather) = at(0.25);
            let (push_dir, push) = at(U_HIT);
            for dir in [gather_dir, push_dir] {
                assert!(dir.x > 0.9 && dir.y.abs() < 0.2, "{style:?}: the bar lies level to his left, not {dir}");
            }
            assert!(push.z > gather.z + 0.2, "{style:?}: driven out from {gather} to {push}");
            // Out at chest height, not down at the waist: the hands no lower than a little under
            // the shoulder, and as far out in front as the arm is long, near enough.
            let p = action(ActionKind::Shove, style, true, U_HIT, &idle(0.0, true));
            let g = globals(&BONES, &p);
            let shoulder = g[UARM_R].transform_point3(Vec3::ZERO);
            assert!(push.y > shoulder.y - 0.2, "{style:?}: hands at {push} under a shoulder at {shoulder}");
            assert!(push.z - shoulder.z > 0.45, "{style:?}: hands at {push} not out in front of {shoulder}");
        }
    }

    /// The weapon points backward in the windup and forward when the blow lands.
    #[test]
    fn swings_wind_up_and_land_in_front() {
        for style in [Style::OneHand, Style::TwoHand] {
            let dir = |u: f32| {
                let p = action(ActionKind::Swing, style, true, u, &idle(0.0, true));
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
        let c = ActionClock { kind: ActionKind::Swing, n: 0, start: 1.0, land: 1.5, end: Some(2.0) };
        assert_eq!(c.u(1.0), 0.0);
        assert!((c.u(1.5) - U_HIT).abs() < 1e-6);
        assert_eq!(c.u(3.0), 1.0);
    }
}
