//! Fog of war: what the player can see right now.
//!
//! Every frame we cast rays from the player through the client's copy of the map (walls and
//! closed doors block sight; windows don't), then rasterise the result into a small mask texture
//! around the player: red = how visible, green = indoors. The rays themselves also go to the GPU
//! (one texel each, [`FogState::rays`]) with the sight range and view cone ([`FogState::sight`]),
//! so the world shader works out what's seen per pixel: line-of-sight edges are smooth lines, not
//! the mask's half-metre steps (the mask still says what's indoors, and which zombies to draw). The
//! world shader darkens whatever is unseen, and turns unseen indoor areas near-black, so a cutaway wall doesn't reveal a room you
//! can't actually see into. Zombies outside the mask aren't drawn at all. A view cone makes what's
//! behind you dimmer, and darkness shortens how far you can see: at night only a few metres,
//! except along a held flashlight's beam or round a candle (#83).

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use zombye_sim::vision;

use crate::daynight::Daylight;
use crate::player::Player;
use crate::server_link::ClientMap;

/// Mask resolution and coverage: 256 × 256 texels at 0.5 m = 128 m square around the player.
const TEXELS: u32 = 256;
const TEXEL_M: f32 = 0.5;
const RAYS: usize = 1024;
/// Always see this far around you, even behind.
const CLOSE_RADIUS: f32 = 2.5;
/// Brightness multiplier for what's behind you.
const BEHIND: f32 = 0.28;
/// How far you see at night with no light in hand (m).
const NIGHT_SIGHT: f32 = 10.0;

#[derive(Resource)]
pub struct FogState {
    pub image: Handle<Image>,
    /// World x/z of the mask's min corner.
    pub origin: Vec2,
    pub size_m: f32,
    /// Two bytes per texel: visibility, indoors.
    values: Vec<u8>,
    /// Each ray's reach in metres, one `R32Float` texel per ray (`RAYS` × 1).
    pub rays: Handle<Image>,
    /// For the shader: (eye x, eye z, sight range, beam reach), (facing x, z, cone cos lo, hi),
    /// (beam cos, behind, close radius, 1 = in use).
    pub sight: [Vec4; 3],
}

impl FogState {
    /// How visible a ground point is right now (0 = unseen, 1 = in plain sight).
    pub fn visibility(&self, p: Vec2) -> f32 {
        let t = ((p - self.origin) / TEXEL_M).floor().as_ivec2();
        if t.x < 0 || t.y < 0 || t.x >= TEXELS as i32 || t.y >= TEXELS as i32 {
            return 0.0;
        }
        self.values[((t.y as u32 * TEXELS + t.x as u32) * 2) as usize] as f32 / 255.0
    }
}

pub struct FogPlugin;

impl Plugin for FogPlugin {
    fn build(&self, app: &mut App) {
        let mut image = Image::new(
            Extent3d { width: TEXELS, height: TEXELS, depth_or_array_layers: 1 },
            TextureDimension::D2,
            vec![0; (TEXELS * TEXELS * 2) as usize],
            TextureFormat::Rg8Unorm,
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        );
        image.sampler = ImageSampler::linear();
        let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        let mut rays = Image::new(
            Extent3d { width: RAYS as u32, height: 1, depth_or_array_layers: 1 },
            TextureDimension::D2,
            vec![0; RAYS * 4],
            TextureFormat::R32Float,
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        );
        rays.sampler = ImageSampler::nearest();
        let rays = app.world_mut().resource_mut::<Assets<Image>>().add(rays);
        app.insert_resource(FogState {
            image: handle,
            origin: Vec2::ZERO,
            size_m: TEXELS as f32 * TEXEL_M,
            values: vec![0; (TEXELS * TEXELS * 2) as usize],
            rays,
            sight: [Vec4::ZERO; 3],
        })
            .add_systems(Update, update_fog);
    }
}

pub fn update_fog(
    map: Res<ClientMap>,
    lights: Res<crate::lights::NightLights>,
    daylight: Res<Daylight>,
    link: Res<crate::server_link::ServerLink>,
    join: Res<crate::server_link::JoinInfo>,
    me: Res<crate::player::LocalPlayer>,
    player: Query<(&Transform, &Player)>,
    mut fog: ResMut<FogState>,
    mut images: ResMut<Assets<Image>>,
) {
    let Ok((tf, player)) = player.single() else { return };
    let eye = Vec2::new(tf.translation.x, tf.translation.z);
    // Sight range: long in daylight, short at night, and shorter still at night while turning (#9).
    let turning = link.own(me.id).is_some_and(|v| v.health.infection == Some(zombye_sim::InfectionStage::Turning));
    let night = if turning { join.infection.turning_night_sight.clamp(0.0, 1.0) } else { 1.0 };
    let dark = 1.0 - daylight.0;
    let mut max = NIGHT_SIGHT * (night + (1.0 - night) * daylight.0) + (70.0 - NIGHT_SIGHT) * daylight.0;
    // A light in hand: a flashlight sees far along its beam, a candle a little way all round.
    let held = crate::daynight::held_light(&join, &link, me.id);
    let beam = held.filter(|l| !l.flame).map_or(0.0, |l| (l.radius_m + 2.0) * dark);
    if let Some(l) = held.filter(|l| l.flame) {
        max = max.max(lerp(max, l.radius_m + 1.5, dark));
    }
    // Tired: a narrower view cone; asleep, eyes shut: only what's right around you (#11).
    let needs = link.own(me.id).map(|v| v.needs).unwrap_or_default();
    if needs.asleep.is_some() {
        max = (max * 0.15).max(CLOSE_RADIUS + 1.0);
    }
    let narrow = if join.needs.level(needs.tiredness) >= zombye_sim::needs::Level::Strong { join.needs.tired_sight.clamp(0.3, 1.0) } else { 1.0 };
    let (cone_lo, cone_hi) = ((-0.6f32).acos() * narrow, (-0.25f32).acos() * narrow);
    let (cone_lo, cone_hi) = (cone_lo.cos(), cone_hi.cos());
    let reach_max = max.max(beam);
    let beam_cos = (crate::daynight::BEAM_OUTER * 1.15).cos();
    // In the dark the rays go on past what we see by ourselves, as far as a lit place shows (below).
    let lit_dark = crate::lights::darkness(daylight.0);
    let far = if lit_dark > 0.0 { crate::lights::FAR_LIGHT } else { 0.0 };
    let rays = vision::visibility_rays(&map.0, eye, reach_max.max(far), RAYS);
    let facing = Vec2::from_angle(player.facing);

    // Snap the mask to the texel grid so it doesn't shimmer as you move.
    let half = fog.size_m * 0.5;
    let origin = ((eye - Vec2::splat(half)) / TEXEL_M).floor() * TEXEL_M;
    fog.origin = origin;
    for ty in 0..TEXELS {
        for tx in 0..TEXELS {
            let q = origin + (Vec2::new(tx as f32, ty as f32) + 0.5) * TEXEL_M;
            let d = q - eye;
            let dist = d.length();
            // Along the beam the sight range is the beam's reach.
            let in_beam = if beam > max && dist > 0.1 { smoothstep(beam_cos - 0.04, beam_cos + 0.04, facing.dot(d / dist)) } else { 0.0 };
            let max = lerp(max, beam, in_beam);
            let v = if dist > max + 1.5 {
                0.0
            } else {
                // Angle → fractional ray index; take the nearer of the two rays so light doesn't leak past corners.
                let los = line_of_sight(&rays, d);
                let range = 1.0 - smoothstep(max * 0.5, max, dist);
                let cone = if dist < CLOSE_RADIUS {
                    1.0
                } else {
                    let c = facing.dot(d / dist.max(1e-4));
                    BEHIND + (1.0 - BEHIND) * smoothstep(cone_lo, cone_hi, c)
                };
                los * range * cone
            };
            let i = ((ty * TEXELS + tx) * 2) as usize;
            fog.values[i] = (v.clamp(0.0, 1.0) * 255.0) as u8;
            fog.values[i + 1] = if map.0.tile(q.floor().as_ivec2()).room != 0 { 255 } else { 0 };
        }
    }
    // What's lit is seen (#83): the ground a window's pool or another survivor's light falls on
    // shows in the dark however far off, given a line of sight to it, so whoever stands in it is drawn.
    for light in &lights.0 {
        let lo = ((light.pos - light.radius - origin) / TEXEL_M).floor().as_ivec2().max(IVec2::ZERO);
        let hi = ((light.pos + light.radius - origin) / TEXEL_M).ceil().as_ivec2().min(IVec2::splat(TEXELS as i32 - 1));
        for ty in lo.y..=hi.y {
            for tx in lo.x..=hi.x {
                let q = origin + (Vec2::new(tx as f32, ty as f32) + 0.5) * TEXEL_M;
                let lit = light.lit(q) * lit_dark;
                if lit <= 0.0 {
                    continue;
                }
                let v = (lit * line_of_sight(&rays, q - eye) * 255.0) as u8;
                let i = ((ty as u32 * TEXELS + tx as u32) * 2) as usize;
                fog.values[i] = fog.values[i].max(v);
            }
        }
    }
    fog.sight = [Vec4::new(eye.x, eye.y, max, beam), Vec4::new(facing.x, facing.y, cone_lo, cone_hi), Vec4::new(beam_cos, BEHIND, CLOSE_RADIUS, 1.0)];
    let handle = fog.image.clone();
    if let Some(mut image) = images.get_mut(&handle) {
        if let Some(data) = image.data.as_mut() {
            data.copy_from_slice(&fog.values);
        }
    }
    let handle = fog.rays.clone();
    if let Some(mut image) = images.get_mut(&handle) {
        if let Some(data) = image.data.as_mut() {
            for (out, r) in data.chunks_exact_mut(4).zip(&rays) {
                out.copy_from_slice(&r.to_le_bytes());
            }
        }
    }
}

/// Is the point `d` from the eye in line of sight (1) or behind something (0), going by the rays
/// cast round the eye (soft over the last metre)? The nearer of the two rays either side, so
/// sight doesn't leak past corners. `los_at` in world.wesl is the same.
fn line_of_sight(rays: &[f32], d: Vec2) -> f32 {
    let tau = std::f32::consts::TAU;
    let n = rays.len();
    let a = d.y.atan2(d.x).rem_euclid(tau) / tau * n as f32;
    let i0 = a.floor() as usize % n;
    let i1 = (i0 + 1) % n;
    let reach = rays[i0].min(rays[i1]);
    1.0 - smoothstep(reach + 0.3, reach + 1.1, d.length())
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_of_sight_stops_at_the_nearer_ray() {
        // Open all round to 45 m, but for a wall 6 m off to the east (the rays round +x).
        let mut rays = vec![45.0f32; RAYS];
        for i in (0..12).chain(RAYS - 12..RAYS) {
            rays[i] = 6.0;
        }
        assert_eq!(line_of_sight(&rays, Vec2::new(5.0, 0.0)), 1.0, "this side of the wall");
        assert_eq!(line_of_sight(&rays, Vec2::new(8.0, 0.0)), 0.0, "behind it");
        assert_eq!(line_of_sight(&rays, Vec2::new(0.0, 30.0)), 1.0, "a lit place far off to the north");
        assert_eq!(line_of_sight(&rays, Vec2::new(0.0, 47.0)), 0.0, "past where the rays go");
        // At the wall's end the nearer ray wins, so sight doesn't leak round the corner.
        let edge = Vec2::from_angle(12.0 / RAYS as f32 * std::f32::consts::TAU - 1e-4) * 20.0;
        assert_eq!(line_of_sight(&rays, edge), 0.0);
    }
}
