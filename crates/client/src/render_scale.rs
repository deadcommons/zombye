//! Render scale (a graphics setting): on a HiDPI screen the 3D world is drawn at a fraction of the
//! window's pixels and scaled up smoothly, while the UI stays at full resolution, sharp.
//!
//! At scale 1 the world camera draws straight to the window and is the UI's camera, as it always
//! was. Below 1 it draws to an image of the scaled size (with a light contrast-adaptive sharpen so
//! the upscale doesn't look soft), and a second camera on the window shows that image behind the
//! UI, which it draws itself at the window's own resolution.
//!
//! Everything that turns a cursor into a point in the world, or a point in the world into a place
//! on screen, goes through [`to_view`] and [`from_view`] so it works at any scale.
//!
//! The scale applies only where it pays and doesn't show: on a screen with a scale factor of 1.5 or
//! more (Retina), the preset's scale; on an ordinary screen, a scale this far below 1 would be
//! visibly soft, so it is raised toward 1 ([`effective`]). Offscreen renders ignore it.

use bevy::anti_alias::contrast_adaptive_sharpening::ContrastAdaptiveSharpening;
use bevy::asset::RenderAssetUsages;
use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::window::{PrimaryWindow, WindowRef};

use crate::camera::IsoCamera;
use crate::graphics::Graphics;

/// The sharpening used while scaled down (CAS, 0..1).
const SHARPEN: f32 = 0.35;

pub struct RenderScalePlugin;

impl Plugin for RenderScalePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, apply.run_if(crate::live).before(bevy::camera::CameraUpdateSystems));
    }
}

/// The scale actually used for a setting on a screen with this scale factor: the setting on HiDPI
/// screens, and on a screen of scale factor 1 a scale a third as far below 1 (0.75 there is 0.92).
pub fn effective(setting: f32, scale_factor: f32) -> f32 {
    let s = setting.clamp(0.5, 1.0);
    if scale_factor >= 1.5 { s } else { 1.0 - (1.0 - s) / 3.0 }
}

/// The world camera's viewport size over the window's (1 at full scale).
fn ratio(camera: &Camera, window: &Window) -> f32 {
    match camera.logical_viewport_size() {
        Some(size) if window.width() > 0.0 => size.x / window.width(),
        _ => 1.0,
    }
}

/// A point in the window (logical pixels, as `Window::cursor_position` gives) in the world camera's
/// viewport.
pub fn to_view(camera: &Camera, window: &Window, p: Vec2) -> Vec2 {
    p * ratio(camera, window)
}

/// A point in the world camera's viewport (as `Camera::world_to_viewport` gives) in the window.
pub fn from_view(camera: &Camera, window: &Window, p: Vec2) -> Vec2 {
    p / ratio(camera, window)
}

/// The image the world is drawn into while scaled, and the camera that shows it with the UI.
#[derive(Resource)]
struct Scaled {
    image: Handle<Image>,
    ui_camera: Entity,
    backdrop: Entity,
    size: UVec2,
}

#[derive(Component)]
struct Backdrop;

fn target_image(size: UVec2) -> Image {
    let mut image = Image::new_fill(
        Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    image.sampler = ImageSampler::linear();
    image
}

#[allow(clippy::too_many_arguments)]
fn apply(
    mut commands: Commands,
    graphics: Res<Graphics>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    scaled: Option<ResMut<Scaled>>,
    mut world_cam: Query<(Entity, &mut RenderTarget), With<IsoCamera>>,
    mut backdrops: Query<&mut ImageNode, With<Backdrop>>,
) {
    let (Ok(window), Ok((cam, mut target))) = (windows.single(), world_cam.single_mut()) else { return };
    let scale = effective(graphics.render_scale, window.scale_factor());
    let full = window.physical_size();
    let size = (full.as_vec2() * scale).round().as_uvec2().max(UVec2::ONE);
    let want_scaled = scale < 0.999 && full.x > 0 && full.y > 0;
    match (want_scaled, scaled) {
        (true, None) => {
            let image = images.add(target_image(size));
            *target = RenderTarget::Image(image.clone().into());
            commands.entity(cam).remove::<IsDefaultUiCamera>().insert(ContrastAdaptiveSharpening { enabled: true, sharpening_strength: SHARPEN, denoise: false });
            let ui_camera = commands
                .spawn((Camera2d, Camera { order: 1, clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Window(WindowRef::Primary), Msaa::Off, IsDefaultUiCamera))
                .id();
            let backdrop = commands
                .spawn((Backdrop, ImageNode::new(image.clone()), Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() }, GlobalZIndex(i32::MIN), Pickable::IGNORE))
                .id();
            commands.insert_resource(Scaled { image, ui_camera, backdrop, size });
        }
        (true, Some(mut s)) if s.size != size => {
            // The window or the setting changed: a new image of the new size.
            let image = images.add(target_image(size));
            *target = RenderTarget::Image(image.clone().into());
            if let Ok(mut node) = backdrops.get_mut(s.backdrop) {
                node.image = image.clone();
            }
            images.remove(&s.image);
            s.image = image;
            s.size = size;
        }
        (false, Some(s)) => {
            *target = RenderTarget::Window(WindowRef::Primary);
            commands.entity(cam).insert(IsDefaultUiCamera).remove::<ContrastAdaptiveSharpening>();
            commands.entity(s.ui_camera).despawn();
            commands.entity(s.backdrop).despawn();
            images.remove(&s.image);
            commands.remove_resource::<Scaled>();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_only_bites_on_hidpi() {
        assert_eq!(effective(0.75, 2.0), 0.75);
        assert_eq!(effective(1.0, 2.0), 1.0);
        assert!((effective(0.75, 1.0) - 0.9167).abs() < 1e-3);
        assert_eq!(effective(0.2, 2.0), 0.5);
    }
}
