//! The film pass (Linear DCM-59): one fullscreen pass after the tone map that softens the top and
//! bottom of the screen (tilt-shift) and adds fine grain (`assets/shaders/film.wesl`). Each is a
//! [`crate::graphics::Graphics`] switch; with both off the camera has no [`Film`] and the pass
//! doesn't run at all.
//!
//! The grain follows visual time, so it stands still in an offscreen render once time stops (two
//! readbacks must match there).

use bevy::core_pipeline::fullscreen_material::{FullscreenMaterial, FullscreenMaterialPlugin};
use bevy::core_pipeline::schedule::Core3dSystems;
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::BoxedSystem;
use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::extract_component::ExtractComponent;
use bevy::render::render_resource::ShaderType;
use bevy::shader::ShaderRef;

use crate::camera::IsoCamera;
use crate::graphics::Graphics;

/// Grain strength by day; at night it is this much of that (dark frames show grain most).
const GRAIN: f32 = 0.085;
const GRAIN_NIGHT: f32 = 0.55;
/// The blur's radius at the very top and bottom of the screen, in pixels of a 1080-high picture.
const TILT_RADIUS: f32 = 2.6;

#[derive(Component, ExtractComponent, Clone, Copy, ShaderType, Default, Debug, PartialEq)]
#[extract_app(RenderApp)]
pub struct Film {
    pub grain: f32,
    pub tilt: f32,
    pub time: f32,
    pub pad: f32,
}

impl FullscreenMaterial for Film {
    fn fragment_shader() -> ShaderRef {
        "shaders/film.wesl".into()
    }

    fn schedule_configs(system: ScheduleConfigs<BoxedSystem>) -> ScheduleConfigs<BoxedSystem> {
        system.in_set(Core3dSystems::PostProcess).after(tonemapping)
    }
}

/// What the camera's [`Film`] should be for these settings, `None` when nothing is on.
pub fn film_for(graphics: &Graphics, daylight: f32, time: f32) -> Option<Film> {
    if !graphics.grain && !graphics.tilt_shift {
        return None;
    }
    let grain = if graphics.grain { GRAIN * (GRAIN_NIGHT + (1.0 - GRAIN_NIGHT) * daylight) } else { 0.0 };
    Some(Film { grain, tilt: if graphics.tilt_shift { TILT_RADIUS } else { 0.0 }, time, pad: 0.0 })
}

pub struct FilmPlugin;

impl Plugin for FilmPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(FullscreenMaterialPlugin::<Film>::default()).add_systems(Update, update_film);
    }
}

fn update_film(
    mut commands: Commands,
    graphics: Res<Graphics>,
    daylight: Res<crate::daynight::Daylight>,
    time: Res<Time>,
    mut cams: Query<(Entity, Option<&mut Film>), With<IsoCamera>>,
) {
    let Ok((cam, film)) = cams.single_mut() else { return };
    match (film_for(&graphics, daylight.0, time.elapsed_secs_wrapped()), film) {
        (Some(want), Some(mut have)) => {
            if *have != want {
                *have = want;
            }
        }
        (Some(want), None) => {
            commands.entity(cam).insert(want);
        }
        (None, Some(_)) => {
            commands.entity(cam).remove::<Film>();
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pass_only_runs_when_something_is_on() {
        assert_eq!(film_for(&Graphics::low(), 1.0, 3.0), None);
        let both = film_for(&Graphics::high(), 1.0, 3.0).unwrap();
        assert!(both.grain > 0.0 && both.tilt > 0.0 && both.time == 3.0);
        let grain_only = film_for(&Graphics { tilt_shift: false, ..Graphics::high() }, 1.0, 0.0).unwrap();
        assert!(grain_only.grain > 0.0 && grain_only.tilt == 0.0);
        // Less grain at night, never none.
        let night = film_for(&Graphics::high(), 0.0, 0.0).unwrap();
        assert!(night.grain > 0.0 && night.grain < both.grain);
    }
}
