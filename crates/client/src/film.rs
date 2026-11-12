//! The film pass (Linear DCM-59): one fullscreen pass after the tone map that softens the top and
//! bottom of the screen (tilt-shift) and adds fine grain (`assets/shaders/film.wesl`). Each is a
//! [`crate::graphics::Graphics`] switch; with both off the camera has no [`Film`] and the pass
//! doesn't run at all.
//!
//! **The old look** (the settings' "Old look": Off, Subtle or Strong; `ClientSettings::old_look`,
//! `--old-look off|subtle|strong` for a session) rides on the same pass: the picture as a faded
//! print, with lifted warm blacks, whites washed to cream, a little colour gone, more grain and a
//! soft glow round the brightest parts. Each strength's numbers are `light.ron`'s `old_look`
//! (`daynight::OldLookStyle`). It is a choice of look, not of cost, so no graphics preset sets it,
//! and it draws with the grain and tilt-shift switches off too.
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
use crate::daynight::OldLookLevel;
use crate::graphics::Graphics;

/// How much of the old look is drawn (see the module docs).
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OldLook {
    #[default]
    Off,
    Subtle,
    Strong,
}

impl OldLook {
    pub const ALL: [OldLook; 3] = [OldLook::Off, OldLook::Subtle, OldLook::Strong];

    pub fn name(self) -> &'static str {
        match self {
            OldLook::Off => "Off",
            OldLook::Subtle => "Subtle",
            OldLook::Strong => "Strong",
        }
    }

    pub fn parse(text: &str) -> Option<OldLook> {
        OldLook::ALL.into_iter().find(|o| o.name().eq_ignore_ascii_case(text.trim()))
    }

    /// This strength's numbers in `light.ron`, `None` when off.
    pub fn level(self) -> Option<OldLookLevel> {
        let style = crate::daynight::look().old_look;
        match self {
            OldLook::Off => None,
            OldLook::Subtle => Some(style.subtle),
            OldLook::Strong => Some(style.strong),
        }
    }
}

/// The old look in force: the settings', or `--old-look`'s for the session.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct OldLookNow(pub OldLook);

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
    /// The old look's glow round bright parts (0 = none).
    pub glow: f32,
    /// The old look: what black becomes (rgb, as the screen shows it) and how much colour goes (w).
    pub black: Vec4,
    /// What white becomes (rgb); w is 1 with the old look on, 0 without.
    pub white: Vec4,
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
pub fn film_for(graphics: &Graphics, old: Option<OldLookLevel>, daylight: f32, time: f32) -> Option<Film> {
    if !graphics.grain && !graphics.tilt_shift && old.is_none() {
        return None;
    }
    let by_night = GRAIN_NIGHT + (1.0 - GRAIN_NIGHT) * daylight;
    let mut film = Film { grain: if graphics.grain { GRAIN * by_night } else { 0.0 }, tilt: if graphics.tilt_shift { TILT_RADIUS } else { 0.0 }, time, ..default() };
    if let Some(o) = old {
        let fade = o.fade * (o.night + (1.0 - o.night) * daylight);
        film.grain += o.grain * by_night;
        film.glow = o.glow;
        film.black = (Vec3::new(o.ink.0, o.ink.1, o.ink.2) * fade).extend(o.desaturate);
        film.white = Vec3::ONE.lerp(Vec3::new(o.paper.0, o.paper.1, o.paper.2), o.wash).extend(1.0);
    }
    Some(film)
}

pub struct FilmPlugin;

impl Plugin for FilmPlugin {
    fn build(&self, app: &mut App) {
        // A flag's look for the session stays until the setting itself is changed (`settings_ui`).
        // Offscreen the store holds the defaults, so a picture has the flag's look or none.
        let flag = app.world().get_resource::<crate::Options>().and_then(|o| o.old_look);
        let old = flag.unwrap_or_else(|| crate::ui_scale::SettingsStore::ensure(app).old_look);
        app.insert_resource(OldLookNow(old)).add_plugins(FullscreenMaterialPlugin::<Film>::default()).add_systems(Update, update_film);
    }
}

fn update_film(
    mut commands: Commands,
    graphics: Res<Graphics>,
    old: Res<OldLookNow>,
    daylight: Res<crate::daynight::Daylight>,
    time: Res<Time>,
    mut cams: Query<(Entity, Option<&mut Film>), With<IsoCamera>>,
) {
    let Ok((cam, film)) = cams.single_mut() else { return };
    match (film_for(&graphics, old.0.level(), daylight.0, time.elapsed_secs_wrapped()), film) {
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
        assert_eq!(film_for(&Graphics::low(), None, 1.0, 3.0), None);
        let both = film_for(&Graphics::high(), None, 1.0, 3.0).unwrap();
        assert!(both.grain > 0.0 && both.tilt > 0.0 && both.time == 3.0);
        assert!(both.white.w == 0.0 && both.glow == 0.0, "no old look unless asked for");
        let grain_only = film_for(&Graphics { tilt_shift: false, ..Graphics::high() }, None, 1.0, 0.0).unwrap();
        assert!(grain_only.grain > 0.0 && grain_only.tilt == 0.0);
        // Less grain at night, never none.
        let night = film_for(&Graphics::high(), None, 0.0, 0.0).unwrap();
        assert!(night.grain > 0.0 && night.grain < both.grain);
    }

    #[test]
    fn the_old_look_fades_more_when_strong_and_less_at_night() {
        assert_eq!(OldLook::default(), OldLook::Off);
        assert_eq!(OldLook::Off.level(), None);
        assert_eq!(OldLook::parse(" strong "), Some(OldLook::Strong));
        assert_eq!(OldLook::parse("sepia"), None);
        let (subtle, strong) = (OldLook::Subtle.level().unwrap(), OldLook::Strong.level().unwrap());
        // It draws with every film switch off, and adds to the grain when that is on.
        let s = film_for(&Graphics::low(), Some(subtle), 1.0, 0.0).unwrap();
        let t = film_for(&Graphics::low(), Some(strong), 1.0, 0.0).unwrap();
        assert!(s.white.w == 1.0 && s.tilt == 0.0 && s.grain > 0.0);
        assert!(film_for(&Graphics::high(), Some(subtle), 1.0, 0.0).unwrap().grain > s.grain);
        // Strong lifts the blacks further, washes the whites further and takes more colour.
        assert!(t.black.truncate().length() > s.black.truncate().length() && s.black.truncate().length() > 0.0);
        assert!(t.white.truncate().length() < s.white.truncate().length() && s.white.x < 1.0);
        assert!(t.black.w > s.black.w && t.grain > s.grain && t.glow > s.glow);
        // Warm: black goes to brown and white to cream.
        assert!(s.black.x > s.black.z && s.white.x > s.white.z);
        // The dark stays darker at night.
        let night = film_for(&Graphics::low(), Some(strong), 0.0, 0.0).unwrap();
        assert!(night.black.truncate().length() < t.black.truncate().length() * 0.5);
    }
}
