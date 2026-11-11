//! Instanced vegetation and litter (Linear DCM-59): grass in three lengths, weeds in the cracks,
//! drifts of red and white wildflowers, loose papers and small stones.
//!
//! **Where** is worked out by the chunk mesher from the tile map alone
//! (`meshgen::landscape::scatter`, deterministic: the same map always grows the same grass), as a
//! list of [`VegSpot`]s per chunk, kept on the chunk's ground entity ([`ChunkVeg`]).
//!
//! **Drawing** reuses the crowd's instanced path (`zombie_render.rs`): every spot is an entity
//! with one of a few dozen small shared meshes ([`KINDS`] x [`VARIANTS`], built here in code),
//! one shared material and a `MeshTag` for its colour, so Bevy batches all the instances of a mesh
//! into one draw; no spot has a mesh, a material or a draw call of its own, and nothing is
//! allocated per frame. The material ([`VegMaterial`]) is the world material's fragment shader
//! (lighting, shadows, fog of war) behind `assets/shaders/vegetation.wesl`, which colours each
//! patch, sways it in the wind and bends it away from the player's feet. Vegetation casts no
//! shadows (it would double the crowd's shadow pass for something ankle-high).
//!
//! **Culling:** only chunks within [`veg_radius`] of the camera's focus have their spots spawned
//! (a few chunks a frame as you walk), the shader shrinks patches to nothing over the last 16% of
//! that radius so nothing pops, and Bevy's frustum culling drops what is off screen.
//!
//! **Settings** (`graphics.rs`): `vegetation` 0..2 is the density: each spot has a rank and is
//! drawn when the density is above it (ranks under 1 are the default cover, 1..2 extra patches
//! between them); `--vegetation off` spawns nothing at all.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshTag, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Face, RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;
use zombye_sim::map::CHUNK_SIZE;

use crate::Options;
use crate::camera::IsoCamera;
use crate::fog::FogState;
use crate::graphics::Graphics;
use crate::materials::{ParamBuffer, param_buffer, write_params};
use crate::meshgen::palette::pal;
use crate::player::Player;

/// What a spot grows or holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VegKind {
    /// Long grass: unmown edges, verges, vacant lots, the forest's edge.
    GrassLong = 0,
    /// Lawn gone a little wild.
    Grass = 1,
    /// Thin, short grass: worn ground, under trees, on dirt.
    GrassThin = 2,
    /// A weed in a crack of the pavement.
    Weed = 3,
    FlowersRed = 4,
    FlowersWhite = 5,
    /// A few loose sheets of paper.
    Paper = 6,
    /// A few small stones.
    Pebbles = 7,
}

pub const KINDS: usize = 8;
/// Different meshes per kind.
pub const VARIANTS: usize = 4;

impl VegKind {
    pub const ALL: [VegKind; KINDS] =
        [VegKind::GrassLong, VegKind::Grass, VegKind::GrassThin, VegKind::Weed, VegKind::FlowersRed, VegKind::FlowersWhite, VegKind::Paper, VegKind::Pebbles];

    /// The kind's name in the release models' `veg/` folder.
    pub fn file(self) -> &'static str {
        ["grass_long", "grass", "grass_thin", "weed", "flowers_red", "flowers_white", "paper", "pebbles"][self as usize]
    }
}

/// Where the ground cover grows and how dry it is, from the data folder's `vegetation.ron` (every
/// field optional: one left out takes its default, no file at all the shipped one).
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VegLook {
    pub dry: f32,
    pub long_from: f32,
    pub scale: f32,
    pub long_scale: f32,
    pub flowers: f32,
    pub flower_cover: f32,
    pub verge_stones: f32,
    pub dirt_stones: f32,
}

impl Default for VegLook {
    fn default() -> Self {
        VegLook { dry: 0.0, long_from: 0.6, scale: 1.0, long_scale: 1.0, flowers: 1.4, flower_cover: 0.4, verge_stones: 0.1, dirt_stones: 0.12 }
    }
}

const SHIPPED_LOOK: &str = include_str!("../../../assets/data/vegetation.ron");
static LOOK: std::sync::OnceLock<VegLook> = std::sync::OnceLock::new();

impl VegLook {
    pub fn parse(text: &str) -> Result<VegLook, String> {
        let look: VegLook = ron::from_str(text).map_err(|e| e.to_string())?;
        let within = |name: &str, v: f32, lo: f32, hi: f32| if (lo..=hi).contains(&v) { Ok(()) } else { Err(format!("{name} must be from {lo} to {hi}, not {v}")) };
        within("dry", look.dry, -1.0, 1.0)?;
        within("long_from", look.long_from, 0.0, 1.0)?;
        within("scale", look.scale, 0.25, 2.0)?;
        within("long_scale", look.long_scale, 0.25, 2.0)?;
        within("flowers", look.flowers, 0.0, 10.0)?;
        within("flower_cover", look.flower_cover, 0.0, 1.0)?;
        within("verge_stones", look.verge_stones, 0.0, 1.0)?;
        within("dirt_stones", look.dirt_stones, 0.0, 1.0)?;
        Ok(look)
    }
}

/// Use `vegetation.ron` from the data folder `dir` (the shipped one if it has none). Call before
/// any chunk is built; later calls change nothing.
pub fn load_look(dir: &std::path::Path) -> Result<(), String> {
    let path = dir.join("vegetation.ron");
    let look = match std::fs::read_to_string(&path) {
        Ok(text) => VegLook::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(_) => shipped_look(),
    };
    let _ = LOOK.set(look);
    Ok(())
}

fn shipped_look() -> VegLook {
    VegLook::parse(SHIPPED_LOOK).expect("the shipped vegetation.ron is good (a test checks)")
}

/// The ground cover's look: the data folder's once [`load_look`]ed, else the shipped one.
pub fn veg_look() -> &'static VegLook {
    LOOK.get_or_init(shipped_look)
}

/// One patch: where, what, how it's turned and scaled, how dry (0 fresh .. 1 straw), and its rank
/// against the density setting (drawn when the density is above it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VegSpot {
    pub pos: Vec2,
    pub kind: VegKind,
    pub variant: u8,
    pub turn: f32,
    pub scale: f32,
    pub dry: f32,
    pub rank: f32,
}

/// A chunk's spots, on its ground entity (`world_render::spawn_chunk`).
#[derive(Component)]
pub struct ChunkVeg {
    pub chunk: IVec2,
    pub spots: Vec<VegSpot>,
}

/// The spot entities of a chunk while they are spawned (its children).
#[derive(Component)]
struct VegShown(Vec<Entity>);

/// Vegetation is drawn out to this far from the camera's focus (metres) at this zoom (metres of
/// world visible top to bottom): just past the corners of a 16:9 view, within limits.
pub fn veg_radius(zoom: f32) -> f32 {
    (zoom * 1.34 + 3.0).clamp(MIN_RADIUS, MAX_RADIUS)
}

const MIN_RADIUS: f32 = 24.0;
/// The view's corners are further than this from the focus above a zoom of about 42 m; from there
/// the grass thins out toward the edges of the screen (at that scale a blade is two pixels).
const MAX_RADIUS: f32 = 60.0;
/// A chunk's spots are removed once it is this much further than the radius.
const KEEP_MARGIN: f32 = 8.0;
/// Chunks whose spots are spawned per frame while walking.
const CHUNKS_PER_FRAME: usize = 2;
/// How hard the wind leans the grass (metres of sway per metre of blade).
const WIND: f32 = 0.16;

pub type VegMaterial = ExtendedMaterial<StandardMaterial, VegExt>;

/// The world material's extension (same bindings, so `world.wesl` is its fragment shader) plus the
/// vegetation's own parameters for `vegetation.wesl`.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct VegExt {
    /// The world materials' own buffer (`materials::ParamBuffer`).
    #[storage(100, read_only)]
    pub params: Handle<ShaderBuffer>,
    #[texture(101)]
    #[sampler(102)]
    pub fog: Handle<Image>,
    #[texture(103, sample_type = "float", filterable = false)]
    pub rays: Handle<Image>,
    /// [`VegParams`], in a buffer for the reason the world's are: the clock in them changes every
    /// frame, and a changed material would have every patch of grass extracted and queued again.
    #[storage(104, read_only)]
    pub veg: Handle<ShaderBuffer>,
}

/// Must match `VegParams` in vegetation.wesl: four `vec4<f32>`.
#[derive(Debug, Clone, Default)]
pub struct VegParams {
    /// x, y = the camera's focus on the ground (world x, z), z = the drawn radius, w = seconds.
    pub focus: Vec4,
    /// x, y = the player's feet (world x, z), z = wind strength.
    pub player: Vec4,
    /// Blade colours, fresh and dry (linear).
    pub green: Vec4,
    pub dry: Vec4,
}

impl MaterialExtension for VegExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/vegetation.wesl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/world.wesl".into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        _descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        Ok(())
    }
}

impl VegParams {
    fn floats(&self) -> Vec<f32> {
        [self.focus, self.player, self.green, self.dry].iter().flat_map(|v| v.to_array()).collect()
    }
}

#[derive(Resource)]
struct VegAssets {
    meshes: [[Handle<Mesh>; VARIANTS]; KINDS],
    material: Handle<VegMaterial>,
    /// The buffer holding the material's [`VegParams`].
    params: Handle<ShaderBuffer>,
}

pub struct VegetationPlugin;

impl Plugin for VegetationPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<VegMaterial>::default())
            .add_systems(Startup, setup)
            .add_systems(Update, stream_veg)
            .add_systems(PostUpdate, update_params.after(crate::materials::update_params));
    }
}

fn setup(mut commands: Commands, fog: Res<FogState>, world: Res<ParamBuffer>, mut buffers: ResMut<Assets<ShaderBuffer>>, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<VegMaterial>>) {
    let handles = VegKind::ALL.map(|kind| std::array::from_fn(|v| meshes.add(patch_mesh(kind, v as u32))));
    let params = param_buffer(&mut buffers, VegParams::default().floats());
    let material = materials.add(ExtendedMaterial {
        // Blades are seen from both sides, lit the same on each (the shader gives them the ground's
        // normal), so no culling and no flipping of normals for back faces.
        base: StandardMaterial { base_color: Color::WHITE, perceptual_roughness: 0.95, reflectance: 0.2, cull_mode: None::<Face>, ..default() },
        extension: VegExt { params: world.0.clone(), fog: fog.image.clone(), rays: fog.rays.clone(), veg: params.clone() },
    });
    commands.insert_resource(VegAssets { meshes: handles, material, params });
}

/// The tint and variety a spot's instance carries to the shader: bits 0-7 dryness, 8-15 brightness.
fn spot_tag(spot: &VegSpot) -> u32 {
    let bright = crate::meshgen::hash01((spot.pos.x * 16.0) as i32, (spot.pos.y * 16.0) as i32, 901);
    (spot.dry.clamp(0.0, 1.0) * 255.0) as u32 | ((bright * 255.0) as u32) << 8
}

/// Shortest distance from `p` to chunk `c`'s square on the ground.
fn chunk_distance(c: IVec2, p: Vec2) -> f32 {
    let lo = (c * CHUNK_SIZE).as_vec2();
    let hi = lo + Vec2::splat(CHUNK_SIZE as f32);
    (p - p.clamp(lo, hi)).length()
}

/// Spawn the spots of chunks that came within the drawn radius, remove those of chunks that left.
fn stream_veg(
    mut commands: Commands,
    graphics: Res<Graphics>,
    options: Res<Options>,
    assets: Option<Res<VegAssets>>,
    cams: Query<&IsoCamera>,
    chunks: Query<(Entity, &ChunkVeg, Option<&VegShown>)>,
    mut drawn_at: Local<Option<f32>>,
) {
    let (Some(assets), Ok(cam)) = (assets, cams.single()) else { return };
    let density = graphics.vegetation;
    // A new density (the graphics preset changed): take every chunk's spots down; they come back
    // at the new density a few chunks a frame.
    if drawn_at.is_some_and(|d| d != density) {
        for (entity, _, shown) in &chunks {
            if let Some(VegShown(spawned)) = shown {
                for &e in spawned {
                    commands.entity(e).despawn();
                }
                commands.entity(entity).remove::<VegShown>();
            }
        }
        *drawn_at = Some(density);
        return;
    }
    *drawn_at = Some(density);
    let focus = cam.focus.xz();
    let radius = veg_radius(cam.zoom);
    // Everything at once for a picture and for the first view; then a few chunks a frame.
    let all_at_once = options.offscreen() || chunks.iter().all(|(.., shown)| shown.is_none());
    let mut budget = if all_at_once { usize::MAX } else { CHUNKS_PER_FRAME };
    for (entity, veg, shown) in &chunks {
        let d = chunk_distance(veg.chunk, focus);
        match shown {
            None if density > 0.0 && d < radius && budget > 0 => {
                budget -= 1;
                let mut spawned = Vec::with_capacity(veg.spots.len());
                commands.entity(entity).with_children(|c| {
                    for spot in veg.spots.iter().filter(|s| s.rank < density) {
                        let mesh = assets.meshes[spot.kind as usize][spot.variant as usize % VARIANTS].clone();
                        let transform = Transform::from_xyz(spot.pos.x, 0.0, spot.pos.y).with_rotation(Quat::from_rotation_y(spot.turn)).with_scale(Vec3::splat(spot.scale));
                        spawned.push(c.spawn((Mesh3d(mesh), MeshMaterial3d(assets.material.clone()), transform, MeshTag::new(spot_tag(spot)), NotShadowCaster)).id());
                    }
                });
                commands.entity(entity).insert(VegShown(spawned));
            }
            Some(VegShown(spawned)) if density <= 0.0 || d > radius + KEEP_MARGIN => {
                for &e in spawned {
                    commands.entity(e).despawn();
                }
                commands.entity(entity).remove::<VegShown>();
            }
            _ => {}
        }
    }
}

/// The vegetation material reads the world's parameters (fog of war, night) from the world's own
/// buffer, and gets its own clock, wind, focus and the player's feet in another.
fn update_params(assets: Option<Res<VegAssets>>, mut buffers: ResMut<Assets<ShaderBuffer>>, time: Res<Time>, cams: Query<&IsoCamera>, player: Query<&Transform, With<Player>>) {
    let (Some(assets), Ok(cam)) = (assets, cams.single()) else { return };
    let feet = player.single().map_or(Vec2::splat(-1e6), |p| p.translation.xz());
    let p = pal();
    let veg = VegParams {
        focus: Vec4::new(cam.focus.x, cam.focus.z, veg_radius(cam.zoom), time.elapsed_secs_wrapped()),
        player: Vec4::new(feet.x, feet.y, WIND, 0.0),
        green: p.grass_blade.to_vec4(),
        dry: p.grass_blade_dry.to_vec4(),
    };
    write_params(&mut buffers, &assets.params, &veg.floats());
}

/// A tiny xorshift for building the meshes (always the same meshes).
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

/// Triangles with a colour per corner; every normal points up (see vegetation.wesl).
#[derive(Default)]
struct Patch {
    positions: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
}

impl Patch {
    fn tri(&mut self, corners: [Vec3; 3], colors: [[f32; 4]; 3]) {
        for (p, c) in corners.into_iter().zip(colors) {
            self.positions.push(p.to_array());
            self.colors.push(c);
        }
    }

    /// A blade of grass: one triangle from a base `w` wide up to a tip `h` high, leaning `lean`
    /// (as a share of its height) along `dir`. Darker at the root, as if shaded by its neighbours:
    /// the ground under thick grass reads as occluded without any screen-space pass.
    fn blade(&mut self, root: Vec3, dir: Vec2, h: f32, w: f32, lean: f32, tone: f32) {
        let side = Vec3::new(-dir.y, 0.0, dir.x) * w * 0.5;
        let tip = root + Vec3::new(dir.x * lean * h, h, dir.y * lean * h);
        let (dark, light) = ([0.5 * tone, 0.5 * tone, 0.5 * tone, 1.0], [1.08 * tone, 1.08 * tone, 1.08 * tone, 1.0]);
        self.tri([root - side, root + side, tip], [dark, dark, light]);
    }

    /// A tuft: `n` blades fanning out from a point.
    fn tuft(&mut self, rng: &mut Rng, at: Vec2, n: u32, h: (f32, f32), w: f32) {
        let a0 = rng.range(0.0, std::f32::consts::TAU);
        for i in 0..n {
            let a = a0 + (i as f32 + rng.range(-0.3, 0.3)) * std::f32::consts::TAU / n as f32;
            let dir = Vec2::from_angle(a);
            let root = Vec3::new(at.x + dir.x * 0.03, 0.0, at.y + dir.y * 0.03);
            self.blade(root, dir, rng.range(h.0, h.1), w * rng.range(0.8, 1.25), rng.range(0.15, 0.7), rng.range(0.82, 1.1));
        }
    }

    /// A flat-shaded little pyramid (a stone, a flower head) of its own colour.
    fn lump(&mut self, rng: &mut Rng, at: Vec3, r: f32, h: f32, sides: u32, colour: LinearRgba) {
        let a0 = rng.range(0.0, std::f32::consts::TAU);
        let ring: Vec<Vec3> = (0..sides).map(|i| at + Vec3::new((a0 + i as f32 * std::f32::consts::TAU / sides as f32).cos(), 0.0, (a0 + i as f32 * std::f32::consts::TAU / sides as f32).sin()) * r * rng.range(0.75, 1.2)).collect();
        let top = at + Vec3::new(rng.range(-0.3, 0.3) * r, h, rng.range(-0.3, 0.3) * r);
        for i in 0..sides as usize {
            let k = 0.78 + 0.3 * (i as f32 / sides as f32) + rng.range(-0.04, 0.04);
            let c = [colour.red * k, colour.green * k, colour.blue * k, 0.0];
            self.tri([ring[i], ring[(i + 1) % sides as usize], top], [c; 3]);
        }
    }

    fn build(self) -> Mesh {
        let n = self.positions.len();
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32, 1.0, 0.0]; n])
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
            .with_inserted_indices(Indices::U32((0..n as u32).collect()))
    }
}

/// Tufts in a patch, blades in a tuft, blade height (m) and width, per kind of grass.
fn grass_shape(kind: VegKind) -> (u32, u32, (f32, f32), f32) {
    match kind {
        VegKind::GrassLong => (13, 7, (0.24, 0.5), 0.085),
        VegKind::Grass => (12, 6, (0.12, 0.26), 0.08),
        VegKind::GrassThin => (4, 5, (0.1, 0.22), 0.07),
        _ => (1, 6, (0.12, 0.26), 0.06),
    }
}

/// The mesh of one variant of a kind: a patch about a metre across round the origin, on the ground.
fn patch(kind: VegKind, variant: u32) -> Patch {
    let mut rng = Rng(0x9E37_79B9 ^ (kind as u32 + 1).wrapping_mul(0x85EB_CA6B) ^ (variant + 1).wrapping_mul(0xC2B2_AE35));
    let mut m = Patch::default();
    let p = pal();
    let spot = |rng: &mut Rng, reach: f32| Vec2::new(rng.range(-reach, reach), rng.range(-reach, reach));
    match kind {
        VegKind::GrassLong | VegKind::Grass | VegKind::GrassThin | VegKind::Weed => {
            let (tufts, blades, h, w) = grass_shape(kind);
            for _ in 0..tufts {
                let at = if kind == VegKind::Weed { Vec2::ZERO } else { spot(&mut rng, 0.46) };
                m.tuft(&mut rng, at, blades, h, w);
            }
        }
        VegKind::FlowersRed | VegKind::FlowersWhite => {
            let colour = if kind == VegKind::FlowersRed { p.flower_red } else { p.flower_cream };
            // Flowers on thin stems among a little grass.
            for _ in 0..3 {
                let at = spot(&mut rng, 0.42);
                m.tuft(&mut rng, at, 4, (0.14, 0.3), 0.06);
            }
            for _ in 0..rng.range(5.0, 9.0) as u32 {
                let at = spot(&mut rng, 0.45);
                let h = rng.range(0.22, 0.46);
                let dir = Vec2::from_angle(rng.range(0.0, std::f32::consts::TAU));
                let lean = rng.range(0.0, 0.25);
                m.blade(Vec3::new(at.x, 0.0, at.y), dir, h, 0.035, lean, 0.9);
                let head = Vec3::new(at.x + dir.x * lean * h, h - 0.02, at.y + dir.y * lean * h);
                let (r, up) = (rng.range(0.04, 0.062), rng.range(0.04, 0.07));
                m.lump(&mut rng, head, r, up, 4, colour);
            }
        }
        VegKind::Paper => {
            // One to three sheets, a corner or an edge of each lifted a little.
            for i in 0..1 + variant % 3 {
                let at = spot(&mut rng, 0.3);
                let a = rng.range(0.0, std::f32::consts::TAU);
                let (u, v) = (Vec2::from_angle(a) * 0.105, Vec2::from_angle(a).perp() * 0.148);
                let y = 0.012 + 0.004 * i as f32;
                let lift = [rng.range(0.0, 0.035), 0.0, rng.range(0.0, 0.02), 0.0];
                let corner = |k: usize, s: Vec2| Vec3::new(at.x + s.x, y + lift[k], at.y + s.y);
                let c = [corner(0, -u - v), corner(1, u - v), corner(2, u + v), corner(3, -u + v)];
                let tone = rng.range(0.82, 1.0);
                let sheet = [p.paper.red * tone, p.paper.green * tone, p.paper.blue * tone, 0.0];
                m.tri([c[0], c[1], c[2]], [sheet; 3]);
                m.tri([c[0], c[2], c[3]], [sheet; 3]);
                // A block of print across part of it.
                if rng.next() < 0.6 {
                    let ink = [sheet[0] * 0.55, sheet[1] * 0.55, sheet[2] * 0.58, 0.0];
                    let (lo, hi) = (rng.range(-0.7, -0.1), rng.range(0.1, 0.7));
                    let q = |su: f32, sv: f32| Vec3::new(at.x + u.x * su + v.x * sv, y + 0.002 + lift[0].max(lift[2]) * 0.0, at.y + u.y * su + v.y * sv);
                    let b = [q(-0.75, lo), q(0.75, lo), q(0.75, hi), q(-0.75, hi)];
                    m.tri([b[0], b[1], b[2]], [ink; 3]);
                    m.tri([b[0], b[2], b[3]], [ink; 3]);
                }
            }
        }
        VegKind::Pebbles => {
            for _ in 0..2 + variant {
                let at = spot(&mut rng, 0.4);
                let r = rng.range(0.035, 0.085);
                let up = r * rng.range(0.5, 0.9);
                m.lump(&mut rng, Vec3::new(at.x, 0.0, at.y), r, up, 5, p.rock);
            }
        }
    }
    m
}

/// Triangles in a kind's variant (for the cost figures).
#[cfg(test)]
pub fn patch_triangles(kind: VegKind, variant: u8) -> usize {
    patch(kind, variant as u32 % VARIANTS as u32).positions.len() / 3
}

/// A patch from a release model (see `release_models::veg`): its `leaves` triangles become blades,
/// tinted by the game and dark at the root as the generated ones are; the rest keep their colours.
fn model_patch(tris: &[([Vec3; 3], LinearRgba, bool)]) -> Patch {
    let mut m = Patch::default();
    let top = tris.iter().filter(|t| t.2).flat_map(|t| t.0).fold(0.05f32, |a, p| a.max(p.y));
    for (i, (corners, colour, leaf)) in tris.iter().enumerate() {
        let colours = corners.map(|p| {
            if *leaf {
                // Whole blades differ a little, by a hash of where they stand in the list.
                let vary = 0.9 + 0.2 * crate::meshgen::hash01(i as i32 / 2, 7, 911);
                let t = (0.42 + 0.7 * (p.y / top).clamp(0.0, 1.0).powf(0.8)) * vary;
                [t, t, t, 1.0]
            } else {
                [colour.red, colour.green, colour.blue, 0.0]
            }
        });
        m.tri(*corners, colours);
    }
    m
}

fn patch_mesh(kind: VegKind, variant: u32) -> Mesh {
    let models = crate::release_models::veg();
    let n = models.variants(kind.file());
    if n > 0 {
        let k = variant % n;
        let name = if k == 0 { kind.file().to_string() } else { format!("{}.{}", kind.file(), k + 1) };
        if let Some(tris) = models.raw(&name) {
            return model_patch(&tris).build();
        }
    }
    patch(kind, variant).build()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Meshes stay small (they are drawn thousands of times), sit on the ground within their
    /// patch, and are the same every time.
    #[test]
    fn patch_meshes_are_small_grounded_and_repeatable() {
        for kind in VegKind::ALL {
            for v in 0..VARIANTS as u32 {
                let m = patch(kind, v);
                let tris = m.positions.len() / 3;
                assert!((1..=96).contains(&tris), "{kind:?} {v}: {tris} triangles");
                for p in &m.positions {
                    assert!(p[1] >= 0.0 && p[1] < 0.75, "{kind:?} {v}: a vertex {} m up", p[1]);
                    assert!(p[0].abs() < 0.8 && p[2].abs() < 0.8, "{kind:?} {v}: a vertex outside its patch");
                }
                assert_eq!(m.positions, patch(kind, v).positions);
            }
            assert_ne!(patch(kind, 0).positions, patch(kind, 1).positions, "{kind:?}: variants differ");
        }
    }

    /// The drawn radius covers the corners of the view at ordinary zooms and is capped beyond.
    #[test]
    fn the_radius_follows_the_zoom() {
        // Half the view on the ground at zoom z (16:9, seen from 30° up): 0.89 z across, z deep.
        for zoom in [8.0f32, 18.0, 26.0, 40.0] {
            let corner = (0.89f32 * zoom).hypot(zoom);
            assert!(veg_radius(zoom) >= corner.min(MAX_RADIUS), "zoom {zoom}");
        }
        assert_eq!(veg_radius(90.0), MAX_RADIUS);
        assert_eq!(veg_radius(8.0), MIN_RADIUS);
        assert_eq!(chunk_distance(IVec2::new(1, 1), Vec2::new(20.0, 20.0)), 0.0);
        assert_eq!(chunk_distance(IVec2::new(1, 1), Vec2::new(10.0, 20.0)), 6.0);
    }

    #[test]
    fn tags_carry_dryness_and_brightness() {
        let spot = |dry: f32| VegSpot { pos: Vec2::new(3.5, 9.25), kind: VegKind::Grass, variant: 0, turn: 0.0, scale: 1.0, dry, rank: 0.0 };
        assert_eq!(spot_tag(&spot(0.0)) & 255, 0);
        assert_eq!(spot_tag(&spot(1.0)) & 255, 255);
        assert_eq!(spot_tag(&spot(7.0)) & 255, 255);
        assert!(spot_tag(&spot(0.5)) >> 16 == 0);
    }
}
