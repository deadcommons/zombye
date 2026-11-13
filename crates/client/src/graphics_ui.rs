//! Graphics presets in the running game: F8 opens a small panel to pick Low, Medium, High, Ultra
//! or Auto (kept in `settings.ron`), what each one changes is applied while you play, and Auto
//! measures the frame time for a few seconds and settles on a preset (`graphics.rs`).
//!
//! Applied live: the grass and flowers' density, the film pass (grain, tilt-shift), the vignette,
//! bloom, the sun's shadow map, multisampling and the render scale (`render_scale.rs`). Contact shadows and ground detail are built into
//! the meshes and materials when the town is, so they change on the next start; the panel says so.

use bevy::input::InputSystems;
use bevy::light::DirectionalLightShadowMap;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;

use crate::Options;
use crate::camera::IsoCamera;
use crate::graphics::{Graphics, Preset};
use crate::saving::Notice;
use crate::theme::{self, BODY, DIM, INK, SMALL, TEXT, TITLE, font};
use crate::ui_scale::SettingsStore;

/// Auto: frames before this (seconds of play) are the town still settling in and aren't counted.
const AUTO_SKIP_SECS: f32 = 3.0;
/// Auto: seconds of frames it measures.
const AUTO_MEASURE_SECS: f32 = 5.0;
/// Auto keeps High when the mean frame takes at most this (about 70 fps), Medium up to
/// [`AUTO_MEDIUM_MS`], else Low. Ultra is only ever picked by hand.
const AUTO_HIGH_MS: f32 = 14.5;
const AUTO_MEDIUM_MS: f32 = 22.0;

const WIDTH: f32 = 430.0 + 2.0 * theme::SHEET_PAD.0;

pub struct GraphicsUiPlugin;

impl Plugin for GraphicsUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GraphicsUi>()
            .init_resource::<AutoMeasure>()
            .add_systems(Startup, spawn)
            .add_systems(PreUpdate, keys.after(InputSystems).after(crate::inventory_ui::TextEntry).run_if(crate::live))
            .add_systems(Update, (pick, auto_pick.run_if(crate::live), apply, show).chain());
    }
}

/// The multisampling for a sample count (1, 2 or 4).
pub fn msaa(samples: u32) -> Msaa {
    match samples {
        0 | 1 => Msaa::Off,
        2 | 3 => Msaa::Sample2,
        _ => Msaa::Sample4,
    }
}

/// Auto's pick for a mean frame time (ms).
pub fn auto_choice(mean_ms: f32) -> Preset {
    if mean_ms <= AUTO_HIGH_MS {
        Preset::High
    } else if mean_ms <= AUTO_MEDIUM_MS {
        Preset::Medium
    } else {
        Preset::Low
    }
}

/// One line on what a preset is.
pub fn describe(p: Preset) -> &'static str {
    match p {
        Preset::Low => "Thin grass, no post effects, 2048 shadow map, 2x MSAA, 3D at 60% on Retina. For slow machines.",
        Preset::Medium => "Most of the look: lighter grass, no tilt-shift or sky light, 2048 shadow map, 3D at 75% on Retina.",
        Preset::High => "The look the game is made to: full grass and every effect, 3D at 75% on Retina, UI sharp.",
        Preset::Ultra => "Denser grass, an 8192 shadow map and the 3D at full Retina resolution.",
        Preset::Auto => "Plays a few seconds at High, measures, and keeps what this machine holds.",
        Preset::Custom => "Set by command-line flags or by hand in settings.ron.",
    }
}

#[derive(Resource, Default)]
pub(crate) struct GraphicsUi {
    pub(crate) open: bool,
    /// A preset clicked in the panel, applied in [`pick`].
    picked: Option<Preset>,
    /// The panel shows this (rebuilt when it changes).
    shown: Option<(bool, Preset)>,
}

/// Auto's frame times while it measures.
#[derive(Resource, Default)]
pub(crate) struct AutoMeasure {
    played: f32,
    frames: u32,
    total: f32,
}

#[derive(Component)]
struct GraphicsPanel;

fn spawn(mut commands: Commands, options: Res<Options>, mut ui: ResMut<GraphicsUi>) {
    ui.open |= options.graphics_panel;
    commands.spawn((
        GraphicsPanel,
        Node { position_type: PositionType::Absolute, top: px(16), left: Val::Percent(50.0), margin: UiRect::left(px(-WIDTH / 2.0)), width: px(WIDTH), flex_direction: FlexDirection::Column, row_gap: px(8), padding: UiRect::axes(px(theme::SHEET_PAD.0), px(theme::SHEET_PAD.1)), display: Display::None, ..default() },
        theme::sheet(),
        GlobalZIndex(9),
    ));
}

fn keys(keys: Res<ButtonInput<KeyCode>>, mut ui: ResMut<GraphicsUi>) {
    if keys.just_pressed(KeyCode::F8) {
        ui.open = !ui.open;
    } else if ui.open && keys.just_pressed(KeyCode::Escape) {
        ui.open = false;
    }
}

/// A preset picked in the panel: the settings it stands for, saved for next time.
fn pick(mut ui: ResMut<GraphicsUi>, mut graphics: ResMut<Graphics>, store: Option<ResMut<SettingsStore>>, mut auto: ResMut<AutoMeasure>, mut notice: ResMut<Notice>, time: Res<Time>) {
    let Some(p) = ui.picked.take() else { return };
    *graphics = Graphics::of(p);
    *auto = AutoMeasure::default();
    save(store, &graphics);
    notice.say(time.elapsed_secs(), format!("Graphics: {}", p.name()));
}

fn save(store: Option<ResMut<SettingsStore>>, graphics: &Graphics) {
    if let Some(mut store) = store
        && let Err(e) = store.change(|s| s.graphics = graphics.clone())
    {
        eprintln!("graphics settings not saved: {e}");
    }
}

/// Auto: measure a few seconds of frames, then settle on a preset and save it.
fn auto_pick(time: Res<Time<Real>>, mut auto: ResMut<AutoMeasure>, mut graphics: ResMut<Graphics>, store: Option<ResMut<SettingsStore>>, mut notice: ResMut<Notice>) {
    if graphics.preset != Preset::Auto {
        return;
    }
    let dt = time.delta_secs();
    auto.played += dt;
    if auto.played < AUTO_SKIP_SECS {
        return;
    }
    auto.frames += 1;
    auto.total += dt;
    if auto.played < AUTO_SKIP_SECS + AUTO_MEASURE_SECS {
        return;
    }
    let mean_ms = auto.total / auto.frames.max(1) as f32 * 1000.0;
    let chosen = auto_choice(mean_ms);
    *graphics = Graphics::of(chosen);
    save(store, &graphics);
    notice.say(time.elapsed_secs(), format!("Graphics: Auto picked {} ({:.0} fps at High). F8 to change.", chosen.name(), 1000.0 / mean_ms));
}

/// Put the settings into effect when they change.
fn apply(graphics: Res<Graphics>, mut shadow: ResMut<DirectionalLightShadowMap>, mut commands: Commands, mut cams: Query<(Entity, &mut Msaa, Has<Bloom>), With<IsoCamera>>) {
    if !graphics.is_changed() {
        return;
    }
    if shadow.size != graphics.shadow_map as usize {
        shadow.size = graphics.shadow_map as usize;
    }
    for (cam, mut msaa, has_bloom) in &mut cams {
        let want = self::msaa(graphics.msaa);
        if *msaa != want {
            *msaa = want;
        }
        match (graphics.bloom, has_bloom) {
            (true, false) => {
                commands.entity(cam).insert(crate::camera::bloom());
            }
            (false, true) => {
                commands.entity(cam).remove::<Bloom>();
            }
            _ => {}
        }
    }
}

fn show(mut commands: Commands, mut ui: ResMut<GraphicsUi>, graphics: Res<Graphics>, options: Res<Options>, mut panel: Query<(Entity, &mut Node), With<GraphicsPanel>>) {
    let want = (ui.open, graphics.preset);
    if ui.shown == Some(want) {
        return;
    }
    ui.shown = Some(want);
    let Ok((entity, mut node)) = panel.single_mut() else { return };
    node.display = if ui.open { Display::Flex } else { Display::None };
    commands.entity(entity).despawn_related::<Children>();
    if !ui.open {
        return;
    }
    let current = graphics.preset;
    let from_flags = options.graphics.is_some() || options.vegetation.is_some() || options.no_post.is_some();
    commands.entity(entity).with_children(|p| {
        p.spawn((Text::new("GRAPHICS"), theme::head(TITLE), TextColor(TEXT), Pickable::IGNORE));
        p.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(6), flex_wrap: FlexWrap::Wrap, ..default() }, Pickable::IGNORE)).with_children(|row| {
            for choice in Preset::CHOICES {
                let on = choice == current;
                row.spawn((Node { padding: UiRect::axes(px(12), px(4)), ..default() }, theme::button(on)))
                    .with_children(|b| {
                        b.spawn((Text::new(choice.name()), theme::strong(BODY), TextColor(INK), Pickable::IGNORE));
                    })
                    .observe(move |_: On<PointerPress>, mut ui: ResMut<GraphicsUi>| ui.picked = Some(choice));
            }
        });
        p.spawn((Text::new(format!("{}: {}", current.name(), describe(current))), font(BODY), TextColor(TEXT), Pickable::IGNORE));
        let mut notes = String::from("Contact shadows, ground detail and sky light change on the next start.");
        if from_flags {
            notes.push_str(" Flags on the command line win over this until you pick here.");
        }
        notes.push_str("  F8 or Esc closes.");
        p.spawn((Text::new(notes), font(SMALL), TextColor(DIM), Pickable::IGNORE));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_picks_by_frame_time() {
        assert_eq!(auto_choice(8.0), Preset::High);
        assert_eq!(auto_choice(14.5), Preset::High);
        assert_eq!(auto_choice(18.0), Preset::Medium);
        assert_eq!(auto_choice(40.0), Preset::Low);
    }

    #[test]
    fn sample_counts() {
        assert_eq!(msaa(1), Msaa::Off);
        assert_eq!(msaa(2), Msaa::Sample2);
        assert_eq!(msaa(4), Msaa::Sample4);
        assert_eq!(msaa(8), Msaa::Sample4);
    }
}
