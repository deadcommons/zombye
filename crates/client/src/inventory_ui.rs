//! The inventory panel (Tab or I): what you carry, hold and wear with your load against the
//! carry limit, beside the container you have open (search one with E) and the floor within reach.
//! Click a row to move it: from you into the open container (or onto the floor if none is open),
//! from a container or the floor into your pockets and bags, off a slot into your bags.
//! Right-click one of your carried stacks to equip, wear, eat or drink it, take a pill, or use a
//! dressing, antiseptic or sewing kit on a wound (it opens the health panel to pick the wound,
//! #10). Shift-click moves just one of a stack. Everything goes to the server as intents; the panel only shows what the latest
//! snapshot said. Moving takes a moment (the server queues it): the header shows how busy you are.
//! The last answer to a request (done, or why not) shows at the foot of the panel, and for a few
//! seconds under the HUD when the panel is shut (the same answer again just stays up longer).
//!
//! **One click, one request.** A press on a row is only recorded by its observer ([`Clicks`]);
//! [`send_clicks`] turns at most one press per button per frame into an intent, and not one for
//! a stack that is already on its way ([`InFlight`]: what was asked, until the snapshot shows the
//! answer, a refusal comes, or 3 s pass). A floor item being taken drops out of the Floor column
//! at once, and the other rows being moved say so.
//!
//! **The floor** is the server's: the ground items the snapshot says this player can reach
//! (`Snapshot::floor`, by the same rule the server checks moves with), not a guess from distance.
//!
//! **Make** (#12): under the columns, every recipe; the ones you have the inputs and tools for
//! (by the server's own rule, `crafting::check`, over our private view) can be clicked to make
//! them, the rest say what's missing. The one under way shows the seconds it has left (our queue is
//! in the private view), and queued ones how many.

use bevy::prelude::*;
use zombye_net::{Intent, PrivateView};
use zombye_sim::{Burden, ItemCategory, ItemDef, ItemStack, JobKind, Place, Slot, Treatment, crafting};

use crate::player::LocalPlayer;
use crate::server_link::{ItemNotesThisFrame, JoinInfo, ServerLink};

/// Is the panel open?
#[derive(Resource, Default)]
pub struct InventoryPanel {
    pub open: bool,
}

/// The latest answer to an item request, and when it came (seconds).
#[derive(Resource, Default)]
struct LastNote {
    text: String,
    ok: bool,
    at: f32,
}

#[derive(Component)]
struct PanelRoot;
#[derive(Component)]
struct NoteLine;

/// What the panel shows, to rebuild it only when that changes.
#[derive(Resource, Default, PartialEq)]
struct Shown(String);

const TEXT: Color = Color::srgb(0.93, 0.9, 0.82);
const DIM: Color = Color::srgb(0.65, 0.63, 0.58);
const ROW: Color = Color::srgba(0.12, 0.12, 0.14, 0.85);
const ROW_HOVER: Color = Color::srgba(0.28, 0.26, 0.2, 0.95);
const OK: Color = Color::srgb(0.95, 0.85, 0.35);
const NO: Color = Color::srgb(0.95, 0.5, 0.35);
const NOTE_SECS: f32 = 4.0;
/// A request with no answer after this long is forgotten (and its row can be clicked again).
const FLIGHT_SECS: f32 = 3.0;

/// One press on a row, as its observer saw it; [`send_clicks`] decides what it sends.
#[derive(Clone, Debug)]
struct RowClick {
    action: RowAction,
    button: PointerButton,
    /// Shift was down: just one of the stack.
    one: bool,
}

/// Presses on rows since [`send_clicks`] last ran.
#[derive(Resource, Default)]
struct Clicks(Vec<RowClick>);

/// Item intents from the panel for the server, sent on by [`flush`].
#[derive(Resource, Default)]
struct Outbox(Vec<Intent>);

/// A request the panel sent: `n` of the stack of `item` at `from`/`index`, at time `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Asked {
    from: Place,
    index: u32,
    item: u16,
    n: u32,
    at: f32,
}

/// What the panel has asked for and not yet seen answered. The answer is the snapshot's view
/// changing (what you have, the open container, your floor), a refusal, or [`FLIGHT_SECS`] going by;
/// any of them forgets everything asked (indices may have moved; the server drops repeats of a
/// request still queued, so a second request after that costs nothing).
#[derive(Resource, Default)]
struct InFlight {
    asked: Vec<Asked>,
    /// Recipes asked to be made, and when.
    crafts: Vec<(u16, f32)>,
    /// The view the requests were made against.
    content: String,
}

impl InFlight {
    /// How many of the stack at `from`/`index` are asked for already.
    fn asked_for(&self, from: Place, index: u32, item: u16) -> u32 {
        self.asked.iter().filter(|a| a.from == from && a.index == index && a.item == item).map(|a| a.n).sum()
    }

    /// Ask for all of `stack` at `from`/`index` (or one, `one`). False, asking nothing, when that
    /// many are on their way already.
    fn ask(&mut self, from: Place, index: u32, stack: ItemStack, one: bool, now: f32) -> bool {
        let have = stack.count.max(1) as u32;
        let before = self.asked_for(from, index, stack.item);
        if before >= have {
            return false;
        }
        let n = if one { 1 } else { have - before };
        self.asked.push(Asked { from, index, item: stack.item, n, at: now });
        true
    }

    /// Ask for recipe `recipe` to be made. False, asking nothing, when it's on its way already.
    fn ask_craft(&mut self, recipe: u16, now: f32) -> bool {
        if self.crafts.iter().any(|(r, _)| *r == recipe) {
            return false;
        }
        self.crafts.push((recipe, now));
        true
    }

    /// The latest view (`content`), whether a refusal just came, and the time.
    fn update(&mut self, content: String, refused: bool, now: f32) {
        if content != self.content || refused {
            self.asked.clear();
            self.crafts.clear();
            self.content = content;
        }
        self.asked.retain(|a| now - a.at < FLIGHT_SECS);
        self.crafts.retain(|(_, at)| now - at < FLIGHT_SECS);
    }
}

pub struct InventoryUiPlugin;

impl Plugin for InventoryUiPlugin {
    fn build(&self, app: &mut App) {
        let open = app.world().get_resource::<crate::Options>().is_some_and(|o| o.inventory);
        app.insert_resource(InventoryPanel { open })
            .init_resource::<LastNote>()
            .init_resource::<Shown>()
            .init_resource::<Clicks>()
            .init_resource::<Outbox>()
            .init_resource::<InFlight>()
            .add_systems(Startup, spawn)
            .add_systems(Update, (toggle, keep_note, track_flight, send_clicks, flush, rebuild).chain());
    }
}

fn font(size: f32) -> TextFont {
    TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }
}

fn spawn(mut commands: Commands) {
    commands.spawn((
        PanelRoot,
        Visibility::Hidden,
        Node {
            position_type: PositionType::Absolute,
            top: px(70),
            right: px(16),
            flex_direction: FlexDirection::Column,
            row_gap: px(6),
            padding: UiRect::all(px(10)),
            max_height: Val::Percent(85.0),
            overflow: Overflow::clip_y(),
            ..default()
        },
        BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.8)),
    ));
    commands.spawn((
        NoteLine,
        Text::new(""),
        font(15.0),
        TextColor(OK),
        Node { position_type: PositionType::Absolute, bottom: px(150), left: px(18), ..default() },
    ));
}

fn toggle(keys: Res<ButtonInput<KeyCode>>, mut panel: ResMut<InventoryPanel>) {
    if keys.just_pressed(KeyCode::Tab) || keys.just_pressed(KeyCode::KeyI) {
        panel.open = !panel.open;
    }
}

fn keep_note(time: Res<Time>, notes: Res<ItemNotesThisFrame>, panel: Res<InventoryPanel>, mut last: ResMut<LastNote>, mut line: Query<(&mut Text, &mut TextColor), With<NoteLine>>) {
    let now = time.elapsed_secs();
    for n in &notes.0 {
        // The same answer again (E pressed over and over where there's nothing): keep it up, once.
        let again = n.text == last.text && n.ok == last.ok && now - last.at < NOTE_SECS;
        *last = LastNote { text: n.text.clone(), ok: n.ok, at: now };
        if !again {
            println!("item: {}{}", if n.ok { "" } else { "refused: " }, n.text);
        }
    }
    let Ok((mut t, mut c)) = line.single_mut() else { return };
    let show = !panel.open && !last.text.is_empty() && time.elapsed_secs() - last.at < NOTE_SECS;
    let want = if show { last.text.clone() } else { String::new() };
    if t.0 != want {
        t.0 = want;
    }
    c.0 = if last.ok { OK } else { NO };
}

/// "Loaf of bread x2 (stale, 2/3 left)": the name, how many, and for food and drink how fresh it
/// is at game minute `now` and the goes left of the top one (#11).
fn name_of(items: &[ItemDef], now: i32, s: &ItemStack) -> String {
    let Some(def) = items.get(s.item as usize) else { return "?".into() };
    let mut out = if s.count > 1 { format!("{} x{}", def.name, s.count) } else { def.name.clone() };
    let mut notes = Vec::new();
    if let Some(f) = zombye_sim::needs::freshness(def, s, now).filter(|f| *f != zombye_sim::Freshness::Fresh) {
        notes.push(f.name().to_string());
    }
    if let Some(c) = def.consumable().filter(|_| s.used > 0) {
        notes.push(format!("{}/{} left", c.uses - s.used, c.uses));
    }
    if !notes.is_empty() {
        out.push_str(&format!(" ({})", notes.join(", ")));
    }
    out
}

fn stack_kg(items: &[ItemDef], s: &ItemStack) -> f32 {
    items.get(s.item as usize).map_or(0.0, |d| d.weight_kg * s.count as f32)
}

/// What a click on a row sends: left (whole stack, or one with Shift) and right (always about
/// one of a carried stack). `stack` is what the row shows.
#[derive(Clone, Debug)]
struct RowAction {
    stack: ItemStack,
    left: Option<(Place, u32, Place)>,
    right: Option<Intent>,
    /// Right-click: use this item on a wound (the health panel picks which).
    use_on: Option<u16>,
    /// Left click: make this recipe (a row of the Make list; `stack` and `left` are unused).
    craft: Option<u16>,
}

/// What the panel shows and the requests are checked against: our private view and our floor
/// (ground item id and stack, nearest first), from the latest snapshot.
fn panel_view(link: &ServerLink, me: u32) -> (Option<PrivateView>, Vec<(u32, ItemStack)>) {
    let floor = link.curr.floor_of(me).iter().filter_map(|g| link.curr.items.iter().find(|it| it.id == *g)).map(|it| (it.id, it.stack)).collect();
    (link.own(me).cloned(), floor)
}

fn track_flight(time: Res<Time>, link: Res<ServerLink>, me: Res<LocalPlayer>, notes: Res<ItemNotesThisFrame>, mut flight: ResMut<InFlight>) {
    let (view, floor) = panel_view(&link, me.id);
    flight.update(format!("{view:?}|{floor:?}"), notes.0.iter().any(|n| !n.ok), time.elapsed_secs());
}

/// The intent a click sends, if any (`None`: nothing for that button, or already on its way).
fn click_intent(click: &RowClick, player: u32, flight: &mut InFlight, now: f32) -> Option<Intent> {
    let a = &click.action;
    match click.button {
        PointerButton::Primary => {
            if let Some(recipe) = a.craft {
                return flight.ask_craft(recipe, now).then_some(Intent::Craft { player, recipe });
            }
            let (from, index, to) = a.left?;
            if !flight.ask(from, index, a.stack, click.one, now) {
                return None;
            }
            let count = if click.one { 1 } else { 0 };
            Some(match (from, to) {
                (Place::Slot(_) | Place::Worn, Place::Carried) => Intent::Unequip { player, from, index },
                (_, Place::Ground) => Intent::Drop { player, from, index, count },
                _ => Intent::MoveItem { player, from, index, to, count },
            })
        }
        PointerButton::Secondary => {
            let intent = a.right.clone()?;
            let index = match intent {
                Intent::Use { index, .. } | Intent::Wear { index, .. } | Intent::Equip { index, .. } => index,
                _ => return Some(intent),
            };
            flight.ask(Place::Carried, index, a.stack, true, now).then_some(intent)
        }
        _ => None,
    }
}

/// This frame's presses on rows, as intents: at most one per button (one physical click is one
/// press, however many times picking reports it), none for what is already on its way.
fn send_clicks(time: Res<Time>, me: Res<LocalPlayer>, mut clicks: ResMut<Clicks>, mut flight: ResMut<InFlight>, mut out: ResMut<Outbox>, mut health: ResMut<crate::health_ui::HealthPanel>) {
    // "Use on…": the health panel opens to pick the wound.
    if let Some(item) = clicks.0.iter().find(|c| c.button == PointerButton::Secondary).and_then(|c| c.action.use_on) {
        (health.open, health.use_item, health.selected) = (true, Some(item), None);
    }
    drain_clicks(me.id, time.elapsed_secs(), &mut clicks, &mut flight, &mut out);
}

fn drain_clicks(player: u32, now: f32, clicks: &mut Clicks, flight: &mut InFlight, out: &mut Outbox) {
    let mut buttons: Vec<PointerButton> = Vec::new();
    for click in clicks.0.drain(..) {
        if buttons.contains(&click.button) {
            continue;
        }
        buttons.push(click.button);
        out.0.extend(click_intent(&click, player, flight, now));
    }
}

fn flush(mut out: ResMut<Outbox>, mut link: ResMut<ServerLink>) {
    for intent in out.0.drain(..) {
        link.send(intent);
    }
}

#[allow(clippy::too_many_arguments)]
fn rebuild(
    mut commands: Commands,
    time: Res<Time>,
    panel: Res<InventoryPanel>,
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    me: Res<LocalPlayer>,
    last: Res<LastNote>,
    flight: Res<InFlight>,
    mut shown: ResMut<Shown>,
    mut root: Query<(Entity, &mut Visibility), With<PanelRoot>>,
) {
    let Ok((root, mut vis)) = root.single_mut() else { return };
    let want = if panel.open { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != want {
        *vis = want;
    }
    if !panel.open {
        return;
    }
    let (view, mut floor) = panel_view(&link, me.id);
    // What's being taken off the floor is gone from it as far as we're concerned.
    floor.retain(|(id, s)| flight.asked_for(Place::Ground, *id, s.item) < s.count as u32);
    let busy = link.curr.players.iter().find(|p| p.id == me.id).map_or((0.0, 0), |p| (p.busy, p.jobs));
    let note_fresh = time.elapsed_secs() - last.at < NOTE_SECS * 2.0;
    // (The game hour too: food goes off as time passes.)
    let now = link.curr.game_minute;
    let signature = format!("{view:?}|{floor:?}|{:.1}|{}|{}|{}|{:?}|{:?}|{}", busy.0, busy.1, if note_fresh { last.text.as_str() } else { "" }, me.dead, flight.asked, flight.crafts, now / 60);
    if shown.0 == signature {
        return;
    }
    shown.0 = signature;
    commands.entity(root).despawn_children();
    let items = &join.items;
    let open = view.as_ref().and_then(|v| v.open.as_ref().map(|c| c.id));
    commands.entity(root).with_children(|c| {
        header(c, view.as_ref(), busy);
        c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(12), align_items: AlignItems::FlexStart, ..default() }).with_children(|cols| {
            // You.
            column(cols, "You", |col| {
                let Some(v) = &view else { return };
                let inv = &v.inventory;
                for (label, slot) in [("Hand", Slot::Hand), ("Other hand", Slot::OffHand), ("Back", Slot::Back), ("Shoulder", Slot::Shoulder)] {
                    match inv.slot(slot) {
                        Some(s) => row(col, format!("{label}: {}{}", name_of(items, now, &s), moving(&flight, Place::Slot(slot), 0, &s)), format!("{:.1} kg", stack_kg(items, &s)), RowAction { stack: s, left: Some((Place::Slot(slot), 0, Place::Carried)), right: None, use_on: None, craft: None }),
                        None => dim(col, format!("{label}: -")),
                    }
                }
                for (k, s) in inv.worn.iter().enumerate() {
                    row(col, format!("Worn: {}{}", name_of(items, now, s), moving(&flight, Place::Worn, k as u32, s)), format!("{:.1} kg", stack_kg(items, s)), RowAction { stack: *s, left: Some((Place::Worn, k as u32, Place::Carried)), right: None, use_on: None, craft: None });
                }
                dim(col, format!("Carried ({:.1} of {:.0} L):", v.carry.used_l, v.carry.room_l));
                if inv.carried.is_empty() {
                    dim(col, "  nothing".into());
                }
                for (k, s) in inv.carried.iter().enumerate() {
                    let def = items.get(s.item as usize);
                    let to = open.map_or(Place::Ground, Place::Container);
                    let treats = def.and_then(Treatment::of_item);
                    let right = def.and_then(|d| match (d.category, treats) {
                        // A pill: take it. A dressing, antiseptic or sewing kit: use it on a wound.
                        (_, Some(t)) if !t.on_wound() => Some(Intent::Treat { player: me.id, wound: None, action: t, item: Some(s.item) }),
                        (_, Some(_)) => None,
                        (ItemCategory::Food | ItemCategory::Drink, _) => Some(Intent::Use { player: me.id, from: Place::Carried, index: k as u32 }),
                        (ItemCategory::Clothing, _) => Some(Intent::Wear { player: me.id, index: k as u32 }),
                        _ => Some(Intent::Equip { player: me.id, index: k as u32 }),
                    });
                    let use_on = treats.filter(|t| t.on_wound()).map(|_| s.item);
                    // Uses left in the opened one (a box of plasters, a roll of gauze).
                    let left = match (def.and_then(|d| d.medical.as_ref()), v.health.aid.opened.iter().find(|o| o.item == s.item)) {
                        (Some(m), Some(o)) => format!(" ({} of {} left)", m.uses.saturating_sub(o.used), m.uses),
                        _ => String::new(),
                    };
                    row(col, format!("  {}{left}{}", name_of(items, now, s), moving(&flight, Place::Carried, k as u32, s)), format!("{:.1} kg", stack_kg(items, s)), RowAction { stack: *s, left: Some((Place::Carried, k as u32, to)), right, use_on, craft: None });
                }
            });
            // The open container.
            if let Some(cv) = view.as_ref().and_then(|v| v.open.as_ref()) {
                let used: f32 = cv.stacks.iter().map(|s| items.get(s.item as usize).map_or(0.0, |d| d.volume_l * s.count as f32)).sum();
                column(cols, &format!("{} ({used:.1} of {:.0} L)", cv.name, cv.room_l), |col| {
                    if cv.stacks.is_empty() {
                        dim(col, "empty".into());
                    }
                    for (k, s) in cv.stacks.iter().enumerate() {
                        row(col, format!("{}{}", name_of(items, now, s), moving(&flight, Place::Container(cv.id), k as u32, s)), format!("{:.1} kg", stack_kg(items, s)), RowAction { stack: *s, left: Some((Place::Container(cv.id), k as u32, Place::Carried)), right: None, use_on: None, craft: None });
                    }
                });
            }
            // The floor.
            column(cols, "Floor", |col| {
                if floor.is_empty() {
                    dim(col, "nothing within reach".into());
                }
                for (id, s) in &floor {
                    row(col, format!("{}{}", name_of(items, now, s), moving(&flight, Place::Ground, *id, s)), format!("{:.1} kg", stack_kg(items, s)), RowAction { stack: *s, left: Some((Place::Ground, *id, Place::Carried)), right: None, use_on: None, craft: None });
                }
                if open.is_none() {
                    dim(col, "E: search the cupboard,".into());
                    dim(col, "drawer or fridge you face".into());
                }
            });
        });
        if let Some(v) = &view {
            make(c, &join, v, busy.0, &flight);
        }
        c.spawn((Text::new("Click: move (a recipe: make it)   Shift-click: just one   Right-click: equip / wear / eat / use on a wound   Tab: close"), font(12.0), TextColor(DIM)));
        if note_fresh && !last.text.is_empty() {
            c.spawn((Text::new(last.text.clone()), font(14.0), TextColor(if last.ok { OK } else { NO })));
        }
    });
}

/// The Make list (#12): each recipe, clickable when we have what it takes, else what's missing;
/// the one under way with its seconds left (`busy`), queued ones counted.
fn make(c: &mut ChildSpawnerCommands, join: &JoinInfo, v: &PrivateView, busy: f32, flight: &InFlight) {
    if join.recipes.is_empty() {
        return;
    }
    let items = &join.items;
    c.spawn((Text::new("Make"), font(15.0), TextColor(TEXT)));
    for (k, r) in join.recipes.iter().enumerate() {
        let k = k as u16;
        let craft = JobKind::Craft { recipe: k };
        let queued = v.jobs.iter().filter(|j| **j == craft).count();
        let state = match (v.jobs.first() == Some(&craft), queued) {
            (true, 1) => format!("  (making: {busy:.1} s left)"),
            (true, n) => format!("  (making: {busy:.1} s left, {} more queued)", n - 1),
            (false, 0) if flight.crafts.iter().any(|(x, _)| *x == k) => "  (asked)".to_string(),
            (false, 0) => String::new(),
            (false, n) => format!("  ({n} queued)"),
        };
        let makes = r.outputs.iter().map(|o| crafting::amount(items, &o.item, o.count)).collect::<Vec<_>>().join(" + ");
        match crafting::check(r, &v.inventory, items) {
            Ok(()) => row(c, format!("{}{state}", r.name), format!("{makes}, {:.0} s", r.secs * join.skills.craft_time_mult(v.skill_level(zombye_sim::Skill::Handiwork))), RowAction { stack: ItemStack::one(0), left: None, right: None, use_on: None, craft: Some(k) }),
            // Under way with what it takes set aside already.
            Err(_) if queued > 0 => dim(c, format!("{}{state}", r.name)),
            Err(why) => dim(c, format!("{}: {why}", r.name)),
        }
    }
}

/// " (moving)" after a row some of which is on its way.
fn moving(flight: &InFlight, from: Place, index: u32, s: &ItemStack) -> &'static str {
    if flight.asked_for(from, index, s.item) > 0 { "  (moving)" } else { "" }
}

fn header(c: &mut ChildSpawnerCommands, view: Option<&PrivateView>, busy: (f32, u8)) {
    let Some(v) = view else {
        c.spawn((Text::new("Inventory"), font(18.0), TextColor(TEXT)));
        return;
    };
    let k = &v.carry;
    let (tag, color) = match k.burden {
        Burden::None => ("", TEXT),
        Burden::Burdened => ("  BURDENED: slower, tires faster", OK),
        Burden::Heavy => ("  HEAVY: slow, can't run", NO),
        Burden::Overloaded => ("  OVERLOADED: crawling, can't run or climb", Color::srgb(1.0, 0.3, 0.25)),
    };
    c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Baseline, ..default() }).with_children(|r| {
        r.spawn((Text::new(format!("{:.1} / {:.0} kg", k.weight_kg, k.limit_kg)), font(18.0), TextColor(color)));
        r.spawn((Text::new(format!("(nothing more past {:.0} kg){tag}", v.max_kg)), font(13.0), TextColor(if tag.is_empty() { DIM } else { color })));
    });
    // A weight bar: the limit is at the 1/2 mark of the hard cap.
    c.spawn((Node { width: px(300), height: px(8), ..default() }, BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)))).with_children(|b| {
        let share = (k.weight_kg / v.max_kg.max(0.1)).clamp(0.0, 1.0);
        b.spawn((Node { width: Val::Percent(share * 100.0), height: Val::Percent(100.0), ..default() }, BackgroundColor(color)));
    });
    if busy.1 > 0 {
        c.spawn((Text::new(format!("busy: {:.1} s ({} queued)", busy.0, busy.1)), font(13.0), TextColor(OK)));
    }
}

fn column(cols: &mut ChildSpawnerCommands, title: &str, body: impl FnOnce(&mut ChildSpawnerCommands)) {
    cols.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), min_width: px(200), ..default() }).with_children(|col| {
        col.spawn((Text::new(title.to_string()), font(15.0), TextColor(TEXT)));
        body(col);
    });
}

fn dim(col: &mut ChildSpawnerCommands, text: String) {
    col.spawn((Text::new(text), font(13.0), TextColor(DIM)));
}

/// A clickable row: `label` on the left, `kg` on the right. A press is only recorded here
/// ([`Clicks`]); [`send_clicks`] decides what it sends.
fn row(col: &mut ChildSpawnerCommands, label: String, kg: String, action: RowAction) {
    col.spawn((
        Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(10), padding: UiRect::axes(px(6), px(2)), ..default() },
        BackgroundColor(ROW),
    ))
    .with_children(|r| {
        r.spawn((Text::new(label), font(14.0), TextColor(TEXT), Pickable::IGNORE));
        r.spawn((Text::new(kg), font(12.0), TextColor(DIM), Pickable::IGNORE));
    })
    .observe(|ev: On<PointerOver>, mut q: Query<&mut BackgroundColor>| {
        if let Ok(mut bg) = q.get_mut(ev.entity) {
            bg.0 = ROW_HOVER;
        }
    })
    .observe(|ev: On<PointerOut>, mut q: Query<&mut BackgroundColor>| {
        if let Ok(mut bg) = q.get_mut(ev.entity) {
            bg.0 = ROW;
        }
    })
    .observe(move |ev: On<PointerPress>, keys: Res<ButtonInput<KeyCode>>, mut clicks: ResMut<Clicks>| {
        let one = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        clicks.0.push(RowClick { action: action.clone(), button: ev.button, one });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::mouse::MouseButtonInput;
    use bevy::window::{CursorMoved, PrimaryWindow, WindowEvent};

    fn beans(n: u16) -> ItemStack {
        ItemStack::new(3, n)
    }

    fn click(left: Option<(Place, u32, Place)>, stack: ItemStack, button: PointerButton, one: bool) -> RowClick {
        RowClick { action: RowAction { stack, left, right: None, use_on: None, craft: None }, button, one }
    }

    /// A stack asked for is asked for once; one at a time (Shift) as many times as there are;
    /// once the view changes (the answer came), it can be asked for again.
    #[test]
    fn a_stack_on_its_way_is_not_asked_for_again() {
        let mut f = InFlight::default();
        let take = Some((Place::Container(4), 0, Place::Carried));
        let c = click(take, beans(1), PointerButton::Primary, false);
        assert!(matches!(click_intent(&c, 1, &mut f, 0.0), Some(Intent::MoveItem { count: 0, .. })));
        assert!(click_intent(&c, 1, &mut f, 0.1).is_none(), "the same row again while it moves");
        f.update("same".into(), false, 0.2);
        f.update("same".into(), false, 0.3);
        // The snapshot changed: answered. (`update` first takes the content as it is.)
        f.update("after".into(), false, 0.4);
        assert!(click_intent(&c, 1, &mut f, 0.5).is_some());
        // Shift-clicks on three: three requests, then none.
        let mut f = InFlight::default();
        let one = click(take, beans(3), PointerButton::Primary, true);
        let sent = (0..5).filter(|k| click_intent(&one, 1, &mut f, *k as f32 * 0.01).is_some()).count();
        assert_eq!(sent, 3);
        // A refusal or a long silence forgets them.
        f.update(f.content.clone(), true, 0.1);
        assert!(click_intent(&one, 1, &mut f, 0.2).is_some());
        f.update(f.content.clone(), false, 0.2 + FLIGHT_SECS);
        assert!(f.asked.is_empty());
        // Dropping goes to the floor; a slot comes off.
        let mut f = InFlight::default();
        let drop = click(Some((Place::Carried, 2, Place::Ground)), beans(2), PointerButton::Primary, true);
        assert!(matches!(click_intent(&drop, 1, &mut f, 0.0), Some(Intent::Drop { index: 2, count: 1, .. })));
        let off = click(Some((Place::Slot(Slot::Hand), 0, Place::Carried)), beans(1), PointerButton::Primary, false);
        assert!(matches!(click_intent(&off, 1, &mut f, 0.0), Some(Intent::Unequip { .. })));
        assert!(click_intent(&off, 1, &mut f, 0.0).is_none());
    }

    /// A click on a recipe asks for it once until the answer comes (#12).
    #[test]
    fn a_recipe_is_asked_for_once() {
        let mut f = InFlight::default();
        let c = RowClick { action: RowAction { stack: ItemStack::one(0), left: None, right: None, use_on: None, craft: Some(2) }, button: PointerButton::Primary, one: false };
        assert!(matches!(click_intent(&c, 1, &mut f, 0.0), Some(Intent::Craft { player: 1, recipe: 2 })));
        assert!(click_intent(&c, 1, &mut f, 0.1).is_none(), "on its way");
        f.update("queued".into(), false, 0.2);
        assert!(click_intent(&c, 1, &mut f, 0.3).is_some(), "answered: another can be asked for");
        assert!(click_intent(&RowClick { button: PointerButton::Secondary, ..c }, 1, &mut f, 0.4).is_none(), "right-click does nothing");
    }

    /// The real input path, headless: Bevy's mouse input, UI layout and picking on a window with a
    /// panel row in it. One press of the button on the row is one recorded press and one intent,
    /// however many frames go by and even if the panel is rebuilt under the cursor meanwhile; a
    /// second press on the same row while its stack is on its way sends nothing more.
    #[test]
    fn one_click_on_a_row_is_one_intent() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::input::InputPlugin,
            bevy::window::WindowPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::image::ImagePlugin::default(),
            bevy::image::TextureAtlasPlugin,
            bevy::mesh::MeshPlugin,
            bevy::a11y::AccessibilityPlugin,
            bevy::input_focus::InputFocusPlugin,
            bevy::transform::TransformPlugin,
            bevy::camera::CameraPlugin,
            bevy::text::TextPlugin,
            bevy::ui::UiPlugin,
            bevy::picking::DefaultPickingPlugins,
        ));
        app.init_resource::<Clicks>().init_resource::<InFlight>().init_resource::<Outbox>().init_resource::<ButtonInput<KeyCode>>();
        // `send_clicks` as the plugin runs it, for player 7, without the server behind it.
        let player = 7u32;
        // How many presses picking reported on rows (what the old panel sent an intent for, each).
        #[derive(Resource, Default)]
        struct Presses(usize);
        app.init_resource::<Presses>();
        app.add_systems(Update, move |time: Res<Time>, mut presses: ResMut<Presses>, mut clicks: ResMut<Clicks>, mut flight: ResMut<InFlight>, mut out: ResMut<Outbox>| {
            presses.0 += clicks.0.len();
            drain_clicks(player, time.elapsed_secs(), &mut clicks, &mut flight, &mut out);
        });
        app.world_mut().spawn(Camera2d);
        let spawn_panel = |world: &mut World| {
            let mut commands = world.commands();
            commands.spawn(Node { position_type: PositionType::Absolute, top: px(0), left: px(0), flex_direction: FlexDirection::Column, ..default() }).with_children(|col| {
                row(col, "Alarm clock".into(), "0.4 kg".into(), RowAction { stack: beans(1), left: Some((Place::Container(4), 0, Place::Carried)), right: None, use_on: None, craft: None });
            });
            world.flush();
        };
        spawn_panel(app.world_mut());
        for _ in 0..3 {
            app.update();
        }
        let window = app.world_mut().query_filtered::<Entity, With<PrimaryWindow>>().single(app.world()).unwrap();
        let send = |app: &mut App, e: WindowEvent| {
            app.world_mut().write_message(e);
        };
        let press = |app: &mut App, state: ButtonState| {
            let e = MouseButtonInput { button: MouseButton::Left, state, window };
            app.world_mut().write_message(e);
            send(app, WindowEvent::MouseButtonInput(e));
        };
        let at = Vec2::new(20.0, 8.0);
        let moved = CursorMoved { window, position: at, delta: None };
        app.world_mut().write_message(moved.clone());
        send(&mut app, WindowEvent::CursorMoved(moved));
        app.update();
        app.update();
        press(&mut app, ButtonState::Pressed);
        app.update();
        // Hold it a few frames, then rebuild the row under the cursor (as a snapshot would) and let go.
        for _ in 0..3 {
            app.update();
        }
        let rows: Vec<Entity> = app.world_mut().query_filtered::<Entity, With<Node>>().iter(app.world()).collect();
        for e in rows {
            if let Ok(ec) = app.world_mut().get_entity_mut(e) {
                ec.despawn();
            }
        }
        spawn_panel(app.world_mut());
        app.update();
        press(&mut app, ButtonState::Released);
        for _ in 0..5 {
            app.update();
        }
        let out = &app.world().resource::<Outbox>().0;
        assert_eq!(out.len(), 1, "one click: {out:?}");
        assert_eq!(app.world().resource::<Presses>().0, 1, "picking reports one press per click");
        assert!(matches!(out[0], Intent::MoveItem { player: 7, from: Place::Container(4), index: 0, to: Place::Carried, count: 0 }));
        // Click it again before the answer: no second request.
        press(&mut app, ButtonState::Pressed);
        app.update();
        press(&mut app, ButtonState::Released);
        app.update();
        assert_eq!(app.world().resource::<Outbox>().0.len(), 1);
        assert_eq!(app.world().resource::<Presses>().0, 2);
        assert!(app.world().resource::<Clicks>().0.is_empty());
    }
}
