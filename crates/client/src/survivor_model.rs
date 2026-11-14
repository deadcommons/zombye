//! Survivors drawn as the modular character (#62, #63): the base body ([`crate::character`]) on its
//! own skeleton, with what each one wears drawn on the same skeleton, posed on the CPU every frame
//! (only a few survivors are ever on screen) and skinned on the GPU.
//!
//! **Animation.** Locomotion is the body's own clips (idle, walk, run, sneak), sampled on the
//! locomotion cycle [`crate::survivor`] keeps (it advances with the distance actually moved; each
//! clip's stride is measured from its planted feet, as `zvat.py` does) and blended by stance and
//! speed. The rest is the grey-box's poses, carried over onto this skeleton ([`Retarget`]): swings,
//! shoves and stomps where the body has no clip of its own (timed to the server as before), the
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
//! the release build's own (`release_chars.rs`), else
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
use zombye_sim::items::WeaponCategory;
use zombye_sim::{BodyPart, ItemDef, WearSlot};

use crate::held::Carry;
use crate::character::{Body, HAIR_REGION, REGIONS, Rig, RigPose, SkinMesh, Wearable, parse_body, parse_wearable};
use crate::rig::{globals, smooth};
use crate::server_link::{JoinInfo, ServerLink};
use crate::survivor::{
    self, FARM_L, FARM_R, HAND_R, HEAD, HIP, HeldItem, JACKET_ME, JACKET_OTHER, PELVIS, SHIN_L, SHIN_R, SPose, Style, SurvivorAnim, THIGH_L,
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
/// (the owner). About 2.9 steps a second walking; a run steps at a sprint's 3.7 a second (a real
/// sprint at 5.5 m/s does, and a slower cadence stretched a jog's legs past what looks human); the
/// sneak as made.
const CADENCE: [f32; 3] = [1.45, 1.85, 0.9];
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
/// The action clips a body can carry (#62): each made in place, starting and ending near the idle's
/// first pose, the blow landing 45% of the way through ([`survivor`]'s `U_HIT`). After the first six,
/// second swings played in turn at random with the first (`swing_one_b`, `swing_two_b`), a knife's
/// thrust (`stab`) and the blow down at a zombie on the ground with a weapon in hand, one-handed
/// (`ground_one`) and two-handed (`ground_two`); bare hands stomp. A body without one plays the
/// clip it stands in for ([`action_clips`]).
const ACTION_CLIPS: [&str; 12] = [
    "punch", "swing_one", "swing_two", "shot", "shove", "stomp", "swing_one_b", "swing_two_b", "stab", "ground_one", "ground_two",
    "shove_armed",
];

/// More clips a body can carry, each played instead of nothing or of a grey-box pose: `hurt` (a blow
/// taken: once, over the upper body, in place of the flinch), `die` (once, ending on the ground, in
/// place of the fall onto the back) and the loops for busy hands, `eat`, `drink`, `loot` (searching
/// a container) and `reload` (the whole body standing, the upper body over the gait when moving).
const LIFE_CLIPS: [&str; 6] = ["hurt", "die", "eat", "drink", "loot", "reload"];
/// And for getting through a window (`climb`) and over a fence (`vault`): made in place, played
/// once over the crossing by how far across he is, the whole body. Without one he walks through.
const CROSS_CLIPS: [&str; 2] = ["climb", "vault"];
/// And for a flight of stairs: loops of two steps made in place (no rise, no travel), played by
/// the distance moved (`survivor::STAIR_CYCLE_M` a loop) in place of the gait, the whole body.
/// Without `stairs_down` he walks down; without either he walks both ways.
const STAIR_CLIPS: [&str; 2] = ["stairs_up", "stairs_down"];
/// And for sitting down: loops made in place, on a chair-high seat (`sit`) and on the ground
/// (`sit_ground`), the whole body, its feet on the floor. Without them the grey-box's.
const SIT_CLIPS: [&str; 2] = ["sit", "sit_ground"];
const HURT: usize = 0;
const DIE: usize = 1;

/// The gun's place among the guard clips ([`guard_slot`]).
const GUARD_GUN: usize = 3;

/// Which of the body's guard clips aiming holds: bare hands, a one-handed weapon, a two-handed one,
/// a gun.
fn guard_slot(style: Style, armed: bool) -> usize {
    match style {
        Style::Gun if armed => GUARD_GUN,
        Style::TwoHand if armed => 2,
        Style::Punch => 0,
        _ if !armed => 0,
        _ => 1,
    }
}

/// The clips of [`ACTION_CLIPS`] that may play an action, by how the held weapon is swung, best
/// first: the first one the body has plays. `n` is the how-manyth action ([`survivor::ActionClock`]),
/// and picks between a swing's two variants.
fn action_clips(kind: zombye_sim::ActionKind, category: WeaponCategory, n: u32) -> &'static [usize] {
    use zombye_sim::ActionKind::*;
    let other = variant(n);
    match (kind, survivor::style(category)) {
        (Swing, Style::Punch) => &[0],
        (Swing, Style::OneHand) if category == WeaponCategory::ShortBlade => &[8, 1],
        (Swing, Style::OneHand) if other => &[6, 1],
        (Swing, Style::OneHand) => &[1],
        (Swing, Style::TwoHand) if other => &[7, 2],
        (Swing, Style::TwoHand) => &[2],
        (Swing, Style::Gun) => &[3],
        // With a melee weapon in hand the shove is the weapon held across the body like a bar and
        // driven out: the body's `shove_armed`, else the grey-box's pose (never the hands' `shove`).
        (Shove, Style::OneHand | Style::TwoHand) => &[11],
        (Shove, _) => &[4],
        (Stomp, Style::OneHand) => &[9, 5],
        (Stomp, Style::TwoHand) => &[10, 5],
        (Stomp, _) => &[5],
    }
}

/// Whether the `n`th action plays a swing's second variant: a hash of `n`, so it doesn't simply
/// alternate.
pub(crate) fn variant(n: u32) -> bool {
    let mut h = n.wrapping_mul(0x9E37_79B9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h & 1 == 1
}

fn angle_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    if d > std::f32::consts::PI { d - TAU } else { d }
}

/// How far the head turns off the chest to keep its eyes on the aim (radians: about 70°), and how
/// far down or up of level it looks through a blow at something standing.
const NECK_TURN: f32 = 1.2;
const LOOK_DOWN: f32 = 0.6;
const LOOK_UP: f32 = 0.3;

/// The head, and what says which way it and the chest face: for keeping the eyes on the aim. The
/// library clips the action clips are made from turn the whole body to bring a blow round (a
/// two-handed chop stands side-on to what it hits), and the head went with it: he swung with his
/// head looking away (the owner).
struct Gaze {
    head: usize,
    /// The head's parent, if it is the neck: it takes half of every turn.
    neck: Option<usize>,
    /// The way the face points, in the head bone's own axes (the rest pose faces +Z).
    face: Vec3,
    /// The upper arms (left, right): the chest faces square to the line between them.
    shoulders: Option<(usize, usize)>,
}

impl Gaze {
    fn new(rig: &Rig) -> Option<Self> {
        let head = rig.find("Head")?;
        let neck = rig.bones[head].parent.filter(|&p| rig.bones[p].name.eq_ignore_ascii_case("neck"));
        let rest = rig.globals(&rig.rest());
        let face = (rest[head].to_scale_rotation_translation().1.inverse() * Vec3::Z).normalize();
        Some(Self { head, neck, face, shoulders: rig.find("LeftArm").zip(rig.find("RightArm")) })
    }

    /// Which way the face points in a pose's model-space bones: its heading (radians about the
    /// vertical, 0 straight ahead, positive to his left) and how far up of level.
    fn looks(&self, g: &[Mat4]) -> (f32, f32) {
        let f = (g[self.head].to_scale_rotation_translation().1 * self.face).normalize();
        (f.x.atan2(f.z), f.y.clamp(-1.0, 1.0).asin())
    }

    /// The chest's heading (as [`Self::looks`]); straight ahead for a body with no arms to go by.
    fn chest(&self, g: &[Mat4]) -> f32 {
        let Some((l, r)) = self.shoulders else { return 0.0 };
        let fwd = (g[l].w_axis.truncate() - g[r].w_axis.truncate()).cross(Vec3::Y);
        if fwd.length_squared() < 1e-8 { 0.0 } else { fwd.x.atan2(fwd.z) }
    }

    /// Turns the head by `q` (a model-space rotation), half of it through the neck.
    fn turn(&self, rig: &Rig, pose: &mut RigPose, q: Quat) {
        let g = rig.globals(pose);
        let rot = |b: usize| g[b].to_scale_rotation_translation().1;
        let above = |b: usize| rig.bones[b].parent.map_or(Quat::IDENTITY, rot);
        let head = q * rot(self.head);
        match self.neck {
            Some(neck) => {
                let turned = Quat::IDENTITY.slerp(q, 0.5) * rot(neck);
                pose.r[neck] = (above(neck).inverse() * turned).normalize();
                pose.r[self.head] = (turned.inverse() * head).normalize();
            }
            None => pose.r[self.head] = (above(self.head).inverse() * head).normalize(),
        }
    }

    /// Eyes on the aim, by `w` (0 to 1): the head turned to look straight ahead, as far as the neck
    /// goes off the chest ([`NECK_TURN`]); `level`, also no further down or up than a blow at
    /// something standing looks ([`LOOK_DOWN`], [`LOOK_UP`]).
    fn aim(&self, rig: &Rig, pose: &mut RigPose, w: f32, level: bool) {
        if w <= 0.0 {
            return;
        }
        let g = rig.globals(pose);
        let (_, pitch) = self.looks(&g);
        let chest = self.chest(&g);
        let yaw = 0f32.clamp(chest - NECK_TURN, chest + NECK_TURN);
        let pitch = if level { pitch.clamp(-LOOK_DOWN, LOOK_UP) } else { pitch };
        let now = (g[self.head].to_scale_rotation_translation().1 * self.face).normalize();
        let want = Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos());
        self.turn(rig, pose, Quat::IDENTITY.slerp(Quat::from_rotation_arc(now, want), w.min(1.0)));
    }
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

/// How a hand holds a thing (#35), each a grip frame on the hand (the grip at the origin, +Z along
/// the thing, +Y its top): a handle in a hammer grip (a bat, a knife, a flashlight), a gun's grip,
/// or a small thing in the fist (a mug, a tin, a bottle). See [`Hand`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hold {
    Handle = 0,
    Gun = 1,
    Fist = 2,
}

/// How far off the socket, out of the palm, the grip is (metres): the socket is in the middle of
/// the hand, so a handle drawn there ran through it; its middle is this far out, a handle's
/// half-thickness off the palm's skin, where the closed fingers go round it.
pub(crate) const PALM: f32 = 0.04;
/// How far under the socket the palm's skin is (metres): what is held lies on it.
pub(crate) const SKIN: f32 = 0.015;
/// A handle in a hammer grip at rest runs across the palm at this angle to the knuckles' line, out
/// past the thumb toward the fingers (radians): from the heel of the hand to between thumb and
/// forefinger, so in a hanging hand it points forward and down, 45 degrees off the forearm.
const TILT: f32 = std::f32::consts::FRAC_PI_4;
/// And its grip is this far back from the socket toward the wrist (metres), so the handle passes
/// the forefinger's root, not through it.
const BACK: f32 = 0.02;
/// The closed hand, round a thin handle (radians): the fingers bent at the knuckles and again
/// halfway along, the thumb swung under the palm and bent in over the fingers. A thicker thing
/// closes it less ([`SurvivorModel::hold`]'s `closed`).
const CURL_FINGERS: f32 = 1.31;
const CURL_TIPS: f32 = 1.4;
const THUMB_UNDER: f32 = 0.61;
const THUMB_IN: f32 = 0.96;

/// The model-space turns of a hand's joints, closed by `closed` (0 open, 1 a fist round a thin
/// handle): the fingers' (at the knuckles), the fingertips' (halfway along, on top of the fingers')
/// and the thumb's. `axes` are the hand's socket axes in the rest pose and `out` which way its
/// fingers point along X (-1 the right hand's): palm down, thumb forward.
fn closing(axes: Quat, out: f32, closed: f32) -> [Quat; 3] {
    let (fingers, palm, thumb) = (axes * Vec3::X * out, axes * Vec3::NEG_Y, axes * Vec3::Z);
    let knuckles = fingers.cross(palm);
    [
        Quat::from_axis_angle(knuckles, CURL_FINGERS * closed),
        Quat::from_axis_angle(knuckles, CURL_TIPS * closed),
        Quat::from_axis_angle(knuckles, THUMB_IN * closed) * Quat::from_axis_angle(thumb.cross(palm), THUMB_UNDER * closed),
    ]
}

/// The joints a hand closes with, where the body has them (a body built with `fingers_r`,
/// `fingertips_r`, `thumb_r` and the same for the left: the bought one, `hand_joints.py` in the art
/// repo; no clip names them, so they keep their rest under the hand until [`SurvivorModel::hold`]
/// turns them). Each with its rest rotation in model space, to turn it about the hand's axes.
struct Fingers {
    /// (bone, its rest rotation in model space): the fingers, the fingertips, the thumb.
    joints: [Option<(usize, Quat)>; 3],
    axes: Quat,
    /// Which way the fingers point along the socket's X.
    out: f32,
}

/// Where a carry has the arm (#35), in model space (+Z forward, +Y up; `out` is the arm's side
/// along X, -1 the right's): the lines of the upper arm and of the forearm. The hand goes on in
/// line with the forearm, thumb up, palm to the body, so what stands in the fist stands upright
/// and a gun's top is up. `None` for a thing at one's side: the arm is the clips'.
/// - In front: the upper arm hangs, the elbow bent just under 90 degrees, the forearm level at the
///   waist and a little in across the body, the fist in front of the belly.
/// - Low and ready: the elbow out a little from the side and bent about 65 degrees, the forearm
///   forward and down 30 degrees and a little out, the fist in front of the hip, clear of the thigh
///   (and far enough out to show past the body from most of the way round).
fn carry_lines(carry: Carry, out: f32) -> Option<(Vec3, Vec3)> {
    match carry {
        Carry::Side => None,
        Carry::Front => Some((Vec3::new(out * 0.10, -1.0, 0.05).normalize(), Vec3::new(-out * 0.20, 0.0, 1.0).normalize())),
        Carry::Low => Some((Vec3::new(out * 0.25, -1.0, -0.10).normalize(), Vec3::new(out * 0.14, -0.577, 1.0).normalize())),
    }
}

/// The turn that takes +X to `along` and +Y as near to `toward` as it can be across it.
fn framed(along: Vec3, toward: Vec3) -> Quat {
    let toward = (toward - along * toward.dot(along)).normalize();
    Quat::from_mat3(&Mat3::from_cols(along, toward, along.cross(toward)))
}

/// One hand and its grips (#35). **The thing follows the hand**: it sits in the hand's grip frame
/// (the grip at the origin on the palm, +Z along the thing, +Y its top), which is fixed on the
/// hand's bone per [`Hold`], and the hand closes round it ([`Fingers`]). The hand's shape is read off
/// its socket in the rest pose, on the socket's axes (the model's, turned by the socket's rotation):
/// the fingers point along X away from the body, the palm faces down (-Y), the thumb is forward
/// (+Z), a T-pose's.
///
/// - A **handle** lies across the palm inside the fingers, out past the thumb, its top to the
///   wrist: at rest [`TILT`] off the knuckles' line toward the fingers, so a knife or a bat points
///   forward and down from a hanging fist; along the knuckles while a weapon's clip has the arm
///   (it shifts in the grip as the arm comes up), which is where the clips mean it to point.
/// - A **gun**'s barrel runs on the way the fingers point, its top to the thumb.
/// - A small thing in the **fist** lies along the knuckles' line, out past the thumb.
///
/// **The right hand's bone is set straight** while it holds something (`natural`). The clips are
/// made for a thing that points model-forward, top up, in the first frame of the idle, fixed on the
/// right hand's bone, and `zretarget.py` turns that bone in every clip to point it so: by the
/// shortest way from where the knuckles' line would have had it, which leaves the hand itself bent
/// off the forearm (the bought idle's by about 60 degrees, thumb up). `natural` is that turn taken
/// back out, the shortest turn that lays the knuckles' line on the clips' line again: the hand as
/// it was animated, hanging in line with the forearm, palm to the thigh, and a handle along its
/// knuckles pointing where the clips mean. Nothing else turns the wrist, at rest or in a blow.
/// (Before, the bone was turned to a whole frame, forward and top up, on top of the grey-box's
/// carry: a twisted wrist, the knife straight out from the forearm.) A gun is the exception: its
/// barrel is along the fingers, a quarter turn off the knuckles, so while a clip aims or reloads it
/// the hand is turned to keep the barrel on the clips' line (`gun`).
struct Hand {
    bone: usize,
    /// The bone's rest rotation in model space, and the socket's axes there.
    rest: Quat,
    axes: Quat,
    /// Which way the fingers point along the socket's X (-1 the right hand's).
    out: f32,
    /// The grip's place: on the palm, from the bone's origin, in model axes at rest.
    palm: Vec3,
    /// The bone's turn that sets the hand straight (the right hand's; none on the left).
    natural: Quat,
    /// The bone's turn that puts a gun's barrel on the clips' line, its top up.
    gun: Quat,
    fingers: Option<Fingers>,
    /// The arm it is on, for a carry: the upper arm's bone and the forearm's (the hand's parent
    /// and its parent), each with its rest rotation in model space and the line it lies on there.
    arm: Option<[(usize, Quat, Vec3); 2]>,
}

impl Hand {
    fn new(body: &Body, retarget: &Retarget, bone: &str, socket: &str, side: f32) -> Self {
        let rig = &body.rig;
        let s = body.sockets.get(socket);
        let bone = rig.find(bone).or(s.map(|s| s.bone)).unwrap_or(0);
        let rest = rig.inverse_bind[bone].inverse().to_scale_rotation_translation().1;
        let (offset, axes) = s.map_or((Vec3::ZERO, Quat::IDENTITY), |s| (s.offset, s.rotation));
        let tag = if side < 0.0 { "r" } else { "l" };
        let joints = ["fingers", "fingertips", "thumb"].map(|part| {
            let b = rig.find(&format!("{part}_{tag}"))?;
            Some((b, rig.inverse_bind[b].inverse().to_scale_rotation_translation().1))
        });
        let fingers = joints[0].is_some().then_some(Fingers { joints, axes, out: side });
        let at = |b: usize| rig.inverse_bind[b].inverse().transform_point3(Vec3::ZERO);
        let arm = rig.bones[bone].parent.and_then(|fore| {
            let upper = rig.bones[fore].parent?;
            let is_arm = |b: usize| rig.bones[b].name.to_lowercase().contains("arm");
            let part = |b: usize, end: usize| (b, rig.inverse_bind[b].inverse().to_scale_rotation_translation().1, (at(end) - at(b)).normalize_or_zero());
            (is_arm(fore) && is_arm(upper)).then(|| [part(upper, fore), part(fore, bone)])
        });
        let mut hand = Self { bone, rest, axes, out: side, palm: offset + axes * Vec3::NEG_Y * PALM, natural: Quat::IDENTITY, gun: Quat::IDENTITY, fingers, arm };
        // The clips' line for what the hand holds, on the bone: model-forward in the reference pose.
        let reference = retarget.ref_global[bone].inverse();
        if side < 0.0 {
            hand.natural = Quat::from_rotation_arc((rest.inverse() * axes * Vec3::Z).normalize(), (reference * Vec3::Z).normalize());
        }
        hand.gun = (reference * hand.frame(Hold::Gun, 1.0).rotation.inverse()).normalize();
        hand
    }

    /// The grip frame on the bone for `hold`, `clip_w` of the way from a hand at rest to one a
    /// weapon's clip has.
    fn frame(&self, hold: Hold, clip_w: f32) -> Transform {
        // On the socket's axes: the fingers point along X away from the body, the thumb along +Z.
        let (fingers, thumb) = (Vec3::X * self.out, Vec3::Z);
        let tilt = if hold == Hold::Handle { TILT * (1.0 - clip_w.clamp(0.0, 1.0)) } else { 0.0 };
        // `tilt` off the knuckles' line (+Z) toward the fingers, its top to the wrist.
        let along = thumb * tilt.cos() + fingers * tilt.sin();
        let top = -fingers * tilt.cos() + thumb * tilt.sin();
        let mut turn = self.axes * Quat::from_mat3(&Mat3::from_cols(top.cross(along), top, along));
        if hold == Hold::Gun {
            // A gun's barrel (+Z) lies along the fingers, its top to the thumb: a quarter turn about X.
            turn *= Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        }
        let at = self.palm - self.axes * fingers * (BACK * tilt / TILT);
        Transform::from_translation(self.rest.inverse() * at).with_rotation(self.rest.inverse() * turn)
    }

    /// The model-space rotations of the upper arm, the forearm and the hand in a carry
    /// ([`carry_lines`]): each bone's rest turned to lie on its line, the elbow's hinge kept (the
    /// upper arm's thumb side toward the forearm) and the hand's thumb up.
    fn carried(&self, carry: Carry) -> Option<[Quat; 3]> {
        let (upper, fore) = carry_lines(carry, self.out)?;
        let [(_, upper_rest, upper_line), (_, fore_rest, fore_line)] = self.arm?;
        let thumb = self.axes * Vec3::Z;
        let to = |line: Vec3, along: Vec3, toward: Vec3| framed(along, toward) * framed(line, thumb).inverse();
        let hand = to(self.axes * Vec3::X * self.out, fore, Vec3::Y);
        Some([to(upper_line, upper, fore) * upper_rest, to(fore_line, fore, Vec3::Y) * fore_rest, hand * self.rest])
    }

    /// The arm of `pose` in that carry, by `w` (0 as the pose has it, 1 the carry): each bone from
    /// the shoulder down turned toward its place under its parent as that now is.
    fn carry(&self, rig: &Rig, pose: &mut RigPose, carry: Carry, w: f32) {
        let (Some(turns), Some(arm)) = (self.carried(carry).filter(|_| w > 0.0), self.arm) else { return };
        let globals = rig.globals(pose);
        let mut parent = rig.bones[arm[0].0].parent.map_or(Quat::IDENTITY, |p| globals[p].to_scale_rotation_translation().1);
        for (b, turn) in [arm[0].0, arm[1].0, self.bone].into_iter().zip(turns) {
            pose.r[b] = pose.r[b].slerp((parent.inverse() * turn).normalize(), w.min(1.0)).normalize();
            parent = (parent * pose.r[b]).normalize();
        }
    }

    /// The bone's turn while it holds something that way, `clip_w` as in [`Hand::frame`].
    fn turn(&self, hold: Hold, clip_w: f32) -> Quat {
        if hold == Hold::Gun { self.natural.slerp(self.gun, clip_w.clamp(0.0, 1.0)) } else { self.natural }
    }
}

/// What a hand has hold of, for [`SurvivorModel::hold`]: how it is held; how much a weapon's clip
/// has the arm (0 at rest; 1 in a guard, a blow, a reload); how much the hand has taken it up (0
/// empty, 1 holding, eased between); how far the hand is closed (0 open, 1 round a thin handle).
#[derive(Clone, Copy, Debug)]
pub struct Grasp {
    pub hold: Hold,
    pub clip_w: f32,
    pub held: f32,
    pub closed: f32,
}

/// Grey-box poses onto the skeleton.
struct Retarget {
    src: Vec<Src>,
    /// Locals of the reference pose, and each bone's model-space rotation in it.
    reference: RigPose,
    ref_global: Vec<Quat>,
    /// Skeleton hip height over the grey-box's.
    hip_scale: f32,
    /// How far round the reference pose's chest is turned from its hips' heading (radians about Y;
    /// the idle's hips are faced forward, its chest isn't quite), for [`Retarget::apply_square`].
    ref_chest: f32,
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
        let ref_chest = Gaze::new(rig).map_or(0.0, |gaze| gaze.chest(&g));
        Self { src, reference, ref_global, hip_scale: (root_y / HIP).clamp(0.5, 2.0), ref_chest }
    }

    /// The grey-box pose `grey` on the skeleton.
    fn apply(&self, body: &Body, grey: &SPose) -> RigPose {
        self.apply_turned(body, grey, Quat::IDENTITY)
    }

    /// The same with the chest square to the hips: the reference's spine, shoulders, neck and head
    /// turned back by its chest's turn, the arms where the grey-box puts them. For poses that use both
    /// arms alike (the weapon shove): over the reference's turned chest the right shoulder stood back,
    /// so the right hand stayed by the chest while the left reached out (the owner).
    fn apply_square(&self, body: &Body, grey: &SPose) -> RigPose {
        self.apply_turned(body, grey, Quat::from_rotation_y(-self.ref_chest))
    }

    fn apply_turned(&self, body: &Body, grey: &SPose, chest: Quat) -> RigPose {
        let gg = globals(&survivor::BONES, grey).map(|m| m.to_scale_rotation_translation().1);
        let rig = &body.rig;
        let mut pose = self.reference.clone();
        let mut global = vec![Quat::IDENTITY; rig.len()];
        for b in 0..rig.len() {
            let parent = rig.bones[b].parent.map_or(Quat::IDENTITY, |p| global[p]);
            global[b] = match self.src[b] {
                Src::Grey(i @ (TORSO | HEAD)) => gg[i] * chest * self.ref_global[b],
                Src::Grey(i) => gg[i] * self.ref_global[b],
                Src::Mix(i, j, f) => gg[i].slerp(gg[j], f) * chest * self.ref_global[b],
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
    /// The body's own action clips, by [`ACTION_CLIPS`]: each one it has is played across that action
    /// instead of the grey-box's poses.
    actions: [Option<usize>; ACTION_CLIPS.len()],
    /// The ready stance held while aiming, a loop: bare hands (`guard_fists`), a one-handed weapon
    /// (`guard_one`), a two-handed one (`guard_two`, else the one-handed) and a gun held out level
    /// (`guard_gun`; without it the grey-box's).
    guards: [Option<usize>; 4],
    /// The body's clips of [`LIFE_CLIPS`].
    life: [Option<usize>; LIFE_CLIPS.len()],
    /// The body's clips of [`CROSS_CLIPS`].
    cross: [Option<usize>; CROSS_CLIPS.len()],
    /// The body's clips of [`STAIR_CLIPS`], and how high each one's lower foot is off the ground
    /// on average over its loop (see `compose`).
    stairs: [Option<usize>; STAIR_CLIPS.len()],
    stair_mid: [f32; STAIR_CLIPS.len()],
    /// The body's clips of [`SIT_CLIPS`].
    sit: [Option<usize>; SIT_CLIPS.len()],
    gaits: [Vec<Gait>; 3],
    /// The bones the hips' turn is taken back out of (the spine's first, under the hips).
    spine_base: Vec<usize>,
    /// The head, for keeping its eyes on the aim through a blow (a rig with a `Head` bone).
    gaze: Option<Gaze>,
    /// Bones an action takes over (the upper body: everything from the spine up), the bones the
    /// flinch turns, all of them.
    upper: Vec<usize>,
    /// The bones a gesture takes (#67): the left arm, and both arms.
    left_arm: Vec<usize>,
    arms: Vec<usize>,
    flinching: Vec<(usize, usize)>,
    all: Vec<usize>,
    /// Body vertices on the feet (for standing them on the ground).
    feet: Vec<usize>,
    /// A light texel of the body's texture, for tinting (see [`light_uv`]).
    light_uv: Option<[f32; 2]>,
    /// The hands (right, left) and how each holds a thing: see [`Hand`].
    hands: [Hand; 2],
    /// Per hand (right, left): its bone and its grip in the rest pose, for the hang joints.
    hang: [(usize, Vec3); 2],
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
        // Nothing asked for: the release build's own body, if it carries one, else this repo's.
        None => crate::release_chars::survivor_in(crate::release_chars::root(), DEFAULT_BODY)
            .unwrap_or_else(|| options.data.join(format!("../models/characters/{DEFAULT_BODY}.glb"))),
    };
    if crate::pack::is_dir(&path) {
        let mut found: Vec<PathBuf> = crate::pack::read_dir(&path).ok()?.into_iter().filter(|p| p.to_string_lossy().ends_with("_body.glb")).collect();
        found.sort();
        return found.into_iter().next();
    }
    crate::pack::is_file(&path).then_some(path)
}

/// Every wearable GLB in `dir`, by file name, read against `body`.
fn load_wearables(dir: &Path, body: &Body, into: &mut HashMap<String, Wearable>) {
    let Ok(entries) = crate::pack::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.into_iter().filter(|p| p.extension().is_some_and(|e| e == "glb")).collect();
    paths.sort();
    for path in paths {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        match crate::pack::read(&path).map_err(|e| e.to_string()).and_then(|b| parse_wearable(&b, &body.rig)) {
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
/// moving clips: a diagonal one's hips are turned on purpose. Then the head's own mean heading:
/// Meshy's idle stands with its hips turned and its head looking where it faces, so with the hips
/// squared the head was left looking 50° to his left, standing and (the idle's first frame being the
/// reference the grey-box's poses are carried onto) aiming too.
fn face_forward(body: &mut Body) {
    let rig = &body.rig;
    let Some(root) = (0..rig.len()).find(|&b| rig.bones[b].parent.is_none()) else { return };
    let gaze = Gaze::new(rig);
    let mean = |yaws: &mut dyn Iterator<Item = f32>| {
        let (sin, cos) = yaws.fold((0.0, 0.0), |(s, c), yaw| (s + yaw.sin(), c + yaw.cos()));
        f32::atan2(sin, cos)
    };
    for clip in body.clips.iter_mut().filter(|c| c.name.eq_ignore_ascii_case("idle")) {
        let hips = mean(&mut clip.frames.iter().map(|f| {
            let fwd = f.r[root] * Vec3::Z;
            fwd.x.atan2(fwd.z)
        }));
        let turn = Quat::from_rotation_y(-hips);
        for f in &mut clip.frames {
            f.r[root] = turn * f.r[root];
            let t = turn * Vec3::new(f.t[root].x, 0.0, f.t[root].z);
            (f.t[root].x, f.t[root].z) = (t.x, t.z);
        }
        if let Some(gaze) = &gaze {
            let head = mean(&mut clip.frames.iter().map(|f| gaze.looks(&rig.globals(f)).0));
            let turn = Quat::from_rotation_y(-head);
            for f in &mut clip.frames {
                gaze.turn(rig, f, turn);
            }
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

/// `clip`'s upper body as it stands over `pose`'s hips: the spine's first bones (`spine_base`) turned
/// so the chest keeps the heading and lean it has in the clip, whatever the hips under it do. Over the
/// clip's own hips it changes nothing.
fn over_hips(clip: &mut RigPose, pose: &RigPose, spine_base: &[usize]) {
    let root = 0;
    for &b in spine_base {
        clip.r[b] = (pose.r[root].inverse() * clip.r[root] * clip.r[b]).normalize();
    }
}

/// Turns a ready stance's whole pose, by `w` (0 to 1), so its chest is square to the aim. The packs'
/// ready stances stand side-on (a fighter's stance: the shipped one 44° round, the release ones 39°),
/// which is right standing still, feet and all. But laid over legs that strafe with the hips square
/// to the aim, the chest kept that heading and he walked with his body twisted (the owner). The
/// stance is turned as one piece, so the arms hold what they held, now in front of him; the head,
/// which looked along the aim, goes round with it and is turned back by [`Gaze::aim`].
fn square_chest(rig: &Rig, gaze: &Gaze, stance: &mut RigPose, w: f32) {
    if w <= 0.0 {
        return;
    }
    let root = 0;
    let chest = gaze.chest(&rig.globals(stance));
    stance.r[root] = (Quat::from_rotation_y(-chest * w.min(1.0)) * stance.r[root]).normalize();
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
        let mut body = match crate::pack::read(&path).map_err(|e| e.to_string()).and_then(|b| parse_body(&b)) {
            Ok(b) => b,
            Err(e) => {
                warn!("survivor body {}: {e}; drawing the grey-box", path.display());
                return None;
            }
        };
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            for line in crate::release_anim::load_into(crate::release_anim::root(), stem, &body.rig, &mut body.clips) {
                println!("{line}");
            }
        }
        face_forward(&mut body);
        let mut wardrobe = HashMap::new();
        load_wearables(&options.data.join("../models/wearables"), &body, &mut wardrobe);
        if let Some(dir) = path.parent() {
            load_wearables(&dir.join("wearables"), &body, &mut wardrobe);
        }
        let idle = body.clips.iter().position(|c| c.name.eq_ignore_ascii_case("idle"));
        let actions = ACTION_CLIPS.map(|name| body.clips.iter().position(|c| c.name.eq_ignore_ascii_case(name)));
        let guard = |name: &str| body.clips.iter().position(|c| c.name.eq_ignore_ascii_case(name));
        let guards = [guard("guard_fists"), guard("guard_one"), guard("guard_two").or_else(|| guard("guard_one")), guard("guard_gun")];
        let life = LIFE_CLIPS.map(guard);
        let cross = CROSS_CLIPS.map(guard);
        let stairs = STAIR_CLIPS.map(guard);
        let sit = SIT_CLIPS.map(guard);
        let gaits = gaits(&mut body);
        let reference = idle.map_or_else(|| body.rig.rest(), |i| body.clips[i].frames[0].clone());
        let retarget = Retarget::new(&body, reference);
        let rig = &body.rig;
        let gaze = Gaze::new(rig);
        let spine_base = (0..rig.len()).filter(|&b| rig.bones[b].parent.is_some_and(|p| rig.bones[p].parent.is_none()) && rig.bones[b].name.to_lowercase().starts_with("spine")).collect();
        let upper: Vec<usize> = {
            let mut up = vec![false; rig.len()];
            for b in 0..rig.len() {
                up[b] = rig.bones[b].name.to_lowercase().starts_with("spine") || rig.bones[b].parent.is_some_and(|p| up[p]);
            }
            (0..rig.len()).filter(|&b| up[b]).collect()
        };
        let left_arm = retarget.bones_of(&survivor::LEFT_ARM);
        let arms = retarget.bones_of(&survivor::ARMS);
        let flinching = (0..rig.len()).filter_map(|b| if let Src::Grey(g) = retarget.src[b] { UPPER.contains(&g).then_some((b, g)) } else { None }).collect();
        let feet_region = REGIONS.iter().position(|r| *r == BodyPart::Feet).unwrap() as u8;
        let feet = (0..body.mesh.vertices()).filter(|&v| body.regions[v] == feet_region).collect();
        // Each hand's grip, on its palm (#35): see `Hand`.
        let hands = [("RightHand", "hand_r", -1.0), ("LeftHand", "hand_l", 1.0)].map(|(bone, socket, side)| Hand::new(&body, &retarget, bone, socket, side));
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
            actions,
            guards,
            life,
            cross,
            stairs,
            sit,
            stair_mid: [0.0; STAIR_CLIPS.len()],
            gaits,
            spine_base,
            gaze,
            upper,
            left_arm,
            arms,
            flinching,
            all: (0..rig.len()).collect(),
            feet,
            light_uv: light,
            hands,
            hang,
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
        let stair_mid = model.stairs.map(|i| i.map_or(0.0, |i| (0..32).map(|k| model.foot_height(&model.body.clips[i].sample(k as f32 / 32.0))).sum::<f32>() / 32.0));
        let model = Self { bare: meshes.add(bare.to_mesh()), stair_mid, ..model };
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

    /// The hands (right, left) of `pose` holding what they hold: each set straight for it and
    /// closed round it (on a body that has the joints for that). See [`Hand`] and [`Grasp`].
    pub fn hold(&self, pose: &mut RigPose, grasps: [Grasp; 2]) {
        for (hand, g) in self.hands.iter().zip(grasps) {
            if g.held > 0.0 {
                pose.r[hand.bone] = (pose.r[hand.bone] * Quat::IDENTITY.slerp(hand.turn(g.hold, g.clip_w), g.held.min(1.0))).normalize();
            }
            if let Some(f) = hand.fingers.as_ref().filter(|_| g.closed > 0.0) {
                for (joint, turn) in f.joints.iter().zip(closing(f.axes, f.out, g.closed.min(1.0))) {
                    if let Some((b, rest)) = *joint {
                        pose.r[b] = (pose.r[b] * rest.inverse() * turn * rest).normalize();
                    }
                }
            }
        }
    }

    /// The arms (right, left) of `pose` carrying what their hands hold (#35), each by its weight
    /// (0 the arm as the pose has it, 1 the carry): the upper arm, the forearm and the hand turned
    /// to the carry's lines in model space, over whatever the gait has the arm doing, so a mug
    /// stays upright in front of the body standing, walking, running and sneaking. After
    /// [`SurvivorModel::hold`], whose straightened wrist it takes the place of.
    pub fn carry(&self, pose: &mut RigPose, carries: [(Carry, f32); 2]) {
        for (hand, (carry, w)) in self.hands.iter().zip(carries) {
            hand.carry(&self.body.rig, pose, carry, w);
        }
    }

    /// The grip frame of hand `hand` (0 right, 1 left) on its bone, holding that way, `clip_w` of
    /// the way from rest to a weapon's clip.
    pub fn grip_frame(&self, hand: usize, hold: Hold, clip_w: f32) -> Transform {
        self.hands[hand].frame(hold, clip_w)
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

    pub fn compose(&self, a: &SurvivorAnim, now: f32, category: WeaponCategory, armed: bool) -> RigPose {
        let style = survivor::style(category);
        let body = &self.body;
        let [walk, run, sneak] = std::array::from_fn(|k| self.moving(k, a));
        let idle_u = self.idle.map_or(0.0, |i| now / body.clips[i].secs + a.player as f32 * 0.37);
        let standing = self.clip_at(self.idle, idle_u).blend(&sneak, a.sneak_w);
        let moving = walk.blend(&run, a.run_w).blend(&sneak, a.sneak_w);
        let mut pose = standing.blend(&moving, a.move_w);
        // On a flight of stairs: the body's own stair loop in place of the gait.
        if let Some(i) = a.stair.filter(|_| a.stair_w > 0.0).and_then(|up| self.stairs[!up as usize]) {
            pose.blend_bones(&body.clips[i].sample(a.stair_u.fract()), a.stair_w, Some(&self.all));
        }
        // Armed or not, the arms hang and swing as the body's own clips have them: what is in a
        // hand follows the hand (#35; before, the grey-box's carry was put on the right arm).
        if a.aim_w > 0.0 {
            let slot = guard_slot(style, armed);
            match self.guards[slot] {
                // Aiming: the body's own ready stance for what he holds, the whole body standing and
                // the upper body over the gait when moving (the swings start from it and come back to it).
                Some(i) => {
                    let mut guard = body.clips[i].sample((now / body.clips[i].secs + a.player as f32 * 0.37).fract());
                    pose.blend_bones(&guard, a.aim_w * (1.0 - a.move_w), Some(&self.all));
                    // On the move the chest squares up to the aim: see `square_chest`. Not a gun's
                    // stance, whose line is the barrel's.
                    let square = if slot == GUARD_GUN { 0.0 } else { a.move_w };
                    if let Some(gaze) = &self.gaze {
                        square_chest(&body.rig, gaze, &mut guard, square);
                    }
                    over_hips(&mut guard, &pose, &self.spine_base);
                    pose.blend_bones(&guard, a.aim_w, Some(&self.upper));
                    if let Some(gaze) = &self.gaze {
                        gaze.aim(&body.rig, &mut pose, a.aim_w * square, false);
                    }
                }
                // A body without one: the grey-box's guard over the upper body.
                _ => pose.blend_bones(&self.retarget.apply(body, &survivor::guard(style, armed)), a.aim_w, Some(&self.upper)),
            }
        }
        if let Some((g, both, w)) = survivor::gesture_now(a) {
            // A gesture (#67): the grey-box's arms carried over, the free arm or both.
            pose.blend_bones(&self.retarget.apply(body, &g), w, Some(if both { self.arms.as_slice() } else { self.left_arm.as_slice() }));
        }
        if let Some((b, w)) = survivor::bonds_now(a) {
            // Tied hands, or hands at work on another survivor (#73): both arms.
            pose.blend_bones(&self.retarget.apply(body, &b), w, Some(self.arms.as_slice()));
        }
        if let Some(i) = a.task.filter(|_| a.task_w > 0.0).and_then(|t| self.life[2 + t as usize]) {
            // Hands busy with something (eating, drinking, searching, reloading): the body's loop
            // for it, the whole body standing and the upper body over the gait when moving.
            let mut busy = body.clips[i].sample((a.task_t / body.clips[i].secs).fract());
            pose.blend_bones(&busy, a.task_w * (1.0 - a.move_w), Some(&self.all));
            over_hips(&mut busy, &pose, &self.spine_base);
            pose.blend_bones(&busy, a.task_w, Some(&self.upper));
        }
        let clip = survivor::action_clock(a, now)
            .and_then(|(kind, n, u, w)| {
                let i = action_clips(kind, category, n).iter().find_map(|&c| self.actions[c])?;
                Some((body.clips[i].sample(u.clamp(0.0, 0.999)), w, kind))
            });
        if let Some((mut clip, w, kind)) = clip {
            // The body's own clip (made with the blow at `U_HIT`): the whole body standing (a stomp
            // always), the upper body over the legs' gait when moving, turned at the waist to face
            // as it does in the clip (the blow lands on the aim whatever the legs do).
            let stomp = kind == zombye_sim::ActionKind::Stomp;
            let legs = if stomp { 1.0 } else { 1.0 - a.move_w };
            pose.blend_bones(&clip, w * legs, Some(&self.all));
            over_hips(&mut clip, &pose, &self.spine_base);
            pose.blend_bones(&clip, w, Some(&self.upper));
            // And his eyes on what he hits: a zombie on the aim, or one on the ground at his feet.
            if let Some(gaze) = &self.gaze {
                gaze.aim(&body.rig, &mut pose, w, !stomp);
            }
        } else if let Some((act, whole, w)) = survivor::action_now(a, now, style, armed) {
            // A shove pushes with both arms alike: the chest square, so both reach out as far.
            let act = match a.action.map(|c| c.kind) {
                Some(zombye_sim::ActionKind::Shove) => self.retarget.apply_square(body, &act),
                _ => self.retarget.apply(body, &act),
            };
            pose.blend_bones(&act, w, Some(if whole { &self.all } else { &self.upper }));
        }
        // A blow taken: the body's own reaction over the upper body, once through, eased in and out;
        // else the flinch, on top: each grey-box turn about the axes it has in the reference pose.
        let w = survivor::flinch_weight(a, now);
        if let Some(i) = self.life[HURT] {
            let u = (now - a.hurt_at) / body.clips[i].secs;
            if (0.0..1.0).contains(&u) {
                let mut hurt = body.clips[i].sample(u);
                over_hips(&mut hurt, &pose, &self.spine_base);
                pose.blend_bones(&hurt, smooth(0.0, 0.12, u) * (1.0 - smooth(0.7, 1.0, u)), Some(&self.upper));
            }
        } else if w > 0.0 {
            let f = survivor::flinch();
            for &(b, g) in &self.flinching {
                let t = self.retarget.ref_global[b];
                pose.r[b] *= t.inverse() * Quat::IDENTITY.slerp(f.rot[g], w) * t;
            }
        }
        // Through a window or over a fence: the body's own clip for it, the whole body, at how far
        // across he is; its feet leave the ground as the clip has them.
        let crossing = a.cross.filter(|_| a.cross_w > 0.0).and_then(|c| self.cross[c as usize]).map(|i| body.clips[i].sample(a.cross_u.clamp(0.0, 0.999)));
        if let Some(over) = &crossing {
            pose.blend_bones(over, a.cross_w, Some(&self.all));
        }
        self.ground(&mut pose);
        // On stairs the loop's steps are real ones: the planted foot goes down under the body as
        // the flight carries it up, the other reaches for the next tread. So the body rides over
        // its lower foot by what the clip has, about the middle of it (the flight's line).
        if let Some((i, k)) = a.stair.filter(|_| a.stair_w > 0.0).and_then(|up| self.stairs[!up as usize].map(|i| (i, !up as usize))) {
            pose.t[0].y += a.stair_w * (self.foot_height(&body.clips[i].sample(a.stair_u.fract())) - self.stair_mid[k]);
        }
        if let Some(mut over) = crossing {
            let before = over.t[0].y;
            self.ground(&mut over);
            pose.t[0].y += a.cross_w * (before - over.t[0].y).max(0.0);
        }
        let root = 0;
        // Running has a moment in the air each step.
        pose.t[root].y += a.run_w * a.move_w * 0.05 * (a.cycle * TAU * 2.0).sin().max(0.0);
        if let Some((grey, w)) = survivor::sitting_now(a) {
            // Sitting: the body's own loop for the seat or the ground, else the grey-box's pose.
            let ground = a.seat == Some(zombye_sim::SeatKind::Ground);
            let mut sit = match self.sit[ground as usize] {
                Some(i) => body.clips[i].sample((now / body.clips[i].secs + a.player as f32 * 0.37).fract()),
                None => self.retarget.apply(body, &grey),
            };
            self.ground(&mut sit);
            pose = pose.blend(&sit, w);
        }
        if let Some((lie, w)) = survivor::lying(a, now) {
            pose = match (self.life[DIE].filter(|_| a.died), a.dead_since) {
                // Dead: the body's own death, once through and then still as it ends.
                (Some(i), Some(since)) => pose.blend(&body.clips[i].sample(((now - since) / body.clips[i].secs).clamp(0.0, 0.999)), w),
                _ => pose.blend(&self.retarget.apply(body, &lie), w),
            };
        }
        pose
    }

    /// Push the pose down (or up) so the lowest point of the feet is on the ground.
    /// How far off the ground the pose's lowest foot is as the pose has it.
    fn foot_height(&self, pose: &RigPose) -> f32 {
        let mut grounded = pose.clone();
        self.ground(&mut grounded);
        pose.t[0].y - grounded.t[0].y
    }

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
        // One flag a region, and one more for the hair under a hat (`HAIR_REGION`).
        let mut hidden = [false; REGIONS.len() + 1];
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
            if takes_top(d).is_some() {
                hidden[HAIR_REGION as usize] = true;
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
    /// For the new-game screen: the body and outfit meshes of a survivor wearing `worn` (item
    /// indices) and holding nothing.
    pub(crate) fn dressed(&self, items: &[ItemDef], worn: &[u16]) -> (Mesh, Option<Mesh>) {
        let (body, outfit) = self.dress(items, &OutfitView { worn: worn.to_vec(), ..default() });
        (body.to_mesh(), outfit.map(|o| o.to_mesh()))
    }

    /// For the new-game screen: standing idle, `secs` into the idle clip, feet on the ground.
    pub(crate) fn standing(&self, secs: f32) -> RigPose {
        let len = self.idle.map_or(1.0, |i| self.body.clips[i].secs.max(0.1));
        let mut pose = self.clip_at(self.idle, (secs / len).fract());
        self.ground(&mut pose);
        pose
    }

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

impl Dress {
    /// The body's and the outfit's entities.
    pub(crate) fn parts(&self) -> (Entity, Entity) {
        (self.body, self.outfit)
    }
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
    for (k, hand) in model.hands.iter().enumerate() {
        commands.spawn((HeldItem { player, hand: k as u8 }, hand.frame(Hold::Handle, 0.0), Visibility::Hidden, ChildOf(joints[hand.bone])));
    }
    let ring = if local { model.ring_me.clone() } else { model.ring_other.clone() };
    let mut ring = commands.spawn((Mesh3d(model.ring.clone()), MeshMaterial3d(ring), Transform::from_xyz(0.0, 0.02, 0.0), NotShadowCaster, ChildOf(root)));
    if !local {
        // Ours stays ours; another's turns violet if the server says a program plays it (`mark_agents`).
        ring.insert(Ring { player, agent: false });
    }
    if local {
        // Ours shows through whatever still covers it (`silhouette.rs`).
        commands.entity(body).insert(crate::silhouette::Silhouetted);
        commands.entity(outfit).insert(crate::silhouette::Silhouetted);
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

    #[test]
    fn moves_follow_the_weapon_and_vary() {
        use zombye_sim::ActionKind::*;
        let name = |k, c, n| action_clips(k, c, n).iter().map(|&i| ACTION_CLIPS[i]).collect::<Vec<_>>();
        assert_eq!(name(Swing, WeaponCategory::ShortBlade, 1), ["stab", "swing_one"]);
        assert_eq!(name(Stomp, WeaponCategory::Unarmed, 1), ["stomp"]);
        assert_eq!(name(Stomp, WeaponCategory::ShortBlunt, 1), ["ground_one", "stomp"]);
        assert_eq!(name(Stomp, WeaponCategory::Axe, 1), ["ground_two", "stomp"]);
        assert_eq!(name(Shove, WeaponCategory::Unarmed, 1), ["shove"]);
        assert_eq!(name(Shove, WeaponCategory::LongBlunt, 1), ["shove_armed"]);
        assert_eq!(name(Shove, WeaponCategory::ShortBlade, 1), ["shove_armed"]);
        // Both two-handed swings turn up, not in a strict alternation.
        let b: Vec<bool> = (0..40).map(|n| name(Swing, WeaponCategory::LongBlunt, n)[0] == "swing_two_b").collect();
        assert!(b.iter().filter(|x| **x).count() > 10 && b.iter().filter(|x| !**x).count() > 10);
        assert!(b.windows(2).any(|w| w[0] == w[1]));
    }

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

    /// What is held lies on the palm, in the hand's own shape (#35): a handle's grip is `PALM` under
    /// the socket, at rest `BACK` toward the wrist and the handle `TILT` off the knuckles' line toward
    /// the fingers, out past the thumb, and along the knuckles while a clip has the arm; a gun's barrel
    /// is on the way the fingers point; a small thing in the fist lies along the knuckles.
    #[test]
    fn a_hand_holds_on_its_palm() {
        let (body, _) = shipped_body();
        let reference = body.clip("idle").unwrap().frames[0].clone();
        let rt = Retarget::new(&body, reference.clone());
        for (bone, socket, side) in [("RightHand", "hand_r", -1.0), ("LeftHand", "hand_l", 1.0)] {
            let hand = Hand::new(&body, &rt, bone, socket, side);
            let (_, at) = body.socket_frame(socket).unwrap();
            let rest = body.rig.inverse_bind[hand.bone].inverse();
            let fingers = Vec3::X * side;
            let palm = at.transform_point3(Vec3::ZERO) - Vec3::Y * PALM;
            let (sin, cos) = TILT.sin_cos();
            for (hold, clip_w, grip, along, top) in [
                (Hold::Handle, 0.0, palm - fingers * BACK, Vec3::Z * cos + fingers * sin, -fingers * cos + Vec3::Z * sin),
                (Hold::Handle, 1.0, palm, Vec3::Z, -fingers),
                (Hold::Gun, 0.0, palm, fingers, Vec3::Z),
                (Hold::Gun, 1.0, palm, fingers, Vec3::Z),
                (Hold::Fist, 0.0, palm, Vec3::Z, -fingers),
            ] {
                let m = rest * hand.frame(hold, clip_w).to_matrix();
                assert!((m.transform_point3(Vec3::ZERO) - grip).length() < 1e-4, "{socket} {hold:?} {clip_w}: not on the palm");
                assert!((m.transform_vector3(Vec3::Z) - along).length() < 1e-4 && (m.transform_vector3(Vec3::Y) - top).length() < 1e-4, "{socket} {hold:?} {clip_w}: turned");
            }
            // The repo's own body has no joints to close its hands with: it is held as before.
            assert!(hand.fingers.is_none() || std::env::var_os("ZOMBYE_SURVIVOR").is_some());
        }
    }

    /// The right hand holding a weapon is set straight, and the weapon points where the clips
    /// mean it (#35): in the first frame of the idle, a handle along the knuckles (as a weapon's
    /// clip has it) and an aimed gun's barrel point model-forward, the gun's top up; at rest the
    /// handle points forward and down from the fist, the way a knife or a bat is carried at one's
    /// side, and the hand hangs in line with the forearm (the owner: "the forearm and wrist visibly
    /// twisted", "the knife sticks out horizontally at 90 degrees to the forearm"). The left hand's
    /// bone is never turned.
    #[test]
    fn a_weapon_hangs_forward_and_down_from_a_straight_wrist() {
        let (body, _) = shipped_body();
        let idle = body.clip("idle").unwrap();
        let rt = Retarget::new(&body, idle.frames[0].clone());
        let hand = Hand::new(&body, &rt, "RightHand", "hand_r", -1.0);
        let rig = &body.rig;
        let held = |pose: &RigPose, hold: Hold, clip_w: f32| {
            let mut pose = pose.clone();
            pose.r[hand.bone] *= hand.turn(hold, clip_w);
            let g = rig.globals(&pose);
            let turn = g[hand.bone].to_scale_rotation_translation().1;
            let arm = (g[hand.bone].transform_point3(Vec3::ZERO) - g[rig.bones[hand.bone].parent.unwrap()].transform_point3(Vec3::ZERO)).normalize();
            let frame = turn * hand.frame(hold, clip_w).rotation;
            (frame * Vec3::Z, frame * Vec3::Y, turn * hand.rest.inverse() * hand.axes * Vec3::X * hand.out, arm)
        };
        let (along, ..) = held(&idle.frames[0], Hold::Handle, 1.0);
        assert!((along - Vec3::Z).length() < 1e-3, "in a weapon's clip a handle points {along}, not where the clips mean");
        let (barrel, top, ..) = held(&idle.frames[0], Hold::Gun, 1.0);
        assert!((barrel - Vec3::Z).length() < 1e-3 && (top - Vec3::Y).length() < 1e-3, "an aimed gun points {barrel}, its top {top}");
        for frame in &idle.frames {
            let (along, _, fingers, arm) = held(frame, Hold::Handle, 0.0);
            assert!(along.z > 0.4 && along.y < -0.3, "a handle at rest points {along}, not forward and down");
            let off = along.angle_between(arm).to_degrees();
            assert!((25.0..=65.0).contains(&off), "a handle at rest is {off:.0} degrees off the forearm");
            let bend = fingers.angle_between(arm).to_degrees();
            assert!(bend < 40.0, "the hand is bent {bend:.0} degrees off the forearm");
        }
        let left = Hand::new(&body, &rt, "LeftHand", "hand_l", 1.0);
        assert_eq!(left.turn(Hold::Fist, 0.0), Quat::IDENTITY);
    }

    /// A small thing at rest is carried in front of the body and a pistol low and ready (#35; the
    /// owner: the mug "lies sideways in a hanging fist and sinks behind the hip", the pistol "hangs
    /// straight down and hides against the leg"). Over every clip, whatever the gait has the arm
    /// doing, either hand: in front, the elbow is bent 70 to 90 degrees, the forearm forward, the
    /// fist in front of the body between the hips and the chest and what stands in it upright; low,
    /// a gun's barrel points forward and down 30 degrees, its top up, the fist out beside the hip.
    /// A thing at one's side, or no weight, leaves the arm to the clips.
    #[test]
    fn a_small_thing_is_carried_in_front_and_a_pistol_low_and_ready() {
        let (body, _) = shipped_body();
        let rt = Retarget::new(&body, body.clip("idle").unwrap().frames[0].clone());
        let rig = &body.rig;
        let hips = rig.find("Hips").unwrap();
        for (bone, socket, side) in [("RightHand", "hand_r", -1.0), ("LeftHand", "hand_l", 1.0)] {
            let hand = Hand::new(&body, &rt, bone, socket, side);
            let [(upper, ..), (fore, ..)] = hand.arm.expect("the body's hands are on arms");
            // Standing and on the move: the clips a carry goes over.
            let gaits = ["idle", "walk", "run", "sneak", "crouch"];
            let mut seen = 0;
            for clip in body.clips.iter().filter(|c| gaits.iter().any(|g| c.name.to_lowercase().starts_with(g))) {
                seen += 1;
                for k in 0..8 {
                    let clip_pose = clip.sample(k as f32 / 8.0);
                    let at = |carry: Carry, w: f32| {
                        let mut pose = clip_pose.clone();
                        hand.carry(rig, &mut pose, carry, w);
                        let g = rig.globals(&pose);
                        let [shoulder, elbow, wrist, hip] = [upper, fore, hand.bone, hips].map(|b| g[b].transform_point3(Vec3::ZERO));
                        let frame = |hold: Hold| (g[hand.bone] * hand.frame(hold, 0.0).to_matrix()).to_scale_rotation_translation().1;
                        (pose, shoulder, elbow, wrist, hip, frame(Hold::Fist), frame(Hold::Gun))
                    };
                    let name = format!("{} {k}/8 {socket}", clip.name);
                    let (_, shoulder, elbow, wrist, hip, fist, _) = at(Carry::Front, 1.0);
                    let bend = (elbow - shoulder).angle_between(wrist - elbow).to_degrees();
                    assert!((70.0..=90.0).contains(&bend), "{name}: the elbow is bent {bend:.0} degrees");
                    let forearm = (wrist - elbow).normalize();
                    assert!(forearm.z > 0.9, "{name}: the forearm points {forearm}, not forward");
                    assert!((fist * Vec3::Z).y > 0.97, "{name}: what stands in the fist has its top to {}", fist * Vec3::Z);
                    assert!(wrist.z > shoulder.z + 0.15 && wrist.x.abs() < shoulder.x.abs(), "{name}: the fist at {wrist} is not in front of the body (shoulder {shoulder})");
                    assert!(wrist.y > hip.y && wrist.y < shoulder.y - 0.15, "{name}: the fist at {wrist} is not at the waist (hips {hip}, shoulder {shoulder})");
                    let (_, shoulder, _, wrist, _, _, gun) = at(Carry::Low, 1.0);
                    let barrel = gun * Vec3::Z;
                    let down = (-barrel.y).asin().to_degrees();
                    assert!(barrel.z > 0.8 && (25.0..=35.0).contains(&down), "{name}: a gun's barrel points {barrel}, {down:.0} degrees down");
                    assert!((gun * Vec3::Y).y > 0.8, "{name}: the gun's top is to {}", gun * Vec3::Y);
                    assert!(wrist.x.abs() > shoulder.x.abs() && wrist.z > shoulder.z + 0.1, "{name}: the fist at {wrist} is not out beside the hip (shoulder {shoulder})");
                    for (carry, w) in [(Carry::Side, 1.0), (Carry::Front, 0.0)] {
                        assert_eq!(at(carry, w).0.r, clip_pose.r, "{name}: {carry:?} by {w} moved the arm");
                    }
                    // Eased: halfway there, the upper arm is halfway between the clip's turn and the carry's.
                    let (half, full) = (at(Carry::Front, 0.5).0.r[upper], at(Carry::Front, 1.0).0.r[upper]);
                    let (to_half, to_full) = (clip_pose.r[upper].angle_between(half), clip_pose.r[upper].angle_between(full));
                    assert!((to_half - to_full * 0.5).abs() < 1e-3 && (half.angle_between(full) - to_full * 0.5).abs() < 1e-3, "{name}: halfway the upper arm is {to_half} of {to_full} round");
                }
            }
            assert!(seen >= 3, "the body has an idle and gaits ({seen} clips)");
        }
    }

    /// Prints where the release body's hands point in its clips (`ZOMBYE_RELEASE_CHARACTERS` and
    /// `ZOMBYE_RELEASE_ANIM` set): each hand's fingers, thumb and palm in model space (+Z forward,
    /// +Y up), the forearm's line, and where the hand and the head are. For fitting what is held to
    /// the clips (#35): `cargo test --release -p zombye-client hands_report -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn hands_report() {
        let body = match crate::release_chars::survivor_in(crate::release_chars::root(), DEFAULT_BODY) {
            Some(path) => {
                let mut body = parse_body(&crate::pack::read(&path).unwrap()).unwrap();
                for line in crate::release_anim::load_into(crate::release_anim::root(), DEFAULT_BODY, &body.rig, &mut body.clips) {
                    println!("{line}");
                }
                face_forward(&mut body);
                body
            }
            None => shipped_body().0,
        };
        let rig = &body.rig;
        let head = rig.find("Head").unwrap();
        let v = |v: Vec3| format!("({:5.2} {:5.2} {:5.2})", v.x, v.y, v.z);
        for clip in &body.clips {
            for k in 0..10 {
                let pose = clip.sample(k as f32 / 10.0);
                let g = rig.globals(&pose);
                print!("{:14} {:.1}s {:.1} head {}", clip.name, clip.secs, k as f32 / 10.0, v(g[head].transform_point3(Vec3::ZERO)));
                for (bone, side) in [("RightHand", -1.0), ("LeftHand", 1.0)] {
                    let b = rig.find(bone).unwrap();
                    let turn = (g[b] * rig.inverse_bind[b]).to_scale_rotation_translation().1;
                    let at = g[b].transform_point3(Vec3::ZERO);
                    let arm = (at - g[rig.bones[b].parent.unwrap()].transform_point3(Vec3::ZERO)).normalize();
                    print!(" | {} F{} T{} P{} arm{} at{}", &bone[..1], v(turn * Vec3::X * side), v(turn * Vec3::Z), v(turn * Vec3::NEG_Y), v(arm), v(at));
                }
                println!();
            }
        }
    }

    /// A hand closes toward its palm (#35): the fingers bend down at the knuckles and their tips
    /// further, the thumb comes under; nothing moves in an open hand; the left hand's is the
    /// right's in a mirror.
    #[test]
    fn a_hand_closes_toward_its_palm() {
        let (palm, thumb) = (Vec3::NEG_Y, Vec3::Z);
        for out in [-1.0, 1.0] {
            let fingers = Vec3::X * out;
            assert!(closing(Quat::IDENTITY, out, 0.0).iter().all(|q| q.angle_between(Quat::IDENTITY) < 1e-5), "an open hand is turned");
            let [knuckles, tips, thumbs] = closing(Quat::IDENTITY, out, 1.0);
            let bent = knuckles * fingers;
            assert!(bent.dot(palm) > 0.9 && bent.dot(fingers) > 0.0, "the fingers bend to the palm: {bent}");
            let curled = knuckles * tips * fingers;
            assert!(curled.dot(fingers) < -0.5, "the fingertips come back under the palm: {curled}");
            // The thumb lies out from the palm's edge, forward and along the fingers: it comes under.
            let over = thumbs * (thumb + fingers).normalize();
            assert!(over.dot(palm) > 0.6, "the thumb comes under the palm, over the fingers: {over}");
            // Half closed is half the turn.
            let half = closing(Quat::IDENTITY, out, 0.5)[0] * fingers;
            assert!((half.angle_between(fingers) - CURL_FINGERS * 0.5).abs() < 1e-4);
        }
        let mirror = |v: Vec3| Vec3::new(-v.x, v.y, v.z);
        let (r, l) = (closing(Quat::IDENTITY, -1.0, 1.0), closing(Quat::IDENTITY, 1.0, 1.0));
        for k in 0..3 {
            for v in [Vec3::X, Vec3::Y, Vec3::Z] {
                assert!((mirror(r[k] * v) - l[k] * mirror(v)).length() < 1e-5, "joint {k} isn't mirrored");
            }
        }
    }

    /// The weapon shove on the body (no clip of its own: the grey-box's pose carried over) pushes with
    /// both arms alike: both hands as far out in front, level with each other (the owner: the right
    /// hand stayed by the chest while the left reached out, over the idle's turned chest).
    #[test]
    fn the_weapon_shove_reaches_out_with_both_hands() {
        let (body, _) = shipped_body();
        let idle = body.clips.iter().position(|c| c.name.eq_ignore_ascii_case("idle")).unwrap();
        let retarget = Retarget::new(&body, body.clips[idle].frames[0].clone());
        let rig = &body.rig;
        let (l, r) = (rig.find("LeftHand").unwrap(), rig.find("RightHand").unwrap());
        for style in [Style::OneHand, Style::TwoHand] {
            let grey = survivor::action(zombye_sim::ActionKind::Shove, style, true, survivor::U_HIT, &survivor::idle(0.0, true));
            let g = rig.globals(&retarget.apply_square(&body, &grey));
            let (lh, rh) = (g[l].w_axis.truncate(), g[r].w_axis.truncate());
            assert!((lh.z - rh.z).abs() < 0.08 && (lh.y - rh.y).abs() < 0.08, "{style:?}: left hand at {lh}, right at {rh}");
            assert!(lh.z > 0.3 && rh.z > 0.3, "{style:?}: hands out in front: left {lh}, right {rh}");
        }
    }

    /// Through every blow the body's own clips play, his eyes are on the aim: straight ahead, or as
    /// far round as the neck goes while the body winds up, and at the blow itself on what it hits
    /// (the owner: with a baseball bat "he swings to the side with his head looking away").
    #[test]
    fn eyes_stay_on_the_aim_through_a_blow() {
        let (body, _) = shipped_body();
        let rig = &body.rig;
        let gaze = Gaze::new(rig).unwrap();
        // Standing, he looks where he faces (the idle's sway aside).
        for f in &body.clip("idle").unwrap().frames {
            let (idle, _) = gaze.looks(&rig.globals(f));
            assert!(idle.abs() < 0.2, "the idle looks {:.0}° off the facing", idle.to_degrees());
        }
        for name in ACTION_CLIPS {
            let Some(clip) = body.clip(name) else { continue };
            let level = !matches!(name, "stomp" | "ground_one" | "ground_two");
            let mut worst = 0f32;
            for k in 0..=40 {
                let mut pose = clip.sample(k as f32 / 40.0 * 0.999);
                worst = worst.max(gaze.looks(&rig.globals(&pose)).0.abs());
                gaze.aim(rig, &mut pose, 1.0, level);
                let g = rig.globals(&pose);
                let ((yaw, pitch), chest) = (gaze.looks(&g), gaze.chest(&g));
                let want = 0f32.clamp(chest - NECK_TURN, chest + NECK_TURN);
                assert!(angle_diff(yaw, want).abs() < 0.03, "{name} at {k}/40: looks {:.0}°, chest {:.0}°", yaw.to_degrees(), chest.to_degrees());
                assert!(!level || (-LOOK_DOWN - 0.03..LOOK_UP + 0.03).contains(&pitch), "{name} at {k}/40: looks {:.0}° up", pitch.to_degrees());
            }
            let mut hit = clip.sample(0.45);
            let was = gaze.looks(&rig.globals(&hit)).0;
            gaze.aim(rig, &mut hit, 1.0, level);
            let now = gaze.looks(&rig.globals(&hit)).0;
            println!("{name}: at the blow looked {:.0}° off the aim, now {:.0}°; furthest off in the clip was {:.0}°", was.to_degrees(), now.to_degrees(), worst.to_degrees());
            // As far as the neck goes off a chest the swing has turned.
            let most = 0.45;
            assert!(now.abs() < most, "{name}: looks {:.0}° off the aim at the blow", now.to_degrees());
        }
    }

    /// Aiming on the move, sideways or back, the chest is square to the aim over the strafing hips
    /// and his eyes are on it: not the ready stance's side-on chest twisted round over them (the
    /// owner: "the body is twisted as he has an arm forward ... when you strafe it can look strange").
    #[test]
    fn aiming_on_the_move_the_chest_is_square_to_the_aim() {
        let (body, _) = shipped_body();
        let rig = &body.rig;
        let gaze = Gaze::new(rig).unwrap();
        let spine_base: Vec<usize> = (0..rig.len()).filter(|&b| rig.bones[b].parent == Some(0) && rig.bones[b].name.to_lowercase().starts_with("spine")).collect();
        let mut up = vec![false; rig.len()];
        for b in 0..rig.len() {
            up[b] = rig.bones[b].name.to_lowercase().starts_with("spine") || rig.bones[b].parent.is_some_and(|p| up[p]);
        }
        let upper: Vec<usize> = (0..rig.len()).filter(|&b| up[b]).collect();
        let (hand_l, hand_r) = (rig.find("LeftHand").unwrap(), rig.find("RightHand").unwrap());
        for guard in ["guard_one", "guard_fists"] {
            let Some(guard_clip) = body.clip(guard) else { continue };
            for legs in ["walk_left", "walk_right", "walk_back", "sneak_left"] {
                let Some(legs_clip) = body.clip(legs) else { continue };
                for k in 0..8 {
                    let u = k as f32 / 8.0;
                    let over = |square: bool| {
                        let mut pose = legs_clip.sample(u);
                        let mut stance = guard_clip.sample(u);
                        let before: Vec<f32> = [hand_l, hand_r].iter().map(|&h| (rig.globals(&stance)[h].w_axis.truncate() - rig.globals(&stance)[upper[0]].w_axis.truncate()).length()).collect();
                        if square {
                            square_chest(rig, &gaze, &mut stance, 1.0);
                        }
                        // The stance is turned as one piece: the hands are where they were about the spine.
                        let after: Vec<f32> = [hand_l, hand_r].iter().map(|&h| (rig.globals(&stance)[h].w_axis.truncate() - rig.globals(&stance)[upper[0]].w_axis.truncate()).length()).collect();
                        assert!(before.iter().zip(&after).all(|(a, b)| (a - b).abs() < 1e-4));
                        over_hips(&mut stance, &pose, &spine_base);
                        pose.blend_bones(&stance, 1.0, Some(&upper));
                        if square {
                            gaze.aim(rig, &mut pose, 1.0, false);
                        }
                        let g = rig.globals(&pose);
                        (gaze.chest(&g).to_degrees(), gaze.looks(&g).0.to_degrees())
                    };
                    let ((was, _), (chest, eyes)) = (over(false), over(true));
                    if k == 0 {
                        println!("{guard} over {legs}: the chest {chest:.0}° off the aim, was {was:.0}°; the eyes {eyes:.0}°");
                    }
                    assert!(chest.abs() < 3.0, "{guard} over {legs} at {u}: the chest is {chest:.0}° off the aim");
                    assert!(eyes.abs() < 3.0, "{guard} over {legs} at {u}: he looks {eyes:.0}° off the aim");
                }
            }
        }
        // By halves: half way round at half the weight, and nothing at none.
        let stance = body.clip("guard_one").unwrap().sample(0.0);
        let side_on = gaze.chest(&rig.globals(&stance));
        assert!(side_on.abs().to_degrees() > 15.0, "the shipped stance is side-on: {:.0}°", side_on.to_degrees());
        let (mut half, mut none) = (stance.clone(), stance.clone());
        square_chest(rig, &gaze, &mut half, 0.5);
        square_chest(rig, &gaze, &mut none, 0.0);
        assert!((gaze.chest(&rig.globals(&half)) - side_on * 0.5).abs() < 0.02);
        assert!(none.r[0].angle_between(stance.r[0]) < 1e-5);
    }

    /// A two-handed swing on the move lands on the aim too: the upper body keeps the clip's heading
    /// over the walk's hips, where the clip's own hips are turned well off it.
    #[test]
    fn a_swing_on_the_move_lands_on_the_aim() {
        let (body, _) = shipped_body();
        let rig = &body.rig;
        let (Some(swing), Some(walk)) = (body.clip("swing_two"), body.clip("walk")) else { return };
        let spine_base: Vec<usize> = (0..rig.len()).filter(|&b| rig.bones[b].parent == Some(0) && rig.bones[b].name.to_lowercase().starts_with("spine")).collect();
        let mut up = vec![false; rig.len()];
        for b in 0..rig.len() {
            up[b] = rig.bones[b].name.to_lowercase().starts_with("spine") || rig.bones[b].parent.is_some_and(|p| up[p]);
        }
        let upper: Vec<usize> = (0..rig.len()).filter(|&b| up[b]).collect();
        let hand = rig.find("RightHand").unwrap();
        let off = |carried: bool| {
            let mut pose = walk.sample(0.3);
            let mut clip = swing.sample(0.45);
            if carried {
                over_hips(&mut clip, &pose, &spine_base);
            }
            pose.blend_bones(&clip, 1.0, Some(&upper));
            let g = rig.globals(&pose);
            let d = g[hand].w_axis.truncate() - g[0].w_axis.truncate();
            d.x.atan2(d.z).to_degrees()
        };
        println!("swing_two over a walk: the hands {:.0}° off the aim, were {:.0}°", off(true), off(false));
        // (The right hand: a one-handed swing's hand is a little to its own side of the aim.)
        assert!(off(true).abs() < 25.0, "the blow lands {:.0}° off the aim", off(true));
        // Standing, on the clip's own hips, nothing changes.
        let whole = swing.sample(0.45);
        let mut same = whole.clone();
        over_hips(&mut same, &whole, &spine_base);
        assert!(spine_base.iter().all(|&b| same.r[b].angle_between(whole.r[b]) < 1e-3));
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

