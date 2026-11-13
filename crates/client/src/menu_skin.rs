//! The flat chrome the menus are drawn in: the main menu, the pause menu and the settings screen.
//!
//! The brand's look (docs/ui-design.md, The screens before the game): solid
//! warm charcoal, one-pixel hairlines, small radii, cream text and mustard as the one accent (the
//! open tab, what is picked, the one main button of a screen). Titles and buttons are set in the
//! brand's Bebas Neue (`theme::brand`), text in Barlow (`theme::font`, `strong`). Everything here is plain Bevy UI
//! (a fill, a border, a radius), so it costs a quad a node and needs no texture.
//!
//! These are the menus' pieces only. When `theme.rs` has flat components for every panel, the menus
//! take theirs and this file goes.

use bevy::prelude::*;

use crate::theme::{self, BODY, SMALL};

// The brand's colours (docs/ui-design.md, The screens before the game; the brand guide's
// palette): warm charcoal ground, cream text, mustard the one accent. The game's own panels keep
// the flat near-black, bone and zombie green.
/// Warm charcoal (#2B2421): a menu's ground and its panels, and the text on a mustard button.
pub const GROUND: Color = Color::srgb(0.169, 0.141, 0.129);
/// Cream (#E9D8BB): text and values.
pub const BONE: Color = Color::srgb(0.914, 0.847, 0.733);
/// Secondary text: what a setting does, hints (8:1 on the ground).
pub const DIM: Color = Color::srgb(0.804, 0.737, 0.627);
/// What needn't be read: a control with nothing to do.
pub const FAINT: Color = Color::srgb(0.592, 0.537, 0.478);
/// Hairlines: cream at 14%.
pub const LINE: Color = Color::srgba(0.914, 0.847, 0.733, 0.14);
/// The fill of a button or chip at rest.
pub const CELL: Color = Color::srgba(0.914, 0.847, 0.733, 0.05);
const CELL_HOVER: Color = Color::srgba(0.914, 0.847, 0.733, 0.1);
const LINE_HOVER: Color = Color::srgba(0.914, 0.847, 0.733, 0.5);
/// The accent, mustard (#E0A646): this one, the open tab, the main button.
pub const ACCENT: Color = Color::srgb(0.878, 0.651, 0.275);
const ACCENT_HOVER: Color = Color::srgb(0.922, 0.722, 0.388);
/// Rust (#D45B2C): the brand's second colour, for a line that should catch the eye.
pub const RUST: Color = Color::srgb(0.831, 0.357, 0.173);
/// The veil between a menu's panel and the game behind it.
pub const VEIL: Color = Color::srgba(0.09, 0.07, 0.06, 0.74);

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
        Look::Primary | Look::Chip(true) => (ACCENT, ACCENT, GROUND),
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
            b.spawn((Text::new(label.to_uppercase()), theme::brand(size + 3.0), TextColor(ink), TextLayout::no_wrap(), Pickable::IGNORE));
        }
        if let Some(detail) = detail {
            b.spawn((Text::new(detail.to_string()), theme::font(SMALL), TextColor(if fill == ACCENT { GROUND } else { DIM }), TextLayout::no_wrap(), Pickable::IGNORE));
        }
    });
    let over = match look {
        Look::Primary => Some((ACCENT_HOVER, ACCENT_HOVER)),
        Look::Plain | Look::Row | Look::Chip(false) => Some((CELL_HOVER, LINE_HOVER)),
        Look::Chip(true) | Look::Disabled => None,
    };
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
    parent.spawn((Text::new(text.to_uppercase()), theme::brand(TITLE + 6.0), TextColor(BONE), TextLayout::no_wrap(), Pickable::IGNORE));
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
    fn text_reads_on_the_ground_and_on_the_accent() {
        assert!(contrast(BONE, GROUND) >= 7.0);
        assert!(contrast(DIM, GROUND) >= 7.0);
        // A mustard button's label is the ground's colour.
        assert!(contrast(GROUND, ACCENT) >= 7.0);
        assert!(contrast(GROUND, ACCENT_HOVER) >= 7.0);
    }
}
