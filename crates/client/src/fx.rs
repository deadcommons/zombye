//! Hit feedback, driven by the combat events in snapshots: blood spurts (a few short-lived red
//! cubes), blood splats on the ground (small dark-red decals that stay, oldest removed first), and,
//! when you get hurt, a red flash at the screen edges and a little camera shake. The hit zombie's
//! own flash and jolt are in [`crate::zombie_render`], your flinch in [`crate::survivor`].
//!
//! Kept cheap so it survives hundreds of zombies: every particle shares one mesh and material (one
//! instanced batch), live particles and decals are capped, and nothing is allocated per frame.

use std::collections::VecDeque;

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use zombye_sim::CombatEventKind;

use crate::materials::WorldMaterials;
use crate::meshgen::{MeshBuilder, rgb};
use crate::player::LocalPlayer;
use crate::server_link::CombatEventsThisFrame;

/// Most blood particles alive at once, and most splats on the ground.
const MAX_PARTICLES: usize = 400;
const MAX_DECALS: usize = 240;
/// Splat shapes to pick from.
const SPLAT_SHAPES: u32 = 6;
/// Splats lie on one of these height layers above the ground, a step apart, and never on the same
/// layer as one they overlap (two at the same height flicker where they cross).
const SPLAT_LAYERS: u8 = 10;
const SPLAT_BASE_Y: f32 = 0.008;
const SPLAT_LAYER_STEP: f32 = 0.0012;
/// A splat mesh reaches about this far from its centre at scale 1 (droplets included).
const SPLAT_REACH: f32 = 0.6;
const GRAVITY: f32 = 9.0;

pub struct FxPlugin;

impl Plugin for FxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraShake>()
            .init_resource::<HurtFlash>()
            .init_resource::<Decals>()
            .add_systems(Startup, setup)
            .add_systems(Update, (spawn_fx, move_particles, fade_flash).chain());
    }
}

/// Camera shake, 0..1 (the camera reads it; it decays by itself).
#[derive(Resource, Default)]
pub struct CameraShake {
    pub trauma: f32,
}

/// The red screen-edge flash, 0..1.
#[derive(Resource, Default)]
struct HurtFlash(f32);

#[derive(Resource)]
struct FxAssets {
    cube: Handle<Mesh>,
    blood: Handle<StandardMaterial>,
    splats: Vec<Handle<Mesh>>,
}

/// Splats on the ground, oldest first: entity, centre, reach (m) and height layer.
#[derive(Resource, Default)]
struct Decals(VecDeque<(Entity, Vec2, f32, u8)>);

/// The layer for a new splat given the layers of the ones it overlaps: just above the highest of
/// them (newer blood on top), or, with no room above, the lowest free layer.
fn pick_layer(overlapping: impl Iterator<Item = u8>) -> u8 {
    let (mut used, mut top) = (0u32, None);
    for l in overlapping {
        used |= 1 << l;
        top = top.max(Some(l));
    }
    match top {
        None => 0,
        Some(t) if t + 1 < SPLAT_LAYERS => t + 1,
        _ => (0..SPLAT_LAYERS).find(|l| used & (1 << l) == 0).unwrap_or(0),
    }
}

#[derive(Component)]
struct Particle {
    vel: Vec3,
    life: f32,
}

#[derive(Component)]
struct Vignette;

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>, mut images: ResMut<Assets<Image>>) {
    let splats = (0..SPLAT_SHAPES).map(|i| meshes.add(splat_mesh(i))).collect();
    commands.insert_resource(FxAssets {
        cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        blood: materials.add(StandardMaterial { base_color: Color::srgb(0.5, 0.02, 0.02), perceptual_roughness: 0.35, ..default() }),
        splats,
    });
    commands.spawn((
        Vignette,
        ImageNode { image: images.add(vignette_image()), color: Color::NONE, image_mode: bevy::ui::widget::NodeImageMode::Stretch, ..default() },
        Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        // Above the world, below the HUD text spawned later.
        ZIndex(-1),
    ));
}

/// A rough blob with a few droplets around it, about 1 m across (scaled per splat), on the ground.
fn splat_mesh(seed: u32) -> Mesh {
    let mut rng = Rng(0x9E37_79B9 ^ (seed + 1).wrapping_mul(0x85EB_CA6B));
    let mut mb = MeshBuilder::default();
    let dark = rgb(0.22, 0.012, 0.008);
    let blob = |mb: &mut MeshBuilder, centre: Vec2, radius: f32, sides: u32, rng: &mut Rng, color: LinearRgba| {
        let pts: Vec<Vec2> = (0..sides)
            .map(|k| {
                let a = k as f32 / sides as f32 * std::f32::consts::TAU;
                centre + Vec2::from_angle(a) * radius * rng.range(0.6, 1.15)
            })
            .collect();
        for k in 0..sides as usize {
            let (a, b) = (pts[k], pts[(k + 1) % sides as usize]);
            let tri = [centre, b, a].map(|p| Vec3::new(p.x, 0.0, p.y));
            // Face up.
            let up = (tri[1] - tri[0]).cross(tri[2] - tri[0]).y > 0.0;
            mb.triangle(if up { tri } else { [tri[0], tri[2], tri[1]] }, color);
        }
    };
    blob(&mut mb, Vec2::ZERO, 0.32, 9, &mut rng, dark);
    let shade = rgb(0.3, 0.02, 0.012);
    blob(&mut mb, Vec2::new(rng.range(-0.1, 0.1), rng.range(-0.1, 0.1)), 0.17, 7, &mut rng, shade);
    for _ in 0..4 {
        let at = Vec2::from_angle(rng.range(0.0, std::f32::consts::TAU)) * rng.range(0.38, 0.55);
        blob(&mut mb, at, rng.range(0.03, 0.07), 5, &mut rng, dark);
    }
    mb.build()
}

/// Transparent in the middle, red toward the edges.
fn vignette_image() -> Image {
    let (w, h) = (160u32, 90u32);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
            let v = (y as f32 + 0.5) / h as f32 * 2.0 - 1.0;
            let d = (u * u + v * v).sqrt() / std::f32::consts::SQRT_2;
            let t = ((d - 0.62) / 0.38).clamp(0.0, 1.0);
            let a = t * t * (3.0 - 2.0 * t);
            data.extend_from_slice(&[190, 8, 6, (a * 255.0) as u8]);
        }
    }
    Image::new(Extent3d { width: w, height: h, depth_or_array_layers: 1 }, TextureDimension::D2, data, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD)
}

/// A tiny xorshift for cosmetic randomness (never touches the sim).
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        let mut x = self.0.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_fx(
    mut commands: Commands,
    events: Res<CombatEventsThisFrame>,
    me: Option<Res<LocalPlayer>>,
    assets: Option<Res<FxAssets>>,
    world: Option<Res<WorldMaterials>>,
    mut decals: ResMut<Decals>,
    mut shake: ResMut<CameraShake>,
    mut flash: ResMut<HurtFlash>,
    particles: Query<(), With<Particle>>,
    mut seed: Local<u32>,
) {
    let (Some(assets), Some(world)) = (assets, world) else { return };
    if events.0.is_empty() {
        return;
    }
    *seed = seed.wrapping_add(0x6D2B_79F5);
    let mut rng = Rng(*seed);
    let mut live = particles.iter().count();
    for e in &events.0 {
        let dir = Vec3::new(e.dir.x, 0.0, e.dir.y);
        // (particles, height they burst from, splats, splat size)
        let (count, height, splats, size) = match e.kind {
            CombatEventKind::ZombieHit => (if e.crit { 12 } else { 7 }, 1.35, 1, 1.0),
            CombatEventKind::ZombieKilled => (16, 1.2, 2, 1.5),
            CombatEventKind::Stomp => (9, 0.25, 1, 1.2),
            CombatEventKind::PlayerHurt => (6, 1.25, 1, 0.8),
            CombatEventKind::Shove => (0, 0.0, 0, 0.0),
        };
        if e.kind == CombatEventKind::PlayerHurt && me.as_ref().is_some_and(|m| m.id == e.player) {
            shake.trauma = (shake.trauma + 0.45).min(1.0);
            flash.0 = flash.0.max(0.8);
        }
        let origin = Vec3::new(e.pos.x, height, e.pos.y) - dir * 0.15;
        for _ in 0..count {
            if live >= MAX_PARTICLES {
                break;
            }
            live += 1;
            let spread = Vec3::new(rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)) * 1.3;
            let vel = dir * rng.range(1.0, 3.2) + spread + Vec3::Y * rng.range(0.8, 2.6);
            let s = rng.range(0.035, 0.075);
            commands.spawn((
                Particle { vel, life: rng.range(0.5, 0.9) },
                Mesh3d(assets.cube.clone()),
                MeshMaterial3d(assets.blood.clone()),
                Transform::from_translation(origin + Vec3::Y * rng.range(-0.1, 0.1)).with_scale(Vec3::splat(s)),
                NotShadowCaster,
            ));
        }
        for k in 0..splats {
            // Thrown along the blow; a kill leaves a pool where the body lands.
            let along = if k == 0 { rng.range(0.3, 0.8) } else { rng.range(0.9, 1.4) };
            let at = e.pos + e.dir * along + Vec2::new(rng.range(-0.15, 0.15), rng.range(-0.15, 0.15));
            let scale = size * rng.range(0.75, 1.25);
            let stretch = rng.range(0.8, 1.3);
            while decals.0.len() >= MAX_DECALS {
                if let Some((old, ..)) = decals.0.pop_front() {
                    commands.entity(old).despawn();
                }
            }
            // A height of its own among the splats it overlaps, so none of them flicker.
            let reach = SPLAT_REACH * scale * stretch.max(1.0);
            let layer = pick_layer(decals.0.iter().filter(|(_, c, r, _)| c.distance(at) < r + reach).map(|d| d.3));
            let y = SPLAT_BASE_Y + SPLAT_LAYER_STEP * layer as f32;
            let shape = assets.splats[(rng.next() * SPLAT_SHAPES as f32) as usize % assets.splats.len()].clone();
            let entity = commands
                .spawn((
                    Mesh3d(shape),
                    MeshMaterial3d(world.ground.clone()),
                    Transform::from_xyz(at.x, y, at.y)
                        .with_rotation(Quat::from_rotation_y(rng.range(0.0, std::f32::consts::TAU)))
                        .with_scale(Vec3::new(scale * stretch, 1.0, scale)),
                    NotShadowCaster,
                ))
                .id();
            decals.0.push_back((entity, at, reach, layer));
        }
    }
}

fn move_particles(time: Res<Time>, mut commands: Commands, mut particles: Query<(Entity, &mut Particle, &mut Transform)>) {
    let dt = time.delta_secs();
    for (e, mut p, mut tf) in &mut particles {
        p.life -= dt;
        p.vel.y -= GRAVITY * dt;
        tf.translation += p.vel * dt;
        if p.life <= 0.0 || tf.translation.y <= 0.02 {
            commands.entity(e).despawn();
        }
    }
}

fn fade_flash(time: Res<Time>, mut flash: ResMut<HurtFlash>, mut shake: ResMut<CameraShake>, mut vignette: Query<&mut ImageNode, With<Vignette>>) {
    let dt = time.delta_secs();
    flash.0 = (flash.0 - dt * 3.5).max(0.0);
    shake.trauma = (shake.trauma - dt * 2.2).max(0.0);
    if let Ok(mut img) = vignette.single_mut() {
        let want = Color::srgba(1.0, 1.0, 1.0, (flash.0 * 0.75).min(1.0));
        if img.color != want {
            img.color = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Overlapping splats always get different heights, newest on top while there's room.
    #[test]
    fn overlapping_splats_never_share_a_layer() {
        assert_eq!(pick_layer([].into_iter()), 0);
        assert_eq!(pick_layer([0, 3].into_iter()), 4);
        let full: Vec<u8> = (0..SPLAT_LAYERS).filter(|l| *l != 2).collect();
        assert_eq!(pick_layer(full.into_iter()), 2);
        // A pile of splats on one spot: each new one differs from every one it overlaps while
        // there are layers to go round.
        let mut pile: Vec<u8> = Vec::new();
        for _ in 0..SPLAT_LAYERS {
            let l = pick_layer(pile.iter().copied());
            assert!(!pile.contains(&l), "{l} reused in {pile:?}");
            pile.push(l);
        }
        assert!(SPLAT_LAYER_STEP * SPLAT_LAYERS as f32 + SPLAT_BASE_Y < 0.025, "splats float too high");
    }
}
