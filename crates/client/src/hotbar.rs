//! The hotbar (#97): a row of nine slots along the bottom of the screen for the things you swap
//! to, on keys 1-9.
//!
//! A slot holds a **kind of item** (its id), not a stack: the bat, the flashlight, water. Its key
//! (or a click on it) does the likely thing with the one you carry ([`press`]): a weapon or tool
//! goes into your hand (what was there into the bags, as `Equip` does), a light into the other
//! hand when the first holds something one-handed, food and drink are eaten or drunk, a pill is
//! taken, clothing put on, and a dressing or antiseptic opens the health panel to pick the wound.
//! It only sends the intents the inventory panel sends, so the server's rules and times apply.
//! A slot shows its key, the item's colour and name and how many you have on you (in your hands
//! and bags); the one in your hand is outlined, and one you don't carry is dimmed.
//!
//! **Filling it:** weapons and lights go onto the first free slot by themselves when they first
//! come into your hands or bags ([`Hotbar::auto_fill`]). With the inventory open, point at an item
//! and press 1-9 to put it on that slot (the same key again takes it off; an item already on
//! another slot swaps with it). Right-click a slot to empty it. While an item's right-click menu is
//! up the number keys are the menu's, and while the search box has the keys they type.
//!
//! The slots are kept between sessions in the client settings file (`settings.ron`,
//! `ui_scale::SettingsStore`). An offscreen render never reads it: `--hotbar ID,ID,...` (`-` for
//! an empty slot) sets the slots for a picture, and auto-fill does the rest.

use bevy::prelude::*;
use zombye_net::Intent;
use zombye_sim::{Inventory, ItemCategory, ItemDef, Place, Slot, Treatment};

use crate::player::LocalPlayer;
use crate::saving::Notice;
use crate::server_link::{JoinInfo, ServerLink};
use crate::theme::{self, DIM, TEXT, font};
use crate::ui_scale::SettingsStore;

/// How many slots: keys 1-9.
pub const SLOTS: usize = 9;

/// (Wide enough for a ten-letter name on one line, "Flashlight"; nine of them still leave the
/// vitals their corner on a 1280 wide screen.)
const SLOT_W: f32 = 92.0;
/// (High enough for the key's line and a name on two lines.)
const SLOT_H: f32 = 62.0;
const GAP: f32 = 4.0;
const PAD: f32 = 8.0;
const RIGHT: f32 = 16.0;
const BOTTOM: f32 = 16.0;
/// How far up from the bottom of the screen the bar reaches (UI pixels), for the current action's
/// bar, which sits just above it (`action_ui.rs`).
pub const REACH: f32 = BOTTOM + SLOT_H + 2.0 * PAD;

const GOLD: Color = theme::GOLD;

const DIGITS: [KeyCode; SLOTS] = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9];

/// The slots: the item id each holds.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct Hotbar {
    pub slots: [Option<String>; SLOTS],
    /// The item kinds on us last frame (`None` before the first look), so only what's newly come
    /// into our hands or bags is put on a slot by itself.
    seen: Option<Vec<u16>>,
    /// Set by `--hotbar` for this session: not written to the settings file.
    session_only: bool,
}

/// Where the item of a slot is on us.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Found {
    Hand,
    OffHand,
    /// The first carried stack of it.
    Carried(u32),
}

/// Where `item` is on us: in the hand, else the other hand, else the first carried stack.
pub fn find(inv: &Inventory, item: u16) -> Option<Found> {
    if inv.hand.is_some_and(|s| s.item == item) {
        Some(Found::Hand)
    } else if inv.off_hand.is_some_and(|s| s.item == item) {
        Some(Found::OffHand)
    } else {
        inv.carried.iter().position(|s| s.item == item).map(|i| Found::Carried(i as u32))
    }
}

/// How many of `item` we have in our hands and bags.
pub fn count(inv: &Inventory, item: u16) -> u32 {
    inv.hand.iter().chain(inv.off_hand.iter()).chain(inv.carried.iter()).filter(|s| s.item == item).map(|s| s.count as u32).sum()
}

/// What a slot's key does.
#[derive(Clone, Debug)]
pub enum Press {
    Send(Intent),
    /// Open the health panel to pick the wound to use this item on.
    UseOn(u16),
    /// Nothing to do, and why (shown as the yellow line).
    Nothing(String),
}

/// What pressing the key of a slot holding `item` does for `player` carrying `inv`: the likely
/// thing with the one in reach, by the same intents the inventory panel sends.
pub fn press(player: u32, inv: &Inventory, items: &[ItemDef], item: u16) -> Press {
    let Some(def) = items.get(item as usize) else { return Press::Nothing("nothing on this slot".into()) };
    let name = def.name.to_lowercase();
    let Some(found) = find(inv, item) else { return Press::Nothing(format!("no {name} on you")) };
    let (from, index) = match found {
        Found::Hand => (Place::Slot(Slot::Hand), 0),
        Found::OffHand => (Place::Slot(Slot::OffHand), 0),
        Found::Carried(i) => (Place::Carried, i),
    };
    match (Treatment::of_item(def), def.category) {
        (Some(t), _) if !t.on_wound() => return Press::Send(Intent::Treat { player, wound: None, action: t, item: Some(item) }),
        (Some(_), _) => return Press::UseOn(item),
        (None, ItemCategory::Food | ItemCategory::Drink) => return Press::Send(Intent::Use { player, from, index }),
        _ => {}
    }
    let Found::Carried(index) = found else { return Press::Nothing(format!("the {name} is in your hand already")) };
    if def.clothing.is_some() {
        return Press::Send(Intent::Wear { player, index });
    }
    // A light beside a one-handed weapon rather than instead of it.
    let hand_one = inv.hand.is_some_and(|h| items.get(h.item as usize).is_some_and(|d| !d.two_handed));
    if def.light.is_some() && !def.two_handed && hand_one && inv.off_hand.is_none() {
        return Press::Send(Intent::MoveItem { player, from: Place::Carried, index, to: Place::Slot(Slot::OffHand), count: 1 });
    }
    Press::Send(Intent::Equip { player, index })
}

/// Does this kind of item go onto the bar by itself: weapons and lights.
pub fn auto_kind(def: &ItemDef) -> bool {
    def.weapon.is_some() || def.light.is_some()
}

impl Hotbar {
    /// From the settings file's list (or `--hotbar`'s): at most [`SLOTS`], an id once.
    pub fn from_ids(ids: &[Option<String>]) -> Self {
        let mut bar = Self::default();
        for (k, id) in ids.iter().take(SLOTS).enumerate() {
            if let Some(id) = id.as_ref().filter(|id| !id.is_empty() && !bar.slots.iter().flatten().any(|s| s == *id)) {
                bar.slots[k] = Some(id.clone());
            }
        }
        bar
    }

    /// `--hotbar bat,-,flashlight`: ids by slot, `-` (or nothing) an empty one.
    pub fn parse(text: &str) -> Self {
        Self::from_ids(&text.split(',').map(|t| t.trim()).map(|t| (!t.is_empty() && t != "-").then(|| t.to_string())).collect::<Vec<_>>())
    }

    /// The slots as the settings file keeps them: up to the last one in use.
    pub fn ids(&self) -> Vec<Option<String>> {
        let n = self.slots.iter().rposition(Option::is_some).map_or(0, |i| i + 1);
        self.slots[..n].to_vec()
    }

    /// Which slot holds the item `id`.
    pub fn slot_of(&self, id: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.as_deref() == Some(id))
    }

    /// Put `id` on slot `k`: what was there goes to the slot `id` came from (a swap), and the
    /// slot's own item again takes it off. Says what happened.
    pub fn assign(&mut self, k: usize, id: &str, name: &str) -> String {
        if k >= SLOTS {
            return String::new();
        }
        if self.slots[k].as_deref() == Some(id) {
            self.slots[k] = None;
            return format!("{name} off the hotbar");
        }
        let was = self.slot_of(id);
        let there = self.slots[k].replace(id.to_string());
        if let Some(was) = was {
            self.slots[was] = there;
        }
        format!("{name} on key {}", k + 1)
    }

    /// Put the weapons and lights that have newly come into our hands or bags on the first free
    /// slots (in the order hand, other hand, bags). Only what wasn't on us last frame: an item
    /// taken off the bar stays off while we keep carrying it.
    pub fn auto_fill(&mut self, inv: &Inventory, items: &[ItemDef]) -> bool {
        let mut on_us: Vec<u16> = Vec::new();
        for s in inv.hand.iter().chain(inv.off_hand.iter()).chain(inv.carried.iter()) {
            if !on_us.contains(&s.item) {
                on_us.push(s.item);
            }
        }
        let mut changed = false;
        for &item in on_us.iter().filter(|i| self.seen.as_ref().is_none_or(|seen| !seen.contains(i))) {
            let Some(def) = items.get(item as usize).filter(|d| auto_kind(d)) else { continue };
            if self.slot_of(&def.id).is_none()
                && let Some(free) = self.slots.iter().position(Option::is_none)
            {
                self.slots[free] = Some(def.id.clone());
                changed = true;
            }
        }
        self.seen = Some(on_us);
        changed
    }
}

/// What one slot shows.
#[derive(Clone, Debug, PartialEq, Default)]
struct SlotView {
    name: String,
    color: Option<(f32, f32, f32)>,
    count: u32,
    held: bool,
    /// Holds an item at all.
    set: bool,
}

/// What the bar shows, to rebuild it only when that changes.
#[derive(Resource, Default, PartialEq)]
struct Shown(Option<Vec<SlotView>>);

/// A slot pressed or emptied with the pointer, for [`keys`] to act on.
#[derive(Resource, Default)]
struct SlotClicks(Vec<(usize, bool)>);

#[derive(Component)]
struct BarRoot;

pub struct HotbarPlugin;

impl Plugin for HotbarPlugin {
    fn build(&self, app: &mut App) {
        let settings = SettingsStore::ensure(app);
        let bar = match app.world().get_resource::<crate::Options>().and_then(|o| o.hotbar.as_deref()) {
            Some(text) => Hotbar { session_only: true, ..Hotbar::parse(text) },
            None => Hotbar::from_ids(&settings.hotbar),
        };
        app.insert_resource(bar).init_resource::<Shown>().init_resource::<SlotClicks>().add_systems(Startup, spawn).add_systems(Update, (fill, keys, rebuild).chain());
    }
}

fn spawn(mut commands: Commands) {
    commands.spawn((
        BarRoot,
        crate::hud::BottomHud,
        Node { position_type: PositionType::Absolute, bottom: px(BOTTOM), right: px(RIGHT), flex_direction: FlexDirection::Row, column_gap: px(GAP), padding: UiRect::all(px(PAD)), ..default() },
        theme::strip(),
    ));
}

/// Keep the settings file up with the slots.
fn keep(bar: &Hotbar, store: &mut SettingsStore) {
    if bar.session_only {
        return;
    }
    let ids = bar.ids();
    if let Err(e) = store.change(|s| s.hotbar = ids) {
        eprintln!("hotbar not remembered: {e}");
    }
}

fn fill(link: Res<ServerLink>, join: Res<JoinInfo>, me: Res<LocalPlayer>, mut bar: ResMut<Hotbar>, mut store: ResMut<SettingsStore>) {
    let Some(v) = link.own(me.id) else { return };
    // (Without `set_changed` every frame: only a new item marks the bar changed.)
    if bar.bypass_change_detection().auto_fill(&v.inventory, &join.items) {
        bar.set_changed();
        keep(&bar, &mut store);
    }
}

#[allow(clippy::too_many_arguments)]
fn keys(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    me: Res<LocalPlayer>,
    join: Res<JoinInfo>,
    panel: Res<crate::inventory_ui::InventoryPanel>,
    menu: Res<crate::inventory_ui::ItemMenu>,
    hovered: Res<crate::inventory_ui::HoveredItem>,
    rows: Query<(), With<Node>>,
    mut clicks: ResMut<SlotClicks>,
    mut bar: ResMut<Hotbar>,
    mut store: ResMut<SettingsStore>,
    mut link: ResMut<ServerLink>,
    mut health: ResMut<crate::health_ui::HealthPanel>,
    mut notice: Option<ResMut<Notice>>,
    options: Option<Res<crate::Options>>,
    mut scripted: Local<Vec<usize>>,
) {
    let mut say = |text: String| {
        if let Some(n) = notice.as_mut().filter(|_| !text.is_empty()) {
            n.say(time.elapsed_secs(), text);
        }
    };
    let mut clicked: Vec<(usize, bool)> = clicks.0.drain(..).collect();
    // `--hotbar-press K@SECS`: as if that slot was pressed then.
    if let Some(script) = options.as_ref().map(|o| &o.hotbar_press) {
        for (i, &(k, after)) in script.iter().enumerate() {
            if !scripted.contains(&i) && time.elapsed_secs() >= after {
                scripted.push(i);
                clicked.push((k - 1, false));
            }
        }
    }
    // Ctrl / Cmd with a number is somebody else's (the UI scale's Ctrl+0 family); a menu's numbers are its own.
    let held = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::SuperLeft, KeyCode::SuperRight]);
    let key = DIGITS.iter().position(|d| keys.just_pressed(*d)).filter(|_| !held && !menu.is_up());
    // Right-click on a slot empties it.
    for &(k, _) in clicked.iter().filter(|(_, empty)| *empty) {
        if let Some(id) = bar.slots[k].take() {
            let name = join.items.iter().find(|d| d.id == id).map_or(id.clone(), |d| d.name.clone());
            say(format!("{name} off the hotbar"));
            keep(&bar, &mut store);
        }
    }
    // A number over an inventory row puts its item on that slot.
    if let (Some(k), true, Some(item)) = (key, panel.open, hovered.item(&rows))
        && let Some(def) = join.items.get(item as usize)
    {
        say(bar.assign(k, &def.id, &def.name));
        keep(&bar, &mut store);
        return;
    }
    if me.dead {
        return;
    }
    let pressed = key.into_iter().chain(clicked.iter().filter(|(_, empty)| !*empty).map(|&(k, _)| k));
    for k in pressed {
        let Some(id) = bar.slots[k].clone() else { continue };
        let Some(item) = join.items.iter().position(|d| d.id == id) else { continue };
        let Some(inv) = link.own(me.id).map(|v| v.inventory.clone()) else { continue };
        match press(me.id, &inv, &join.items, item as u16) {
            Press::Send(intent) => link.send(intent),
            Press::UseOn(item) => (health.open, health.use_item, health.selected) = (true, Some(item), None),
            Press::Nothing(why) => say(why),
        }
    }
}

fn rebuild(mut commands: Commands, bar: Res<Hotbar>, link: Res<ServerLink>, join: Res<JoinInfo>, me: Res<LocalPlayer>, mut shown: ResMut<Shown>, root: Query<Entity, With<BarRoot>>) {
    let Ok(root) = root.single() else { return };
    let inv = link.own(me.id).map(|v| &v.inventory);
    let views: Vec<SlotView> = bar
        .slots
        .iter()
        .map(|slot| {
            let Some(id) = slot else { return SlotView::default() };
            let Some((item, def)) = join.items.iter().enumerate().find(|(_, d)| d.id == *id) else {
                // An item this game's data doesn't have: the slot keeps it, greyed.
                return SlotView { name: id.clone(), set: true, ..default() };
            };
            let (count, held) = inv.map_or((0, false), |inv| (count(inv, item as u16), inv.hand.is_some_and(|s| s.item == item as u16)));
            SlotView { name: def.name.clone(), color: Some(def.color), count, held, set: true }
        })
        .collect();
    if shown.0.as_ref() == Some(&views) {
        return;
    }
    commands.entity(root).despawn_children();
    commands.entity(root).with_children(|c| {
        for (k, v) in views.iter().enumerate() {
            let have = v.count > 0;
            // A hatched slot with a worn rim (`theme::slot`); the one in your hand has a gold edge.
            let rest = if have { theme::SLOT_REST } else { theme::SLOT_EMPTY };
            c.spawn((
                Node { width: px(SLOT_W), height: px(SLOT_H), flex_direction: FlexDirection::Column, padding: UiRect::axes(px(4), px(3)), border: UiRect::all(px(1)), overflow: Overflow::clip(), ..default() },
                theme::slot(rest),
                BorderColor::all(if v.held { GOLD } else { Color::NONE }),
            ))
            .with_children(|s| {
                // The key, the item's colour, how many.
                s.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(5), ..default() }, Pickable::IGNORE)).with_children(|top| {
                    top.spawn((Text::new(format!("{}", k + 1)), theme::head(theme::BODY + 1.0), TextColor(if have { GOLD } else { DIM }), Pickable::IGNORE));
                    if let Some(c) = v.color.filter(|_| have) {
                        top.spawn((Node { width: px(10), height: px(10), border_radius: BorderRadius::all(px(2)), ..default() }, BackgroundColor(Color::srgb(c.0, c.1, c.2)), Pickable::IGNORE));
                    }
                    top.spawn((Node { flex_grow: 1.0, ..default() }, Pickable::IGNORE));
                    if v.count > 1 {
                        top.spawn((Text::new(format!("{}", v.count)), font(theme::SMALL), TextColor(TEXT), Pickable::IGNORE));
                    }
                });
                if v.set {
                    s.spawn((Text::new(v.name.clone()), font(theme::SMALL), TextColor(if have { TEXT } else { DIM }), Pickable::IGNORE));
                }
            })
            .observe(|ev: On<PointerOver>, mut q: Query<&mut ImageNode>| {
                if let Ok(mut bg) = q.get_mut(ev.entity) {
                    bg.color = theme::SLOT_HOVER;
                }
            })
            .observe(move |ev: On<PointerOut>, mut q: Query<&mut ImageNode>| {
                if let Ok(mut bg) = q.get_mut(ev.entity) {
                    bg.color = rest;
                }
            })
            .observe(move |ev: On<PointerPress>, mut clicks: ResMut<SlotClicks>| match ev.button {
                PointerButton::Primary => clicks.0.push((k, false)),
                PointerButton::Secondary => clicks.0.push((k, true)),
                _ => {}
            });
        }
    });
    shown.0 = Some(views);
}

#[cfg(test)]
mod tests {
    use super::*;
    use zombye_sim::ItemStack;

    fn items() -> Vec<ItemDef> {
        zombye_net::content::load_config(&zombye_net::content::default_data_dir(), 1).expect("game data").items
    }

    /// (`Intent` has no `PartialEq`: compared as printed.)
    fn same(a: Press, b: Press) {
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    fn idx(items: &[ItemDef], id: &str) -> u16 {
        items.iter().position(|d| d.id == id).unwrap_or_else(|| panic!("no item {id}")) as u16
    }

    fn stack(items: &[ItemDef], id: &str, count: u16) -> ItemStack {
        ItemStack::new(idx(items, id), count)
    }

    #[test]
    fn a_key_does_the_likely_thing_with_what_you_carry() {
        let items = items();
        let (bat, torch, water, knife) = (idx(&items, "baseball_bat"), idx(&items, "flashlight"), idx(&items, "water_bottle"), idx(&items, "kitchen_knife"));
        let mut inv = Inventory::default();
        // Not on you: nothing, and it says so.
        same(press(1, &inv, &items, knife), Press::Nothing("no kitchen knife on you".into()));
        inv.carried = vec![stack(&items, "water_bottle", 2), stack(&items, "kitchen_knife", 1), stack(&items, "flashlight", 1), stack(&items, "baseball_bat", 1)];
        // A weapon goes into the hand; a drink is drunk where it is.
        same(press(1, &inv, &items, knife), Press::Send(Intent::Equip { player: 1, index: 1 }));
        same(press(1, &inv, &items, water), Press::Send(Intent::Use { player: 1, from: Place::Carried, index: 0 }));
        // With empty hands the light goes into the hand; beside a one-handed weapon, into the other.
        same(press(1, &inv, &items, torch), Press::Send(Intent::Equip { player: 1, index: 2 }));
        inv.hand = Some(inv.carried.remove(1));
        same(press(1, &inv, &items, torch), Press::Send(Intent::MoveItem { player: 1, from: Place::Carried, index: 1, to: Place::Slot(Slot::OffHand), count: 1 }));
        // What's in the hand already stays there; the bat's key swaps to it (it needs both hands).
        same(press(1, &inv, &items, knife), Press::Nothing("the kitchen knife is in your hand already".into()));
        same(press(1, &inv, &items, bat), Press::Send(Intent::Equip { player: 1, index: 2 }));
        inv.hand = Some(inv.carried.remove(2));
        assert!(items[bat as usize].two_handed, "the bat is two-handed in the shipped data");
        same(press(1, &inv, &items, torch), Press::Send(Intent::Equip { player: 1, index: 1 }));
        // A drink held is drunk from the hand.
        inv.hand = Some(stack(&items, "water_bottle", 1));
        same(press(1, &inv, &items, water), Press::Send(Intent::Use { player: 1, from: Place::Slot(Slot::Hand), index: 0 }));
        assert_eq!(count(&inv, water), 3);
    }

    #[test]
    fn pills_are_taken_and_dressings_ask_for_a_wound() {
        let items = items();
        let pill = items.iter().position(|d| Treatment::of_item(d).is_some_and(|t| !t.on_wound())).expect("a pill in the data") as u16;
        let dressing = items.iter().position(|d| Treatment::of_item(d).is_some_and(|t| t.on_wound())).expect("a dressing in the data") as u16;
        let inv = Inventory { carried: vec![ItemStack::new(pill, 1), ItemStack::new(dressing, 1)], ..Default::default() };
        assert!(matches!(press(3, &inv, &items, pill), Press::Send(Intent::Treat { player: 3, wound: None, item: Some(i), .. }) if i == pill));
        same(press(3, &inv, &items, dressing), Press::UseOn(dressing));
    }

    #[test]
    fn weapons_and_lights_take_the_first_free_slots_once() {
        let items = items();
        let mut bar = Hotbar::default();
        let mut inv = Inventory { hand: Some(stack(&items, "baseball_bat", 1)), carried: vec![stack(&items, "water_bottle", 1), stack(&items, "flashlight", 1)], ..Default::default() };
        assert!(bar.auto_fill(&inv, &items));
        assert_eq!(bar.ids(), [Some("baseball_bat".to_string()), Some("flashlight".to_string())]);
        // Nothing new: nothing changes.
        assert!(!bar.auto_fill(&inv, &items));
        // Taken off the bar while still carried: it stays off.
        bar.slots[1] = None;
        assert!(!bar.auto_fill(&inv, &items));
        assert_eq!(bar.slot_of("flashlight"), None);
        // Dropped and picked up again: back on, on the first free slot; a second one of a kind
        // already there doesn't take another.
        inv.carried.pop();
        bar.auto_fill(&inv, &items);
        inv.carried.push(stack(&items, "flashlight", 1));
        inv.carried.push(stack(&items, "kitchen_knife", 1));
        inv.carried.push(stack(&items, "kitchen_knife", 1));
        assert!(bar.auto_fill(&inv, &items));
        assert_eq!(bar.ids(), [Some("baseball_bat".to_string()), Some("flashlight".to_string()), Some("kitchen_knife".to_string())]);
        // A full bar takes no more.
        let mut full = Hotbar::from_ids(&(0..SLOTS).map(|k| Some(format!("thing_{k}"))).collect::<Vec<_>>());
        assert!(!full.auto_fill(&inv, &items));
    }

    /// On the smallest screen the panels are laid out for, the bar leaves the vitals their corner
    /// (16 in, about 330 wide), and a slot holds a ten-letter name on one line.
    #[test]
    fn the_bar_fits_beside_the_vitals_at_1280() {
        let width = SLOTS as f32 * SLOT_W + (SLOTS as f32 - 1.0) * GAP + 2.0 * PAD;
        assert!(crate::ui_scale::FIT_VIEW.x - RIGHT - width >= 16.0 + 330.0 + 8.0, "{width}");
        // Measured in the face the names are set in: the names that went on one line in the
        // monospace still do, and no word of a longer one is wider than a slot (a name wraps at
        // its spaces, and a slot is two lines of it high).
        for name in ["Flashlight", "Baseball bat", "Crowbar", "Fire axe"] {
            let w = theme::width(theme::Face::Text, name, theme::SMALL);
            assert!(w <= SLOT_W - 10.0, "{name}: {w}");
        }
        for word in "Kitchen knife Hunting rifle Painkillers Antiseptic Screwdriver".split(' ') {
            let w = theme::width(theme::Face::Text, word, theme::SMALL);
            assert!(w <= SLOT_W - 10.0, "{word}: {w}");
        }
        assert!(SLOT_H - 8.0 >= 1.2 * (theme::BODY + 1.0) + 2.0 * 1.2 * theme::SMALL);
    }

    #[test]
    fn slots_are_assigned_swapped_and_cleared() {
        let mut bar = Hotbar::parse("bat,-,torch");
        assert_eq!(bar.ids(), [Some("bat".to_string()), None, Some("torch".to_string())]);
        assert_eq!(bar.assign(4, "water", "Water"), "Water on key 5");
        assert_eq!(bar.slot_of("water"), Some(4));
        // Onto a slot in use: the two swap.
        assert_eq!(bar.assign(0, "torch", "Torch"), "Torch on key 1");
        assert_eq!((bar.slot_of("torch"), bar.slot_of("bat")), (Some(0), Some(2)));
        // Its own key again takes it off.
        assert_eq!(bar.assign(0, "torch", "Torch"), "Torch off the hotbar");
        assert_eq!(bar.ids(), [None, None, Some("bat".to_string()), None, Some("water".to_string())]);
        // An id is only ever on one slot, and a list longer than the keys is cut.
        let many: Vec<Option<String>> = (0..12).map(|k| Some(if k < 2 { "same".to_string() } else { format!("i{k}") })).collect();
        let bar = Hotbar::from_ids(&many);
        assert_eq!(bar.slots.iter().flatten().filter(|s| *s == "same").count(), 1);
        assert_eq!(bar.slots[8].as_deref(), Some("i8"));
        assert_eq!(Hotbar::parse("").ids(), Vec::<Option<String>>::new());
    }
}
