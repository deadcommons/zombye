//! How a damaged car looks (#68), from the parts a snapshot gives for it (`VehicleView::parts`:
//! engine, tyres, windows and body, in whole per cents). Client only: nothing here is in the sim.
//!
//! - **The engine** smokes from under the bonnet: thin and pale once it is under half, thick and
//!   dark under a fifth, and for a while after it is wrecked in front of us it burns ([`smoke`]).
//! - **The windows** crack in two stages and are gone when wrecked ([`glass_stage`], [`Glass`]: a
//!   texture of cracks made in code, laid over the panes by where they are, [`crack_uvs`]).
//! - **The body** is scraped dark, more of it the more it has taken ([`body_stage`], the wear's
//!   `scuffs`, baked into the car's vertex colours as its rust and dust are).
//! - **The tyres**: the car sits lower on flat ones ([`tyre_sink`]).
//!
//! `vehicles_render` draws a car again when a stage changes ([`stage`]); the smoke is this
//! module's own.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::server_link::{ClientMap, ServerLink};
use crate::vehicles_render::DrawnCars;

/// The parts' places in `VehicleView::parts` (`zombye_sim::vehicles::PARTS`).
pub const ENGINE: usize = 0;
pub const TYRES: usize = 1;
pub const WINDOWS: usize = 2;
pub const BODY: usize = 3;

/// How scraped the body is drawn, 0 (sound) to 3, by its per cent.
pub fn body_stage(pct: u8) -> u8 {
    match pct {
        85.. => 0,
        60..=84 => 1,
        30..=59 => 2,
        _ => 3,
    }
}

/// How much of the body a stage's scrapes cover (the wear's `scuffs`).
pub fn scuffs(stage: u8) -> f32 {
    [0.0, 0.35, 0.65, 1.0][stage.min(3) as usize]
}

/// The windows: 0 whole, 1 cracked, 2 crazed all over, 3 gone.
pub fn glass_stage(pct: u8) -> u8 {
    match pct {
        80.. => 0,
        45..=79 => 1,
        1..=44 => 2,
        0 => 3,
    }
}

/// The body's and the windows' stages in one number: a car is drawn again when it changes.
pub fn stage(parts: [u8; 4]) -> u8 {
    body_stage(parts[BODY]) * 4 + glass_stage(parts[WINDOWS])
}

/// The two halves of [`stage`]: the body's, the windows'.
pub fn stages(stage: u8) -> (u8, u8) {
    (stage / 4, stage % 4)
}

/// How far (m) a car sits down on its tyres at `pct`: nothing while they hold air, on the rims
/// when they are wrecked.
pub fn tyre_sink(pct: u8) -> f32 {
    const FLAT_BELOW: f32 = 50.0;
    const RIMS: f32 = 0.09;
    ((FLAT_BELOW - pct as f32) / FLAT_BELOW).clamp(0.0, 1.0) * RIMS
}

/// What comes out from under the bonnet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Smoke {
    /// Puffs a second.
    pub rate: f32,
    /// Thick and dark, not thin and pale.
    pub dark: bool,
    /// How big a puff grows (m across) and how long it lasts (s).
    pub size: f32,
    pub life: f32,
}

/// The smoke of an engine at `pct`, running or not (a running one smokes more). None while it is
/// more than half sound.
pub fn smoke(pct: u8, running: bool) -> Option<Smoke> {
    let more = if running { 1.5 } else { 1.0 };
    match pct {
        50.. => None,
        20..=49 => {
            let bad = (50 - pct) as f32 / 30.0;
            Some(Smoke { rate: (2.5 + 4.0 * bad) * more, dark: false, size: 0.5 + 0.3 * bad, life: 1.8 })
        }
        1..=19 => Some(Smoke { rate: 9.0 * more, dark: true, size: 0.95, life: 2.6 }),
        0 => Some(Smoke { rate: 7.0, dark: true, size: 1.1, life: 3.0 }),
    }
}

/// An engine wrecked in front of us burns this long (s), then only smokes.
pub const BURN_SECS: f32 = 45.0;
/// At most this many puffs and flames alive at once.
const MAX_PUFFS: usize = 220;
/// Where the smoke comes out: this share of the car's length ahead of its middle, this high.
const BONNET_ALONG: f32 = 0.33;
const BONNET_HEIGHT: f32 = 0.95;

/// A crack texture's tile is this many metres of glass.
const CRACK_TILE_M: f32 = 0.8;
const CRACK_PX: u32 = 128;

/// UVs for a glass mesh that lay a texture over each pane by where it is in the model: the two
/// axes its normal points least along, [`CRACK_TILE_M`] to a tile.
pub fn crack_uvs(positions: &[[f32; 3]], normals: &[[f32; 3]]) -> Vec<[f32; 2]> {
    positions
        .iter()
        .zip(normals)
        .map(|(p, n)| {
            let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
            let (u, v) = if ay >= ax && ay >= az {
                (p[0], p[2])
            } else if ax >= az {
                (p[2], p[1])
            } else {
                (p[0], p[1])
            };
            [u / CRACK_TILE_M, v / CRACK_TILE_M]
        })
        .collect()
}

/// `mesh` (a car's glass) with [`crack_uvs`] as its UVs. Left as it is without positions and
/// normals to go by.
pub fn with_crack_uvs(mut mesh: Mesh) -> Mesh {
    let uvs = match (mesh.attribute(Mesh::ATTRIBUTE_POSITION), mesh.attribute(Mesh::ATTRIBUTE_NORMAL)) {
        (Some(VertexAttributeValues::Float32x3(p)), Some(VertexAttributeValues::Float32x3(n))) => crack_uvs(p, n),
        _ => return mesh,
    };
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh
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

    fn between(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// How near (in tiles, 0..) the point `p` is to the segment from `a` to `b`, the tile wrapping
/// round at 1.
fn near_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let mut best = f32::MAX;
    for ox in [-1.0, 0.0, 1.0] {
        for oy in [-1.0, 0.0, 1.0] {
            let q = p + Vec2::new(ox, oy);
            let ab = b - a;
            let t = ((q - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
            best = best.min(q.distance(a + ab * t));
        }
    }
    best
}

/// The cracks of a glass stage (1 or 2) as line segments in a tile (0..1 each way): stars of
/// jagged cracks from a few blows, and at stage 2 rings round them.
fn crack_lines(stage: u8) -> Vec<(Vec2, Vec2)> {
    let mut rng = Rng(0x5eed_0000 + stage as u32);
    let (stars, rays, reach) = if stage >= 2 { (4, 9, 0.42) } else { (2, 6, 0.3) };
    let mut lines = Vec::new();
    for _ in 0..stars {
        let hub = Vec2::new(rng.next(), rng.next());
        let mut ring: Vec<Vec<Vec2>> = Vec::new();
        for r in 0..rays {
            let mut angle = (r as f32 + rng.between(-0.3, 0.3)) / rays as f32 * std::f32::consts::TAU;
            let len = reach * rng.between(0.45, 1.0);
            let (mut at, steps) = (hub, 4);
            let mut pts = vec![hub];
            for _ in 0..steps {
                angle += rng.between(-0.35, 0.35);
                let next = at + Vec2::from_angle(angle) * (len / steps as f32);
                lines.push((at, next));
                pts.push(next);
                at = next;
            }
            ring.push(pts);
        }
        if stage >= 2 {
            // Rings: each ray's nth point joined to the next ray's, here and there.
            for k in 1..=2usize {
                for r in 0..ring.len() {
                    if rng.next() < 0.7 {
                        lines.push((ring[r][k], ring[(r + 1) % ring.len()][k]));
                    }
                }
            }
        }
    }
    lines
}

/// The glass of stage 1 or 2 as a tiling texture: see-through glass with pale cracks across it.
fn crack_image(stage: u8) -> Image {
    let lines = crack_lines(stage);
    // Glass between the cracks: a little milkier once it is crazed all over.
    let (glass, half_width) = if stage >= 2 { ([150u8, 166, 178, 105u8], 0.9) } else { ([140, 158, 173, 80], 0.7) };
    let crack = [236.0, 241.0, 246.0, 235.0];
    let mut data = Vec::with_capacity((CRACK_PX * CRACK_PX * 4) as usize);
    for y in 0..CRACK_PX {
        for x in 0..CRACK_PX {
            let p = Vec2::new((x as f32 + 0.5) / CRACK_PX as f32, (y as f32 + 0.5) / CRACK_PX as f32);
            let d = lines.iter().map(|(a, b)| near_segment(p, *a, *b)).fold(f32::MAX, f32::min) * CRACK_PX as f32;
            // 1 on a crack, fading over a pixel at its edge.
            let on = (half_width + 0.5 - d).clamp(0.0, 1.0);
            for k in 0..4 {
                data.push((glass[k] as f32 + (crack[k] - glass[k] as f32) * on) as u8);
            }
        }
    }
    let mut image = Image::new(Extent3d { width: CRACK_PX, height: CRACK_PX, depth_or_array_layers: 1 }, TextureDimension::D2, data, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor { address_mode_u: ImageAddressMode::Repeat, address_mode_v: ImageAddressMode::Repeat, ..ImageSamplerDescriptor::linear() });
    image
}

/// The see-through glass of cars in parts, whole and at each cracked stage (made on first use).
#[derive(Resource, Default)]
pub struct Glass([Option<Handle<StandardMaterial>>; 3]);

impl Glass {
    /// The material for glass at `stage` (0 whole, 1 and 2 cracked; 3, gone, has none).
    pub fn at(&mut self, stage: u8, materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> Option<Handle<StandardMaterial>> {
        let slot = self.0.get_mut(stage as usize)?;
        Some(
            slot.get_or_insert_with(|| {
                let whole = StandardMaterial { base_color: Color::srgba(0.55, 0.62, 0.68, 0.3), alpha_mode: AlphaMode::Blend, perceptual_roughness: 0.08, reflectance: 0.35, ..default() };
                materials.add(match stage {
                    0 => whole,
                    _ => StandardMaterial { base_color: Color::WHITE, base_color_texture: Some(images.add(crack_image(stage))), perceptual_roughness: 0.25, ..whole },
                })
            })
            .clone(),
        )
    }
}

/// A puff of smoke or a lick of flame: rising, growing to `size` and shrinking away at the end
/// of its `life`.
#[derive(Component)]
struct Puff {
    vel: Vec3,
    age: f32,
    life: f32,
    size: f32,
}

#[derive(Resource)]
struct SmokeAssets {
    mesh: Handle<Mesh>,
    pale: Handle<StandardMaterial>,
    dark: Handle<StandardMaterial>,
    flame: Handle<StandardMaterial>,
}

/// What we know of each car's engine between frames: puffs owed, flames owed, the engine's per
/// cent when last looked at, and how long it still burns.
#[derive(Default, Clone, Copy)]
struct Engine {
    owed: f32,
    flames_owed: f32,
    last: Option<u8>,
    burning: f32,
}

#[derive(Resource, Default)]
struct Engines(HashMap<u32, Engine>);

pub struct CarDamagePlugin;

impl Plugin for CarDamagePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Glass>().init_resource::<Engines>().add_systems(Startup, setup).add_systems(Update, (smoke_cars, move_puffs).chain());
    }
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let smoke = |r: f32, a: f32| StandardMaterial { base_color: Color::srgba(r, r, r * 1.03, a), alpha_mode: AlphaMode::Blend, perceptual_roughness: 1.0, reflectance: 0.0, ..default() };
    commands.insert_resource(SmokeAssets {
        mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        pale: materials.add(smoke(0.72, 0.3)),
        dark: materials.add(smoke(0.07, 0.55)),
        flame: materials.add(StandardMaterial { base_color: Color::BLACK, emissive: LinearRgba::rgb(16.0, 6.5, 1.2), perceptual_roughness: 1.0, ..default() }),
    });
}

/// Puffs from the bonnet of every damaged car we can see, and flames from one wrecked just now.
fn smoke_cars(
    mut commands: Commands,
    time: Res<Time>,
    link: Res<ServerLink>,
    map: Res<ClientMap>,
    drawn: Res<DrawnCars>,
    assets: Option<Res<SmokeAssets>>,
    mut engines: ResMut<Engines>,
    puffs: Query<(), With<Puff>>,
    mut rng: Local<u32>,
) {
    let views = &link.curr.vehicles;
    if views.is_empty() {
        if !engines.0.is_empty() {
            engines.0.clear();
        }
        return;
    }
    let Some(assets) = assets else { return };
    let dt = time.delta_secs();
    engines.0.retain(|id, _| views.iter().any(|v| v.id == *id));
    let mut rng = {
        *rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        Rng(*rng | 1)
    };
    let mut room = MAX_PUFFS.saturating_sub(puffs.iter().count());
    for v in views {
        let pct = v.parts[ENGINE];
        let e = engines.0.entry(v.id).or_default();
        // Wrecked while we watched: it burns for a while. One found wrecked only smokes.
        if pct == 0 && e.last.is_some_and(|l| l > 0) {
            e.burning = BURN_SECS;
        }
        e.last = Some(pct);
        e.burning = (e.burning - dt).max(0.0);
        let Some(s) = smoke(pct, v.engine_on) else {
            (e.owed, e.flames_owed) = (0.0, 0.0);
            continue;
        };
        let Some(car) = drawn.0.iter().find(|c| c.id == v.id && c.shown) else { continue };
        let dir = Vec2::from_angle(car.heading);
        let side = dir.perp();
        let bonnet = car.pos + dir * (car.length * BONNET_ALONG);
        let base = Vec3::new(bonnet.x, map.0.ground(bonnet) + BONNET_HEIGHT, bonnet.y);
        // The car's own speed is left behind by its smoke: it trails.
        e.owed = (e.owed + s.rate * dt).min(6.0);
        while e.owed >= 1.0 && room > 0 {
            e.owed -= 1.0;
            room -= 1;
            let off = side * rng.between(-0.35, 0.35) * car.width + dir * rng.between(-0.25, 0.25);
            let at = base + Vec3::new(off.x, rng.between(-0.05, 0.1), off.y);
            let vel = Vec3::new(rng.between(-0.25, 0.25) + 0.35, rng.between(0.9, 1.5), rng.between(-0.25, 0.25));
            let puff = Puff { vel, age: 0.0, life: s.life * rng.between(0.75, 1.25), size: s.size * rng.between(0.7, 1.2) };
            let turn = Quat::from_euler(EulerRot::XYZ, rng.between(0.0, 6.28), rng.between(0.0, 6.28), rng.between(0.0, 6.28));
            let tf = Transform::from_translation(at).with_rotation(turn).with_scale(Vec3::splat(0.08));
            let material = if s.dark { assets.dark.clone() } else { assets.pale.clone() };
            commands.spawn((puff, Mesh3d(assets.mesh.clone()), MeshMaterial3d(material), tf, NotShadowCaster));
        }
        if e.burning > 0.0 {
            // Dying down over its last ten seconds.
            e.flames_owed = (e.flames_owed + 14.0 * (e.burning / 10.0).min(1.0) * dt).min(6.0);
            while e.flames_owed >= 1.0 && room > 0 {
                e.flames_owed -= 1.0;
                room -= 1;
                let off = side * rng.between(-0.3, 0.3) * car.width + dir * rng.between(-0.3, 0.3);
                let at = base + Vec3::new(off.x, rng.between(-0.1, 0.05), off.y);
                let puff = Puff { vel: Vec3::new(0.0, rng.between(1.2, 2.2), 0.0), age: 0.0, life: rng.between(0.25, 0.5), size: rng.between(0.16, 0.34) };
                let tf = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(rng.between(0.0, 6.28))).with_scale(Vec3::splat(0.08));
                commands.spawn((puff, Mesh3d(assets.mesh.clone()), MeshMaterial3d(assets.flame.clone()), tf, NotShadowCaster));
            }
        }
    }
}

/// A puff's size (as a share of its full size) `u` (0..1) of the way through its life: it swells
/// fast, then thins away to nothing.
pub fn puff_scale(u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    let grow = (u / 0.35).min(1.0);
    let grow = 0.15 + 0.85 * grow * (2.0 - grow);
    let fade = ((1.0 - u) / 0.4).min(1.0);
    grow * fade
}

fn move_puffs(time: Res<Time>, mut commands: Commands, mut puffs: Query<(Entity, &mut Puff, &mut Transform)>) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (e, mut p, mut tf) in &mut puffs {
        p.age += dt;
        if p.age >= p.life {
            commands.entity(e).despawn();
            continue;
        }
        // It slows as it rises and drifts with the air.
        let slow = (1.0 - 0.6 * dt).max(0.0);
        p.vel.x *= slow;
        p.vel.z *= slow;
        p.vel.y *= (1.0 - 0.25 * dt).max(0.0);
        tf.translation += p.vel * dt;
        tf.rotate_y(0.6 * dt);
        tf.scale = Vec3::splat(p.size * puff_scale(p.age / p.life));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sound car shows nothing; each part's damage shows in steps, worse the lower it is.
    #[test]
    fn damage_shows_in_stages() {
        assert_eq!(stage([100; 4]), 0);
        assert_eq!((body_stage(100), body_stage(84), body_stage(59), body_stage(0)), (0, 1, 2, 3));
        assert_eq!((glass_stage(100), glass_stage(79), glass_stage(44), glass_stage(1), glass_stage(0)), (0, 1, 2, 2, 3));
        for body in [100u8, 70, 40, 5] {
            for glass in [100u8, 60, 20, 0] {
                assert_eq!(stages(stage([100, 100, glass, body])), (body_stage(body), glass_stage(glass)));
            }
        }
        assert!(scuffs(0) == 0.0 && scuffs(1) < scuffs(2) && scuffs(3) == 1.0);
        // The engine and the tyres don't draw the car again.
        assert_eq!(stage([0, 0, 100, 100]), 0);
    }

    #[test]
    fn a_failing_engine_smokes_more_and_darker() {
        assert_eq!(smoke(100, true), None);
        assert_eq!(smoke(50, true), None);
        let (light, worse, dark, dead) = (smoke(49, false).unwrap(), smoke(25, false).unwrap(), smoke(10, false).unwrap(), smoke(0, false).unwrap());
        assert!(!light.dark && !worse.dark && dark.dark && dead.dark);
        assert!(light.rate < worse.rate && worse.rate < dark.rate);
        assert!(smoke(30, true).unwrap().rate > smoke(30, false).unwrap().rate, "a running one smokes more");
    }

    #[test]
    fn flat_tyres_sit_the_car_down() {
        assert_eq!(tyre_sink(100), 0.0);
        assert_eq!(tyre_sink(50), 0.0);
        assert!(tyre_sink(25) > 0.0 && tyre_sink(25) < tyre_sink(0));
        assert!(tyre_sink(0) < 0.12, "never through the ground");
    }

    #[test]
    fn a_puff_swells_then_thins_away() {
        assert!(puff_scale(0.0) > 0.0 && puff_scale(0.0) < 0.3);
        assert!((puff_scale(0.5) - 1.0).abs() < 1e-5);
        assert!(puff_scale(0.9) < 0.5 && puff_scale(1.0) == 0.0);
    }

    /// The cracks are a texture that tiles, with glass left between them; the worse stage has
    /// more of them.
    #[test]
    fn cracks_cover_some_of_the_glass() {
        let share = |stage: u8| {
            let image = crack_image(stage);
            let data = image.data.as_ref().expect("pixels");
            data.chunks(4).filter(|p| p[3] > 180).count() as f32 / (CRACK_PX * CRACK_PX) as f32
        };
        let (one, two) = (share(1), share(2));
        assert!(one > 0.01 && one < 0.2, "{one}");
        assert!(two > one * 1.5 && two < 0.45, "{two}");
        // A pane's UVs go by the axes it lies in: a windscreen (facing along z) by x and y.
        let uv = crack_uvs(&[[0.4, 1.2, 2.0]], &[[0.0, 0.3, 0.95]]);
        assert_eq!(uv, vec![[0.4 / CRACK_TILE_M, 1.2 / CRACK_TILE_M]]);
        let roof = crack_uvs(&[[0.4, 1.5, 0.8]], &[[0.0, 1.0, 0.0]]);
        assert_eq!(roof, vec![[0.4 / CRACK_TILE_M, 0.8 / CRACK_TILE_M]]);
    }
}
