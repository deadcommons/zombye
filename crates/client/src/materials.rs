//! The world material: `StandardMaterial` plus two tricks done in the fragment shader:
//! - **fog of war**: anything outside the player's line of sight is darkened and desaturated;
//! - **cutaway**: walls on the camera side of the player drop to knee-high stubs as whole pieces,
//!   roofs that hide the player disappear, and tree canopies fade, so your character is never
//!   hidden. Inside a building, all of its camera-side walls drop.
//!
//! Every world mesh uses one of a handful of [`WorldMaterial`]s, and their shared parameters are
//! refreshed once per frame.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::camera::IsoCamera;
use crate::fog::FogState;
use crate::player::Player;
use crate::server_link::ClientMap;
use zombye_sim::map::TileMap;

pub type WorldMaterial = ExtendedMaterial<StandardMaterial, WorldExt>;

const SHADER: &str = "shaders/world.wesl";

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct WorldExt {
    #[uniform(100)]
    pub params: WorldParams,
    #[texture(101)]
    #[sampler(102)]
    pub fog: Handle<Image>,
}

#[derive(ShaderType, Reflect, Debug, Clone, Default)]
pub struct WorldParams {
    /// xyz = player chest position, w = cutaway radius in metres (0 = off).
    pub player: Vec4,
    /// xyz = unit vector from the scene toward the camera.
    pub camera_dir: Vec4,
    /// x, y = world x/z of the fog mask's min corner, z = mask size in metres.
    pub fog_rect: Vec4,
    /// rgb = tint for unseen areas, w = how dark unseen areas get (0..1).
    pub fog_tint: Vec4,
    /// Bounds of the building the player is in: (min x, min z, max x, max z). min > max = outdoors.
    pub inside_rect: Vec4,
}

/// The shader's wall rule (`wall_cut` in world.wesl), for things drawn as separate entities (door
/// leaves, window panes).
pub fn wall_is_cut(mid: Vec2, player: Vec2, camera_dir: Vec3, inside: Vec4) -> bool {
    let c = camera_dir.xz().normalize_or_zero();
    let d = mid - player;
    let along = d.dot(c);
    let perp = (d - along * c).length();
    if inside.x <= inside.z && mid.x >= inside.x - 0.1 && mid.y >= inside.y - 0.1 && mid.x <= inside.z + 0.1 && mid.y <= inside.w + 0.1 {
        return along > 0.0;
    }
    along > 0.0 && along < 5.5 && perp < 1.6
}

/// `inside_rect` for the building containing `p`.
pub fn inside_rect(map: &TileMap, p: Vec2) -> Vec4 {
    match map.building_at(p) {
        Some(i) => {
            let b = &map.buildings[i];
            Vec4::new(b.min.x as f32, b.min.y as f32, (b.min.x + b.size.x) as f32, (b.min.y + b.size.y) as f32)
        }
        None => Vec4::new(1.0, 1.0, 0.0, 0.0),
    }
}

impl MaterialExtension for WorldExt {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }
}

/// Every world material, so their shared parameters can be refreshed together.
#[derive(Resource, Default)]
pub struct WorldMaterials {
    pub all: Vec<Handle<WorldMaterial>>,
    /// Terrain and floors (vertex coloured).
    pub ground: Handle<WorldMaterial>,
    /// Buildings, furniture, trees and props (vertex coloured, double-sided).
    pub building: Handle<WorldMaterial>,
    /// Window glass: vertex coloured with alpha, alpha-to-coverage (needs MSAA, on by default),
    /// double-sided.
    pub glass: Handle<WorldMaterial>,
}

pub struct WorldMaterialPlugin;

impl Plugin for WorldMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<WorldMaterial>::default())
            .add_systems(Startup, create_materials)
            .add_systems(PostUpdate, update_params);
    }
}

fn create_materials(mut commands: Commands, fog: Res<FogState>, mut materials: ResMut<Assets<WorldMaterial>>) {
    let mut all = Vec::new();
    let mut make = |base: StandardMaterial| {
        let h = materials.add(ExtendedMaterial { base, extension: WorldExt { params: WorldParams::default(), fog: fog.image.clone() } });
        all.push(h.clone());
        h
    };
    let vertex_colored = |roughness: f32, double_sided: bool| StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: roughness,
        double_sided,
        cull_mode: if double_sided { None } else { Some(bevy::render::render_resource::Face::Back) },
        ..default()
    };
    let ground = make(vertex_colored(0.95, false));
    let building = make(vertex_colored(0.85, true));
    // Alpha to coverage instead of blending: drawn with the opaque pass (depth tested and written,
    // nothing to sort), each pixel covering as many MSAA samples as its alpha asks, so panes seen
    // through other panes, zombies behind glass and the cutaway all just work.
    let glass = make(StandardMaterial { alpha_mode: AlphaMode::AlphaToCoverage, reflectance: 0.6, ..vertex_colored(0.15, true) });
    commands.insert_resource(WorldMaterials { all, ground, building, glass });
}

fn update_params(
    fog: Res<FogState>,
    map: Res<ClientMap>,
    world_materials: Option<Res<WorldMaterials>>,
    mut materials: ResMut<Assets<WorldMaterial>>,
    player: Query<&Transform, With<Player>>,
    cams: Query<&IsoCamera>,
) {
    let (Some(wm), Ok(p), Ok(cam)) = (world_materials, player.single(), cams.single()) else { return };
    let params = WorldParams {
        player: (p.translation.with_y(1.1)).extend(0.0),
        camera_dir: cam.offset_dir().extend(0.0),
        fog_rect: Vec4::new(fog.origin.x, fog.origin.y, fog.size_m, 0.0),
        fog_tint: Vec4::new(0.55, 0.6, 0.75, 0.72),
        inside_rect: inside_rect(&map.0, p.translation.xz()),
    };
    for h in &wm.all {
        if let Some(mut m) = materials.get_mut(h) {
            m.extension.params = params.clone();
        }
    }
}
