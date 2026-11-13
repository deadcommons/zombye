//! Light shafts (light.ron `shafts`, the graphics settings' "Light shafts", Ultra only): a thin
//! box of air round the player that the sun's light scatters in, so where the sunlight is broken up
//! (through the trees, between houses, in at a window) it shows as beams. The engine's volumetric
//! fog: the camera marches through the box per pixel, reading the sun's shadow map.
//!
//! Costly (a pass over the screen, `steps` samples a pixel), so only in Ultra and off by night,
//! when the moon's light is too weak to show. Taken on and off live: it is a pass of its own, so
//! it builds no material's shaders.

use bevy::light::{FogVolume, VolumetricFog, VolumetricLight};
use bevy::prelude::*;

use crate::camera::IsoCamera;
use crate::daynight::{look, Daylight, Sun};

pub struct ShaftsPlugin;

impl Plugin for ShaftsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (apply, follow).chain().after(crate::daynight::update_lights));
    }
}

#[derive(Component)]
struct Air;

/// On or off with the setting (and by day only): the camera's fog, the sun's part in it and the
/// box of air.
fn apply(
    graphics: Res<crate::graphics::Graphics>,
    daylight: Res<Daylight>,
    mut commands: Commands,
    cams: Query<(Entity, Has<VolumetricFog>), With<IsoCamera>>,
    sun: Query<(Entity, Has<VolumetricLight>), With<Sun>>,
    air: Query<Entity, With<Air>>,
) {
    let s = look().shafts;
    // A little either side of the line so dusk doesn't flick it on and off.
    let was = cams.iter().any(|(_, has)| has);
    let want = graphics.light_shafts && s.density > 0.0 && daylight.0 > if was { 0.15 } else { 0.2 };
    for (cam, has) in &cams {
        match (want, has) {
            (true, false) => {
                commands.entity(cam).insert(VolumetricFog { ambient_intensity: 0.0, step_count: s.steps, jitter: 0.0, ..default() });
            }
            (false, true) => {
                commands.entity(cam).remove::<VolumetricFog>();
            }
            _ => {}
        }
    }
    for (e, has) in &sun {
        match (want, has) {
            (true, false) => {
                commands.entity(e).insert(VolumetricLight);
            }
            (false, true) => {
                commands.entity(e).remove::<VolumetricLight>();
            }
            _ => {}
        }
    }
    match (want, air.single()) {
        (true, Err(_)) => {
            let c = s.colour;
            commands.spawn((
                Air,
                FogVolume { fog_color: Color::srgb(c.0, c.1, c.2), density_factor: s.density, absorption: s.absorption, scattering: s.scattering, scattering_asymmetry: s.asymmetry, ..default() },
                Transform::from_scale(Vec3::new(s.size.0, s.size.1, s.size.0)),
            ));
        }
        (false, Ok(e)) => {
            commands.entity(e).despawn();
        }
        _ => {}
    }
}

/// The box stays round what the camera looks at, standing on the ground, moved only once that
/// has gone a few metres (a moved box redraws nothing but the pass that reads it anyway).
fn follow(cams: Query<&IsoCamera>, mut air: Query<&mut Transform, With<Air>>) {
    let (Ok(cam), Ok(mut tf)) = (cams.single(), air.single_mut()) else { return };
    let half = tf.scale.y * 0.5;
    let at = cam.focus + Vec3::Y * (half - 2.0);
    if tf.translation.distance(at) > 4.0 {
        tf.translation = at;
    }
}
