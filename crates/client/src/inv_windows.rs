//! The inventory screen's three windows (#45): Gear, You and Nearby.
//!
//! On a wide screen the three used to be columns pinned across the whole window, the gear at the
//! far left and the open container at the far right, with the game hidden behind a veil. Now each
//! is a **window of its own** in the frame's middle (between the rail, the top bar and the
//! footer), and the game shows between and round them:
//!
//! - They start **together in the middle** ([`home`]), each as tall as what is in it (and no
//!   taller than the room under it: then it scrolls).
//! - A window is **dragged by its title** and stays where it is let go ([`Windows::at`], its
//!   top-left corner in UI pixels from the middle's own corner, kept in `settings.ron` as
//!   `ClientSettings::inventory_windows`). At least [`KEEP`] pixels of it always stay on the
//!   screen and its title never leaves it ([`clamped`]). The one touched last is drawn on top.
//! - A **double-click on a title** puts that window back where it starts.
//! - **Columns, as before,** when the window is too small for the three side by side ([`fits`]:
//!   1280 x 720 is) or with `--tab columns`.
//! - **Fixed** ([`Windows::fixed`]): the windows stand where they start and can't be dragged. It
//!   is for playing with a controller (#12), whose stick moves between the three by their order,
//!   not by where a mouse left them: the controller's code sets it while the pad is the one in
//!   use. `--tab fixed` for a picture of it.
//!
//! `--panel-at gear:X,Y;you:X,Y;nearby:X,Y` places them for a picture (an offscreen render never
//! reads the settings file). Only the Inventory tab has windows: Make, Build, Health and Skills
//! are full screens on the veil.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::inventory_ui::InventoryPanel;
use crate::theme::{self, flat, font};
use crate::ui_scale::SettingsStore;

/// The windows, in order: their names in `--panel-at` and their titles.
pub const NAMES: [&str; 3] = ["gear", "you", "nearby"];
/// How wide each is (UI pixels): three gear slots across, five tiles of your things, four nearby.
pub const WIDTHS: [f32; 3] = [348.0, 602.0, 482.0];
/// Between two windows where they start.
const GAP: f32 = 12.0;
/// Round the windows where they start, and under a window that reaches the bottom.
const MARGIN: f32 = 16.0;
/// The least room the middle must have top to bottom for windows (else columns).
const MIN_AREA_H: f32 = 560.0;
/// A window is never squeezed shorter than this, however low it is dragged.
const MIN_H: f32 = 240.0;
/// The strip along a window's top that drags it.
pub const TITLE_H: f32 = 38.0;
/// This much of a window always stays on the screen.
pub const KEEP: f32 = 80.0;
/// How far a press has to move before it's a drag (UI pixels).
const DRAG_START: f32 = 4.0;
/// Two presses on a title within this long are a double-click (seconds).
const DOUBLE_CLICK_SECS: f32 = 0.35;
/// The veil over the game while the windows are up: light, so the game reads behind them.
pub const VEIL: Color = Color::srgba(0.078, 0.078, 0.078, 0.22);
/// A window's backing: nearly opaque, so its text keeps its contrast over sunlit ground.
const BACK: Color = Color::srgba(0.086, 0.086, 0.086, 0.965);
/// A title strip's tint over the backing.
const STRIP: Color = Color::srgba(0.949, 0.902, 0.788, 0.07);

/// One of the three windows (by its place in [`NAMES`]).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Win(pub usize);

/// Where the windows are, and the drag under way.
#[derive(Resource, Default, Debug)]
pub struct Windows {
    /// Where each was dragged to (`None`: where it starts).
    pub at: [Option<Vec2>; 3],
    /// They stand where they start and can't be dragged (a controller in use, #12).
    pub fixed: bool,
    /// Columns whatever the window's size (`--tab columns`).
    pub columns: bool,
    /// The frame's middle, in UI pixels ([`area`]); zero until the first frame.
    pub area: Vec2,
    /// The window drawn over the others: the one touched last.
    pub top: usize,
    /// A title pressed this frame (its observer says so; [`track`] takes it up).
    pub pressed: Option<usize>,
    grab: Option<Grab>,
    last_press: Option<(usize, f32)>,
}

#[derive(Clone, Copy, Debug)]
struct Grab {
    win: usize,
    /// From the window's corner to the pointer where it was pressed.
    off: Vec2,
    from: Vec2,
    moved: bool,
}

impl Windows {
    /// From the settings file, with `--panel-at`'s places over it and `--tab`'s words.
    pub fn new(kept: [Option<(f32, f32)>; 3], panel_at: &str, tabs: &str) -> Self {
        let mut at = kept.map(|a| a.map(Vec2::from).filter(|v| v.is_finite()));
        for part in panel_at.split(';') {
            let Some((name, xy)) = part.split_once(':') else { continue };
            let Some(k) = NAMES.iter().position(|n| *n == name.trim().to_ascii_lowercase()) else { continue };
            let xy = xy.split_once(',').and_then(|(x, y)| Some(Vec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))).filter(|v| v.is_finite());
            if let Some(xy) = xy {
                at[k] = Some(xy);
            }
        }
        let has = |word: &str| tabs.split(',').any(|t| t.trim().eq_ignore_ascii_case(word));
        Self { at, fixed: has("fixed"), columns: has("columns"), top: 1, ..default() }
    }

    /// Windows (not columns) for a middle this size.
    pub fn on(&self) -> bool {
        !self.columns && fits(self.area)
    }

    /// Each window's corner and the tallest it may be, as things stand.
    pub fn places(&self) -> [(Vec2, f32); 3] {
        places(self.area, self.at, self.fixed)
    }

    /// A window is being moved (not just pressed).
    pub fn moving(&self) -> bool {
        self.grab.is_some_and(|g| g.moved)
    }
}

/// The frame's middle for a screen of `view` UI pixels: what the rail, top bar and footer leave.
pub fn area(view: Vec2) -> Vec2 {
    Vec2::new((view.x - flat::RAIL).max(0.0), (view.y - flat::TOP_BAR - flat::FOOTER).max(0.0))
}

/// Is the middle big enough for the three windows side by side?
pub fn fits(area: Vec2) -> bool {
    area.x >= WIDTHS.iter().sum::<f32>() + 2.0 * GAP + 2.0 * MARGIN && area.y >= MIN_AREA_H
}

/// Where the windows start: side by side in the middle of `area`, a little down from its top.
pub fn home(area: Vec2) -> [Vec2; 3] {
    let all = WIDTHS.iter().sum::<f32>() + 2.0 * GAP;
    let x = ((area.x - all) * 0.5).max(MARGIN).round();
    let y = (area.y * 0.06).max(MARGIN).round();
    [Vec2::new(x, y), Vec2::new(x + WIDTHS[0] + GAP, y), Vec2::new(x + WIDTHS[0] + WIDTHS[1] + 2.0 * GAP, y)]
}

/// Where a window `width` wide asked to be at `at` goes: its title never above the middle's top
/// or under its bottom, and [`KEEP`] of it showing at either side.
pub fn clamped(at: Vec2, width: f32, area: Vec2) -> Vec2 {
    let x = at.x.clamp(KEEP - width, (area.x - KEEP).max(KEEP - width));
    let y = at.y.clamp(0.0, (area.y - KEEP).max(0.0));
    Vec2::new(x.round(), y.round())
}

/// Each window's corner and the tallest it may be (so it ends [`MARGIN`] above the middle's
/// bottom while it has [`MIN_H`] to do so in). `fixed`: where they start, whatever `at` says.
pub fn places(area: Vec2, at: [Option<Vec2>; 3], fixed: bool) -> [(Vec2, f32); 3] {
    let home = home(area);
    std::array::from_fn(|k| {
        let p = match at[k] {
            Some(at) if !fixed => clamped(at, WIDTHS[k], area),
            _ => home[k],
        };
        (p, (area.y - p.y - MARGIN).max(MIN_H))
    })
}

/// A window: its frame and title strip. What is in it is spawned as its children after the strip.
pub fn frame<'a>(c: &'a mut ChildSpawnerCommands, wins: &Windows, k: usize, title: &str, right: Option<String>) -> EntityCommands<'a> {
    let (at, max_h) = wins.places()[k];
    let mut e = c.spawn((
        Win(k),
        ZIndex(i32::from(wins.top == k)),
        Node { position_type: PositionType::Absolute, left: px(at.x), top: px(at.y), width: px(WIDTHS[k]), max_height: px(max_h), flex_direction: FlexDirection::Column, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(8)), overflow: Overflow::clip(), ..default() },
        BackgroundColor(BACK),
        BorderColor::all(flat::LINE),
    ));
    let fixed = wins.fixed;
    e.with_children(|w| {
        let mut strip = w.spawn((Node { height: px(TITLE_H), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, justify_content: JustifyContent::SpaceBetween, column_gap: px(12), padding: UiRect::axes(px(16), px(0)), border: UiRect::bottom(px(1)), flex_shrink: 0.0, overflow: Overflow::clip_x(), ..default() }, BackgroundColor(STRIP), BorderColor::all(flat::LINE)));
        strip.with_children(|s| {
            s.spawn((Text::new(title.to_uppercase()), theme::head(flat::HEAD), TextColor(flat::BONE), TextLayout::no_wrap(), Pickable::IGNORE));
            if let Some(right) = right {
                s.spawn((Text::new(right), font(flat::SMALL), TextColor(flat::DIM), TextLayout::no_wrap(), Pickable::IGNORE));
            }
        });
        if !fixed {
            strip.observe(move |_: On<PointerPress>, mut wins: ResMut<Windows>| wins.pressed = Some(k));
        }
    });
    e
}

/// Keeps the middle's size, and moves a window while its title is held: before the screen is
/// built, so a frame built this tick has the windows where the pointer has them.
#[allow(clippy::too_many_arguments)]
pub fn track(
    time: Res<Time>,
    options: Option<Res<crate::Options>>,
    ui: Option<Res<UiScale>>,
    window: Query<&Window, With<PrimaryWindow>>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    panel: Option<Res<InventoryPanel>>,
    mut wins: ResMut<Windows>,
    store: Option<ResMut<SettingsStore>>,
) {
    let scale = ui.as_ref().map_or(1.0, |u| u.0.max(0.01));
    let view = options.as_ref().and_then(|o| crate::ui_scale::view(o, &window));
    let now_area = view.map_or(Vec2::ZERO, |v| area(v / scale));
    if wins.area != now_area {
        wins.area = now_area;
    }
    let open = panel.as_ref().is_some_and(|p| p.open);
    let pressed = wins.pressed.take();
    if !open || !wins.on() || wins.fixed {
        wins.grab = None;
        return;
    }
    // The pointer from the middle's own corner, in UI pixels.
    let cursor = window.single().ok().and_then(|w| w.cursor_position()).map(|c| c / scale - Vec2::new(flat::RAIL, flat::TOP_BAR));
    let down = mouse.as_ref().is_some_and(|m| m.pressed(MouseButton::Left));
    let mut changed = false;
    if let (Some(k), Some(c)) = (pressed.filter(|k| *k < 3), cursor) {
        let now = time.elapsed_secs();
        wins.top = k;
        if wins.last_press.is_some_and(|(was, at)| was == k && now - at <= DOUBLE_CLICK_SECS) {
            // A double-click: back where it starts.
            changed = wins.at[k].is_some();
            wins.at[k] = None;
            wins.grab = None;
            wins.last_press = None;
        } else {
            let corner = wins.places()[k].0;
            wins.grab = Some(Grab { win: k, off: c - corner, from: c, moved: false });
            wins.last_press = Some((k, now));
        }
    }
    if let Some(mut g) = wins.grab {
        if !down {
            changed |= g.moved;
            wins.grab = None;
        } else if let Some(c) = cursor {
            if !g.moved && c.distance(g.from) > DRAG_START {
                g.moved = true;
            }
            if g.moved {
                let area = wins.area;
                let to = clamped(c - g.off, WIDTHS[g.win], area);
                if wins.at[g.win] != Some(to) {
                    wins.at[g.win] = Some(to);
                }
            }
            wins.grab = Some(g);
        }
    }
    if changed && let Some(mut store) = store {
        let at = wins.at.map(|a| a.map(|v| (v.x, v.y)));
        if let Err(e) = store.change(|s| s.inventory_windows = at) {
            eprintln!("inventory windows not remembered: {e}");
        }
    }
}

/// Puts each window where [`Windows`] has it (a drag moves it without the screen being built
/// again), the one touched last on top.
pub fn place(wins: Res<Windows>, mut nodes: Query<(&Win, &mut Node, &mut ZIndex)>) {
    if nodes.is_empty() {
        return;
    }
    let places = wins.places();
    for (win, mut node, mut z) in &mut nodes {
        let Some((at, max_h)) = places.get(win.0).copied() else { continue };
        let (left, top, max_height) = (px(at.x), px(at.y), px(max_h));
        if node.left != left || node.top != top || node.max_height != max_height {
            node.left = left;
            node.top = top;
            node.max_height = max_height;
        }
        let want = i32::from(wins.top == win.0);
        if z.0 != want {
            z.0 = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The middle of a 1920 x 1080 window.
    fn hd() -> Vec2 {
        area(Vec2::new(1920.0, 1080.0))
    }

    #[test]
    fn small_windows_keep_their_columns() {
        assert!(!fits(area(Vec2::new(1280.0, 720.0))), "1280 x 720: columns, as before");
        assert!(fits(area(Vec2::new(1600.0, 900.0))));
        assert!(fits(hd()));
        assert!(!fits(area(Vec2::new(1920.0, 600.0))), "too low");
        let mut w = Windows { area: hd(), ..default() };
        assert!(w.on());
        w.columns = true;
        assert!(!w.on(), "--tab columns");
        assert!(!Windows::default().on(), "before the first frame: no size yet");
    }

    #[test]
    fn they_start_together_in_the_middle() {
        // A 6K screen at UI scale 100%.
        for view in [Vec2::new(1920.0, 1080.0), Vec2::new(6016.0, 3384.0)] {
            let a = area(view);
            let h = home(a);
            let (left, right) = (h[0].x, h[2].x + WIDTHS[2]);
            assert!((left - (a.x - right)).abs() <= 1.0, "centred: {left} and {} to spare", a.x - right);
            assert!(right - left < 1500.0, "one glance wide, however wide the screen");
            assert_eq!(h[1].x, h[0].x + WIDTHS[0] + GAP);
            assert_eq!(h[2].x, h[1].x + WIDTHS[1] + GAP);
            assert!(h.iter().all(|p| p.y == h[0].y && p.y >= MARGIN));
            // Nothing dragged: that is where they are, each ending above the middle's bottom.
            let p = places(a, [None; 3], false);
            for k in 0..3 {
                assert_eq!(p[k].0, h[k]);
                assert!((p[k].0.y + p[k].1 - (a.y - MARGIN)).abs() < 0.01);
            }
        }
    }

    #[test]
    fn a_dragged_window_stays_where_it_is_put_and_on_the_screen() {
        let a = hd();
        let p = places(a, [Some(Vec2::new(40.0, 300.0)), None, None], false);
        assert_eq!(p[0].0, Vec2::new(40.0, 300.0));
        assert_eq!(p[1].0, home(a)[1], "the others where they start");
        assert_eq!(p[0].1, a.y - 300.0 - MARGIN, "shorter, the lower it is");
        // Off every side: the title stays in reach and some of it on the screen.
        assert_eq!(clamped(Vec2::new(-5000.0, -90.0), WIDTHS[0], a), Vec2::new(KEEP - WIDTHS[0], 0.0));
        assert_eq!(clamped(Vec2::new(5000.0, 5000.0), WIDTHS[2], a), Vec2::new(a.x - KEEP, a.y - KEEP));
        // Dragged right down, it is still tall enough to use.
        assert_eq!(places(a, [None, None, Some(Vec2::new(900.0, 5000.0))], false)[2].1, MIN_H);
        // A place kept from a bigger screen comes back onto this one.
        let far = places(a, [None, Some(Vec2::new(4000.0, 2000.0)), None], false)[1].0;
        assert!(far.x <= a.x - KEEP && far.y <= a.y - KEEP);
    }

    #[test]
    fn fixed_windows_stand_where_they_start() {
        let a = hd();
        let at = [Some(Vec2::new(40.0, 300.0)), Some(Vec2::new(900.0, 10.0)), None];
        assert_eq!(places(a, at, true).map(|p| p.0), home(a), "a controller's layout: never where the mouse left them");
        let w = Windows::new([None; 3], "", "rows, Fixed");
        assert!(w.fixed && !w.columns);
    }

    #[test]
    fn places_come_from_the_settings_and_the_flag() {
        let w = Windows::new([Some((10.0, 20.0)), None, Some((f32::NAN, 0.0))], "inventory:1,2; Nearby: 700, 90.5 ;you:x,3;skills:4,5", "");
        assert_eq!(w.at, [Some(Vec2::new(10.0, 20.0)), None, Some(Vec2::new(700.0, 90.5))], "the flag over the file; a broken place is none; other panels' names are theirs");
        assert!(!w.fixed && !w.columns);
    }
}
