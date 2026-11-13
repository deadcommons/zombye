//! Silhouettes: the player is never lost behind what the cutaways leave standing.
//!
//! From outside, buildings keep their roofs and upper storeys, so a player behind a house (or
//! any player, with the walls up) can still be covered. While `Cutaway::occluded` says so, the
//! local survivor's own skinned meshes are drawn once more in one flat tint with the depth test
//! turned round (`silhouette.wesl`): only the pixels where something stands in front of the body
//! show, as a flat shape through the roof or wall. It is a second draw of two small meshes in the
//! transparent phase of the main pass: no new pass, nothing full-screen, and nothing at all while
//! the player is in plain view (the twins are hidden).
//!
//! **Zombies stay hidden behind walls by default.** With the "zombie silhouettes" setting
//! (Shift+F10, `--zombie-silhouettes`, kept in the settings file) the zombies the fog of war
//! already shows (the ones the survivor sees) get the same treatment where a wall, roof or upper
//! storey covers them from the camera (`zombie_render.rs`): never one the survivor doesn't see.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::NotShadowCaster;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, CompareFunction, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;

use crate::cutaway::{Cutaway, CutawaySystems};
use crate::daynight::Daylight;

/// How far toward the camera a body's depth is moved before it is compared, in metres: nothing
/// nearer than this counts as covering it (its own limbs, the weapon in its hand, a wall it leans on).
pub const SELF_CLEAR_M: f32 = 0.7;
/// That, in depth-buffer units (the camera is orthographic: depth is linear).
pub const SELF_CLEAR: f32 = SELF_CLEAR_M / (crate::camera::FAR - crate::camera::NEAR);
/// Seconds for the silhouette to come and go.
const FADE_SECS: f32 = 0.15;
/// The player's silhouette: the mustard of its marker ring, this opaque, dimmed to this share at night.
const TINT: Color = crate::survivor::JACKET_ME;
const ALPHA: f32 = 0.62;
const NIGHT: f32 = 0.22;
/// A zombie's: a dull red (linear), and its alpha.
pub const ZOMBIE_TINT: Vec4 = Vec4::new(0.42, 0.05, 0.04, 0.5);

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct SilhouetteMaterial {
    #[uniform(0)]
    pub look: SilhouetteLook,
}

/// Must match `SilhouetteLook` in silhouette.wesl.
#[derive(ShaderType, Debug, Clone)]
pub struct SilhouetteLook {
    /// The tint (linear, straight alpha).
    pub color: Vec4,
    /// x = how far toward the camera the body's depth is moved, in depth-buffer units.
    pub depth: Vec4,
}

impl Material for SilhouetteMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/silhouette.wesl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    // Only the main pass: a silhouette is in no depth prepass and casts no shadow.
    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        behind_only(descriptor);
        Ok(())
    }
}

/// Draw only where something nearer the camera is already drawn: the depth test turned round
/// (the depth buffer is reversed: nearer is greater), and nothing written to it.
pub fn behind_only(descriptor: &mut RenderPipelineDescriptor) {
    if let Some(depth) = descriptor.depth_stencil.as_mut() {
        depth.depth_compare = Some(CompareFunction::Less);
        depth.depth_write_enabled = Some(false);
    }
}

/// On a mesh of the local survivor (its body, its outfit): it gets a silhouette twin.
#[derive(Component)]
pub struct Silhouetted;

/// A silhouetted mesh's twin.
#[derive(Component)]
struct Twin(Entity);

/// The player's silhouette material, and how much of it shows now (0 none, 1 all).
#[derive(Resource)]
struct PlayerSilhouette {
    material: Handle<SilhouetteMaterial>,
    shown: f32,
    drawn: Vec4,
}

/// The setting: zombies the survivor sees show through what covers them from the camera. Off by
/// default, so danger behind a wall stays hidden.
#[derive(Resource, Default)]
pub struct ZombieSilhouettes(pub bool);

pub struct SilhouettePlugin;

impl Plugin for SilhouettePlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().get_resource::<crate::Options>().cloned();
        // The flag, else (never offscreen) the settings file.
        let zombies = match options.as_ref().and_then(|o| o.zombie_silhouettes) {
            Some(on) => on,
            None if options.as_ref().is_some_and(|o| !o.offscreen()) => crate::ui_scale::SettingsStore::ensure(app).zombie_silhouettes,
            None => false,
        };
        app.add_plugins(MaterialPlugin::<SilhouetteMaterial>::default())
            .insert_resource(ZombieSilhouettes(zombies))
            .add_systems(Startup, setup)
            .add_systems(Update, (toggle_zombies, player_silhouette.after(CutawaySystems)));
    }
}

fn setup(mut commands: Commands, mut materials: ResMut<Assets<SilhouetteMaterial>>) {
    let look = SilhouetteLook { color: Vec4::ZERO, depth: Vec4::new(SELF_CLEAR, 0.0, 0.0, 0.0) };
    commands.insert_resource(PlayerSilhouette { material: materials.add(SilhouetteMaterial { look }), shown: 0.0, drawn: Vec4::ZERO });
}

/// The player's tint by daylight (dim at night: a shape, not a lamp) and by how much of it shows.
fn tint(daylight: f32, shown: f32) -> Vec4 {
    let c = TINT.to_linear();
    let k = NIGHT + (1.0 - NIGHT) * daylight.clamp(0.0, 1.0);
    Vec4::new(c.red * k, c.green * k, c.blue * k, ALPHA * shown * shown * (3.0 - 2.0 * shown))
}

/// Shift+F10: zombie silhouettes on or off, kept in the settings file.
fn toggle_zombies(keys: Res<ButtonInput<KeyCode>>, time: Res<Time>, mut on: ResMut<ZombieSilhouettes>, store: Option<ResMut<crate::ui_scale::SettingsStore>>, notice: Option<ResMut<crate::saving::Notice>>) {
    if !keys.just_pressed(KeyCode::F10) || !keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) || keys.any_pressed(crate::cutaway::CTRL_KEYS) {
        return;
    }
    on.0 = !on.0;
    let value = on.0;
    if let Some(mut store) = store
        && let Err(e) = store.change(|s| s.zombie_silhouettes = value)
    {
        eprintln!("{e}");
    }
    if let Some(mut notice) = notice {
        notice.say(time.elapsed_secs(), if value { "Zombie silhouettes on: the ones you see show through what covers them" } else { "Zombie silhouettes off" }.to_string());
    }
}

/// Keep each silhouetted mesh's twin (same mesh, same skeleton, the silhouette material) and show
/// the twins only while something still covers the player.
#[allow(clippy::type_complexity)]
fn player_silhouette(
    mut commands: Commands,
    time: Res<Time>,
    cutaway: Res<Cutaway>,
    daylight: Option<Res<Daylight>>,
    state: Option<ResMut<PlayerSilhouette>>,
    mut materials: ResMut<Assets<SilhouetteMaterial>>,
    sources: Query<(Entity, Option<&Mesh3d>, &SkinnedMesh, &ChildOf, &Visibility, Option<&Twin>), With<Silhouetted>>,
    mut twins: Query<(&mut Mesh3d, &mut Visibility), Without<Silhouetted>>,
) {
    let Some(mut state) = state else { return };
    let step = time.delta_secs() / FADE_SECS;
    let shown = if cutaway.occluded { (state.shown + step).min(1.0) } else { (state.shown - step).max(0.0) };
    if shown != state.shown {
        state.shown = shown;
    }
    let color = tint(daylight.map_or(1.0, |d| d.0), shown);
    if color != state.drawn {
        state.drawn = color;
        if let Some(mut m) = materials.get_mut(&state.material) {
            m.look.color = color;
        }
    }
    for (entity, mesh, skin, parent, visibility, twin) in &sources {
        let Some(mesh) = mesh else { continue };
        let want = if shown > 0.0 && *visibility != Visibility::Hidden { Visibility::Inherited } else { Visibility::Hidden };
        let Some(Twin(twin)) = twin else {
            let twin = commands
                .spawn((mesh.clone(), MeshMaterial3d(state.material.clone()), skin.clone(), NoFrustumCulling, NotShadowCaster, Transform::default(), want, ChildOf(parent.parent())))
                .id();
            commands.entity(entity).insert(Twin(twin));
            continue;
        };
        let Ok((mut twin_mesh, mut twin_visibility)) = twins.get_mut(*twin) else { continue };
        if twin_mesh.0 != mesh.0 {
            twin_mesh.0 = mesh.0.clone();
        }
        if *twin_visibility != want {
            *twin_visibility = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The body's depth is moved 0.7 m toward the camera before the test, so its own limbs (well
    /// within that) never count as covering it, and the tint is nothing while nothing covers the
    /// player, a dim shape at night.
    #[test]
    fn the_silhouette_is_a_dim_flat_tint_that_fades() {
        assert!((SELF_CLEAR * (crate::camera::FAR - crate::camera::NEAR) - SELF_CLEAR_M).abs() < 1e-6);
        assert!(SELF_CLEAR > 0.0 && SELF_CLEAR < 0.01);
        assert_eq!(tint(1.0, 0.0).w, 0.0);
        assert!((tint(1.0, 1.0).w - ALPHA).abs() < 1e-6);
        let (day, night) = (tint(1.0, 1.0), tint(0.0, 1.0));
        assert!(night.x < day.x * 0.3 && night.w == day.w);
        let half = tint(1.0, 0.5).w;
        assert!(half > 0.2 && half < 0.4);
    }
}
