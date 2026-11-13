//! The controller's wheels (#12, stage 2): a ring of choices round the middle of the screen,
//! picked with a stick.
//!
//! - **Items** (LB held, `pad.ron`'s `items`): the hotbar's things you carry, then the other
//!   weapons, lights, food, drink and medicine you carry. Let go of LB to use the one picked
//!   (as its hotbar key would: `hotbar::press`); A uses it at once; B or nothing picked does nothing.
//! - **Make** (RB held, `make`): the recipes you can make now, by the server's rule over what you
//!   carry, your floor and the open container. Let go to make the one picked.
//! - **Actions** (A held on a thing: `interact.rs`): everything you can do with what you face (the
//!   lines of its right-click menu). The thumb that held A is the one that picks, so this wheel
//!   stays up after A is let go: A takes the one picked, B goes back.
//!
//! Either stick picks, whichever is pushed further; the pick stays when the stick comes back,
//! so a wheel can be let go of after the stick. While a wheel is up the pad does nothing else
//! (`pad::read_pad` hands the sticks to [`Pad::select`] and keeps the rest back) and the game goes
//! on: it is not a pause. Drawn by `wheel_ui.rs`. Nothing here reaches the sim: a choice is the
//! intent its menu line, hotbar key or Make row sends.

use bevy::prelude::*;
use zombye_net::Intent;
use zombye_sim::{ItemCategory, ItemDef};

use crate::pad::{InputMode, Pad};
use crate::player::LocalPlayer;
use crate::server_link::{JoinInfo, ServerLink};

/// At most this many choices on a wheel (beyond, the inventory or the menu has them).
pub const MAX: usize = 12;
/// How far a stick must be pushed (0..1, past its dead zone) to pick.
pub const PICK_AT: f32 = 0.45;

/// Which wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Items,
    Make,
    Actions,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Kind::Items => "Items",
            Kind::Make => "Make",
            Kind::Actions => "Actions",
        }
    }

    /// The wheel stays up after its button is let go (the action wheel: A's thumb picks).
    pub fn latched(self) -> bool {
        self == Kind::Actions
    }
}

/// What a choice does.
#[derive(Clone, Debug)]
pub enum Does {
    /// Send this, turning to `facing` first (a world menu's line).
    Send(Intent, Option<f32>),
    /// The health screen, to pick the wound to use this item on.
    UseOn(u16),
    /// Only says something (why it can't be done).
    Say(String),
}

/// One choice on a wheel.
#[derive(Clone, Debug)]
pub struct Entry {
    /// What it is ("Baseball bat", "Open", "Rip a T-shirt into rags").
    pub label: String,
    /// A word more, or empty: "x3", "in hand", "12 s".
    pub detail: String,
    /// The item drawn for it (an index into the items), if it is one.
    pub item: Option<u16>,
    /// It can be done now (else drawn dimmed; choosing it says why).
    pub ready: bool,
    pub does: Does,
}

/// The wheel that is up, if any.
#[derive(Resource, Default, Clone, Debug)]
pub struct Wheel {
    pub open: Option<Kind>,
    /// Over the wheel: its kind's title, or what is faced ("Door").
    pub title: String,
    pub entries: Vec<Entry>,
    /// The choice the stick points at (kept when the stick comes back).
    pub pick: Option<usize>,
    /// Bumped whenever any of the above changes, for the drawing to know.
    pub changes: u32,
}

impl Wheel {
    pub fn show(&mut self, kind: Kind, title: impl Into<String>, mut entries: Vec<Entry>) {
        entries.truncate(MAX);
        self.open = Some(kind);
        self.title = title.into();
        self.entries = entries;
        self.pick = None;
        self.changes += 1;
    }

    pub fn close(&mut self) {
        if self.open.is_some() {
            self.open = None;
            self.entries.clear();
            self.pick = None;
            self.changes += 1;
        }
    }

    pub fn is_up(&self) -> bool {
        self.open.is_some()
    }

    /// Point at `dir` (a stick, y up): pick the choice there once it's pushed far enough.
    pub fn point(&mut self, dir: Vec2) {
        if let Some(i) = sector(dir, self.entries.len()).filter(|_| dir.length() >= PICK_AT)
            && self.pick != Some(i)
        {
            self.pick = Some(i);
            self.changes += 1;
        }
    }

    pub fn picked(&self) -> Option<&Entry> {
        self.pick.and_then(|i| self.entries.get(i))
    }
}

/// The angle of choice `i` of `n`, clockwise from straight up, in radians.
pub fn angle_of(i: usize, n: usize) -> f32 {
    std::f32::consts::TAU * i as f32 / n.max(1) as f32
}

/// Which of `n` choices round a wheel `dir` points at (y up, the first straight up, the rest
/// clockwise, each owning the slice of the circle round it). `None` with no choices or no direction.
pub fn sector(dir: Vec2, n: usize) -> Option<usize> {
    if n == 0 || dir.length_squared() < 1e-6 {
        return None;
    }
    // Clockwise from up: atan2 of (x, y).
    let a = dir.x.atan2(dir.y).rem_euclid(std::f32::consts::TAU);
    let step = std::f32::consts::TAU / n as f32;
    Some(((a + step / 2.0) / step) as usize % n)
}

/// The item wheel's choices: the hotbar's things you carry, in its order, then the other weapons,
/// lights, food, drink and medicine you carry, one a kind, at most [`MAX`].
pub fn item_entries(me: u32, inv: &zombye_sim::Inventory, items: &[ItemDef], hotbar: &[Option<String>]) -> Vec<Entry> {
    let mut kinds: Vec<u16> = Vec::new();
    for id in hotbar.iter().flatten() {
        if let Some(i) = items.iter().position(|d| &d.id == id).map(|i| i as u16)
            && crate::hotbar::count(inv, i) > 0
            && !kinds.contains(&i)
        {
            kinds.push(i);
        }
    }
    let held = inv.hand.iter().chain(inv.off_hand.iter()).chain(inv.carried.iter());
    for s in held {
        let Some(def) = items.get(s.item as usize) else { continue };
        let useful = crate::hotbar::auto_kind(def) || matches!(def.category, ItemCategory::Food | ItemCategory::Drink | ItemCategory::Medical);
        if useful && !kinds.contains(&s.item) {
            kinds.push(s.item);
        }
    }
    kinds
        .into_iter()
        .take(MAX)
        .map(|item| {
            let def = &items[item as usize];
            let in_hand = inv.hand.is_some_and(|h| h.item == item) || inv.off_hand.is_some_and(|h| h.item == item);
            let n = crate::hotbar::count(inv, item);
            let detail = if in_hand { "in hand".to_string() } else if n > 1 { format!("x{n}") } else { String::new() };
            let does = match crate::hotbar::press(me, inv, items, item) {
                crate::hotbar::Press::Send(i) => Does::Send(i, None),
                crate::hotbar::Press::UseOn(i) => Does::UseOn(i),
                crate::hotbar::Press::Nothing(why) => Does::Say(why),
            };
            Entry { label: def.name.clone(), detail, item: Some(item), ready: !matches!(does, Does::Say(_)), does }
        })
        .collect()
}

/// The action wheel's choices: a world menu's lines (`world_actions::entries`).
pub fn action_entries(lines: &[crate::world_actions::WorldEntry]) -> Vec<Entry> {
    lines
        .iter()
        .map(|l| match &l.intent {
            Some(i) => Entry { label: l.label.clone(), detail: String::new(), item: None, ready: true, does: Does::Send(i.clone(), l.facing) },
            None => Entry { label: l.label.clone(), detail: String::new(), item: None, ready: false, does: Does::Say(l.label.clone()) },
        })
        .collect()
}

/// The make wheel's choices: the recipes `ok` passes (the server's rule over what is to hand),
/// those that make something you can hold first, at most [`MAX`].
pub fn make_entries(me: u32, recipes: &[zombye_sim::RecipeDef], items: &[ItemDef], ok: impl Fn(&zombye_sim::RecipeDef) -> bool) -> Vec<Entry> {
    recipes
        .iter()
        .enumerate()
        .filter(|(_, r)| ok(r))
        .take(MAX)
        .map(|(k, r)| {
            let item = r.outputs.first().and_then(|o| items.iter().position(|d| d.id == o.item)).map(|i| i as u16);
            Entry { label: r.name.clone(), detail: format!("{:.0} s", r.secs), item, ready: true, does: Does::Send(Intent::Craft { player: me, recipe: k as u16 }, None) }
        })
        .collect()
}

pub struct WheelPlugin;

impl Plugin for WheelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Wheel>().add_systems(Update, (open, steer).chain().run_if(crate::live)).add_systems(Update, script);
    }
}

/// The item or make wheel's title and choices, now.
fn build(kind: Kind, me: &LocalPlayer, join: &JoinInfo, link: &ServerLink, map: &crate::server_link::ClientMap, bar: Option<&crate::hotbar::Hotbar>) -> Option<(&'static str, Vec<Entry>)> {
    match kind {
        Kind::Items => {
            let v = link.own(me.id)?;
            let hotbar: Vec<Option<String>> = bar.map_or(Vec::new(), |b| b.slots.to_vec());
            let entries = item_entries(me.id, &v.inventory, &join.items, &hotbar);
            Some((if entries.is_empty() { "Nothing to use" } else { "Items" }, entries))
        }
        Kind::Make => {
            let ok = crate::inventory_ui::make_check(link, map, join, me);
            let entries = make_entries(me.id, &join.recipes, &join.items, ok);
            Some((if entries.is_empty() { "Nothing to make here" } else { "Make" }, entries))
        }
        Kind::Actions => None,
    }
}

/// LB or RB held long enough: its wheel comes up (while the pad is in use and nothing else is).
#[allow(clippy::too_many_arguments)]
fn open(
    pad: Res<Pad>,
    mode: Res<InputMode>,
    me: Res<LocalPlayer>,
    join: Res<JoinInfo>,
    link: Res<ServerLink>,
    map: Res<crate::server_link::ClientMap>,
    bar: Option<Res<crate::hotbar::Hotbar>>,
    mut wheel: ResMut<Wheel>,
) {
    if wheel.is_up() || !mode.pad || me.dead {
        return;
    }
    let kind = if pad.items_start {
        Kind::Items
    } else if pad.make_start {
        Kind::Make
    } else {
        return;
    };
    if let Some((title, entries)) = build(kind, &me, &join, &link, &map, bar.as_deref()) {
        wheel.show(kind, title, entries);
    }
}

/// `--wheel items|make[:N]`: that wheel up once the game has our view, the Nth choice picked
/// (pictures; it stays up, nothing reads the pad offscreen).
#[allow(clippy::too_many_arguments)]
fn script(
    options: Res<crate::Options>,
    me: Option<Res<LocalPlayer>>,
    join: Option<Res<JoinInfo>>,
    link: Option<Res<ServerLink>>,
    map: Option<Res<crate::server_link::ClientMap>>,
    bar: Option<Res<crate::hotbar::Hotbar>>,
    mut wheel: ResMut<Wheel>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let Some((kind, pick)) = options.wheel.as_deref().and_then(scripted) else {
        *done = true;
        return;
    };
    let (Some(me), Some(join), Some(link), Some(map)) = (me, join, link, map) else { return };
    let Some((title, entries)) = build(kind, &me, &join, &link, &map, bar.as_deref()) else {
        *done = true;
        return;
    };
    *done = true;
    let n = entries.len();
    wheel.show(kind, title, entries);
    wheel.pick = pick.filter(|&i| i < n);
}

/// While a wheel is up: the stick picks; letting go of its button (or A) takes the pick, B
/// goes back.
#[allow(clippy::too_many_arguments)]
fn steer(
    time: Res<Time>,
    pad: Res<Pad>,
    mode: Res<InputMode>,
    mut me: ResMut<LocalPlayer>,
    mut link: ResMut<ServerLink>,
    mut wheel: ResMut<Wheel>,
    mut health: ResMut<crate::health_ui::HealthPanel>,
    mut notice: Option<ResMut<crate::saving::Notice>>,
) {
    let Some(kind) = wheel.open else { return };
    if !mode.pad || me.dead || pad.cancel {
        wheel.close();
        return;
    }
    wheel.point(pad.select);
    let let_go = match kind {
        Kind::Items => !pad.items_held,
        Kind::Make => !pad.make_held,
        Kind::Actions => false,
    };
    if !(pad.confirm || let_go) {
        return;
    }
    let chosen = wheel.picked().cloned();
    wheel.close();
    let Some(entry) = chosen else { return };
    match entry.does {
        Does::Send(intent, facing) => {
            if let Some(f) = facing {
                me.curr.facing = f;
                me.aim = f;
            }
            link.send(intent);
        }
        Does::UseOn(item) => (health.open, health.use_item, health.selected) = (true, Some(item), None),
        Does::Say(why) => {
            if let Some(n) = notice.as_mut() {
                n.say(time.elapsed_secs(), why);
            }
        }
    }
}

/// `--wheel items|make[:N]`: a wheel up from the start with the Nth choice picked, for pictures.
pub fn scripted(spec: &str) -> Option<(Kind, Option<usize>)> {
    let (k, n) = spec.split_once(':').map_or((spec, None), |(k, n)| (k, n.parse().ok()));
    let kind = match k {
        "items" => Kind::Items,
        "make" => Kind::Make,
        "actions" => Kind::Actions,
        _ => return None,
    };
    Some((kind, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stick_picks_the_slice_it_points_into() {
        assert_eq!(sector(Vec2::Y, 4), Some(0));
        assert_eq!(sector(Vec2::X, 4), Some(1));
        assert_eq!(sector(-Vec2::Y, 4), Some(2));
        assert_eq!(sector(-Vec2::X, 4), Some(3));
        // Just left of up is still the first.
        assert_eq!(sector(Vec2::new(-0.1, 1.0), 4), Some(0));
        assert_eq!(sector(Vec2::new(0.1, 1.0), 8), Some(0));
        assert_eq!(sector(Vec2::new(1.0, 1.0), 8), Some(1));
        assert_eq!(sector(Vec2::ZERO, 4), None);
        assert_eq!(sector(Vec2::Y, 0), None);
    }

    #[test]
    fn the_pick_stays_when_the_stick_comes_back() {
        let mut w = Wheel::default();
        let e = Entry { label: "a".into(), detail: String::new(), item: None, ready: true, does: Does::Say(String::new()) };
        w.show(Kind::Items, "Items", vec![e.clone(), e.clone(), e.clone()]);
        w.point(Vec2::new(0.0, 0.2));
        assert_eq!(w.pick, None, "a nudge picks nothing");
        w.point(Vec2::new(0.9, -0.3));
        assert_eq!(w.pick, Some(1));
        w.point(Vec2::ZERO);
        assert_eq!(w.pick, Some(1));
        w.close();
        assert!(!w.is_up() && w.entries.is_empty());
    }

    #[test]
    fn a_wheel_holds_at_most_twelve() {
        let mut w = Wheel::default();
        let e = Entry { label: "a".into(), detail: String::new(), item: None, ready: true, does: Does::Say(String::new()) };
        w.show(Kind::Make, "Make", vec![e; 30]);
        assert_eq!(w.entries.len(), MAX);
    }

    #[test]
    fn scripted_wheels_parse() {
        assert_eq!(scripted("items:2"), Some((Kind::Items, Some(2))));
        assert_eq!(scripted("make"), Some((Kind::Make, None)));
        assert_eq!(scripted("pie"), None);
    }
}
