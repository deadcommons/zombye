//! The inventory panel (Tab or I): what you carry, hold and wear with your load against the
//! carry limit, beside the container you have open (search one with E) and the floor within reach.
//! Click a row to move it: from you into the open container (or onto the floor if none is open),
//! from a container or the floor into your pockets and bags, off a slot into your bags.
//! Right-click one of your carried stacks to equip, wear, eat or drink it. Shift-click moves just
//! one of a stack. Everything goes to the server as intents; the panel only shows what the latest
//! snapshot said. Moving takes a moment (the server queues it): the header shows how busy you are.
//! The last answer to a request (done, or why not) shows at the foot of the panel, and for a few
//! seconds under the HUD when the panel is shut.

use bevy::prelude::*;
use zombye_net::{Intent, PrivateView};
use zombye_sim::combat::PICKUP_REACH;
use zombye_sim::{Burden, ItemCategory, ItemDef, ItemStack, Place, Slot};

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

pub struct InventoryUiPlugin;

impl Plugin for InventoryUiPlugin {
    fn build(&self, app: &mut App) {
        let open = app.world().get_resource::<crate::Options>().is_some_and(|o| o.inventory);
        app.insert_resource(InventoryPanel { open }).init_resource::<LastNote>().init_resource::<Shown>().add_systems(Startup, spawn).add_systems(Update, (toggle, keep_note, rebuild).chain());
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
    if let Some(n) = notes.0.last() {
        *last = LastNote { text: n.text.clone(), ok: n.ok, at: time.elapsed_secs() };
        println!("item: {}{}", if n.ok { "" } else { "refused: " }, n.text);
    }
    let Ok((mut t, mut c)) = line.single_mut() else { return };
    let show = !panel.open && !last.text.is_empty() && time.elapsed_secs() - last.at < NOTE_SECS;
    let want = if show { last.text.clone() } else { String::new() };
    if t.0 != want {
        t.0 = want;
    }
    c.0 = if last.ok { OK } else { NO };
}

fn name_of(items: &[ItemDef], s: &ItemStack) -> String {
    let name = items.get(s.item as usize).map_or("?", |d| d.name.as_str());
    if s.count > 1 { format!("{name} x{}", s.count) } else { name.to_string() }
}

fn stack_kg(items: &[ItemDef], s: &ItemStack) -> f32 {
    items.get(s.item as usize).map_or(0.0, |d| d.weight_kg * s.count as f32)
}

/// What a click on a row sends: left (whole stack, or one with Shift) and right.
#[derive(Clone)]
struct RowAction {
    left: Option<(Place, u32, Place)>,
    right: Option<Intent>,
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
    let view = link.own(me.id).cloned();
    let pos = me.curr.pos;
    let floor: Vec<(u32, ItemStack)> = link.curr.items.iter().filter(|it| it.pos.distance(pos) <= PICKUP_REACH).map(|it| (it.id, it.stack)).collect();
    let busy = link.curr.players.iter().find(|p| p.id == me.id).map_or((0.0, 0), |p| (p.busy, p.jobs));
    let note_fresh = time.elapsed_secs() - last.at < NOTE_SECS * 2.0;
    let signature = format!("{view:?}|{floor:?}|{:.1}|{}|{}|{}", busy.0, busy.1, if note_fresh { last.text.as_str() } else { "" }, me.dead);
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
                        Some(s) => row(col, format!("{label}: {}", name_of(items, &s)), format!("{:.1} kg", stack_kg(items, &s)), RowAction { left: Some((Place::Slot(slot), 0, Place::Carried)), right: None }, me.id),
                        None => dim(col, format!("{label}: -")),
                    }
                }
                for (k, s) in inv.worn.iter().enumerate() {
                    row(col, format!("Worn: {}", name_of(items, s)), format!("{:.1} kg", stack_kg(items, s)), RowAction { left: Some((Place::Worn, k as u32, Place::Carried)), right: None }, me.id);
                }
                dim(col, format!("Carried ({:.1} of {:.0} L):", v.carry.used_l, v.carry.room_l));
                if inv.carried.is_empty() {
                    dim(col, "  nothing".into());
                }
                for (k, s) in inv.carried.iter().enumerate() {
                    let def = items.get(s.item as usize);
                    let to = open.map_or(Place::Ground, Place::Container);
                    let right = def.map(|d| match d.category {
                        ItemCategory::Food | ItemCategory::Drink => Intent::Use { player: me.id, from: Place::Carried, index: k as u32 },
                        ItemCategory::Clothing => Intent::Wear { player: me.id, index: k as u32 },
                        _ => Intent::Equip { player: me.id, index: k as u32 },
                    });
                    row(col, format!("  {}", name_of(items, s)), format!("{:.1} kg", stack_kg(items, s)), RowAction { left: Some((Place::Carried, k as u32, to)), right }, me.id);
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
                        row(col, name_of(items, s), format!("{:.1} kg", stack_kg(items, s)), RowAction { left: Some((Place::Container(cv.id), k as u32, Place::Carried)), right: None }, me.id);
                    }
                });
            }
            // The floor.
            column(cols, "Floor", |col| {
                if floor.is_empty() {
                    dim(col, "nothing within reach".into());
                }
                for (id, s) in &floor {
                    row(col, name_of(items, s), format!("{:.1} kg", stack_kg(items, s)), RowAction { left: Some((Place::Ground, *id, Place::Carried)), right: None }, me.id);
                }
                if open.is_none() {
                    dim(col, "E: search the cupboard,".into());
                    dim(col, "drawer or fridge you face".into());
                }
            });
        });
        c.spawn((Text::new("Click: move   Shift-click: just one   Right-click: equip / wear / eat   Tab: close"), font(12.0), TextColor(DIM)));
        if note_fresh && !last.text.is_empty() {
            c.spawn((Text::new(last.text.clone()), font(14.0), TextColor(if last.ok { OK } else { NO })));
        }
    });
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

/// A clickable row: `label` on the left, `kg` on the right.
fn row(col: &mut ChildSpawnerCommands, label: String, kg: String, action: RowAction, player: u32) {
    let (left, right) = (action.clone(), action);
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
    .observe(move |ev: On<PointerPress>, keys: Res<ButtonInput<KeyCode>>, mut link: ResMut<ServerLink>| {
        let one = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        match ev.button {
            PointerButton::Primary => {
                if let Some((from, index, to)) = left.left {
                    let count = if one { 1 } else { 0 };
                    let intent = match (from, to) {
                        (Place::Slot(_) | Place::Worn, Place::Carried) => Intent::Unequip { player, from, index },
                        (_, Place::Ground) => Intent::Drop { player, from, index, count },
                        _ => Intent::MoveItem { player, from, index, to, count },
                    };
                    link.send(intent);
                }
            }
            PointerButton::Secondary => {
                if let Some(i) = right.right.clone() {
                    link.send(i);
                }
            }
            _ => {}
        }
    });
}
