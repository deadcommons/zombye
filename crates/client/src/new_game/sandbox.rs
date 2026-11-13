//! The new-game screen's Sandbox page: every tuning number the world has, by name.
//!
//! On the left, the groups `assets/data/sandbox.ron` names (Zombies: senses, Survivors, ...), or
//! with **Everything** every section of the world's tuning (`zombye_sim::tune::SECTIONS`). On the
//! right, what is in the one picked: each number or switch with arrows to step it, as it stands
//! with the preset and the page's other changes on (`Screen::tuned`). A change goes into the
//! setup's `values` by path (`Setup::values`), so it is saved with the world from its first tick,
//! and on a server is the same for everyone; a value stepped back to what the preset gives is no
//! change. Named numbers step by their file's step and stay in its range; the rest step by about
//! a twentieth of what they are (`nudge`). Every change is checked as the world will check it
//! (`tune::set`), and refused with the reason.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use zombye_sim::difficulty::SandboxOption;
use zombye_sim::tune::{self, Leaf, Value};

use super::*;

/// Where the page is.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct State {
    /// Every section, not the named groups.
    pub everything: bool,
    /// The group (or section) open.
    pub view: usize,
    /// How far down the two lists are, in UI pixels (not a redraw: the wheel sets them).
    pub scroll: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Do {
    /// The named groups (false) or everything (true).
    List(bool),
    View(usize),
    Step(String, i32),
    Reset(String),
    ResetAll,
}

/// A list on the page that the wheel scrolls.
#[derive(Component)]
pub(super) struct Scroll(usize);

impl Screen {
    /// The world's tuning as the page stands: the preset with every change on.
    pub(super) fn tuned(&self) -> WorldConfig {
        self.tuned_with(&self.setup)
    }

    fn tuned_with(&self, setup: &Setup) -> WorldConfig {
        let mut c = self.config.clone();
        let _ = self.rules.difficulty.apply(setup, &mut c);
        c
    }

    pub(super) fn sandbox_do(&mut self, d: Do) {
        match d {
            Do::List(all) => {
                if self.sandbox.everything != all {
                    self.sandbox = State { everything: all, ..Default::default() };
                }
            }
            Do::View(k) => {
                self.sandbox.view = k;
                self.sandbox.scroll[1] = 0.0;
            }
            Do::Step(path, by) => {
                let now = tune::leaves(&self.tuned()).into_iter().find(|l| l.path == path);
                if let Some(leaf) = now {
                    let next = step(self.rules.sandbox.option(&path), leaf.value, by);
                    self.set_value(&path, next);
                }
            }
            Do::Reset(path) => {
                self.setup.values.remove(&path);
            }
            Do::ResetAll => self.setup.values.clear(),
        }
    }

    /// `path` set to `v`, unless the world would refuse it (the reason on the footer). What the
    /// preset already gives isn't a change.
    fn set_value(&mut self, path: &str, v: Value) {
        let text = v.text();
        let mut without = self.setup.clone();
        without.values.remove(path);
        let mut base = self.tuned_with(&without);
        if tune::get(&base, path).as_deref() == Some(text.as_str()) {
            self.setup.values.remove(path);
            return;
        }
        match tune::set(&mut base, path, &text) {
            Ok(()) => {
                self.setup.values.insert(path.to_string(), text);
            }
            Err(e) => self.error = Some(e),
        }
    }
}

/// `v` stepped `by`: by the file's step and range when it names it, else [`nudge`]d; a switch
/// turns over.
pub(super) fn step(opt: Option<&SandboxOption>, v: Value, by: i32) -> Value {
    match (v, opt.filter(|o| o.step > 0.0)) {
        (Value::Bool(b), _) => Value::Bool(!b),
        (Value::Num(x), Some(o)) => Value::Num(o.stepped(x, by)),
        (Value::Whole(n), Some(o)) => Value::Whole(o.stepped(n as f64, by).round() as i64),
        (Value::Num(x), None) => Value::Num(nudge(x, by)),
        (Value::Whole(n), None) => {
            let m = n.saturating_add((n.abs() / 10).max(1) * by as i64);
            Value::Whole(if n >= 0 { m.clamp(0, tune::LIMIT as i64) } else { m })
        }
    }
}

/// A number with no step of its own, moved by a step of its size: half its leading power of ten
/// (22 by 5 to 25, 0.6 by 0.05), never through 0 from above and never past [`tune::LIMIT`].
pub(super) fn nudge(x: f64, by: i32) -> f64 {
    let s = if x == 0.0 { 0.1 } else { 10f64.powf(x.abs().log10().floor()) / 2.0 };
    let mut v = ((x + s * by as f64) / s).round() * s;
    if x >= 0.0 {
        v = v.max(0.0);
    }
    (v.clamp(-tune::LIMIT, tune::LIMIT) * 1e6).round() / 1e6
}

/// A value as the page shows it: "22 m", "1.5x", "On".
pub(super) fn shown(v: Value, unit: &str) -> String {
    let n = match v {
        Value::Bool(b) => return if b { "On" } else { "Off" }.to_string(),
        Value::Num(x) => format!("{}", (x * 1000.0).round() / 1000.0),
        Value::Whole(n) => n.to_string(),
    };
    match unit {
        "" => n,
        "x" | "%" | "°" => format!("{n}{unit}"),
        u => format!("{n} {u}"),
    }
}

/// A path's last parts in words, for the Everything list: `respawn.unseen_secs` is "respawn: unseen secs".
fn path_words(path: &str) -> String {
    let rest = path.split_once('.').map_or(path, |(_, r)| r);
    rest.replace('.', ": ").replace('_', " ")
}

/// How many of the setup's own values `is` takes.
fn changed_in(screen: &Screen, mut is: impl FnMut(&str) -> bool) -> usize {
    screen.setup.values.keys().filter(|p| is(p.as_str())).count()
}

/// A small button with a word on it.
fn small(parent: &mut ChildSpawnerCommands, label: &str, act: Act) {
    let mut e = parent.spawn((Node { height: px(24), padding: UiRect::horizontal(px(8)), flex_shrink: 0.0, justify_content: JustifyContent::Center, align_items: AlignItems::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(RADIUS)), ..default() }, BackgroundColor(CELL), BorderColor::all(LINE)));
    e.with_children(|b| line(b, label.to_uppercase(), theme::head(flat::LABEL), BONE));
    live(&mut e, Look::CELL, Some(act));
}

fn scroller<'a>(c: &'a mut ChildSpawnerCommands, k: usize, y: f32) -> EntityCommands<'a> {
    c.spawn((Scroll(k), ScrollPosition(Vec2::new(0.0, y)), Node { flex_direction: FlexDirection::Column, row_gap: px(4), flex_grow: 1.0, flex_basis: px(0), min_height: px(0), overflow: Overflow::scroll_y(), ..default() }))
}

/// One thing on the right: its name, whether it is changed, its arrows and value.
struct Item {
    path: String,
    name: String,
    about: String,
    unit: String,
}

pub(super) fn page(cols: &mut ChildSpawnerCommands, screen: &Screen) {
    let st = &screen.sandbox;
    let groups = &screen.rules.sandbox.groups;
    let names: Vec<(String, usize)> = if st.everything {
        tune::SECTIONS.iter().map(|s| (s.name.to_string(), changed_in(screen, |p| p.split('.').next() == Some(s.id)))).collect()
    } else {
        groups.iter().map(|g| (g.name.clone(), changed_in(screen, |p| g.options.iter().any(|o| o.path == p)))).collect()
    };
    let view = st.view.min(names.len().saturating_sub(1));
    // The groups.
    column(cols, 0.8, false).with_children(|list| {
        list.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(8), flex_shrink: 0.0, margin: UiRect::bottom(px(6)), ..default() }, Pickable::IGNORE)).with_children(|r| {
            chip(r, "Named", None, !st.everything, Act::Sandbox(Do::List(false)));
            chip(r, "Everything", None, st.everything, Act::Sandbox(Do::List(true)));
        });
        scroller(list, 0, st.scroll[0]).with_children(|l| {
            for (k, (name, changed)) in names.iter().enumerate() {
                let about = if *changed > 0 { format!("{name}: {changed} changed by you.") } else { name.clone() };
                row(l, k == view, Some(Act::Sandbox(Do::View(k))), about).with_children(|r| {
                    r.spawn((Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), overflow: Overflow::clip(), ..default() }, Pickable::IGNORE)).with_children(|n| line(n, name.clone(), font(BODY), BONE));
                    if *changed > 0 {
                        line(r, format!("{changed}"), theme::strong(SMALL), GREEN);
                    }
                });
            }
        });
    });
    // What is in the one open.
    let tuned = screen.tuned();
    let leaves: Vec<Leaf> = tune::leaves(&tuned);
    let items: Vec<Item> = if st.everything {
        let id = tune::SECTIONS.get(view).map_or("", |s| s.id);
        leaves
            .iter()
            .filter(|l| l.path.split('.').next() == Some(id))
            .map(|l| {
                let named = screen.rules.sandbox.option(&l.path);
                Item { path: l.path.clone(), name: named.map_or_else(|| path_words(&l.path), |o| o.name.clone()), about: named.map_or(String::new(), |o| o.about.clone()), unit: named.map_or(String::new(), |o| o.unit.clone()) }
            })
            .collect()
    } else {
        groups.get(view).map_or_else(Vec::new, |g| g.options.iter().map(|o| Item { path: o.path.clone(), name: o.name.clone(), about: o.about.clone(), unit: o.unit.clone() }).collect())
    };
    column(cols, 1.3, true).with_children(|list| {
        list.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Center, flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|top| {
            top.spawn((Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), overflow: Overflow::clip(), ..default() }, Pickable::IGNORE)).with_children(|t| column_title(t, names.get(view).map_or("", |n| n.0.as_str())));
            let all = screen.setup.values.len();
            if all > 0 {
                line(top, format!("{all} changed in all"), font(SMALL), DIM);
                small(top, "Reset all", Act::Sandbox(Do::ResetAll));
            }
        });
        scroller(list, 1, st.scroll[1]).with_children(|l| {
            for it in &items {
                let Some(leaf) = leaves.iter().find(|x| x.path == it.path) else { continue };
                let own = screen.setup.values.contains_key(&it.path);
                let about = format!("{}{}{}  ({})", it.name, if it.about.is_empty() { "" } else { ": " }, it.about, it.path);
                l.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(8), height: px(ROW_H + 2.0), align_items: AlignItems::Center, padding: UiRect::horizontal(px(10)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(RADIUS)), flex_shrink: 0.0, ..default() }, BackgroundColor(CELL), BorderColor::all(if own { GREEN } else { Color::NONE })))
                    .observe(move |_: On<PointerOver>, mut detail: Query<&mut Text, With<Detail>>| {
                        for mut t in &mut detail {
                            if t.0 != about {
                                t.0 = about.clone();
                            }
                        }
                    })
                    .with_children(|r| {
                        r.spawn((Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), overflow: Overflow::clip(), ..default() }, Pickable::IGNORE)).with_children(|n| line(n, it.name.clone(), font(BODY), BONE));
                        if own {
                            small(r, "Reset", Act::Sandbox(Do::Reset(it.path.clone())));
                        }
                        arrow(r, "<", Act::Sandbox(Do::Step(it.path.clone(), -1)));
                        r.spawn((Node { width: px(118), justify_content: JustifyContent::Center, flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|n| line(n, shown(leaf.value, &it.unit), theme::strong(BODY), if own { GREEN } else { BONE }));
                        arrow(r, ">", Act::Sandbox(Do::Step(it.path.clone(), 1)));
                    });
            }
        });
    });
}

/// The wheel over a list scrolls it, and the page remembers where.
pub(super) fn wheel(mut wheel: MessageReader<MouseWheel>, window: Query<&Window, With<bevy::window::PrimaryWindow>>, mut lists: Query<(&Scroll, &ComputedNode, &UiGlobalTransform, &mut ScrollPosition)>, mut screen: ResMut<Screen>) {
    let px = wheel.read().map(|ev| if ev.unit == MouseScrollUnit::Line { ev.y * 40.0 } else { ev.y }).fold(0.0, |a, b| a + b);
    if px == 0.0 {
        return;
    }
    let Some(cursor) = window.single().ok().and_then(|w| w.physical_cursor_position()) else { return };
    for (which, node, at, mut pos) in &mut lists {
        if !node.contains_point(*at, cursor) {
            continue;
        }
        let max = (node.content_size.y - node.size.y) * node.inverse_scale_factor();
        let y = crate::hud::scrolled(pos.y, px, max);
        if pos.y != y {
            pos.y = y;
        }
        // Not `dirty`: nothing is drawn again for a scroll.
        screen.bypass_change_detection().sandbox.scroll[which.0] = y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_step_by_their_size_and_switches_turn_over() {
        assert_eq!(nudge(22.0, 1), 25.0);
        assert_eq!(nudge(0.6, -1), 0.55);
        assert_eq!(nudge(0.0, 1), 0.1);
        assert_eq!(nudge(0.05, -1), 0.045);
        assert_eq!(nudge(0.0, -1), 0.0);
        assert_eq!(step(None, Value::Bool(true), 1), Value::Bool(false));
        assert_eq!(step(None, Value::Whole(3), -1), Value::Whole(2));
        assert_eq!(step(None, Value::Whole(0), -1), Value::Whole(0));
        assert_eq!(step(None, Value::Whole(200), 1), Value::Whole(220));
        let o = SandboxOption { path: "zombie.sight_day".into(), name: "Sight".into(), about: String::new(), min: 2.0, max: 80.0, step: 1.0, unit: "m".into() };
        assert_eq!(step(Some(&o), Value::Num(79.6), 1), Value::Num(80.0));
        assert_eq!(shown(Value::Num(22.0), "m"), "22 m");
        assert_eq!(shown(Value::Num(1.25), "x"), "1.25x");
        assert_eq!(path_words("population.respawn.unseen_secs"), "respawn: unseen secs");
    }
}
