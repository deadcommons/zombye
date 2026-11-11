//! Campfires: a street prop of the kind `campfire` (`street_props.ron`) burns at night: a warm,
//! wavering point light over it, and what stands in its light is seen from afar like anything a
//! light falls on (`lights::NightLights`). Its numbers are `light.ron`'s `fire`. Client only: the
//! sim knows a campfire as a prop like any other.

use bevy::prelude::*;

use crate::daynight::{Daylight, look};
use crate::lights::{FAR_LIGHT, NightLights, SeenLight, darkness, flicker};
use crate::server_link::ClientMap;

/// The street prop kind that burns.
pub const KIND: &str = "campfire";

#[derive(Component)]
pub(crate) struct Fire(f32);

/// Where the map's campfires stand (the middle of each one's tiles).
pub fn places(map: &zombye_sim::map::TileMap) -> Vec<Vec2> {
    map.street_props
        .iter()
        .filter(|p| map.prop_kinds.get(p.kind as usize).is_some_and(|k| k.id == KIND))
        .map(|p| {
            let (min, size) = map.street_prop_rect(p);
            min.as_vec2() + size.as_vec2() * 0.5
        })
        .collect()
}

/// The fires' lights: made once for the map (a town has a handful), lit as it gets dark, and the
/// ones near us put on the list for the fog mask. Runs after the window and held lights.
pub(crate) fn fires(
    mut commands: Commands,
    map: Res<ClientMap>,
    daylight: Res<Daylight>,
    time: Res<Time>,
    me: Query<&Transform, (With<crate::player::Player>, Without<Fire>)>,
    mut lights: Query<(&Fire, &mut PointLight, &Transform, &mut Visibility)>,
    mut seen: ResMut<NightLights>,
    mut made: Local<bool>,
) {
    let fire = look().fire;
    if !*made {
        *made = true;
        let at = places(&map.0);
        if !at.is_empty() {
            info!("{} campfires, the first at {:.0}, {:.0}", at.len(), at[0].x, at[0].y);
        }
        for (i, at) in at.into_iter().enumerate() {
            let light = PointLight {
                intensity: 0.0,
                range: fire.range,
                radius: 0.25,
                color: Color::srgb(fire.colour.0, fire.colour.1, fire.colour.2),
                shadow_maps_enabled: false,
                ..default()
            };
            commands.spawn((Fire(i as f32 + 0.5), light, Transform::from_xyz(at.x, fire.height, at.y), Visibility::Hidden));
        }
        return;
    }
    let dark = darkness(daylight.0);
    let here = me.single().map(|t| t.translation.xz()).ok();
    for (f, mut light, tf, mut vis) in &mut lights {
        let at = tf.translation.xz();
        let near = here.is_some_and(|h| h.distance(at) < FAR_LIGHT + fire.range);
        let want = if dark > 0.0 && near { Visibility::Visible } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        if want == Visibility::Hidden {
            continue;
        }
        let lumens = fire.lumens * dark * flicker(time.elapsed_secs(), f.0);
        if light.intensity != lumens {
            light.intensity = lumens;
        }
        seen.0.push(SeenLight::round(at, fire.seen));
    }
}

/// A campfire someone built (`Snapshot::fires`): a ring of stones and two crossed logs, a flame
/// while it burns, and a light of its own (at any hour, warmer than the street props' at night).
#[derive(Component)]
pub(crate) struct Built {
    id: u32,
    flame: Entity,
    light: Entity,
}

/// The shared meshes and materials of built fires, made the first time there is one.
pub(crate) struct BuiltLook {
    cube: Handle<Mesh>,
    stone: Handle<StandardMaterial>,
    log: Handle<StandardMaterial>,
    ash: Handle<StandardMaterial>,
    flame: Handle<StandardMaterial>,
    /// The release build's campfire street prop (`street/campfire.glb`), drawn in place of the
    /// grey-box stones and logs where there is one, and the material for its vertex colours.
    model: Option<(Handle<Mesh>, Handle<StandardMaterial>)>,
}

/// Stones round a built fire, and the ring's radius.
const STONES: usize = 9;
const RING: f32 = 0.42;

/// Built fires: spawned, lit and put out as the snapshot says, despawned when gone. Runs after
/// [`fires`] (it adds to the same list of seen lights).
pub(crate) fn built_fires(
    mut commands: Commands,
    link: Res<crate::server_link::ServerLink>,
    fog: Res<crate::fog::FogState>,
    daylight: Res<Daylight>,
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut built: Query<(Entity, &Built, &Transform, &mut Visibility)>,
    mut parts: Query<(&mut Visibility, &mut Transform), Without<Built>>,
    mut lights: Query<&mut PointLight>,
    mut seen: ResMut<NightLights>,
    mut look: Local<Option<BuiltLook>>,
) {
    let fires = &link.curr.fires;
    if fires.is_empty() && built.is_empty() {
        return;
    }
    let fire = crate::daynight::look().fire;
    let look = look.get_or_insert_with(|| {
        let mut mat = |c: (f32, f32, f32), r: f32| materials.add(StandardMaterial { base_color: Color::srgb(c.0, c.1, c.2), perceptual_roughness: r, ..default() });
        let stone = mat((0.42, 0.40, 0.37), 0.9);
        let log = mat((0.32, 0.21, 0.12), 0.85);
        let ash = mat((0.12, 0.11, 0.10), 1.0);
        let flame = materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.55, 0.15),
            emissive: LinearRgba::rgb(9.0, 3.2, 0.6),
            unlit: true,
            ..default()
        });
        let model = crate::release_models::street().mesh(KIND).map(|m| (meshes.add(m), materials.add(StandardMaterial { base_color: Color::WHITE, perceptual_roughness: 0.85, ..default() })));
        BuiltLook { cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)), stone, log, ash, flame, model }
    });
    // Gone: despawn.
    for (e, b, _, _) in &built {
        if !fires.iter().any(|f| f.id == b.id) {
            commands.entity(e).despawn();
        }
    }
    let dark = darkness(daylight.0);
    let t = time.elapsed_secs();
    for f in fires {
        let at = Vec2::from(f.pos);
        // A burning fire is seen as a light is; an unlit one as anything on the ground.
        let shown = f.lit || fog.visibility(at) > 0.35;
        let want = if shown { Visibility::Inherited } else { Visibility::Hidden };
        let found = built.iter_mut().find(|(_, b, _, _)| b.id == f.id);
        let Some((_, b, _, mut vis)) = found else {
            spawn_built(&mut commands, look, f, &fire, want);
            continue;
        };
        if *vis != want {
            *vis = want;
        }
        let wobble = flicker(t, f.id as f32);
        if let Ok((mut v, mut tf)) = parts.get_mut(b.flame) {
            let fv = if f.lit { Visibility::Inherited } else { Visibility::Hidden };
            if *v != fv {
                *v = fv;
            }
            if f.lit {
                // Taller with more fuel on, and never still.
                let h = 0.32 + 0.18 * (f.fuel_min as f32 / 240.0).min(1.0);
                let scale = Vec3::new(0.26, h * wobble, 0.26);
                let y = 0.1 + scale.y * 0.5;
                if tf.scale != scale {
                    tf.scale = scale;
                    tf.translation.y = y;
                }
                tf.rotation = Quat::from_rotation_y(t * 0.9 + f.id as f32);
            }
        }
        if let Ok(mut light) = lights.get_mut(b.light) {
            // By day a little light still shows on the ground beside it; at night it's the fire's own.
            let lumens = if f.lit { fire.lumens * dark.max(0.15) * wobble } else { 0.0 };
            if light.intensity != lumens {
                light.intensity = lumens;
            }
        }
        if f.lit && dark > 0.0 {
            seen.0.push(SeenLight::round(at, fire.seen));
        }
    }
}

fn spawn_built(commands: &mut Commands, look: &BuiltLook, f: &zombye_sim::FireView, fire: &crate::daynight::FireLook, vis: Visibility) {
    let at = Vec2::from(f.pos);
    let light = commands
        .spawn((
            PointLight { intensity: 0.0, range: fire.range, radius: 0.25, color: Color::srgb(fire.colour.0, fire.colour.1, fire.colour.2), shadow_maps_enabled: false, ..default() },
            Transform::from_xyz(0.0, fire.height, 0.0),
        ))
        .id();
    let flame = commands
        .spawn((Mesh3d(look.cube.clone()), MeshMaterial3d(look.flame.clone()), Transform::from_xyz(0.0, 0.3, 0.0).with_scale(Vec3::new(0.26, 0.4, 0.26)), Visibility::Hidden))
        .id();
    let mut root = commands.spawn((Built { id: f.id, flame, light }, Transform::from_xyz(at.x, 0.0, at.y), vis));
    root.add_child(light).add_child(flame);
    if let Some((mesh, material)) = &look.model {
        root.with_children(|c| {
            c.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), Transform::from_rotation(Quat::from_rotation_y(f.id as f32 * 1.3))));
        });
        return;
    }
    root.with_children(|c| {
        // The ring of stones, each a little different.
        for i in 0..STONES {
            let a = i as f32 / STONES as f32 * std::f32::consts::TAU + f.id as f32 * 0.37;
            let s = 0.13 + 0.04 * ((i * 7 + f.id as usize) % 3) as f32;
            c.spawn((
                Mesh3d(look.cube.clone()),
                MeshMaterial3d(look.stone.clone()),
                Transform::from_xyz(a.cos() * RING, s * 0.35, a.sin() * RING).with_rotation(Quat::from_rotation_y(a * 1.7)).with_scale(Vec3::new(s, s * 0.7, s * 0.85)),
            ));
        }
        // Ash in the middle and two logs across it.
        c.spawn((Mesh3d(look.cube.clone()), MeshMaterial3d(look.ash.clone()), Transform::from_xyz(0.0, 0.01, 0.0).with_scale(Vec3::new(0.62, 0.02, 0.62))));
        for (k, turn) in [0.6f32, -0.7].into_iter().enumerate() {
            c.spawn((
                Mesh3d(look.cube.clone()),
                MeshMaterial3d(look.log.clone()),
                Transform::from_xyz(0.0, 0.06 + k as f32 * 0.07, 0.0).with_rotation(Quat::from_rotation_y(turn)).with_scale(Vec3::new(0.62, 0.09, 0.1)),
            ));
        }
    });
}
