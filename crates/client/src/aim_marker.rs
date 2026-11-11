//! The aim marker: what shows where you aim while the right mouse is held.
//!
//! **With a melee weapon or bare hands:** the swing's wedge on the ground in front of you, out to
//! the weapon's reach and as wide as its arc (a faint fill with a bright rim), and a small ring
//! under the cursor. Both turn red while a swing now would hit a standing zombie (the server's
//! own test: any of its body in reach and arc; [`in_swing`]).
//!
//! **With a gun:** a reticle under the cursor, four ticks round a dot. The gap between the ticks is
//! the gun's spread where you aim, in metres of the world ([`spread_radius`]): wider further off,
//! wider on the move, wide for a shotgun. With the cursor on a body it sits on the point of that
//! body the gun is aimed at (`player::gun_aim`: the head, with the cursor on the head) and turns
//! red. Beyond the gun's range, or with nothing in it, it is dimmed.
//!
//! The reticle and the ring are drawn in the world, facing the camera, lifted toward it along the
//! line of sight: the camera is orthographic, so that moves nothing on screen, and nothing in the
//! world covers them. Their size is kept the same on screen at any zoom; only a gun's spread is in
//! real metres. While any of it shows the system pointer is hidden.
//!
//! `--aim-at DX,DY` shows it in an offscreen picture (`--render-to`), as if you aimed with the
//! cursor on the ground that many metres east and south of you, or with `--aim-at zombie` on the
//! nearest standing zombie; `--gun ID` then names the weapon (a picture's world doesn't tick, so
//! nothing was put in his hand).
//!
//! Not shown: how much tired or hurt arms widen a gun's spread (the server's, by the shot's
//! power), and whether something stands in a shot's way.

use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use zombye_sim::ZombieAnim;
use zombye_sim::ai::ZOMBIE_RADIUS;
use zombye_sim::ranged::{MUZZLE_HEIGHT, RangedDef};

use crate::autoplay::AutoInput;
use crate::camera::IsoCamera;
use crate::player::{LocalPlayer, Player};
use crate::server_link::{JoinInfo, ServerLink};

/// The zoom (metres of world top to bottom) the sizes below are given at; at another zoom they
/// are scaled to look the same on screen.
const ZOOM: f32 = 26.0;
/// A reticle tick: how long and how thick (metres at [`ZOOM`]), and the dark edge round it.
const TICK: Vec2 = Vec2::new(0.3, 0.07);
const EDGE: f32 = 0.035;
/// The dot in the middle of the reticle (its radius).
const DOT: f32 = 0.05;
/// The least gap from the reticle's middle to its ticks, however tight the gun.
const MIN_GAP: f32 = 0.16;
/// The ring under the cursor with a melee weapon: inner and outer radius.
const RING: (f32, f32) = (0.11, 0.17);
/// How high the reticle is lifted toward the camera (metres up): over roofs and trees.
const LIFT: f32 = 14.0;
/// How high over the ground the swing's wedge is drawn.
const WEDGE_Y: f32 = 0.05;
/// How fast the reticle's gap follows the spread (per second).
const GAP_RATE: f32 = 14.0;

// The brand's bone white and accent red (zombye-art's palette anchors), and its near-black.
const BONE: Color = Color::srgb(0.95, 0.9, 0.79);
const RED: Color = Color::srgb(0.84, 0.15, 0.17);
const DIMMED: Color = Color::srgba(0.62, 0.62, 0.6, 0.6);
const DARK: Color = Color::srgba(0.08, 0.08, 0.08, 0.85);

pub struct AimMarkerPlugin;

impl Plugin for AimMarkerPlugin {
    fn build(&self, app: &mut App) {
        // After the camera has moved this frame, so the marker never trails the cursor.
        app.add_systems(Startup, setup).add_systems(PostUpdate, (show, hide_pointer).chain().before(bevy::transform::TransformSystems::Propagate));
    }
}

/// What the marker is made of.
#[derive(Resource)]
struct AimMarker {
    /// Under the cursor, facing the camera: the parent of the reticle and the ring.
    pointer: Entity,
    reticle: Entity,
    ring: Entity,
    /// The reticle's four ticks (east, north, west, south), each with its dark edge behind it.
    ticks: [(Entity, Entity); 4],
    /// The dot in the reticle's middle (kept the same size on screen).
    dot: Entity,
    /// The swing's wedge on the ground, and the reach and arc its mesh was built for.
    wedge: Entity,
    wedge_for: Option<(f32, f32)>,
    /// The colour of the reticle and ring, and of the wedge.
    ink: Handle<StandardMaterial>,
    ground: Handle<StandardMaterial>,
    /// The reticle's gap as drawn (it follows the spread smoothly); `None` while hidden.
    gap: Option<f32>,
    /// Whether any of it shows (the system pointer is hidden then).
    shown: bool,
}

/// How far off the aim a gun's shots land `dist` metres away (metres): its spread, wider on the
/// move, as `World::fire` has it for a shot at full power.
pub fn spread_radius(gun: &RangedDef, dist: f32, moving: bool) -> f32 {
    let degrees = gun.spread_degrees + if moving { gun.move_spread_degrees } else { 0.0 };
    dist.max(0.0) * degrees.to_radians().tan()
}

/// Would a swing from `from` facing `facing`, reaching `reach` over an arc of `arc_degrees`, hit
/// a body standing at `at`? The server's test (`combat::land`): its middle within the reach plus
/// its radius, and any of it inside the arc.
pub fn in_swing(from: Vec2, facing: f32, reach: f32, arc_degrees: f32, at: Vec2) -> bool {
    let to = at - from;
    let d = to.length();
    if d > reach + ZOMBIE_RADIUS {
        return false;
    }
    if d <= 1e-4 {
        return true;
    }
    let off = Vec2::from_angle(facing).dot(to / d).clamp(-1.0, 1.0).acos();
    off <= (arc_degrees * 0.5).to_radians() + (ZOMBIE_RADIUS / d).min(1.0).asin()
}

/// A flat mesh of triangles with a colour at each corner.
fn mesh(pos: Vec<[f32; 3]>, colors: Vec<[f32; 4]>, idx: Vec<u32>, normal: [f32; 3]) -> Mesh {
    let normals = vec![normal; pos.len()];
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(idx))
}

/// A 1 × 1 square facing +z.
fn square_mesh() -> Mesh {
    let pos = vec![[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.5, 0.5, 0.0], [-0.5, 0.5, 0.0]];
    mesh(pos, vec![[1.0; 4]; 4], vec![0, 1, 2, 0, 2, 3], [0.0, 0.0, 1.0])
}

/// A ring facing +z from radius `inner` (0: a disc) to `outer`.
fn ring_mesh(inner: f32, outer: f32) -> Mesh {
    let n = 32;
    let (mut pos, mut idx) = (Vec::new(), Vec::new());
    for i in 0..n {
        let d = Vec2::from_angle(i as f32 / n as f32 * TAU);
        pos.push([d.x * inner, d.y * inner, 0.0]);
        pos.push([d.x * outer, d.y * outer, 0.0]);
        let (k, next) = (i as u32 * 2, ((i + 1) % n) as u32 * 2);
        idx.extend_from_slice(&[k, k + 1, next, k + 1, next + 1, next]);
    }
    let colors = vec![[1.0; 4]; pos.len()];
    mesh(pos, colors, idx, [0.0, 0.0, 1.0])
}

/// The swing's wedge, flat on the ground, pointing along +x from the origin: a faint fill that
/// grows toward the rim, a bright rim at `reach`, and a short bright line in along each side.
/// The corners' colours are white with an opacity; the material gives the colour.
pub fn wedge_mesh(reach: f32, arc_degrees: f32) -> Mesh {
    let half = (arc_degrees.clamp(1.0, 360.0) * 0.5).to_radians();
    let whole = arc_degrees >= 359.5;
    let n = ((arc_degrees / 6.0).ceil() as usize).clamp(6, 60);
    let (rim, side, fill, bright) = (0.06_f32.min(reach * 0.2), 0.035, 0.1, 0.85);
    let start = (reach * 0.35).min(0.5);
    let (mut pos, mut colors, mut idx) = (Vec::new(), Vec::new(), Vec::new());
    // A band between two radii across the arc, its opacity `a0` inside and `a1` outside.
    let mut band = |r0: f32, r1: f32, a0: f32, a1: f32| {
        let base = pos.len() as u32;
        for i in 0..=n {
            let d = Vec2::from_angle(-half + 2.0 * half * i as f32 / n as f32);
            pos.push([d.x * r0, 0.0, d.y * r0]);
            pos.push([d.x * r1, 0.0, d.y * r1]);
            colors.extend_from_slice(&[[1.0, 1.0, 1.0, a0], [1.0, 1.0, 1.0, a1]]);
            if i < n {
                let k = base + i as u32 * 2;
                idx.extend_from_slice(&[k, k + 2, k + 1, k + 1, k + 2, k + 3]);
            }
        }
    };
    band(start, reach - rim, 0.0, fill);
    band(reach - rim, reach, bright, bright);
    if !whole {
        // The two sides: from half way out to the rim.
        for sign in [-1.0, 1.0] {
            let d = Vec2::from_angle(sign * half);
            let across = d.perp() * side * 0.5;
            let (a, b) = (d * (reach * 0.55), d * (reach - rim));
            let base = pos.len() as u32;
            for (p, alpha) in [(a - across, 0.0), (a + across, 0.0), (b + across, bright), (b - across, bright)] {
                pos.push([p.x, 0.0, p.y]);
                colors.push([1.0, 1.0, 1.0, alpha]);
            }
            idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    mesh(pos, colors, idx, [0.0, 1.0, 0.0])
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    // Unlit and over everything: it is an instrument, not a thing in the world's light.
    let mut material = |c: Color| materials.add(StandardMaterial { base_color: c, unlit: true, alpha_mode: AlphaMode::Blend, cull_mode: None, double_sided: true, ..default() });
    let (ink, dark, ground) = (material(BONE), material(DARK), material(BONE));
    let (square, dot, dot_edge) = (meshes.add(square_mesh()), meshes.add(ring_mesh(0.0, DOT)), meshes.add(ring_mesh(0.0, DOT + EDGE)));
    let (ring, ring_edge) = (meshes.add(ring_mesh(RING.0, RING.1)), meshes.add(ring_mesh(RING.0 - EDGE, RING.1 + EDGE)));

    let hidden = || (Transform::default(), Visibility::Hidden);
    let pointer = commands.spawn(hidden()).id();
    let reticle = commands.spawn((hidden(), ChildOf(pointer))).id();
    let ring_root = commands.spawn((hidden(), ChildOf(pointer))).id();
    let dot_root = commands.spawn((hidden(), ChildOf(reticle))).id();
    // A part and, a little further from the camera, its dark edge.
    let mut part = |parent: Entity, mesh: &Handle<Mesh>, material: &Handle<StandardMaterial>, behind: bool| {
        let z = if behind { -0.3 } else { 0.0 };
        commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), Transform::from_xyz(0.0, 0.0, z), NotShadowCaster, ChildOf(parent))).id()
    };
    let ticks = [0, 1, 2, 3].map(|_| (part(reticle, &square, &ink, false), part(reticle, &square, &dark, true)));
    part(dot_root, &dot, &ink, false);
    part(dot_root, &dot_edge, &dark, true);
    part(ring_root, &ring, &ink, false);
    part(ring_root, &ring_edge, &dark, true);

    let wedge = commands.spawn((Mesh3d(meshes.add(wedge_mesh(1.0, 90.0))), MeshMaterial3d(ground.clone()), Transform::default(), Visibility::Hidden, NotShadowCaster)).id();
    commands.insert_resource(AimMarker { pointer, reticle, ring: ring_root, ticks, dot: dot_root, wedge, wedge_for: None, ink, ground, gap: None, shown: false });
}

/// Give a material this colour (only when it changes, so nothing is uploaded again for nothing).
fn tint(materials: &mut Assets<StandardMaterial>, handle: &Handle<StandardMaterial>, color: Color) {
    if materials.get(handle).is_some_and(|m| m.base_color != color)
        && let Some(mut m) = materials.get_mut(handle)
    {
        m.base_color = color;
    }
}

#[allow(clippy::too_many_arguments)]
fn show(
    windows: Query<&Window, With<PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform, &IsoCamera)>,
    options: Res<crate::Options>,
    auto: Res<AutoInput>,
    time: Res<Time>,
    join: Res<JoinInfo>,
    link: Res<ServerLink>,
    fog: Res<crate::fog::FogState>,
    me: Res<LocalPlayer>,
    players: Query<(&Player, &Transform)>,
    mut marker: ResMut<AimMarker>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut parts: Query<(&mut Transform, &mut Visibility), Without<Player>>,
    mut wedge_mesh_of: Query<&mut Mesh3d>,
) {
    let marker = &mut *marker;
    let set = |parts: &mut Query<(&mut Transform, &mut Visibility), Without<Player>>, e: Entity, on: bool, tf: Option<Transform>| {
        if let Ok((mut t, mut v)) = parts.get_mut(e) {
            let want = if on { Visibility::Inherited } else { Visibility::Hidden };
            if *v != want {
                *v = want;
            }
            if let Some(tf) = tf {
                *t = tf;
            }
        }
    };
    let hide = |marker: &mut AimMarker, parts: &mut Query<(&mut Transform, &mut Visibility), Without<Player>>| {
        for e in [marker.pointer, marker.wedge] {
            set(parts, e, false, None);
        }
        (marker.gap, marker.shown) = (None, false);
    };

    // `--aim-at` (offscreen pictures): as if aiming with the cursor that far from us on the ground.
    let staged = options.aim_at.filter(|_| options.offscreen()).map(Vec2::from);
    let on_zombie = options.aim_at_zombie && options.offscreen();
    let (Ok((camera, cam_tf, iso)), Ok((player, player_tf))) = (cams.single(), players.single()) else { return hide(marker, &mut parts) };
    if me.dead || !(me.aiming || staged.is_some() || on_zombie) {
        return hide(marker, &mut parts);
    }
    let from = Vec2::new(player_tf.translation.x, player_tf.translation.z);
    // Only what the fog of war draws: the marker mustn't say that a body stands where we see none.
    let seen = |p: Vec2| fog.visibility(p) > 0.35;
    let standing = || link.curr.zombies.iter().filter(|z| !matches!(z.anim, ZombieAnim::Dead | ZombieAnim::Down | ZombieAnim::GetUp)).map(|z| Vec2::from(z.pos)).filter(|&p| seen(p));
    // (where, how high up the cursor's line passes there)
    let staged = match on_zombie {
        true => standing().min_by(|a, b| a.distance_squared(from).total_cmp(&b.distance_squared(from))).map(|z| (z, 1.1)).or(Some((from + Vec2::X * 3.0, 0.0))),
        false => staged.map(|by| (from + by, 0.0)),
    };
    // The line of sight through the cursor. `--autoplay` aims without one: only the wedge shows.
    let sight = match staged {
        Some((at, up)) => Some((Vec3::new(at.x, up, at.y), *cam_tf.forward())),
        None if auto.active => None,
        None => windows
            .single()
            .ok()
            .and_then(|w| Some((w, w.cursor_position()?)))
            .and_then(|(w, cursor)| camera.viewport_to_world(cam_tf, crate::render_scale::to_view(camera, w, cursor)).ok())
            .map(|ray| (ray.origin, *ray.direction)),
    };
    let ground = sight.filter(|(_, dir)| dir.y.abs() > 1e-4).map(|(origin, dir)| origin + dir * (-origin.y / dir.y));
    let facing = match (staged, ground) {
        (Some(_), Some(g)) => (g.z - from.y).atan2(g.x - from.x),
        _ => player.facing,
    };
    // The same on screen at any zoom.
    let k = iso.zoom / ZOOM;
    // A staged picture is of a world that doesn't tick, so `--gun` never reached the hand: take it by name.
    let named = staged.and(options.gun.as_deref()).and_then(|id| join.weapons.iter().find(|w| w.id == id));
    let weapon = named.or_else(|| me.weapon.and_then(|w| join.weapons.get(w as usize)));

    // Where the pointer's parts sit (a point under the cursor), and their colour.
    let mut at: Option<Vec3> = ground;
    let mut color = BONE;
    let gun = weapon.and_then(|w| w.ranged.as_ref());
    if let Some(gun) = gun {
        set(&mut parts, marker.wedge, false, None);
        let mut dist = ground.map_or(0.0, |g| Vec2::new(g.x, g.z).distance(from));
        if let Some((origin, dir)) = sight {
            // As `player::face_cursor` aims it: at the body the cursor is on.
            let zombies = link.curr.zombies.iter().filter(|z| z.anim != ZombieAnim::Dead).map(|z| (Vec2::from(z.pos), matches!(z.anim, ZombieAnim::Down | ZombieAnim::GetUp)));
            let others = link.curr.players.iter().filter(|p| p.id != me.id && !p.dead).map(|p| (Vec2::from(p.pos), p.asleep || p.knock.is_some_and(|k| k.down)));
            if let Some((to, pitch)) = crate::player::gun_aim(from, origin, dir, zombies.chain(others).filter(|&(p, _)| seen(p))) {
                dist = to.length();
                let body = from + to;
                at = Some(Vec3::new(body.x, MUZZLE_HEIGHT + pitch.tan() * dist.max(0.5), body.y));
                color = RED;
            }
        }
        let empty = link.own(me.id).is_some_and(|p| p.inventory.hand.is_some_and(|g| g.used == 0));
        if dist > gun.range || empty {
            color = DIMMED;
        }
        let moving = me.curr.pos.distance_squared(me.prev.pos) > 1e-8;
        let want = spread_radius(gun, dist, moving).max(MIN_GAP * k);
        let gap = marker.gap.map_or(want, |g| g + (want - g) * (1.0 - (-GAP_RATE * time.delta_secs()).exp()));
        marker.gap = Some(gap);
        for (i, (tick, edge)) in marker.ticks.into_iter().enumerate() {
            let out = Vec2::from_angle(i as f32 * FRAC_PI_2) * (gap + TICK.x * k * 0.5);
            let turn = Quat::from_rotation_z(i as f32 * FRAC_PI_2);
            set(&mut parts, tick, true, Some(Transform::from_xyz(out.x, out.y, 0.0).with_rotation(turn).with_scale(Vec3::new(TICK.x * k, TICK.y * k, 1.0))));
            set(&mut parts, edge, true, Some(Transform::from_xyz(out.x, out.y, -0.3).with_rotation(turn).with_scale(Vec3::new((TICK.x + 2.0 * EDGE) * k, (TICK.y + 2.0 * EDGE) * k, 1.0))));
        }
    } else {
        let (reach, arc) = weapon.map_or((0.9, 90.0), |w| (w.range, w.arc_degrees));
        if standing().any(|z| in_swing(from, facing, reach, arc, z)) {
            color = RED;
        }
        if marker.wedge_for != Some((reach, arc))
            && let Ok(mut mesh) = wedge_mesh_of.get_mut(marker.wedge)
        {
            let old = std::mem::replace(&mut mesh.0, meshes.add(wedge_mesh(reach, arc)));
            meshes.remove(&old);
            marker.wedge_for = Some((reach, arc));
        }
        set(&mut parts, marker.wedge, true, Some(Transform::from_xyz(from.x, WEDGE_Y, from.y).with_rotation(Quat::from_rotation_y(-facing))));
        tint(&mut materials, &marker.ground, color);
        marker.gap = None;
    }
    tint(&mut materials, &marker.ink, color);
    set(&mut parts, marker.reticle, gun.is_some(), Some(Transform::default()));
    set(&mut parts, marker.dot, true, Some(Transform::from_scale(Vec3::new(k, k, 1.0))));
    set(&mut parts, marker.ring, gun.is_none(), Some(Transform::from_scale(Vec3::new(k, k, 1.0))));
    match (at, sight) {
        (Some(at), Some((_, dir))) if dir.y < -1e-4 => {
            // Toward the camera along the line of sight: the same place on screen, over everything.
            let lifted = at - dir * ((LIFT - at.y).max(0.0) / -dir.y);
            set(&mut parts, marker.pointer, true, Some(Transform::from_translation(lifted).with_rotation(cam_tf.rotation())));
        }
        _ => set(&mut parts, marker.pointer, false, None),
    }
    marker.shown = true;
}

/// The marker is the pointer while it shows: hide the system's.
fn hide_pointer(marker: Res<AimMarker>, auto: Res<AutoInput>, mut cursors: Query<&mut bevy::window::CursorOptions, With<PrimaryWindow>>) {
    let Ok(mut cursor) = cursors.single_mut() else { return };
    let visible = !(marker.shown && !auto.active);
    if cursor.visible != visible {
        cursor.visible = visible;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gun(spread: f32, moving: f32) -> RangedDef {
        ron::from_str(&format!("(ammo: \"rounds\", capacity: 5, round_seconds: 0.5, range: 40.0, spread_degrees: {spread}, move_spread_degrees: {moving}, far_damage: 0.5)")).expect("a gun")
    }

    #[test]
    fn a_guns_spread_widens_with_distance_and_on_the_move() {
        let pistol = gun(1.2, 3.0);
        let (near, far) = (spread_radius(&pistol, 5.0, false), spread_radius(&pistol, 20.0, false));
        assert!((near - 5.0 * 1.2_f32.to_radians().tan()).abs() < 1e-6 && (far / near - 4.0).abs() < 1e-4, "{near} {far}");
        assert!((spread_radius(&pistol, 20.0, true) / far - 3.5).abs() < 0.01, "moving adds the move spread");
        assert_eq!(spread_radius(&pistol, -1.0, false), 0.0);
        // A shotgun's pattern at 10 m is most of a metre either way.
        assert!((spread_radius(&gun(4.5, 2.0), 10.0, false) - 0.787).abs() < 0.01);
    }

    #[test]
    fn the_swing_test_is_the_servers() {
        let (reach, arc) = (1.4, 90.0);
        // Straight ahead: in reach while any of the body is.
        assert!(in_swing(Vec2::ZERO, 0.0, reach, arc, Vec2::new(1.6, 0.0)));
        assert!(!in_swing(Vec2::ZERO, 0.0, reach, arc, Vec2::new(1.8, 0.0)));
        // Off to the side: 45° is the edge of the arc, and a body just past it still has a
        // shoulder inside.
        assert!(in_swing(Vec2::ZERO, 0.0, reach, arc, Vec2::from_angle(50_f32.to_radians()) * 1.2));
        assert!(!in_swing(Vec2::ZERO, 0.0, reach, arc, Vec2::from_angle(75_f32.to_radians()) * 1.2));
        // Behind: never, unless it stands on us.
        assert!(!in_swing(Vec2::ZERO, 0.0, reach, arc, Vec2::new(-0.8, 0.0)));
        assert!(in_swing(Vec2::ZERO, 0.0, reach, arc, Vec2::ZERO));
        // It turns with the facing.
        assert!(in_swing(Vec2::new(3.0, 3.0), FRAC_PI_2, reach, arc, Vec2::new(3.0, 4.2)));
    }

    #[test]
    fn the_wedge_reaches_the_weapons_reach_and_no_further() {
        for (reach, arc) in [(0.9, 90.0), (1.6, 140.0), (1.1, 360.0)] {
            let mesh = wedge_mesh(reach, arc);
            let Some(bevy::mesh::VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else { panic!("positions") };
            let far = pos.iter().map(|p| Vec2::new(p[0], p[2]).length()).fold(0.0, f32::max);
            assert!((far - reach).abs() < 1e-4, "{reach} m, {arc}°: out to {far}");
            let half = (arc * 0.5_f32).to_radians();
            let widest = pos.iter().filter(|p| Vec2::new(p[0], p[2]).length() > 0.2).map(|p| p[2].atan2(p[0]).abs()).fold(0.0, f32::max);
            assert!(widest <= half + 0.06, "{reach} m, {arc}°: {widest} rad wide of {half}");
            assert!(pos.iter().all(|p| p[1] == 0.0));
        }
    }
}
