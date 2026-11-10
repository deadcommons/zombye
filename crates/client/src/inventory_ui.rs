//! The inventory panel (Tab or I): two panes side by side. Left, what you carry, with your load
//! against the carry limit; right, the container you have open (search one with E) or the floor
//! within reach. Slim tabs down each pane's outer edge pick what it shows: "Carried" or "Worn"
//! (what you hold and wear, slot by slot) on the left, the open container or "Floor" on the right
//! (a newly opened container comes to the front; the Worn tab can show the body view instead of
//! its list, [`body`], Linear DCM-43). Click a row to move it across: from you into
//! what the right pane shows (the container, or the floor), from a container or the floor into
//! your pockets and bags, off a slot into your bags. The `>>` and `<<` buttons between the panes
//! move everything a pane shows.
//!
//! **Rows** (Linear DCM-56) are one line with the same cells in every pane, so they line up: a
//! swatch of the item's colour, its name (and dimmed what there is to say: stale, goes left,
//! moving), its type, how many and their weight. The column headers sort both panes by name, type
//! or weight ([`Sort`]: a press, again for the other way round, a third time back to the order
//! things lie in). Several stacks of one item (things that don't stack, or more than a stack
//! holds) collapse into one row with their count and weight together ([`arrange`]); a click
//! moves them all, and the fold mark at its start opens it to its stacks, each a row of its own.
//! The panel has two text sizes: one for rows and everything small, one for titles.
//! Right-click any row for a small flat menu of what can be done with it (#95; [`menu_entries`]):
//! equip, wear, eat or drink, take a pill, use a dressing, antiseptic or sewing kit on a wound (it
//! opens the health panel to pick the wound, #10), put it in the open container, drop it, take it,
//! or split the stack (one, or half). The likeliest first; 1-9 pick by number, Esc or a click
//! elsewhere closes it. The same menu opens on a ground item in the world: right-click it with the
//! panel open, or tap the right mouse on it (a quick tap, not held to aim). Alt-click (Option-click on a Mac) moves just one of a stack. Everything goes to
//! the server as intents; the panel only shows what the latest
//! snapshot said. Moving takes a moment (the server queues it): the current action's bar
//! (`action_ui.rs`) shows how busy you are.
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
//! **Finding and moving several** (#94): a search box and category chips narrow both panes (the
//! chips are toggles: several can be on, "All" turns them off). They are out of sight until
//! wanted: the "Find" control at the top right (or `/`) brings them out, and they stay while a
//! search or a chip is set ([`Find::in_use`]); "Find" again puts them away, cleared. `/` or a
//! click on the box types (Enter or Esc to stop; while typing, the game's keys are held back).
//! Each pane's title has a "pick all" for the stacks it shows; Ctrl-click picks rows (a collapsed
//! row: all its stacks), Shift-click picks every row from the last one picked to it in that pane
//! (only rows on show: the search and chips narrow a range too), and with something picked the
//! top row says how many and sends every picked stack to your bags, the open container or the
//! floor at once. A name too long for its cell is cut off, never wrapped. `--find TEXT`,
//! `--filter CATEGORY[,CATEGORY]` and `--pick N,N` set them from the start (for pictures), as do
//! `--tab worn|floor`, `--sort name|type|weight` and `--expand` (collapsed rows opened);
//! `--pick-range A,B[@SECS]` Ctrl-clicks carried row A and Shift-clicks row B, and
//! `--drop-picked-after SECS` presses the move-to row's "floor" (for recordings).
//!
//! **Make** (#12): under the panes, one line saying how many things you can make now; a click
//! opens the list. Every recipe; the ones you have the inputs and tools for
//! (by the server's own rule, `crafting::check`, over our private view) can be clicked to make
//! them, the rest say what's missing. The one under way shows the seconds it has left (our queue is
//! in the private view), and queued ones how many.

use bevy::prelude::*;
use zombye_net::{Intent, PrivateView};
use zombye_sim::{Burden, ItemCategory, ItemDef, ItemStack, JobKind, Place, Slot, Treatment, crafting};

use crate::player::LocalPlayer;
use crate::server_link::{ItemNotesThisFrame, JoinInfo, ServerLink};

mod body;

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

use crate::theme::{self, DIM, INK, ROW, ROW_HOVER, ROW_PICKED, TEXT, font};

/// The fold mark's small backing, and the search box's (typing: lit).
const CHIP: Color = Color::srgba(1.0, 1.0, 1.0, 0.1);
const CHIP_ON: Color = theme::ROW_PICKED;
/// A pane's backing (set into the panel's sheet), a tab that isn't the one shown, and a hairline.
const PANE: Color = theme::INSET;
const TAB: Color = Color::srgba(1.0, 1.0, 1.0, 0.05);
const LINE: Color = theme::LINE;
const OK: Color = theme::GOLD;
const NO: Color = theme::BAD;
const NOTE_SECS: f32 = 4.0;
/// The panel's two text sizes: rows (and everything small), and headers.
const ROW_FONT: f32 = theme::BODY;
const HEAD_FONT: f32 = 18.0;
/// The tabs' labels and the column headers: the heading face, a size up from the rows (it is
/// condensed, so it reads smaller than the text face at the same size).
const TAB_FONT: f32 = 16.0;
/// A pane's width, a tab's, and a row's cells: the fold mark, the swatch, type, count and weight
/// (the name takes the rest).
const PANE_W: f32 = 400.0;
const TAB_W: f32 = 26.0;
const W_LEAD: f32 = 12.0;
const W_SWATCH: f32 = 10.0;
const W_KIND: f32 = 78.0;
const W_QTY: f32 = 28.0;
const W_KG: f32 = 40.0;
const CELL_GAP: f32 = 6.0;
/// The panel's widest: two panes with their tabs, the move-all buttons between, the padding.
/// (918: on a 1280 screen it stops short of the health panel on the left, which opens beside it
/// to use something on a wound.)
const PANEL_W: f32 = 2.0 * (PANE_W + TAB_W) + 42.0 + 2.0 * theme::SHEET_PAD.0;
/// A request with no answer after this long is forgotten (and its row can be clicked again).
const FLIGHT_SECS: f32 = 3.0;

/// One press on a row, as its observer saw it; [`send_clicks`] decides what it sends.
#[derive(Clone, Debug)]
struct RowClick {
    action: RowAction,
    button: PointerButton,
    /// Alt (Option on a Mac) was down: just one of the stack.
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
    Materials,
    Bags,
    Other,
}

impl Group {
    const ALL: [Group; 9] = [Group::Food, Group::Drink, Group::Medical, Group::Weapons, Group::Clothing, Group::Tools, Group::Materials, Group::Bags, Group::Other];

    fn of(c: ItemCategory) -> Group {
        match c {
            ItemCategory::Food => Group::Food,
            ItemCategory::Drink => Group::Drink,
            ItemCategory::Medical => Group::Medical,
            ItemCategory::Weapon => Group::Weapons,
            ItemCategory::Clothing => Group::Clothing,
            ItemCategory::Tool | ItemCategory::Light => Group::Tools,
            ItemCategory::Material => Group::Materials,
            ItemCategory::Bag => Group::Bags,
            ItemCategory::Junk => Group::Other,
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
            Group::Materials => "Materials",
            Group::Bags => "Bags",
            Group::Other => "Other",
        }
    }

    fn parse(s: &str) -> Option<Group> {
        Group::ALL.into_iter().find(|g| g.name().eq_ignore_ascii_case(s.trim()))
    }

    /// `--filter food,drink`: the chips named, in the chips' order; names that aren't one are left out.
    fn parse_list(s: &str) -> Vec<Group> {
        Group::ALL.into_iter().filter(|g| s.split(',').any(|t| Group::parse(t) == Some(*g))).collect()
    }
}

/// What the columns show: names containing `query` (any case), of one of `groups` if any chip is on.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
pub struct Find {
    query: String,
    /// Typing into the search box (the game's keys are held back meanwhile).
    pub typing: bool,
    /// The chips that are on (each a toggle; none: every kind).
    groups: Vec<Group>,
    /// The search box and chips asked for with the "Find" control, though nothing is set yet.
    open: bool,
}

impl Find {
    fn shows(&self, items: &[ItemDef], s: &ItemStack) -> bool {
        let Some(def) = items.get(s.item as usize) else { return true };
        (self.groups.is_empty() || self.groups.contains(&Group::of(def.category))) && (self.query.is_empty() || def.name.to_lowercase().contains(&self.query.to_lowercase()))
    }

    /// Whether the search box and chips are on show: only while they're in use (typing, a search
    /// or a chip set) or asked for.
    fn in_use(&self) -> bool {
        self.open || self.typing || !self.query.is_empty() || !self.groups.is_empty()
    }

    /// The "Find" control pressed: show the search box and chips; in use, put them away, cleared.
    fn press(&mut self) {
        *self = Find { open: !self.in_use(), ..default() };
    }

    /// A chip pressed: on if it was off, off if it was on (the others stay as they are).
    fn toggle(&mut self, g: Group) {
        match self.groups.iter().position(|x| *x == g) {
            Some(i) => {
                self.groups.remove(i);
            }
            None => self.groups.push(g),
        }
    }
}

/// A row of the panel: where its stack is, its index there and its item (to tell it's still the
/// same stack after things move).
type Key = (Place, u32, u16);

/// Which column a row is in: ranges (Shift-click) stay within one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    You,
    Container,
    Floor,
}

impl Column {
    fn of(place: Place) -> Column {
        match place {
            Place::Slot(_) | Place::Worn | Place::Carried => Column::You,
            Place::Container(_) => Column::Container,
            Place::Ground => Column::Floor,
        }
    }
}

/// Stacks picked with Ctrl-click and Shift-click, and the row a Shift-click's range starts from
/// (the last row Ctrl- or Shift-clicked with nothing to range from).
#[derive(Resource, Default, Debug, Clone, PartialEq)]
struct Picked {
    keys: Vec<Key>,
    anchor: Option<Key>,
}

impl Picked {
    /// Ctrl-click: pick the row, or unpick it if it was; ranges start from it next.
    fn toggle(&mut self, key: Key) {
        match self.keys.iter().position(|k| *k == key) {
            Some(i) => {
                self.keys.remove(i);
            }
            None => self.keys.push(key),
        }
        self.anchor = Some(key);
    }

    /// Ctrl-click on a row of several stacks (one item, collapsed): all picked, or if they all
    /// were, all unpicked; ranges start from its first.
    fn toggle_all(&mut self, keys: &[Key]) {
        match keys {
            [] => {}
            [key] => self.toggle(*key),
            _ if keys.iter().all(|k| self.keys.contains(k)) => {
                self.keys.retain(|k| !keys.contains(k));
                self.anchor = Some(keys[0]);
            }
            _ => {
                let new: Vec<Key> = keys.iter().filter(|k| !self.keys.contains(k)).copied().collect();
                self.keys.extend(new);
                self.anchor = Some(keys[0]);
            }
        }
    }

    /// Shift-click on a row of `keys` (several for a collapsed row): the range to its first and
    /// to its last, so the whole row is in it from either side.
    fn extend_to_row(&mut self, keys: &[Key], listed: &[Key]) {
        if let (Some(first), Some(last)) = (keys.first(), keys.last()) {
            self.extend_to(*first, listed);
            self.extend_to(*last, listed);
        }
    }

    /// Shift-click: pick every row from the anchor to this one as `listed` shows them (the rows
    /// drawn, in order: what the search and chips leave), added to what is picked already. With
    /// no anchor in the same column on screen, just this row, which becomes the anchor.
    fn extend_to(&mut self, key: Key, listed: &[Key]) {
        let col = Column::of(key.0);
        let rows: Vec<Key> = listed.iter().copied().filter(|k| Column::of(k.0) == col).collect();
        let range = match (self.anchor.and_then(|a| rows.iter().position(|k| *k == a)), rows.iter().position(|k| *k == key)) {
            (Some(a), Some(b)) => &rows[a.min(b)..=a.max(b)],
            _ => {
                self.anchor = Some(key);
                std::slice::from_ref(&key)
            }
        };
        for k in range {
            if !self.keys.contains(k) {
                self.keys.push(*k);
            }
        }
    }

    /// A column's "pick all": every stack `listed` shows in `col` (what the search and chips
    /// leave) added to what is picked; if they all were already, they're unpicked instead. What
    /// you hold and wear is never part of it (pick all, then "to floor", would undress you): of
    /// yours, only what you carry.
    fn pick_all(&mut self, col: Column, listed: &[Key]) {
        let rows: Vec<Key> = listed.iter().copied().filter(|k| Column::of(k.0) == col && !matches!(k.0, Place::Slot(_) | Place::Worn)).collect();
        if rows.iter().all(|k| self.keys.contains(k)) {
            self.keys.retain(|k| !rows.contains(k));
        } else {
            for k in rows {
                if !self.keys.contains(&k) {
                    self.keys.push(k);
                }
            }
        }
        self.anchor = None;
    }
}

/// What the column headers sort the rows by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SortKey {
    /// As they lie in the pack, the container or on the floor.
    #[default]
    None,
    Name,
    Kind,
    Weight,
}

impl SortKey {
    /// `--sort name|type|weight` (also `category`, `kg`).
    fn parse(s: &str) -> Option<SortKey> {
        match s.trim().to_ascii_lowercase().as_str() {
            "name" => Some(SortKey::Name),
            "type" | "category" | "kind" => Some(SortKey::Kind),
            "weight" | "kg" => Some(SortKey::Weight),
            _ => None,
        }
    }
}

/// The order of the rows in both panes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Sort {
    key: SortKey,
    /// Z to A, or lightest first (a weight sort starts heaviest first).
    flipped: bool,
}

impl Sort {
    /// A column header pressed: sort by it; again, the other way round; a third time, back to
    /// the order things lie in.
    fn press(&mut self, key: SortKey) {
        *self = match (self.key == key, self.flipped) {
            (false, _) => Sort { key, flipped: false },
            (true, false) => Sort { key, flipped: true },
            (true, true) => Sort::default(),
        };
    }

    /// The mark after a header: which way its column is sorted, if it is.
    fn mark(&self, key: SortKey) -> &'static str {
        match (self.key == key, self.flipped != (key == SortKey::Weight)) {
            (false, _) => "",
            (true, false) => " ^",
            (true, true) => " v",
        }
    }
}

/// One stack where it lies: a row of a pane, or one of the stacks a collapsed row stands for.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Line {
    from: Place,
    index: u32,
    stack: ItemStack,
}

impl Line {
    fn key(&self) -> Key {
        (self.from, self.index, self.stack.item)
    }
}

/// A pane's rows from its stacks: stacks of the same item collapse into one row (in the place of
/// the first), then the rows are put in `sort`'s order (stable: equal rows stay as they lay).
fn arrange(items: &[ItemDef], lines: impl IntoIterator<Item = Line>, sort: Sort) -> Vec<Vec<Line>> {
    let mut rows: Vec<Vec<Line>> = Vec::new();
    for l in lines {
        match rows.iter_mut().find(|r| r[0].stack.item == l.stack.item) {
            Some(r) => r.push(l),
            None => rows.push(vec![l]),
        }
    }
    let def = |r: &Vec<Line>| items.get(r[0].stack.item as usize);
    let name = |r: &Vec<Line>| def(r).map_or(String::new(), |d| d.name.to_lowercase());
    let kind = |r: &Vec<Line>| def(r).map_or(usize::MAX, |d| Group::ALL.iter().position(|g| *g == Group::of(d.category)).unwrap_or(usize::MAX));
    let kg = |r: &Vec<Line>| r.iter().map(|l| stack_kg(items, &l.stack)).sum::<f32>();
    match sort.key {
        SortKey::None => return rows,
        SortKey::Name => rows.sort_by_key(|r| name(r)),
        SortKey::Kind => rows.sort_by_key(|r| (kind(r), name(r))),
        // Heaviest first.
        SortKey::Weight => rows.sort_by(|a, b| kg(b).total_cmp(&kg(a))),
    }
    if sort.flipped {
        rows.reverse();
    }
    rows
}

/// How the panes are set: which tab each shows, the order of the rows, and which collapsed rows
/// are opened.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
struct PanelUi {
    /// The left pane shows what you hold and wear (else what you carry).
    worn: bool,
    /// ...and shows it on the outline of a person (`body.rs`, DCM-43) instead of as a list.
    body: bool,
    /// The right pane shows the floor though a container is open.
    floor: bool,
    /// The container open when we last looked (`None`: not looked yet).
    seen_open: Option<Option<u32>>,
    sort: Sort,
    /// Rows of several stacks start opened (`--expand`, for pictures); `folds` then lists the shut ones.
    unfold_all: bool,
    /// The rows (column, item) whose fold mark was pressed.
    folds: Vec<(Column, u16)>,
}

impl PanelUi {
    /// Whether the row of several stacks of `item` in `col` is opened to its stacks.
    fn unfolded(&self, col: Column, item: u16) -> bool {
        self.unfold_all != self.folds.contains(&(col, item))
    }

    fn toggle_fold(&mut self, key: (Column, u16)) {
        match self.folds.iter().position(|k| *k == key) {
            Some(i) => {
                self.folds.remove(i);
            }
            None => self.folds.push(key),
        }
    }

    /// What the right pane shows, and so where a click on what you carry sends it: the open
    /// container, or the floor (no container open, or its Floor tab picked).
    fn right(&self, open: Option<u32>) -> Place {
        match open {
            Some(id) if !self.floor => Place::Container(id),
            _ => Place::Ground,
        }
    }

    /// The container open now: a newly opened one comes to the front of the right pane.
    fn saw(&mut self, open: Option<u32>) {
        if self.seen_open != Some(open) {
            if self.seen_open.is_some() && open.is_some() {
                self.floor = false;
            }
            self.seen_open = Some(open);
        }
    }
}

/// The item rows the panel drew last, in order (for Shift-click's ranges).
#[derive(Resource, Default, Debug)]
struct Listed(Vec<Key>);

/// Where the move-to row over the columns was asked to send every picked stack this frame.
#[derive(Resource, Default)]
struct SendPicked(Option<Place>);

/// A move-all button between the panes pressed this frame: every stack that column shows goes
/// across to the other pane.
#[derive(Resource, Default)]
struct MoveAll(Option<Column>);

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
        // Open as it was left last session (`panels.rs`), or by `--inventory`.
        let settings = crate::ui_scale::SettingsStore::ensure(app);
        let open = options.as_ref().is_some_and(|o| o.inventory) || settings.panels.inventory.open;
        let find = Find { query: options.as_ref().and_then(|o| o.find.clone()).unwrap_or_default(), typing: false, groups: options.as_ref().and_then(|o| o.filter.as_deref()).map_or(Vec::new(), Group::parse_list), open: false };
        let tabs = options.as_ref().and_then(|o| o.tab.clone()).unwrap_or_default().to_ascii_lowercase();
        let ui = PanelUi {
            // (`--tab body`: the Worn tab as the body view; else as it was left last session.)
            worn: tabs.split(',').any(|t| matches!(t.trim(), "worn" | "body")),
            body: tabs.split(',').any(|t| t.trim() == "body") || settings.inventory_body,
            floor: tabs.split(',').any(|t| t.trim() == "floor"),
            sort: Sort { key: options.as_ref().and_then(|o| o.sort.as_deref()).and_then(SortKey::parse).unwrap_or_default(), flipped: false },
            unfold_all: options.as_ref().is_some_and(|o| o.expand),
            ..default()
        };
        let picked = Picked { keys: options.as_ref().map_or(Vec::new(), |o| o.pick.iter().map(|&k| (Place::Carried, k, u16::MAX)).collect()), anchor: None };
        app.insert_resource(InventoryPanel { open })
            .insert_resource(find)
            .insert_resource(picked)
            .insert_resource(ui)
            .init_resource::<MoveAll>()
            .init_resource::<SendPicked>()
            .init_resource::<Listed>()
            .init_resource::<LastNote>()
            .init_resource::<Shown>()
            .init_resource::<Clicks>()
            .init_resource::<Outbox>()
            .init_resource::<InFlight>()
            .init_resource::<MakeUi>()
            .init_resource::<ItemMenu>()
            .init_resource::<HoveredItem>()
            .add_systems(Startup, spawn.after(crate::hud::spawn_hud))
            .add_systems(PreUpdate, (type_search, menu_keys).chain().in_set(TextEntry).after(bevy::input::InputSystems))
            .add_systems(Update, (toggle, keep_note, track_flight, follow_open, prune_picked, script_picks, menu_from_options, send_clicks, world_menu, script_menu, menu_act, send_picked, flush, remember_view, rebuild, draw_menu).chain());
    }
}

/// The Worn tab's view (list or body) is kept between sessions, written when it changes.
fn remember_view(ui: Res<PanelUi>, mut store: ResMut<crate::ui_scale::SettingsStore>) {
    if ui.is_changed()
        && store.settings.inventory_body != ui.body
        && let Err(e) = store.change(|s| s.inventory_body = ui.body)
    {
        eprintln!("{e}");
    }
}

fn spawn(mut commands: Commands, column: Query<Entity, With<crate::hud::BottomLeft>>) {
    commands.spawn((
        PanelRoot,
        crate::hud::TopPanel,
        crate::panels::PanelId::Inventory,
        Visibility::Hidden,
        Node {
            position_type: PositionType::Absolute,
            top: px(70),
            right: px(16),
            flex_direction: FlexDirection::Column,
            row_gap: px(8),
            padding: UiRect::axes(px(theme::SHEET_PAD.0), px(theme::SHEET_PAD.1)),
            max_width: px(PANEL_W),
            max_height: Val::Percent(85.0),
            overflow: Overflow::scroll_y(),
            ..default()
        },
        theme::sheet(),
    ));
    // The latest note while the panel is shut: just above the HUD's bottom-left column (the tags
    // and the vitals), however tall that is, on a backing so it reads over the world (#97).
    let note = (NoteLine, Text::new(""), theme::strong(15.0), TextColor(OK), TextLayout::no_wrap(), BackgroundColor(Color::NONE), Pickable::IGNORE);
    match column.single() {
        Ok(column) => {
            commands.spawn((note, Node { position_type: PositionType::Absolute, bottom: percent(100), left: px(0), margin: UiRect::bottom(px(6)), ..default() }, ChildOf(column)));
        }
        Err(_) => {
            commands.spawn((note, Node { position_type: PositionType::Absolute, bottom: px(150), left: px(18), ..default() }));
        }
    }
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
            find.open = true;
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
            Key::Enter | Key::Escape => {
                find.typing = false;
                // Nothing set: the box goes away again.
                find.open = !find.query.is_empty() || !find.groups.is_empty();
            }
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

fn keep_note(time: Res<Time>, notes: Res<ItemNotesThisFrame>, panel: Res<InventoryPanel>, mut last: ResMut<LastNote>, mut line: Query<(&mut Text, &mut TextColor, &mut BackgroundColor), With<NoteLine>>) {
    let now = time.elapsed_secs();
    for n in &notes.0 {
        // The same answer again (E pressed over and over where there's nothing): keep it up, once.
        let again = n.text == last.text && n.ok == last.ok && now - last.at < NOTE_SECS;
        *last = LastNote { text: n.text.clone(), ok: n.ok, at: now };
        if !again {
            println!("item: {}{}", if n.ok { "" } else { "refused: " }, n.text);
        }
    }
    let Ok((mut t, mut c, mut bg)) = line.single_mut() else { return };
    let show = !panel.open && !last.text.is_empty() && time.elapsed_secs() - last.at < NOTE_SECS;
    let want = if show { last.text.clone() } else { String::new() };
    if t.0 != want {
        t.0 = want;
        bg.0 = if show { crate::theme::HUD_BG } else { Color::NONE };
    }
    c.0 = if last.ok { OK } else { NO };
}

/// "Loaf of bread x2 (stale, 2/3 left)": the name, how many, and for food and drink how fresh it
/// is at game minute `now` and the goes left of the top one (#11).
fn name_of(items: &[ItemDef], now: i32, s: &ItemStack) -> String {
    let Some(def) = items.get(s.item as usize) else { return "?".into() };
    let mut out = if s.count > 1 { format!("{} x{}", def.name, s.count) } else { def.name.clone() };
    let notes = state_of(items, now, s);
    if !notes.is_empty() {
        out.push_str(&format!(" ({notes})"));
    }
    out
}

/// "stale, 2/3 left": what a row says after the name about food and drink (how fresh at game
/// minute `now`, the goes left of the top one); empty for the rest.
fn state_of(items: &[ItemDef], now: i32, s: &ItemStack) -> String {
    let Some(def) = items.get(s.item as usize) else { return String::new() };
    let mut notes = Vec::new();
    if let Some(f) = zombye_sim::needs::freshness(def, s, now).filter(|f| *f != zombye_sim::Freshness::Fresh) {
        notes.push(f.name().to_string());
    }
    if let Some(c) = def.consumable().filter(|_| s.used > 0) {
        notes.push(format!("{}/{} left", c.uses - s.used, c.uses));
    } else if s.used > 0 {
        // A gun or a magazine (#72): what's in it.
        notes.push(format!("{} in it", s.used));
    } else if def.has_tag(zombye_sim::ranged::TAG_MAGAZINE) {
        notes.push("empty".into());
    }
    notes.join(", ")
}

fn stack_kg(items: &[ItemDef], s: &ItemStack) -> f32 {
    items.get(s.item as usize).map_or(0.0, |d| d.weight_kg * s.count as f32)
}

/// What a click on a row sends: left (whole stack, or one with Alt) and right (always about
/// one of a carried stack). `stack` is what the row shows.
#[derive(Clone, Debug)]
struct RowAction {
    stack: ItemStack,
    /// The menu's title (the item's name).
    title: String,
    left: Option<(Place, u32, Place)>,
    /// A collapsed row: the other stacks of the same item it stands for (a click moves them too).
    more: Vec<Line>,
    /// Right-click: the menu of what can be done with it (#95), the likeliest first.
    menu: Vec<MenuEntry>,
    /// Left click: make this recipe (a row of the Make list; `stack` and `left` are unused).
    craft: Option<u16>,
}

impl RowAction {
    /// The stacks the row stands for (one, or several of one item), for picking.
    fn keys(&self) -> Vec<Key> {
        self.left.iter().map(|&(from, index, _)| (from, index, self.stack.item)).chain(self.more.iter().map(Line::key)).collect()
    }
}

/// What the panel shows and the requests are checked against: our private view and our floor
/// (ground item id and stack, nearest first), from the latest snapshot.
fn panel_view(link: &ServerLink, me: u32) -> (Option<PrivateView>, Vec<(u32, ItemStack)>) {
    let floor = link.curr.floor_of(me).iter().filter_map(|g| link.curr.items.iter().find(|it| it.id == *g)).map(|it| (it.id, it.stack)).collect();
    (link.own(me).cloned(), floor)
}

/// The stack at `place`/`index` in that view (a ground item's id is its index), if there is one.
fn stack_at(v: &PrivateView, floor: &[(u32, ItemStack)], place: Place, index: u32) -> Option<ItemStack> {
    let i = index as usize;
    match place {
        Place::Carried => v.inventory.carried.get(i).copied(),
        Place::Worn => v.inventory.worn.get(i).copied(),
        Place::Slot(slot) => v.inventory.slot(slot),
        Place::Container(id) => v.open.as_ref().filter(|c| c.id == id).and_then(|c| c.stacks.get(i)).copied(),
        Place::Ground => floor.iter().find(|(g, _)| *g == index).map(|(_, s)| *s),
    }
}

fn track_flight(time: Res<Time>, link: Res<ServerLink>, me: Res<LocalPlayer>, notes: Res<ItemNotesThisFrame>, mut flight: ResMut<InFlight>) {
    let (view, floor) = panel_view(&link, me.id);
    flight.update(format!("{view:?}|{floor:?}"), notes.0.iter().any(|n| !n.ok), time.elapsed_secs());
}

/// A container newly opened comes to the front of the right pane, and a search that has just
/// finished opens the panel if it was shut, so what was found is on show ([`search_done`]).
fn follow_open(link: Res<ServerLink>, me: Res<LocalPlayer>, mut ui: ResMut<PanelUi>, mut panel: ResMut<InventoryPanel>, mut searching: Local<Option<u32>>) {
    let view = link.own(me.id);
    let open = view.and_then(|v| v.open.as_ref().map(|c| c.id));
    if ui.seen_open != Some(open) {
        ui.saw(open);
    }
    let now = view.and_then(|v| match v.jobs.first() {
        Some(JobKind::Search { container }) => Some(*container),
        _ => None,
    });
    if search_done(*searching, now, open) && !panel.open {
        panel.open = true;
    }
    *searching = now;
}

/// Has the search of a container just finished with it open? `was` and `now`: the container our
/// job under way was searching when we last looked, and is now; `open`: the one open to us. A
/// search called off, or one that ended out of reach, leaves that container shut, so it opens
/// nothing; neither does a save loaded with a container open (nothing was being searched).
fn search_done(was: Option<u32>, now: Option<u32>, open: Option<u32>) -> bool {
    was.is_some() && was != now && was == open
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
    if picked.keys.is_empty() {
        return;
    }
    let (Some(v), floor) = panel_view(&link, me.id) else { return };
    let at = |place: Place, index: u32| stack_at(&v, &floor, place, index).map(|s| s.item);
    let kept: Vec<(Place, u32, u16)> = picked.keys.iter().filter_map(|&(p, i, item)| at(p, i).filter(|now| item == u16::MAX || item == *now).map(|now| (p, i, now))).collect();
    if kept != picked.keys {
        picked.keys = kept;
    }
}

/// `--pick-range A,B[@SECS]` and `--drop-picked-after SECS`, for recordings: once SECS have gone
/// by and the panel shows carried rows A and B, Ctrl-click A and Shift-click B (as a player
/// would); later, press the move-to row's "floor". Each happens once.
fn script_picks(time: Res<Time>, options: Option<Res<crate::Options>>, listed: Res<Listed>, mut picked: ResMut<Picked>, mut send: ResMut<SendPicked>, mut done: Local<(bool, bool)>) {
    let Some(o) = options else { return };
    let now = time.elapsed_secs();
    if let Some((a, b, after)) = o.pick_range
        && !done.0
        && now >= after
    {
        let carried = |i: u32| listed.0.iter().copied().find(|k| k.0 == Place::Carried && k.1 == i);
        if let (Some(a), Some(b)) = (carried(a), carried(b)) {
            picked.toggle(a);
            picked.extend_to(b, &listed.0);
            done.0 = true;
        }
    }
    if let Some(after) = o.drop_picked_after
        && !done.1
        && now >= after
        && !picked.keys.is_empty()
    {
        send.0 = Some(Place::Ground);
        done.1 = true;
    }
}

/// The bar's "→ bags / → container / → floor": every picked stack there, then nothing picked.
/// And the move-all buttons between the panes: every stack that pane shows (what the search and
/// chips leave; of yours, what you carry, never what you hold or wear) across to the other.
#[allow(clippy::too_many_arguments)]
fn send_picked(time: Res<Time>, me: Res<LocalPlayer>, link: Res<ServerLink>, ui: Res<PanelUi>, listed: Res<Listed>, mut send: ResMut<SendPicked>, mut all: ResMut<MoveAll>, mut picked: ResMut<Picked>, mut flight: ResMut<InFlight>, mut out: ResMut<Outbox>) {
    let (to, across) = (send.0.take(), all.0.take());
    if to.is_none() && across.is_none() {
        return;
    }
    let (view, floor) = panel_view(&link, me.id);
    let Some(v) = view else { return };
    let now = time.elapsed_secs();
    let mut moves: Vec<(Key, Place)> = Vec::new();
    if let Some(to) = to {
        moves.extend(std::mem::take(&mut picked.keys).into_iter().map(|k| (k, to)));
        picked.anchor = None;
    }
    if let Some(col) = across {
        let to = if col == Column::You { ui.right(v.open.as_ref().map(|c| c.id)) } else { Place::Carried };
        let rest: Vec<Key> = all_of(col, &listed.0).into_iter().filter(|k| !moves.iter().any(|(m, _)| m == k)).collect();
        moves.extend(rest.into_iter().map(|k| (k, to)));
    }
    // From the back of each list first, so the indices of the ones still to go don't shift.
    moves.sort_by_key(|&((_, i, _), _)| std::cmp::Reverse(i));
    for ((from, index, item), to) in moves {
        let Some(stack) = stack_at(&v, &floor, from, index).filter(|s| s.item == item) else { continue };
        if let Some(intent) = move_intent(me.id, from, index, to, 0)
            && flight.ask(from, index, stack, false, now)
        {
            out.0.push(intent);
        }
    }
}

/// What a move-all button moves: the stacks `listed` shows in `col`, but never what you hold or wear.
fn all_of(col: Column, listed: &[Key]) -> Vec<Key> {
    listed.iter().copied().filter(|k| Column::of(k.0) == col && !matches!(k.0, Place::Slot(_) | Place::Worn)).collect()
}

/// The intents a click sends: [`click_intent`]'s one, or for a left click on a collapsed row one
/// for each of its stacks, from the back first so the indices of the ones still to go don't shift.
fn click_intents(click: &RowClick, player: u32, flight: &mut InFlight, now: f32) -> Vec<Intent> {
    let a = &click.action;
    let Some((from, index, to)) = a.left.filter(|_| click.button == PointerButton::Primary && !click.one && a.craft.is_none() && !a.more.is_empty()) else {
        return click_intent(click, player, flight, now).into_iter().collect();
    };
    let mut lines = vec![Line { from, index, stack: a.stack }];
    lines.extend(a.more.iter().copied());
    lines.sort_by_key(|l| std::cmp::Reverse(l.index));
    lines.into_iter().filter(|l| flight.ask(l.from, l.index, l.stack, false, now)).filter_map(|l| move_intent(player, l.from, l.index, to, 0)).collect()
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
        _ => None,
    }
}

/// This frame's presses on rows, as intents: at most one per button (one physical click is one
/// press, however many times picking reports it), none for what is already on its way.
fn send_clicks(time: Res<Time>, me: Res<LocalPlayer>, windows: Query<&Window, With<bevy::window::PrimaryWindow>>, mut clicks: ResMut<Clicks>, mut flight: ResMut<InFlight>, mut out: ResMut<Outbox>, mut menu: ResMut<ItemMenu>) {
    menu_clicks(&mut clicks, &mut menu, windows.single().ok().and_then(|w| w.cursor_position()));
    drain_clicks(me.id, time.elapsed_secs(), &mut clicks, &mut flight, &mut out);
}

/// What this frame's presses on rows mean for the menu: a right-click opens that row's, at `at`.
/// With a menu up, a left click anywhere else only closes it ([`menu_act`]): it is dropped here,
/// so it doesn't also move the row that happens to be under the pointer.
fn menu_clicks(clicks: &mut Clicks, menu: &mut ItemMenu, at: Option<Vec2>) {
    if menu.open.is_some() {
        clicks.0.retain(|c| c.button != PointerButton::Primary);
    }
    if let Some(c) = clicks.0.iter().find(|c| c.button == PointerButton::Secondary && !c.action.menu.is_empty()) {
        menu.show(c.action.title.clone(), c.action.stack, c.action.menu.clone(), at);
    }
}

fn drain_clicks(player: u32, now: f32, clicks: &mut Clicks, flight: &mut InFlight, out: &mut Outbox) {
    let mut buttons: Vec<PointerButton> = Vec::new();
    for click in clicks.0.drain(..) {
        if buttons.contains(&click.button) {
            continue;
        }
        buttons.push(click.button);
        out.0.extend(click_intents(&click, player, flight, now));
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
    ui: Res<PanelUi>,
    mut listed: ResMut<Listed>,
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
    // (The seconds a job has left only show in the Make list, so only count when it's open.)
    let secs = if make_ui.shown { busy.0 } else { 0.0 };
    let signature = format!("{view:?}|{floor:?}|{secs:.1}|{}|{}|{}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{}|{:?}", busy.1, if note_fresh { last.text.as_str() } else { "" }, me.dead, flight.asked, flight.crafts, now / 60, *find, picked.keys, make_ui.open, make_ui.favs, make_ui.shown, *ui);
    if shown.0 == signature {
        return;
    }
    shown.0 = signature;
    commands.entity(root).despawn_children();
    let items = &join.items;
    let open = view.as_ref().and_then(|v| v.open.as_ref().map(|c| (c.id, c.name.to_lowercase())));
    // What the right pane shows, and so where a click on what you carry sends it.
    let right = ui.right(open.as_ref().map(|(id, _)| *id));
    let ctx = Ctx { items, now, me: me.id, flight: &flight, find: &find, picked: &picked, ui: &ui, view: view.as_ref(), open: open.as_ref().map(|(id, n)| (*id, n.as_str())) };
    // The item rows drawn, in order, for Shift-click's ranges and the move-all buttons.
    let mut rows: Vec<Key> = Vec::new();
    commands.entity(root).with_children(|c| {
        // The top row: the move-to bar for picked stacks (always this high, so picking a first row
        // doesn't shift the rows under the pointer) and the one control that brings out the search.
        let kg: f32 = view.as_ref().map_or(0.0, |v| picked.keys.iter().filter_map(|&(p, i, _)| stack_at(v, &floor, p, i)).map(|s| stack_kg(items, &s)).sum());
        top_bar(c, &find, picked.keys.len(), kg, open.clone());
        if find.in_use() {
            toolbar(c, &find);
        }
        c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::FlexStart, ..default() }).with_children(|panes| {
            // Left: what you carry, or (its other tab) what you hold and wear.
            panes.spawn(tab_strip()).with_children(|t| {
                tab(t, "Carried", !ui.worn).observe(|_: On<PointerPress>, mut ui: ResMut<PanelUi>| ui.worn = false);
                tab(t, "Worn", ui.worn).observe(|_: On<PointerPress>, mut ui: ResMut<PanelUi>| ui.worn = true);
            });
            let load = view.as_ref().map(|v| {
                let k = &v.carry;
                let (tag, color) = match k.burden {
                    Burden::None => ("", TEXT),
                    Burden::Burdened => ("  BURDENED", OK),
                    Burden::Heavy => ("  HEAVY", NO),
                    Burden::Overloaded => ("  OVERLOADED", Color::srgb(1.0, 0.3, 0.25)),
                };
                Gauge { share: Some(k.weight_kg / k.limit_kg.max(0.1)), color, left: format!("{:.1} / {:.1} kg{tag}", k.weight_kg, k.limit_kg), right: format!("{:.1} / {:.0} L", k.used_l, k.room_l) }
            });
            pane(panes, if ui.worn { "Worn and held" } else { "Carried" }, if ui.worn { PaneTop::View { body: ui.body } } else { PaneTop::PickAll(Column::You) }, load.unwrap_or_default(), ui.sort, |col| {
                let Some(v) = &view else { return };
                let inv = &v.inventory;
                if ui.worn && ui.body {
                    body::draw(&ctx, &mut rows, col, inv);
                    return;
                }
                if ui.worn {
                    for (label, slot) in [("Hand", Slot::Hand), ("Off hand", Slot::OffHand), ("Back", Slot::Back), ("Shoulder", Slot::Shoulder)] {
                        match inv.slot(slot) {
                            Some(stack) => ctx.slot_row(&mut rows, col, Line { from: Place::Slot(slot), index: 0, stack }, label),
                            None => empty_row(col, label),
                        }
                    }
                    for (k, s) in inv.worn.iter().enumerate().filter(|(k, s)| ctx.shows(Place::Worn, *k as u32, s)) {
                        ctx.slot_row(&mut rows, col, Line { from: Place::Worn, index: k as u32, stack: *s }, "Worn");
                    }
                    return;
                }
                if inv.carried.is_empty() {
                    dim(col, "Nothing".into());
                } else if !inv.carried.iter().any(|s| find.shows(items, s)) {
                    dim(col, "Nothing that matches".into());
                }
                ctx.rows(&mut rows, col, inv.carried.iter().enumerate().map(|(k, s)| Line { from: Place::Carried, index: k as u32, stack: *s }), right);
            });
            // Between the panes: everything one shows, across to the other.
            panes.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(6), width: px(34), margin: UiRect::axes(px(4), px(0)), padding: UiRect::top(px(94)), flex_shrink: 0.0, ..default() }).with_children(|mid| {
                let across = if matches!(right, Place::Ground) { Column::Floor } else { Column::Container };
                chip(mid, ">>".into(), false).observe(|_: On<PointerPress>, mut all: ResMut<MoveAll>| all.0 = Some(Column::You));
                chip(mid, "<<".into(), false).observe(move |_: On<PointerPress>, mut all: ResMut<MoveAll>| all.0 = Some(across));
            });
            // Right: the open container, or (its other tab, and with none open) the floor in reach.
            let floor_kg: f32 = floor.iter().map(|(_, s)| stack_kg(items, s)).sum();
            match view.as_ref().and_then(|v| v.open.as_ref()).filter(|_| matches!(right, Place::Container(_))) {
                Some(cv) => {
                    let used = zombye_sim::inventory::stacks_volume(items, &cv.stacks);
                    let kg: f32 = cv.stacks.iter().map(|s| stack_kg(items, s)).sum();
                    let gauge = Gauge { share: Some(used / cv.room_l.max(0.1)), color: TEXT, left: format!("{used:.1} / {:.0} L", cv.room_l), right: format!("{kg:.1} kg") };
                    pane(panes, &cv.name, PaneTop::PickAll(Column::Container), gauge, ui.sort, |col| {
                        if cv.stacks.is_empty() {
                            dim(col, "Empty".into());
                        } else if !cv.stacks.iter().any(|s| find.shows(items, s)) {
                            dim(col, "Nothing that matches".into());
                        }
                        ctx.rows(&mut rows, col, cv.stacks.iter().enumerate().map(|(k, s)| Line { from: Place::Container(cv.id), index: k as u32, stack: *s }), Place::Carried);
                    });
                }
                None => {
                    let gauge = Gauge { share: None, color: TEXT, left: "Within reach".into(), right: format!("{floor_kg:.1} kg") };
                    pane(panes, "Floor", PaneTop::PickAll(Column::Floor), gauge, ui.sort, |col| {
                        if floor.is_empty() {
                            dim(col, "Nothing within reach".into());
                        } else if !floor.iter().any(|(_, s)| find.shows(items, s)) {
                            dim(col, "Nothing that matches".into());
                        }
                        ctx.rows(&mut rows, col, floor.iter().map(|(id, s)| Line { from: Place::Ground, index: *id, stack: *s }), Place::Carried);
                        if open.is_none() {
                            dim(col, "E: search the cupboard, drawer or fridge you face".into());
                        }
                    });
                }
            }
            panes.spawn(tab_strip()).with_children(|t| {
                if let Some(cv) = view.as_ref().and_then(|v| v.open.as_ref()) {
                    tab(t, &format!("{} {}", cv.name, cv.stacks.len()), matches!(right, Place::Container(_))).observe(|_: On<PointerPress>, mut ui: ResMut<PanelUi>| ui.floor = false);
                }
                tab(t, &format!("Floor {}", floor.len()), matches!(right, Place::Ground)).observe(|_: On<PointerPress>, mut ui: ResMut<PanelUi>| ui.floor = true);
            });
        });
        if let Some(v) = &view {
            make(c, &join, v, busy.0, &flight, &make_ui);
        }
        c.spawn((Text::new("Click: move across   Alt-click: just one   Ctrl / Shift-click: pick   Right-click: menu   Tab: close"), font(ROW_FONT), TextColor(DIM)));
        if note_fresh && !last.text.is_empty() {
            c.spawn((Text::new(last.text.clone()), font(ROW_FONT), TextColor(if last.ok { OK } else { NO })));
        }
    });
    listed.0 = rows;
}

/// What drawing a pane's rows needs to know.
struct Ctx<'a> {
    items: &'a [ItemDef],
    /// The game minute (food goes off).
    now: i32,
    me: u32,
    flight: &'a InFlight,
    find: &'a Find,
    picked: &'a Picked,
    ui: &'a PanelUi,
    view: Option<&'a PrivateView>,
    /// The open container (id, name), for the menus.
    open: Option<(u32, &'a str)>,
}

impl Ctx<'_> {
    fn shows(&self, from: Place, index: u32, s: &ItemStack) -> bool {
        on_show(self.find, self.items, self.picked, from, index, s)
    }

    /// What a click on the row of `l` (and `more`, the other stacks it stands for) does.
    fn act(&self, l: &Line, to: Place, more: Vec<Line>) -> RowAction {
        RowAction { stack: l.stack, title: name_of(self.items, self.now, &l.stack), left: Some((l.from, l.index, to)), more, menu: menu_entries(self.me, self.items, l.from, l.index, &l.stack, self.open), craft: None }
    }

    /// What follows the name, dimmed: how fresh, goes left, uses left in the opened one (a box of
    /// plasters, a roll of gauze), and that it is on its way.
    fn note(&self, l: &Line) -> String {
        let mut notes = Vec::new();
        let state = state_of(self.items, self.now, &l.stack);
        if !state.is_empty() {
            notes.push(state);
        }
        let medical = self.items.get(l.stack.item as usize).and_then(|d| d.medical.as_ref());
        let opened = self.view.and_then(|v| v.health.aid.opened.iter().find(|o| o.item == l.stack.item));
        if let (Place::Carried, Some(m), Some(o)) = (l.from, medical, opened) {
            notes.push(format!("{} of {} left", m.uses.saturating_sub(o.used), m.uses));
        }
        if self.flight.asked_for(l.from, l.index, l.stack.item) > 0 {
            notes.push("moving".into());
        }
        notes.join(", ")
    }

    /// The cells of the row of one stack; `kind` instead of its category (the slot it is in).
    fn cells(&self, l: &Line, lead: Lead, kind: Option<&str>) -> Cells {
        let def = self.items.get(l.stack.item as usize);
        Cells {
            lead,
            swatch: def.map(|d| Color::srgb(d.color.0, d.color.1, d.color.2)),
            name: def.map_or("?".into(), |d| d.name.clone()),
            note: self.note(l),
            kind: kind.map_or_else(|| def.map_or("", |d| Group::of(d.category).name()).to_string(), str::to_string),
            qty: if l.stack.count > 1 { l.stack.count.to_string() } else { String::new() },
            kg: format!("{:.1}", stack_kg(self.items, &l.stack)),
            faint: false,
        }
    }

    /// The row of something held or worn: its slot where the others say their type; a click
    /// takes it off into the bags.
    fn slot_row(&self, listed: &mut Vec<Key>, col: &mut ChildSpawnerCommands, l: Line, slot: &str) {
        listed.push(l.key());
        row(col, self.cells(&l, Lead::Nothing, Some(slot)), self.act(&l, Place::Carried, Vec::new()), self.picked.keys.contains(&l.key()));
    }

    /// A pane's rows from its stacks (`lines`: the ones the search and chips leave get a row), a
    /// click sending them to `to`. Several stacks of one item are one row with their count and
    /// weight together and a fold mark: pressed, the stacks show under it, each a row of its own.
    /// `listed` gets every stack drawn, in order.
    fn rows(&self, listed: &mut Vec<Key>, col: &mut ChildSpawnerCommands, lines: impl Iterator<Item = Line>, to: Place) {
        let is_picked = |l: &Line| self.picked.keys.contains(&l.key());
        for stacks in arrange(self.items, lines.filter(|l| self.shows(l.from, l.index, &l.stack)), self.ui.sort) {
            let first = stacks[0];
            if stacks.len() == 1 {
                listed.push(first.key());
                row(col, self.cells(&first, Lead::Nothing, None), self.act(&first, to, Vec::new()), is_picked(&first));
                continue;
            }
            let fold = (Column::of(first.from), first.stack.item);
            // (With only some of its stacks picked it shows them, so what is picked is in view.)
            let open = self.ui.unfolded(fold.0, fold.1) || (stacks.iter().any(is_picked) && !stacks.iter().all(is_picked));
            let mut cells = self.cells(&first, Lead::Fold { open, key: fold }, None);
            cells.qty = stacks.iter().map(|l| l.stack.count as u32).sum::<u32>().to_string();
            cells.kg = format!("{:.1}", stacks.iter().map(|l| stack_kg(self.items, &l.stack)).sum::<f32>());
            cells.note = if stacks.iter().any(|l| self.flight.asked_for(l.from, l.index, l.stack.item) > 0) { "moving".into() } else { String::new() };
            row(col, cells, self.act(&first, to, stacks[1..].to_vec()), stacks.iter().all(is_picked));
            for l in &stacks {
                listed.push(l.key());
                if open {
                    row(col, self.cells(l, Lead::Child, None), self.act(l, to, Vec::new()), is_picked(l));
                }
            }
        }
    }
}

/// The Make list's own state (#96): which families are open and which recipes are pinned.
/// Pinned recipes are saved per player in the saves folder (`make-favourites-P.txt`, one recipe id a line).
#[derive(Resource, Default, Debug)]
struct MakeUi {
    /// Families (by their first output's item id) opened to their variants.
    open: Vec<String>,
    /// Pinned recipe ids.
    favs: Vec<String>,
    /// The list is opened (it starts as one line saying how many you can make).
    shown: bool,
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
            // A line `open:KEY` opens the list and a family from the start (`open:*`: all), for pictures.
            for l in t.lines().map(str::trim).filter(|l| !l.is_empty()) {
                match l.strip_prefix("open:") {
                    Some(k) => {
                        self.open.push(k.to_string());
                        self.shown = true;
                    }
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
        Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(10), padding: UiRect::axes(px(6), px(3)), ..default() },
        BackgroundColor(ROW),
    ))
    .with_children(|r| {
        r.spawn((Text::new(label), font(ROW_FONT), TextColor(color), Pickable::IGNORE));
        r.spawn((Text::new(right), font(ROW_FONT), TextColor(DIM), Pickable::IGNORE));
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
            clicks.0.push(RowClick { action: RowAction { stack: ItemStack::one(0), title: String::new(), left: None, more: Vec::new(), menu: Vec::new(), craft: Some(*k) }, button: ev.button, one: false })
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
    // One line until it's asked for: its title, a fold mark, and how many you can make now.
    let ready = (0..join.recipes.len()).filter(|&k| can(k)).count();
    let summary = match (ui.shown, ready) {
        (true, _) => "click a group to open it; right-click a recipe to pin it (*)".to_string(),
        (false, 0) => "nothing you can make yet".to_string(),
        (false, n) => format!("{n} you can make now"),
    };
    c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Center, margin: UiRect::top(px(4)), ..default() })
        .with_children(|t| {
            t.spawn((Node { width: px(W_LEAD + 10.0), justify_content: JustifyContent::Center, ..default() }, theme::button(false), Pickable::IGNORE)).with_children(|f| {
                f.spawn((Text::new(if ui.shown { "-" } else { "+" }), font(ROW_FONT), TextColor(INK), Pickable::IGNORE));
            });
            t.spawn((Text::new("MAKE"), theme::head(HEAD_FONT), TextColor(TEXT), Pickable::IGNORE));
            t.spawn((Text::new(summary), font(ROW_FONT), TextColor(DIM), Pickable::IGNORE));
        })
        .observe(|ev: On<PointerPress>, mut ui: ResMut<MakeUi>| {
            if ev.button == PointerButton::Primary {
                ui.shown = !ui.shown;
            }
        });
    if !ui.shown {
        return;
    }
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

/// What a pane says under its title about how full it is: a bar (`share` of it filled; none for
/// the floor, which has no limit), and a figure at each end of the line under it.
#[derive(Default)]
struct Gauge {
    share: Option<f32>,
    color: Color,
    left: String,
    right: String,
}

/// What stands at the end of a pane's title.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PaneTop {
    /// "pick all" for the stacks the pane shows.
    PickAll(Column),
    /// The pane of what you hold and wear: "List" and "Body", the two ways it shows them
    /// (`body`: on the outline of a person, `body.rs`).
    View { body: bool },
}

/// A pane: one title (with what `top` says at its end), how full it is, the column headers (a
/// press sorts both panes by that column; none over the body view) and its rows.
fn pane(panes: &mut ChildSpawnerCommands, title: &str, top: PaneTop, gauge: Gauge, sort: Sort, body: impl FnOnce(&mut ChildSpawnerCommands)) {
    panes.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(1), width: px(PANE_W), min_height: px(230), padding: UiRect::all(px(8)), flex_shrink: 0.0, ..default() }, BackgroundColor(PANE))).with_children(|col| {
        // The pane's title, centred; "pick all" at its right end, out of the layout.
        col.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::Center, align_items: AlignItems::Center, margin: UiRect::bottom(px(6)), ..default() }).with_children(|t| {
            t.spawn((Text::new(title.to_uppercase()), theme::head(HEAD_FONT), TextColor(TEXT), TextLayout::no_wrap()));
            match top {
                // Every stack this pane shows, picked in one click (again: unpicked).
                PaneTop::PickAll(which) => {
                    t.spawn((Text::new("pick all"), font(ROW_FONT), TextColor(DIM), TextLayout::no_wrap(), Node { position_type: PositionType::Absolute, right: px(0), ..default() }))
                        .observe(move |_: On<PointerPress>, listed: Res<Listed>, mut picked: ResMut<Picked>| picked.pick_all(which, &listed.0));
                }
                // The two ways the pane of what you hold and wear shows them, at the title's right
                // end, out of the layout (the title stays centred).
                PaneTop::View { body } => {
                    t.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(2), position_type: PositionType::Absolute, right: px(0), ..default() }).with_children(|v| {
                        chip(v, "List".into(), !body).observe(|_: On<PointerPress>, mut ui: ResMut<PanelUi>| ui.body = false);
                        chip(v, "Body".into(), body).observe(|_: On<PointerPress>, mut ui: ResMut<PanelUi>| ui.body = true);
                    });
                }
            }
        });
        // How full it is, as a brushed bar (none for the floor, which has no limit).
        // (Not shrinking: a panel taller than its room scrolls, and would squeeze the bar to nothing.)
        let mut bar = col.spawn(Node { height: px(8), flex_shrink: 0.0, ..default() });
        if let Some(share) = gauge.share {
            bar.insert(theme::bar_track()).with_children(|b| {
                b.spawn((Node { width: Val::Percent(share.clamp(0.0, 1.0) * 100.0), height: Val::Percent(100.0), ..default() }, theme::bar_fill(gauge.color)));
            });
        }
        col.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, margin: UiRect::axes(px(0), px(4)), ..default() }).with_children(|g| {
            g.spawn((Text::new(gauge.left), font(ROW_FONT), TextColor(gauge.color), TextLayout::no_wrap()));
            g.spawn((Text::new(gauge.right), font(ROW_FONT), TextColor(DIM), TextLayout::no_wrap()));
        });
        if top == (PaneTop::View { body: true }) {
            body(col);
            return;
        }
        // The column headers, over the rows' cells.
        col.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(CELL_GAP), padding: UiRect::axes(px(6), px(3)), ..default() }).with_children(|h| {
            h.spawn(Node { width: px(W_LEAD + CELL_GAP + W_SWATCH), flex_shrink: 0.0, ..default() });
            head(h, "Name", Some(SortKey::Name), sort, Node { flex_grow: 1.0, flex_basis: px(0), ..default() });
            head(h, "Type", Some(SortKey::Kind), sort, Node { width: px(W_KIND), flex_shrink: 0.0, ..default() });
            head(h, "Qty", None, sort, Node { width: px(W_QTY), flex_shrink: 0.0, justify_content: JustifyContent::FlexEnd, ..default() });
            head(h, "kg", Some(SortKey::Weight), sort, Node { width: px(W_KG), flex_shrink: 0.0, justify_content: JustifyContent::FlexEnd, ..default() });
        });
        col.spawn((Node { height: px(1), flex_shrink: 0.0, margin: UiRect::bottom(px(3)), ..default() }, BackgroundColor(LINE)));
        body(col);
    });
}

/// A column header; pressed, both panes sort by `key` (again: the other way; a third time: unsorted).
fn head(h: &mut ChildSpawnerCommands, label: &str, key: Option<SortKey>, sort: Sort, node: Node) {
    let (mark, on) = key.map_or(("", false), |k| (sort.mark(k), sort.key == k));
    let mut e = h.spawn(node);
    e.with_children(|c| {
        c.spawn((Text::new(format!("{label}{mark}")), theme::head(TAB_FONT), TextColor(if on { TEXT } else { DIM }), TextLayout::no_wrap(), Pickable::IGNORE));
    });
    if let Some(key) = key {
        e.observe(move |_: On<PointerPress>, mut ui: ResMut<PanelUi>| ui.sort.press(key));
    }
}

/// The strip down a pane's outer edge that its tabs hang in.
fn tab_strip() -> Node {
    Node { flex_direction: FlexDirection::Column, row_gap: px(4), width: px(TAB_W), flex_shrink: 0.0, ..default() }
}

/// How tall a tab is for its label (in capitals, in the heading face, measured from the face's own
/// widths) with 12 UI pixels of room at each end.
fn tab_height(label: &str) -> f32 {
    (theme::width(theme::Face::Head, &label.to_uppercase(), TAB_FONT) + 24.0).ceil()
}

/// A slim tab on a pane's edge, its label turned to read down it; `on`: the one the pane shows.
/// The caller observes its presses.
fn tab<'a>(strip: &'a mut ChildSpawnerCommands, label: &str, on: bool) -> EntityCommands<'a> {
    // (The label doesn't take part in the tab's height once turned, so the tab is sized for it.)
    let height = tab_height(label);
    let mut e = strip.spawn((Node { width: px(TAB_W), height: px(height), justify_content: JustifyContent::Center, align_items: AlignItems::Center, flex_shrink: 0.0, ..default() }, BackgroundColor(if on { PANE } else { TAB })));
    e.with_children(|t| {
        t.spawn((Text::new(label.to_uppercase()), theme::head(TAB_FONT), TextColor(if on { TEXT } else { DIM }), TextLayout::no_wrap(), Node { flex_shrink: 0.0, ..default() }, UiTransform::from_rotation(Rot2::degrees(90.0)), Pickable::IGNORE));
    });
    e
}

/// A small clickable label (a chip, a button); the caller observes its presses.
fn chip<'a>(parent: &'a mut ChildSpawnerCommands, text: String, on: bool) -> EntityCommands<'a> {
    // A plain light button with a worn edge (`theme::button`); the one that's on is gold.
    let mut e = parent.spawn((Node { padding: UiRect::axes(px(7), px(3)), justify_content: JustifyContent::Center, ..default() }, theme::button(on)));
    e.with_children(|b| {
        b.spawn((Text::new(text), theme::strong(ROW_FONT), TextColor(INK), TextLayout::no_wrap(), Pickable::IGNORE));
    });
    e
}

/// The top row. Left: the panel's name, or with stacks picked what to do with them (send them all
/// to the bags, the open container or the floor). Right: the one small control that brings out the
/// search box and the chips (or puts them away, cleared).
fn top_bar(c: &mut ChildSpawnerCommands, find: &Find, n: usize, kg: f32, open: Option<(u32, String)>) {
    c.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, min_height: px(30), flex_shrink: 0.0, ..default() }).with_children(|bar| {
        // The panel's name, centred over both panes (out of the layout: the controls either side
        // come and go without moving it). With stacks picked, the move-to row has the line.
        if n == 0 {
            bar.spawn((Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(0), bottom: px(0), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }, Pickable::IGNORE)).with_children(|t| {
                t.spawn((Text::new("INVENTORY"), theme::head(theme::TITLE), TextColor(TEXT), TextLayout::no_wrap(), Pickable::IGNORE));
            });
        }
        bar.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(6), align_items: AlignItems::Center, ..default() }).with_children(|l| {
            if n == 0 {
                return;
            }
            l.spawn((Text::new(format!("{n} picked ({kg:.1} kg), move to:")), font(ROW_FONT), TextColor(OK), TextLayout::no_wrap()));
            chip(l, "bags".into(), false).observe(|_: On<PointerPress>, mut send: ResMut<SendPicked>| send.0 = Some(Place::Carried));
            if let Some((id, name)) = open {
                chip(l, name, false).observe(move |_: On<PointerPress>, mut send: ResMut<SendPicked>| send.0 = Some(Place::Container(id)));
            }
            chip(l, "floor".into(), false).observe(|_: On<PointerPress>, mut send: ResMut<SendPicked>| send.0 = Some(Place::Ground));
            chip(l, "clear".into(), false).observe(|_: On<PointerPress>, mut picked: ResMut<Picked>| *picked = Picked::default());
        });
        chip(bar, if find.in_use() { "Find  x".into() } else { "Find  /".into() }, find.in_use()).observe(|_: On<PointerPress>, mut find: ResMut<Find>| find.press());
    });
}

/// The search box and the category chips: on show only while in use ([`Find::in_use`]).
fn toolbar(c: &mut ChildSpawnerCommands, find: &Find) {
    c.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(4), row_gap: px(4), align_items: AlignItems::Center, max_width: px(PANEL_W - 2.0 * theme::SHEET_PAD.0), ..default() }).with_children(|bar| {
        let text = match (find.query.is_empty(), find.typing) {
            (true, false) => "Search ( / )".to_string(),
            (_, true) => format!("Search: {}_", find.query),
            (false, false) => format!("Search: {}", find.query),
        };
        bar.spawn((Node { min_width: px(170), padding: UiRect::axes(px(7), px(3)), ..default() }, BackgroundColor(if find.typing { CHIP_ON } else { CHIP })))
            .with_children(|b| {
                b.spawn((Text::new(text), font(ROW_FONT), TextColor(if find.query.is_empty() && !find.typing { DIM } else { TEXT }), Pickable::IGNORE));
            })
            .observe(|_: On<PointerPress>, mut find: ResMut<Find>| find.typing = true);
        if !find.query.is_empty() {
            chip(bar, "x".into(), false).observe(|_: On<PointerPress>, mut find: ResMut<Find>| find.query.clear());
        }
        chip(bar, "All".into(), find.groups.is_empty()).observe(|_: On<PointerPress>, mut find: ResMut<Find>| find.groups.clear());
        for g in Group::ALL {
            chip(bar, g.name().into(), find.groups.contains(&g)).observe(move |_: On<PointerPress>, mut find: ResMut<Find>| find.toggle(g));
        }
    });
}

/// Whether the stack at `from`/`index` gets a row: it fits the search and chips, or it is picked
/// (a picked stack always shows, so what the bar will move is in view).
fn on_show(find: &Find, items: &[ItemDef], picked: &Picked, from: Place, index: u32, s: &ItemStack) -> bool {
    find.shows(items, s) || picked.keys.contains(&(from, index, s.item))
}

/// A line of plain dimmed text in a pane (nothing here, nothing that matches).
fn dim(col: &mut ChildSpawnerCommands, text: String) {
    col.spawn((Text::new(text), font(ROW_FONT), TextColor(DIM), Node { margin: UiRect::axes(px(6), px(3)), ..default() }));
}

/// What stands at the start of a row.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Lead {
    Nothing,
    /// The fold mark of a row of several stacks (`open`: they show under it); `key` is the row.
    Fold { open: bool, key: (Column, u16) },
    /// One of the stacks under an opened row: set in, with no swatch of its own.
    Child,
}

/// What a row says, cell by cell: every row of every pane has the same cells, so they line up.
#[derive(Clone, Debug)]
struct Cells {
    lead: Lead,
    /// The item's own colour, as a small mark before its name.
    swatch: Option<Color>,
    name: String,
    /// After the name, dimmed (stale, 2/3 left, moving).
    note: String,
    /// The item's kind; for what you hold and wear, the slot.
    kind: String,
    /// How many, when more than one.
    qty: String,
    kg: String,
    /// An empty slot: all of it dimmed.
    faint: bool,
}

impl Cells {
    /// A row that only says `name` and `kg` (tests).
    #[cfg(test)]
    fn plain(name: &str, kg: &str) -> Cells {
        Cells { lead: Lead::Nothing, swatch: None, name: name.into(), note: String::new(), kind: String::new(), qty: String::new(), kg: kg.into(), faint: false }
    }
}

/// A fixed-width cell of a row, its text at the start or (numbers) at the end.
fn cell(r: &mut ChildSpawnerCommands, width: f32, text: String, color: Color, end: bool) {
    r.spawn((Node { width: px(width), flex_shrink: 0.0, justify_content: if end { JustifyContent::FlexEnd } else { JustifyContent::FlexStart }, overflow: Overflow::clip_x(), ..default() }, Pickable::IGNORE)).with_children(|c| {
        c.spawn((Text::new(text), font(ROW_FONT), TextColor(color), TextLayout::no_wrap(), Pickable::IGNORE));
    });
}

/// A row's cells on a backing of `bg`: fold mark, swatch, name (and note), kind, count, weight.
fn cells_row<'a>(col: &'a mut ChildSpawnerCommands, cells: Cells, bg: Color) -> EntityCommands<'a> {
    let text = if cells.faint { DIM } else { TEXT };
    let mut e = col.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(CELL_GAP), padding: UiRect::axes(px(6), px(3)), flex_shrink: 0.0, ..default() }, BackgroundColor(bg)));
    e.with_children(|r| {
        match cells.lead {
            // The fold mark takes its own presses: they open and shut the row, and go no further
            // (the row under it would move its stacks).
            Lead::Fold { open, key } => {
                r.spawn((Node { width: px(W_LEAD), flex_shrink: 0.0, justify_content: JustifyContent::Center, ..default() }, BackgroundColor(CHIP)))
                    .with_children(|f| {
                        f.spawn((Text::new(if open { "-" } else { "+" }), font(ROW_FONT), TextColor(TEXT), Pickable::IGNORE));
                    })
                    .observe(move |mut ev: On<PointerPress>, mut ui: Option<ResMut<PanelUi>>| {
                        ev.propagate(false);
                        if let (PointerButton::Primary, Some(ui)) = (ev.button, ui.as_mut()) {
                            ui.toggle_fold(key);
                        }
                    });
            }
            _ => {
                r.spawn((Node { width: px(W_LEAD), flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
            }
        }
        let swatch = cells.swatch.filter(|_| cells.lead != Lead::Child).unwrap_or(Color::NONE);
        r.spawn((Node { width: px(W_SWATCH), height: px(W_SWATCH), flex_shrink: 0.0, border_radius: BorderRadius::all(px(2)), ..default() }, BackgroundColor(swatch), Pickable::IGNORE));
        // One line whatever the name: it never wraps, and a name too long for its cell is cut
        // off at the next cell instead of pushing it out.
        r.spawn((Node { min_width: px(0), flex_grow: 1.0, flex_shrink: 1.0, flex_basis: px(0), column_gap: px(8), overflow: Overflow::clip_x(), ..default() }, Pickable::IGNORE)).with_children(|l| {
            l.spawn((Text::new(cells.name), font(ROW_FONT), TextColor(text), TextLayout::no_wrap(), Node { flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
            if !cells.note.is_empty() {
                l.spawn((Text::new(cells.note), font(ROW_FONT), TextColor(DIM), TextLayout::no_wrap(), Node { flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
            }
        });
        cell(r, W_KIND, cells.kind, DIM, false);
        cell(r, W_QTY, cells.qty, text, true);
        cell(r, W_KG, cells.kg, DIM, true);
    });
    e
}

/// A slot with nothing in it, in the pane of what you hold and wear.
fn empty_row(col: &mut ChildSpawnerCommands, slot: &str) {
    cells_row(col, Cells { lead: Lead::Nothing, swatch: None, name: "-".into(), note: String::new(), kind: slot.into(), qty: String::new(), kg: String::new(), faint: true }, Color::NONE);
}

/// A clickable row of `cells`, highlighted if `picked`. A press is only recorded here
/// ([`Clicks`]); [`send_clicks`] decides what it sends. Ctrl-click picks it (or unpicks it)
/// instead, and Shift-click picks every row from the last one picked to it; a row standing for
/// several stacks picks them all.
fn row(col: &mut ChildSpawnerCommands, cells: Cells, action: RowAction, picked: bool) {
    let rest = if picked { ROW_PICKED } else { ROW };
    let mut e = cells_row(col, cells, rest);
    clickable(&mut e, action, rest);
}

/// What makes a row (or a tile of the body view) of `action` one to press: the hover colour (back
/// to `rest` after), the item under the pointer for the hotbar, and its presses.
fn clickable(e: &mut EntityCommands, action: RowAction, rest: Color) {
    // The item under the pointer, for the hotbar's keys (not a recipe's row).
    let item = action.left.filter(|_| action.craft.is_none()).map(|_| action.stack.item);
    e.observe(move |ev: On<PointerOver>, mut q: Query<&mut BackgroundColor>, mut hovered: Option<ResMut<HoveredItem>>| {
        if let Ok(mut bg) = q.get_mut(ev.entity) {
            bg.0 = ROW_HOVER;
        }
        if let Some(h) = hovered.as_mut() {
            h.0 = item.map(|i| (ev.entity, i));
        }
    })
    .observe(move |ev: On<PointerOut>, mut q: Query<&mut BackgroundColor>, mut hovered: Option<ResMut<HoveredItem>>| {
        if let Ok(mut bg) = q.get_mut(ev.entity) {
            bg.0 = rest;
        }
        if let Some(h) = hovered.as_mut().filter(|h| h.0.is_some_and(|(e, _)| e == ev.entity)) {
            h.0 = None;
        }
    })
    .observe(move |ev: On<PointerPress>, keys: Res<ButtonInput<KeyCode>>, mut clicks: ResMut<Clicks>, mut picked: Option<ResMut<Picked>>, listed: Option<Res<Listed>>| {
        let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
        let keys_of = action.keys();
        if (shift || ctrl)
            && ev.button == PointerButton::Primary
            && !keys_of.is_empty()
            && let Some(picked) = picked.as_mut()
        {
            match (shift, &listed) {
                (true, Some(listed)) => picked.extend_to_row(&keys_of, &listed.0),
                _ => picked.toggle_all(&keys_of),
            }
            return;
        }
        let one = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
        clicks.0.push(RowClick { action: action.clone(), button: ev.button, one });
    });
}

// ---- The right-click menu (#95) ----

/// What a menu entry does.
#[derive(Clone, Debug)]
enum MenuDo {
    /// Send this intent about the stack at `from`/`index` (`whole`: all of it, else part).
    Send { intent: Intent, from: Place, index: u32, whole: bool },
    /// Open the health panel to pick the wound to use this item on.
    UseOn(u16),
}

/// One line of the menu.
#[derive(Clone, Debug)]
struct MenuEntry {
    label: String,
    act: MenuDo,
}

/// Everything that can be done with the stack `s` at `from`/`index`, the likeliest first: one
/// level, no sub-menus. `open` is the container the player has open (id, name).
fn menu_entries(player: u32, items: &[ItemDef], from: Place, index: u32, s: &ItemStack, open: Option<(u32, &str)>) -> Vec<MenuEntry> {
    let Some(def) = items.get(s.item as usize) else { return Vec::new() };
    let mut out = Vec::new();
    let mk = |label: String, intent: Option<Intent>, whole: bool| intent.map(|intent| MenuEntry { label, act: MenuDo::Send { intent, from, index, whole } });
    let n = s.count;
    // Half of 2 or 3 is one, which "one" already offers.
    let half = n / 2;
    let halves = half > 1;
    let into = |to: Place, count: u16| move_intent(player, from, index, to, count);
    match from {
        Place::Carried => {
            match (Treatment::of_item(def), def.category) {
                (Some(t), _) if !t.on_wound() => out.extend(mk(format!("Take {}", def.name.to_lowercase()), Some(Intent::Treat { player, wound: None, action: t, item: Some(s.item) }), false)),
                (Some(_), _) => out.push(MenuEntry { label: "Use on a wound...".into(), act: MenuDo::UseOn(s.item) }),
                (None, ItemCategory::Food) => out.extend(mk("Eat".into(), Some(Intent::Use { player, from, index }), false)),
                (None, ItemCategory::Drink) => out.extend(mk("Drink".into(), Some(Intent::Use { player, from, index }), false)),
                // Something a mod's script gives a use to (#25).
                (None, _) if def.usable() => out.extend(mk("Use".into(), Some(Intent::Use { player, from, index }), false)),
                // A gun's magazine (#72): using it pushes loose rounds into it.
                (None, _) if def.has_tag(zombye_sim::ranged::TAG_MAGAZINE) => out.extend(mk("Fill with rounds".into(), Some(Intent::Use { player, from, index }), false)),
                (None, ItemCategory::Clothing) => out.extend(mk("Wear".into(), Some(Intent::Wear { player, index }), false)),
                (None, ItemCategory::Bag) => out.extend(mk("Put it on".into(), Some(Intent::Equip { player, index }), false)),
                (None, _) => out.extend(mk("Hold in hand".into(), Some(Intent::Equip { player, index }), false)),
            }
            if let Some((id, name)) = open {
                out.extend(mk(format!("Put in {name}"), into(Place::Container(id), 0), true));
            }
            out.extend(mk("Drop".into(), into(Place::Ground, 0), true));
            if n > 1 {
                if let Some((id, name)) = open {
                    out.extend(mk(format!("Put one in {name}"), into(Place::Container(id), 1), false));
                    if halves {
                        out.extend(mk(format!("Put half in {name} ({half})"), into(Place::Container(id), half), false));
                    }
                }
                out.extend(mk("Drop one".into(), into(Place::Ground, 1), false));
                if halves {
                    out.extend(mk(format!("Drop half ({half})"), into(Place::Ground, half), false));
                }
            }
        }
        Place::Slot(_) | Place::Worn => {
            out.extend(mk(if from == Place::Worn { "Take off".into() } else { "Put in bags".into() }, into(Place::Carried, 0), true));
            if let Some((id, name)) = open {
                out.extend(mk(format!("Put in {name}"), into(Place::Container(id), 0), true));
            }
            out.extend(mk("Drop".into(), into(Place::Ground, 0), true));
        }
        Place::Container(_) | Place::Ground => {
            let take = if from == Place::Ground { "Pick up" } else { "Take" };
            if def.weapon.is_some() {
                out.extend(mk("Take in hand".into(), into(Place::Slot(Slot::Hand), 0), true));
            }
            out.extend(mk(take.into(), into(Place::Carried, 0), true));
            if from == Place::Ground
                && let Some((id, name)) = open
            {
                out.extend(mk(format!("Put in {name}"), into(Place::Container(id), 0), true));
            }
            if n > 1 {
                out.extend(mk(format!("{take} one"), into(Place::Carried, 1), false));
                if halves {
                    out.extend(mk(format!("{take} half ({half})"), into(Place::Carried, half), false));
                }
            }
        }
    }
    out
}

/// The menu on screen, if any.
#[derive(Resource, Default)]
pub struct ItemMenu {
    open: Option<MenuState>,
    /// The entry picked (a click or a number key), done by [`menu_act`].
    chosen: Option<usize>,
    /// Opened this frame (the press that opened it doesn't close it).
    fresh: bool,
    version: u32,
}

struct MenuState {
    title: String,
    stack: ItemStack,
    entries: Vec<MenuEntry>,
    /// Where (the pointer when it opened); `None`: beside the panel.
    at: Option<Vec2>,
}

impl ItemMenu {
    fn show(&mut self, title: String, stack: ItemStack, entries: Vec<MenuEntry>, at: Option<Vec2>) {
        self.open = Some(MenuState { title, stack, entries, at });
        self.chosen = None;
        self.fresh = true;
        self.version += 1;
    }

    fn close(&mut self) {
        if self.open.take().is_some() {
            self.version += 1;
        }
        self.chosen = None;
    }
}

impl ItemMenu {
    /// A menu is on screen (its number keys are its own).
    pub fn is_up(&self) -> bool {
        self.open.is_some()
    }
}

/// The item of the inventory row the pointer is over (and the row), for the hotbar: a number key
/// over a row puts that item on that slot (`hotbar.rs`).
#[derive(Resource, Default)]
pub struct HoveredItem(pub Option<(Entity, u16)>);

impl HoveredItem {
    /// The item hovered, while its row is still there (rows are rebuilt when the panel changes).
    pub fn item(&self, rows: &Query<(), With<Node>>) -> Option<u16> {
        self.0.filter(|(e, _)| rows.contains(*e)).map(|(_, i)| i)
    }
}

#[derive(Component)]
struct MenuRoot;

const DIGITS: [KeyCode; 9] = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9];

/// 1-9 pick an entry, Esc closes; those keys go no further while a menu is up.
fn menu_keys(mut keys: ResMut<ButtonInput<KeyCode>>, mut menu: ResMut<ItemMenu>) {
    let Some(m) = &menu.open else { return };
    let n = m.entries.len();
    if let Some(k) = DIGITS.iter().position(|d| keys.just_pressed(*d)).filter(|&k| k < n) {
        menu.chosen = Some(k);
    } else if keys.just_pressed(KeyCode::Escape) {
        menu.close();
    }
    for d in DIGITS.iter().chain([&KeyCode::Escape]) {
        keys.reset(*d);
    }
}

/// Open the menu for the stack at `from`/`index`, at `at`.
fn open_menu(menu: &mut ItemMenu, link: &ServerLink, join: &JoinInfo, me: u32, from: Place, index: u32, at: Option<Vec2>) -> bool {
    let (Some(v), floor) = panel_view(link, me) else { return false };
    let Some(s) = stack_at(&v, &floor, from, index) else { return false };
    let open = v.open.as_ref().map(|c| (c.id, c.name.to_lowercase()));
    let entries = menu_entries(me, &join.items, from, index, &s, open.as_ref().map(|(id, n)| (*id, n.as_str())));
    if entries.is_empty() {
        return false;
    }
    menu.show(name_of(&join.items, link.curr.game_minute, &s), s, entries, at);
    true
}

/// The menu on a ground item in the world: right-click it with the panel open, or tap the right
/// mouse on it with the panel shut (held, it aims). Only items within reach (our floor).
#[allow(clippy::too_many_arguments)]
fn world_menu(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<crate::camera::IsoCamera>>,
    hover: Res<bevy::picking::hover::HoverMap>,
    nodes: Query<(), With<Node>>,
    panel: Res<InventoryPanel>,
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    me: Res<LocalPlayer>,
    mut menu: ResMut<ItemMenu>,
    mut tap: Local<Option<(f32, Vec2)>>,
) {
    let (Ok(window), Ok((camera, cam_tf))) = (windows.single(), cams.single()) else { return };
    let Some(cursor) = window.cursor_position() else { return };
    let now = time.elapsed_secs();
    let mut want = false;
    if mouse.just_pressed(MouseButton::Right) {
        let over_ui = hover.values().any(|m| m.keys().any(|e| nodes.contains(*e)));
        if over_ui || menu.fresh || me.dead {
            *tap = None;
            return;
        }
        if panel.open {
            want = true;
        } else {
            *tap = Some((now, cursor));
        }
    }
    if mouse.just_released(MouseButton::Right)
        && let Some((t, p)) = tap.take()
    {
        want = now - t < 0.25 && p.distance(cursor) < 8.0;
    }
    if !want {
        return;
    }
    let Ok(ray) = camera.viewport_to_world(cam_tf, crate::render_scale::to_view(camera, window, cursor)) else { return };
    if ray.direction.y.abs() < 1e-4 {
        return;
    }
    let hit = ray.origin + *ray.direction * (-ray.origin.y / ray.direction.y);
    let ground = Vec2::new(hit.x, hit.z);
    let reach = link.curr.floor_of(me.id);
    let nearest = link.curr.items.iter().filter(|it| reach.contains(&it.id)).map(|it| (it.id, it.pos.distance(ground))).filter(|(_, d)| *d < 0.8).min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((id, _)) = nearest {
        open_menu(&mut menu, &link, &join, me.id, Place::Ground, id, Some(cursor));
    }
}

/// `--menu SPEC` (for pictures): the menu open from the start on `carried:N`, `worn:N`, `hand`,
/// `container:N` or `ground[:N]` (the Nth item on our floor), beside the panel.
fn menu_from_options(options: Option<Res<crate::Options>>, link: Res<ServerLink>, join: Res<JoinInfo>, me: Res<LocalPlayer>, mut menu: ResMut<ItemMenu>, mut done: Local<bool>) {
    if *done {
        return;
    }
    let Some(spec) = options.as_ref().and_then(|o| o.menu.clone()) else {
        *done = true;
        return;
    };
    let (kind, n) = spec.split_once(':').map_or((spec.as_str(), 0), |(k, n)| (k, n.parse().unwrap_or(0)));
    let (Some(v), floor) = panel_view(&link, me.id) else { return };
    let (from, index) = match kind {
        "carried" => (Place::Carried, n),
        "worn" => (Place::Worn, n),
        "hand" => (Place::Slot(Slot::Hand), 0),
        "container" => match &v.open {
            Some(c) => (Place::Container(c.id), n),
            None => return,
        },
        "ground" => match floor.get(n as usize) {
            Some((id, _)) => (Place::Ground, *id),
            None => return,
        },
        _ => {
            *done = true;
            return;
        }
    };
    if open_menu(&mut menu, &link, &join, me.id, from, index, None) {
        *done = true;
    }
}

/// `--menu-pick N[@SECS]` (for recordings): press the open menu's number key N after SECS.
fn script_menu(time: Res<Time>, options: Option<Res<crate::Options>>, mut menu: ResMut<ItemMenu>, mut done: Local<bool>) {
    let Some((n, after)) = options.as_ref().and_then(|o| o.menu_pick) else { return };
    if !*done && time.elapsed_secs() >= after && menu.open.as_ref().is_some_and(|m| n <= m.entries.len()) {
        menu.chosen = Some(n - 1);
        *done = true;
    }
}

/// Do the entry picked, or close the menu on a press anywhere else, and when the panel opens or
/// shuts under it.
#[allow(clippy::too_many_arguments)]
fn menu_act(time: Res<Time>, mouse: Res<ButtonInput<MouseButton>>, me: Res<LocalPlayer>, panel: Res<InventoryPanel>, mut panel_was: Local<bool>, mut menu: ResMut<ItemMenu>, mut flight: ResMut<InFlight>, mut out: ResMut<Outbox>, mut health: ResMut<crate::health_ui::HealthPanel>) {
    if *panel_was != panel.open {
        *panel_was = panel.open;
        if !menu.fresh {
            menu.close();
        }
    }
    if let Some(k) = menu.chosen.take() {
        let picked = menu.open.as_ref().and_then(|m| m.entries.get(k).cloned().map(|e| (e, m.stack)));
        menu.close();
        let Some((entry, stack)) = picked else { return };
        match entry.act {
            MenuDo::UseOn(item) => (health.open, health.use_item, health.selected) = (true, Some(item), None),
            MenuDo::Send { intent, from, index, whole } => {
                if flight.ask(from, index, stack, !whole, time.elapsed_secs()) {
                    out.0.push(intent);
                }
            }
        }
    } else if menu.open.is_some() && !menu.fresh && (mouse.just_pressed(MouseButton::Left) || mouse.just_pressed(MouseButton::Right)) {
        menu.close();
    }
    if me.dead {
        menu.close();
    }
    menu.fresh = false;
}

/// About how big a menu of `entries` lines is drawn (UI pixels), for keeping it on screen.
fn menu_size(entries: usize) -> Vec2 {
    Vec2::new(280.0 + 2.0 * theme::STRIP_PAD.0, 34.0 + 2.0 * theme::STRIP_PAD.1 + 25.0 * entries as f32)
}

/// Draw the menu when it changes: its title, then the entries numbered, the first highlighted.
fn draw_menu(mut commands: Commands, menu: Res<ItemMenu>, ui: Res<UiScale>, windows: Query<&Window, With<bevy::window::PrimaryWindow>>, mut drawn: Local<u32>, roots: Query<Entity, With<MenuRoot>>) {
    if *drawn == menu.version {
        return;
    }
    *drawn = menu.version;
    for e in &roots {
        commands.entity(e).despawn();
    }
    let Some(m) = &menu.open else { return };
    // The pointer in UI pixels (the UI scale, #109, scales every `px`), kept on screen.
    let at = m.at.map(|at| {
        let at = at / ui.0.max(0.1) + Vec2::splat(4.0);
        match windows.single() {
            Ok(w) => at.min((Vec2::new(w.width(), w.height()) / ui.0.max(0.1) - menu_size(m.entries.len())).max(Vec2::ZERO)),
            Err(_) => at,
        }
    });
    // On a painted strip of its own, like every small piece of the HUD (`theme::strip`).
    let pad = UiRect::axes(px(theme::STRIP_PAD.0), px(theme::STRIP_PAD.1));
    let node = match at {
        Some(at) => Node { position_type: PositionType::Absolute, left: px(at.x), top: px(at.y), flex_direction: FlexDirection::Column, row_gap: px(1), padding: pad, min_width: px(190), ..default() },
        None => Node { position_type: PositionType::Absolute, right: px(PANEL_W + 30.0), top: px(140), flex_direction: FlexDirection::Column, row_gap: px(1), padding: pad, min_width: px(190), ..default() },
    };
    commands
        .spawn((MenuRoot, node, GlobalZIndex(20), theme::strip()))
        .with_children(|c| {
            c.spawn((Text::new(m.title.to_uppercase()), theme::head(theme::BODY), TextColor(DIM), Node { align_self: AlignSelf::Center, padding: UiRect::axes(px(6), px(2)), ..default() }));
            c.spawn((Node { height: px(2), margin: UiRect::bottom(px(3)), ..default() }, theme::rule(), Pickable::IGNORE));
            for (k, e) in m.entries.iter().enumerate() {
                let rest = if k == 0 { ROW_PICKED } else { ROW };
                c.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(10), padding: UiRect::axes(px(6), px(3)), ..default() }, BackgroundColor(rest)))
                    .with_children(|r| {
                        r.spawn((Text::new(format!("{}", k + 1)), theme::strong(theme::SMALL), TextColor(OK), Pickable::IGNORE));
                        r.spawn((Text::new(e.label.clone()), font(theme::BODY), TextColor(TEXT), Pickable::IGNORE));
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
                    .observe(move |ev: On<PointerPress>, mut menu: ResMut<ItemMenu>| {
                        if ev.button == PointerButton::Primary {
                            menu.chosen = Some(k);
                        }
                    });
            }
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
        RowClick { action: RowAction { stack, title: String::new(), left, more: Vec::new(), menu: Vec::new(), craft: None }, button, one }
    }

    fn shipped_items() -> Vec<ItemDef> {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        zombye_net::content::load_config(&data, 1).unwrap().items
    }

    /// Stacks of one item collapse into one row where the first lay; the headers sort the rows by
    /// name, by type (then name) and by weight (the whole row's, heaviest first), either way round.
    #[test]
    fn a_finished_search_opens_the_panel() {
        // Searching container 7, then done with it open.
        assert!(search_done(Some(7), None, Some(7)));
        // Still searching; nothing searched (a save loaded with one open); called off, nothing open;
        // called off with another container open.
        assert!(!search_done(Some(7), Some(7), None));
        assert!(!search_done(None, None, Some(7)));
        assert!(!search_done(Some(7), None, None));
        assert!(!search_done(Some(7), None, Some(3)));
        // One search done and the next under way.
        assert!(search_done(Some(7), Some(8), Some(7)));
    }

    #[test]
    fn stacks_collapse_and_rows_sort() {
        let items = shipped_items();
        let stack = |id: &str, n: u16| ItemStack::new(items.iter().position(|d| d.id == id).unwrap() as u16, n);
        let carried = [stack("t_shirt", 1), stack("tinned_beans", 6), stack("rope", 1), stack("t_shirt", 1), stack("water_bottle", 2), stack("tinned_beans", 4), stack("t_shirt", 1)];
        let lines = || carried.iter().enumerate().map(|(k, s)| Line { from: Place::Carried, index: k as u32, stack: *s });
        let ids = |rows: &[Vec<Line>]| rows.iter().map(|r| items[r[0].stack.item as usize].id.as_str()).collect::<Vec<_>>();
        let rows = arrange(&items, lines(), Sort::default());
        assert_eq!(ids(&rows), ["t_shirt", "tinned_beans", "rope", "water_bottle"], "as they lie, each item once");
        assert_eq!(rows[0].iter().map(|l| l.index).collect::<Vec<_>>(), [0, 3, 6], "the row stands for every stack of it");
        assert_eq!(rows[1].iter().map(|l| l.stack.count).sum::<u16>(), 10);
        let mut sort = Sort::default();
        sort.press(SortKey::Name);
        assert_eq!(ids(&arrange(&items, lines(), sort)), ["water_bottle", "rope", "t_shirt", "tinned_beans"], "Bottle of water, Rope, T-shirt, Tin of beans");
        sort.press(SortKey::Name);
        assert_eq!(ids(&arrange(&items, lines(), sort)), ["tinned_beans", "t_shirt", "rope", "water_bottle"]);
        sort.press(SortKey::Name);
        assert_eq!(sort, Sort::default(), "a third press: as they lie again");
        sort.press(SortKey::Kind);
        assert_eq!(ids(&arrange(&items, lines(), sort)), ["tinned_beans", "water_bottle", "t_shirt", "rope"], "food, drink, clothing, materials: the chips' order");
        sort.press(SortKey::Weight);
        assert_eq!((sort.mark(SortKey::Weight), sort.mark(SortKey::Name)), (" v", ""));
        assert_eq!(ids(&arrange(&items, lines(), sort)), ["tinned_beans", "water_bottle", "rope", "t_shirt"], "ten tins outweigh the rest");
        sort.press(SortKey::Weight);
        assert_eq!(ids(&arrange(&items, lines(), sort))[0], "t_shirt");
        assert_eq!((SortKey::parse("Type"), SortKey::parse("kg"), SortKey::parse("x")), (Some(SortKey::Kind), Some(SortKey::Weight), None));
    }

    /// A click on a collapsed row moves every stack it stands for, from the back first (so the
    /// indices still to go don't shift), each asked for once; Alt-click takes one of the first.
    #[test]
    fn a_collapsed_row_moves_all_its_stacks() {
        let shirts = |more: Vec<Line>, one: bool| RowClick { action: RowAction { stack: beans(1), title: String::new(), left: Some((Place::Carried, 0, Place::Container(4))), more, menu: Vec::new(), craft: None }, button: PointerButton::Primary, one };
        let more = vec![Line { from: Place::Carried, index: 3, stack: beans(1) }, Line { from: Place::Carried, index: 6, stack: beans(2) }];
        let mut f = InFlight::default();
        let sent = click_intents(&shirts(more.clone(), false), 1, &mut f, 0.0);
        let indices: Vec<u32> = sent.iter().map(|i| match i {
            Intent::MoveItem { from: Place::Carried, index, to: Place::Container(4), count: 0, .. } => *index,
            other => panic!("{other:?}"),
        }).collect();
        assert_eq!(indices, [6, 3, 0]);
        assert!(click_intents(&shirts(more.clone(), false), 1, &mut f, 0.1).is_empty(), "all on their way");
        let mut f = InFlight::default();
        let sent = click_intents(&shirts(more.clone(), true), 1, &mut f, 0.0);
        assert!(matches!(sent[..], [Intent::MoveItem { index: 0, count: 1, .. }]), "{sent:?}");
        // Its keys are every stack's: Ctrl-click picks them all (again: unpicks them), and a
        // Shift-click's range takes the whole row in from either side.
        let keys = shirts(more, false).action.keys();
        assert_eq!(keys, vec![(Place::Carried, 0, 3), (Place::Carried, 3, 3), (Place::Carried, 6, 3)]);
        let mut p = Picked::default();
        p.toggle_all(&keys);
        assert_eq!(p.keys, keys);
        p.toggle_all(&keys);
        assert!(p.keys.is_empty());
        // Listed as drawn: a row of 1, the collapsed row (0, 3, 6), a row of 2.
        let listed: Vec<Key> = [1, 0, 3, 6, 2].into_iter().map(|i| (Place::Carried, i, 3)).collect();
        let mut p = Picked::default();
        p.toggle(listed[0]);
        p.extend_to_row(&keys, &listed);
        assert_eq!(p.keys, listed[..4].to_vec(), "from above: down to its last stack");
        let mut p = Picked::default();
        p.toggle(listed[4]);
        p.extend_to_row(&keys, &listed);
        assert_eq!(p.keys.len(), 4, "from below: up to its first");
        assert!(!p.keys.contains(&listed[0]));
    }

    /// The panes' state: the right pane shows the open container unless its Floor tab is picked
    /// (a click on what you carry goes where it shows), a newly opened container comes to the
    /// front, the move-all buttons take what a pane lists but never what you hold or wear, and
    /// the Find control shows the search and chips only while they're wanted.
    #[test]
    fn tabs_move_all_and_the_find_control() {
        let mut ui = PanelUi::default();
        assert_eq!((ui.right(None), ui.right(Some(7))), (Place::Ground, Place::Container(7)));
        ui.saw(Some(7));
        ui.floor = true;
        assert_eq!(ui.right(Some(7)), Place::Ground);
        ui.saw(Some(7));
        assert!(ui.floor, "the same container: the tab stays");
        ui.saw(Some(9));
        assert_eq!(ui.right(Some(9)), Place::Container(9), "another one opened: to the front");
        // `--tab floor` holds for the container open at the start.
        let mut ui = PanelUi { floor: true, ..default() };
        ui.saw(Some(7));
        assert!(ui.floor);
        // Fold marks: shut by default, opened by a press; with `--expand` the other way round.
        assert!(!ui.unfolded(Column::You, 3));
        ui.toggle_fold((Column::You, 3));
        assert!(ui.unfolded(Column::You, 3) && !ui.unfolded(Column::Floor, 3));
        ui.unfold_all = true;
        assert!(!ui.unfolded(Column::You, 3) && ui.unfolded(Column::Floor, 3));
        let mut listed: Vec<Key> = vec![(Place::Slot(Slot::Back), 0, 9), (Place::Worn, 0, 8)];
        listed.extend(carried(3));
        listed.push((Place::Ground, 40, 3));
        assert_eq!(all_of(Column::You, &listed), carried(3));
        assert_eq!(all_of(Column::Floor, &listed), vec![(Place::Ground, 40, 3)]);
        assert!(all_of(Column::Container, &listed).is_empty());
        let mut f = Find::default();
        assert!(!f.in_use());
        f.press();
        assert!(f.in_use());
        f.toggle(Group::Food);
        f.query = "bean".into();
        f.press();
        assert_eq!(f, Find::default(), "put away, cleared");
        f.groups = vec![Group::Food];
        assert!(f.in_use(), "a chip set from the start (--filter) shows them");
    }

    /// The fold mark of a collapsed row, through Bevy's input and picking: a press on it opens
    /// the row and goes no further (nothing is moved); a press beside it on the row is the row's.
    #[test]
    fn the_fold_mark_opens_a_row_without_moving_it() {
        let mut app = ui_app();
        app.init_resource::<Clicks>().init_resource::<PanelUi>().init_resource::<ButtonInput<KeyCode>>();
        app.world_mut().spawn(Camera2d);
        {
            let world = app.world_mut();
            let mut commands = world.commands();
            commands.spawn(Node { position_type: PositionType::Absolute, top: px(0), left: px(0), width: px(300), flex_direction: FlexDirection::Column, ..default() }).with_children(|col| {
                let cells = Cells { lead: Lead::Fold { open: false, key: (Column::You, 3) }, ..Cells::plain("T-shirt", "0.5") };
                row(col, cells, RowAction { stack: beans(1), title: String::new(), left: Some((Place::Carried, 0, Place::Ground)), more: vec![Line { from: Place::Carried, index: 2, stack: beans(1) }], menu: Vec::new(), craft: None }, false);
            });
            world.flush();
        }
        for _ in 0..3 {
            app.update();
        }
        let window = app.world_mut().query_filtered::<Entity, With<PrimaryWindow>>().single(app.world()).unwrap();
        let click = |app: &mut App, at: Vec2| {
            let moved = CursorMoved { window, position: at, delta: None };
            app.world_mut().write_message(moved.clone());
            app.world_mut().write_message(WindowEvent::CursorMoved(moved));
            app.update();
            for state in [ButtonState::Pressed, ButtonState::Released] {
                let e = MouseButtonInput { button: MouseButton::Left, state, window };
                app.world_mut().write_message(e);
                app.world_mut().write_message(WindowEvent::MouseButtonInput(e));
                app.update();
            }
            app.update();
        };
        // The mark is the row's first cell: 6 px of padding, then W_LEAD wide.
        click(&mut app, Vec2::new(6.0 + W_LEAD / 2.0, 10.0));
        assert_eq!(app.world().resource::<PanelUi>().folds, vec![(Column::You, 3)], "opened");
        assert!(app.world().resource::<Clicks>().0.is_empty(), "and nothing moved");
        click(&mut app, Vec2::new(120.0, 10.0));
        assert_eq!(app.world().resource::<Clicks>().0.len(), 1, "beside it, the row's own press");
        assert_eq!(app.world().resource::<PanelUi>().folds.len(), 1);
    }

    /// A stack asked for is asked for once; one at a time (Alt) as many times as there are;
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
        // Alt-clicks on three: three requests, then none.
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

    /// Carried rows 0..n of beans (item 3), as the You column lists them.
    fn carried(n: u32) -> Vec<Key> {
        (0..n).map(|i| (Place::Carried, i, 3)).collect()
    }

    /// Shift-click picks every row from the anchor (the last row Ctrl-clicked) to this one,
    /// forward or backward; with no anchor, just the row (which becomes the anchor).
    #[test]
    fn shift_click_picks_a_range_forward_and_backward() {
        let rows = carried(6);
        let mut p = Picked::default();
        p.toggle(rows[1]);
        p.extend_to(rows[4], &rows);
        assert_eq!(p.keys, rows[1..=4].to_vec(), "forward");
        let mut p = Picked::default();
        p.toggle(rows[4]);
        p.extend_to(rows[1], &rows);
        let mut got = p.keys.clone();
        got.sort_by_key(|k| k.1);
        assert_eq!(got, rows[1..=4].to_vec(), "backward");
        // No anchor: just the one row, and it anchors the next range.
        let mut p = Picked::default();
        p.extend_to(rows[2], &rows);
        assert_eq!(p.keys, vec![rows[2]]);
        p.extend_to(rows[0], &rows);
        assert_eq!(p.keys.len(), 3);
        // A range doesn't reach into another column: an anchor on the floor starts afresh.
        let mut listed = rows.clone();
        listed.push((Place::Ground, 40, 3));
        let mut p = Picked::default();
        p.toggle((Place::Ground, 40, 3));
        p.extend_to(rows[3], &listed);
        assert_eq!(p.keys, vec![(Place::Ground, 40, 3), rows[3]]);
    }

    /// Ctrl adds single rows and Shift adds a range to them; neither drops what was picked.
    #[test]
    fn ctrl_and_shift_add_up() {
        let rows = carried(8);
        let mut p = Picked::default();
        p.toggle(rows[0]);
        p.toggle(rows[6]);
        // Shift from the last Ctrl-click (6) to 3: 3..=6, with 0 still picked.
        p.extend_to(rows[3], &rows);
        let mut got = p.keys.clone();
        got.sort_by_key(|k| k.1);
        assert_eq!(got, vec![rows[0], rows[3], rows[4], rows[5], rows[6]]);
        // Ctrl-click one in the range off again; another Shift-click doesn't pick it twice.
        p.toggle(rows[4]);
        assert!(!p.keys.contains(&rows[4]));
        p.extend_to(rows[7], &rows);
        let mut got = p.keys.clone();
        got.sort_by_key(|k| k.1);
        assert_eq!(got, vec![rows[0], rows[3], rows[4], rows[5], rows[6], rows[7]], "from 4 (the last Ctrl-click) to 7");
        // Shift-click again extends from the same anchor: nothing new, nothing doubled.
        p.extend_to(rows[5], &rows);
        assert_eq!(p.keys.len(), 6);
    }

    /// A range across an active search picks only the rows on show: the ones the filter hides
    /// between the two ends stay unpicked.
    #[test]
    fn a_range_under_a_search_picks_only_the_rows_shown() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let config = zombye_net::content::load_config(&data, 1).unwrap();
        let items = &config.items;
        let stack = |id: &str| ItemStack::one(items.iter().position(|d| d.id == id).unwrap() as u16);
        let carried = [stack("tinned_beans"), stack("rag"), stack("tinned_beans"), stack("kitchen_knife"), stack("tinned_beans"), stack("rag")];
        let find = Find { query: "bean".into(), ..default() };
        let mut p = Picked::default();
        // What the panel lists under the search, by the same rule `rebuild` draws rows by.
        let listed = |p: &Picked| -> Vec<Key> { carried.iter().enumerate().filter(|(k, s)| on_show(&find, items, p, Place::Carried, *k as u32, s)).map(|(k, s)| (Place::Carried, k as u32, s.item)).collect() };
        let rows = listed(&p);
        assert_eq!(rows.iter().map(|k| k.1).collect::<Vec<_>>(), vec![0, 2, 4]);
        p.toggle(rows[0]);
        p.extend_to(rows[2], &rows);
        assert_eq!(p.keys.iter().map(|k| k.1).collect::<Vec<_>>(), vec![0, 2, 4], "the rag and the knife between are hidden, so not picked");
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
        f.groups = Group::parse_list("weapons");
        assert!(f.shows(items, &knife) && !f.shows(items, &beans));
        f.groups = vec![Group::Food];
        assert!(f.shows(items, &beans) && !f.shows(items, &knife));
        // Chips are toggles: a second one adds its kind, pressing one again takes it off.
        f.toggle(Group::Weapons);
        assert!(f.shows(items, &beans) && f.shows(items, &knife) && !f.shows(items, &rag));
        f.toggle(Group::Food);
        assert!(!f.shows(items, &beans) && f.shows(items, &knife));
        // With a search too, both must hold.
        f.toggle(Group::Food);
        f.query = "knife".into();
        assert!(!f.shows(items, &beans) && f.shows(items, &knife));
        // Materials are a chip of their own (#94); junk is Other.
        let rope = stack("rope");
        assert_eq!(Group::of(items[rope.item as usize].category), Group::Materials);
        assert_eq!(Group::of(ItemCategory::Junk), Group::Other);
        // `--filter` takes several, any case, and ignores what isn't a chip.
        assert_eq!(Group::parse_list("drink, FOOD,nonsense"), vec![Group::Food, Group::Drink]);
        // Every category has a chip, and every chip a category.
        for g in Group::ALL {
            assert_eq!(Group::parse(g.name()), Some(g));
        }
    }

    /// A column's "pick all" picks what the column shows (under a search, only that), never what
    /// you hold or wear, and never another column's rows; pressed again, it unpicks them.
    #[test]
    fn pick_all_picks_a_columns_rows_on_show() {
        let mut listed: Vec<Key> = vec![(Place::Slot(Slot::Back), 0, 9), (Place::Worn, 0, 8)];
        listed.extend(carried(3));
        listed.extend([(Place::Container(4), 0, 3), (Place::Ground, 40, 3), (Place::Ground, 41, 5)]);
        let mut p = Picked::default();
        p.pick_all(Column::You, &listed);
        assert_eq!(p.keys, carried(3), "the pack and what's worn stay out of it");
        p.pick_all(Column::Floor, &listed);
        assert_eq!(p.keys.len(), 5);
        assert!(!p.keys.contains(&(Place::Container(4), 0, 3)));
        // Some picked already: the rest join them. All picked: unpicked, the other column's stay.
        let mut p = Picked::default();
        p.toggle(carried(3)[1]);
        p.pick_all(Column::You, &listed);
        assert_eq!(p.keys.len(), 3);
        p.pick_all(Column::Floor, &listed);
        p.pick_all(Column::You, &listed);
        assert_eq!(p.keys, vec![(Place::Ground, 40, 3), (Place::Ground, 41, 5)]);
        // A column with nothing to pick changes nothing.
        p.pick_all(Column::Container, &[]);
        assert_eq!(p.keys.len(), 2);
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

    /// Every action on an item is one entry of one flat menu, the likeliest first (#95).
    #[test]
    fn the_menu_lists_every_action_likeliest_first() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let config = zombye_net::content::load_config(&data, 1).unwrap();
        let items = &config.items;
        let stack = |id: &str, n: u16| ItemStack::new(items.iter().position(|d| d.id == id).unwrap() as u16, n);
        let labels = |from: Place, s: ItemStack, open: Option<(u32, &str)>| menu_entries(1, items, from, 2, &s, open).into_iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(labels(Place::Carried, stack("tinned_beans", 3), Some((7, "fridge"))), ["Eat", "Put in fridge", "Drop", "Put one in fridge", "Drop one"], "half of three is one: not offered twice");
        assert_eq!(labels(Place::Carried, stack("tinned_beans", 4), Some((7, "fridge"))), ["Eat", "Put in fridge", "Drop", "Put one in fridge", "Put half in fridge (2)", "Drop one", "Drop half (2)"]);
        assert_eq!(labels(Place::Ground, stack("tinned_beans", 2), Some((7, "fridge"))), ["Pick up", "Put in fridge", "Pick up one"]);
        assert!(menu_entries(1, items, Place::Carried, 0, &stack("tinned_beans", 60), Some((7, "fridge"))).len() <= DIGITS.len(), "every entry has a number key");
        assert_eq!(labels(Place::Carried, stack("kitchen_knife", 1), None), ["Hold in hand", "Drop"]);
        // A thing a mod gives a use to (#25): "Use", not "Hold in hand". Without the mod, as before.
        assert_eq!(labels(Place::Carried, stack("alarm_clock", 1), None), ["Hold in hand", "Drop"]);
        let mut modded = config.clone();
        zombye_mods::load_into(&mut modded, &[data.join("../../mods/alarm_clock")]).unwrap();
        let clock = ItemStack::new(modded.items.iter().position(|d| d.id == "alarm_clock").unwrap() as u16, 1);
        assert_eq!(menu_entries(1, &modded.items, Place::Carried, 0, &clock, None).into_iter().map(|e| e.label).collect::<Vec<_>>(), ["Use", "Drop"]);
        assert_eq!(labels(Place::Carried, stack("rag", 1), None)[0], "Use on a wound...");
        assert_eq!(labels(Place::Worn, stack("rag", 1), None), ["Take off", "Drop"]);
        assert_eq!(labels(Place::Ground, stack("kitchen_knife", 1), None), ["Take in hand", "Pick up"]);
        assert_eq!(labels(Place::Container(7), stack("tinned_beans", 4), None), ["Take", "Take one", "Take half (2)"]);
        // The intents are the ones a drag or a click would send.
        let e = menu_entries(1, items, Place::Carried, 2, &stack("tinned_beans", 4), None);
        assert!(matches!(e[0].act, MenuDo::Send { intent: Intent::Use { index: 2, .. }, whole: false, .. }));
        assert!(matches!(e[2].act, MenuDo::Send { intent: Intent::Drop { index: 2, count: 1, .. }, whole: false, .. }));
        assert!(matches!(e[3].act, MenuDo::Send { intent: Intent::Drop { index: 2, count: 2, .. }, .. }));
        let e = menu_entries(1, items, Place::Ground, 9, &stack("kitchen_knife", 1), None);
        assert!(matches!(e[0].act, MenuDo::Send { intent: Intent::MoveItem { from: Place::Ground, index: 9, to: Place::Slot(Slot::Hand), .. }, .. }));
    }

    /// A right-click on a row opens its menu; with a menu up, a left click on a row behind it
    /// moves nothing (it only closes the menu), and a right-click on another row opens that one's.
    #[test]
    fn a_click_behind_an_open_menu_moves_nothing() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let items = zombye_net::content::load_config(&data, 1).unwrap().items;
        let beans = ItemStack::new(items.iter().position(|d| d.id == "tinned_beans").unwrap() as u16, 4);
        let on_row = |index: u32, button| RowClick {
            action: RowAction { stack: beans, title: format!("row {index}"), left: Some((Place::Carried, index, Place::Ground)), more: Vec::new(), menu: menu_entries(1, &items, Place::Carried, index, &beans, None), craft: None },
            button,
            one: false,
        };
        let (mut clicks, mut menu, mut flight, mut out) = (Clicks::default(), ItemMenu::default(), InFlight::default(), Outbox::default());
        clicks.0.push(on_row(0, PointerButton::Secondary));
        menu_clicks(&mut clicks, &mut menu, Some(Vec2::new(10.0, 10.0)));
        drain_clicks(1, 0.0, &mut clicks, &mut flight, &mut out);
        assert_eq!(menu.open.as_ref().map(|m| m.title.as_str()), Some("row 0"));
        assert!(out.0.is_empty(), "opening a menu sends nothing");
        clicks.0.push(on_row(1, PointerButton::Primary));
        menu_clicks(&mut clicks, &mut menu, None);
        drain_clicks(1, 0.1, &mut clicks, &mut flight, &mut out);
        assert!(out.0.is_empty(), "the left click behind the menu moved nothing");
        clicks.0.push(on_row(2, PointerButton::Secondary));
        menu_clicks(&mut clicks, &mut menu, None);
        assert_eq!(menu.open.as_ref().map(|m| m.title.as_str()), Some("row 2"));
        // Shut, a left click moves the row as ever.
        menu.close();
        clicks.0.push(on_row(1, PointerButton::Primary));
        menu_clicks(&mut clicks, &mut menu, None);
        drain_clicks(1, 0.2, &mut clicks, &mut flight, &mut out);
        assert_eq!(out.0.len(), 1);
    }

    /// A click on a recipe asks for it once until the answer comes (#12).
    #[test]
    fn a_recipe_is_asked_for_once() {
        let mut f = InFlight::default();
        let c = RowClick { action: RowAction { stack: ItemStack::one(0), title: String::new(), left: None, more: Vec::new(), menu: Vec::new(), craft: Some(2) }, button: PointerButton::Primary, one: false };
        assert!(matches!(click_intent(&c, 1, &mut f, 0.0), Some(Intent::Craft { player: 1, recipe: 2 })));
        assert!(click_intent(&c, 1, &mut f, 0.1).is_none(), "on its way");
        f.update("queued".into(), false, 0.2);
        assert!(click_intent(&c, 1, &mut f, 0.3).is_some(), "answered: another can be asked for");
        assert!(click_intent(&RowClick { button: PointerButton::Secondary, ..c }, 1, &mut f, 0.4).is_none(), "right-click sends nothing itself");
    }

    /// The real input path, headless: Bevy's mouse input, UI layout and picking on a window with a
    /// panel row in it. One press of the button on the row is one recorded press and one intent,
    /// however many frames go by and even if the panel is rebuilt under the cursor meanwhile; a
    /// second press on the same row while its stack is on its way sends nothing more.
    /// A headless app with Bevy's input, a window, UI layout and picking, for clicking rows.
    pub(super) fn ui_app() -> App {
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
        // The skin's typefaces: without them a row's text has no size, and so no height to press.
        app.add_plugins(theme::ThemePlugin);
        app
    }

    /// Clicks on rows through Bevy's input and picking, with modifiers held: Ctrl picks a row,
    /// Shift picks the range to it, Alt asks for just one of the stack (and picks nothing).
    #[test]
    fn modifier_clicks_pick_rows_and_alt_takes_one() {
        let mut app = ui_app();
        app.init_resource::<Clicks>().init_resource::<Picked>().init_resource::<Listed>();
        app.world_mut().spawn(Camera2d);
        let keys: Vec<Key> = carried(4);
        app.world_mut().resource_mut::<Listed>().0 = keys.clone();
        {
            let world = app.world_mut();
            let mut commands = world.commands();
            commands.spawn(Node { position_type: PositionType::Absolute, top: px(0), left: px(0), width: px(300), flex_direction: FlexDirection::Column, ..default() }).with_children(|col| {
                for k in &keys {
                    row(col, Cells::plain(&format!("Beans {}", k.1), "0.4"), RowAction { stack: beans(2), left: Some((k.0, k.1, Place::Ground)), more: Vec::new(), title: String::new(), menu: Vec::new(), craft: None }, false);
                }
            });
            world.flush();
        }
        for _ in 0..3 {
            app.update();
        }
        let window = app.world_mut().query_filtered::<Entity, With<PrimaryWindow>>().single(app.world()).unwrap();
        // Rows are about 23 px tall (14 px text plus padding); find each one's centre from layout.
        let centres: Vec<Vec2> = {
            let mut q = app.world_mut().query::<(&ComputedNode, &UiGlobalTransform, &BackgroundColor)>();
            let mut v: Vec<Vec2> = q.iter(app.world()).filter(|(_, _, bg)| bg.0 == ROW).map(|(_, t, _)| t.translation).collect();
            v.sort_by(|a, b| a.y.total_cmp(&b.y));
            v
        };
        assert_eq!(centres.len(), 4);
        let click = |app: &mut App, at: Vec2, modifier: Option<KeyCode>| {
            if let Some(m) = modifier {
                app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(m);
            }
            let moved = CursorMoved { window, position: at, delta: None };
            app.world_mut().write_message(moved.clone());
            app.world_mut().write_message(WindowEvent::CursorMoved(moved));
            app.update();
            for state in [ButtonState::Pressed, ButtonState::Released] {
                let e = MouseButtonInput { button: MouseButton::Left, state, window };
                app.world_mut().write_message(e);
                app.world_mut().write_message(WindowEvent::MouseButtonInput(e));
                app.update();
            }
            if let Some(m) = modifier {
                app.world_mut().resource_mut::<ButtonInput<KeyCode>>().release(m);
            }
            app.update();
        };
        click(&mut app, centres[3], Some(KeyCode::ControlLeft));
        assert_eq!(app.world().resource::<Picked>().keys, vec![keys[3]]);
        click(&mut app, centres[1], Some(KeyCode::ShiftLeft));
        let mut got = app.world().resource::<Picked>().keys.clone();
        got.sort_by_key(|k| k.1);
        assert_eq!(got, keys[1..=3].to_vec(), "Shift-click picks 1 to 3");
        assert!(app.world().resource::<Clicks>().0.is_empty(), "picking sends nothing");
        click(&mut app, centres[0], Some(KeyCode::AltLeft));
        let clicks = &app.world().resource::<Clicks>().0;
        assert_eq!(clicks.len(), 1);
        assert!(clicks[0].one, "Alt-click: just one");
        assert_eq!(app.world().resource::<Picked>().keys.len(), 3, "Alt-click picks nothing");
    }

    #[test]
    fn one_click_on_a_row_is_one_intent() {
        let mut app = ui_app();
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
                row(col, Cells::plain("Alarm clock", "0.4"), RowAction { stack: beans(1), title: String::new(), left: Some((Place::Container(4), 0, Place::Carried)), more: Vec::new(), menu: Vec::new(), craft: None }, false);
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
