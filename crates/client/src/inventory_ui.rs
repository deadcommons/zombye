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
//! **Finding and moving several** (#94): a search box (`/` or a click to type, Enter or Esc to
//! stop; while typing, the game's keys are held back) and category chips narrow every column; each
//! column's title says where a click sends things; Ctrl-click picks rows, and the bar under the
//! columns moves every picked stack to your bags, the open container or the floor at once.
//! `--find TEXT`, `--filter CATEGORY` and `--pick N,N` set them from the start (for pictures).
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
const ROW_PICKED: Color = Color::srgba(0.36, 0.3, 0.12, 0.95);
const CHIP: Color = Color::srgba(0.16, 0.16, 0.18, 0.9);
const CHIP_ON: Color = Color::srgba(0.55, 0.45, 0.15, 0.95);
const OK: Color = Color::srgb(0.95, 0.85, 0.35);
const NO: Color = Color::srgb(0.95, 0.5, 0.35);
const NOTE_SECS: f32 = 4.0;
/// The panel's widest.
const PANEL_W: f32 = 880.0;
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

/// The kinds of thing the category chips pick out (several item categories to a chip).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Food,
    Drink,
    Medical,
    Weapons,
    Clothing,
    Tools,
    Bags,
    Other,
}

impl Group {
    const ALL: [Group; 8] = [Group::Food, Group::Drink, Group::Medical, Group::Weapons, Group::Clothing, Group::Tools, Group::Bags, Group::Other];

    fn of(c: ItemCategory) -> Group {
        match c {
            ItemCategory::Food => Group::Food,
            ItemCategory::Drink => Group::Drink,
            ItemCategory::Medical => Group::Medical,
            ItemCategory::Weapon => Group::Weapons,
            ItemCategory::Clothing => Group::Clothing,
            ItemCategory::Tool | ItemCategory::Light => Group::Tools,
            ItemCategory::Bag => Group::Bags,
            ItemCategory::Material | ItemCategory::Junk => Group::Other,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Group::Food => "Food",
            Group::Drink => "Drink",
            Group::Medical => "Medical",
            Group::Weapons => "Weapons",
            Group::Clothing => "Clothing",
            Group::Tools => "Tools",
            Group::Bags => "Bags",
            Group::Other => "Other",
        }
    }

    fn parse(s: &str) -> Option<Group> {
        Group::ALL.into_iter().find(|g| g.name().eq_ignore_ascii_case(s))
    }
}

/// What the columns show: names containing `query` (any case), of `group` if one is picked.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
pub struct Find {
    query: String,
    /// Typing into the search box (the game's keys are held back meanwhile).
    pub typing: bool,
    group: Option<Group>,
}

impl Find {
    fn shows(&self, items: &[ItemDef], s: &ItemStack) -> bool {
        let Some(def) = items.get(s.item as usize) else { return true };
        self.group.is_none_or(|g| Group::of(def.category) == g) && (self.query.is_empty() || def.name.to_lowercase().contains(&self.query.to_lowercase()))
    }
}

/// Stacks picked with Ctrl-click: where each is, its index there and its item (to tell it's still
/// the same stack after things move).
#[derive(Resource, Default, Debug, Clone, PartialEq)]
struct Picked(Vec<(Place, u32, u16)>);

/// Where the bar under the columns was asked to send every picked stack this frame.
#[derive(Resource, Default)]
struct SendPicked(Option<Place>);

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
        let options = app.world().get_resource::<crate::Options>().cloned();
        let open = options.as_ref().is_some_and(|o| o.inventory);
        let find = Find { query: options.as_ref().and_then(|o| o.find.clone()).unwrap_or_default(), typing: false, group: options.as_ref().and_then(|o| o.filter.as_deref()).and_then(Group::parse) };
        let picked = Picked(options.as_ref().map_or(Vec::new(), |o| o.pick.iter().map(|&k| (Place::Carried, k, u16::MAX)).collect()));
        app.insert_resource(InventoryPanel { open })
            .insert_resource(find)
            .insert_resource(picked)
            .init_resource::<SendPicked>()
            .init_resource::<LastNote>()
            .init_resource::<Shown>()
            .init_resource::<Clicks>()
            .init_resource::<Outbox>()
            .init_resource::<InFlight>()
            .init_resource::<MakeUi>()
            .add_systems(Startup, spawn)
            .add_systems(PreUpdate, type_search.in_set(TextEntry).after(bevy::input::InputSystems))
            .add_systems(Update, (toggle, keep_note, track_flight, prune_picked, send_clicks, send_picked, flush, rebuild).chain());
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
            max_width: px(PANEL_W),
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

/// Typing into the search box: before anything else reads the keys (order other `PreUpdate` key
/// handlers after it), so while it's on the keys only edit the search.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextEntry;

fn type_search(mut typed: MessageReader<bevy::input::keyboard::KeyboardInput>, mut keys: ResMut<ButtonInput<KeyCode>>, panel: Res<InventoryPanel>, mut find: ResMut<Find>) {
    use bevy::input::keyboard::Key;
    if !panel.open {
        find.typing = false;
        typed.clear();
        return;
    }
    if !find.typing {
        if keys.just_pressed(KeyCode::Slash) {
            find.typing = true;
            keys.reset_all();
        }
        typed.clear();
        return;
    }
    for k in typed.read().filter(|k| k.state.is_pressed()) {
        match &k.logical_key {
            Key::Character(c) => find.query.extend(c.chars().filter(|c| !c.is_control())),
            Key::Space => find.query.push(' '),
            Key::Backspace => {
                find.query.pop();
            }
            Key::Enter | Key::Escape => find.typing = false,
            _ => {}
        }
    }
    keys.reset_all();
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

/// The intent that moves `count` (0: all) of the stack at `from`/`index` to `to`, if it goes anywhere.
fn move_intent(player: u32, from: Place, index: u32, to: Place, count: u16) -> Option<Intent> {
    Some(match (from, to) {
        _ if from == to => return None,
        (Place::Slot(_) | Place::Worn, Place::Carried) => Intent::Unequip { player, from, index },
        (_, Place::Ground) => Intent::Drop { player, from, index, count },
        _ => Intent::MoveItem { player, from, index, to, count },
    })
}

/// Forget picked stacks that have moved or gone; picks from `--pick` (item not yet known) take the
/// item they find.
fn prune_picked(link: Res<ServerLink>, me: Res<LocalPlayer>, mut picked: ResMut<Picked>) {
    if picked.0.is_empty() {
        return;
    }
    let (Some(v), floor) = panel_view(&link, me.id) else { return };
    let at = |place: Place, index: u32| -> Option<u16> {
        let i = index as usize;
        match place {
            Place::Carried => v.inventory.carried.get(i).map(|s| s.item),
            Place::Worn => v.inventory.worn.get(i).map(|s| s.item),
            Place::Slot(slot) => v.inventory.slot(slot).map(|s| s.item),
            Place::Container(id) => v.open.as_ref().filter(|c| c.id == id).and_then(|c| c.stacks.get(i)).map(|s| s.item),
            Place::Ground => floor.iter().find(|(g, _)| *g == index).map(|(_, s)| s.item),
        }
    };
    let kept: Vec<(Place, u32, u16)> = picked.0.iter().filter_map(|&(p, i, item)| at(p, i).filter(|now| item == u16::MAX || item == *now).map(|now| (p, i, now))).collect();
    if kept != picked.0 {
        picked.0 = kept;
    }
}

/// The bar's "→ bags / → container / → floor": every picked stack there, then nothing picked.
fn send_picked(time: Res<Time>, me: Res<LocalPlayer>, link: Res<ServerLink>, mut send: ResMut<SendPicked>, mut picked: ResMut<Picked>, mut flight: ResMut<InFlight>, mut out: ResMut<Outbox>) {
    let Some(to) = send.0.take() else { return };
    let (view, floor) = panel_view(&link, me.id);
    let Some(v) = view else { return };
    let now = time.elapsed_secs();
    // From the back of each list first, so the indices of the ones still to go don't shift.
    let mut list = std::mem::take(&mut picked.0);
    list.sort_by_key(|&(_, i, _)| std::cmp::Reverse(i));
    for (from, index, item) in list {
        let stack = match from {
            Place::Carried => v.inventory.carried.get(index as usize).copied(),
            Place::Worn => v.inventory.worn.get(index as usize).copied(),
            Place::Slot(slot) => v.inventory.slot(slot),
            Place::Container(_) => v.open.as_ref().and_then(|c| c.stacks.get(index as usize).copied()),
            Place::Ground => floor.iter().find(|(g, _)| *g == index).map(|(_, s)| *s),
        };
        let Some(stack) = stack.filter(|s| s.item == item) else { continue };
        if let Some(intent) = move_intent(me.id, from, index, to, 0)
            && flight.ask(from, index, stack, false, now)
        {
            out.0.push(intent);
        }
    }
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
            move_intent(player, from, index, to, if click.one { 1 } else { 0 })
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
    find: Res<Find>,
    picked: Res<Picked>,
    mut make_ui: ResMut<MakeUi>,
    options: Option<Res<crate::Options>>,
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
    if !make_ui.loaded {
        make_ui.load(options.as_ref().map_or(std::path::Path::new("saves"), |o| o.saves.as_path()), me.id);
    }
    let (view, mut floor) = panel_view(&link, me.id);
    // What's being taken off the floor is gone from it as far as we're concerned.
    floor.retain(|(id, s)| flight.asked_for(Place::Ground, *id, s.item) < s.count as u32);
    let busy = link.curr.players.iter().find(|p| p.id == me.id).map_or((0.0, 0), |p| (p.busy, p.jobs));
    let note_fresh = time.elapsed_secs() - last.at < NOTE_SECS * 2.0;
    // (The game hour too: food goes off as time passes.)
    let now = link.curr.game_minute;
    let signature = format!("{view:?}|{floor:?}|{:.1}|{}|{}|{}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{:?}", busy.0, busy.1, if note_fresh { last.text.as_str() } else { "" }, me.dead, flight.asked, flight.crafts, now / 60, *find, picked.0, make_ui.open, make_ui.favs);
    if shown.0 == signature {
        return;
    }
    shown.0 = signature;
    commands.entity(root).despawn_children();
    let items = &join.items;
    let open = view.as_ref().and_then(|v| v.open.as_ref().map(|c| c.id));
    let open_name = view.as_ref().and_then(|v| v.open.as_ref().map(|c| c.name.to_lowercase()));
    let is_picked = |from: Place, index: u32, s: &ItemStack| picked.0.contains(&(from, index, s.item));
    let shows = |s: &ItemStack| find.shows(items, s);
    // A picked stack always shows, so what the bar will move is in view.
    let shown_at = |from: Place, index: u32, s: &ItemStack| shows(s) || is_picked(from, index, s);
    commands.entity(root).with_children(|c| {
        header(c, view.as_ref(), busy);
        toolbar(c, &find);
        c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(12), align_items: AlignItems::FlexStart, ..default() }).with_children(|cols| {
            // You.
            let mine_to = format!("click: to {}", open_name.as_deref().unwrap_or("floor"));
            column(cols, "You", &mine_to, |col| {
                let Some(v) = &view else { return };
                let inv = &v.inventory;
                for (label, slot) in [("Hand", Slot::Hand), ("Other hand", Slot::OffHand), ("Back", Slot::Back), ("Shoulder", Slot::Shoulder)] {
                    match inv.slot(slot) {
                        Some(s) => row(col, format!("{label}: {}{}", name_of(items, now, &s), moving(&flight, Place::Slot(slot), 0, &s)), format!("{:.1} kg", stack_kg(items, &s)), RowAction { stack: s, left: Some((Place::Slot(slot), 0, Place::Carried)), right: None, use_on: None, craft: None }, is_picked(Place::Slot(slot), 0, &s)),
                        None => dim(col, format!("{label}: -")),
                    }
                }
                for (k, s) in inv.worn.iter().enumerate().filter(|(k, s)| shown_at(Place::Worn, *k as u32, s)) {
                    row(col, format!("Worn: {}{}", name_of(items, now, s), moving(&flight, Place::Worn, k as u32, s)), format!("{:.1} kg", stack_kg(items, s)), RowAction { stack: *s, left: Some((Place::Worn, k as u32, Place::Carried)), right: None, use_on: None, craft: None }, is_picked(Place::Worn, k as u32, s));
                }
                dim(col, format!("Carried ({:.1} of {:.0} L):", v.carry.used_l, v.carry.room_l));
                if inv.carried.is_empty() {
                    dim(col, "  nothing".into());
                } else if !inv.carried.iter().any(shows) {
                    dim(col, "  nothing that matches".into());
                }
                for (k, s) in inv.carried.iter().enumerate().filter(|(k, s)| shown_at(Place::Carried, *k as u32, s)) {
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
                    row(col, format!("  {}{left}{}", name_of(items, now, s), moving(&flight, Place::Carried, k as u32, s)), format!("{:.1} kg", stack_kg(items, s)), RowAction { stack: *s, left: Some((Place::Carried, k as u32, to)), right, use_on, craft: None }, is_picked(Place::Carried, k as u32, s));
                }
            });
            // The open container.
            if let Some(cv) = view.as_ref().and_then(|v| v.open.as_ref()) {
                let used = zombye_sim::inventory::stacks_volume(items, &cv.stacks);
                column(cols, &format!("{} ({used:.1} of {:.0} L)", cv.name, cv.room_l), "click: to bags", |col| {
                    if cv.stacks.is_empty() {
                        dim(col, "empty".into());
                    } else if !cv.stacks.iter().any(shows) {
                        dim(col, "nothing that matches".into());
                    }
                    for (k, s) in cv.stacks.iter().enumerate().filter(|(k, s)| shown_at(Place::Container(cv.id), *k as u32, s)) {
                        row(col, format!("{}{}", name_of(items, now, s), moving(&flight, Place::Container(cv.id), k as u32, s)), format!("{:.1} kg", stack_kg(items, s)), RowAction { stack: *s, left: Some((Place::Container(cv.id), k as u32, Place::Carried)), right: None, use_on: None, craft: None }, is_picked(Place::Container(cv.id), k as u32, s));
                    }
                });
            }
            // The floor.
            column(cols, "Floor", "click: to bags", |col| {
                if floor.is_empty() {
                    dim(col, "nothing within reach".into());
                } else if !floor.iter().any(|(_, s)| shows(s)) {
                    dim(col, "nothing that matches".into());
                }
                for (id, s) in floor.iter().filter(|(id, s)| shown_at(Place::Ground, *id, s)) {
                    row(col, format!("{}{}", name_of(items, now, s), moving(&flight, Place::Ground, *id, s)), format!("{:.1} kg", stack_kg(items, s)), RowAction { stack: *s, left: Some((Place::Ground, *id, Place::Carried)), right: None, use_on: None, craft: None }, is_picked(Place::Ground, *id, s));
                }
                if open.is_none() {
                    dim(col, "E: search the cupboard,".into());
                    dim(col, "drawer or fridge you face".into());
                }
            });
        });
        if !picked.0.is_empty() {
            picked_bar(c, picked.0.len(), open.zip(open_name.clone()));
        }
        if let Some(v) = &view {
            make(c, &join, v, busy.0, &flight, &make_ui);
        }
        c.spawn((Text::new("Click: move (a recipe: make it; right-click pins it)   Shift-click: just one   Ctrl-click: pick several"), font(12.0), TextColor(DIM)));
        c.spawn((Text::new("Right-click: equip / wear / eat / use on a wound   /: search   Tab: close"), font(12.0), TextColor(DIM)));
        if note_fresh && !last.text.is_empty() {
            c.spawn((Text::new(last.text.clone()), font(14.0), TextColor(if last.ok { OK } else { NO })));
        }
    });
}

/// The Make list's own state (#96): which families are open and which recipes are pinned.
/// Pinned recipes are saved per player in the saves folder (`make-favourites-P.txt`, one recipe id a line).
#[derive(Resource, Default, Debug)]
struct MakeUi {
    /// Families (by their first output's item id) opened to their variants.
    open: Vec<String>,
    /// Pinned recipe ids.
    favs: Vec<String>,
    path: Option<std::path::PathBuf>,
    loaded: bool,
}

impl MakeUi {
    fn toggle_open(&mut self, key: &str) {
        match self.open.iter().position(|k| k == key) {
            Some(i) => {
                self.open.remove(i);
            }
            None => self.open.push(key.to_string()),
        }
    }

    fn toggle_fav(&mut self, id: &str) {
        match self.favs.iter().position(|k| k == id) {
            Some(i) => {
                self.favs.remove(i);
            }
            None => self.favs.push(id.to_string()),
        }
        if let Some(p) = &self.path {
            let mut lines: Vec<String> = self.favs.clone();
            lines.extend(self.open.iter().filter(|k| *k == "*").map(|k| format!("open:{k}")));
            let _ = std::fs::write(p, lines.join("\n"));
        }
    }

    fn load(&mut self, saves: &std::path::Path, player: u32) {
        let p = saves.join(format!("make-favourites-{player}.txt"));
        if let Ok(t) = std::fs::read_to_string(&p) {
            // A line `open:KEY` opens a family from the start (`open:*`: all), for pictures.
            for l in t.lines().map(str::trim).filter(|l| !l.is_empty()) {
                match l.strip_prefix("open:") {
                    Some(k) => self.open.push(k.to_string()),
                    None => self.favs.push(l.to_string()),
                }
            }
        }
        self.path = Some(p);
        self.loaded = true;
    }
}

/// What a click on a Make row does.
#[derive(Clone)]
enum MakeAct {
    Craft(u16),
    Family(String),
    None,
}

/// A row of the Make list: left click makes the recipe or opens the family; right click pins or
/// unpins the recipe.
fn make_row(c: &mut ChildSpawnerCommands, label: String, right: String, act: MakeAct, fav: Option<String>, color: Color) {
    let hover = !matches!(act, MakeAct::None);
    c.spawn((
        Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(10), padding: UiRect::axes(px(6), px(2)), ..default() },
        BackgroundColor(ROW),
    ))
    .with_children(|r| {
        r.spawn((Text::new(label), font(14.0), TextColor(color), Pickable::IGNORE));
        r.spawn((Text::new(right), font(12.0), TextColor(DIM), Pickable::IGNORE));
    })
    .observe(move |ev: On<PointerOver>, mut q: Query<&mut BackgroundColor>| {
        if hover && let Ok(mut bg) = q.get_mut(ev.entity) {
            bg.0 = ROW_HOVER;
        }
    })
    .observe(|ev: On<PointerOut>, mut q: Query<&mut BackgroundColor>| {
        if let Ok(mut bg) = q.get_mut(ev.entity) {
            bg.0 = ROW;
        }
    })
    .observe(move |ev: On<PointerPress>, mut clicks: ResMut<Clicks>, mut ui: ResMut<MakeUi>| match (ev.button, &act) {
        (PointerButton::Primary, MakeAct::Craft(k)) => {
            clicks.0.push(RowClick { action: RowAction { stack: ItemStack::one(0), left: None, right: None, use_on: None, craft: Some(*k) }, button: ev.button, one: false })
        }
        (PointerButton::Primary, MakeAct::Family(key)) => ui.toggle_open(key),
        (PointerButton::Secondary, _) => {
            if let Some(id) = &fav {
                ui.toggle_fav(id);
            }
        }
        _ => {}
    });
}

/// The Make list (#12, #96): recipes you can make first, families of variants (rags from a
/// T-shirt, a jumper, ...) folded into one row that opens, pinned ones on top (right-click to pin),
/// and what the rest are missing in plain words, dimmed. The one under way shows its seconds left
/// (`busy`), queued ones counted.
fn make(c: &mut ChildSpawnerCommands, join: &JoinInfo, v: &PrivateView, busy: f32, flight: &InFlight, ui: &MakeUi) {
    if join.recipes.is_empty() {
        return;
    }
    let items = &join.items;
    let mult = join.skills.craft_time_mult(v.skill_level(zombye_sim::Skill::Handiwork));
    // One recipe's row. `lead` is "* " for a pinned one.
    let recipe_row = |c: &mut ChildSpawnerCommands, k: usize, indent: &str, lead: &str| {
        let r = &join.recipes[k];
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
        let fav = Some(r.id.clone());
        match crafting::check(r, &v.inventory, items) {
            Ok(()) => make_row(c, format!("{indent}{lead}{}{state}", r.name), format!("{makes}, {:.0} s", r.secs * mult), MakeAct::Craft(k), fav, TEXT),
            // Under way with what it takes set aside already.
            Err(_) if queued > 0 => make_row(c, format!("{indent}{lead}{}{state}", r.name), String::new(), MakeAct::None, fav, DIM),
            Err(why) => make_row(c, format!("{indent}{lead}{}", r.name), why, MakeAct::None, fav, DIM),
        }
    };
    let can = |k: usize| crafting::check(&join.recipes[k], &v.inventory, items).is_ok() || v.jobs.contains(&JobKind::Craft { recipe: k as u16 });
    let pinned = |k: usize| ui.favs.contains(&join.recipes[k].id);
    c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Baseline, ..default() }).with_children(|t| {
        t.spawn((Text::new("Make"), font(15.0), TextColor(TEXT)));
        t.spawn((Text::new("click a group to open it; right-click a recipe to pin it (*)"), font(12.0), TextColor(DIM)));
    });
    // Pinned first, makeable ones leading.
    let mut favs: Vec<usize> = (0..join.recipes.len()).filter(|&k| pinned(k)).collect();
    favs.sort_by_key(|&k| !can(k));
    for k in favs {
        recipe_row(c, k, "", "* ");
    }
    // The rest in families by what they make first.
    let mut families: Vec<(String, Vec<usize>)> = Vec::new();
    for (k, r) in join.recipes.iter().enumerate().filter(|(k, _)| !pinned(*k)) {
        let key = r.outputs.first().map_or(r.id.clone(), |o| o.item.clone());
        match families.iter_mut().find(|(f, _)| *f == key) {
            Some((_, v)) => v.push(k),
            None => families.push((key, vec![k])),
        }
    }
    // Families with something to make come first (stable, so the file's order otherwise).
    families.sort_by_key(|(_, ks)| !ks.iter().any(|&k| can(k)));
    for (key, mut ks) in families {
        if ks.len() == 1 {
            recipe_row(c, ks[0], "", "");
            continue;
        }
        let ready = ks.iter().filter(|&&k| can(k)).count();
        let open = ui.open.contains(&key) || ui.open.iter().any(|k| k == "*");
        let name = items.iter().find(|d| d.id == key).map_or(key.clone(), |d| format!("{}s", d.name));
        let summary = if ready == 0 { format!("{} ways, none yet", ks.len()) } else { format!("{ready} of {} ways ready", ks.len()) };
        make_row(c, format!("{} {name}", if open { "v" } else { ">" }), summary, MakeAct::Family(key.clone()), None, if ready > 0 { TEXT } else { DIM });
        if open {
            ks.sort_by_key(|&k| !can(k));
            for k in ks {
                recipe_row(c, k, "    ", "");
            }
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
        // `busy.1` counts the job under way too.
        let more = if busy.1 > 1 { format!(" ({} more queued)", busy.1 - 1) } else { String::new() };
        c.spawn((Text::new(format!("busy: {:.1} s{more}", busy.0)), font(13.0), TextColor(OK)));
    }
}

/// A column: its title, where a click on one of its rows sends it, and its rows.
fn column(cols: &mut ChildSpawnerCommands, title: &str, sends: &str, body: impl FnOnce(&mut ChildSpawnerCommands)) {
    cols.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), min_width: px(200), ..default() }).with_children(|col| {
        col.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Baseline, ..default() }).with_children(|t| {
            t.spawn((Text::new(title.to_string()), font(15.0), TextColor(TEXT)));
            t.spawn((Text::new(sends.to_string()), font(12.0), TextColor(DIM)));
        });
        body(col);
    });
}

/// A small clickable label (a chip, a button); the caller observes its presses.
fn chip<'a>(parent: &'a mut ChildSpawnerCommands, text: String, on: bool) -> EntityCommands<'a> {
    let mut e = parent.spawn((Node { padding: UiRect::axes(px(6), px(2)), ..default() }, BackgroundColor(if on { CHIP_ON } else { CHIP })));
    e.with_children(|b| {
        b.spawn((Text::new(text), font(12.0), TextColor(TEXT), Pickable::IGNORE));
    });
    e
}

/// The search box and the category chips.
fn toolbar(c: &mut ChildSpawnerCommands, find: &Find) {
    c.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(4), row_gap: px(4), align_items: AlignItems::Center, max_width: px(PANEL_W - 20.0), ..default() }).with_children(|bar| {
        let text = match (find.query.is_empty(), find.typing) {
            (true, false) => "Search ( / )".to_string(),
            (_, true) => format!("Search: {}_", find.query),
            (false, false) => format!("Search: {}", find.query),
        };
        bar.spawn((Node { min_width: px(160), padding: UiRect::axes(px(6), px(2)), ..default() }, BackgroundColor(if find.typing { CHIP_ON } else { CHIP })))
            .with_children(|b| {
                b.spawn((Text::new(text), font(12.0), TextColor(if find.query.is_empty() && !find.typing { DIM } else { TEXT }), Pickable::IGNORE));
            })
            .observe(|_: On<PointerPress>, mut find: ResMut<Find>| find.typing = true);
        if !find.query.is_empty() {
            chip(bar, "x".into(), false).observe(|_: On<PointerPress>, mut find: ResMut<Find>| find.query.clear());
        }
        chip(bar, "All".into(), find.group.is_none()).observe(|_: On<PointerPress>, mut find: ResMut<Find>| find.group = None);
        for g in Group::ALL {
            chip(bar, g.name().into(), find.group == Some(g)).observe(move |_: On<PointerPress>, mut find: ResMut<Find>| {
                find.group = if find.group == Some(g) { None } else { Some(g) };
            });
        }
    });
}

/// What to do with the picked stacks: send them all to the bags, the open container or the floor.
fn picked_bar(c: &mut ChildSpawnerCommands, n: usize, open: Option<(u32, String)>) {
    c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(4), align_items: AlignItems::Center, ..default() }).with_children(|bar| {
        bar.spawn((Text::new(format!("{n} picked:")), font(13.0), TextColor(OK)));
        chip(bar, "to bags".into(), false).observe(|_: On<PointerPress>, mut send: ResMut<SendPicked>| send.0 = Some(Place::Carried));
        if let Some((id, name)) = open {
            chip(bar, format!("to {name}"), false).observe(move |_: On<PointerPress>, mut send: ResMut<SendPicked>| send.0 = Some(Place::Container(id)));
        }
        chip(bar, "to floor".into(), false).observe(|_: On<PointerPress>, mut send: ResMut<SendPicked>| send.0 = Some(Place::Ground));
        chip(bar, "clear".into(), false).observe(|_: On<PointerPress>, mut picked: ResMut<Picked>| picked.0.clear());
    });
}

fn dim(col: &mut ChildSpawnerCommands, text: String) {
    col.spawn((Text::new(text), font(13.0), TextColor(DIM)));
}

/// A clickable row: `label` on the left, `kg` on the right, highlighted if `picked`. A press is
/// only recorded here ([`Clicks`]); [`send_clicks`] decides what it sends. Ctrl-click picks it
/// (or unpicks it) instead.
fn row(col: &mut ChildSpawnerCommands, label: String, kg: String, action: RowAction, picked: bool) {
    let rest = if picked { ROW_PICKED } else { ROW };
    col.spawn((
        Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(10), padding: UiRect::axes(px(6), px(2)), ..default() },
        BackgroundColor(rest),
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
    .observe(move |ev: On<PointerOut>, mut q: Query<&mut BackgroundColor>| {
        if let Ok(mut bg) = q.get_mut(ev.entity) {
            bg.0 = rest;
        }
    })
    .observe(move |ev: On<PointerPress>, keys: Res<ButtonInput<KeyCode>>, mut clicks: ResMut<Clicks>, mut picked: Option<ResMut<Picked>>| {
        if keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight])
            && ev.button == PointerButton::Primary
            && let (Some((from, index, _)), Some(picked)) = (action.left, picked.as_mut())
        {
            let key = (from, index, action.stack.item);
            match picked.0.iter().position(|k| *k == key) {
                Some(i) => {
                    picked.0.remove(i);
                }
                None => picked.0.push(key),
            }
            return;
        }
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

    /// The search and the chips narrow by name (any case) and by category (#94).
    #[test]
    fn search_and_chips_narrow_the_lists() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let config = zombye_net::content::load_config(&data, 1).unwrap();
        let items = &config.items;
        let stack = |id: &str| ItemStack::one(items.iter().position(|d| d.id == id).unwrap() as u16);
        let (beans, rag, knife) = (stack("tinned_beans"), stack("rag"), stack("kitchen_knife"));
        let mut f = Find::default();
        assert!(f.shows(items, &beans) && f.shows(items, &rag));
        f.query = "BEAN".into();
        assert!(f.shows(items, &beans) && !f.shows(items, &rag));
        f.query.clear();
        f.group = Group::parse("weapons");
        assert!(f.shows(items, &knife) && !f.shows(items, &beans));
        f.group = Some(Group::Food);
        assert!(f.shows(items, &beans) && !f.shows(items, &knife));
    }

    /// Typing into the search box, headless through Bevy's input: `/` starts it, letters go into
    /// the search and are held back from the game (W doesn't stay pressed), Backspace and Enter work.
    #[test]
    fn typing_edits_the_search_and_holds_the_keys_back() {
        use bevy::input::ButtonState;
        use bevy::input::keyboard::{Key, KeyboardInput};
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::input::InputPlugin));
        app.insert_resource(InventoryPanel { open: true }).init_resource::<Find>();
        app.add_systems(PreUpdate, type_search.in_set(TextEntry).after(bevy::input::InputSystems));
        let key = |app: &mut App, code: KeyCode, logical: Key| {
            for state in [ButtonState::Pressed, ButtonState::Released] {
                app.world_mut().write_message(KeyboardInput { key_code: code, logical_key: logical.clone(), state, text: None, repeat: false, window: Entity::PLACEHOLDER });
                app.update();
            }
        };
        let ch = |c: &str| Key::Character(c.into());
        // Not typing yet: W is the game's.
        app.world_mut().write_message(KeyboardInput { key_code: KeyCode::KeyW, logical_key: ch("w"), state: ButtonState::Pressed, text: None, repeat: false, window: Entity::PLACEHOLDER });
        app.update();
        assert!(app.world().resource::<ButtonInput<KeyCode>>().pressed(KeyCode::KeyW));
        assert!(app.world().resource::<Find>().query.is_empty());
        app.world_mut().write_message(KeyboardInput { key_code: KeyCode::KeyW, logical_key: ch("w"), state: ButtonState::Released, text: None, repeat: false, window: Entity::PLACEHOLDER });
        app.update();
        key(&mut app, KeyCode::Slash, ch("/"));
        assert!(app.world().resource::<Find>().typing);
        for c in ["r", "a", "w", "g"] {
            app.world_mut().write_message(KeyboardInput { key_code: KeyCode::KeyW, logical_key: ch(c), state: ButtonState::Pressed, text: None, repeat: false, window: Entity::PLACEHOLDER });
            app.update();
            assert!(!app.world().resource::<ButtonInput<KeyCode>>().pressed(KeyCode::KeyW), "held back while typing");
        }
        key(&mut app, KeyCode::Backspace, Key::Backspace);
        key(&mut app, KeyCode::Backspace, Key::Backspace);
        key(&mut app, KeyCode::KeyG, ch("g"));
        assert_eq!(app.world().resource::<Find>().query, "rag");
        key(&mut app, KeyCode::Enter, Key::Enter);
        assert!(!app.world().resource::<Find>().typing);
        assert_eq!(app.world().resource::<Find>().query, "rag");
    }

    /// Where a move goes decides the intent: off a slot into the bags is taking it off, anything
    /// to the floor a drop, nowhere new nothing.
    #[test]
    fn moves_become_the_right_intents() {
        assert!(matches!(move_intent(1, Place::Slot(Slot::Hand), 0, Place::Carried, 0), Some(Intent::Unequip { .. })));
        assert!(matches!(move_intent(1, Place::Container(3), 2, Place::Ground, 0), Some(Intent::Drop { index: 2, .. })));
        assert!(matches!(move_intent(1, Place::Ground, 9, Place::Container(3), 0), Some(Intent::MoveItem { to: Place::Container(3), .. })));
        assert!(move_intent(1, Place::Carried, 0, Place::Carried, 0).is_none());
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
                row(col, "Alarm clock".into(), "0.4 kg".into(), RowAction { stack: beans(1), left: Some((Place::Container(4), 0, Place::Carried)), right: None, use_on: None, craft: None }, false);
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
