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
