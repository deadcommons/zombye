//! The world material: `StandardMaterial` plus two tricks done in the fragment shader:
//! - **fog of war**: anything outside the player's line of sight is darkened and desaturated;
//! - **cutaway**: walls on the camera side of the player drop to knee-high stubs as whole pieces,
//!   roofs that hide the player disappear, and the foliage of trees in front of the player fades
//!   out over a quarter of a second, leaving their trunk and branches dimmed and thinned, so your
//!   character is never hidden. Inside a building, all of its camera-side walls drop.
//!
//! Every world mesh uses one of a handful of [`WorldMaterial`]s, and their shared parameters are
//! refreshed once per frame. Only materials that need it are built with the cutaway (`CUTAWAY` in
//! the shader): a shader that can discard a fragment loses the GPU's hidden-surface removal for
//! everything drawn with it, so the ground and most chunks' props don't carry it. A chunk's props
//! switch to the cutaway version while one of its trees is fading.

use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;

use crate::camera::IsoCamera;
use crate::fog::FogState;
use crate::player::Player;
use crate::server_link::ClientMap;
use zombye_sim::map::TileMap;

pub type WorldMaterial = ExtendedMaterial<StandardMaterial, WorldExt>;

const SHADER: &str = "shaders/world.wesl";

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
#[bind_group_data(WorldKey)]
pub struct WorldExt {
    #[uniform(100)]
    pub params: WorldParams,
    #[texture(101)]
    #[sampler(102)]
    pub fog: Handle<Image>,
    /// Built with the cutaway (walls, roofs, fading foliage)? Fixed when the material is made.
    pub cutaway: bool,
}

/// The pipeline key of a [`WorldExt`]: whether its shader has the cutaway.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq)]
pub struct WorldKey {
    pub cutaway: bool,
}

impl From<&WorldExt> for WorldKey {
    fn from(ext: &WorldExt) -> Self {
        WorldKey { cutaway: ext.cutaway }
    }
}

#[derive(ShaderType, Reflect, Debug, Clone)]
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
    /// x = how many entries of `trees` are in use.
    pub tree_count: Vec4,
    /// Trees whose foliage is fading: (trunk foot x, z, fade 0..1 (1 = foliage gone), 0).
    pub trees: [Vec4; MAX_FADING_TREES],
}

impl Default for WorldParams {
    fn default() -> Self {
        Self {
            player: Vec4::ZERO,
            camera_dir: Vec4::ZERO,
            fog_rect: Vec4::ZERO,
            fog_tint: Vec4::ZERO,
            inside_rect: Vec4::ZERO,
            tree_count: Vec4::ZERO,
            trees: [Vec4::ZERO; MAX_FADING_TREES],
        }
    }
}

/// Must match the array size in world.wesl.
pub const MAX_FADING_TREES: usize = 32;
/// Seconds for a tree's foliage to fade out or come back.
const FADE_SECS: f32 = 0.25;

/// Does a tree's crown hide (part of) the player, seen from the camera? The crown and the player
/// are compared where they land in the view plane: a crown nearer the camera than the player whose
/// outline overlaps the player's.
pub fn crown_hides(t: &crate::meshgen::trees::TreeSpec, player: Vec3, camera_dir: Vec3) -> bool {
    let d = t.crown - player;
    let along = d.dot(camera_dir);
    if along <= 0.0 {
        return false;
    }
    let perp = (d - along * camera_dir).length();
    perp < t.crown_r.max(t.crown_h * 0.9) * 0.9 + 0.6
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

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if key.bind_group_data.cutaway
            && let Some(fragment) = descriptor.fragment.as_mut()
        {
            fragment.shader_defs.push("CUTAWAY".into());
        }
        Ok(())
    }
}

/// Every world material, so their shared parameters can be refreshed together.
#[derive(Resource, Default)]
pub struct WorldMaterials {
    pub all: Vec<Handle<WorldMaterial>>,
    /// Terrain and floors (vertex coloured).
    pub ground: Handle<WorldMaterial>,
    /// Buildings and furniture (vertex coloured, double-sided).
    pub building: Handle<WorldMaterial>,
    /// Trees, bushes, hedges and fences: closed shapes wound outward, so single-sided (half the
    /// fragments of drawing their insides too), and without the cutaway...
    pub props: Handle<WorldMaterial>,
    /// ...but with it for the chunks where a tree is fading (`world_render::fade_chunks`).
    pub props_cut: Handle<WorldMaterial>,
    /// Window glass: vertex coloured with alpha, alpha-to-coverage (needs MSAA, on by default),
    /// double-sided.
    pub glass: Handle<WorldMaterial>,
}

pub struct WorldMaterialPlugin;

impl Plugin for WorldMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<WorldMaterial>::default())
            .init_resource::<TreeFades>()
            .add_systems(Startup, create_materials)
            .add_systems(PostUpdate, update_params);
    }
}

fn create_materials(mut commands: Commands, fog: Res<FogState>, mut materials: ResMut<Assets<WorldMaterial>>) {
    let mut all = Vec::new();
    let mut make = |base: StandardMaterial, cutaway: bool| {
        let h = materials.add(ExtendedMaterial { base, extension: WorldExt { params: WorldParams::default(), fog: fog.image.clone(), cutaway } });
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
    let ground = make(vertex_colored(0.95, false), false);
    let building = make(vertex_colored(0.85, true), true);
    let props = make(vertex_colored(0.85, false), false);
    let props_cut = make(vertex_colored(0.85, false), true);
    // Alpha to coverage instead of blending: drawn with the opaque pass (depth tested and written,
    // nothing to sort), each pixel covering as many MSAA samples as its alpha asks, so panes seen
    // through other panes, zombies behind glass and the cutaway all just work.
    let glass = make(StandardMaterial { alpha_mode: AlphaMode::AlphaToCoverage, reflectance: 0.6, ..vertex_colored(0.15, true) }, true);
    commands.insert_resource(WorldMaterials { all, ground, building, props, props_cut, glass });
}

/// Each fading tree's foliage, by tile: (tile, trunk foot, 0 shown .. 1 gone).
#[derive(Resource, Default)]
pub struct TreeFades(pub Vec<(IVec2, Vec2, f32)>);

#[allow(clippy::too_many_arguments)]
pub fn update_params(
    fog: Res<FogState>,
    map: Res<ClientMap>,
    time: Res<Time>,
    world_materials: Option<Res<WorldMaterials>>,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut fades: ResMut<TreeFades>,
    player: Query<&Transform, With<Player>>,
    cams: Query<&IsoCamera>,
) {
    let (Some(wm), Ok(p), Ok(cam)) = (world_materials, player.single(), cams.single()) else { return };
    let chest = p.translation.with_y(1.1);
    let camera_dir = cam.offset_dir();
    // Trees in front of the player fade their foliage out; the rest fade back in.
    let step = time.delta_secs() / FADE_SECS;
    let here = IVec2::new(chest.x.floor() as i32, chest.z.floor() as i32);
    let mut hiding: Vec<(IVec2, Vec2)> = Vec::new();
    const REACH: i32 = 14;
    for y in here.y - REACH..=here.y + REACH {
        for x in here.x - REACH..=here.x + REACH {
            let t = IVec2::new(x, y);
            if !map.0.in_bounds(t) {
                continue;
            }
            if let Some(spec) = crate::meshgen::trees::tree_at(x, y, map.0.tile(t).feature)
                && crown_hides(&spec, chest, camera_dir)
            {
                hiding.push((t, spec.base.xz()));
            }
        }
    }
    for f in fades.0.iter_mut() {
        f.2 = (f.2 - step).max(0.0);
    }
    for (t, anchor) in hiding {
        match fades.0.iter_mut().find(|f| f.0 == t) {
            // Undo this frame's fade-in and fade out instead.
            Some(f) => f.2 = (f.2 + 2.0 * step).min(1.0),
            None => fades.0.push((t, anchor, step.min(1.0))),
        }
    }
    fades.0.retain(|f| f.2 > 0.0);
    // The most faded first, should there ever be more than fit.
    fades.0.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut trees = [Vec4::ZERO; MAX_FADING_TREES];
    let n = fades.0.len().min(MAX_FADING_TREES);
    for (slot, f) in trees.iter_mut().zip(&fades.0) {
        *slot = Vec4::new(f.1.x, f.1.y, f.2, 0.0);
    }
    let params = WorldParams {
        player: chest.extend(0.0),
        camera_dir: camera_dir.extend(0.0),
        fog_rect: Vec4::new(fog.origin.x, fog.origin.y, fog.size_m, 0.0),
        fog_tint: Vec4::new(0.55, 0.6, 0.75, 0.72),
        inside_rect: inside_rect(&map.0, p.translation.xz()),
        tree_count: Vec4::new(n as f32, 0.0, 0.0, 0.0),
        trees,
    };
    for h in &wm.all {
        if let Some(mut m) = materials.get_mut(h) {
            m.extension.params = params.clone();
        }
    }
}
