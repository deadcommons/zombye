//! The on-screen keyboard (#12, stage 3): typing with a controller.
//!
//! While the controller is the one in use and a line is being typed (the chat line, the
//! inventory's search, a field of the server browser), a keyboard comes up at the foot of the
//! screen. The d-pad or the left stick moves over its keys, A types the one picked, X deletes, Y
//! is a space, LB shifts, Menu is Enter (done) and B is Esc (cancel). It types by writing the same
//! keyboard messages a real keyboard does, so every text field takes it with no change of its own;
//! their key codes are ones no keyboard has (`KeyCode::Unidentified`), so no game key is pressed
//! and the pad stays the one in use. `--osk` shows it from the start, for pictures.
//!
//! Nothing here reaches the sim, a save or a snapshot.

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKeyCode};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::menu_skin::{self as skin, ACCENT, BONE, CELL, GROUND, LINE};
use crate::pad::{InputMode, dead_zone};
use crate::theme;

/// The letter rows; the last row is [`ROW_KEYS`].
pub const ROWS: [&str; 4] = ["1234567890", "qwertyuiop", "asdfghjkl'", "zxcvbnm,.?"];
/// Shifted, the same places.
pub const SHIFTED: [&str; 4] = ["!@#$%&*()-", "QWERTYUIOP", "ASDFGHJKL\"", "ZXCVBNM;:/"];
/// The bottom row: each key and the columns it spans (of ten).
pub const ROW_KEYS: [(OskKey, usize, usize); 4] = [(OskKey::Shift, 0, 1), (OskKey::Space, 2, 6), (OskKey::Back, 7, 8), (OskKey::Done, 9, 9)];
const COLS: i32 = 10;
const LAST_ROW: i32 = ROWS.len() as i32;

const REPEAT_FIRST: f32 = 0.4;
const REPEAT_NEXT: f32 = 0.12;
const KEY: f32 = 46.0;
const GAP: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OskKey {
    Char(char),
    Shift,
    Space,
    Back,
    Done,
}

/// The key at column `col` of row `row` (row 0 the top).
pub fn key_at(at: IVec2, shift: bool) -> OskKey {
    let (col, row) = (at.x.clamp(0, COLS - 1) as usize, at.y.clamp(0, LAST_ROW));
    if row == LAST_ROW {
        return ROW_KEYS.iter().find(|k| (k.1..=k.2).contains(&col)).map_or(OskKey::Done, |k| k.0);
    }
    let rows = if shift { SHIFTED } else { ROWS };
    OskKey::Char(rows[row as usize].chars().nth(col).unwrap_or(' '))
}

/// One step from `at` (x right, y down the rows), round the ends. On the bottom row a step across
/// goes to the next key's first column, whatever its width.
pub fn step(at: IVec2, by: IVec2) -> IVec2 {
    let mut at = at;
    if by.y != 0 {
        at.y = (at.y + by.y).rem_euclid(LAST_ROW + 1);
    }
    if by.x != 0 {
        if at.y == LAST_ROW {
            let i = ROW_KEYS.iter().position(|k| (k.1..=k.2).contains(&(at.x as usize))).unwrap_or(0) as i32;
            let n = ROW_KEYS.len() as i32;
            at.x = ROW_KEYS[(i + by.x).rem_euclid(n) as usize].1 as i32;
        } else {
            at.x = (at.x + by.x).rem_euclid(COLS);
        }
    }
    at
}

/// The keyboard: where its cursor is, shifted or not.
#[derive(Resource, Default, Debug)]
pub struct Osk {
    pub at: IVec2,
    pub shift: bool,
    /// Up now.
    up: bool,
    /// Every button let go since it came up (the hold that opened the chat doesn't type).
    armed: bool,
    held: IVec2,
    next_step: f32,
    /// What the panel shows (redrawn when it changes).
    shown: Option<(bool, IVec2, bool)>,
}

pub struct OskPlugin;

impl Plugin for OskPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Osk>().add_systems(Startup, spawn).add_systems(Update, (read, draw).chain());
    }
}

#[derive(Component)]
struct Root;

fn spawn(mut commands: Commands) {
    commands.spawn((
        Root,
        Node { position_type: PositionType::Absolute, bottom: px(150), left: px(0), right: px(0), justify_content: JustifyContent::Center, display: Display::None, ..default() },
        GlobalZIndex(40),
        Pickable::IGNORE,
    ));
}

/// The pad's way held as a step (x right, y down the rows).
fn way(g: &Gamepad) -> IVec2 {
    let s = dead_zone(g.left_stick());
    let pressed = |b: GamepadButton| g.pressed(b) as i32;
    let d = IVec2::new(pressed(GamepadButton::DPadRight) - pressed(GamepadButton::DPadLeft), pressed(GamepadButton::DPadDown) - pressed(GamepadButton::DPadUp));
    if d != IVec2::ZERO {
        d
    } else {
        let s = crate::focus::stick_step(s);
        IVec2::new(s.x, -s.y)
    }
}

#[allow(clippy::too_many_arguments)]
fn read(
    mut osk: ResMut<Osk>,
    options: Res<crate::Options>,
    mode: Option<Res<InputMode>>,
    chat: Option<Res<crate::chat::ChatEntry>>,
    find: Option<Res<crate::inventory_ui::Find>>,
    browser: Option<Res<crate::browser_ui::BrowserUi>>,
    gamepads: Query<&Gamepad>,
    window: Query<Entity, With<PrimaryWindow>>,
    time: Res<Time>,
    mut out: MessageWriter<KeyboardInput>,
) {
    let typing = chat.is_some_and(|c| c.typing) || find.is_some_and(|f| f.typing) || browser.is_some_and(|b| b.typing());
    let pad = mode.as_ref().is_some_and(|m| m.pad);
    let up = (pad && typing) || options.osk;
    if up != osk.up {
        *osk = Osk { up, shown: osk.shown, ..default() };
    }
    let Some(g) = mode.and_then(|m| m.gamepad()).and_then(|e| gamepads.get(e).ok()).or_else(|| gamepads.iter().next()) else { return };
    if !up || !typing {
        return;
    }
    if !osk.armed {
        osk.armed = g.get_pressed().next().is_none() && dead_zone(g.left_stick()) == Vec2::ZERO;
        return;
    }
    let now = time.elapsed_secs();
    let held = way(g);
    if held != osk.held {
        osk.held = held;
        osk.next_step = now + REPEAT_FIRST;
        if held != IVec2::ZERO {
            osk.at = step(osk.at, held);
        }
    } else if held != IVec2::ZERO && now >= osk.next_step {
        osk.next_step = now + REPEAT_NEXT;
        osk.at = step(osk.at, held);
    }
    let window = window.single().unwrap_or(Entity::PLACEHOLDER);
    let mut send = |logical: Key| {
        for state in [ButtonState::Pressed, ButtonState::Released] {
            out.write(KeyboardInput { key_code: KeyCode::Unidentified(NativeKeyCode::Unidentified), logical_key: logical.clone(), state, text: None, repeat: false, window });
        }
    };
    let just = |b: GamepadButton| g.just_pressed(b);
    let mut key = None;
    if just(GamepadButton::South) {
        key = Some(key_at(osk.at, osk.shift));
    }
    if just(GamepadButton::West) {
        key = Some(OskKey::Back);
    }
    if just(GamepadButton::North) {
        key = Some(OskKey::Space);
    }
    if just(GamepadButton::LeftTrigger) {
        key = Some(OskKey::Shift);
    }
    if just(GamepadButton::Start) {
        key = Some(OskKey::Done);
    }
    match key {
        Some(OskKey::Char(c)) => {
            send(Key::Character(c.to_string().into()));
            osk.shift = false;
        }
        Some(OskKey::Shift) => osk.shift = !osk.shift,
        Some(OskKey::Space) => send(Key::Space),
        Some(OskKey::Back) => send(Key::Backspace),
        Some(OskKey::Done) => send(Key::Enter),
        None => {}
    }
    if just(GamepadButton::East) {
        send(Key::Escape);
    }
}

fn draw(mut commands: Commands, mut osk: ResMut<Osk>, mut root: Query<(Entity, &mut Node), With<Root>>) {
    let want = (osk.up, osk.at, osk.shift);
    if osk.shown == Some(want) {
        return;
    }
    osk.shown = Some(want);
    let Ok((root, mut node)) = root.single_mut() else { return };
    node.display = if osk.up { Display::Flex } else { Display::None };
    commands.entity(root).despawn_related::<Children>();
    if !osk.up {
        return;
    }
    let (at, shift) = (osk.at, osk.shift);
    commands.entity(root).with_children(|c| {
        c.spawn((
            Node { flex_direction: FlexDirection::Column, row_gap: px(GAP), padding: UiRect::all(px(14)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(8)), ..default() },
            skin::panel(),
            Pickable::IGNORE,
        ))
        .with_children(|p| {
            for row in 0..=LAST_ROW {
                p.spawn((Node { column_gap: px(GAP), ..default() }, Pickable::IGNORE)).with_children(|r| {
                    let keys: Vec<(OskKey, usize, usize)> = if row == LAST_ROW { ROW_KEYS.to_vec() } else { (0..COLS as usize).map(|i| (key_at(IVec2::new(i as i32, row), shift), i, i)).collect() };
                    for (k, from, to) in keys {
                        let picked = at.y == row && (from..=to).contains(&(at.x as usize));
                        let label = match k {
                            OskKey::Char(c) => c.to_string(),
                            OskKey::Shift => "Shift".into(),
                            OskKey::Space => "Space".into(),
                            OskKey::Back => "Delete".into(),
                            OskKey::Done => "Done".into(),
                        };
                        let span = (to - from + 1) as f32;
                        let on = k == OskKey::Shift && shift;
                        let (fill, ink) = if picked { (ACCENT, GROUND) } else if on { (LINE, BONE) } else { (CELL, BONE) };
                        r.spawn((
                            Node { width: px(KEY * span + GAP * (span - 1.0)), height: px(KEY), align_items: AlignItems::Center, justify_content: JustifyContent::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(6)), ..default() },
                            BackgroundColor(fill),
                            BorderColor::all(if picked { ACCENT } else { LINE }),
                            Pickable::IGNORE,
                        ))
                        .with_children(|b| {
                            let size = if matches!(k, OskKey::Char(_)) { 20.0 } else { theme::SMALL };
                            b.spawn((Text::new(label), theme::strong(size), TextColor(ink), TextLayout::no_wrap(), Pickable::IGNORE));
                        });
                    }
                });
            }
            p.spawn((Node { column_gap: px(18), margin: UiRect::top(px(4)), justify_content: JustifyContent::Center, ..default() }, Pickable::IGNORE)).with_children(|h| {
                use crate::pad::Button as B;
                for (k, does) in [(B::A, "type"), (B::X, "delete"), (B::Y, "space"), (B::LB, "shift"), (B::Menu, "done"), (B::B, "cancel")] {
                    skin::keycap_as_is(h, &crate::pad::button(k), does);
                }
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_has_ten_keys_both_ways() {
        for (a, b) in ROWS.iter().zip(SHIFTED) {
            assert_eq!(a.chars().count(), COLS as usize);
            assert_eq!(b.chars().count(), COLS as usize);
        }
        let mut cols: Vec<usize> = ROW_KEYS.iter().flat_map(|k| k.1..=k.2).collect();
        cols.dedup();
        assert_eq!(cols, (0..COLS as usize).collect::<Vec<_>>(), "the bottom row covers every column once");
    }

    #[test]
    fn keys_are_where_they_are_drawn() {
        assert_eq!(key_at(IVec2::new(0, 1), false), OskKey::Char('q'));
        assert_eq!(key_at(IVec2::new(0, 1), true), OskKey::Char('Q'));
        assert_eq!(key_at(IVec2::new(4, 4), false), OskKey::Space);
        assert_eq!(key_at(IVec2::new(9, 4), false), OskKey::Done);
    }

    #[test]
    fn steps_go_round_and_jump_the_wide_keys() {
        assert_eq!(step(IVec2::new(9, 1), IVec2::X), IVec2::new(0, 1));
        assert_eq!(step(IVec2::new(0, 0), IVec2::new(0, -1)), IVec2::new(0, 4));
        assert_eq!(step(IVec2::new(1, 4), IVec2::X), IVec2::new(2, 4), "Shift to Space");
        assert_eq!(step(IVec2::new(2, 4), IVec2::X), IVec2::new(7, 4), "Space to Delete");
        assert_eq!(step(IVec2::new(9, 4), IVec2::X), IVec2::new(0, 4), "Done round to Shift");
        assert_eq!(step(IVec2::new(5, 4), IVec2::new(0, 1)), IVec2::new(5, 0), "down from Space to the top row");
    }
}
