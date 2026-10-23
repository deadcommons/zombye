//! The status tags (#11): small tags bottom left, beside the health and endurance bars, one for each
//! status that applies (hunger, thirst, tiredness, endurance, load, pain, bleeding, sick, and asleep),
//! coloured by how bad it is. Hover one for what it does. They come from our private view and the
//! predicted endurance through the same function the server's MCP tools use
//! (`PrivateView::status_tags`), so the HUD and an agent see the same tags. Also B to sleep (in
//! `player.rs`).

use bevy::prelude::*;
use zombye_sim::{StatusTag, TagKind};

use crate::player::LocalPlayer;
use crate::server_link::{JoinInfo, ServerLink};

/// Where the tags go: right of the vitals panel (`hud.rs`), bottom-aligned with it.
const LEFT: f32 = 372.0;
const TEXT: Color = Color::srgb(0.95, 0.93, 0.88);
const TIP_BG: Color = Color::srgba(0.02, 0.02, 0.03, 0.85);

#[derive(Component)]
struct TagsRow;
#[derive(Component)]
struct TipText;

/// The tags on screen, to rebuild them only when they change.
#[derive(Resource, Default, PartialEq)]
struct Shown(Vec<StatusTag>);

/// The tooltip of the tag the pointer is over.
#[derive(Resource, Default)]
struct Hovered(Option<String>);

pub struct NeedsUiPlugin;

impl Plugin for NeedsUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Shown>().init_resource::<Hovered>().add_systems(Startup, spawn).add_systems(Update, (rebuild, tooltip).chain());
    }
}

fn font(size: f32) -> TextFont {
    TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }
}

/// The tag's colour: by kind for the gentle ones, amber to red as they get worse.
fn color(t: &StatusTag) -> Color {
    match (t.kind, t.severity) {
        (TagKind::Asleep, _) => Color::srgba(0.2, 0.3, 0.6, 0.9),
        (_, 0 | 1) => Color::srgba(0.45, 0.42, 0.25, 0.9),
        (_, 2) => Color::srgba(0.75, 0.42, 0.1, 0.92),
        _ => Color::srgba(0.72, 0.1, 0.08, 0.95),
    }
}

fn spawn(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: px(16),
                left: px(LEFT),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexStart,
                row_gap: px(4),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|c| {
            c.spawn((TipText, Text::new(""), font(13.0), TextColor(TEXT), Node { max_width: px(360), padding: UiRect::all(px(6)), ..default() }, BackgroundColor(TIP_BG), Visibility::Hidden));
            c.spawn((TagsRow, Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(4), row_gap: px(4), max_width: px(380), ..default() }, Pickable::IGNORE));
        });
}

fn rebuild(mut commands: Commands, link: Res<ServerLink>, join: Res<JoinInfo>, me: Res<LocalPlayer>, mut shown: ResMut<Shown>, mut hovered: ResMut<Hovered>, row: Query<Entity, With<TagsRow>>) {
    let Ok(row) = row.single() else { return };
    let tags = match link.own(me.id) {
        Some(v) if !me.dead => v.status_tags(&join.needs, me.curr.endurance, me.curr.winded, join.combat.tired_below, link.curr.time_scale),
        _ => Vec::new(),
    };
    if shown.0 == tags {
        return;
    }
    commands.entity(row).despawn_children();
    hovered.0 = None;
    commands.entity(row).with_children(|r| {
        for t in &tags {
            let tip = t.tip.clone();
            r.spawn((Node { padding: UiRect::axes(px(7), px(3)), ..default() }, BackgroundColor(color(t))))
                .with_children(|chip| {
                    chip.spawn((Text::new(t.label.clone()), font(13.0), TextColor(TEXT), Pickable::IGNORE));
                })
                .observe(move |_: On<PointerOver>, mut h: ResMut<Hovered>| h.0 = Some(tip.clone()))
                .observe(|_: On<PointerOut>, mut h: ResMut<Hovered>| h.0 = None);
        }
    });
    shown.0 = tags;
}

fn tooltip(hovered: Res<Hovered>, mut tip: Query<(&mut Text, &mut Visibility), With<TipText>>) {
    let Ok((mut text, mut vis)) = tip.single_mut() else { return };
    let want = hovered.0.clone().unwrap_or_default();
    if text.0 != want {
        text.0 = want;
    }
    let v = if hovered.0.is_some() { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != v {
        *vis = v;
    }
}
