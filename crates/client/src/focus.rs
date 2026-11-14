//! Moving round a screen's buttons with a controller (#12, stage 3).
//!
//! Every button made with `menu_skin::button` (except a faint one with nothing to do) is
//! [`Focusable`], and so is every pressable cell of the new-game screen. While the controller is
//! the one in use and the main menu, the new-game screen, the pause menu or the settings is up, one
//! of them has the focus, outlined in the menus' mustard: the d-pad or the left stick moves it to the nearest
//! button that way (as if the pointer were over it: lit, and on the new-game screen its line shown),
//! A presses it (the same press a click makes, so every button works with no change of its own),
//! the bumpers change the settings' tab or the new-game page, and B goes back (Esc, `pad.rs`).
//! The inventory's frame goes the same way (#12, stage 4): its things, slots, chips and rail tabs
//! are focusable, A is a click on what has the focus (move it across, or press the chip), Y the
//! right-click (the thing's menu, whose lines are then the focus), and the bumpers go round the
//! frame's screens. A press the pad makes is never the start of a drag ([`Focus::pad_press`]), and
//! the thing's card and menu stand by the focus, not the mouse ([`Focus::pointer_at`]).
//! Only the buttons of the screen on top count (the one with the highest `GlobalZIndex` over it).
//! When a screen is drawn again the focus goes to the button nearest where it was.
//!
//! Nothing here reaches the sim, a save or a snapshot.

use bevy::camera::NormalizedRenderTarget;
use bevy::picking::backend::HitData;
use bevy::picking::events::{Pointer, PointerOut, PointerOver, PointerPress};
use bevy::picking::pointer::{Location, PointerButton, PointerId};
use bevy::prelude::*;

use crate::menu_skin::ACCENT;
use crate::pad::{InputMode, Pad};

/// A button the pad can move to and press.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Focusable;

/// A screen's main button, or a choice that is on (the open tab): the focus starts on the first.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Lead;

/// Seconds the stick is held before the focus moves on by itself, then between steps.
const REPEAT_FIRST: f32 = 0.4;
const REPEAT_NEXT: f32 = 0.15;
/// How far over the stick goes before it moves the focus.
const STICK_STEP: f32 = 0.5;

/// Which button has the focus.
#[derive(Resource, Default, Debug)]
pub struct Focus {
    pub on: Option<Entity>,
    /// Where the focused button's middle was (physical pixels), to find the nearest after a redraw.
    last: Option<Vec2>,
    /// The screen it is on (its `GlobalZIndex`), to start again on a screen that comes up over it.
    layer: Option<i32>,
    /// The stick's way held, and when it next steps.
    held: IVec2,
    next_step: f32,
    /// The button the outline is drawn on.
    outlined: Option<Entity>,
    /// Where the focus last was on each screen under the top one, to come back to it when the
    /// screen over it goes (an item's menu over the inventory).
    under: Vec<(i32, Vec2)>,
    /// The press being sent is the pad's (set while its observers run): it is a click at once,
    /// never the start of a drag.
    pub pad_press: bool,
}

impl Focus {
    /// Where the focused button's middle is (physical pixels), while the pad has a focus: what a
    /// screen stands its card or menu by in place of the pointer.
    pub fn pointer_at(&self) -> Option<Vec2> {
        self.on.and(self.last)
    }
}

pub struct FocusPlugin;

impl Plugin for FocusPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Focus>().add_systems(Update, (navigate, outline, into_view).chain());
    }
}

/// The button to start on: the top one, then the left one (the left counts for half: the settings'
/// tabs before its Close in the corner).
pub fn first(items: &[(Entity, Vec2)]) -> Option<Entity> {
    items.iter().min_by(|a, b| (a.1.y + a.1.x * 0.5).total_cmp(&(b.1.y + b.1.x * 0.5))).map(|i| i.0)
}

/// The button nearest `from` the way `dir` goes (screen pixels, y down): the one least far along
/// that way plus twice as far across it, of those at least a pixel along. None that way: none.
pub fn next(from: Vec2, dir: Vec2, items: &[(Entity, Vec2)]) -> Option<Entity> {
    let dir = dir.normalize_or_zero();
    items
        .iter()
        .filter_map(|&(e, c)| {
            let d = c - from;
            let along = d.dot(dir);
            (along >= 1.0).then(|| (e, along + (d - dir * along).length() * 2.0))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|i| i.0)
}

/// The button nearest `to`.
pub fn nearest(to: Vec2, items: &[(Entity, Vec2)]) -> Option<Entity> {
    items.iter().min_by(|a, b| a.1.distance_squared(to).total_cmp(&b.1.distance_squared(to))).map(|i| i.0)
}

/// A stick's way as one of the four steps (x right, y up), or none in the middle.
pub fn stick_step(stick: Vec2) -> IVec2 {
    if stick.length() < STICK_STEP {
        IVec2::ZERO
    } else if stick.x.abs() > stick.y.abs() {
        IVec2::new(stick.x.signum() as i32, 0)
    } else {
        IVec2::new(0, stick.y.signum() as i32)
    }
}

#[allow(clippy::too_many_arguments)]
fn navigate(
    mut commands: Commands,
    mut focus: ResMut<Focus>,
    mode: Option<Res<InputMode>>,
    pad: Option<Res<Pad>>,
    pause: Option<Res<crate::pause_menu::PauseMenu>>,
    menu: Option<Res<crate::main_menu::MenuLayer>>,
    front: Option<Res<crate::new_game::Front>>,
    mut new_game: Option<ResMut<crate::new_game::Screen>>,
    mut settings: Option<ResMut<crate::settings_ui::SettingsUi>>,
    (inventory, item_menu, mut tab_step): (Option<Res<crate::inventory_ui::InventoryPanel>>, Option<Res<crate::inventory_ui::ItemMenu>>, Option<ResMut<crate::inventory_ui::TabStep>>),
    time: Res<Time>,
    buttons: Query<(Entity, &ComputedNode, &UiGlobalTransform, &InheritedVisibility, Has<Lead>), With<Focusable>>,
    parents: Query<&ChildOf>,
    z: Query<&GlobalZIndex>,
) {
    focus.pad_press = false;
    let settings_open = settings.as_ref().is_some_and(|s| s.open);
    let front_up = front.as_ref().is_some_and(|f| f.up());
    let inventory_open = inventory.as_ref().is_some_and(|i| i.open) || item_menu.as_ref().is_some_and(|m| m.is_up());
    let up = pause.as_ref().is_some_and(|p| p.open) || settings_open || menu.as_ref().is_some_and(|m| m.up) || front_up || inventory_open;
    if !up || !mode.as_ref().is_some_and(|m| m.pad) {
        if focus.on.is_some() || focus.layer.is_some() {
            *focus = Focus { outlined: focus.outlined, ..default() };
        }
        return;
    }
    // The screen a button is on: the highest GlobalZIndex over it.
    let layer_of = |mut e: Entity| {
        let mut best = None;
        loop {
            if let Ok(GlobalZIndex(i)) = z.get(e) {
                best = Some(best.map_or(*i, |b: i32| b.max(*i)));
            }
            match parents.get(e) {
                Ok(p) => e = p.parent(),
                Err(_) => return best.unwrap_or(0),
            }
        }
    };
    let all: Vec<(Entity, Vec2, i32, bool)> = buttons
        .iter()
        .filter(|(_, n, _, v, _)| v.get() && n.size().min_element() > 0.0)
        .map(|(e, _, t, _, lead)| (e, t.translation, layer_of(e), lead))
        .collect();
    let Some(top) = all.iter().map(|b| b.2).max() else { return };
    let items: Vec<(Entity, Vec2)> = all.iter().filter(|b| b.2 == top).map(|b| (b.0, b.1)).collect();
    let leads: Vec<(Entity, Vec2)> = all.iter().filter(|b| b.2 == top && b.3).map(|b| (b.0, b.1)).collect();
    let start = first(&leads).or_else(|| first(&items));
    // Where the focus is: still on a button of the top screen, else nearest where it was, else the first.
    let here = focus.on.and_then(|e| items.iter().find(|i| i.0 == e).copied());
    // A screen come up over this one: remember where the focus was here. Back on a screen that
    // was under: where it was on it.
    if let Some(was) = focus.layer.filter(|l| *l != top) {
        if was < top
            && let Some(last) = focus.last
        {
            focus.under.retain(|u| u.0 < was);
            focus.under.push((was, last));
        } else {
            let back = focus.under.iter().find(|u| u.0 == top).map(|u| u.1);
            focus.under.retain(|u| u.0 < top);
            focus.last = back;
            focus.layer = back.map(|_| top);
        }
    }
    let on = match here {
        Some(h) => Some(h),
        None if focus.layer == Some(top) => focus.last.and_then(|l| nearest(l, &items)).or(start).and_then(|e| items.iter().find(|i| i.0 == e).copied()),
        None => start.and_then(|e| items.iter().find(|i| i.0 == e).copied()),
    };
    focus.layer = Some(top);
    let Some((mut on, mut at)) = on else {
        focus.on = None;
        return;
    };
    let pad = pad.map(|p| *p).unwrap_or_default();
    // A step: the d-pad pressed, or the stick held (at once, then repeating).
    let now = time.elapsed_secs();
    let held = stick_step(pad.nav_stick);
    let mut step = pad.nav_step;
    if held != focus.held {
        focus.held = held;
        focus.next_step = now + REPEAT_FIRST;
        if held != IVec2::ZERO {
            step = held;
        }
    } else if held != IVec2::ZERO && now >= focus.next_step {
        focus.next_step = now + REPEAT_NEXT;
        step = held;
    }
    if step != IVec2::ZERO
        && let Some(e) = next(at, Vec2::new(step.x as f32, -step.y as f32), &items)
        && let Some(&(e, c)) = items.iter().find(|i| i.0 == e)
    {
        (on, at) = (e, c);
    }
    // The pointer's way in and out, as if it had moved there: the hover look, and the new-game
    // screen's line about the row pointed at.
    if focus.on != Some(on) {
        if let Some(old) = focus.on.filter(|o| items.iter().any(|i| i.0 == *o)) {
            commands.trigger(PointerOut { entity: old, pointer: pointer(at), hit: hit() });
        }
        commands.trigger(PointerOver { entity: on, pointer: pointer(at), hit: hit() });
    }
    focus.on = Some(on);
    focus.last = Some(at);
    if pad.nav_tab != 0 {
        if settings_open && let Some(s) = settings.as_mut() {
            s.step_tab(pad.nav_tab);
        } else if front_up && let Some(s) = new_game.as_mut() {
            s.step_tab(pad.nav_tab);
        } else if inventory_open && let Some(s) = tab_step.as_mut() {
            s.0 = pad.nav_tab;
        }
    }
    if pad.confirm || pad.alt {
        focus.pad_press = true;
        commands.trigger(press(on, at, if pad.confirm { PointerButton::Primary } else { PointerButton::Secondary }));
    }
}

/// The press a click of `button` on `entity` makes, as if the pointer were at `at`.
fn press(entity: Entity, at: Vec2, button: PointerButton) -> PointerPress {
    PointerPress { entity, pointer: pointer(at), button, hit: hit(), count: 1 }
}

/// The mouse's pointer, as if at `at`.
fn pointer(at: Vec2) -> Pointer {
    Pointer::new(PointerId::Mouse, Location { target: NormalizedRenderTarget::None { width: 1, height: 1 }, position: at })
}

fn hit() -> HitData {
    HitData::new(Entity::PLACEHOLDER, 0.0, None, None)
}

/// The mustard outline round the button with the focus (and none round the one that had it).
fn outline(mut commands: Commands, mut focus: ResMut<Focus>, exists: Query<(), With<Focusable>>) {
    if focus.outlined == focus.on {
        return;
    }
    // (Either may be gone by the time the commands run: a press that leaves its screen, Start on
    // the new-game screen, despawns the button it was made on. Hence the `try_`s.)
    if let Some(old) = focus.outlined
        && exists.get(old).is_ok()
    {
        commands.entity(old).try_remove::<Outline>();
    }
    if let Some(new) = focus.on.filter(|e| exists.get(*e).is_ok()) {
        commands.entity(new).try_insert(Outline::new(px(2), px(2), ACCENT));
    }
    focus.outlined = focus.on;
}

/// The focused button scrolled into sight, in whatever scrolls round it.
fn into_view(focus: Res<Focus>, nodes: Query<(&ComputedNode, &UiGlobalTransform)>, parents: Query<&ChildOf>, mut scrolls: Query<(&ComputedNode, &UiGlobalTransform, &mut ScrollPosition)>) {
    let Some(on) = focus.on.filter(|_| focus.is_changed()) else { return };
    let Ok((button, at)) = nodes.get(on) else { return };
    let mut e = on;
    while let Ok(p) = parents.get(e) {
        e = p.parent();
        let Ok((node, middle, mut pos)) = scrolls.get_mut(e) else { continue };
        if node.content_size.y <= node.size.y {
            continue;
        }
        // Physical pixels: how far the button's top is above the view's, or its foot below.
        let margin = 12.0 / node.inverse_scale_factor();
        let (top, foot) = (at.translation.y - button.size().y / 2.0 - margin, at.translation.y + button.size().y / 2.0 + margin);
        let (view_top, view_foot) = (middle.translation.y - node.size.y / 2.0, middle.translation.y + node.size.y / 2.0);
        let by = if top < view_top { top - view_top } else if foot > view_foot { foot - view_foot } else { 0.0 };
        if by != 0.0 {
            let max = (node.content_size.y - node.size.y) * node.inverse_scale_factor();
            pos.y = (pos.y + by * node.inverse_scale_factor()).clamp(0.0, max.max(0.0));
        }
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(i: u32) -> Entity {
        Entity::from_raw_u32(i).unwrap()
    }

    /// A column of three and a row beside the middle one: the steps go where the eye expects.
    #[test]
    fn steps_go_to_the_nearest_button_that_way() {
        let items = [(e(1), Vec2::new(100.0, 100.0)), (e(2), Vec2::new(100.0, 150.0)), (e(3), Vec2::new(100.0, 200.0)), (e(4), Vec2::new(300.0, 160.0)), (e(5), Vec2::new(300.0, 400.0))];
        let down = Vec2::new(0.0, 1.0);
        assert_eq!(next(Vec2::new(100.0, 100.0), down, &items), Some(e(2)));
        assert_eq!(next(Vec2::new(100.0, 150.0), down, &items), Some(e(3)));
        assert_eq!(next(Vec2::new(100.0, 150.0), Vec2::X, &items), Some(e(4)), "right goes to the one level with it, not the far one");
        assert_eq!(next(Vec2::new(300.0, 160.0), -Vec2::X, &items), Some(e(2)));
        assert_eq!(next(Vec2::new(100.0, 100.0), -down, &items), None, "nothing above the top");
        assert_eq!(first(&items), Some(e(1)));
        assert_eq!(nearest(Vec2::new(290.0, 170.0), &items), Some(e(4)));
    }

    #[test]
    fn the_stick_steps_one_way_at_a_time() {
        assert_eq!(stick_step(Vec2::new(0.2, 0.1)), IVec2::ZERO);
        assert_eq!(stick_step(Vec2::new(0.9, 0.3)), IVec2::X);
        assert_eq!(stick_step(Vec2::new(0.1, -0.8)), IVec2::new(0, -1));
    }

    /// The settings' tabs down the left come before its Close in the top corner.
    #[test]
    fn a_screen_starts_on_its_top_left_button() {
        let items = [(e(1), Vec2::new(1100.0, 80.0)), (e(2), Vec2::new(300.0, 160.0)), (e(3), Vec2::new(300.0, 210.0))];
        assert_eq!(first(&items), Some(e(2)));
    }
}
