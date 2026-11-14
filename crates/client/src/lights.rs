//! Light sources in the dark, other than the one in our own hand (#83, second round; `daynight.rs`
//! has the sun, the moon and our own flashlight or candle):
//!
//! - **Room lights** (`zombye_sim::power`): the server says which rooms are lit (switch on, power
//!   on) in every snapshot ([`LitRooms`]). The nearest few lit ground-floor rooms get a light under
//!   their ceiling at night ([`MAX_ROOM_LIGHTS`]), so a room you switch on is lit inside.
//! - **Window pools:** a ground-floor window with a light on behind it ([`LitRooms`])
//!   is a small warm light at night, so it throws a pool on the ground outside and into its room,
//!   and whatever walks through it is lit. No shadows (dozens can be in view).
//! - **Other survivors' lights:** what another player holds reaches us in `Snapshot::outfits`, so
//!   the nearest few with a flashlight or a candle get the same cone or pool as ours, shadows
//!   included (a beam mustn't come through a wall).
//! - **What's lit is seen:** at night you only see a few metres (`fog.rs`), but a lit place in
//!   line of sight shows however far off it is. The world shader works that out per pixel from how
//!   brightly a surface is lit; [`NightLights`] lists the same lights for the fog mask, which
//!   decides which zombies, survivors and items are drawn.

use bevy::prelude::*;

use crate::daynight::{BEAM_AIM_M, BEAM_HEIGHT, BEAM_INNER, BEAM_OUTER, Daylight, HeldLight, held_light_in};
use crate::player::LocalPlayer;
use crate::server_link::{ClientMap, JoinInfo, ServerLink};
use std::collections::HashMap;

/// Lit rooms given a light of their own at once, and from how far off.
pub const MAX_ROOM_LIGHTS: usize = 6;
const ROOM_LIGHT_REACH: f32 = 30.0;
/// A room light: how bright, how high (under a 3 m ceiling), and its reach past the room's own size.
const ROOM_LUMENS: f32 = 160_000.0;
const ROOM_HEIGHT: f32 = 2.6;
const ROOM_SPILL: f32 = 2.5;

/// The rooms lit now and whether the town's grid is on, as the server last said
/// (`Snapshot::lit_rooms`, `grid_on`). Changes only when they do, so `is_changed` means a light
/// went on or off.
#[derive(Resource, Default, Debug, PartialEq)]
pub struct LitRooms {
    pub rooms: Vec<u16>,
    pub grid: bool,
}

impl LitRooms {
    pub fn is_lit(&self, room: u16) -> bool {
        room != 0 && self.rooms.binary_search(&room).is_ok()
    }

    /// Either of the rooms either side of a wall.
    pub fn either(&self, rooms: (u16, u16)) -> bool {
        self.is_lit(rooms.0) || self.is_lit(rooms.1)
    }
}

/// Take the lit rooms from the newest snapshot.
pub fn sync_lit_rooms(link: Res<ServerLink>, mut lit: ResMut<LitRooms>) {
    if lit.rooms != link.curr.lit_rooms || lit.grid != link.curr.grid_on {
        lit.rooms.clone_from(&link.curr.lit_rooms);
        lit.grid = link.curr.grid_on;
    }
}

/// Where each ground-floor room is: the middle of its tiles and the radius round them, worked out
/// the first time it is lit (`None`: no ground-floor tiles, an upstairs room).
#[derive(Resource, Default)]
struct RoomPlaces(HashMap<u16, Option<(Vec2, f32)>>);

impl RoomPlaces {
    fn of(&mut self, map: &zombye_sim::TileMap, room: u16) -> Option<(Vec2, f32)> {
        *self.0.entry(room).or_insert_with(|| room_place(map, room))
    }
}

fn room_place(map: &zombye_sim::TileMap, room: u16) -> Option<(Vec2, f32)> {
    let b = map.buildings.get(map.rooms.get(room as usize)?.building as usize)?;
    let (mut lo, mut hi, mut n) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN), 0);
    for y in b.min.y..b.min.y + b.size.y {
        for x in b.min.x..b.min.x + b.size.x {
            if map.tile(IVec2::new(x, y)).room == room {
                let t = Vec2::new(x as f32, y as f32);
                (lo, hi, n) = (lo.min(t), hi.max(t + Vec2::ONE), n + 1);
            }
        }
    }
    (n > 0).then(|| ((lo + hi) * 0.5, (hi - lo).length() * 0.5))
}

/// One of the pool of room lights.
#[derive(Component)]
struct RoomLight(usize);

/// A window pool: how far its light reaches, how bright, and how high up the window it sits.
const WINDOW_RANGE: f32 = 8.0;
const WINDOW_LUMENS: f32 = 140_000.0;
const WINDOW_HEIGHT: f32 = 1.5;
/// How much of a window light's range counts as lit for the fog mask.
const WINDOW_SEEN: f32 = 4.0;
/// Other survivors' lights drawn at once (each casts shadows), and from how far.
const MAX_OTHER_LIGHTS: usize = 3;
const OTHER_LIGHT_REACH: f32 = 60.0;
/// Lit places further than this from us aren't picked out of the dark ([`NightLights`]).
pub const FAR_LIGHT: f32 = 45.0;
/// The lit ground right round someone holding a flashlight (its spill), for the mask.
const HOLDER_SPILL: f32 = 1.6;

/// A light as the fog mask needs it: where, how far it lights the ground well, and for a beam its
/// direction and the cosine of its half-angle (`cos` -1: all round).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeenLight {
    pub pos: Vec2,
    pub radius: f32,
    pub dir: Vec2,
    pub cos: f32,
}

impl SeenLight {
    pub(crate) fn round(pos: Vec2, radius: f32) -> Self {
        Self { pos, radius, dir: Vec2::X, cos: -1.0 }
    }

    /// How well lit a ground point is by this light, 0..1 (no walls: the mask adds line of sight).
    pub fn lit(&self, p: Vec2) -> f32 {
        let d = p - self.pos;
        let dist = d.length();
        if dist >= self.radius {
            return 0.0;
        }
        let fall = 1.0 - smoothstep(self.radius * 0.6, self.radius, dist);
        if self.cos <= -1.0 || dist < 0.3 {
            return fall;
        }
        fall * smoothstep(self.cos - 0.04, self.cos + 0.04, self.dir.dot(d / dist))
    }
}

/// The lights near us that show in the dark this frame (empty by day). See the module docs.
#[derive(Resource, Default)]
pub struct NightLights(pub Vec<SeenLight>);

/// A ground-floor window's pool, lit while either room it is between is.
#[derive(Component)]
struct WindowLight {
    rooms: (u16, u16),
}

/// One of the pool of lights for other survivors: a cone and a flame per slot.
#[derive(Component)]
struct OtherBeam(usize);

#[derive(Component)]
struct OtherFlame(usize);

pub struct LightsPlugin;

impl Plugin for LightsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NightLights>()
            .init_resource::<LitRooms>()
            .init_resource::<RoomPlaces>()
            .add_systems(Startup, spawn_other_lights)
            .add_systems(Update, (sync_lit_rooms, window_lights, room_lights, other_lights, crate::vehicles_render::car_lights, crate::campfire::fires, crate::campfire::built_fires).chain().before(crate::fog::update_fog));
    }
}

/// The light of a ground-floor window between `rooms` whose edge has its middle at `mid`: off
/// until night, and while neither room is lit.
pub fn spawn_window_light(commands: &mut Commands, mid: Vec2, rooms: (u16, u16)) -> Entity {
    let light = PointLight { intensity: 0.0, range: WINDOW_RANGE, radius: 0.3, color: Color::srgb(1.0, 0.6, 0.28), shadow_maps_enabled: false, ..default() };
    commands.spawn((WindowLight { rooms }, light, Transform::from_xyz(mid.x, WINDOW_HEIGHT, mid.y), Visibility::Hidden)).id()
}

fn spawn_other_lights(mut commands: Commands) {
    for slot in 0..MAX_ROOM_LIGHTS {
        let light = PointLight { intensity: 0.0, range: 8.0, radius: 0.2, color: Color::srgb(1.0, 0.78, 0.52), shadow_maps_enabled: false, ..default() };
        commands.spawn((RoomLight(slot), light, Transform::default(), Visibility::Hidden));
    }
    for slot in 0..MAX_OTHER_LIGHTS {
        let beam = SpotLight {
            intensity: 0.0,
            range: 14.0,
            radius: 0.05,
            color: Color::srgb(1.0, 0.93, 0.8),
            shadow_maps_enabled: true,
            outer_angle: BEAM_OUTER,
            inner_angle: BEAM_INNER,
            ..default()
        };
        commands.spawn((OtherBeam(slot), beam, Transform::default(), Visibility::Hidden));
        let flame = PointLight { intensity: 0.0, range: 4.0, radius: 0.05, color: Color::srgb(1.0, 0.72, 0.42), shadow_maps_enabled: true, ..default() };
        commands.spawn((OtherFlame(slot), flame, Transform::default(), Visibility::Hidden));
    }
}

/// How dark it is for light sources: 0 by day, 1 at night (as the lit windows' glow, `materials.rs`).
pub fn darkness(daylight: f32) -> f32 {
    1.0 - smoothstep(0.05, 0.4, daylight)
}

/// A flame's brightness wavering a little, about 1: two slow sines and a quick one, out of step
/// per light (`seed`).
pub fn flicker(t: f32, seed: f32) -> f32 {
    let s = seed * 7.3;
    1.0 + 0.07 * (t * 9.1 + s).sin() + 0.05 * (t * 14.7 + s * 1.7).sin() + 0.04 * (t * 31.0 + s * 0.6).sin()
}

/// Window pools come on as it gets dark, and the ones near us go on the list for the mask.
fn window_lights(
    daylight: Res<Daylight>,
    lit: Res<LitRooms>,
    me: Query<&Transform, (With<crate::player::Player>, Without<WindowLight>)>,
    mut windows: Query<(&WindowLight, &mut PointLight, &Transform, &mut Visibility)>,
    mut seen: ResMut<NightLights>,
) {
    seen.0.clear();
    let dark = darkness(daylight.0);
    let here = me.single().map(|t| t.translation.xz()).ok();
    for (window, mut light, tf, mut vis) in &mut windows {
        let on = dark > 0.0 && lit.either(window.rooms);
        let want = if on { Visibility::Visible } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        let lumens = WINDOW_LUMENS * dark;
        if light.intensity != lumens {
            light.intensity = lumens;
        }
        let at = tf.translation.xz();
        if on && here.is_some_and(|h| h.distance(at) < FAR_LIGHT + WINDOW_SEEN) {
            seen.0.push(SeenLight::round(at, WINDOW_SEEN));
        }
    }
}

/// The nearest lit ground-floor rooms get a light under the ceiling at night, and what it lights
/// goes on the list for the mask.
fn room_lights(
    daylight: Res<Daylight>,
    lit: Res<LitRooms>,
    map: Res<ClientMap>,
    mut places: ResMut<RoomPlaces>,
    me: Query<&Transform, (With<crate::player::Player>, Without<RoomLight>)>,
    mut lights: Query<(&RoomLight, &mut PointLight, &mut Transform, &mut Visibility)>,
    mut seen: ResMut<NightLights>,
) {
    let dark = darkness(daylight.0);
    let here = me.single().map(|t| t.translation.xz()).ok();
    let mut near: Vec<(f32, u16, Vec2, f32)> = Vec::new();
    if dark > 0.0
        && let Some(h) = here
    {
        for &room in &lit.rooms {
            if let Some((mid, radius)) = places.of(&map.0, room) {
                let d = (h.distance(mid) - radius).max(0.0);
                if d < ROOM_LIGHT_REACH {
                    near.push((d, room, mid, radius));
                }
            }
        }
        near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        near.truncate(MAX_ROOM_LIGHTS);
    }
    for (slot, mut light, mut tf, mut vis) in &mut lights {
        match near.get(slot.0) {
            Some(&(_, _, mid, radius)) => {
                let at = Vec3::new(mid.x, ROOM_HEIGHT, mid.y);
                if tf.translation != at {
                    tf.translation = at;
                }
                light.range = radius + ROOM_SPILL;
                light.intensity = ROOM_LUMENS * dark;
                if *vis != Visibility::Visible {
                    *vis = Visibility::Visible;
                }
                seen.0.push(SeenLight::round(mid, radius + 0.5));
            }
            None => {
                if *vis != Visibility::Hidden {
                    *vis = Visibility::Hidden;
                }
            }
        }
    }
}

/// The nearest other survivors holding a light get one drawn: a flashlight's cone along their
/// facing or a candle's pool, as ours (`daynight::held_light_system`).
#[allow(clippy::too_many_arguments)]
fn other_lights(
    join: Res<JoinInfo>,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    daylight: Res<Daylight>,
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    mut beams: Query<(&OtherBeam, &mut SpotLight, &mut Transform, &mut Visibility), Without<OtherFlame>>,
    mut flames: Query<(&OtherFlame, &mut PointLight, &mut Transform, &mut Visibility), Without<OtherBeam>>,
    mut seen: ResMut<NightLights>,
    map: Res<crate::server_link::ClientMap>,
) {
    let alpha = fixed.overstep_fraction();
    let ours = link.curr.players.iter().find(|p| p.id == me.id).map(|p| Vec2::from(p.pos));
    // (distance from us, where, facing, the light, the player's id), nearest first.
    let mut held: Vec<(f32, Vec2, f32, HeldLight, u32)> = link
        .curr
        .players
        .iter()
        .filter(|p| p.id != me.id && !p.dead)
        .filter_map(|p| {
            let outfit = link.curr.outfits.iter().find(|o| o.id == p.id).filter(|o| !o.light_off)?;
            let light = held_light_in(&join, [outfit.hand, outfit.off_hand])?;
            let prev = link.prev.players.iter().find(|q| q.id == p.id).unwrap_or(p);
            let pos = Vec2::from(prev.pos).lerp(Vec2::from(p.pos), alpha);
            let far = ours.map_or(0.0, |o| o.distance(pos));
            (far < OTHER_LIGHT_REACH).then_some((far, pos, p.facing, light, p.id))
        })
        .collect();
    held.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.4.cmp(&b.4)));
    held.truncate(MAX_OTHER_LIGHTS);
    // Weaker by day, as ours.
    let dim = 1.0 - 0.85 * daylight.0;
    let dark = darkness(daylight.0);
    let slot = |i: usize| held.get(i).copied();
    for (beam, mut spot, mut tf, mut vis) in &mut beams {
        match slot(beam.0).filter(|h| !h.3.flame) {
            Some((_, pos, facing, light, _)) => {
                let dir = Vec2::from_angle(facing);
                let (foot, forward) = (Vec3::new(pos.x, map.0.ground(pos), pos.y), Vec3::new(dir.x, 0.0, dir.y));
                *tf = Transform::from_translation(foot + Vec3::Y * BEAM_HEIGHT + forward * 0.35).looking_at(foot + forward * BEAM_AIM_M, Vec3::Y);
                spot.range = light.radius_m + 2.0;
                spot.intensity = 900_000.0 * dim;
                *vis = Visibility::Visible;
                if dark > 0.0 {
                    seen.0.push(SeenLight { pos, radius: light.radius_m, dir, cos: (BEAM_OUTER * 0.9).cos() });
                    seen.0.push(SeenLight::round(pos, HOLDER_SPILL));
                }
            }
            None => *vis = Visibility::Hidden,
        }
    }
    for (flame, mut point, mut tf, mut vis) in &mut flames {
        match slot(flame.0).filter(|h| h.3.flame) {
            Some((_, pos, facing, light, id)) => {
                let dir = Vec2::from_angle(facing);
                *tf = Transform::from_xyz(pos.x + dir.x * 0.3, 1.1 + map.0.ground(pos), pos.y + dir.y * 0.3);
                point.range = light.radius_m + 1.0;
                point.intensity = 40_000.0 * dim * flicker(time.elapsed_secs(), id as f32);
                *vis = Visibility::Visible;
                if dark > 0.0 {
                    seen.0.push(SeenLight::round(pos, light.radius_m));
                }
            }
            None => *vis = Visibility::Hidden,
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_beam_lights_what_is_ahead_of_it_and_a_pool_all_round() {
        let beam = SeenLight { pos: Vec2::new(10.0, 10.0), radius: 12.0, dir: Vec2::X, cos: (BEAM_OUTER * 0.9).cos() };
        assert!(beam.lit(Vec2::new(15.0, 10.0)) > 0.99, "straight ahead, well inside its reach");
        assert_eq!(beam.lit(Vec2::new(5.0, 10.0)), 0.0, "behind");
        assert_eq!(beam.lit(Vec2::new(10.0, 15.0)), 0.0, "to the side");
        assert_eq!(beam.lit(Vec2::new(23.0, 10.0)), 0.0, "past its reach");
        let mid = beam.lit(Vec2::new(20.0, 10.0));
        assert!(mid > 0.0 && mid < 1.0, "fading toward the end: {mid}");
        let pool = SeenLight::round(Vec2::ZERO, 3.5);
        for a in 0..8 {
            assert!(pool.lit(Vec2::from_angle(a as f32 * 0.8) * 1.5) > 0.99);
        }
        assert_eq!(pool.lit(Vec2::new(0.0, 3.6)), 0.0);
    }

    #[test]
    fn lit_rooms_say_which_rooms_are_lit() {
        let lit = LitRooms { rooms: vec![3, 9, 14], grid: true };
        assert!(lit.is_lit(9) && !lit.is_lit(4) && !lit.is_lit(0));
        assert!(lit.either((0, 14)) && lit.either((3, 0)) && !lit.either((0, 5)));
    }

    #[test]
    fn a_room_is_found_by_its_tiles() {
        let w = zombye_sim::World::new(zombye_sim::WorldConfig::new(1, 0, vec![zombye_sim::BuildingTemplate::from_ron(include_str!("../../../assets/data/buildings/bungalow.ron")).unwrap()]));
        let room = w.map.room_at(w.player_spawn);
        let (mid, radius) = room_place(&w.map, room).expect("a ground-floor room");
        assert_eq!(w.map.room_at(mid), room, "its middle is in it");
        assert!(radius > 1.0 && radius < 12.0, "{radius}");
        assert!(mid.distance(w.player_spawn) < radius + 0.01);
    }

    #[test]
    fn lights_are_off_by_day_and_full_at_night() {
        assert_eq!(darkness(1.0), 0.0);
        assert_eq!(darkness(0.4), 0.0);
        assert_eq!(darkness(0.0), 1.0);
        assert!(darkness(0.2) > 0.0 && darkness(0.2) < 1.0);
    }

    #[test]
    fn a_flame_wavers_a_little_and_never_goes_out() {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for i in 0..2000 {
            let f = flicker(i as f32 * 0.013, 3.0);
            (lo, hi) = (lo.min(f), hi.max(f));
        }
        assert!(lo > 0.8 && hi < 1.2, "{lo}..{hi}");
        assert!(hi - lo > 0.15, "it should be seen to waver: {lo}..{hi}");
        assert_ne!(flicker(1.0, 1.0), flicker(1.0, 2.0), "two candles out of step");
    }
}
