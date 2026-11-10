//! Dev tool: a time-of-day slider for judging the light (#83). F3 opens it at the foot of the
//! screen; drag along it, or hold `[` / `]`, to pick an hour. Only what this client draws follows
//! it (sun, moon, ambient, grade, vignette, a held light, and how far you see in the fog of war);
//! the server's clock, the zombies' senses and the HUD's time don't. F3 again closes it and the
//! light goes back to the server's hour.

use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;

const BAR_W: f32 = 520.0;
/// Hours a second while `[` or `]` is held.
const SCRUB_RATE: f32 = 3.0;

/// The hour the light is drawn at instead of the server's, while the slider is open.
#[derive(Resource, Default)]
pub struct LightPreview {
    pub hour: Option<f32>,
    /// The mouse is on the slider (so a click there doesn't also swing).
    pub hovered: bool,
}

impl LightPreview {
    /// The hour to light the world at.
    pub fn hour_or(&self, server: f32) -> f32 {
        self.hour.unwrap_or(server)
    }
}

#[derive(Component)]
struct Panel;

#[derive(Component)]
struct Track;

#[derive(Component)]
struct Handle;

#[derive(Component)]
struct Label;

pub struct DevTimePlugin;

impl Plugin for DevTimePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LightPreview>().add_systems(Startup, spawn_panel).add_systems(Update, (toggle, drive, show).chain());
    }
}

fn spawn_panel(mut commands: Commands) {
    commands
        .spawn((
            Panel,
            Node {
                position_type: PositionType::Absolute,
                bottom: px(18),
                left: Val::Percent(50.0),
                margin: UiRect::left(px(-(BAR_W / 2.0 + 12.0))),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                padding: UiRect::all(px(12)),
                ..default()
            },
            BackgroundColor(crate::theme::HUD_BG),
            Visibility::Hidden,
        ))
        .with_children(|c| {
            c.spawn((Label, Text::new(""), crate::theme::mono(14.0), TextColor(Color::srgb(0.35, 0.9, 1.0))));
            c.spawn((Track, Node { width: px(BAR_W), height: px(16), ..default() }, BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.12)), RelativeCursorPosition::default()))
                .with_children(|t| {
                    // Dawn, noon and dusk marks.
                    for h in [6.0, 12.0, 20.0] {
                        t.spawn((
                            Node { position_type: PositionType::Absolute, left: Val::Percent(h / 24.0 * 100.0), width: px(1), height: Val::Percent(100.0), ..default() },
                            BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.3)),
                        ));
                    }
                    t.spawn((Handle, Node { position_type: PositionType::Absolute, left: Val::Percent(50.0), width: px(6), height: Val::Percent(100.0), margin: UiRect::left(px(-3)), ..default() }, BackgroundColor(Color::srgb(0.35, 0.9, 1.0))));
                });
        });
}

fn toggle(keys: Res<ButtonInput<KeyCode>>, link: Res<crate::server_link::ServerLink>, mut preview: ResMut<LightPreview>) {
    if keys.just_pressed(KeyCode::F3) {
        preview.hour = match preview.hour {
            Some(_) => None,
            None => Some(link.curr.time_of_day),
        };
    }
}

fn drive(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time>,
    track: Query<&RelativeCursorPosition, With<Track>>,
    mut preview: ResMut<LightPreview>,
    mut dragging: Local<bool>,
) {
    let Some(mut hour) = preview.hour else {
        preview.hovered = false;
        *dragging = false;
        return;
    };
    let cursor = track.single().ok();
    let over = cursor.is_some_and(|c| c.cursor_over());
    if mouse.just_pressed(MouseButton::Left) && over {
        *dragging = true;
    }
    if !mouse.pressed(MouseButton::Left) {
        *dragging = false;
    }
    if *dragging && let Some(n) = cursor.and_then(|c| c.normalized) {
        // `normalized` runs from -0.5 to 0.5 across the node.
        hour = ((n.x + 0.5).clamp(0.0, 1.0) * 24.0).min(23.99);
    }
    let step = SCRUB_RATE * time.delta_secs();
    if keys.pressed(KeyCode::BracketLeft) {
        hour = (hour - step).rem_euclid(24.0);
    }
    if keys.pressed(KeyCode::BracketRight) {
        hour = (hour + step).rem_euclid(24.0);
    }
    preview.hour = Some(hour);
    preview.hovered = over || *dragging;
}

fn show(
    preview: Res<LightPreview>,
    mut panel: Query<&mut Visibility, With<Panel>>,
    mut handle: Query<&mut Node, With<Handle>>,
    mut label: Query<&mut Text, With<Label>>,
) {
    if let Ok(mut v) = panel.single_mut() {
        *v = if preview.hour.is_some() { Visibility::Inherited } else { Visibility::Hidden };
    }
    let Some(hour) = preview.hour else { return };
    if let Ok(mut n) = handle.single_mut() {
        n.left = Val::Percent(hour / 24.0 * 100.0);
    }
    if let Ok(mut t) = label.single_mut() {
        let (h, m) = (hour.floor() as u32, ((hour.fract()) * 60.0).floor() as u32);
        t.0 = format!("LIGHT PREVIEW {h:02}:{m:02}   drag, or hold [ ]   F3 closes (the server's clock runs on)");
    }
}
