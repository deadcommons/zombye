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
//!
//! Two rules, kept apart: **what is drawn** (zombies, players, items: [`FogState::visibility`], the
//! constants below) and **how the ground looks** ([`FogLook`], from the data folder's `fog.ron`: a
//! soft cone only a little brighter than what's behind you, and how dim what's out of sight is).

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
/// Always see this far around you, even behind (what is drawn; the look is [`FogLook`]'s).
const CLOSE_RADIUS: f32 = 2.5;
/// How visible what's behind you is (what is drawn: under the 0.35 a zombie needs to show).
const BEHIND: f32 = 0.28;

/// What is out of sight: its colour cast, how much darker it is (0..1) and how much of its colour
/// it keeps (0 grey, 1 all).
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unseen {
    pub tint: (f32, f32, f32),
    pub dark: f32,
    pub colour: f32,
}

/// How the fog of war looks: `fog.ron` in the data folder (each field described there; one left
/// out takes its default, no file at all the shipped one).
#[derive(Resource, Clone, Copy, Debug, PartialEq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FogLook {
    pub behind: f32,
    pub cone_full: f32,
    pub cone_behind: f32,
    pub close_radius: f32,
    pub close_fade: f32,
    pub range_fade_from: f32,
    pub unseen_day: Unseen,
    pub unseen_night: Unseen,
}

const SHIPPED: &str = include_str!("../../../assets/data/fog.ron");

impl Default for FogLook {
    fn default() -> Self {
        FogLook {
            behind: 0.5,
            cone_full: 60.0,
            cone_behind: 125.0,
            close_radius: 1.0,
            close_fade: 4.0,
            range_fade_from: 0.5,
            unseen_day: Unseen { tint: (0.94, 0.9, 0.84), dark: 0.4, colour: 0.85 },
            unseen_night: Unseen { tint: (0.55, 0.6, 0.75), dark: 0.86, colour: 0.25 },
        }
    }
}

impl FogLook {
    pub fn parse(text: &str) -> Result<FogLook, String> {
        let look: FogLook = ron::from_str(text).map_err(|e| e.to_string())?;
        let unit = |name: &str, v: f32| if (0.0..=1.0).contains(&v) { Ok(()) } else { Err(format!("{name} must be from 0 to 1, not {v}")) };
        unit("behind", look.behind)?;
        unit("range_fade_from", look.range_fade_from)?;
        for (name, u) in [("unseen_day", look.unseen_day), ("unseen_night", look.unseen_night)] {
            unit(&format!("{name}.dark"), u.dark)?;
            unit(&format!("{name}.colour"), u.colour)?;
            for c in [u.tint.0, u.tint.1, u.tint.2] {
                if !(0.0..=2.0).contains(&c) {
                    return Err(format!("{name}.tint must be from 0 to 2, not {c}"));
                }
            }
        }
        if !(look.cone_full >= 0.0 && look.cone_full < look.cone_behind && look.cone_behind <= 180.0) {
            return Err(format!("the cone must have 0 <= cone_full < cone_behind <= 180, not {} and {}", look.cone_full, look.cone_behind));
        }
        if !(look.close_radius >= 0.0 && look.close_radius <= 50.0 && look.close_fade >= 0.0 && look.close_fade <= 50.0) {
            return Err(format!("close_radius and close_fade must be from 0 to 50, not {} and {}", look.close_radius, look.close_fade));
        }
        Ok(look)
    }

    /// `fog.ron` from the data folder `dir`, the shipped one if it has none.
    pub fn load(dir: &std::path::Path) -> Result<FogLook, String> {
        let path = dir.join("fog.ron");
        match std::fs::read_to_string(&path) {
            Ok(text) => FogLook::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(_) => Ok(FogLook::parse(SHIPPED).expect("the shipped fog.ron is good (a test checks)")),
        }
    }

    /// What's out of sight at this daylight (0 night, 1 day): rgb its tint, w how dark it gets.
    pub fn unseen_tint(&self, daylight: f32) -> Vec4 {
        let v = |u: Unseen| Vec4::new(u.tint.0, u.tint.1, u.tint.2, u.dark);
        v(self.unseen_night).lerp(v(self.unseen_day), daylight.clamp(0.0, 1.0))
    }

    /// How much of its colour an unseen surface keeps at this daylight (the rest goes grey).
    pub fn unseen_colour(&self, daylight: f32) -> f32 {
        lerp(self.unseen_night.colour, self.unseen_day.colour, daylight.clamp(0.0, 1.0))
    }
}
/// How far you see at night with no light in hand (m).
const NIGHT_SIGHT: f32 = 10.0;

#[derive(Resource)]
pub struct FogState {
    /// The mask texture: green = indoors. Its red (how visible) is no longer filled in: the shader
    /// works that out per pixel from the rays, and [`FogState::visibility`] per point asked for.
    pub image: Handle<Image>,
    /// World x/z of the mask's min corner.
    pub origin: Vec2,
    pub size_m: f32,
    /// Each ray's reach in metres, one `R32Float` texel per ray (`RAYS` × 1).
    pub rays: Handle<Image>,
    /// For the shader, the look ([`FogLook`]): (eye x, eye z, sight range, beam reach), (facing x,
    /// z, cone cos lo, hi), (beam cos, behind, close radius, 1 = in use).
    pub sight: [Vec4; 3],
    /// More of the look for the shader: (metres the cone eases in over past the close radius, the
    /// share of the sight range the far fade starts at).
    pub soft: Vec2,
    /// What [`FogState::visibility`] works from; `None` until the first update.
    view: Option<View>,
    /// What the rays were last cast with (eye, reach), and the origin the mask texture was last
    /// filled for: neither is done again until it changes (or the map does).
    cast: Option<(Vec2, f32)>,
    filled: Option<Vec2>,
}

/// The player's sight as of the last update: everything a mask texel's visibility follows from.
struct View {
    rays: Vec<f32>,
    eye: Vec2,
    facing: Vec2,
    max: f32,
    beam: f32,
    beam_cos: f32,
    cone: (f32, f32),
    lit_dark: f32,
    lights: Vec<crate::lights::SeenLight>,
}

impl View {
    /// How visible the mask texel (`tx`, `ty`) is, 0..=255: sight along the rays, fading with range
    /// and behind the player, and whatever a light in the dark shows of it.
    fn texel(&self, origin: Vec2, tx: u32, ty: u32) -> u8 {
        let q = origin + (Vec2::new(tx as f32, ty as f32) + 0.5) * TEXEL_M;
        let d = q - self.eye;
        let dist = d.length();
        // Along the beam the sight range is the beam's reach.
        let in_beam = if self.beam > self.max && dist > 0.1 { smoothstep(self.beam_cos - 0.04, self.beam_cos + 0.04, self.facing.dot(d / dist)) } else { 0.0 };
        let max = lerp(self.max, self.beam, in_beam);
        let v = if dist > max + 1.5 {
            0.0
        } else {
            // Angle → fractional ray index; take the nearer of the two rays so light doesn't leak past corners.
            let los = line_of_sight(&self.rays, d);
            let range = 1.0 - smoothstep(max * 0.5, max, dist);
            let cone = if dist < CLOSE_RADIUS {
                1.0
            } else {
                let c = self.facing.dot(d / dist.max(1e-4));
                BEHIND + (1.0 - BEHIND) * smoothstep(self.cone.0, self.cone.1, c)
            };
            los * range * cone
        };
        let mut value = (v.clamp(0.0, 1.0) * 255.0) as u8;
        // What's lit is seen (#83): the ground a window's pool or another survivor's light falls on
        // shows in the dark however far off, given a line of sight to it, so whoever stands in it is drawn.
        let t = IVec2::new(tx as i32, ty as i32);
        for light in &self.lights {
            let lo = ((light.pos - light.radius - origin) / TEXEL_M).floor().as_ivec2().max(IVec2::ZERO);
            let hi = ((light.pos + light.radius - origin) / TEXEL_M).ceil().as_ivec2().min(IVec2::splat(TEXELS as i32 - 1));
            if t.cmplt(lo).any() || t.cmpgt(hi).any() {
                continue;
            }
            let lit = light.lit(q) * self.lit_dark;
            if lit > 0.0 {
                value = value.max((lit * line_of_sight(&self.rays, q - self.eye) * 255.0) as u8);
            }
        }
        value
    }
}

impl FogState {
    /// How visible a ground point is right now (0 = unseen, 1 = in plain sight): the value of the
    /// half-metre mask texel it is in, worked out when asked. A few thousand zombies, items and
    /// players ask each frame; filling all 65,536 texels for them cost over a millisecond.
    pub fn visibility(&self, p: Vec2) -> f32 {
        let t = ((p - self.origin) / TEXEL_M).floor().as_ivec2();
        if t.x < 0 || t.y < 0 || t.x >= TEXELS as i32 || t.y >= TEXELS as i32 {
            return 0.0;
        }
        self.view.as_ref().map_or(0.0, |v| v.texel(self.origin, t.x as u32, t.y as u32) as f32 / 255.0)
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
            rays,
            sight: [Vec4::ZERO; 3],
            soft: Vec2::new(0.0, 0.5),
            view: None,
            cast: None,
            filled: None,
        })
            .init_resource::<FogLook>()
            .add_systems(Update, update_fog);
    }
}

pub fn update_fog(
    map: Res<ClientMap>,
    lights: Res<crate::lights::NightLights>,
    daylight: Res<Daylight>,
    look: Res<FogLook>,
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
    let facing = Vec2::from_angle(player.facing);
    let fog = &mut *fog;
    let map_changed = map.is_changed();

    // The rays: cast again only when the eye, their reach or the map changed (standing still costs
    // nothing), and sent to the GPU only when they came out different.
    let cast = (eye, reach_max.max(far));
    let mut view = fog.view.take();
    if fog.cast != Some(cast) || map_changed || view.is_none() {
        fog.cast = Some(cast);
        let rays = cast_rays(&map.0, cast.0, cast.1);
        if view.as_ref().is_none_or(|v| v.rays != rays) {
            if let Some(mut image) = images.get_mut(&fog.rays)
                && let Some(data) = image.data.as_mut()
            {
                for (out, r) in data.chunks_exact_mut(4).zip(&rays) {
                    out.copy_from_slice(&r.to_le_bytes());
                }
            }
        }
        match view.as_mut() {
            Some(v) => v.rays = rays,
            None => view = Some(View { rays, eye, facing, max, beam, beam_cos, cone: (cone_lo, cone_hi), lit_dark, lights: Vec::new() }),
        }
    }
    if let Some(v) = view.as_mut() {
        (v.eye, v.facing, v.max, v.beam, v.beam_cos, v.cone, v.lit_dark) = (eye, facing, max, beam, beam_cos, (cone_lo, cone_hi), lit_dark);
        v.lights.clear();
        v.lights.extend_from_slice(&lights.0);
    }
    fog.view = view;

    // Snap the mask to the texel grid so it doesn't shimmer as you move. The texture only says
    // what's indoors now, which changes when the mask moves on by a texel (every half metre walked).
    let half = fog.size_m * 0.5;
    let origin = ((eye - Vec2::splat(half)) / TEXEL_M).floor() * TEXEL_M;
    fog.origin = origin;
    if (fog.filled != Some(origin) || map_changed)
        && let Some(mut image) = images.get_mut(&fog.image)
        && let Some(data) = image.data.as_mut()
    {
        fog.filled = Some(origin);
        for ty in 0..TEXELS {
            for tx in 0..TEXELS {
                let q = origin + (Vec2::new(tx as f32, ty as f32) + 0.5) * TEXEL_M;
                data[((ty * TEXELS + tx) * 2 + 1) as usize] = if map.0.tile(q.floor().as_ivec2()).room != 0 { 255 } else { 0 };
            }
        }
    }
    // The look: its own, softer cone (tiredness narrows it as it does the real one).
    let (look_lo, look_hi) = ((look.cone_behind.to_radians() * narrow).cos(), (look.cone_full.to_radians() * narrow).cos());
    fog.sight = [Vec4::new(eye.x, eye.y, max, beam), Vec4::new(facing.x, facing.y, look_lo, look_hi), Vec4::new(beam_cos, look.behind, look.close_radius, 1.0)];
    fog.soft = Vec2::new(look.close_fade, look.range_fade_from);
}

/// The sight rays (`vision::visibility_rays`, the same values), cast in runs on the task pool: a
/// thousand rays of up to 70 m are most of what's left of the fog's cost while walking.
fn cast_rays(map: &zombye_sim::map::TileMap, eye: Vec2, reach: f32) -> Vec<f32> {
    const RUN: usize = RAYS / 8;
    let Some(pool) = bevy::tasks::ComputeTaskPool::try_get() else { return vision::visibility_rays(map, eye, reach, RAYS) };
    let mut runs = pool.scope(|s| {
        for k in 0..RAYS / RUN {
            s.spawn(async move { (k, (k * RUN..(k + 1) * RUN).map(|i| vision::cast_ray(map, eye, Vec2::from_angle(i as f32 / RAYS as f32 * std::f32::consts::TAU), reach)).collect::<Vec<f32>>()) });
        }
    });
    runs.sort_by_key(|r| r.0);
    runs.into_iter().flat_map(|r| r.1).collect()
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

    /// The mask as it was filled before (every texel's sight, then each light's box over it).
    fn old_mask(v: &View, origin: Vec2) -> Vec<u8> {
        let mut values = vec![0u8; (TEXELS * TEXELS) as usize];
        for ty in 0..TEXELS {
            for tx in 0..TEXELS {
                let q = origin + (Vec2::new(tx as f32, ty as f32) + 0.5) * TEXEL_M;
                let d = q - v.eye;
                let dist = d.length();
                let in_beam = if v.beam > v.max && dist > 0.1 { smoothstep(v.beam_cos - 0.04, v.beam_cos + 0.04, v.facing.dot(d / dist)) } else { 0.0 };
                let max = lerp(v.max, v.beam, in_beam);
                let s = if dist > max + 1.5 {
                    0.0
                } else {
                    let los = line_of_sight(&v.rays, d);
                    let range = 1.0 - smoothstep(max * 0.5, max, dist);
                    let cone = if dist < CLOSE_RADIUS { 1.0 } else { BEHIND + (1.0 - BEHIND) * smoothstep(v.cone.0, v.cone.1, v.facing.dot(d / dist.max(1e-4))) };
                    los * range * cone
                };
                values[(ty * TEXELS + tx) as usize] = (s.clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
        for light in &v.lights {
            let lo = ((light.pos - light.radius - origin) / TEXEL_M).floor().as_ivec2().max(IVec2::ZERO);
            let hi = ((light.pos + light.radius - origin) / TEXEL_M).ceil().as_ivec2().min(IVec2::splat(TEXELS as i32 - 1));
            for ty in lo.y..=hi.y {
                for tx in lo.x..=hi.x {
                    let q = origin + (Vec2::new(tx as f32, ty as f32) + 0.5) * TEXEL_M;
                    let lit = light.lit(q) * v.lit_dark;
                    if lit <= 0.0 {
                        continue;
                    }
                    let i = (ty as u32 * TEXELS + tx as u32) as usize;
                    values[i] = values[i].max((lit * line_of_sight(&v.rays, q - v.eye) * 255.0) as u8);
                }
            }
        }
        values
    }

    /// Asking for a point's visibility gives exactly what the filled mask held for its texel: by
    /// day, and at night with a flashlight's beam and lights in the dark (one half off the mask).
    #[test]
    fn a_point_asked_for_is_the_old_masks_texel() {
        use crate::lights::SeenLight;
        let eye = Vec2::new(203.37, 181.62);
        let origin = ((eye - Vec2::splat(64.0)) / TEXEL_M).floor() * TEXEL_M;
        // Rays of uneven reach, as among houses.
        let rays = |reach: f32| (0..RAYS).map(|i| (4.0 + 53.0 * crate::meshgen::hash01(i as i32 / 7, 3, 77)).min(reach)).collect::<Vec<f32>>();
        let cone = ((-0.6f32).acos().cos(), (-0.25f32).acos().cos());
        let day = View { rays: rays(70.0), eye, facing: Vec2::from_angle(0.7), max: 70.0, beam: 0.0, beam_cos: 0.8, cone, lit_dark: 0.0, lights: Vec::new() };
        let lights = vec![
            SeenLight { pos: eye + Vec2::new(30.0, -12.0), radius: 5.0, dir: Vec2::ZERO, cos: -1.0 },
            SeenLight { pos: eye + Vec2::new(-20.0, 25.0), radius: 14.0, dir: Vec2::X, cos: 0.84 },
            SeenLight { pos: origin + Vec2::new(-1.0, 40.0), radius: 6.0, dir: Vec2::ZERO, cos: -1.0 },
        ];
        let night = View { rays: rays(45.0), eye, facing: Vec2::from_angle(-2.1), max: 10.0, beam: 20.0, beam_cos: 0.85, cone, lit_dark: 1.0, lights };
        for (name, view) in [("day", day), ("night", night)] {
            let old = old_mask(&view, origin);
            assert!(old.iter().filter(|v| **v > 0).count() > 500 && old.iter().any(|v| *v == 0), "{name}: a mask with seen and unseen parts");
            if name == "night" {
                let far = |i: usize| (origin + (Vec2::new((i as u32 % TEXELS) as f32, (i as u32 / TEXELS) as f32) + 0.5) * TEXEL_M).distance(eye) > 23.0;
                assert!(old.iter().enumerate().any(|(i, v)| *v > 0 && far(i)), "a light past the night's sight and the beam shows");
            }
            let fog = FogState { image: Handle::default(), origin, size_m: 128.0, rays: Handle::default(), sight: [Vec4::ZERO; 3], soft: Vec2::ZERO, view: Some(view), cast: None, filled: None };
            for ty in 0..TEXELS {
                for tx in 0..TEXELS {
                    // Anywhere in the texel, its corners included.
                    for at in [Vec2::new(0.25, 0.25), Vec2::new(0.0, 0.0), Vec2::new(0.49, 0.49)] {
                        let p = origin + Vec2::new(tx as f32, ty as f32) * TEXEL_M + at;
                        assert_eq!(fog.visibility(p), old[(ty * TEXELS + tx) as usize] as f32 / 255.0, "{name}: texel {tx}, {ty}");
                    }
                }
            }
            assert_eq!(fog.visibility(origin - Vec2::splat(0.1)), 0.0, "off the mask");
        }
    }

    /// The shipped fog.ron is the defaults, mistakes are named, and the look is subtle: what's
    /// behind you is nearly as bright as what's ahead, and by day what's out of sight is a
    /// little dimmer, not a dark hole (at night it stays nearly black, #83).
    #[test]
    fn the_shipped_look_is_subtle() {
        let look = FogLook::parse(SHIPPED).unwrap();
        assert_eq!(look, FogLook::default());
        // Behind you is dimmer enough to see the cone, never a hard wedge or a ring.
        assert!((0.4..=0.7).contains(&look.behind) && look.close_fade >= 3.0);
        assert!(look.cone_behind - look.cone_full >= 45.0);
        let day = look.unseen_tint(1.0);
        assert!(day.w <= 0.45 && day.x > day.z && look.unseen_colour(1.0) >= 0.8, "{day}");
        assert_eq!(look.unseen_tint(0.0), Vec4::new(0.55, 0.6, 0.75, 0.86));
        assert_eq!(look.unseen_colour(0.0), 0.25);
        assert_eq!(FogLook::parse("(behind: 0.5)").unwrap().behind, 0.5);
        assert!(FogLook::parse("(behind: 1.5)").unwrap_err().contains("behind"));
        assert!(FogLook::parse("(cone_full: 170, cone_behind: 90)").unwrap_err().contains("cone"));
        assert!(FogLook::parse("(unseen_day: (tint: (1, 1, 1), dark: 2, colour: 1))").unwrap_err().contains("unseen_day.dark"));
        assert!(FogLook::parse("(bhind: 0.5)").is_err());
    }

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
