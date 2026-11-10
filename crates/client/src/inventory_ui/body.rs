//! The body view (Linear DCM-43, #99): the "Worn" tab of the inventory panel drawn on a simple
//! outline of a person instead of as a list. A column of places either side of the figure
//! ([`Spot`]: the head, face, neck, torso, arms, hands, legs and feet for what is worn, and the two
//! hands, the back and the shoulder for what is held and carried), each listing what is there,
//! outermost first; the figure's parts take the colour of what covers them ([`tint`]), a bag shows
//! on its back or at its hip and a bar in each hand that holds something.
//!
//! Every item is a tile with the list's own presses ([`super::clickable`], the same [`RowAction`]
//! the list's row has): a click takes it off into the bags, Alt-click, Ctrl- and Shift-click and
//! the right-click menu as there, and the hotbar's number keys over it. The list stays the
//! default; the "List" / "Body" control in the pane's title switches, remembered in
//! `settings.ron` (`ClientSettings::inventory_body`). `--tab body` starts with it, for pictures.
//!
//! The search and chips don't narrow it: every place always shows what is in it.

use bevy::prelude::*;
use zombye_sim::{BodyPart, Inventory, ItemDef, ItemStack, Layer, Place, Slot, WearSlot};

use super::{CELL_GAP, Ctx, DIM, Key, Lead, Line, ROW_FONT, ROW_PICKED, TEXT, W_SWATCH, clickable, font};

/// A place of the body view: where worn things are listed, or a slot that holds one thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Spot {
    Head,
    Face,
    Neck,
    Torso,
    Arms,
    Hands,
    Legs,
    Feet,
    Hand,
    OffHand,
    Back,
    Shoulder,
}

impl Spot {
    /// The column left of the figure and the one right of it, top to bottom. (The figure faces
    /// us, so the hand that swings is on our left.)
    pub(super) const LEFT: [Spot; 6] = [Spot::Head, Spot::Face, Spot::Torso, Spot::Arms, Spot::Hand, Spot::Legs];
    pub(super) const RIGHT: [Spot; 6] = [Spot::Neck, Spot::Back, Spot::Shoulder, Spot::OffHand, Spot::Hands, Spot::Feet];

    pub(super) fn label(self) -> &'static str {
        match self {
            Spot::Head => "Head",
            Spot::Face => "Face",
            Spot::Neck => "Neck",
            Spot::Torso => "Torso",
            Spot::Arms => "Arms",
            Spot::Hands => "Hands",
            Spot::Legs => "Legs",
            Spot::Feet => "Feet",
            Spot::Hand => "Hand",
            Spot::OffHand => "Off hand",
            Spot::Back => "Back",
            Spot::Shoulder => "Shoulder",
        }
    }

    /// The inventory slot this place is, if it is one (else it lists worn things).
    pub(super) fn slot(self) -> Option<Slot> {
        match self {
            Spot::Hand => Some(Slot::Hand),
            Spot::OffHand => Some(Slot::OffHand),
            Spot::Back => Some(Slot::Back),
            Spot::Shoulder => Some(Slot::Shoulder),
            _ => None,
        }
    }

    /// Where a worn item is listed: the accessory place it takes (a hat on the head, glasses on
    /// the face, a scarf round the neck), else the main body part it covers (a hoodie the torso
    /// though it has arms, boots the feet though they reach up the legs). Anything else worn (a
    /// mod's item that covers nothing): the torso.
    pub(super) fn of(def: &ItemDef) -> Spot {
        let Some(c) = &def.clothing else { return Spot::Torso };
        for (takes, spot) in [(WearSlot::HeadTop, Spot::Head), (WearSlot::Face, Spot::Face), (WearSlot::Neck, Spot::Neck)] {
            if c.takes.contains(&takes) {
                return spot;
            }
        }
        for (covers, spot) in [(BodyPart::Torso, Spot::Torso), (BodyPart::Feet, Spot::Feet), (BodyPart::Hands, Spot::Hands), (BodyPart::Legs, Spot::Legs), (BodyPart::Arms, Spot::Arms), (BodyPart::Head, Spot::Head)] {
            if c.covers.contains(&covers) {
                return spot;
            }
        }
        Spot::Torso
    }

    /// Whether the worn item `def` reaches this place of the figure (for its colour): it covers
    /// that part, or takes that accessory place. `hat`: something worn takes the head top (a hood
    /// is then down).
    fn reached_by(self, def: &ItemDef, hat: bool) -> bool {
        let Some(c) = &def.clothing else { return self == Spot::Torso };
        let covers = |part: BodyPart| c.covers.contains(&part);
        match self {
            Spot::Head => covers(BodyPart::Head) || c.takes.contains(&WearSlot::HeadTop) || (c.hood && !hat),
            Spot::Face => c.takes.contains(&WearSlot::Face),
            Spot::Neck => c.takes.contains(&WearSlot::Neck),
            Spot::Torso => covers(BodyPart::Torso),
            Spot::Arms => covers(BodyPart::Arms),
            Spot::Hands => covers(BodyPart::Hands),
            Spot::Legs => covers(BodyPart::Legs),
            Spot::Feet => covers(BodyPart::Feet),
            Spot::Hand | Spot::OffHand | Spot::Back | Spot::Shoulder => false,
        }
    }
}

fn layer_of(def: Option<&ItemDef>) -> Layer {
    def.and_then(|d| d.clothing.as_ref()).map_or(Layer::default(), |c| c.layer)
}

/// What is at `spot`: the one thing in a slot, or the worn things listed there, outermost first
/// (a jacket over the hoodie over the T-shirt). Every held and worn stack is at exactly one place.
pub(super) fn lines(items: &[ItemDef], inv: &Inventory, spot: Spot) -> Vec<Line> {
    if let Some(slot) = spot.slot() {
        return inv.slot(slot).map(|stack| Line { from: Place::Slot(slot), index: 0, stack }).into_iter().collect();
    }
    let def = |s: &ItemStack| items.get(s.item as usize);
    let mut worn: Vec<Line> = inv.worn.iter().enumerate().filter(|(_, s)| def(s).map_or(Spot::Torso, Spot::of) == spot).map(|(k, s)| Line { from: Place::Worn, index: k as u32, stack: *s }).collect();
    worn.sort_by_key(|l| std::cmp::Reverse(layer_of(def(&l.stack))));
    worn
}

/// The colour the figure shows at `spot`: of what the slot holds, or of the worn item that
/// reaches that part, the one listed there before one that only reaches it (the jeans' colour on
/// the legs, not the boots'), then the outermost. `None`: bare.
pub(super) fn tint(items: &[ItemDef], inv: &Inventory, spot: Spot) -> Option<Color> {
    let colour = |d: &ItemDef| Color::srgb(d.color.0, d.color.1, d.color.2);
    if let Some(slot) = spot.slot() {
        return inv.slot(slot).and_then(|s| items.get(s.item as usize)).map(colour);
    }
    let worn = || inv.worn.iter().filter_map(|s| items.get(s.item as usize));
    let hat = worn().any(|d| d.clothing.as_ref().is_some_and(|c| c.takes.contains(&WearSlot::HeadTop)));
    worn().filter(|d| spot.reached_by(d, hat)).max_by_key(|d| (Spot::of(d) == spot, layer_of(Some(d)))).map(colour)
}

/// The figure's box, and a place's column beside it (two of them and the figure fill a pane).
/// (The figure is laid out on a 108 x 268 grid drawn at [`SCALE`], so that on a 1280 x 720
/// screen with the objectives up it is all in view without scrolling.)
const SCALE: f32 = 0.86;
const FIGURE_W: f32 = 108.0 * SCALE;
const FIGURE_H: f32 = 268.0 * SCALE;
const COLUMN_W: f32 = 141.0;
const GAP: f32 = 4.0;
/// A place's backing, an item's tile on it, the figure's bare fill and its outline.
const SPOT: Color = crate::theme::INSET;
const TILE: Color = Color::srgba(1.0, 1.0, 1.0, 0.07);
const BARE: Color = Color::srgb(0.19, 0.19, 0.21);
const OUTLINE: Color = Color::srgb(0.66, 0.64, 0.57);

/// The body view, as the body of the "Worn" pane. `listed` gets every stack drawn, in order.
pub(super) fn draw(ctx: &Ctx, listed: &mut Vec<Key>, col: &mut ChildSpawnerCommands, inv: &Inventory) {
    col.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(GAP), align_items: AlignItems::FlexStart, margin: UiRect::top(px(2)), flex_shrink: 0.0, ..default() }).with_children(|b| {
        column(ctx, listed, b, inv, &Spot::LEFT);
        figure(b, ctx.items, inv);
        column(ctx, listed, b, inv, &Spot::RIGHT);
    });
}

fn column(ctx: &Ctx, listed: &mut Vec<Key>, b: &mut ChildSpawnerCommands, inv: &Inventory, spots: &[Spot]) {
    b.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), width: px(COLUMN_W), flex_shrink: 0.0, ..default() }).with_children(|c| {
        for &spot in spots {
            let here = lines(ctx.items, inv, spot);
            // (Nothing shipped is listed at the arms: sleeves come with the torso's garment. The
            // place shows only when something is.)
            if here.is_empty() && spot == Spot::Arms {
                continue;
            }
            c.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(1), padding: UiRect::axes(px(3), px(2)), flex_shrink: 0.0, ..default() }, BackgroundColor(SPOT))).with_children(|s| {
                // A place with nothing at it is one line: its name, and a dash at the end.
                s.spawn((Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, padding: UiRect::axes(px(1), px(0)), ..default() }, Pickable::IGNORE)).with_children(|t| {
                    t.spawn((Text::new(spot.label().to_uppercase()), crate::theme::head(ROW_FONT), TextColor(DIM), TextLayout::no_wrap(), Pickable::IGNORE));
                    if here.is_empty() {
                        t.spawn((Text::new("-"), font(ROW_FONT), TextColor(DIM), Pickable::IGNORE));
                    }
                });
                for l in here {
                    listed.push(l.key());
                    tile(ctx, s, &l);
                }
            });
        }
    });
}

/// One held or worn thing at its place: its colour, its name (cut off, never wrapped) and what
/// the list says after it; pressed as a row of the list is.
fn tile(ctx: &Ctx, s: &mut ChildSpawnerCommands, l: &Line) {
    let cells = ctx.cells(l, Lead::Nothing, None);
    let rest = if ctx.picked.keys.contains(&l.key()) { ROW_PICKED } else { TILE };
    let mut e = s.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(CELL_GAP), padding: UiRect::axes(px(4), px(2)), flex_shrink: 0.0, ..default() }, BackgroundColor(rest)));
    e.with_children(|r| {
        r.spawn((Node { width: px(W_SWATCH), height: px(W_SWATCH), flex_shrink: 0.0, border_radius: BorderRadius::all(px(2)), ..default() }, BackgroundColor(cells.swatch.unwrap_or(Color::NONE)), Pickable::IGNORE));
        r.spawn((Node { min_width: px(0), flex_grow: 1.0, flex_shrink: 1.0, flex_basis: px(0), column_gap: px(6), overflow: Overflow::clip_x(), ..default() }, Pickable::IGNORE)).with_children(|n| {
            n.spawn((Text::new(cells.name), font(ROW_FONT), TextColor(TEXT), TextLayout::no_wrap(), Node { flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
            if !cells.note.is_empty() {
                n.spawn((Text::new(cells.note), font(ROW_FONT), TextColor(DIM), TextLayout::no_wrap(), Node { flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
            }
        });
    });
    clickable(&mut e, ctx.act(l, Place::Carried, Vec::new()), rest);
}

/// The outline of a person, facing us, each part in the colour of what covers it.
fn figure(b: &mut ChildSpawnerCommands, items: &[ItemDef], inv: &Inventory) {
    let t = |spot: Spot| tint(items, inv, spot);
    let round = |r: f32| BorderRadius::all(px(r * SCALE));
    b.spawn((Node { width: px(FIGURE_W), height: px(FIGURE_H), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|f| {
        // A bag on the back shows round the shoulders.
        if let Some(c) = t(Spot::Back) {
            part(f, [24.0, 44.0, 60.0, 76.0], round(10.0), Some(c));
        }
        part(f, [48.0, 36.0, 12.0, 12.0], round(0.0), None);
        part(f, [37.0, 4.0, 34.0, 34.0], round(17.0), None);
        // What is on the head covers its top half; what is on the face, a band across it.
        if let Some(c) = t(Spot::Head) {
            part(f, [37.0, 4.0, 34.0, 16.0], BorderRadius { top_left: px(17.0 * SCALE).into(), top_right: px(17.0 * SCALE).into(), bottom_left: px(0).into(), bottom_right: px(0).into() }, Some(c));
        }
        if let Some(c) = t(Spot::Face) {
            part(f, [40.0, 20.0, 28.0, 9.0], round(3.0), Some(c));
        }
        for x in [15.0, 79.0] {
            part(f, [x, 50.0, 14.0, 78.0], round(6.0), t(Spot::Arms));
            part(f, [x, 129.0, 14.0, 14.0], round(7.0), t(Spot::Hands));
        }
        for x in [32.0, 55.0] {
            part(f, [x, 130.0, 21.0, 116.0], round(6.0), t(Spot::Legs));
        }
        part(f, [26.0, 247.0, 27.0, 14.0], round(5.0), t(Spot::Feet));
        part(f, [55.0, 247.0, 27.0, 14.0], round(5.0), t(Spot::Feet));
        part(f, [31.0, 46.0, 46.0, 88.0], round(8.0), t(Spot::Torso));
        if let Some(c) = t(Spot::Neck) {
            part(f, [41.0, 36.0, 26.0, 12.0], round(5.0), Some(c));
        }
        // A bag from the shoulder hangs at the hip; what a hand holds, a bar from it.
        if let Some(c) = t(Spot::Shoulder) {
            part(f, [70.0, 112.0, 24.0, 28.0], round(4.0), Some(c));
        }
        if let Some(c) = t(Spot::Hand) {
            part(f, [4.0, 92.0, 8.0, 58.0], round(3.0), Some(c));
        }
        if let Some(c) = t(Spot::OffHand) {
            part(f, [96.0, 118.0, 8.0, 32.0], round(3.0), Some(c));
        }
    });
}

/// One piece of the figure at `[left, top, width, height]`, outlined, filled with `fill` (bare
/// without one).
fn part(f: &mut ChildSpawnerCommands, at: [f32; 4], border_radius: BorderRadius, fill: Option<Color>) {
    f.spawn((
        Node { position_type: PositionType::Absolute, left: px(at[0] * SCALE), top: px(at[1] * SCALE), width: px(at[2] * SCALE), height: px(at[3] * SCALE), border: UiRect::all(px(1.5)), border_radius, ..default() },
        BackgroundColor(fill.unwrap_or(BARE)),
        BorderColor::all(OUTLINE),
        Pickable::IGNORE,
    ));
}

#[cfg(test)]
mod tests {
    use super::super::{Find, InFlight, PanelUi, Picked};
    use super::*;

    fn shipped_items() -> Vec<ItemDef> {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        zombye_net::content::load_config(&data, 1).unwrap().items
    }

    fn stack(items: &[ItemDef], id: &str) -> ItemStack {
        ItemStack::one(items.iter().position(|d| d.id == id).unwrap_or_else(|| panic!("no item {id}")) as u16)
    }

    fn dressed(items: &[ItemDef]) -> Inventory {
        let s = |id: &str| stack(items, id);
        Inventory {
            hand: Some(s("crowbar")),
            off_hand: Some(s("flashlight")),
            back: Some(s("hiking_pack")),
            shoulder: Some(s("tote_bag")),
            worn: ["t_shirt", "jeans", "hoodie", "work_boots", "work_gloves", "glasses", "baseball_cap", "scarf", "leather_jacket"].into_iter().map(s).collect(),
            carried: vec![s("tinned_beans")],
        }
    }

    /// Every shipped thing that can be worn has its place, the two columns hold every place once,
    /// and every slot is one of them.
    #[test]
    fn every_slot_and_wearable_has_a_place() {
        let items = shipped_items();
        let all: Vec<Spot> = Spot::LEFT.into_iter().chain(Spot::RIGHT).collect();
        for (k, s) in all.iter().enumerate() {
            assert!(!all[..k].contains(s), "{s:?} twice");
        }
        for slot in [Slot::Hand, Slot::OffHand, Slot::Back, Slot::Shoulder] {
            assert_eq!(all.iter().filter(|s| s.slot() == Some(slot)).count(), 1, "{slot:?}");
        }
        let place = |id: &str| Spot::of(&items[stack(&items, id).item as usize]);
        assert_eq!(place("hoodie"), Spot::Torso);
        assert_eq!(place("jeans"), Spot::Legs);
        assert_eq!(place("work_boots"), Spot::Feet, "though they reach up the legs");
        assert_eq!(place("work_gloves"), Spot::Hands);
        assert_eq!(place("glasses"), Spot::Face);
        assert_eq!(place("dust_mask"), Spot::Face);
        assert_eq!(place("baseball_cap"), Spot::Head);
        assert_eq!(place("balaclava"), Spot::Head);
        assert_eq!(place("scarf"), Spot::Neck);
        for d in items.iter().filter(|d| d.clothing.is_some()) {
            let spot = Spot::of(d);
            assert!(spot.slot().is_none() && all.contains(&spot), "{}", d.id);
            assert!(spot.reached_by(d, true), "{} colours the place it is listed at", d.id);
        }
    }

    /// Everything held and worn is at exactly one place, worn things outermost first; what is
    /// only carried is nowhere.
    #[test]
    fn everything_held_and_worn_is_at_one_place() {
        let items = shipped_items();
        let inv = dressed(&items);
        let all: Vec<Line> = Spot::LEFT.into_iter().chain(Spot::RIGHT).flat_map(|s| lines(&items, &inv, s)).collect();
        assert_eq!(all.len(), 4 + inv.worn.len());
        for slot in [Slot::Hand, Slot::OffHand, Slot::Back, Slot::Shoulder] {
            assert_eq!(all.iter().filter(|l| l.from == Place::Slot(slot)).count(), 1);
        }
        for k in 0..inv.worn.len() as u32 {
            assert_eq!(all.iter().filter(|l| l.from == Place::Worn && l.index == k).count(), 1, "worn {k}");
        }
        assert!(all.iter().all(|l| l.from != Place::Carried));
        let ids = |spot: Spot| lines(&items, &inv, spot).iter().map(|l| items[l.stack.item as usize].id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(Spot::Torso), ["leather_jacket", "hoodie", "t_shirt"], "outermost first");
        assert_eq!(ids(Spot::Hand), ["crowbar"]);
        assert_eq!(ids(Spot::Arms), Vec::<String>::new());
        // Their indices are the inventory's own (what the intents name).
        let jacket = lines(&items, &inv, Spot::Torso)[0];
        assert_eq!((jacket.from, jacket.index), (Place::Worn, 8));
        assert!(lines(&items, &Inventory::default(), Spot::Torso).is_empty() && lines(&items, &Inventory::default(), Spot::Back).is_empty());
    }

    /// The figure's colours: the outermost thing on each part, a part's own garment before one
    /// that only reaches it, a hood up until a hat is worn, nothing where nothing is.
    #[test]
    fn the_figure_takes_the_colour_of_what_covers_it() {
        let items = shipped_items();
        let colour = |id: &str| {
            let d = &items[stack(&items, id).item as usize];
            Some(Color::srgb(d.color.0, d.color.1, d.color.2))
        };
        let inv = dressed(&items);
        let t = |spot: Spot| tint(&items, &inv, spot);
        assert_eq!(t(Spot::Torso), colour("leather_jacket"));
        assert_eq!(t(Spot::Arms), colour("leather_jacket"), "nothing is listed at the arms, the jacket reaches them");
        assert_eq!(t(Spot::Legs), colour("jeans"), "not the boots that reach up them");
        assert_eq!(t(Spot::Feet), colour("work_boots"));
        assert_eq!(t(Spot::Hands), colour("work_gloves"));
        assert_eq!(t(Spot::Head), colour("baseball_cap"), "the hood is down under a hat");
        assert_eq!(t(Spot::Face), colour("glasses"));
        assert_eq!(t(Spot::Neck), colour("scarf"));
        assert_eq!((t(Spot::Hand), t(Spot::OffHand), t(Spot::Back), t(Spot::Shoulder)), (colour("crowbar"), colour("flashlight"), colour("hiking_pack"), colour("tote_bag")));
        let hooded = Inventory { worn: vec![stack(&items, "hoodie")], ..default() };
        assert_eq!(tint(&items, &hooded, Spot::Head), colour("hoodie"), "the hood is up over a bare head");
        assert_eq!(tint(&items, &hooded, Spot::Legs), None);
        let bare = Inventory::default();
        for spot in Spot::LEFT.into_iter().chain(Spot::RIGHT) {
            assert_eq!(tint(&items, &bare, spot), None, "{spot:?}");
        }
    }

    /// The view drawn in a headless app and clicked through Bevy's picking: a click on a thing in
    /// the hand asks to put it in the bags, a right-click on something worn opens its menu (take
    /// off first), Ctrl-click picks it; every held and worn stack was listed.
    #[test]
    fn its_tiles_take_the_lists_clicks() {
        use super::super::{Clicks, ItemMenu, Listed, Outbox, drain_clicks, menu_clicks};
        use zombye_sim::Slot;
        use bevy::input::ButtonState;
        use bevy::input::mouse::MouseButtonInput;
        use bevy::window::{CursorMoved, PrimaryWindow, WindowEvent};
        use zombye_net::Intent;

        let items = shipped_items();
        let inv = dressed(&items);
        let mut app = super::super::tests::ui_app();
        app.init_resource::<Clicks>().init_resource::<Picked>().init_resource::<Listed>();
        app.world_mut().spawn(Camera2d);
        let mut listed = Vec::new();
        {
            let (flight, find, picked, ui) = (InFlight::default(), Find::default(), Picked::default(), PanelUi::default());
            let ctx = Ctx { items: &items, now: 0, me: 7, flight: &flight, find: &find, picked: &picked, ui: &ui, view: None, open: None };
            let world = app.world_mut();
            let mut commands = world.commands();
            commands.spawn(Node { position_type: PositionType::Absolute, top: px(0), left: px(0), width: px(384), flex_direction: FlexDirection::Column, ..default() }).with_children(|col| draw(&ctx, &mut listed, col, &inv));
            world.flush();
        }
        assert_eq!(listed.len(), 4 + inv.worn.len(), "every held and worn stack is drawn");
        for _ in 0..3 {
            app.update();
        }
        let window = app.world_mut().query_filtered::<Entity, With<PrimaryWindow>>().single(app.world()).unwrap();
        // The tiles, top to bottom then left to right: the cap, the glasses, the torso's three ...
        let tiles: Vec<Vec2> = {
            let mut q = app.world_mut().query::<(&UiGlobalTransform, &BackgroundColor)>();
            let mut v: Vec<Vec2> = q.iter(app.world()).filter(|(_, bg)| bg.0 == TILE).map(|(t, _)| t.translation).collect();
            v.sort_by(|a, b| (a.x, a.y).partial_cmp(&(b.x, b.y)).unwrap());
            v
        };
        assert_eq!(tiles.len(), listed.len());
        let (left, right): (Vec<Vec2>, Vec<Vec2>) = tiles.iter().partition(|t| t.x < 192.0);
        // Left: cap, glasses, jacket, hoodie, T-shirt, crowbar, jeans. Right: scarf, pack, tote, flashlight, gloves, boots.
        assert_eq!((left.len(), right.len()), (7, 6));
        let click = |app: &mut App, at: Vec2, button: MouseButton, modifier: Option<KeyCode>| {
            if let Some(m) = modifier {
                app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(m);
            }
            let moved = CursorMoved { window, position: at, delta: None };
            app.world_mut().write_message(moved.clone());
            app.world_mut().write_message(WindowEvent::CursorMoved(moved));
            app.update();
            for state in [ButtonState::Pressed, ButtonState::Released] {
                let e = MouseButtonInput { button, state, window };
                app.world_mut().write_message(e);
                app.world_mut().write_message(WindowEvent::MouseButtonInput(e));
                app.update();
            }
            if let Some(m) = modifier {
                app.world_mut().resource_mut::<ButtonInput<KeyCode>>().release(m);
            }
            app.update();
        };
        // The crowbar in the hand: a click puts it in the bags.
        click(&mut app, left[5], MouseButton::Left, None);
        let mut clicks = Clicks(std::mem::take(&mut app.world_mut().resource_mut::<Clicks>().0));
        assert_eq!(clicks.0.len(), 1);
        let (mut flight, mut out) = (InFlight::default(), Outbox::default());
        drain_clicks(7, 0.0, &mut clicks, &mut flight, &mut out);
        assert!(matches!(out.0[..], [Intent::Unequip { player: 7, from: Place::Slot(Slot::Hand), index: 0 }]), "{:?}", out.0);
        // The jacket: a right-click opens its menu, "Take off" first.
        click(&mut app, left[2], MouseButton::Right, None);
        let mut clicks = Clicks(std::mem::take(&mut app.world_mut().resource_mut::<Clicks>().0));
        let mut menu = ItemMenu::default();
        menu_clicks(&mut clicks, &mut menu, None);
        let m = menu.open.as_ref().expect("a menu");
        assert_eq!(m.title, "Leather jacket");
        assert_eq!(m.entries[0].label, "Take off");
        // The boots: Ctrl-click picks them, and sends nothing.
        click(&mut app, right[5], MouseButton::Left, Some(KeyCode::ControlLeft));
        let boots = stack(&items, "work_boots").item;
        assert_eq!(app.world().resource::<Picked>().keys, vec![(Place::Worn, 3, boots)]);
        assert!(app.world().resource::<Clicks>().0.is_empty());
    }
}
