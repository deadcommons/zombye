//! Fog of war: what the player can see right now.
//!
//! Every frame we cast rays from the player through the client's copy of the map (walls and
//! closed doors block sight; windows don't), then rasterise the result into a small mask texture
//! around the player: red = how visible, green = indoors. The world shader darkens whatever is
//! unseen, and turns unseen indoor areas near-black, so a cutaway wall doesn't reveal a room you
//! can't actually see into. Zombies outside the mask aren't drawn at all. A view cone makes what's
//! behind you dimmer, and darkness shortens how far you can see.

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

#[derive(Resource)]
pub struct FogState {
    pub image: Handle<Image>,
    /// World x/z of the mask's min corner.
    pub origin: Vec2,
    pub size_m: f32,
    /// Two bytes per texel: visibility, indoors.
    values: Vec<u8>,
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
        app.insert_resource(FogState { image: handle, origin: Vec2::ZERO, size_m: TEXELS as f32 * TEXEL_M, values: vec![0; (TEXELS * TEXELS * 2) as usize] })
            .add_systems(Update, update_fog);
    }
}

fn update_fog(
    map: Res<ClientMap>,
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
    let mut max = 18.0 * (night + (1.0 - night) * daylight.0) + 52.0 * daylight.0;
    // Tired: a narrower view cone; asleep, eyes shut: only what's right around you (#11).
    let needs = link.own(me.id).map(|v| v.needs).unwrap_or_default();
    if needs.asleep.is_some() {
        max = (max * 0.15).max(CLOSE_RADIUS + 1.0);
    }
    let narrow = if join.needs.level(needs.tiredness) >= zombye_sim::needs::Level::Strong { join.needs.tired_sight.clamp(0.3, 1.0) } else { 1.0 };
    let (cone_lo, cone_hi) = ((-0.6f32).acos() * narrow, (-0.25f32).acos() * narrow);
    let (cone_lo, cone_hi) = (cone_lo.cos(), cone_hi.cos());
    let rays = vision::visibility_rays(&map.0, eye, max, RAYS);
    let facing = Vec2::from_angle(player.facing);

    // Snap the mask to the texel grid so it doesn't shimmer as you move.
    let half = fog.size_m * 0.5;
    let origin = ((eye - Vec2::splat(half)) / TEXEL_M).floor() * TEXEL_M;
    fog.origin = origin;
    let tau = std::f32::consts::TAU;
    for ty in 0..TEXELS {
        for tx in 0..TEXELS {
            let q = origin + (Vec2::new(tx as f32, ty as f32) + 0.5) * TEXEL_M;
            let d = q - eye;
            let dist = d.length();
            let v = if dist > max + 1.5 {
                0.0
            } else {
                // Angle → fractional ray index; take the nearer of the two rays so light doesn't leak past corners.
                let a = d.y.atan2(d.x).rem_euclid(tau) / tau * RAYS as f32;
                let i0 = a.floor() as usize % RAYS;
                let i1 = (i0 + 1) % RAYS;
                let reach = rays[i0].min(rays[i1]);
                let los = 1.0 - smoothstep(reach + 0.3, reach + 1.1, dist);
                let range = 1.0 - smoothstep(max * 0.75, max, dist);
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
    let handle = fog.image.clone();
    if let Some(mut image) = images.get_mut(&handle) {
        if let Some(data) = image.data.as_mut() {
            data.copy_from_slice(&fog.values);
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
