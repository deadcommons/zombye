//! The controller's wheel on screen (#12, stage 2): `wheel.rs`'s choices as tiles on a ring round
//! the middle of the window, the one the stick points at in green, and in the hub what it is.
//!
//! The first choice is straight up and the rest go clockwise, by `wheel::angle_of`, the rule the
//! stick picks by. The world shows through: a light veil, a soft dark disc behind the ring, and
//! only the hub, the tiles and the footer are solid enough to read on. Nothing here takes the
//! pointer. Built again only when the wheel says it changed (`Wheel::changes`), the pad's maker
//! changed (its buttons' names) or the help text did. Not gated on `crate::live`, so an offscreen
//! picture shows it (`--render-to shot.png --hud --wheel items:1`).

use bevy::prelude::*;

use crate::pad::{Family, InputMode};
use crate::server_link::JoinInfo;
use crate::theme::{self, flat};
use crate::wheel::{Entry, Kind, Wheel};

/// From the middle of the ring to the middle of a tile, in UI pixels.
const RADIUS: f32 = 170.0;
/// A tile's side, and the picked one's.
const TILE: f32 = 76.0;
const TILE_PICKED: f32 = 90.0;
/// The box the ring is laid out in: the ring, the picked tile's reach and a margin.
const BOX: f32 = 2.0 * RADIUS + TILE_PICKED + 20.0;
/// The hub's diameter: what the tiles leave free in the middle.
const HUB: f32 = 2.0 * RADIUS - TILE_PICKED - 14.0;
/// The most letters of a label a tile without a picture shows.
const TILE_LETTERS: usize = 20;
/// Over the HUD, the hotbar, the current action (8) and the first night's panel (9); under the
/// debug stats (15), the map (25) and the pause menu (30).
const Z: i32 = 12;

#[derive(Component)]
struct Root;

pub struct WheelUiPlugin;

impl Plugin for WheelUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn).add_systems(Update, draw);
    }
}

/// Where the middle of choice `i` of `n` is from the middle of a ring of `radius`: x to the
/// right, y up. The first straight up, the rest clockwise (`wheel::angle_of`).
pub fn tile_pos(i: usize, n: usize, radius: f32) -> Vec2 {
    let a = crate::wheel::angle_of(i, n);
    Vec2::new(a.sin(), a.cos()) * radius
}

/// A label short enough for a tile: cut at a word where there is one, with dots.
fn short(label: &str) -> String {
    if label.chars().count() <= TILE_LETTERS {
        return label.to_string();
    }
    let cut: String = label.chars().take(TILE_LETTERS - 3).collect();
    let cut = match cut.rfind(' ') {
        Some(at) if at >= 8 => &cut[..at],
        _ => cut.as_str(),
    };
    format!("{}...", cut.trim_end())
}

fn spawn(mut commands: Commands) {
    commands.spawn((
        Root,
        Node {
            position_type: PositionType::Absolute,
            left: px(0),
            top: px(0),
            width: percent(100),
            height: percent(100),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            display: Display::None,
            ..default()
        },
        // A light veil: the game goes on behind the wheel and must be seen to.
        BackgroundColor(flat::INK.with_alpha(0.22)),
        GlobalZIndex(Z),
        Pickable::IGNORE,
    ));
}

/// The wheel as it is now, when it changed.
fn draw(
    wheel: Option<Res<Wheel>>,
    join: Option<Res<JoinInfo>>,
    mode: Option<Res<InputMode>>,
    mut root: Query<(Entity, &mut Node), With<Root>>,
    mut drawn: Local<Option<(u32, Family, bool)>>,
    mut commands: Commands,
) {
    let Some(wheel) = wheel else { return };
    let Ok((entity, mut node)) = root.single_mut() else { return };
    let family = mode.as_ref().map_or(Family::default(), |m| m.family);
    let hints = crate::help_text::keys();
    let now = (wheel.changes, family, hints);
    if *drawn == Some(now) {
        return;
    }
    *drawn = Some(now);
    commands.entity(entity).despawn_related::<Children>();
    let display = if wheel.open.is_some() { Display::Flex } else { Display::None };
    if node.display != display {
        node.display = display;
    }
    let Some(kind) = wheel.open else { return };
    let items: &[zombye_sim::ItemDef] = join.as_deref().map(|j| j.items.as_slice()).unwrap_or(&[]);
    commands.entity(entity).with_children(|c| {
        c.spawn((Node { width: px(BOX), height: px(BOX), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|ring| {
            // A soft dark disc under it all, so the ring holds together over any ground.
            ring.spawn((
                Node { position_type: PositionType::Absolute, left: px(0), top: px(0), width: px(BOX), height: px(BOX), border_radius: BorderRadius::MAX, ..default() },
                BackgroundColor(flat::INK.with_alpha(0.5)),
                Pickable::IGNORE,
            ));
            hub(ring, &wheel);
            let n = wheel.entries.len();
            for (i, entry) in wheel.entries.iter().enumerate() {
                tile(ring, entry, tile_pos(i, n, RADIUS), wheel.pick == Some(i), items);
            }
            if hints {
                footer(ring, kind, n == 0, family);
            }
        });
    });
}

/// The middle: the wheel's title, what is picked and a word more about it.
fn hub(ring: &mut ChildSpawnerCommands, wheel: &Wheel) {
    let at = (BOX - HUB) / 2.0;
    ring.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(at),
            top: px(at),
            width: px(HUB),
            height: px(HUB),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            row_gap: px(6),
            padding: UiRect::all(px(26)),
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::MAX,
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(flat::VEIL),
        BorderColor::all(flat::LINE),
        Pickable::IGNORE,
    ))
    .with_children(|h| {
        let line = |h: &mut ChildSpawnerCommands, text: String, face: TextFont, colour: Color| {
            h.spawn((Text::new(text), face, TextColor(colour), TextLayout::justify(Justify::Center), Node { max_width: px(HUB - 52.0), ..default() }, Pickable::IGNORE));
        };
        // With nothing to choose from the title is all there is to say.
        if wheel.entries.is_empty() {
            line(h, wheel.title.clone(), theme::strong(flat::TITLE), flat::BONE);
            return;
        }
        line(h, wheel.title.to_uppercase(), theme::head(flat::BODY), flat::DIM);
        match wheel.picked() {
            Some(entry) => {
                line(h, entry.label.clone(), theme::strong(flat::TITLE), flat::BONE);
                if !entry.detail.is_empty() {
                    line(h, entry.detail.clone(), theme::font(flat::HEAD), flat::DIM);
                }
                // Dimmed on the ring, and said here in a word.
                if !entry.ready {
                    line(h, "Not now".to_string(), theme::strong(flat::BODY), flat::AMBER);
                }
            }
            None => line(h, "Point a stick".to_string(), theme::font(flat::HEAD), flat::DIM),
        }
    });
}

/// One choice, its middle `pos` from the ring's (y up).
fn tile(ring: &mut ChildSpawnerCommands, entry: &Entry, pos: Vec2, picked: bool, items: &[zombye_sim::ItemDef]) {
    let size = if picked { TILE_PICKED } else { TILE };
    let border = if picked { 3.0 } else { 1.0 };
    ring.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: px(BOX / 2.0 + pos.x - size / 2.0),
            top: px(BOX / 2.0 - pos.y - size / 2.0),
            width: px(size),
            height: px(size),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            border: UiRect::all(px(border)),
            border_radius: BorderRadius::all(px(flat::RADIUS + 4.0)),
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(flat::SOLID),
        BorderColor::all(if picked { flat::GREEN } else { flat::LINE }),
        Pickable::IGNORE,
    ))
    .with_children(|t| {
        if picked {
            // The accent's wash over the ink, as a picked row has.
            t.spawn((Node { position_type: PositionType::Absolute, left: px(0), top: px(0), right: px(0), bottom: px(0), ..default() }, BackgroundColor(flat::GREEN_SOFT), Pickable::IGNORE));
        }
        match entry.item {
            Some(i) => flat::item_icon(t, items.get(i as usize), if picked { 68.0 } else { 56.0 }, if entry.ready { 1.0 } else { 0.35 }),
            None => {
                let colour = match (entry.ready, picked) {
                    (false, _) => flat::DIM.with_alpha(0.55),
                    (true, true) => flat::BONE,
                    (true, false) => flat::DIM,
                };
                t.spawn((
                    Text::new(short(&entry.label)),
                    theme::strong(flat::SMALL),
                    TextColor(colour),
                    TextLayout::justify(Justify::Center),
                    Node { max_width: px(size - 2.0 * border - 8.0), ..default() },
                    Pickable::IGNORE,
                ));
            }
        }
    });
}

/// Under the ring: the buttons, by the names the pad in use gives them.
fn footer(ring: &mut ChildSpawnerCommands, kind: Kind, empty: bool, family: Family) {
    use crate::pad::Button as B;
    let word = |r: &mut ChildSpawnerCommands, text: &str| {
        r.spawn((Text::new(text.to_string()), theme::font(flat::BODY), TextColor(flat::DIM), TextLayout::no_wrap(), Pickable::IGNORE));
    };
    // Wider than the ring's box either side, so a long line stays centred under it.
    ring.spawn((Node { position_type: PositionType::Absolute, left: px(-200), right: px(-200), top: px(BOX + 12.0), justify_content: JustifyContent::Center, ..default() }, Pickable::IGNORE)).with_children(|f| {
        f.spawn((
            Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(7), padding: UiRect::axes(px(16), px(8)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(flat::RADIUS + 2.0)), flex_shrink: 0.0, ..default() },
            BackgroundColor(flat::VEIL),
            BorderColor::all(flat::LINE),
            Pickable::IGNORE,
        ))
        .with_children(|r| {
            if kind.latched() {
                if !empty {
                    flat::key(r, B::A.label(family));
                    word(r, "do it");
                    word(r, "\u{b7}");
                }
            } else {
                let (held, does) = if kind == Kind::Make { (B::RB, "to make") } else { (B::LB, "to use") };
                word(r, "Let go of");
                flat::key(r, held.label(family));
                word(r, if empty { "to close" } else { does });
                word(r, "\u{b7}");
            }
            flat::key(r, B::B.label(family));
            word(r, "back");
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_tile_is_straight_up_and_the_rest_go_clockwise() {
        let up = tile_pos(0, 8, RADIUS);
        assert!(up.x.abs() < 1e-3 && (up.y - RADIUS).abs() < 1e-3, "{up}");
        // A quarter of the way round is to the right, half way straight down.
        let right = tile_pos(2, 8, RADIUS);
        assert!((right.x - RADIUS).abs() < 1e-3 && right.y.abs() < 1e-3, "{right}");
        let down = tile_pos(4, 8, RADIUS);
        assert!(down.x.abs() < 1e-3 && (down.y + RADIUS).abs() < 1e-3, "{down}");
    }

    #[test]
    fn a_stick_pushed_at_a_tile_picks_it() {
        for n in 1..=crate::wheel::MAX {
            for i in 0..n {
                assert_eq!(crate::wheel::sector(tile_pos(i, n, 1.0), n), Some(i), "{i} of {n}");
            }
        }
    }

    #[test]
    fn tiles_of_a_full_wheel_do_not_touch() {
        let n = crate::wheel::MAX;
        let gap = tile_pos(0, n, RADIUS).distance(tile_pos(1, n, RADIUS));
        // The picked one is bigger than its neighbours.
        assert!(gap > (TILE + TILE_PICKED) / 2.0, "{gap}");
    }

    #[test]
    fn long_labels_are_cut_for_a_tile() {
        assert_eq!(short("Open"), "Open");
        let cut = short("Rip a T-shirt into rags and more");
        assert!(cut.ends_with("...") && cut.chars().count() <= TILE_LETTERS, "{cut}");
        assert_eq!(cut, "Rip a T-shirt...");
    }
}
