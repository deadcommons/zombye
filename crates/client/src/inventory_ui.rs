//! The inventory screen (Tab or I), the first screen of the flat frame (docs/ui-design.md): a top
//! bar, a rail of screens down the left, three columns, and a footer of key hints, on a veil over
//! the world.
//!
//! **Gear** (left): what each hand holds, what is worn and carried place by place ([`body::Spot`]),
//! the hotbar, and at the foot the load against the carry limit. **You** (middle): a pane for the
//! pockets and for each bag carried, each with its own room (`Inventory::pouches`): a thing is in
//! one of them (`ItemStack::home`). **Nearby** (right): the open container (search one with E) and
//! the floor within reach, each a pane; a press on a pane's name makes it the one your things go
//! to. "Take all" and "Put mine in" at its foot move everything across.
//!
//! **Nothing scrolls sideways.** Things are tiles that wrap ([`Ctx::tiles`]) or, with the "Rows"
//! control, compact one-line rows ([`Ctx::rows`]); each column scrolls up and down by itself
//! ([`scroll_frame`]); several stacks of one item collapse into one tile or row with their count
//! together ([`arrange`]; the fold mark opens it to its stacks); a pane folds to its head; the
//! search box and the category chips narrow every pane at once.
//!
//! Click a thing to move it across: from you into the pane of Nearby that is the target, from a
//! container or the floor into your pockets and bags, off a slot into your bags. Alt-click
//! (Option on a Mac) moves just one of a stack; Ctrl-click picks, Shift-click picks a range, and
//! with something picked a bar at the foot of You sends them all to a bag, the container or the
//! floor. Right-click for the menu of what can be done with it (#95; [`menu_entries`]): the
//! likeliest first, 1-9 pick by number, Esc or a click elsewhere closes it; the same menu opens
//! on a ground item in the world. Pointing at a thing shows a card with what there is to say about
//! it ([`HoverCard`]). Everything goes to the server as intents; the screen only shows what the
//! latest snapshot said. The last answer to a request (done, or why not) shows in the footer, and
//! for a few seconds under the HUD when the screen is shut.
//!
//! **One click, one request.** A press is only recorded by its observer ([`Clicks`]);
//! [`send_clicks`] turns at most one press per button per frame into an intent, and not one for
//! a stack that is already on its way ([`InFlight`]: what was asked, until the snapshot shows the
//! answer, a refusal comes, or 3 s pass).
//!
//! **The floor** is the server's: the ground items the snapshot says this player can reach
//! (`Snapshot::floor`, by the same rule the server checks moves with), not a guess from distance.
//!
//! **Make** (#12) is the rail's second screen: every recipe; the ones you have the inputs and
//! tools for (by the server's own rule, `crafting::check`, over our private view) can be clicked
//! to make them, the rest say what's missing.
//!
//! For pictures: `--find TEXT`, `--filter CATEGORY[,CATEGORY]`, `--pick N,N`, `--tab floor|rows`,
//! `--sort name|type|weight`, `--expand`; for recordings `--pick-range A,B[@SECS]` and
//! `--drop-picked-after SECS`.

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

/// The frame's screens, down its rail. Health and Skills draw their own middle (`health_ui`,
/// `skills_ui`) over the frame while theirs is the one shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Inventory,
    Make,
    Build,
    Health,
    Skills,
}

/// A press on the rail: the screen to show.
#[derive(Resource, Default)]
struct Goto(Option<Tab>);

/// Keeps the frame and its screens in step. Health, Skills and Build are shown while their own
/// resources say open (H, K and Shift+B, `--health`, "use on a wound"): opening one opens the frame
/// on it and shuts the others; its key again goes back to the game; shutting the frame shuts all.
fn screens(mut panel: ResMut<InventoryPanel>, mut health: ResMut<crate::health_ui::HealthPanel>, mut skills: ResMut<crate::skills_ui::SkillsPanel>, mut build: ResMut<crate::build_ui::BuildMenu>, mut make: ResMut<MakeUi>, mut goto: ResMut<Goto>, mut was: Local<[bool; 4]>) {
    let [p, h, s, b] = *was;
    let mut want = [panel.open, health.open, skills.open, build.open];
    if let Some(tab) = goto.0.take() {
        want = [true, tab == Tab::Health, tab == Tab::Skills, tab == Tab::Build];
        if matches!(tab, Tab::Inventory | Tab::Make) && make.shown != (tab == Tab::Make) {
            make.shown = tab == Tab::Make;
        }
    } else if health.open && !h {
        want = [true, true, false, false];
    } else if skills.open && !s {
        want = [true, false, true, false];
    } else if build.open && !b {
        want = [true, false, false, true];
    } else if p && panel.open && ((h && !health.open) || (s && !skills.open) || (b && !build.open)) {
        want[0] = false;
    }
    if !want[0] {
        want = [false; 4];
    }
    if panel.open != want[0] {
        panel.open = want[0];
    }
    if health.open != want[1] {
        health.open = want[1];
        if !want[1] {
            health.use_item = None;
        }
    }
    if skills.open != want[2] {
        skills.open = want[2];
    }
    if build.open != want[3] {
        build.open = want[3];
    }
    *was = want;
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

use crate::theme::{self, flat, font};
use body::Spot;
use zombye_sim::Pouch;

// The flat skin's colours (docs/ui-design.md), under the names the rows have always used.
const TEXT: Color = flat::BONE;
const DIM: Color = flat::DIM;
const INK: Color = flat::INK;
const ROW: Color = flat::CELL;
const ROW_HOVER: Color = flat::CELL_HOVER;
const ROW_PICKED: Color = flat::GREEN_SOFT;
/// A fold mark's small backing.
const CHIP: Color = Color::srgba(0.949, 0.902, 0.788, 0.1);
const LINE: Color = flat::LINE;
const OK: Color = flat::GREEN;
const NO: Color = flat::RED_TEXT;
const NOTE_SECS: f32 = 4.0;
const ROW_FONT: f32 = flat::BODY;
/// A compact row's cells: the fold mark, the icon, type, count and weight (the name takes the rest).
const W_LEAD: f32 = 12.0;
const W_ICON: f32 = 22.0;
const W_KIND: f32 = 78.0;
const W_QTY: f32 = 28.0;
const W_KG: f32 = 40.0;
const CELL_GAP: f32 = 6.0;
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
}

impl Find {
    fn shows(&self, items: &[ItemDef], s: &ItemStack) -> bool {
        let Some(def) = items.get(s.item as usize) else { return true };
        (self.groups.is_empty() || self.groups.contains(&Group::of(def.category))) && (self.query.is_empty() || def.name.to_lowercase().contains(&self.query.to_lowercase()))
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
            Place::Slot(_) | Place::Worn | Place::Carried | Place::Pouch(_) => Column::You,
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
    /// What the sort control says.
    fn name(&self) -> &'static str {
        match self.key {
            SortKey::None => "as packed",
            SortKey::Name => "name",
            SortKey::Kind => "type",
            SortKey::Weight => "weight",
        }
    }

    /// The sort control pressed: by name, by type, by weight, then as things lie again.
    fn next(&mut self) {
        let key = match self.key {
            SortKey::None => SortKey::Name,
            SortKey::Name => SortKey::Kind,
            SortKey::Kind => SortKey::Weight,
            SortKey::Weight => SortKey::None,
        };
        *self = Sort { key, flipped: false };
    }

    /// A column header pressed: sort by it; again, the other way round; a third time, back to
    /// the order things lie in.
    #[cfg(test)]
    fn press(&mut self, key: SortKey) {
        *self = match (self.key == key, self.flipped) {
            (false, _) => Sort { key, flipped: false },
            (true, false) => Sort { key, flipped: true },
            (true, true) => Sort::default(),
        };
    }

    /// The mark after a header: which way its column is sorted, if it is.
    #[cfg(test)]
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

/// How the panes are set: tiles or rows, which are folded, where your things go, the order,
/// and which collapsed rows are opened.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
struct PanelUi {
    /// Things show as compact rows (else as tiles).
    rows: bool,
    /// The panes folded to their heads.
    shut: Vec<Pane>,
    /// Your things go to the floor though a container is open.
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
        let find = Find { query: options.as_ref().and_then(|o| o.find.clone()).unwrap_or_default(), typing: false, groups: options.as_ref().and_then(|o| o.filter.as_deref()).map_or(Vec::new(), Group::parse_list) };
        let tabs = options.as_ref().and_then(|o| o.tab.clone()).unwrap_or_default().to_ascii_lowercase();
        let ui = PanelUi {
            rows: tabs.split(',').any(|t| t.trim() == "rows"),
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
            .init_resource::<Drag>()
            .init_resource::<Goto>()
            .init_resource::<Outbox>()
            .init_resource::<InFlight>()
            .init_resource::<MakeUi>()
            .init_resource::<ItemMenu>()
            .init_resource::<HoveredItem>()
            .init_resource::<HoverCard>()
            .init_resource::<FollowUp>()
            .init_resource::<Scrolls>()
            .add_systems(Startup, spawn.after(crate::hud::spawn_hud))
            .add_systems(PreUpdate, (type_search, menu_keys).chain().in_set(TextEntry).after(bevy::input::InputSystems))
            .add_systems(PreUpdate, click_away.after(bevy::input::InputSystems).after(bevy::picking::PickingSystems::Hover))
            .add_systems(Update, ((toggle, make_key, screens.after(crate::health_ui::toggle).after(crate::skills_ui::toggle)).chain(), keep_note, track_flight, follow_open, prune_picked, script_picks, menu_from_options, drag, send_clicks, world_menu, script_menu, menu_act, follow_up, send_picked, flush, scroll_frame, rebuild, draw_menu, draw_card, hide_hud).chain())
            .add_systems(Update, count_down.after(rebuild));
    }
}

/// The frame covers the screen and has the hotbar and load in it: the HUD's bottom-left column
/// and the hotbar are not drawn under it (they would show faintly through the veil).
fn hide_hud(panel: Res<InventoryPanel>, mut hud: Query<&mut Visibility, Or<(With<crate::hud::BottomLeft>, With<crate::hotbar::BarRoot>)>>) {
    let want = if panel.open { Visibility::Hidden } else { Visibility::Inherited };
    for mut v in &mut hud {
        if *v != want {
            *v = want;
        }
    }
}

fn spawn(mut commands: Commands, column: Query<Entity, With<crate::hud::BottomLeft>>) {
    // The frame: the whole window, veiled (docs/ui-design.md). Over the HUD; the sheets that
    // still float (health, skills) and the menus are over it.
    commands.spawn((PanelRoot, Visibility::Hidden, GlobalZIndex(6), Node { position_type: PositionType::Absolute, left: px(0), top: px(0), width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, ..default() }, BackgroundColor(flat::VEIL)));
    // The latest note while the panel is shut: just above the HUD's bottom-left column (the tags
    // and the vitals), however tall that is, on a backing so it reads over the world (#97).
    let note = (NoteLine, Text::new(""), theme::strong(15.0), TextColor(theme::GOLD), TextLayout::no_wrap(), BackgroundColor(Color::NONE), Pickable::IGNORE);
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
            }
            _ => {}
        }
    }
    keys.reset_all();
}

fn toggle(keys: Res<ButtonInput<KeyCode>>, mut panel: ResMut<InventoryPanel>, link: Res<ServerLink>, me: Res<LocalPlayer>, map: Res<crate::server_link::ClientMap>, mut out: ResMut<Outbox>) {
    if crate::keys::just_pressed(&keys, crate::keys::Action::Inventory) {
        panel.open = !panel.open;
        if panel.open
            && let Some(intent) = open_body(&link, me.id, &map.0)
        {
            out.0.push(intent);
        }
    }
}

/// Shift+C: the Make screen, or back to the game when it is the one shown.
fn make_key(keys: Res<ButtonInput<KeyCode>>, mut panel: ResMut<InventoryPanel>, make: Res<MakeUi>, mut goto: ResMut<Goto>, health: Option<Res<crate::health_ui::HealthPanel>>, skills: Option<Res<crate::skills_ui::SkillsPanel>>, build: Option<Res<crate::build_ui::BuildMenu>>) {
    if !crate::keys::just_pressed(&keys, crate::keys::Action::Make) {
        return;
    }
    let other = health.is_some_and(|h| h.open) || skills.is_some_and(|s| s.open) || build.is_some_and(|b| b.open);
    if panel.open && make.shown && !other {
        panel.open = false;
    } else {
        goto.0 = Some(Tab::Make);
    }
}

/// Opening the inventory next to a dead zombie searches its body, so what it wears and carries is
/// in the right-hand pane without pressing E: unless a container is open already or a search is
/// under way. By the server's own reach rule ([`zombye_sim::corpses::body_in_reach`]).
fn open_body(link: &ServerLink, me: u32, map: &zombye_sim::TileMap) -> Option<Intent> {
    let view = link.own(me)?;
    if view.open.is_some() || view.jobs.iter().any(|j| matches!(j, JobKind::Search { .. })) {
        return None;
    }
    let p = link.curr.players.iter().find(|p| p.id == me && !p.dead)?;
    let bodies = link.curr.zombies.iter().filter(|z| z.anim == zombye_sim::ZombieAnim::Dead).map(|z| (z.id, Vec2::from(z.pos)));
    let c = zombye_sim::corpses::body_in_reach(map, Vec2::from(p.pos), bodies)?;
    Some(Intent::Search { player: me, container: Some(c) })
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
    c.0 = if last.ok { theme::GOLD } else { theme::BAD };
}

/// "Loaf of bread x2 (stale, 2/3 left)": the name, how many, and for food and drink how fresh it
/// is at game minute `now` and the goes left of the top one (#11).
fn name_of(items: &[ItemDef], wear: Wear, now: i32, s: &ItemStack) -> String {
    let Some(def) = items.get(s.item as usize) else { return "?".into() };
    let mut out = if s.count > 1 { format!("{} x{}", def.name, s.count) } else { def.name.clone() };
    let notes = state_of(items, wear, now, s);
    if !notes.is_empty() {
        out.push_str(&format!(" ({notes})"));
    }
    out
}

/// "stale, 2/3 left": what a row says after the name about food and drink (how fresh at game
/// minute `now`, the goes left of the top one), "worn" or "battered" about a weapon that has
/// seen use (`zombye_sim::wear`); empty for the rest.
fn state_of(items: &[ItemDef], wear: Wear, now: i32, s: &ItemStack) -> String {
    let Some(def) = items.get(s.item as usize) else { return String::new() };
    let mut notes = Vec::new();
    if let Some(f) = zombye_sim::needs::freshness(def, s, now).filter(|f| *f != zombye_sim::Freshness::Fresh) {
        notes.push(f.name().to_string());
    }
    if let Some(c) = def.consumable().filter(|_| s.used > 0) {
        notes.push(format!("{}/{} left", c.uses - s.used, c.uses));
    } else if let Some((_, _, state)) = zombye_sim::wear::condition(items, wear.weapons, s, wear.battered_below) {
        if state != zombye_sim::wear::Condition::Good {
            notes.push(state.label().into());
        }
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
    /// Left click: make this recipe so many times over (a row of the Make list; `stack` and `left`
    /// are unused).
    craft: Option<(u16, u16)>,
}

impl RowAction {
    /// The stacks the row stands for (one, or several of one item), for picking.
    fn keys(&self) -> Vec<Key> {
        self.left.iter().map(|&(from, index, _)| (from, index, self.stack.item)).chain(self.more.iter().map(Line::key)).collect()
    }
}

/// The Make list's rule for a recipe, for the make wheel (`wheel.rs`): can it be made here and
/// now, from what we carry, our floor and the open container, with the heat or water it needs.
pub fn make_check<'a>(link: &'a ServerLink, map: &crate::server_link::ClientMap, join: &'a JoinInfo, me: &LocalPlayer) -> impl Fn(&zombye_sim::RecipeDef) -> bool + 'a {
    let (view, floor) = panel_view(link, me.id);
    let near = view.as_ref().map(|v| stock(v, &floor));
    let heat = zombye_sim::fire::heat_near(&map.0, &join.fire, link.curr.fires.iter().map(|f| (Vec2::from(f.pos), f.lit)), me.curr.pos, me.curr.level, |_| link.curr.grid_on);
    let water = zombye_sim::power::water_near(&map.0, &join.water, link.curr.water_on, me.curr.pos, me.curr.level);
    let cistern = crate::world_actions::cistern_here(&map.0, &join.water, &link.curr, me.curr.pos, me.curr.level).is_some();
    move |r: &zombye_sim::RecipeDef| near.as_ref().is_some_and(|n| crafting::check(r, n, &join.items).is_ok() && (!r.heat || heat) && (!r.water || water) && (!r.cistern || cistern))
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
        // (Only somewhere things go to.)
        Place::Pouch(_) => None,
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
fn follow_open(link: Res<ServerLink>, me: Res<LocalPlayer>, mut ui: ResMut<PanelUi>, mut panel: ResMut<InventoryPanel>, mut make: ResMut<MakeUi>, mut searching: Local<Option<u32>>) {
    let view = link.own(me.id);
    let open = view.and_then(|v| v.open.as_ref().map(|c| c.id));
    if ui.seen_open != Some(open) {
        ui.saw(open);
    }
    let now = view.and_then(|v| match v.jobs.first() {
        Some(JobKind::Search { container }) => Some(*container),
        _ => None,
    });
    if search_done(*searching, now, open) {
        if !panel.open {
            panel.open = true;
        }
        if make.shown {
            make.shown = false;
        }
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
    // Make all: as many crafts as there is the stuff for, one request each (the server queues
    // another only while there's enough for it too).
    if let Some((recipe, times)) = a.craft.filter(|&(_, n)| n > 1 && click.button == PointerButton::Primary) {
        return if flight.ask_craft(recipe, now) { (0..times).map(|_| Intent::Craft { player, recipe }).collect() } else { Vec::new() };
    }
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
            if let Some((recipe, _)) = a.craft {
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
    (time, map): (Res<Time>, Res<crate::server_link::ClientMap>),
    panel: Res<InventoryPanel>,
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    me: Res<LocalPlayer>,
    last: Res<LastNote>,
    flight: Res<InFlight>,
    (find, picked, ui): (Res<Find>, Res<Picked>, Res<PanelUi>),
    (scrolls, bar, mut card): (Res<Scrolls>, Option<Res<crate::hotbar::Hotbar>>, ResMut<HoverCard>),
    (health, skills, build): (Option<Res<crate::health_ui::HealthPanel>>, Option<Res<crate::skills_ui::SkillsPanel>>, Option<Res<crate::build_ui::BuildMenu>>),
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
        if card.0.is_some() {
            card.0 = None;
        }
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
    // (The seconds a job has left are not in it: `count_down` writes them into the Make row in
    // place, so the list isn't built again ten times a second, which made the row under the
    // pointer flicker between its hover and plain backings.)
    // Heat to cook with where we stand (`fire::heat_near`, the server's rule; stoves go by the grid).
    let heat = zombye_sim::fire::heat_near(&map.0, &join.fire, link.curr.fires.iter().map(|f| (Vec2::from(f.pos), f.lit)), me.curr.pos, me.curr.level, |_| link.curr.grid_on);
    // Running water, else a toilet's cistern with water left (`power::cistern_near`).
    let water = (
        zombye_sim::power::water_near(&map.0, &join.water, link.curr.water_on, me.curr.pos, me.curr.level),
        crate::world_actions::cistern_here(&map.0, &join.water, &link.curr, me.curr.pos, me.curr.level).is_some(),
    );
    let clock = clock_text(link.curr.time_of_day);
    let hotbar = bar.as_ref().map(|b| b.ids());
    let tab = match (health.is_some_and(|h| h.open), skills.is_some_and(|s| s.open), build.is_some_and(|b| b.open), make_ui.shown) {
        (true, ..) => Tab::Health,
        (_, true, ..) => Tab::Skills,
        (_, _, true, _) => Tab::Build,
        (.., true) => Tab::Make,
        _ => Tab::Inventory,
    };
    let signature = format!("{heat}|{water:?}|{tab:?}|{view:?}|{floor:?}|{}|{}|{}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{}|{:?}|{clock}|{hotbar:?}|{:?}", busy.1, if note_fresh { last.text.as_str() } else { "" }, me.dead, flight.asked, flight.crafts, now / 60, *find, picked.keys, make_ui.open, make_ui.favs, make_ui.shown, *ui, (crate::help_text::level(), crate::pad::showing()));
    if shown.0 == signature {
        return;
    }
    shown.0 = signature;
    commands.entity(root).despawn_children();
    // (What the pointer was on is gone with the rest.)
    card.0 = None;
    let items = &join.items;
    let open = view.as_ref().and_then(|v| v.open.as_ref().map(|c| (c.id, c.name.to_lowercase())));
    // Where a click on what you carry sends it: the pane of the Nearby column that is the target.
    let right = ui.right(open.as_ref().map(|(id, _)| *id));
    // The pockets and each bag carried: (pouch, its name, litres of room, litres used).
    let pouches: Vec<(Pouch, String, f32, f32)> = view.as_ref().map_or(Vec::new(), |v| v.inventory.pouches(items, join.carry.pockets_l).into_iter().map(|(p, room, used)| (p, pouch_name(items, &v.inventory, p), room, used)).collect());
    let pouch_names: Vec<(Pouch, String)> = pouches.iter().map(|(p, n, ..)| (*p, n.to_lowercase())).collect();
    let have = view.as_ref().map(|v| stock(v, &floor));
    let ctx = Ctx { stock: have.as_ref(), items, wear: Wear::of(&join), now, me: me.id, flight: &flight, find: &find, picked: &picked, ui: &ui, view: view.as_ref(), open: open.as_ref().map(|(id, n)| (*id, n.as_str())), pouches: &pouch_names, recipes: &join.recipes };
    // The item rows drawn, in order, for Shift-click's ranges and the move-all buttons.
    let mut rows: Vec<Key> = Vec::new();
    let kg: f32 = view.as_ref().map_or(0.0, |v| picked.keys.iter().filter_map(|&(p, i, _)| stack_at(v, &floor, p, i)).map(|s| stack_kg(items, &s)).sum());
    let y = scrolls.0;
    let note = (note_fresh && !last.text.is_empty()).then(|| (last.text.clone(), last.ok));
    commands.entity(root).with_children(|c| {
        top(c, &format!("{tab:?}"), &clock);
        c.spawn(Node { flex_direction: FlexDirection::Row, flex_grow: 1.0, flex_basis: px(0), min_height: px(0), ..default() }).with_children(|mid| {
            rail(mid, tab);
            let Some(v) = &view else { return };
            // (Health, Skills and Build draw their own middle over the frame.)
            if matches!(tab, Tab::Health | Tab::Skills | Tab::Build) {
                return;
            }
            if tab == Tab::Make {
                // What lies in reach and the open container count too ([`stock`], the server's
                // rule): a log too big for a bag is sawn where it lies.
                let near = PrivateView { inventory: stock(v, &floor), ..v.clone() };
                make_column(mid, &join, &near, busy.0, &flight, &make_ui, y[3], heat, water);
                return;
            }
            gear(mid, &ctx, &mut rows, v, bar.as_deref(), y[0]);
            you(mid, &ctx, &mut rows, v, &pouches, right, y[1], (picked.keys.len(), kg));
            nearby(mid, &ctx, &mut rows, v, &floor, right, y[2]);
        });
        footer(c, note, tab);
    });
    listed.0 = rows;
}

/// "14:20" from the hour of the day.
fn clock_text(hour: f32) -> String {
    let mins = (hour.rem_euclid(24.0) * 60.0) as u32 / 10 * 10;
    format!("{:02}:{:02}", mins / 60 % 24, mins % 60)
}

/// What a pouch is called on its pane: "Pockets", or the bag's name.
fn pouch_name(items: &[ItemDef], inv: &zombye_sim::Inventory, p: Pouch) -> String {
    p.slot().and_then(|s| inv.slot(s)).and_then(|bag| items.get(bag.item as usize)).map_or("Pockets".into(), |d| d.name.clone())
}

/// Where a pouch is carried, after its name.
fn pouch_where(p: Pouch) -> &'static str {
    match p {
        Pouch::Pockets => "",
        Pouch::Back => "on your back",
        Pouch::Shoulder => "on your shoulder",
        Pouch::Hand => "in your hand",
        Pouch::OffHand => "in your other hand",
    }
}

// ---- The frame (docs/ui-design.md): top bar, rail, columns, footer ----

/// A column's part that scrolls up and down (never sideways): the wheel moves it
/// ([`scroll_frame`]), and it comes back where it was after a rebuild ([`Scrolls`]).
#[derive(Component, Clone, Copy)]
pub(crate) struct Scroller(usize);

/// How far down each scroller is (gear, you, nearby, make, wounds, skills, condition, build), in UI pixels.
#[derive(Resource, Default)]
pub(crate) struct Scrolls(pub(crate) [f32; 8]);

pub(crate) fn scroller<'a>(c: &'a mut ChildSpawnerCommands, k: usize, y: f32, padding: UiRect, gap: f32) -> EntityCommands<'a> {
    c.spawn((Scroller(k), ScrollPosition(Vec2::new(0.0, y)), Node { flex_direction: FlexDirection::Column, flex_grow: 1.0, flex_basis: px(0), min_height: px(0), row_gap: px(gap), padding, overflow: Overflow::scroll_y(), ..default() }))
}

/// The mouse wheel over a column scrolls it.
fn scroll_frame(mut wheel: MessageReader<bevy::input::mouse::MouseWheel>, panel: Res<InventoryPanel>, window: Query<&Window, With<bevy::window::PrimaryWindow>>, mut scrollers: Query<(&Scroller, &ComputedNode, &UiGlobalTransform, &mut ScrollPosition)>, mut scrolls: ResMut<Scrolls>) {
    use bevy::input::mouse::MouseScrollUnit;
    let px = wheel.read().map(|ev| if ev.unit == MouseScrollUnit::Line { ev.y * 40.0 } else { ev.y }).fold(0.0, |a, b| a + b);
    if px == 0.0 || !panel.open {
        return;
    }
    let Some(cursor) = window.single().ok().and_then(|w| w.physical_cursor_position()) else { return };
    for (which, node, at, mut pos) in &mut scrollers {
        if !node.contains_point(*at, cursor) {
            continue;
        }
        // Physical pixels to UI pixels, as `ScrollPosition` is.
        let max = (node.content_size.y - node.size.y) * node.inverse_scale_factor();
        let y = crate::hud::scrolled(pos.y, px, max);
        if pos.y != y {
            pos.y = y;
        }
        if let Some(kept) = scrolls.0.get_mut(which.0) {
            *kept = y;
        }
    }
}

/// The top bar: our mark, the screen's name, the hour, and the way out.
fn top(c: &mut ChildSpawnerCommands, title: &str, clock: &str) {
    c.spawn((Node { height: px(flat::TOP_BAR), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, justify_content: JustifyContent::SpaceBetween, padding: UiRect::axes(px(28), px(0)), border: UiRect::bottom(px(1)), flex_shrink: 0.0, ..default() }, BorderColor::all(LINE))).with_children(|b| {
        b.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(18), ..default() }, Pickable::IGNORE)).with_children(|l| {
            // The mark: the road Z, or without the brand's pictures a green head with two eyes.
            if let Some((picture, node)) = crate::release_brand::image(crate::release_brand::Mark::Z, 34.0) {
                l.spawn((picture, node, Pickable::IGNORE));
            } else {
            l.spawn((Node { width: px(38), height: px(38), border_radius: BorderRadius::MAX, flex_shrink: 0.0, ..default() }, BackgroundColor(flat::GREEN), Pickable::IGNORE)).with_children(|m| {
                for left in [9.0, 22.0] {
                    m.spawn((Node { position_type: PositionType::Absolute, left: px(left), top: px(13), width: px(7), height: px(11), border_radius: BorderRadius::MAX, ..default() }, BackgroundColor(INK), Pickable::IGNORE));
                }
            });
            }
            l.spawn((Text::new(title.to_uppercase()), theme::head(flat::DISPLAY), TextColor(TEXT), TextLayout::no_wrap(), Pickable::IGNORE));
            l.spawn((Text::new(clock.to_string()), font(flat::BODY), TextColor(DIM), TextLayout::no_wrap(), Node { margin: UiRect::top(px(6)), ..default() }, Pickable::IGNORE));
        });
        b.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect::axes(px(8), px(6)), ..default() })
            .with_children(|r| {
                r.spawn((Text::new("Close"), font(flat::BODY), TextColor(DIM), Pickable::IGNORE));
                if let Some(k) = crate::pad::prompt_action(crate::keys::Action::Inventory) {
                    flat::key(r, &k);
                }
            })
            .observe(|_: On<PointerPress>, mut panel: ResMut<InventoryPanel>| panel.open = false);
    });
}

/// The rail down the left: the frame's screens.
fn rail(mid: &mut ChildSpawnerCommands, tab: Tab) {
    mid.spawn((Node { width: px(flat::RAIL), flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect::top(px(12)), border: UiRect::right(px(1)), flex_shrink: 0.0, ..default() }, BorderColor::all(LINE))).with_children(|r| {
        // Each screen's key, as bound (`keys.rs`).
        use crate::keys::{Action, short};
        for (to, icon, key) in [(Tab::Inventory, "hiking_pack", Some(Action::Inventory)), (Tab::Make, "claw_hammer", Some(Action::Make)), (Tab::Build, "wooden_plank", Some(Action::Build)), (Tab::Health, "plasters", Some(Action::Health)), (Tab::Skills, "magazine", Some(Action::Skills))] {
            let key = key.map(short);
            rail_tab(r, &format!("{to:?}"), icon, key.as_deref(), tab == to).observe(move |_: On<PointerPress>, mut goto: ResMut<Goto>| goto.0 = Some(to));
        }
    });
}

fn rail_tab<'a>(r: &'a mut ChildSpawnerCommands, label: &str, icon: &str, key: Option<&str>, on: bool) -> EntityCommands<'a> {
    let rest = if on { flat::GREEN_SOFT } else { Color::NONE };
    let mut e = r.spawn((Node { height: px(84), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, row_gap: px(6), flex_shrink: 0.0, ..default() }, BackgroundColor(rest)));
    e.with_children(|t| {
        if on {
            t.spawn((Node { position_type: PositionType::Absolute, left: px(0), top: px(0), bottom: px(0), width: px(3), ..default() }, BackgroundColor(flat::GREEN), Pickable::IGNORE));
        }
        if let Some(image) = flat::icon(icon, Color::srgba(1.0, 1.0, 1.0, if on { 1.0 } else { 0.55 })) {
            t.spawn((Node { width: px(32), height: px(32), ..default() }, image, Pickable::IGNORE));
        }
        t.spawn((Text::new(label.to_uppercase()), theme::head(flat::SMALL), TextColor(if on { TEXT } else { DIM }), TextLayout::no_wrap(), Pickable::IGNORE));
        if let Some(k) = key {
            t.spawn((Node { position_type: PositionType::Absolute, top: px(3), right: px(5), ..default() }, Pickable::IGNORE)).with_children(|c| {
                c.spawn((Text::new(k.to_string()), font(flat::SMALL), TextColor(flat::FAINT), TextLayout::no_wrap(), Pickable::IGNORE));
            });
        }
    });
    if !on {
        flat::hover(&mut e, rest);
    }
    e
}

/// The footer: the latest answer to a request, then what the mouse does.
fn footer(c: &mut ChildSpawnerCommands, note: Option<(String, bool)>, tab: Tab) {
    c.spawn((Node { height: px(flat::FOOTER), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(22), padding: UiRect::axes(px(28), px(0)), border: UiRect::top(px(1)), flex_shrink: 0.0, overflow: Overflow::clip_x(), ..default() }, BorderColor::all(LINE))).with_children(|f| {
        if let Some((text, ok)) = note {
            f.spawn((Text::new(text), theme::strong(flat::BODY), TextColor(if ok { OK } else { NO }), TextLayout::no_wrap(), Node { flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
        }
        // The controller doesn't move round this screen yet: only the way back.
        if crate::pad::showing() {
            let back = match tab {
                Tab::Health => crate::keys::Action::Health,
                Tab::Skills => crate::keys::Action::Skills,
                Tab::Build => crate::keys::Action::Build,
                _ => crate::keys::Action::Inventory,
            };
            let b = crate::pad::prompt_action(back).or_else(|| crate::pad::prompt_action(crate::keys::Action::Inventory)).unwrap_or_else(|| crate::pad::button(crate::pad::Button::B));
            flat::hint(f, &[&b], "back to the game");
            return;
        }
        match tab {
            Tab::Health => {
                flat::hint(f, &["Click"], "a wound: what it needs, and what you can do");
                flat::hint(f, &["Right-click"], "stop using an item");
                flat::hint(f, &[&crate::keys::short(crate::keys::Action::Health)], "back to the game");
                return;
            }
            Tab::Skills => {
                flat::hint(f, &[&crate::keys::short(crate::keys::Action::Skills)], "back to the game");
                return;
            }
            Tab::Build => {
                flat::hint(f, &["Click"], "build it on the window or door you face");
                flat::hint(f, &["Right-click"], "a window or door in the world: board it up");
                flat::hint(f, &[&crate::keys::short(crate::keys::Action::Build)], "back to the game");
                return;
            }
            _ => {}
        }
        flat::hint(f, &["Click"], "move");
        flat::hint(f, &["Drag"], "to a bag, a slot or the floor");
        flat::hint(f, &["Alt", "Click"], "move one");
        flat::hint(f, &["Ctrl", "Click"], "pick several");
        flat::hint(f, &["Shift", "Click"], "pick a range");
        flat::hint(f, &["Right-click"], "actions");
        flat::hint(f, &["Shift", "1-9"], "onto the hotbar");
    });
}

/// A column's title.
fn column_title(c: &mut ChildSpawnerCommands, title: &str, right: Option<String>) {
    c.spawn((Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::FlexEnd, flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|t| {
        t.spawn((Text::new(title.to_uppercase()), theme::head(flat::TITLE), TextColor(TEXT), TextLayout::no_wrap(), Pickable::IGNORE));
        if let Some(right) = right {
            t.spawn((Text::new(right), font(flat::SMALL), TextColor(DIM), TextLayout::no_wrap(), Pickable::IGNORE));
        }
    });
}

/// The places of the Gear column's grid, in reading order (the two hands have cards of their own).
const GEAR: [Spot; 10] = [Spot::Head, Spot::Face, Spot::Neck, Spot::Torso, Spot::Arms, Spot::Hands, Spot::Legs, Spot::Feet, Spot::Back, Spot::Shoulder];
/// A slot of the Gear grid (three across in the narrowest column).
const SLOT: f32 = 92.0;

/// The Gear column: what the hands hold, what is worn and carried place by place, the hotbar,
/// and at its foot the load.
fn gear(mid: &mut ChildSpawnerCommands, ctx: &Ctx, listed: &mut Vec<Key>, v: &PrivateView, bar: Option<&crate::hotbar::Hotbar>, y: f32) {
    let inv = &v.inventory;
    mid.spawn((Node { width: percent(27), min_width: px(336), max_width: px(500), flex_direction: FlexDirection::Column, flex_shrink: 0.0, border: UiRect::right(px(1)), ..default() }, BorderColor::all(LINE))).with_children(|col| {
        scroller(col, 0, y, UiRect::new(px(20), px(20), px(18), px(16)), 8.0).with_children(|s| {
            column_title(s, "Gear", None);
            for (spot, label, empty) in [(Spot::Hand, "Main hand", "Nothing: your fists"), (Spot::OffHand, "Off hand", "Nothing")] {
                ctx.hand_card(listed, s, body::lines(ctx.items, inv, spot).first().copied(), label, empty, if spot == Spot::Hand { Slot::Hand } else { Slot::OffHand });
            }
            s.spawn((DropTo::Gear, Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(8), row_gap: px(8), flex_shrink: 0.0, ..default() })).with_children(|g| {
                for spot in GEAR {
                    let here = body::lines(ctx.items, inv, spot);
                    // (Nothing shipped is listed at the arms: sleeves come with the torso's garment.)
                    if here.is_empty() && spot != Spot::Arms {
                        g.spawn((Node { width: px(SLOT), height: px(SLOT), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(flat::RADIUS)), flex_shrink: 0.0, ..default() }, BorderColor::all(LINE))).with_children(|e| {
                            e.spawn((Text::new(spot.label().to_uppercase()), theme::head(flat::SMALL), TextColor(flat::FAINT), TextLayout::no_wrap(), Node { position_type: PositionType::Absolute, top: px(4), left: px(7), ..default() }, Pickable::IGNORE));
                        });
                    }
                    for l in here {
                        listed.push(l.key());
                        ctx.tile(g, &l, Some(spot.label()), Lead::Nothing, l.stack.count as u32, String::new(), ctx.act(&l, Place::Carried, Vec::new()), ctx.picked.keys.contains(&l.key()), SLOT);
                    }
                }
            });
            if let Some(bar) = bar {
                s.spawn((Text::new("HOTBAR"), theme::head(flat::SMALL), TextColor(DIM), Node { margin: UiRect::top(px(8)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
                s.spawn((Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(6), row_gap: px(6), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|h| {
                    for (k, id) in bar.slots.iter().enumerate() {
                        let def = id.as_ref().and_then(|id| ctx.items.iter().position(|d| d.id == *id));
                        let have = def.is_some_and(|i| crate::hotbar::count(inv, i as u16) > 0);
                        h.spawn((DropTo::Hotbar(k), Node { width: px(44), height: px(44), justify_content: JustifyContent::Center, align_items: AlignItems::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(flat::RADIUS)), flex_shrink: 0.0, ..default() }, BorderColor::all(LINE), BackgroundColor(if def.is_some() { ROW } else { Color::NONE }), Pickable::IGNORE)).with_children(|c| {
                            if let Some(i) = def {
                                flat::item_icon(c, ctx.items.get(i), 30.0, if have { 1.0 } else { 0.35 });
                            }
                            c.spawn((Text::new((k + 1).to_string()), font(flat::SMALL), TextColor(flat::FAINT), Node { position_type: PositionType::Absolute, top: px(0), left: px(4), ..default() }, Pickable::IGNORE));
                        });
                    }
                });
            }
        });
        // The load, always in view.
        let k = &v.carry;
        let (word, color) = match k.burden {
            Burden::None => ("Fine", flat::GREEN),
            Burden::Burdened => ("Burdened", flat::AMBER),
            Burden::Heavy => ("Heavy", flat::AMBER),
            Burden::Overloaded => ("Overloaded", flat::RED_TEXT),
        };
        col.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(6), padding: UiRect::axes(px(20), px(12)), border: UiRect::top(px(1)), flex_shrink: 0.0, ..default() }, BorderColor::all(LINE), Pickable::IGNORE)).with_children(|l| {
            l.spawn((Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::FlexEnd, ..default() }, Pickable::IGNORE)).with_children(|t| {
                t.spawn((Text::new("LOAD"), theme::head(flat::HEAD), TextColor(DIM), Pickable::IGNORE));
                t.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::FlexEnd, column_gap: px(5), ..default() }, Pickable::IGNORE)).with_children(|n| {
                    n.spawn((Text::new(format!("{:.1}", k.weight_kg)), theme::strong(flat::TITLE), TextColor(TEXT), Pickable::IGNORE));
                    n.spawn((Text::new(format!("/ {:.1} kg", k.limit_kg)), font(flat::BODY), TextColor(DIM), Node { margin: UiRect::bottom(px(3)), ..default() }, Pickable::IGNORE));
                });
            });
            flat::bar(l, k.weight_kg / k.limit_kg.max(0.1), color, Node { height: px(5), ..default() });
            l.spawn((Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, ..default() }, Pickable::IGNORE)).with_children(|t| {
                t.spawn((Text::new(word), theme::strong(flat::SMALL), TextColor(color), Pickable::IGNORE));
                t.spawn((Text::new(format!("{:.1} / {:.0} L of room used", k.used_l, k.room_l)), font(flat::SMALL), TextColor(DIM), Pickable::IGNORE));
            });
        });
    });
}

/// The You column: the search, sort and chips, then a pane for the pockets and for each bag,
/// each with its own room; with stacks picked, where to send them, at its foot.
#[allow(clippy::too_many_arguments)]
fn you(mid: &mut ChildSpawnerCommands, ctx: &Ctx, listed: &mut Vec<Key>, v: &PrivateView, pouches: &[(Pouch, String, f32, f32)], right: Place, y: f32, picked: (usize, f32)) {
    let inv = &v.inventory;
    mid.spawn((DropTo::Place(Place::Carried), Node { flex_direction: FlexDirection::Column, flex_grow: 1.0, flex_basis: px(0), min_width: px(0), ..default() })).with_children(|col| {
        col.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(10), padding: UiRect::new(px(24), px(24), px(18), px(10)), flex_shrink: 0.0, ..default() }).with_children(|h| {
            find_row(h, ctx.find, ctx.ui);
            h.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(6), row_gap: px(6), ..default() }).with_children(|bar| {
                flat::chip(bar, "All", ctx.find.groups.is_empty()).observe(|_: On<PointerPress>, mut find: ResMut<Find>| find.groups.clear());
                for g in Group::ALL {
                    flat::chip(bar, g.name(), ctx.find.groups.contains(&g)).observe(move |_: On<PointerPress>, mut find: ResMut<Find>| find.toggle(g));
                }
            });
            let n: u32 = inv.carried.iter().map(|s| s.count as u32).sum();
            let kg: f32 = inv.carried.iter().map(|s| stack_kg(ctx.items, s)).sum();
            column_title(h, "You", Some(format!("{n} {}, {kg:.1} kg carried", if n == 1 { "thing" } else { "things" })));
        });
        scroller(col, 1, y, UiRect::new(px(24), px(24), px(0), px(16)), 12.0).with_children(|s| {
            for (p, name, room, used) in pouches {
                // (A stack whose pouch isn't there, should that ever be: with the pockets.)
                let here = |s: &ItemStack| s.home == *p || (*p == Pouch::Pockets && !pouches.iter().any(|x| x.0 == s.home));
                let lines: Vec<Line> = inv.carried.iter().enumerate().filter(|(_, s)| here(s)).map(|(k, s)| Line { from: Place::Carried, index: k as u32, stack: *s }).collect();
                let pane = Pane::Pouch(*p);
                let shut = ctx.ui.shut.contains(&pane);
                let bag = p.slot().and_then(|slot| inv.slot(slot)).and_then(|b| ctx.items.get(b.item as usize));
                drop_pane(s, Place::Pouch(*p)).with_children(|s| {
                    pane_head(s, Head { pane, def: bag.or_else(|| ctx.items.iter().find(|d| d.id == "jeans")), name, sub: pouch_where(*p).into(), right: format!("{used:.1} / {room:.0} L"), share: Some(used / room.max(0.1)), shut, count: lines.len(), target: None });
                    if !shut {
                        ctx.pane_body(listed, s, lines, right, "Empty");
                    }
                });
            }
        });
        if picked.0 > 0 {
            col.spawn((Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, align_items: AlignItems::Center, column_gap: px(8), row_gap: px(6), margin: UiRect::new(px(24), px(24), px(0), px(12)), padding: UiRect::axes(px(12), px(8)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(flat::RADIUS)), flex_shrink: 0.0, ..default() }, BorderColor::all(flat::GREEN), BackgroundColor(flat::GREEN_SOFT))).with_children(|l| {
                l.spawn((Text::new(format!("{} picked, {:.1} kg", picked.0, picked.1)), theme::strong(flat::BODY), TextColor(TEXT), TextLayout::no_wrap(), Pickable::IGNORE));
                l.spawn((Text::new("move to"), font(flat::BODY), TextColor(DIM), TextLayout::no_wrap(), Pickable::IGNORE));
                flat::chip(l, "Bags", false).observe(|_: On<PointerPress>, mut send: ResMut<SendPicked>| send.0 = Some(Place::Carried));
                if pouches.len() > 1 {
                    for (p, name, ..) in pouches {
                        let p = *p;
                        flat::chip(l, name, false).observe(move |_: On<PointerPress>, mut send: ResMut<SendPicked>| send.0 = Some(Place::Pouch(p)));
                    }
                }
                if let Some((id, name)) = ctx.open {
                    flat::chip(l, name, false).observe(move |_: On<PointerPress>, mut send: ResMut<SendPicked>| send.0 = Some(Place::Container(id)));
                }
                flat::chip(l, "Floor", false).observe(|_: On<PointerPress>, mut send: ResMut<SendPicked>| send.0 = Some(Place::Ground));
                flat::chip(l, "Clear", false).observe(|_: On<PointerPress>, mut picked: ResMut<Picked>| *picked = Picked::default());
            });
        }
    });
}

/// The search box, how the panes are sorted, and how they show their things (tiles or rows).
fn find_row(h: &mut ChildSpawnerCommands, find: &Find, ui: &PanelUi) {
    h.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), ..default() }).with_children(|r| {
        let (text, color) = match (find.query.is_empty(), find.typing) {
            (true, false) => ("Search".to_string(), DIM),
            (_, true) => (format!("{}_", find.query), TEXT),
            (false, false) => (find.query.clone(), TEXT),
        };
        r.spawn((
            Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), height: px(38), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, justify_content: JustifyContent::SpaceBetween, padding: UiRect::axes(px(12), px(0)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(flat::RADIUS)), overflow: Overflow::clip_x(), ..default() },
            BackgroundColor(ROW),
            BorderColor::all(if find.typing { flat::GREEN } else { LINE }),
        ))
        .with_children(|b| {
            b.spawn((Text::new(text), font(flat::BODY), TextColor(color), TextLayout::no_wrap(), Pickable::IGNORE));
            if !find.typing && !crate::pad::showing() {
                flat::key(b, "/");
            }
        })
        .observe(|_: On<PointerPress>, mut find: ResMut<Find>| find.typing = true);
        if !find.query.is_empty() || !find.groups.is_empty() {
            flat::chip(r, "Clear", false).observe(|_: On<PointerPress>, mut find: ResMut<Find>| *find = Find::default());
        }
        flat::chip(r, &format!("Sort: {}", ui.sort.name()), false).observe(|_: On<PointerPress>, mut ui: ResMut<PanelUi>| ui.sort.next());
        flat::chip(r, if ui.rows { "View: rows" } else { "View: tiles" }, false).observe(|_: On<PointerPress>, mut ui: ResMut<PanelUi>| ui.rows = !ui.rows);
    });
}

/// The Nearby column: the open container and the floor in reach, each a pane; a press on a
/// pane's name makes it the one your things go to. At its foot, everything across at once.
fn nearby(mid: &mut ChildSpawnerCommands, ctx: &Ctx, listed: &mut Vec<Key>, v: &PrivateView, floor: &[(u32, ItemStack)], right: Place, y: f32) {
    let items = ctx.items;
    mid.spawn((DropTo::Place(right), Node { width: percent(27), min_width: px(300), max_width: px(500), flex_direction: FlexDirection::Column, flex_shrink: 0.0, border: UiRect::left(px(1)), ..default() }, BorderColor::all(LINE))).with_children(|col| {
        col.spawn((Node { flex_direction: FlexDirection::Column, padding: UiRect::new(px(20), px(20), px(18), px(10)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|h| column_title(h, "Nearby", None));
        let to_floor = matches!(right, Place::Ground);
        scroller(col, 2, y, UiRect::new(px(20), px(20), px(0), px(16)), 12.0).with_children(|s| {
            if let Some(cv) = &v.open {
                let used = zombye_sim::inventory::stacks_volume(items, &cv.stacks);
                let shut = ctx.ui.shut.contains(&Pane::Container);
                drop_pane(s, Place::Container(cv.id)).with_children(|s| {
                    pane_head(s, Head { pane: Pane::Container, def: None, name: &cv.name, sub: if to_floor { String::new() } else { "your things go here".into() }, right: format!("{used:.1} / {:.0} L", cv.room_l), share: Some(used / cv.room_l.max(0.1)), shut, count: cv.stacks.len(), target: Some(!to_floor) });
                    if !shut {
                        ctx.pane_body(listed, s, cv.stacks.iter().enumerate().map(|(k, s)| Line { from: Place::Container(cv.id), index: k as u32, stack: *s }).collect(), Place::Carried, "Empty");
                    }
                });
            }
            let kg: f32 = floor.iter().map(|(_, s)| stack_kg(items, s)).sum();
            let shut = ctx.ui.shut.contains(&Pane::Floor);
            let sub = if v.open.is_some() && to_floor { "your things go here" } else { "within reach" };
            drop_pane(s, Place::Ground).with_children(|s| {
                pane_head(s, Head { pane: Pane::Floor, def: None, name: "Floor", sub: sub.into(), right: format!("{:.1} kg", kg.max(0.0)), share: None, shut, count: floor.len(), target: Some(to_floor && v.open.is_some()) });
                if !shut {
                    ctx.pane_body(listed, s, floor.iter().map(|(id, s)| Line { from: Place::Ground, index: *id, stack: *s }).collect(), Place::Carried, "Nothing within reach");
                }
            });
            if v.open.is_none() {
                dim(s, "E: search the cupboard, drawer or fridge you face".into());
            }
        });
        col.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), padding: UiRect::new(px(20), px(20), px(10), px(14)), flex_shrink: 0.0, ..default() }).with_children(|b| {
            let across = if to_floor { Column::Floor } else { Column::Container };
            flat::button(b, "Take all", true).observe(move |_: On<PointerPress>, mut all: ResMut<MoveAll>| all.0 = Some(across));
            flat::button(b, if to_floor { "Drop mine" } else { "Put mine in" }, false).observe(|_: On<PointerPress>, mut all: ResMut<MoveAll>| all.0 = Some(Column::You));
        });
    });
}

/// A pane's head and things together: somewhere a dragged thing can be dropped (`to`).
fn drop_pane<'a>(s: &'a mut ChildSpawnerCommands, to: Place) -> EntityCommands<'a> {
    s.spawn((DropTo::Place(to), Node { flex_direction: FlexDirection::Column, row_gap: px(12), flex_shrink: 0.0, ..default() }))
}

/// A pane that folds: the pockets or a bag, the open container, the floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Pouch(Pouch),
    Container,
    Floor,
}

/// What a pane's head says.
struct Head<'a> {
    pane: Pane,
    /// The bag, for its icon.
    def: Option<&'a ItemDef>,
    name: &'a str,
    /// After the name, quietly: where the bag is carried, or that this pane is the target.
    sub: String,
    /// At the right end: how full.
    right: String,
    /// How full, as the line under the head (`None`: no limit, a plain hairline).
    share: Option<f32>,
    shut: bool,
    count: usize,
    /// A pane of the Nearby column: whether your things go to it (a press on its head makes it so).
    target: Option<bool>,
}

/// A pane's head: a fold mark, its name and how full it is, over a thin bar of that.
fn pane_head(s: &mut ChildSpawnerCommands, h: Head) {
    let pane = h.pane;
    let mut e = s.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(6), flex_shrink: 0.0, ..default() });
    e.with_children(|c| {
        c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), ..default() }, Pickable::IGNORE)).with_children(|r| {
            r.spawn((Node { width: px(20), height: px(20), justify_content: JustifyContent::Center, align_items: AlignItems::Center, border_radius: BorderRadius::all(px(4)), flex_shrink: 0.0, ..default() }, BackgroundColor(CHIP)))
                .with_children(|f| {
                    f.spawn((Text::new(if h.shut { "+" } else { "-" }), theme::strong(flat::SMALL), TextColor(TEXT), Pickable::IGNORE));
                })
                .observe(move |mut ev: On<PointerPress>, mut ui: ResMut<PanelUi>| {
                    ev.propagate(false);
                    match ui.shut.iter().position(|p| *p == pane) {
                        Some(i) => {
                            ui.shut.remove(i);
                        }
                        None => ui.shut.push(pane),
                    }
                });
            if h.def.is_some() {
                flat::item_icon(r, h.def, 24.0, 1.0);
            }
            r.spawn((Text::new(h.name.to_uppercase()), theme::head(flat::HEAD), TextColor(TEXT), TextLayout::no_wrap(), Pickable::IGNORE));
            if !h.sub.is_empty() {
                r.spawn((Text::new(h.sub), font(flat::SMALL), TextColor(if h.target == Some(true) { flat::GREEN } else { DIM }), TextLayout::no_wrap(), Pickable::IGNORE));
            }
            r.spawn((Node { flex_grow: 1.0, ..default() }, Pickable::IGNORE));
            if h.shut {
                r.spawn((Text::new(format!("{} {}", h.count, if h.count == 1 { "stack" } else { "stacks" })), font(flat::SMALL), TextColor(DIM), TextLayout::no_wrap(), Pickable::IGNORE));
            }
            r.spawn((Text::new(h.right), theme::strong(flat::SMALL), TextColor(TEXT), TextLayout::no_wrap(), Pickable::IGNORE));
        });
        match h.share {
            Some(share) => flat::bar(c, share, if share > 1.0 { flat::RED } else if share > 0.85 { flat::AMBER } else { flat::GREEN }, Node::default()),
            None => flat::line(c),
        }
    });
    if h.target.is_some() {
        e.observe(move |_: On<PointerPress>, mut ui: ResMut<PanelUi>| {
            let floor = pane == Pane::Floor;
            if ui.floor != floor {
                ui.floor = floor;
            }
        });
    }
}

/// The Make screen: the recipes, in the frame's one wide column.
fn make_column(mid: &mut ChildSpawnerCommands, join: &JoinInfo, v: &PrivateView, busy: f32, flight: &InFlight, ui: &MakeUi, y: f32, heat: bool, water: (bool, bool)) {
    mid.spawn(Node { flex_direction: FlexDirection::Column, flex_grow: 1.0, flex_basis: px(0), min_width: px(0), max_width: px(1000), ..default() }).with_children(|col| {
        col.spawn((Node { flex_direction: FlexDirection::Column, padding: UiRect::new(px(24), px(24), px(18), px(10)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|h| {
            column_title(h, "Things to make", crate::help_text::keys().then(|| "Click a group to open it. Shift-click a recipe to make all you can. Right-click a recipe to pin it (*).".into()));
        });
        scroller(col, 3, y, UiRect::new(px(24), px(24), px(0), px(16)), 3.0).with_children(|s| {
            if join.recipes.is_empty() {
                dim(s, "Nothing to make".into());
            }
            make(s, join, v, busy, flight, ui, heat, water);
        });
    });
}

/// What it takes to say how worn a weapon is: the weapons, and the share of its life under
/// which one is battered (the server's `CombatTuning::battered_below`).
#[derive(Clone, Copy)]
pub(crate) struct Wear<'a> {
    pub(crate) weapons: &'a [zombye_sim::WeaponDef],
    pub(crate) battered_below: f32,
}

impl<'a> Wear<'a> {
    pub(crate) fn of(join: &'a JoinInfo) -> Self {
        Self { weapons: &join.weapons, battered_below: join.combat.battered_below }
    }
}

/// What drawing a pane's rows needs to know.
struct Ctx<'a> {
    items: &'a [ItemDef],
    wear: Wear<'a>,
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
    /// The pockets and bags carried (which, name), for the menus.
    pouches: &'a [(Pouch, String)],
    recipes: &'a [crafting::RecipeDef],
    /// What we have to craft with ([`stock`]).
    stock: Option<&'a zombye_sim::Inventory>,
}

/// What we have to craft with, by the server's rule (`World::craft_stock`): what we carry and
/// hold, what lies on our floor in reach, and what is in the container we have open.
fn stock(v: &PrivateView, floor: &[(u32, ItemStack)]) -> zombye_sim::Inventory {
    let near = crafting::with_floor(&v.inventory, floor.iter().map(|f| f.1));
    crafting::with_floor(&near, v.open.iter().flat_map(|c| c.stacks.iter().copied()))
}

impl Ctx<'_> {
    fn shows(&self, from: Place, index: u32, s: &ItemStack) -> bool {
        on_show(self.find, self.items, self.picked, from, index, s)
    }

    /// What a click on the row of `l` (and `more`, the other stacks it stands for) does.
    fn act(&self, l: &Line, to: Place, more: Vec<Line>) -> RowAction {
        RowAction { stack: l.stack, title: name_of(self.items, self.wear, self.now, &l.stack), left: Some((l.from, l.index, to)), more, menu: menu_entries(self.me, self.items, l.from, l.index, &l.stack, self.open, self.pouches, &self.makes(l)), craft: None }
    }

    /// The recipes a thing lying nearby would let us make ([`makes_with`]), or one of our own starts
    /// ([`makes_from`]).
    fn makes(&self, l: &Line) -> Vec<(u16, String, u16, u16)> {
        match (l.from, self.view, self.stock) {
            (Place::Container(_) | Place::Ground, Some(v), Some(near)) => makes_with(self.recipes, self.items, &v.inventory, near, &l.stack),
            (_, Some(_), Some(near)) => makes_from(self.recipes, self.items, near, &l.stack),
            _ => Vec::new(),
        }
    }

    /// What follows the name, dimmed: how fresh, goes left, uses left in the opened one (a box of
    /// plasters, a roll of gauze), and that it is on its way.
    fn note(&self, l: &Line) -> String {
        let mut notes = Vec::new();
        let state = state_of(self.items, self.wear, self.now, &l.stack);
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
            icon: def.map(|d| d.id.clone()),
            name: def.map_or("?".into(), |d| d.name.clone()),
            note: self.note(l),
            kind: kind.map_or_else(|| def.map_or("", |d| Group::of(d.category).name()).to_string(), str::to_string),
            qty: if l.stack.count > 1 { l.stack.count.to_string() } else { String::new() },
            kg: format!("{:.1}", stack_kg(self.items, &l.stack)),
            faint: false,
        }
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

    /// A pane's things as tiles that wrap (never a line wider than the pane): the same stacks,
    /// order and folding as [`Ctx::rows`]. Several stacks of one item are one tile with their
    /// count together and a fold mark; opened, each stack is a tile after it.
    fn tiles(&self, listed: &mut Vec<Key>, col: &mut ChildSpawnerCommands, lines: impl Iterator<Item = Line>, to: Place) {
        let is_picked = |l: &Line| self.picked.keys.contains(&l.key());
        col.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(8), row_gap: px(8), flex_shrink: 0.0, ..default() }).with_children(|wrap| {
            for stacks in arrange(self.items, lines.filter(|l| self.shows(l.from, l.index, &l.stack)), self.ui.sort) {
                let first = stacks[0];
                if stacks.len() == 1 {
                    listed.push(first.key());
                    self.tile(wrap, &first, None, Lead::Nothing, first.stack.count as u32, self.note(&first), self.act(&first, to, Vec::new()), is_picked(&first), flat::TILE);
                    continue;
                }
                let fold = (Column::of(first.from), first.stack.item);
                let open = self.ui.unfolded(fold.0, fold.1) || (stacks.iter().any(is_picked) && !stacks.iter().all(is_picked));
                let note = if stacks.iter().any(|l| self.flight.asked_for(l.from, l.index, l.stack.item) > 0) { "moving".into() } else { String::new() };
                self.tile(wrap, &first, None, Lead::Fold { open, key: fold }, stacks.iter().map(|l| l.stack.count as u32).sum(), note, self.act(&first, to, stacks[1..].to_vec()), stacks.iter().all(is_picked), flat::TILE);
                for l in &stacks {
                    listed.push(l.key());
                    if open {
                        self.tile(wrap, l, None, Lead::Child, l.stack.count as u32, self.note(l), self.act(l, to, Vec::new()), is_picked(l), flat::TILE);
                    }
                }
            }
        });
    }

    /// One thing as a tile, `size` square: its icon, how many at the top right, its name along
    /// the foot (one line, cut off if too long: the hover card has it whole), and at the top left
    /// `label` (the place it is worn at) or else what there is to say about it (`note`: stale,
    /// 2/3 left, moving). A weapon that wears shows how much life it has left as a thin bar.
    #[allow(clippy::too_many_arguments)]
    fn tile(&self, wrap: &mut ChildSpawnerCommands, l: &Line, label: Option<&str>, lead: Lead, qty: u32, note: String, action: RowAction, picked: bool, size: f32) {
        let def = self.items.get(l.stack.item as usize);
        let rest = if picked { ROW_PICKED } else { ROW };
        let moving = self.flight.asked_for(l.from, l.index, l.stack.item) > 0;
        let edge = match (picked, lead) {
            (true, _) => flat::GREEN,
            (false, Lead::Child) => Color::srgba(0.553, 0.69, 0.42, 0.45),
            _ => LINE,
        };
        let mut e = wrap.spawn((Node { width: px(size), height: px(size), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(flat::RADIUS)), flex_shrink: 0.0, overflow: Overflow::clip(), ..default() }, BorderColor::all(edge), BackgroundColor(rest)));
        e.with_children(|t| {
            let icon = (size * 0.5).round();
            t.spawn((Node { position_type: PositionType::Absolute, left: px((size - 2.0 - icon) / 2.0), top: px((size - icon) / 2.0 - 8.0), ..default() }, Pickable::IGNORE)).with_children(|i| flat::item_icon(i, def, icon, if moving { 0.4 } else { 1.0 }));
            // Top left: the fold mark, then the place or the note.
            t.spawn((Node { position_type: PositionType::Absolute, left: px(5), top: px(3), max_width: px(size - 38.0), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(4), overflow: Overflow::clip_x(), ..default() }, Pickable::IGNORE)).with_children(|tl| {
                if let Lead::Fold { open, key } = lead {
                    tl.spawn((Node { width: px(18), height: px(18), justify_content: JustifyContent::Center, align_items: AlignItems::Center, border_radius: BorderRadius::all(px(4)), flex_shrink: 0.0, ..default() }, BackgroundColor(CHIP)))
                        .with_children(|f| {
                            f.spawn((Text::new(if open { "-" } else { "+" }), theme::strong(flat::SMALL), TextColor(TEXT), Pickable::IGNORE));
                        })
                        .observe(move |mut ev: On<PointerPress>, mut ui: Option<ResMut<PanelUi>>| {
                            ev.propagate(false);
                            if let (PointerButton::Primary, Some(ui)) = (ev.button, ui.as_mut()) {
                                ui.toggle_fold(key);
                            }
                        });
                }
                match label {
                    Some(label) => {
                        tl.spawn((Text::new(label.to_uppercase()), theme::head(flat::SMALL), TextColor(DIM), TextLayout::no_wrap(), Node { flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
                    }
                    None if !note.is_empty() => {
                        tl.spawn((Text::new(note.clone()), font(flat::SMALL), TextColor(note_colour(&note)), TextLayout::no_wrap(), Node { flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
                    }
                    None => {}
                }
            });
            if qty > 1 {
                t.spawn((Text::new(format!("x{qty}")), theme::strong(flat::SMALL), TextColor(TEXT), TextLayout::no_wrap(), Node { position_type: PositionType::Absolute, right: px(6), top: px(3), ..default() }, Pickable::IGNORE));
            }
            // The name: centred when it fits, else from the start and cut off.
            let mut name = def.map_or("?".to_string(), |d| d.name.clone());
            let wide = |n: &str| theme::width(theme::Face::Text, n, flat::SMALL) > size - 12.0;
            let fits = !wide(&name);
            if !fits {
                // Too long for the tile: cut, with dots (the card on hover has it whole).
                while name.chars().count() > 3 && wide(&format!("{}...", name.trim_end())) {
                    name.pop();
                }
                name = format!("{}...", name.trim_end());
            }
            t.spawn((Node { position_type: PositionType::Absolute, left: px(5), right: px(5), bottom: px(7), justify_content: if fits { JustifyContent::Center } else { JustifyContent::FlexStart }, overflow: Overflow::clip_x(), ..default() }, Pickable::IGNORE)).with_children(|n| {
                n.spawn((Text::new(name), font(flat::SMALL), TextColor(TEXT), TextLayout::no_wrap(), Node { flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
            });
            // A weapon that wears: the life it has left.
            if let Some((left, of, state)) = zombye_sim::wear::condition(self.items, self.wear.weapons, &l.stack, self.wear.battered_below) {
                use zombye_sim::wear::Condition;
                let colour = match state {
                    Condition::Good => flat::GREEN,
                    Condition::Broken => flat::RED,
                    _ => flat::AMBER,
                };
                flat::bar(t, left as f32 / (of as f32).max(1.0), colour, Node { position_type: PositionType::Absolute, left: px(8), right: px(8), bottom: px(3), height: px(2), ..default() });
            }
        });
        clickable(&mut e, action, rest);
    }

    /// What a hand holds, as a card across the Gear column (or that it holds nothing).
    fn hand_card(&self, listed: &mut Vec<Key>, col: &mut ChildSpawnerCommands, l: Option<Line>, label: &str, empty: &str, slot: Slot) {
        let drop = DropTo::Place(Place::Slot(slot));
        let node = Node { height: px(64), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), padding: UiRect::axes(px(12), px(0)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(flat::RADIUS)), flex_shrink: 0.0, overflow: Overflow::clip_x(), ..default() };
        let Some(l) = l else {
            col.spawn((drop, node, BorderColor::all(LINE), Pickable::IGNORE)).with_children(|c| {
                c.spawn((Node { width: px(40), flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
                c.spawn((Node { flex_direction: FlexDirection::Column, ..default() }, Pickable::IGNORE)).with_children(|t| {
                    t.spawn((Text::new(label.to_uppercase()), theme::head(flat::SMALL), TextColor(flat::FAINT), TextLayout::no_wrap(), Pickable::IGNORE));
                    t.spawn((Text::new(empty.to_string()), font(flat::BODY), TextColor(DIM), TextLayout::no_wrap(), Pickable::IGNORE));
                });
            });
            return;
        };
        listed.push(l.key());
        let picked = self.picked.keys.contains(&l.key());
        let rest = if picked { ROW_PICKED } else { ROW };
        let def = self.items.get(l.stack.item as usize);
        let note = self.note(&l);
        let mut e = col.spawn((drop, node, BorderColor::all(if picked { flat::GREEN } else { LINE }), BackgroundColor(rest)));
        e.with_children(|c| {
            flat::item_icon(c, def, 40.0, 1.0);
            c.spawn((Node { flex_direction: FlexDirection::Column, flex_grow: 1.0, flex_basis: px(0), min_width: px(0), overflow: Overflow::clip_x(), ..default() }, Pickable::IGNORE)).with_children(|t| {
                t.spawn((Text::new(label.to_uppercase()), theme::head(flat::SMALL), TextColor(DIM), TextLayout::no_wrap(), Pickable::IGNORE));
                t.spawn((Text::new(def.map_or("?".to_string(), |d| d.name.clone())), theme::strong(flat::BODY), TextColor(TEXT), TextLayout::no_wrap(), Pickable::IGNORE));
            });
            c.spawn((Node { flex_direction: FlexDirection::Column, align_items: AlignItems::FlexEnd, flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|r| {
                r.spawn((Text::new(format!("{:.1} kg", stack_kg(self.items, &l.stack))), font(flat::SMALL), TextColor(DIM), TextLayout::no_wrap(), Pickable::IGNORE));
                if !note.is_empty() {
                    r.spawn((Text::new(note.clone()), font(flat::SMALL), TextColor(note_colour(&note)), TextLayout::no_wrap(), Pickable::IGNORE));
                }
            });
        });
        clickable(&mut e, self.act(&l, Place::Carried, Vec::new()), rest);
    }

    /// A pane's things: tiles, or rows; or a line saying there are none (or none that match).
    fn pane_body(&self, listed: &mut Vec<Key>, col: &mut ChildSpawnerCommands, lines: Vec<Line>, to: Place, empty: &str) {
        if lines.is_empty() {
            dim(col, empty.into());
            return;
        }
        if !lines.iter().any(|l| self.shows(l.from, l.index, &l.stack)) {
            dim(col, "Nothing that matches".into());
            return;
        }
        if self.ui.rows {
            col.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(1), flex_shrink: 0.0, ..default() }).with_children(|rows| self.rows(listed, rows, lines.into_iter(), to));
        } else {
            self.tiles(listed, col, lines.into_iter(), to);
        }
    }
}

/// The colour of what a tile says about its thing: the worst red, a warning amber, the rest quiet.
fn note_colour(note: &str) -> Color {
    if ["mouldy", "broken"].iter().any(|w| note.contains(w)) {
        flat::RED_TEXT
    } else if ["stale", "battered", "worn", "empty"].iter().any(|w| note.contains(w)) {
        flat::AMBER
    } else {
        DIM
    }
}

/// The pockets and each bag carried, with the name a menu or a chip calls it by.
fn pouch_names(items: &[ItemDef], inv: &zombye_sim::Inventory, pockets_l: f32) -> Vec<(Pouch, String)> {
    inv.pouches(items, pockets_l).into_iter().map(|(p, ..)| (p, pouch_name(items, inv, p).to_lowercase())).collect()
}

/// The thing the pointer is on (its tile or row, its stack, and how many stacks that stands
/// for), for the card that says the rest about it ([`draw_card`]).
#[derive(Resource, Default)]
struct HoverCard(Option<(Entity, ItemStack, usize)>);

#[derive(Component)]
struct CardRoot;

/// The hover card: a thing's whole name, its kind and weight, and what there is to say about
/// it, beside the pointer. Drawn when what the pointer is on changes.
#[allow(clippy::too_many_arguments)]
fn draw_card(mut commands: Commands, card: Res<HoverCard>, panel: Res<InventoryPanel>, menu: Res<ItemMenu>, join: Res<JoinInfo>, link: Res<ServerLink>, ui: Res<UiScale>, windows: Query<&Window, With<bevy::window::PrimaryWindow>>, mut drawn: Local<Option<Entity>>, roots: Query<Entity, With<CardRoot>>) {
    let want = card.0.filter(|_| panel.open && menu.open.is_none());
    if want.map(|w| w.0) == *drawn {
        return;
    }
    *drawn = want.map(|w| w.0);
    for e in &roots {
        commands.entity(e).despawn();
    }
    let Some((_, stack, n)) = want else { return };
    let Some(def) = join.items.get(stack.item as usize) else { return };
    let Some(window) = windows.single().ok() else { return };
    let Some(cursor) = window.cursor_position() else { return };
    let size = Vec2::new(280.0, 120.0);
    let at = (cursor / ui.0.max(0.1) + Vec2::new(18.0, 18.0)).min((Vec2::new(window.width(), window.height()) / ui.0.max(0.1) - size).max(Vec2::ZERO));
    let state = state_of(&join.items, Wear::of(&join), link.curr.game_minute, &stack);
    let mut facts = vec![Group::of(def.category).name().to_string(), format!("{:.1} kg", def.weight_kg), format!("{:.1} L", def.volume_l)];
    if stack.count > 1 {
        facts.push(format!("{} in the stack", stack.count));
    }
    if n > 1 {
        facts.push(format!("{n} stacks"));
    }
    commands
        .spawn((CardRoot, GlobalZIndex(19), Node { position_type: PositionType::Absolute, left: px(at.x), top: px(at.y), max_width: px(size.x), flex_direction: FlexDirection::Column, row_gap: px(3), padding: UiRect::axes(px(12), px(9)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(8)), ..default() }, BackgroundColor(flat::SOLID), BorderColor::all(Color::srgba(0.949, 0.902, 0.788, 0.25)), Pickable::IGNORE))
        .with_children(|c| {
            c.spawn((Text::new(def.name.clone()), theme::strong(flat::BODY), TextColor(TEXT), Pickable::IGNORE));
            c.spawn((Text::new(facts.join("  ·  ")), font(flat::SMALL), TextColor(DIM), Pickable::IGNORE));
            if !state.is_empty() {
                c.spawn((Text::new(state.clone()), font(flat::SMALL), TextColor(note_colour(&state)), Pickable::IGNORE));
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
    /// The list is opened (it starts as one line saying how many you can make).
    shown: bool,
    path: Option<std::path::PathBuf>,
    loaded: bool,
    /// The row the pointer is on (`MakeAct::key`), so a row built again under it keeps its
    /// hover backing instead of flashing plain for a frame.
    hover: Option<String>,
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

/// The Make list's family of recipes that need heat.
const COOKING: &str = "cooking";

/// What a click on a Make row does.
#[derive(Clone)]
enum MakeAct {
    /// Make this recipe; Shift-click makes it as many times as there's the stuff for (the second).
    Craft(u16, u16),
    Family(String),
    None,
}

impl MakeAct {
    /// What the row is, the same from one build of the list to the next.
    fn key(&self, label: &str) -> String {
        match self {
            MakeAct::Craft(k, _) => format!("craft:{k}"),
            MakeAct::Family(f) => format!("family:{f}"),
            MakeAct::None => format!("none:{label}"),
        }
    }
}

/// The label of the recipe under way: its name before the seconds left, which `count_down`
/// writes in each frame; `more` queued after it.
#[derive(Component, Clone, Debug, PartialEq)]
struct MakingLeft {
    head: String,
    more: usize,
}

impl MakingLeft {
    fn text(&self, busy: f32) -> String {
        match self.more {
            0 => format!("{}  (making: {busy:.1} s left)", self.head),
            n => format!("{}  (making: {busy:.1} s left, {n} more queued)", self.head),
        }
    }
}

/// The seconds left on the recipe under way, written into its row in place.
fn count_down(link: Res<ServerLink>, me: Res<LocalPlayer>, mut rows: Query<(&MakingLeft, &mut Text)>) {
    let busy = link.curr.players.iter().find(|p| p.id == me.id).map_or(0.0, |p| p.busy);
    for (m, mut text) in &mut rows {
        let want = m.text(busy);
        if text.0 != want {
            text.0 = want;
        }
    }
}

/// A row of the Make list: left click makes the recipe or opens the family; right click pins or
/// unpins the recipe.
#[allow(clippy::too_many_arguments)]
fn make_row(c: &mut ChildSpawnerCommands, ui: &MakeUi, label: String, making: Option<MakingLeft>, right: String, act: MakeAct, fav: Option<String>, color: Color) {
    let hover = !matches!(act, MakeAct::None);
    let key = act.key(&label);
    // Built again under the pointer: start hovered, as it was.
    let bg = if hover && ui.hover.as_deref() == Some(key.as_str()) { ROW_HOVER } else { ROW };
    let (over_key, out_key) = (key.clone(), key);
    c.spawn((
        Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, column_gap: px(10), padding: UiRect::axes(px(10), px(6)), border_radius: BorderRadius::all(px(4)), flex_shrink: 0.0, ..default() },
        BackgroundColor(bg),
    ))
    .with_children(|r| {
        let mut text = r.spawn((Text::new(label), font(ROW_FONT), TextColor(color), Pickable::IGNORE));
        if let Some(m) = making {
            text.insert(m);
        }
        r.spawn((Text::new(right), font(ROW_FONT), TextColor(DIM), Pickable::IGNORE));
    })
    .observe(move |ev: On<PointerOver>, mut q: Query<&mut BackgroundColor>, mut ui: ResMut<MakeUi>| {
        if hover && let Ok(mut bg) = q.get_mut(ev.entity) {
            bg.0 = ROW_HOVER;
        }
        ui.hover = Some(over_key.clone());
    })
    .observe(move |ev: On<PointerOut>, mut q: Query<&mut BackgroundColor>, mut ui: ResMut<MakeUi>| {
        if let Ok(mut bg) = q.get_mut(ev.entity) {
            bg.0 = ROW;
        }
        if ui.hover.as_deref() == Some(out_key.as_str()) {
            ui.hover = None;
        }
    })
    .observe(move |ev: On<PointerPress>, keys: Res<ButtonInput<KeyCode>>, mut clicks: ResMut<Clicks>, mut ui: ResMut<MakeUi>| match (ev.button, &act) {
        (PointerButton::Primary, MakeAct::Craft(k, all)) => {
            // Shift: make as many as there's the stuff for.
            let times = if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) { (*all).max(1) } else { 1 };
            clicks.0.push(RowClick { action: RowAction { stack: ItemStack::one(0), title: String::new(), left: None, more: Vec::new(), menu: Vec::new(), craft: Some((*k, times)) }, button: ev.button, one: false })
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

/// How many times over "make all" makes `r` from what `v` has: as many as there's the stuff for
/// (beyond what is queued already), no more than `room` (the job queue's) and at most 16.
fn make_all(k: u16, r: &zombye_sim::RecipeDef, v: &PrivateView, items: &[ItemDef], room: u32) -> u32 {
    // Those already queued have their share set aside (the server counts them in).
    let queued = v.jobs.iter().filter(|j| **j == JobKind::Craft { recipe: k }).count() as u32;
    let cap = room.min(16);
    crafting::times_can(r, &v.inventory, items, cap + queued).saturating_sub(queued).min(cap)
}

/// The Make list (#12, #96): recipes you can make first, families of variants (rags from a
/// T-shirt, a jumper, ...) folded into one row that opens, pinned ones on top (right-click to pin),
/// and what the rest are missing in plain words, dimmed. The one under way shows its seconds left
/// (`busy`), queued ones counted.
fn make(c: &mut ChildSpawnerCommands, join: &JoinInfo, v: &PrivateView, busy: f32, flight: &InFlight, ui: &MakeUi, heat: bool, water: (bool, bool)) {
    let (water, cistern) = water;
    if join.recipes.is_empty() {
        return;
    }
    let items = &join.items;
    let mult = join.skills.craft_time_mult(v.skill_level(zombye_sim::Skill::Handiwork));
    // Room left in the job queue (make all fills it no further).
    let room = (join.carry.max_jobs as usize).saturating_sub(v.jobs.len()) as u32;
    // The server's rule, with the heat a cooking recipe needs (`fire::heat_near`, worked out by the caller).
    let check = |r: &zombye_sim::RecipeDef| crafting::check(r, &v.inventory, items).and_then(|()| if r.heat && !heat { Err(crafting::NEEDS_HEAT.to_string()) } else if r.water && !water { Err(zombye_sim::power::NEEDS_WATER.to_string()) } else if r.cistern && !cistern { Err(zombye_sim::power::NEEDS_CISTERN.to_string()) } else { Ok(()) });
    // One recipe's row. `lead` is "* " for a pinned one.
    let recipe_row = |c: &mut ChildSpawnerCommands, k: usize, indent: &str, lead: &str| {
        let r = &join.recipes[k];
        let k = k as u16;
        let craft = JobKind::Craft { recipe: k };
        let queued = v.jobs.iter().filter(|j| **j == craft).count();
        let head = format!("{indent}{lead}{}", r.name);
        // The one under way counts down in place (`count_down`).
        let making = (v.jobs.first() == Some(&craft)).then(|| MakingLeft { head: head.clone(), more: queued.saturating_sub(1) });
        let state = match (&making, queued) {
            (Some(m), _) => m.text(busy),
            (None, 0) if flight.crafts.iter().any(|(x, _)| *x == k) => format!("{head}  (asked)"),
            (None, 0) => head.clone(),
            (None, n) => format!("{head}  ({n} queued)"),
        };
        let makes = match r.places {
            Some(zombye_sim::Placed::Campfire) => "a campfire".to_string(),
            None => r.outputs.iter().map(|o| crafting::amount(items, &o.item, o.count)).collect::<Vec<_>>().join(" + "),
        };
        let fav = Some(r.id.clone());
        match check(r) {
            Ok(()) => {
                let all = make_all(k, r, v, items, room);
                let more = if all > 1 && crate::help_text::keys() { format!(", Shift-click: all {all}") } else { String::new() };
                make_row(c, ui, state, making, format!("{makes}, {:.0} s{more}", r.secs * mult), MakeAct::Craft(k, all as u16), fav, TEXT)
            }
            // Under way with what it takes set aside already.
            Err(_) if queued > 0 => make_row(c, ui, state, making, String::new(), MakeAct::None, fav, DIM),
            // (In full: each thing missing, how many you have, that a tool is kept. Short: their names.)
            Err(why) => make_row(c, ui, head, None, if crate::help_text::explains() { why } else { crafting::missing_brief(r, &v.inventory, items).unwrap_or(why) }, MakeAct::None, fav, DIM),
        }
    };
    let can = |k: usize| check(&join.recipes[k]).is_ok() || v.jobs.contains(&JobKind::Craft { recipe: k as u16 });
    let pinned = |k: usize| ui.favs.contains(&join.recipes[k].id);
    // Pinned first, makeable ones leading.
    let mut favs: Vec<usize> = (0..join.recipes.len()).filter(|&k| pinned(k)).collect();
    favs.sort_by_key(|&k| !can(k));
    for k in favs {
        recipe_row(c, k, "", "* ");
    }
    // The rest in families by what they make first.
    let mut families: Vec<(String, Vec<usize>)> = Vec::new();
    for (k, r) in join.recipes.iter().enumerate().filter(|(k, _)| !pinned(*k)) {
        // Cooking is one family of its own, whatever it makes.
        let key = if r.heat { COOKING.to_string() } else { r.outputs.first().map_or(r.id.clone(), |o| o.item.clone()) };
        match families.iter_mut().find(|(f, _)| *f == key) {
            Some((_, v)) => v.push(k),
            None => families.push((key, vec![k])),
        }
    }
    // Families with something to make come first (stable, so the file's order otherwise).
    families.sort_by_key(|(_, ks)| !ks.iter().any(|&k| can(k)));
    // By a fire or a stove, cooking comes first, opened.
    if heat && let Some(i) = families.iter().position(|(k, _)| k == COOKING) {
        let f = families.remove(i);
        families.insert(0, f);
    }
    for (key, mut ks) in families {
        if ks.len() == 1 && key != COOKING {
            recipe_row(c, ks[0], "", "");
            continue;
        }
        let ready = ks.iter().filter(|&&k| can(k)).count();
        let cooking = key == COOKING;
        let open = ui.open.contains(&key) || ui.open.iter().any(|k| k == "*") || (cooking && heat);
        let name = if cooking { "Cooking".to_string() } else { items.iter().find(|d| d.id == key).map_or(key.clone(), |d| format!("{}s", d.name)) };
        let summary = match (cooking, heat, ready) {
            (true, false, _) => "needs a lit fire or a working stove".to_string(),
            (true, true, 0) => format!("{} dishes, none you have the food for", ks.len()),
            (true, true, n) => format!("{n} of {} dishes ready", ks.len()),
            (false, _, 0) => format!("{} ways, none yet", ks.len()),
            (false, _, n) => format!("{n} of {} ways ready", ks.len()),
        };
        make_row(c, ui, format!("{} {name}", if open { "v" } else { ">" }), None, summary, MakeAct::Family(key.clone()), None, if ready > 0 { TEXT } else { DIM });
        if open {
            ks.sort_by_key(|&k| !can(k));
            for k in ks {
                recipe_row(c, k, "    ", "");
            }
        }
    }
}

/// Whether the stack at `from`/`index` gets a row: it fits the search and chips, or it is picked
/// (a picked stack always shows, so what the bar will move is in view).
fn on_show(find: &Find, items: &[ItemDef], picked: &Picked, from: Place, index: u32, s: &ItemStack) -> bool {
    find.shows(items, s) || picked.keys.contains(&(from, index, s.item))
}

/// A line of plain dimmed text in a pane (nothing here, nothing that matches).
fn dim(col: &mut ChildSpawnerCommands, text: String) {
    col.spawn((Text::new(text), font(ROW_FONT), TextColor(DIM), Node { margin: UiRect::axes(px(2), px(3)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
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
    /// The item's own colour, as a small mark before its name when it has no icon.
    swatch: Option<Color>,
    /// The item's id, for its icon.
    icon: Option<String>,
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
        Cells { lead: Lead::Nothing, swatch: None, icon: None, name: name.into(), note: String::new(), kind: String::new(), qty: String::new(), kg: kg.into(), faint: false }
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
    let mut e = col.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(CELL_GAP), padding: UiRect::axes(px(6), px(3)), border_radius: BorderRadius::all(px(4)), flex_shrink: 0.0, ..default() }, BackgroundColor(bg)));
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
        // Its icon (one of the stacks under an opened row: none of its own), else its colour.
        let own = cells.lead != Lead::Child;
        match cells.icon.as_deref().filter(|_| own).and_then(|id| flat::icon(id, Color::WHITE)) {
            Some(image) => {
                r.spawn((Node { width: px(W_ICON), height: px(W_ICON), flex_shrink: 0.0, ..default() }, image, Pickable::IGNORE));
            }
            None => {
                let swatch = cells.swatch.filter(|_| own).unwrap_or(Color::NONE);
                r.spawn((Node { width: px(W_ICON), height: px(W_ICON), flex_shrink: 0.0, justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }, Pickable::IGNORE)).with_children(|b| {
                    b.spawn((Node { width: px(10), height: px(10), border_radius: BorderRadius::all(px(2)), ..default() }, BackgroundColor(swatch), Pickable::IGNORE));
                });
            }
        }
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
    let (stack, n) = (action.stack, 1 + action.more.len());
    e.observe(move |ev: On<PointerOver>, mut q: Query<&mut BackgroundColor>, mut hovered: Option<ResMut<HoveredItem>>, mut card: Option<ResMut<HoverCard>>| {
        if let Ok(mut bg) = q.get_mut(ev.entity) {
            bg.0 = ROW_HOVER;
        }
        if let Some(h) = hovered.as_mut() {
            h.0 = item.map(|i| (ev.entity, i));
        }
        if let Some(c) = card.as_mut() {
            c.0 = item.map(|_| (ev.entity, stack, n));
        }
    })
    .observe(move |ev: On<PointerOut>, mut q: Query<&mut BackgroundColor>, mut hovered: Option<ResMut<HoveredItem>>, mut card: Option<ResMut<HoverCard>>| {
        if let Ok(mut bg) = q.get_mut(ev.entity) {
            bg.0 = rest;
        }
        if let Some(h) = hovered.as_mut().filter(|h| h.0.is_some_and(|(e, _)| e == ev.entity)) {
            h.0 = None;
        }
        if let Some(c) = card.as_mut().filter(|c| c.0.is_some_and(|(e, ..)| e == ev.entity)) {
            c.0 = None;
        }
    })
    .observe(move |ev: On<PointerPress>, keys: Res<ButtonInput<KeyCode>>, mut clicks: ResMut<Clicks>, mut picked: Option<ResMut<Picked>>, listed: Option<Res<Listed>>, mut drag: Option<ResMut<Drag>>, windows: Query<&Window, With<bevy::window::PrimaryWindow>>| {
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
        let click = RowClick { action: action.clone(), button: ev.button, one };
        // The left button on a thing is held ([`drag`]): let go where it was pressed it is the
        // click, moved first it is dragged.
        if let (PointerButton::Primary, Some(_), None, Some(drag)) = (ev.button, action.left, action.craft, drag.as_mut()) {
            drag.0 = Some(Held { click, from: windows.single().ok().and_then(|w| w.cursor_position()).unwrap_or_default(), dragging: false });
            return;
        }
        clicks.0.push(click);
    });
}

// ---- Dragging things ----

/// Somewhere a dragged thing can be let go.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
enum DropTo {
    Place(Place),
    /// The gear slots: wherever the thing belongs ([`belongs`]).
    Gear,
    /// A slot of the hotbar: the thing's kind goes on that key.
    Hotbar(usize),
}

/// The left button held down on a thing.
struct Held {
    click: RowClick,
    /// Where the pointer was when it went down (window pixels).
    from: Vec2,
    /// It has moved far enough to be a drag.
    dragging: bool,
}

/// The thing the left button is held on, if any.
#[derive(Resource, Default)]
struct Drag(Option<Held>);

/// The picture of the dragged thing, under the pointer.
#[derive(Component)]
struct Ghost;

/// A press that moves less than this (window pixels) before it is let go is a click.
const DRAG_PX: f32 = 6.0;

/// Where a thing goes when it is dropped on the gear slots: clothing is worn, a bag goes where
/// it is carried, anything else into the hand.
fn belongs(def: &ItemDef) -> Place {
    use zombye_sim::items::BagCarry;
    match (def.category, def.bag.as_ref().map(|b| b.carried)) {
        (ItemCategory::Clothing, _) => Place::Worn,
        (_, Some(BagCarry::Back)) => Place::Slot(Slot::Back),
        (_, Some(BagCarry::Shoulder)) => Place::Slot(Slot::Shoulder),
        (_, Some(_)) => Place::Slot(Slot::OffHand),
        _ => Place::Slot(Slot::Hand),
    }
}

/// The intents that dropping the thing of `action` on `to` sends: every stack its tile stands
/// for (just the first onto a slot or the body, which take one thing; `one`: one of the first),
/// from the back first so the indices of the ones still to go don't shift. Nothing for a stack
/// that is already there, or already on its way.
fn drop_intents(action: &RowAction, to: Place, one: bool, player: u32, flight: &mut InFlight, now: f32) -> Vec<Intent> {
    let Some((from, index, _)) = action.left else { return Vec::new() };
    let single = matches!(to, Place::Slot(_) | Place::Worn);
    let mut lines = vec![Line { from, index, stack: action.stack }];
    if !single && !one {
        lines.extend(action.more.iter().copied());
    }
    lines.sort_by_key(|l| std::cmp::Reverse(l.index));
    let count = if single || one { 1 } else { 0 };
    let there = |l: &Line| matches!((l.from, to), (Place::Carried, Place::Pouch(p)) if l.stack.home == p);
    lines.into_iter().filter(|l| !there(l)).filter_map(|l| move_intent(player, l.from, l.index, to, count).filter(|_| flight.ask(l.from, l.index, l.stack, count == 1, now))).collect()
}

/// The drop target under `cursor` (physical pixels): the smallest one it is in.
fn drop_under<'a>(targets: impl Iterator<Item = (Entity, &'a DropTo, &'a ComputedNode, &'a UiGlobalTransform)>, cursor: Vec2) -> Option<(Entity, DropTo)> {
    targets.filter(|(_, _, node, at)| node.contains_point(**at, cursor)).min_by(|a, b| (a.2.size.x * a.2.size.y).total_cmp(&(b.2.size.x * b.2.size.y))).map(|(e, to, ..)| (e, *to))
}

/// The held thing: let go where it was pressed, the click it always was ([`Clicks`]); moved
/// first, its picture follows the pointer, the place under it is outlined, and letting go there
/// moves it: a bag's or the pockets' pane, a hand, the gear slots (worn, or where a bag is
/// carried), the open container, the floor, a hotbar slot. Dragging one of several picked
/// stacks to a pane moves them all. Esc or the right button calls it off.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn drag(
    mut commands: Commands,
    time: Res<Time>,
    (mouse, keys): (Option<Res<ButtonInput<MouseButton>>>, Res<ButtonInput<KeyCode>>),
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    (ui, join, me): (Option<Res<UiScale>>, Option<Res<JoinInfo>>, Option<Res<LocalPlayer>>),
    (panel, menu, picked): (Option<Res<InventoryPanel>>, Option<Res<ItemMenu>>, Option<Res<Picked>>),
    (mut held, mut clicks, mut flight, mut out): (ResMut<Drag>, ResMut<Clicks>, ResMut<InFlight>, ResMut<Outbox>),
    (mut send, mut bar, mut card): (Option<ResMut<SendPicked>>, Option<ResMut<crate::hotbar::SlotClicks>>, Option<ResMut<HoverCard>>),
    targets: Query<(Entity, &DropTo, &ComputedNode, &UiGlobalTransform)>,
    mut ghosts: Query<(Entity, &mut Node), With<Ghost>>,
    mut marked: Local<Option<Entity>>,
) {
    let mut unmark = |commands: &mut Commands, now: Option<Entity>| {
        if *marked == now {
            return;
        }
        if let Some(mut e) = marked.and_then(|e| commands.get_entity(e).ok()) {
            e.try_remove::<Outline>();
        }
        if let Some(mut e) = now.and_then(|e| commands.get_entity(e).ok()) {
            e.try_insert(Outline::new(px(2), px(2), flat::GREEN));
        }
        *marked = now;
    };
    let Some(h) = held.0.as_mut() else {
        unmark(&mut commands, None);
        for (e, _) in &ghosts {
            commands.entity(e).despawn();
        }
        return;
    };
    let window = windows.single().ok();
    let cursor = window.and_then(|w| w.cursor_position());
    let down = mouse.as_ref().is_some_and(|m| m.pressed(MouseButton::Left));
    let off = keys.just_pressed(KeyCode::Escape) || mouse.as_ref().is_some_and(|m| m.just_pressed(MouseButton::Right)) || menu.as_ref().is_some_and(|m| m.is_up()) || panel.as_ref().is_some_and(|p| !p.open);
    if off && (h.dragging || down) {
        held.0 = None;
        return;
    }
    if let Some(c) = cursor
        && !h.dragging
        && down
        && c.distance(h.from) > DRAG_PX
    {
        h.dragging = true;
        let def = join.as_ref().and_then(|j| j.items.get(h.click.action.stack.item as usize));
        let n: u32 = h.click.action.stack.count as u32 + h.click.action.more.iter().map(|l| l.stack.count as u32).sum::<u32>();
        commands.spawn((Ghost, GlobalZIndex(30), Node { position_type: PositionType::Absolute, left: px(-200), top: px(-200), width: px(64), height: px(64), justify_content: JustifyContent::Center, align_items: AlignItems::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(flat::RADIUS)), ..default() }, BackgroundColor(flat::SOLID), BorderColor::all(flat::GREEN), Pickable::IGNORE)).with_children(|g| {
            flat::item_icon(g, def, 44.0, 1.0);
            if n > 1 {
                g.spawn((Text::new(format!("x{n}")), theme::strong(flat::SMALL), TextColor(TEXT), TextLayout::no_wrap(), Node { position_type: PositionType::Absolute, right: px(5), top: px(2), ..default() }, Pickable::IGNORE));
            }
        });
    }
    let over = if h.dragging { window.and_then(|w| w.physical_cursor_position()).and_then(|c| drop_under(targets.iter(), c)) } else { None };
    if h.dragging {
        if let Some(c) = card.as_mut().filter(|c| c.0.is_some()) {
            c.0 = None;
        }
        let scale = ui.as_ref().map_or(1.0, |u| u.0.max(0.1));
        if let (Some(c), Ok((_, mut node))) = (cursor, ghosts.single_mut()) {
            let at = c / scale - Vec2::splat(32.0);
            if node.left != px(at.x) || node.top != px(at.y) {
                (node.left, node.top) = (px(at.x), px(at.y));
            }
        }
        unmark(&mut commands, over.map(|(e, _)| e));
    }
    if down {
        return;
    }
    let Some(h) = held.0.take() else { return };
    if !h.dragging {
        clicks.0.push(h.click);
        return;
    }
    let Some((_, to)) = over else { return };
    let a = &h.click.action;
    let place = match to {
        DropTo::Place(p) => p,
        DropTo::Gear => match join.as_ref().and_then(|j| j.items.get(a.stack.item as usize)) {
            Some(def) => belongs(def),
            None => return,
        },
        DropTo::Hotbar(k) => {
            if let Some(bar) = bar.as_mut() {
                bar.1.push((k, a.stack.item));
            }
            return;
        }
    };
    // One of several picked stacks, dragged to a pane: they all go.
    let key = a.left.map(|(from, index, _)| (from, index, a.stack.item));
    if let (Some(picked), Some(send), false) = (picked.as_ref(), send.as_mut(), matches!(place, Place::Slot(_) | Place::Worn))
        && picked.keys.len() > 1
        && key.is_some_and(|k| picked.keys.contains(&k))
    {
        send.0 = Some(place);
        return;
    }
    let one = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    out.0.extend(drop_intents(a, place, one, me.map_or(0, |m| m.id), &mut flight, time.elapsed_secs()));
}

// ---- The right-click menu (#95) ----

/// What a menu entry does.
#[derive(Clone, Debug)]
enum MenuDo {
    /// Send this intent about the stack at `from`/`index` (`whole`: all of it, else part).
    Send { intent: Intent, from: Place, index: u32, whole: bool },
    /// Open the health panel to pick the wound to use this item on.
    UseOn(u16),
    /// Something in the world (`world_actions`): send this, turning our survivor to `facing`
    /// first if given, as the server will.
    World { intent: Intent, facing: Option<f32> },
    /// Something lying nearby (a container's, the floor's): take what it needs with `take`, and
    /// once it is in our pockets or bags do `then` with it ([`FollowUp`]).
    TakeThen { take: Intent, from: Place, index: u32, item: u16, then: Then },
    /// A line that only says something (too far away); picking it just closes the menu.
    Nothing,
    /// Send this `times` over (make all: the server queues each while there's the stuff for it).
    Many { intent: Intent, times: u16 },
}

/// What to do with a thing taken for it, once it has arrived ([`MenuDo::TakeThen`]).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Then {
    /// Eat, drink or use it.
    Use,
    /// Take the pill.
    Treat(Treatment),
    /// Make this recipe with it.
    Craft(u16),
}

/// The second half of a [`MenuDo::TakeThen`]: what was asked for, waiting for the thing to show
/// in what we carry (a few seconds at most; a refused take just lets it lapse).
#[derive(Resource, Default)]
struct FollowUp(Option<Follow>);

struct Follow {
    item: u16,
    then: Then,
    /// How many of the item we carried when it was asked.
    had: u32,
    until: f32,
}

/// How many of `item` are carried (pockets and bags).
fn carried_count(inv: &zombye_sim::Inventory, item: u16) -> u32 {
    inv.carried.iter().filter(|s| s.item == item).map(|s| s.count as u32).sum()
}

/// Send the second half of a take-then once the thing has arrived: the use, the pill, or the craft
/// (a craft once the server's own rule says it can be made).
fn follow_up(time: Res<Time>, link: Res<ServerLink>, join: Res<JoinInfo>, me: Res<LocalPlayer>, mut follow: ResMut<FollowUp>, mut out: ResMut<Outbox>) {
    let Some(f) = &follow.0 else { return };
    let (Some(v), _) = panel_view(&link, me.id) else { return };
    if time.elapsed_secs() > f.until || me.dead {
        follow.0 = None;
        return;
    }
    if carried_count(&v.inventory, f.item) <= f.had {
        return;
    }
    let Some(index) = v.inventory.carried.iter().rposition(|s| s.item == f.item) else { return };
    let (player, index) = (me.id, index as u32);
    match f.then {
        Then::Use => out.0.push(Intent::Use { player, from: Place::Carried, index }),
        Then::Treat(action) => out.0.push(Intent::Treat { player, wound: None, action, item: Some(f.item) }),
        Then::Craft(recipe) => {
            if join.recipes.get(recipe as usize).is_none_or(|r| crafting::check(r, &v.inventory, &join.items).is_err()) {
                return;
            }
            out.0.push(Intent::Craft { player, recipe });
        }
    }
    follow.0 = None;
}

/// The recipes a stack lying nearby would let us make once taken, that we can't make without it:
/// (recipe, its name, how many of the stack to take, how many times over it could be made: "make all").
/// A recipe that can be made now with what lies in reach (`near`: ours and the floor's) comes
/// with 0 to take: it is made straight away, where it lies.
fn makes_with(recipes: &[crafting::RecipeDef], items: &[ItemDef], inv: &zombye_sim::Inventory, near: &zombye_sim::Inventory, s: &ItemStack) -> Vec<(u16, String, u16, u16)> {
    let Some(def) = items.get(s.item as usize) else { return Vec::new() };
    let mut out = Vec::new();
    for (k, r) in recipes.iter().enumerate() {
        let Some(need) = r.inputs.iter().find(|x| x.item == def.id).map(|x| x.count as u32) else { continue };
        if crafting::check(r, near, items).is_ok() && crafting::check(r, inv, items).is_err() {
            out.push((k as u16, r.name.clone(), 0, crafting::times_can(r, near, items, ALL_MAX) as u16));
            continue;
        }
        let have: u32 = inv.carried.iter().filter(|c| c.item == s.item).map(|c| crafting::whole(c) as u32).sum();
        if have >= need || (crafting::whole(s) as u32) < need - have {
            continue;
        }
        let take = (need - have) as u16;
        let mut with = inv.clone();
        with.carried.push(ItemStack { count: take, used: 0, ..*s });
        if crafting::check(r, &with, items).is_ok() {
            out.push((k as u16, r.name.clone(), take, 1));
        }
    }
    out
}

/// The recipes one of our own things goes into that we can make now with what we have and what
/// lies in reach (`near`): opening a box of nails, ripping a shirt into rags, mending a bat. Each is
/// made where it is (0 to take). Recipes that need heat or running water are left to the Make list
/// and the fire's and the sink's own menus, which know whether there is any.
/// The most "make all" queues at once.
const ALL_MAX: u32 = 16;

fn makes_from(recipes: &[crafting::RecipeDef], items: &[ItemDef], near: &zombye_sim::Inventory, s: &ItemStack) -> Vec<(u16, String, u16, u16)> {
    let Some(def) = items.get(s.item as usize) else { return Vec::new() };
    recipes
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.heat && !r.water && !r.cistern && r.places.is_none())
        .filter(|(_, r)| r.inputs.iter().any(|x| x.item == def.id) || r.repairs.as_ref().is_some_and(|m| def.has_tag(&m.tag)))
        .filter(|(_, r)| crafting::check(r, near, items).is_ok())
        .map(|(k, r)| (k as u16, r.name.clone(), 0, crafting::times_can(r, near, items, ALL_MAX) as u16))
        .collect()
}

/// One line of the menu.
#[derive(Clone, Debug)]
struct MenuEntry {
    label: String,
    act: MenuDo,
}

/// Everything that can be done with the stack `s` at `from`/`index`, the likeliest first: one
/// level, no sub-menus. `open` is the container the player has open (id, name), `pouches` the
/// pockets and bags carried (a carried thing can go from one to another).
fn menu_entries(player: u32, items: &[ItemDef], from: Place, index: u32, s: &ItemStack, open: Option<(u32, &str)>, pouches: &[(Pouch, String)], makes: &[(u16, String, u16, u16)]) -> Vec<MenuEntry> {
    let Some(def) = items.get(s.item as usize) else { return Vec::new() };
    let mut out = Vec::new();
    let mk = |label: String, intent: Option<Intent>, whole: bool| intent.map(|intent| MenuEntry { label, act: MenuDo::Send { intent, from, index, whole } });
    let n = s.count;
    // Half of 2 or 3 is one, which "one" already offers.
    let half = n / 2;
    let halves = half > 1;
    let into = |to: Place, count: u16| move_intent(player, from, index, to, count);
    // A roller blind: fit it to the window or glass door faced (the server checks reach and side).
    if matches!(from, Place::Carried | Place::Slot(_)) && def.has_tag(&zombye_sim::BlindTuning::default().tag) {
        out.extend(mk("Fit to the window".into(), Some(Intent::Blind { player, op: zombye_sim::BlindOp::Fit }), false));
    }
    // The recipes it goes into that can be made now (opening a box of nails, ripping a shirt, mending).
    let crafts = |out: &mut Vec<MenuEntry>| {
        for (recipe, name, _, all) in makes {
            out.push(MenuEntry { label: name.clone(), act: MenuDo::Send { intent: Intent::Craft { player, recipe: *recipe }, from, index, whole: false } });
            if *all > 1 {
                out.push(MenuEntry { label: format!("{name}: all {all}"), act: MenuDo::Many { intent: Intent::Craft { player, recipe: *recipe }, times: *all } });
            }
        }
    };
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
            crafts(&mut out);
            if let Some((id, name)) = open {
                out.extend(mk(format!("Put in {name}"), into(Place::Container(id), 0), true));
            }
            // Into another of the bags carried (or the pockets).
            for (p, name) in pouches.iter().filter(|(p, _)| *p != s.home) {
                out.extend(mk(format!("Put in {name}"), into(Place::Pouch(*p), 0), true));
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
            crafts(&mut out);
            if let Some((id, name)) = open {
                out.extend(mk(format!("Put in {name}"), into(Place::Container(id), 0), true));
            }
            out.extend(mk("Drop".into(), into(Place::Ground, 0), true));
        }
        // (Only somewhere things go to.)
        Place::Pouch(_) => {}
        Place::Container(_) | Place::Ground => {
            let take = if from == Place::Ground { "Pick up" } else { "Take" };
            // What can be done with it straight from where it lies: one is taken, then used
            // (worn and held things go straight to their place).
            let then = |label: String, then: Then, count: u16| into(Place::Carried, count).map(|take| MenuEntry { label, act: MenuDo::TakeThen { take, from, index, item: s.item, then } });
            match (Treatment::of_item(def), def.category) {
                (Some(t), _) if !t.on_wound() => out.extend(then(format!("Take {}", def.name.to_lowercase()), Then::Treat(t), 1)),
                (Some(_), _) => {}
                (None, ItemCategory::Food) => out.extend(then("Eat".into(), Then::Use, 1)),
                (None, ItemCategory::Drink) => out.extend(then("Drink".into(), Then::Use, 1)),
                (None, _) if def.usable() => out.extend(then("Use".into(), Then::Use, 1)),
                (None, ItemCategory::Clothing) => out.extend(mk("Wear".into(), into(Place::Worn, 1), false)),
                (None, ItemCategory::Bag) => {
                    let slot = match def.bag.as_ref().map(|b| b.carried) {
                        Some(zombye_sim::items::BagCarry::Back) => Slot::Back,
                        Some(zombye_sim::items::BagCarry::Shoulder) => Slot::Shoulder,
                        _ => Slot::OffHand,
                    };
                    out.extend(mk("Put it on".into(), into(Place::Slot(slot), 1), false));
                }
                (None, _) => {}
            }
            if def.weapon.is_some() {
                out.extend(mk("Take in hand".into(), into(Place::Slot(Slot::Hand), 0), true));
            }
            for (recipe, name, count, all) in makes {
                if *count == 0 {
                    out.push(MenuEntry { label: name.clone(), act: MenuDo::Send { intent: Intent::Craft { player, recipe: *recipe }, from, index, whole: false } });
                    if *all > 1 {
                        out.push(MenuEntry { label: format!("{name}: all {all}"), act: MenuDo::Many { intent: Intent::Craft { player, recipe: *recipe }, times: *all } });
                    }
                } else {
                    out.extend(then(name.clone(), Then::Craft(*recipe), *count));
                }
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
    /// Open the menu of a thing in the world (a door, window, container) at the pointer.
    pub fn show_world(&mut self, title: String, entries: Vec<crate::world_actions::WorldEntry>, at: Vec2) {
        let entries = entries
            .into_iter()
            .map(|e| MenuEntry { label: e.label, act: match e.intent { Some(intent) => MenuDo::World { intent, facing: e.facing }, None => MenuDo::Nothing } })
            .collect();
        self.show(title, ItemStack::one(0), entries, Some(at));
    }

    /// A menu is on screen (its number keys are its own).
    pub fn is_up(&self) -> bool {
        self.open.is_some()
    }
}

/// The item of the inventory row the pointer is over (and the row), for the hotbar: Shift and a
/// number key over a row puts that item on that slot (`hotbar.rs`).
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
pub(crate) fn open_menu(menu: &mut ItemMenu, link: &ServerLink, join: &JoinInfo, me: u32, from: Place, index: u32, at: Option<Vec2>) -> bool {
    let (Some(v), floor) = panel_view(link, me) else { return false };
    let Some(s) = stack_at(&v, &floor, from, index) else { return false };
    let open = v.open.as_ref().map(|c| (c.id, c.name.to_lowercase()));
    let pouches = pouch_names(&join.items, &v.inventory, join.carry.pockets_l);
    let near = stock(&v, &floor);
    let makes = if matches!(from, Place::Container(_) | Place::Ground) { makes_with(&join.recipes, &join.items, &v.inventory, &near, &s) } else { makes_from(&join.recipes, &join.items, &near, &s) };
    let entries = menu_entries(me, &join.items, from, index, &s, open.as_ref().map(|(id, n)| (*id, n.as_str())), &pouches, &makes);
    if entries.is_empty() {
        return false;
    }
    menu.show(name_of(&join.items, Wear::of(join), link.curr.game_minute, &s), s, entries, at);
    true
}

/// Is the pointer on a panel, button or the like? A node over the whole window (an overlay)
/// doesn't count.
fn pointer_on_ui(window: &Window, hover: &bevy::picking::hover::HoverMap, nodes: &Query<&ComputedNode>) -> bool {
    let whole = window.physical_size().as_vec2() * 0.95;
    hover.values().any(|m| m.keys().any(|e| nodes.get(*e).is_ok_and(|n| !(n.size().x >= whole.x && n.size().y >= whole.y))))
}

/// A left click off every panel and button (on the world) while the inventory is open closes it,
/// and is used up doing so: both mouse buttons are reset until they're pressed again, so the
/// click doesn't swing, start aiming (right held), or reach anything else that reads the mouse.
/// Not while an item menu is open (that click closes the menu, `menu_act`), or one just opened.
/// A right click on the world still opens its menu (`world_menu`).
fn click_away(
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    hover: Option<Res<bevy::picking::hover::HoverMap>>,
    nodes: Query<&ComputedNode>,
    menu: Res<ItemMenu>,
    mut panel: ResMut<InventoryPanel>,
) {
    if !panel.open || !mouse.just_pressed(MouseButton::Left) || menu.open.is_some() || menu.fresh {
        return;
    }
    let (Ok(window), Some(hover)) = (windows.single(), hover) else { return };
    if pointer_on_ui(window, &hover, &nodes) {
        return;
    }
    panel.open = false;
    mouse.reset(MouseButton::Left);
    mouse.reset(MouseButton::Right);
}

/// The menu on a ground item in the world: right-click it with the panel open, or tap the right
/// mouse on it with the panel shut (held, it aims). Only items within reach (our floor). The same
/// tap on a door, window, fence, blind or container opens its menu (`world_actions`).
#[allow(clippy::too_many_arguments)]
fn world_menu(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<crate::camera::IsoCamera>>,
    hover: Res<bevy::picking::hover::HoverMap>,
    nodes: Query<&ComputedNode>,
    panel: Res<InventoryPanel>,
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    me: Res<LocalPlayer>,
    map: Res<crate::server_link::ClientMap>,
    mut menu: ResMut<ItemMenu>,
    mut tap: Local<Option<(f32, Vec2)>>,
    mut sites: Local<Option<Vec<zombye_sim::containers::ContainerSite>>>,
    mut switches: Local<Option<Vec<Option<zombye_sim::power::SwitchSpot>>>>,
) {
    let (Ok(window), Ok((camera, cam_tf))) = (windows.single(), cams.single()) else { return };
    let Some(cursor) = window.cursor_position() else { return };
    let now = time.elapsed_secs();
    let mut want = false;
    if mouse.just_pressed(MouseButton::Right) {
        if pointer_on_ui(window, &hover, &nodes) || menu.fresh || me.dead {
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
    // Where the pointer's ray comes down (first person, looking level or up: nowhere near).
    let ground_t = if ray.direction.y < -1e-4 { -ray.origin.y / ray.direction.y } else { 1e6 };
    let hit = ray.origin + *ray.direction * ground_t;
    let ground = Vec2::new(hit.x, hit.z);
    let reach = link.curr.floor_of(me.id);
    let nearest = link.curr.items.iter().filter(|it| reach.contains(&it.id)).map(|it| (it.id, it.pos.distance(ground))).filter(|(_, d)| *d < 0.8).min_by(|a, b| a.1.total_cmp(&b.1));
    // A door, window or container under the pointer, unless an item on the floor is (and nothing
    // stands in front of it).
    let sites = sites.get_or_insert_with(|| zombye_sim::containers::find_sites(&map.0, &join.containers));
    let thing = crate::world_actions::pick(&map.0, sites, me.curr.pos, ray.origin, *ray.direction);
    // A body lying in front of a door or cupboard is what the pointer is on.
    let bodies = link.curr.zombies.iter().filter(|z| z.anim == zombye_sim::ZombieAnim::Dead).map(|z| (z.id, Vec2::from(z.pos)));
    let body = crate::world_actions::pick_body(bodies, me.curr.pos, ray.origin, *ray.direction);
    let thing = match (thing, body) {
        (Some(a), Some(b)) => Some(if b.1 < a.1 { b } else { a }),
        (a, b) => a.or(b),
    };
    let switches = switches.get_or_insert_with(|| zombye_sim::power::switch_spots(&map.0));
    let switch = crate::world_actions::pick_switch(switches, me.curr.pos, ray.origin, *ray.direction);
    let thing = match (thing, switch) {
        (Some(a), Some(b)) => Some(if b.1 < a.1 { b } else { a }),
        (a, b) => a.or(b),
    };
    let fire = crate::world_actions::pick_fire(&link.curr.fires, me.curr.pos, ray.origin, *ray.direction);
    let thing = match (thing, fire) {
        (Some(a), Some(b)) => Some(if b.1 < a.1 { b } else { a }),
        (a, b) => a.or(b),
    };
    let tap = crate::world_actions::pick_tap(&map.0, sites, &join.water, me.curr.pos, me.curr.level, ray.origin, *ray.direction);
    let thing = match (thing, tap) {
        (Some(a), Some(b)) => Some(if b.1 < a.1 { b } else { a }),
        (a, b) => a.or(b),
    };
    let thing = thing.filter(|(_, t)| nearest.is_none() || *t < ground_t - 0.3);
    if let Some((target, _)) = thing {
        let (title, mut entries) = crate::world_actions::entries(target, &map.0, sites, &join.containers, &join.items, &join.builds, &join.blind_tag, join.fire.reach, me.id, me.curr.pos, link.own(me.id));
        // A sink, a bath or a toilet: fill a bottle there too (running water, or once it's off a
        // toilet's cistern).
        let at = match target {
            crate::world_actions::Target::Container(k) => sites.get(k).map(|s| (s.min, s.level as i8)),
            crate::world_actions::Target::Tap(min, floor, _) => Some((min, floor)),
            _ => None,
        };
        if let Some((min, floor)) = at {
            let fills = crate::world_actions::tap_entries(min, floor, &map.0, &join.water, &link.curr, &join.recipes, &join.items, me.id, me.curr.pos, me.curr.level, link.own(me.id));
            if fills.is_empty() && matches!(target, crate::world_actions::Target::Tap(..)) {
                let cistern = join.water.cisterns.kinds.iter().any(|k| map.0.buildings.iter().flat_map(|b| b.furniture.iter()).any(|f| f.min == min && f.level == floor && f.kind == *k));
                entries.push(crate::world_actions::WorldEntry::say(match (link.curr.water_on, cistern) {
                    (true, _) => "Too far away: walk up to it",
                    (false, true) => "The cistern is empty (or too far away)",
                    (false, false) => "No water: the taps have run dry",
                }));
            }
            entries.extend(fills);
        }
        if !entries.is_empty() {
            menu.show_world(title, entries, cursor);
        }
    } else if let Some((id, _)) = nearest {
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
fn menu_act(time: Res<Time>, mouse: Res<ButtonInput<MouseButton>>, mut me: ResMut<LocalPlayer>, panel: Res<InventoryPanel>, mut panel_was: Local<bool>, mut menu: ResMut<ItemMenu>, mut flight: ResMut<InFlight>, mut out: ResMut<Outbox>, mut health: ResMut<crate::health_ui::HealthPanel>, link: Res<ServerLink>, mut follow: ResMut<FollowUp>) {
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
            MenuDo::World { intent, facing } => {
                // Turn as the server turns us, so the inputs that follow keep that facing.
                if let Some(f) = facing {
                    me.curr.facing = f;
                    me.aim = f;
                }
                out.0.push(intent);
            }
            MenuDo::Nothing => {}
            MenuDo::TakeThen { take, from, index, item, then } => {
                if flight.ask(from, index, stack, true, time.elapsed_secs()) {
                    out.0.push(take);
                    let had = panel_view(&link, me.id).0.map_or(0, |v| carried_count(&v.inventory, item));
                    follow.0 = Some(Follow { item, then, had, until: time.elapsed_secs() + 4.0 });
                }
            }
            MenuDo::Send { intent, from, index, whole } => {
                if flight.ask(from, index, stack, !whole, time.elapsed_secs()) {
                    out.0.push(intent);
                }
            }
            MenuDo::Many { intent, times } => {
                out.0.extend((0..times).map(|_| intent.clone()));
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
    Vec2::new(300.0, 52.0 + 29.0 * entries as f32)
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
    // A floating piece of the flat skin: solid ink, a hairline round it.
    let node = Node { position_type: PositionType::Absolute, flex_direction: FlexDirection::Column, row_gap: px(1), padding: UiRect::all(px(8)), min_width: px(200), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(8)), ..default() };
    let node = match at {
        Some(at) => Node { left: px(at.x), top: px(at.y), ..node },
        None => Node { left: percent(40), top: px(190), ..node },
    };
    commands
        .spawn((MenuRoot, node, GlobalZIndex(20), BackgroundColor(flat::SOLID), BorderColor::all(Color::srgba(0.949, 0.902, 0.788, 0.25))))
        .with_children(|c| {
            c.spawn((Text::new(m.title.to_uppercase()), theme::head(flat::BODY), TextColor(DIM), TextLayout::no_wrap(), Node { padding: UiRect::axes(px(8), px(2)), ..default() }));
            c.spawn((Node { height: px(1), margin: UiRect::axes(px(0), px(4)), ..default() }, BackgroundColor(LINE), Pickable::IGNORE));
            for (k, e) in m.entries.iter().enumerate() {
                let rest = if k == 0 { ROW_PICKED } else { Color::NONE };
                c.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(10), padding: UiRect::axes(px(8), px(5)), border_radius: BorderRadius::all(px(4)), ..default() }, BackgroundColor(rest)))
                    .with_children(|r| {
                        r.spawn((Text::new(format!("{}", k + 1)), theme::strong(flat::SMALL), TextColor(OK), Node { width: px(12), ..default() }, Pickable::IGNORE));
                        let colour = if matches!(e.act, MenuDo::Nothing) { DIM } else { TEXT };
                        r.spawn((Text::new(e.label.clone()), font(flat::BODY), TextColor(colour), TextLayout::no_wrap(), Pickable::IGNORE));
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
    }

    /// A left click on the world (off every panel) with the inventory open shuts it and is used
    /// up: no button is down afterwards, so nothing behind it swings or aims. A click on the panel
    /// leaves it open, and with an item menu up the click is the menu's.
    #[test]
    fn a_click_off_the_panel_shuts_it_and_goes_no_further() {
        let mut app = ui_app();
        app.insert_resource(InventoryPanel { open: true }).init_resource::<ItemMenu>();
        app.add_systems(PreUpdate, click_away.after(bevy::input::InputSystems).after(bevy::picking::PickingSystems::Hover));
        // Whatever reads the mouse after it (as the swing and aim do in `Update`).
        #[derive(Resource, Default)]
        struct Seen(u32);
        app.init_resource::<Seen>().add_systems(Update, |mouse: Res<ButtonInput<MouseButton>>, mut seen: ResMut<Seen>| {
            if mouse.just_pressed(MouseButton::Left) || mouse.pressed(MouseButton::Left) || mouse.pressed(MouseButton::Right) {
                seen.0 += 1;
            }
        });
        app.world_mut().spawn(Camera2d);
        app.world_mut().spawn((Node { position_type: PositionType::Absolute, top: px(0), left: px(0), width: px(300), height: px(200), ..default() }, BackgroundColor(Color::BLACK)));
        for _ in 0..3 {
            app.update();
        }
        let window = app.world_mut().query_filtered::<Entity, With<PrimaryWindow>>().single(app.world()).unwrap();
        let press = |app: &mut App, at: Vec2, buttons: &[MouseButton], state: ButtonState| {
            let moved = CursorMoved { window, position: at, delta: None };
            app.world_mut().write_message(moved.clone());
            app.world_mut().write_message(WindowEvent::CursorMoved(moved));
            app.update();
            for &button in buttons {
                let e = MouseButtonInput { button, state, window };
                app.world_mut().write_message(e);
                app.world_mut().write_message(WindowEvent::MouseButtonInput(e));
            }
            app.update();
        };
        let open = |app: &App| app.world().resource::<InventoryPanel>().open;
        let seen = |app: &App| app.world().resource::<Seen>().0;
        // On the panel: it stays open, and the click is the panel's.
        press(&mut app, Vec2::new(100.0, 100.0), &[MouseButton::Left], ButtonState::Pressed);
        assert!(open(&app), "a click on the panel keeps it open");
        press(&mut app, Vec2::new(100.0, 100.0), &[MouseButton::Left], ButtonState::Released);
        // An item menu up: the click off the panel only closes the menu (`menu_act`).
        app.world_mut().resource_mut::<ItemMenu>().open = Some(MenuState { title: String::new(), stack: beans(1), entries: Vec::new(), at: None });
        press(&mut app, Vec2::new(600.0, 400.0), &[MouseButton::Left], ButtonState::Pressed);
        assert!(open(&app), "with a menu up the click is the menu's");
        press(&mut app, Vec2::new(600.0, 400.0), &[MouseButton::Left], ButtonState::Released);
        app.world_mut().resource_mut::<ItemMenu>().open = None;
        // Off the panel, the right button held (as if to aim): shut, and nothing behind it saw a button.
        app.world_mut().resource_mut::<Seen>().0 = 0;
        press(&mut app, Vec2::new(600.0, 400.0), &[MouseButton::Right, MouseButton::Left], ButtonState::Pressed);
        assert!(!open(&app), "a click on the world shuts the panel");
        assert_eq!(seen(&app), 0, "and is used up: no swing, no aim");
        app.update();
        assert_eq!(seen(&app), 0, "the buttons stay up while held");
        press(&mut app, Vec2::new(600.0, 400.0), &[MouseButton::Right, MouseButton::Left], ButtonState::Released);
        // Shut, the next click is the game's again.
        press(&mut app, Vec2::new(600.0, 400.0), &[MouseButton::Left], ButtonState::Pressed);
        assert!(seen(&app) > 0, "a later click reaches the game");
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
        let labels = |from: Place, s: ItemStack, open: Option<(u32, &str)>| menu_entries(1, items, from, 2, &s, open, &[], &[]).into_iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(labels(Place::Carried, stack("tinned_beans", 3), Some((7, "fridge"))), ["Eat", "Put in fridge", "Drop", "Put one in fridge", "Drop one"], "half of three is one: not offered twice");
        assert_eq!(labels(Place::Carried, stack("tinned_beans", 4), Some((7, "fridge"))), ["Eat", "Put in fridge", "Drop", "Put one in fridge", "Put half in fridge (2)", "Drop one", "Drop half (2)"]);
        assert_eq!(labels(Place::Ground, stack("tinned_beans", 2), Some((7, "fridge"))), ["Eat", "Pick up", "Put in fridge", "Pick up one"]);
        assert!(menu_entries(1, items, Place::Carried, 0, &stack("tinned_beans", 60), Some((7, "fridge")), &[], &[]).len() <= DIGITS.len(), "every entry has a number key");
        assert_eq!(labels(Place::Carried, stack("kitchen_knife", 1), None), ["Hold in hand", "Drop"]);
        // A thing a mod gives a use to (#25): "Use", not "Hold in hand". Without the mod, as before.
        assert_eq!(labels(Place::Carried, stack("alarm_clock", 1), None), ["Hold in hand", "Drop"]);
        let mut modded = config.clone();
        zombye_mods::load_into(&mut modded, &[data.join("../../mods/alarm_clock")]).unwrap();
        let clock = ItemStack::new(modded.items.iter().position(|d| d.id == "alarm_clock").unwrap() as u16, 1);
        assert_eq!(menu_entries(1, &modded.items, Place::Carried, 0, &clock, None, &[], &[]).into_iter().map(|e| e.label).collect::<Vec<_>>(), ["Use", "Drop"]);
        assert_eq!(labels(Place::Carried, stack("rag", 1), None)[0], "Use on a wound...");
        assert_eq!(labels(Place::Worn, stack("rag", 1), None), ["Take off", "Drop"]);
        assert_eq!(labels(Place::Ground, stack("kitchen_knife", 1), None), ["Take in hand", "Pick up"]);
        assert_eq!(labels(Place::Container(7), stack("tinned_beans", 4), None), ["Eat", "Take", "Take one", "Take half (2)"]);
        // Things lying nearby can be used straight away: worn and slung go to their place, food
        // and a recipe's input are taken (one, or what the recipe needs) and then used.
        assert_eq!(labels(Place::Container(7), stack("hoodie", 1), None), ["Wear", "Take"]);
        assert_eq!(labels(Place::Ground, stack("hiking_pack", 1), None), ["Put it on", "Pick up"]);
        let e = menu_entries(1, items, Place::Container(7), 2, &stack("hoodie", 1), None, &[], &[]);
        assert!(matches!(e[0].act, MenuDo::Send { intent: Intent::MoveItem { from: Place::Container(7), index: 2, to: Place::Worn, count: 1, .. }, .. }));
        let e = menu_entries(1, items, Place::Ground, 9, &stack("tinned_beans", 3), None, &[], &[]);
        assert!(matches!(e[0].act, MenuDo::TakeThen { take: Intent::MoveItem { from: Place::Ground, index: 9, to: Place::Carried, count: 1, .. }, then: Then::Use, .. }));
        let tee = stack("t_shirt", 1);
        let makes = makes_with(&config.recipes, items, &zombye_sim::Inventory::default(), &zombye_sim::Inventory::default(), &tee);
        assert_eq!(makes.len(), 1, "ripping it into rags needs nothing else: {makes:?}");
        let e = menu_entries(1, items, Place::Ground, 4, &tee, None, &[], &makes);
        assert_eq!(e.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Wear", makes[0].1.as_str(), "Pick up"]);
        assert!(matches!(e[1].act, MenuDo::TakeThen { then: Then::Craft(_), .. }));
        // Jeans need something that cuts: not offered without it; with one already carried, no need to take another.
        assert!(makes_with(&config.recipes, items, &zombye_sim::Inventory::default(), &zombye_sim::Inventory::default(), &stack("jeans", 1)).is_empty());
        let mut inv = zombye_sim::Inventory::default();
        inv.carried.push(tee);
        assert!(makes_with(&config.recipes, items, &inv, &inv, &tee).is_empty());
        // Our own things offer what they make, made where they are: a box of nails opened.
        let nails = stack("nails", 1);
        let mut inv = zombye_sim::Inventory::default();
        inv.carried.push(nails);
        let makes = makes_from(&config.recipes, items, &inv, &nails);
        assert_eq!(makes.iter().map(|m| m.1.as_str()).collect::<Vec<_>>(), ["Open a box of nails"]);
        let e = menu_entries(1, items, Place::Carried, 0, &nails, None, &[], &makes);
        assert_eq!(e[1].label, "Open a box of nails");
        assert!(matches!(e[1].act, MenuDo::Send { intent: Intent::Craft { .. }, .. }));
        // Several: make all, as many as there are.
        let three = stack("nails", 3);
        let mut inv = zombye_sim::Inventory::default();
        inv.carried.push(three);
        let makes = makes_from(&config.recipes, items, &inv, &three);
        assert_eq!(makes[0].3, 3);
        let e = menu_entries(1, items, Place::Carried, 0, &three, None, &[], &makes);
        assert_eq!(e[2].label, "Open a box of nails: all 3");
        assert!(matches!(e[2].act, MenuDo::Many { intent: Intent::Craft { .. }, times: 3 }));
        // The intents are the ones a drag or a click would send.
        let e = menu_entries(1, items, Place::Carried, 2, &stack("tinned_beans", 4), None, &[], &[]);
        assert!(matches!(e[0].act, MenuDo::Send { intent: Intent::Use { index: 2, .. }, whole: false, .. }));
        assert!(matches!(e[2].act, MenuDo::Send { intent: Intent::Drop { index: 2, count: 1, .. }, whole: false, .. }));
        assert!(matches!(e[3].act, MenuDo::Send { intent: Intent::Drop { index: 2, count: 2, .. }, .. }));
        let e = menu_entries(1, items, Place::Ground, 9, &stack("kitchen_knife", 1), None, &[], &[]);
        assert!(matches!(e[0].act, MenuDo::Send { intent: Intent::MoveItem { from: Place::Ground, index: 9, to: Place::Slot(Slot::Hand), .. }, .. }));
    }

    /// With bags on, a carried stack's menu offers each other place it could be kept (never the
    /// one it is in), as a move into that bag; the sort control goes round its four orders.
    #[test]
    fn the_menu_offers_the_other_bags_and_the_sort_goes_round() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let items = zombye_net::content::load_config(&data, 1).unwrap().items;
        let mut beans = ItemStack::new(items.iter().position(|d| d.id == "tinned_beans").unwrap() as u16, 2);
        beans.home = Pouch::Back;
        let pouches = [(Pouch::Pockets, "pockets".to_string()), (Pouch::Back, "rucksack".to_string()), (Pouch::Shoulder, "tote bag".to_string())];
        let e = menu_entries(1, &items, Place::Carried, 3, &beans, None, &pouches, &[]);
        assert_eq!(e.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["Eat", "Put in pockets", "Put in tote bag", "Drop", "Drop one"]);
        assert!(matches!(e[2].act, MenuDo::Send { intent: Intent::MoveItem { from: Place::Carried, index: 3, to: Place::Pouch(Pouch::Shoulder), count: 0, .. }, whole: true, .. }));
        // Things on the floor or in a cupboard go where the server puts them: no bag entries.
        assert!(menu_entries(1, &items, Place::Ground, 3, &beans, None, &pouches, &[]).iter().all(|e| !e.label.starts_with("Put in")));
        let mut sort = Sort::default();
        let mut seen = Vec::new();
        for _ in 0..4 {
            sort.next();
            seen.push(sort.name());
        }
        assert_eq!(seen, ["name", "type", "weight", "as packed"]);
    }

    /// A right-click on a row opens its menu; with a menu up, a left click on a row behind it
    /// moves nothing (it only closes the menu), and a right-click on another row opens that one's.
    #[test]
    fn a_click_behind_an_open_menu_moves_nothing() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let items = zombye_net::content::load_config(&data, 1).unwrap().items;
        let beans = ItemStack::new(items.iter().position(|d| d.id == "tinned_beans").unwrap() as u16, 4);
        let on_row = |index: u32, button| RowClick {
            action: RowAction { stack: beans, title: format!("row {index}"), left: Some((Place::Carried, index, Place::Ground)), more: Vec::new(), menu: menu_entries(1, &items, Place::Carried, index, &beans, None, &[], &[]), craft: None },
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
        let c = RowClick { action: RowAction { stack: ItemStack::one(0), title: String::new(), left: None, more: Vec::new(), menu: Vec::new(), craft: Some((2, 1)) }, button: PointerButton::Primary, one: false };
        assert!(matches!(click_intent(&c, 1, &mut f, 0.0), Some(Intent::Craft { player: 1, recipe: 2 })));
        assert!(click_intent(&c, 1, &mut f, 0.1).is_none(), "on its way");
        f.update("queued".into(), false, 0.2);
        assert!(click_intent(&c, 1, &mut f, 0.3).is_some(), "answered: another can be asked for");
        // Shift-click: make all, one request each.
        let mut f = InFlight::default();
        let all = RowClick { action: RowAction { craft: Some((2, 4)), ..c.action.clone() }, ..c.clone() };
        assert_eq!(click_intents(&all, 1, &mut f, 0.0).len(), 4);
        assert!(click_intents(&all, 1, &mut f, 0.1).is_empty(), "already asked");
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
    #[test]
    fn a_dragged_thing_goes_where_it_is_dropped() {
        let items = shipped_items();
        let idx = |id: &str| items.iter().position(|d| d.id == id).unwrap() as u16;
        let stack = |id: &str, n: u16, home: Pouch| ItemStack { home, ..ItemStack::new(idx(id), n) };
        let act = |from: Place, index: u32, s: ItemStack, more: Vec<Line>| RowAction { stack: s, title: String::new(), left: Some((from, index, Place::Carried)), more, menu: Vec::new(), craft: None };
        let mut flight = InFlight::default();
        // From the floor into the rucksack: the whole stack, to that bag.
        let beans = stack("tinned_beans", 3, Pouch::Pockets);
        let out = drop_intents(&act(Place::Ground, 9, beans, Vec::new()), Place::Pouch(Pouch::Back), false, 1, &mut flight, 0.0);
        assert!(matches!(out[..], [Intent::MoveItem { from: Place::Ground, index: 9, to: Place::Pouch(Pouch::Back), count: 0, .. }]), "{out:?}");
        // Again before the answer: nothing more.
        assert!(drop_intents(&act(Place::Ground, 9, beans, Vec::new()), Place::Pouch(Pouch::Back), false, 1, &mut flight, 0.1).is_empty());
        // A tile of two stacks, one already in the bag it is dropped on: only the other moves.
        let mut flight = InFlight::default();
        let (a, b) = (stack("tinned_beans", 2, Pouch::Pockets), stack("tinned_beans", 1, Pouch::Back));
        let out = drop_intents(&act(Place::Carried, 0, a, vec![Line { from: Place::Carried, index: 4, stack: b }]), Place::Pouch(Pouch::Back), false, 1, &mut flight, 0.0);
        assert!(matches!(out[..], [Intent::MoveItem { from: Place::Carried, index: 0, to: Place::Pouch(Pouch::Back), .. }]), "{out:?}");
        // To the floor: both, from the back first; with Alt just one of the first.
        let mut flight = InFlight::default();
        let out = drop_intents(&act(Place::Carried, 0, a, vec![Line { from: Place::Carried, index: 4, stack: b }]), Place::Ground, false, 1, &mut flight, 0.0);
        assert!(matches!(out[..], [Intent::Drop { index: 4, count: 0, .. }, Intent::Drop { index: 0, count: 0, .. }]), "{out:?}");
        let mut flight = InFlight::default();
        let out = drop_intents(&act(Place::Carried, 0, a, vec![Line { from: Place::Carried, index: 4, stack: b }]), Place::Ground, true, 1, &mut flight, 0.0);
        assert!(matches!(out[..], [Intent::Drop { index: 0, count: 1, .. }]), "{out:?}");
        // Onto the gear slots a thing goes where it belongs, one of it; what is worn back to a bag.
        assert_eq!(belongs(&items[idx("hoodie") as usize]), Place::Worn);
        assert_eq!(belongs(&items[idx("hiking_pack") as usize]), Place::Slot(Slot::Back));
        assert_eq!(belongs(&items[idx("tinned_beans") as usize]), Place::Slot(Slot::Hand));
        let mut flight = InFlight::default();
        let hoodie = stack("hoodie", 2, Pouch::Pockets);
        let out = drop_intents(&act(Place::Container(3), 1, hoodie, Vec::new()), Place::Worn, false, 1, &mut flight, 0.0);
        assert!(matches!(out[..], [Intent::MoveItem { from: Place::Container(3), index: 1, to: Place::Worn, count: 1, .. }]), "{out:?}");
        let out = drop_intents(&act(Place::Worn, 0, hoodie, Vec::new()), Place::Pouch(Pouch::Pockets), false, 1, &mut flight, 0.0);
        assert!(matches!(out[..], [Intent::MoveItem { from: Place::Worn, index: 0, to: Place::Pouch(Pouch::Pockets), .. }]), "{out:?}");
        // Dropped where it already is: nothing.
        assert!(drop_intents(&act(Place::Worn, 0, hoodie, Vec::new()), Place::Worn, false, 1, &mut InFlight::default(), 0.0).is_empty());
    }

    /// The whole of a drag, as the plugin runs it: press on a tile, move, let go over a pane.
    #[test]
    fn a_press_moved_is_a_drag_and_one_let_go_in_place_a_click() {
        let mut app = ui_app();
        app.init_resource::<Clicks>().init_resource::<InFlight>().init_resource::<Outbox>().init_resource::<Drag>().init_resource::<ButtonInput<KeyCode>>();
        app.add_systems(Update, drag);
        app.world_mut().spawn(Camera2d);
        app.world_mut().commands().spawn(Node { position_type: PositionType::Absolute, top: px(0), left: px(0), flex_direction: FlexDirection::Row, column_gap: px(100), ..default() }).with_children(|col| {
            col.spawn(Node { width: px(200), flex_direction: FlexDirection::Column, ..default() }).with_children(|c| {
                row(c, Cells::plain("Alarm clock", "0.4"), RowAction { stack: beans(1), title: String::new(), left: Some((Place::Carried, 0, Place::Ground)), more: Vec::new(), menu: Vec::new(), craft: None }, false);
            });
            col.spawn((DropTo::Place(Place::Pouch(Pouch::Back)), Node { width: px(200), height: px(200), ..default() }));
        });
        app.world_mut().flush();
        for _ in 0..3 {
            app.update();
        }
        let window = app.world_mut().query_filtered::<Entity, With<PrimaryWindow>>().single(app.world()).unwrap();
        let press = |app: &mut App, state: ButtonState| {
            let e = MouseButtonInput { button: MouseButton::Left, state, window };
            app.world_mut().write_message(e);
            app.world_mut().write_message(WindowEvent::MouseButtonInput(e));
        };
        let go = |app: &mut App, position: Vec2| {
            app.world_mut().query::<&mut Window>().single_mut(app.world_mut()).unwrap().set_cursor_position(Some(position));
            let moved = CursorMoved { window, position, delta: None };
            app.world_mut().write_message(moved.clone());
            app.world_mut().write_message(WindowEvent::CursorMoved(moved));
            app.update();
            app.update();
        };
        // Pressed and let go where it was: the click.
        go(&mut app, Vec2::new(20.0, 8.0));
        press(&mut app, ButtonState::Pressed);
        app.update();
        assert!(app.world().resource::<Drag>().0.is_some(), "held");
        assert!(app.world().resource::<Clicks>().0.is_empty(), "nothing until it is let go");
        press(&mut app, ButtonState::Released);
        app.update();
        app.update();
        assert_eq!(app.world().resource::<Clicks>().0.len(), 1);
        assert!(app.world().resource::<Outbox>().0.is_empty());
        app.world_mut().resource_mut::<Clicks>().0.clear();
        // Pressed, moved over the bag's pane, let go: it goes to that bag, and no click.
        press(&mut app, ButtonState::Pressed);
        app.update();
        go(&mut app, Vec2::new(400.0, 100.0));
        assert_eq!(app.world_mut().query_filtered::<(), With<Ghost>>().iter(app.world()).count(), 1, "its picture follows the pointer");
        assert_eq!(app.world_mut().query_filtered::<(), With<Outline>>().iter(app.world()).count(), 1, "the pane under it is outlined");
        press(&mut app, ButtonState::Released);
        app.update();
        app.update();
        let out = &app.world().resource::<Outbox>().0;
        assert!(matches!(out[..], [Intent::MoveItem { from: Place::Carried, index: 0, to: Place::Pouch(Pouch::Back), count: 0, .. }]), "{out:?}");
        assert!(app.world().resource::<Clicks>().0.is_empty());
        assert_eq!(app.world_mut().query_filtered::<(), With<Ghost>>().iter(app.world()).count(), 0);
        assert_eq!(app.world_mut().query_filtered::<(), With<Outline>>().iter(app.world()).count(), 0);
        // Dropped on nothing: nothing.
        app.world_mut().resource_mut::<InFlight>().asked.clear();
        go(&mut app, Vec2::new(20.0, 8.0));
        press(&mut app, ButtonState::Pressed);
        app.update();
        go(&mut app, Vec2::new(20.0, 400.0));
        press(&mut app, ButtonState::Released);
        app.update();
        app.update();
        assert_eq!(app.world().resource::<Outbox>().0.len(), 1);
    }

    /// Health and Skills are screens of the frame: their keys open it on them, the rail goes
    /// between them, the key again or shutting the frame goes back to the game.
    #[test]
    fn health_and_skills_are_screens_of_the_frame() {
        use crate::health_ui::HealthPanel;
        use crate::skills_ui::SkillsPanel;
        let mut app = App::new();
        app.init_resource::<InventoryPanel>().init_resource::<HealthPanel>().init_resource::<SkillsPanel>().init_resource::<crate::build_ui::BuildMenu>().init_resource::<MakeUi>().init_resource::<Goto>().add_systems(Update, screens);
        let state = |app: &mut App| (app.world().resource::<InventoryPanel>().open, app.world().resource::<HealthPanel>().open, app.world().resource::<SkillsPanel>().open);
        app.update();
        assert_eq!(state(&mut app), (false, false, false));
        // H: the frame opens on Health.
        app.world_mut().resource_mut::<HealthPanel>().open = true;
        app.update();
        assert_eq!(state(&mut app), (true, true, false));
        // K: over to Skills.
        app.world_mut().resource_mut::<SkillsPanel>().open = true;
        app.update();
        assert_eq!(state(&mut app), (true, false, true));
        // The rail: to Make, and the frame stays.
        app.world_mut().resource_mut::<Goto>().0 = Some(Tab::Make);
        app.update();
        assert_eq!(state(&mut app), (true, false, false));
        assert!(app.world().resource::<MakeUi>().shown);
        app.world_mut().resource_mut::<Goto>().0 = Some(Tab::Health);
        app.update();
        assert_eq!(state(&mut app), (true, true, false));
        // H again: back to the game.
        app.world_mut().resource_mut::<HealthPanel>().open = false;
        app.update();
        assert_eq!(state(&mut app), (false, false, false));
        // "Use on a wound" from the open inventory: Health, with the item kept; Tab shuts it all.
        app.world_mut().resource_mut::<InventoryPanel>().open = true;
        app.update();
        *app.world_mut().resource_mut::<HealthPanel>() = HealthPanel { open: true, selected: None, use_item: Some(3) };
        app.update();
        assert_eq!(state(&mut app), (true, true, false));
        assert_eq!(app.world().resource::<HealthPanel>().use_item, Some(3));
        app.world_mut().resource_mut::<InventoryPanel>().open = false;
        app.update();
        assert_eq!(state(&mut app), (false, false, false));
        assert_eq!(app.world().resource::<HealthPanel>().use_item, None);
    }

}
