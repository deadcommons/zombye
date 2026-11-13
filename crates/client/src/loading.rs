//! The loading screen: the logo over the whole window from the game's first frame until the
//! world behind it is ready to be looked at, then a short fade.
//!
//! The game builds its town and meshes before its first frame, but the GPU is still compiling
//! shaders for the frames after it, so things pop in one by one. This covers that: it stays up
//! until the render world has had no pipeline waiting for a few frames ([`ready`]), and never
//! longer than [`DEADLINE`]. In your own world time stands still behind it (`pause_menu::stop_time`),
//! so nothing happens to a survivor you can't see yet; on a server the world goes on.
//!
//! It shows when the game was started as a player starts it ([`wanted`]: from the main menu, or a
//! plain start with the menu skipped). A run driven by flags (`--load`, `--fight-demo`,
//! `--screenshot`...) goes straight in as before, so no recipe or recording changes.
//! `--loading-screen` shows it regardless and holds it up, for pictures:
//! `--render-to shot.png --hud --loading-screen`.
//!
//! With the brand folder (`release_brand.rs`) it is the logo; without, the name as text.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::render::render_resource::PipelineCache;
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::Options;
use crate::menu_skin::{BONE, DIM, GROUND};
use crate::release_brand::{self as brand, Mark};
use crate::theme;

/// Frames before the screen may go: the first few queue the pipelines it waits for.
const MIN_FRAMES: u32 = 12;
/// Frames in a row with no pipeline waiting.
const QUIET_FRAMES: u32 = 5;
/// The least it shows, so it is a screen and not a flash.
const MIN_SHOWN: Duration = Duration::from_millis(700);
/// The most: whatever is still compiling then pops in over the game.
const DEADLINE: Duration = Duration::from_secs(20);
/// The fade out.
const FADE_SECS: f32 = 0.35;
/// How wide the logo is drawn, in UI pixels.
const LOGO_WIDTH: f32 = 520.0;

/// Whether this run starts behind the loading screen.
pub fn wanted(o: &Options) -> bool {
    o.loading_screen || (!o.offscreen() && (o.no_menu || o.plain) && o.screenshot.is_none() && o.exit_after.is_none())
}

/// The loading screen's state. `up` while it covers the game (fading included).
#[derive(Resource)]
pub struct Loading {
    pub up: bool,
    /// Held up for a picture (`--loading-screen`).
    hold: bool,
    frames: u32,
    quiet: u32,
    started: Instant,
    /// Seconds into the fade, once it began.
    fading: Option<f32>,
}

/// Pipelines the render world is still waiting for, counted at the end of each render frame.
#[derive(Resource, Clone, Default)]
struct Waiting(Arc<AtomicUsize>);

#[derive(Component)]
struct Root;

pub struct LoadingPlugin;

impl Plugin for LoadingPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().resource::<Options>();
        if !wanted(options) {
            return;
        }
        let hold = options.loading_screen;
        let waiting = Waiting::default();
        app.insert_resource(Loading { up: true, hold, frames: 0, quiet: 0, started: Instant::now(), fading: None })
            .insert_resource(waiting.clone())
            .add_systems(Startup, spawn)
            .add_systems(Update, drive.run_if(|l: Res<Loading>| l.up));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.insert_resource(waiting).add_systems(Render, count_waiting.in_set(RenderSystems::Cleanup));
        }
    }
}

fn count_waiting(cache: Res<PipelineCache>, waiting: Res<Waiting>) {
    waiting.0.store(cache.waiting_pipelines().count(), Ordering::Relaxed);
}

fn spawn(mut commands: Commands) {
    commands
        .spawn((
            Root,
            Node { position_type: PositionType::Absolute, left: px(0), top: px(0), width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, row_gap: px(30), ..default() },
            BackgroundColor(GROUND),
            // Over every panel and the HUD.
            GlobalZIndex(i32::MAX - 8),
        ))
        .with_children(|c| {
            if !brand::title(c, Mark::Logo, LOGO_WIDTH, DIM) {
                brand::text_title(c, 104.0, BONE, DIM);
            }
            c.spawn((Text::new("LOADING"), theme::head(18.0), TextColor(DIM), TextLayout::no_wrap(), Pickable::IGNORE));
        });
}

/// Whether the world behind the screen is ready to be shown.
fn ready(frames: u32, quiet: u32, shown: Duration) -> bool {
    shown >= DEADLINE || (frames >= MIN_FRAMES && quiet >= QUIET_FRAMES && shown >= MIN_SHOWN)
}

/// How much of itself the screen still shows `secs` into its fade (1 to 0, eased).
fn fade(secs: f32) -> f32 {
    let t = (secs / FADE_SECS).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

fn drive(
    mut commands: Commands,
    mut loading: ResMut<Loading>,
    waiting: Res<Waiting>,
    time: Res<Time<Real>>,
    root: Query<Entity, With<Root>>,
    children: Query<&Children>,
    mut backs: Query<&mut BackgroundColor, With<Root>>,
    mut texts: Query<&mut TextColor>,
    mut pictures: Query<&mut ImageNode>,
) {
    if loading.hold {
        return;
    }
    loading.frames += 1;
    loading.quiet = if waiting.0.load(Ordering::Relaxed) == 0 { loading.quiet + 1 } else { 0 };
    if loading.fading.is_none() && ready(loading.frames, loading.quiet, loading.started.elapsed()) {
        loading.fading = Some(0.0);
    }
    let Some(secs) = loading.fading.map(|s| s + time.delta_secs()) else { return };
    loading.fading = Some(secs);
    let Ok(root) = root.single() else {
        loading.up = false;
        return;
    };
    if secs >= FADE_SECS {
        commands.entity(root).despawn();
        loading.up = false;
        return;
    }
    let a = fade(secs);
    for e in std::iter::once(root).chain(children.iter_descendants(root)) {
        if let Ok(mut b) = backs.get_mut(e) {
            b.0.set_alpha(a);
        }
        if let Ok(mut t) = texts.get_mut(e) {
            t.0.set_alpha(a);
        }
        if let Ok(mut p) = pictures.get_mut(e) {
            p.color.set_alpha(a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_waits_for_the_shaders_but_not_for_ever() {
        let s = Duration::from_secs;
        // The first frames have queued nothing yet: not ready however quiet.
        assert!(!ready(3, 3, s(2)));
        // Something still compiling.
        assert!(!ready(200, 0, s(5)));
        // Quiet for long enough, but it only just came up.
        assert!(!ready(MIN_FRAMES, QUIET_FRAMES, Duration::from_millis(100)));
        assert!(ready(MIN_FRAMES, QUIET_FRAMES, s(1)));
        // Never past the deadline.
        assert!(ready(5000, 0, DEADLINE));
    }

    #[test]
    fn the_fade_runs_from_all_to_nothing() {
        assert_eq!(fade(0.0), 1.0);
        assert_eq!(fade(FADE_SECS), 0.0);
        assert_eq!(fade(FADE_SECS * 5.0), 0.0);
        assert!(fade(FADE_SECS * 0.25) > fade(FADE_SECS * 0.75));
    }

    #[test]
    fn only_a_players_start_gets_the_screen() {
        let of = |args: &[&str]| Options::parse(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        // From the main menu, and a plain start with the menu skipped.
        assert!(wanted(&of(&["--load", "autosave", "--no-menu"])));
        assert!(wanted(&of(&[])));
        // A run set up by flags goes straight in, and a picture never has it unless asked.
        assert!(!wanted(&of(&["--fight-demo", "--autoplay"])));
        assert!(!wanted(&of(&["--render-to", "/tmp/x.png", "--no-menu"])));
        assert!(wanted(&of(&["--render-to", "/tmp/x.png", "--hud", "--loading-screen"])));
    }
}
