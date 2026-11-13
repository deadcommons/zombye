//! The flat chrome the menus are drawn in: the main menu, the pause menu and the settings screen.
//!
//! Quiet, flat and dark: solid fills, one-pixel hairlines, small radii, bone-white text and zombie
//! green as the one accent (the open tab, what is picked, the one main button of a screen). The
//! typefaces are the skin's own (`theme::font`, `strong`, `head`). Everything here is plain Bevy UI
//! (a fill, a border, a radius), so it costs a quad a node and needs no texture.
//!
//! These are the menus' pieces only. When `theme.rs` has flat components for every panel, the menus
//! take theirs and this file goes.

use bevy::prelude::*;

use crate::theme::{self, BODY, SMALL};

/// Near-black: a menu's ground and its panels.
pub const GROUND: Color = Color::srgb(0.078, 0.078, 0.078);
/// Text and values.
pub const BONE: Color = Color::srgb(0.949, 0.902, 0.788);
/// Secondary text: what a setting does, hints.
pub const DIM: Color = Color::srgb(0.725, 0.69, 0.604);
/// What needn't be read: a control with nothing to do.
pub const FAINT: Color = Color::srgb(0.49, 0.467, 0.416);
/// Hairlines.
pub const LINE: Color = Color::srgba(0.949, 0.902, 0.788, 0.11);
/// The fill of a button or chip at rest.
pub const CELL: Color = Color::srgba(0.949, 0.902, 0.788, 0.045);
const CELL_HOVER: Color = Color::srgba(0.949, 0.902, 0.788, 0.09);
const LINE_HOVER: Color = Color::srgba(0.949, 0.902, 0.788, 0.5);
/// The accent: this one.
pub const GREEN: Color = Color::srgb(0.553, 0.69, 0.42);
const GREEN_HOVER: Color = Color::srgb(0.64, 0.77, 0.51);
/// The veil between a menu's panel and the game behind it.
pub const VEIL: Color = Color::srgba(0.03, 0.03, 0.03, 0.72);

/// A screen's title, in capitals.
pub const TITLE: f32 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// The one main button of a screen: green.
    Primary,
    /// Any other button.
    Plain,
    /// One line of a list (a save): its label as written, not in capitals.
    Row,
    /// A choice that is on (green) or off: a tab, a setting's value.
    Chip(bool),
    /// Nothing to do: there, but faint, and it takes no hover.
    Disabled,
}

/// A panel standing over a veil or the ground: solid, a hairline round it.
pub fn panel() -> (BackgroundColor, BorderColor) {
    (BackgroundColor(GROUND), BorderColor::all(LINE))
}

/// A button: `label`, and `detail` (if any) dimmer at its right end. `width` `Val::Auto` makes it as
/// wide as its label.
pub fn button<'a>(parent: &'a mut ChildSpawnerCommands, label: &str, detail: Option<&str>, look: Look, width: Val) -> EntityCommands<'a> {
    let (fill, line, ink) = match look {
        Look::Primary | Look::Chip(true) => (GREEN, GREEN, GROUND),
        Look::Plain | Look::Row | Look::Chip(false) => (CELL, LINE, BONE),
        Look::Disabled => (Color::NONE, LINE, FAINT),
    };
    let (height, size) = if matches!(look, Look::Chip(_) | Look::Disabled) { (32.0, 15.0) } else { (38.0, 17.0) };
    let wide = width != Val::Auto;
    let mut e = parent.spawn((
        Node {
            width,
            height: px(height),
            min_width: px(height),
            padding: UiRect::horizontal(px(if wide { 16.0 } else { 12.0 })),
            column_gap: px(14),
            align_items: AlignItems::Center,
            justify_content: if detail.is_some() { JustifyContent::SpaceBetween } else if wide { JustifyContent::FlexStart } else { JustifyContent::Center },
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(6)),
            flex_shrink: 0.0,
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(fill),
        BorderColor::all(line),
    ));
    e.with_children(|b| {
        if look == Look::Row {
            b.spawn((Text::new(label.to_string()), theme::strong(BODY), TextColor(ink), TextLayout::no_wrap(), Pickable::IGNORE));
        } else {
            b.spawn((Text::new(label.to_uppercase()), theme::head(size), TextColor(ink), TextLayout::no_wrap(), Pickable::IGNORE));
        }
        if let Some(detail) = detail {
            b.spawn((Text::new(detail.to_string()), theme::font(SMALL), TextColor(if fill == GREEN { GROUND } else { DIM }), TextLayout::no_wrap(), Pickable::IGNORE));
        }
    });
    let over = match look {
        Look::Primary => Some((GREEN_HOVER, GREEN_HOVER)),
        Look::Plain | Look::Row | Look::Chip(false) => Some((CELL_HOVER, LINE_HOVER)),
        Look::Chip(true) | Look::Disabled => None,
    };
    // A controller can move to it and press it (`focus.rs`).
    if look != Look::Disabled {
        e.insert(crate::focus::Focusable);
    }
    if let Some((over_fill, over_line)) = over {
        e.observe(move |ev: On<PointerOver>, mut q: Query<(&mut BackgroundColor, &mut BorderColor)>| {
            if let Ok((mut bg, mut border)) = q.get_mut(ev.entity) {
                (bg.0, *border) = (over_fill, BorderColor::all(over_line));
            }
        })
        .observe(move |ev: On<PointerOut>, mut q: Query<(&mut BackgroundColor, &mut BorderColor)>| {
            if let Ok((mut bg, mut border)) = q.get_mut(ev.entity) {
                (bg.0, *border) = (fill, BorderColor::all(line));
            }
        });
    }
    e
}

/// A key to press and what it does.
pub fn keycap(parent: &mut ChildSpawnerCommands, key: &str, does: &str) {
    parent.spawn((Node { align_items: AlignItems::Center, column_gap: px(8), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|row| {
        row.spawn((Node { height: px(24), min_width: px(24), padding: UiRect::horizontal(px(7)), align_items: AlignItems::Center, justify_content: JustifyContent::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(5)), ..default() }, BorderColor::all(LINE_HOVER), Pickable::IGNORE))
            .with_children(|k| {
                k.spawn((Text::new(key.to_string()), theme::strong(SMALL), TextColor(BONE), TextLayout::no_wrap(), Pickable::IGNORE));
            });
        row.spawn((Text::new(does.to_string()), theme::font(SMALL), TextColor(DIM), TextLayout::no_wrap(), Pickable::IGNORE));
    });
}

/// A hairline across its parent.
pub fn rule(parent: &mut ChildSpawnerCommands) {
    parent.spawn((Node { height: px(1), align_self: AlignSelf::Stretch, flex_shrink: 0.0, ..default() }, BackgroundColor(LINE), Pickable::IGNORE));
}

/// A hairline down its parent, between two columns.
pub fn divider(parent: &mut ChildSpawnerCommands) {
    parent.spawn((Node { width: px(1), align_self: AlignSelf::Stretch, flex_shrink: 0.0, ..default() }, BackgroundColor(LINE), Pickable::IGNORE));
}

/// A screen's title.
pub fn title(parent: &mut ChildSpawnerCommands, text: &str) {
    parent.spawn((Text::new(text.to_uppercase()), theme::head(TITLE), TextColor(BONE), TextLayout::no_wrap(), Pickable::IGNORE));
}

/// Empty room that pushes what follows to the far end of a row.
pub fn spacer(parent: &mut ChildSpawnerCommands) {
    parent.spawn((Node { flex_grow: 1.0, ..default() }, Pickable::IGNORE));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative luminance of an sRGB colour (WCAG).
    fn luminance(c: Color) -> f32 {
        let c = c.to_linear();
        0.2126 * c.red + 0.7152 * c.green + 0.0722 * c.blue
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn text_reads_on_the_ground_and_on_green() {
        assert!(contrast(BONE, GROUND) >= 7.0);
        assert!(contrast(DIM, GROUND) >= 7.0);
        // A green button's label is the ground's colour.
        assert!(contrast(GROUND, GREEN) >= 7.0);
    }
}
