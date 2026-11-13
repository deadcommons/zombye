//! One key for the world (`keys::Action::Interact`, E): tapped, it does the likeliest thing for
//! what the player faces; held, it opens the menu of everything that can be done with it (the
//! same menu a right tap on the thing gives, `world_actions`). A prompt at the bottom of the
//! screen always says what E will do.
//!
//! What is faced, in order: at the wheel, getting out; a car beside you, getting in (or a parked
//! one that won't start: where the nearest that will is); then the door, window, fence, light
//! switch or campfire in front ([`world_actions::faced`]: open, shut, climb through, flick,
//! tend); else the things on our floor (pick up); else the container or body in reach (search).
//! The likeliest thing is the first line of that thing's menu ([`world_actions::entries`]), so the
//! key, the prompt and the menus never disagree. Nothing is decided here: the server checks each
//! request as ever.
//!
//! Also here: F switches the light in your hands on and off (`keys::Action::Flashlight`).

use bevy::prelude::*;
use zombye_net::Intent;
use zombye_sim::Place;

use crate::inventory_ui::{InventoryPanel, ItemMenu};
use crate::keys::{self, Action};
use crate::player::LocalPlayer;
use crate::server_link::{ClientMap, JoinInfo, ServerLink};
use crate::theme::{self, DIM, TEXT, font};
use crate::world_actions::{self, Target, WorldEntry};

/// How long E is held before it opens the menu instead (seconds).
pub const HOLD_SECS: f32 = 0.3;

/// What E would do now.
#[derive(Clone, Debug)]
pub enum Faced {
    /// Get out of the car (at the wheel).
    Exit,
    /// Get into this car.
    Enter(Option<u32>),
    /// A parked car that won't start (its name).
    Parked(String),
    /// A thing in the world, its menu's title and lines, and where it is.
    Thing { title: String, lines: Vec<WorldEntry>, at: Vec2 },
    /// Our floor: the nearest item on it, and its name.
    Floor(u32, String),
}

impl Faced {
    /// The likeliest line: what a tap does, and what the prompt says.
    pub fn primary(&self) -> (String, Option<String>) {
        match self {
            Faced::Exit => ("Get out".into(), None),
            Faced::Enter(_) => ("Get in".into(), Some("car".into())),
            Faced::Parked(name) => ("Try the car".into(), Some(name.clone())),
            Faced::Thing { title, lines, .. } => {
                let line = lines.iter().find(|l| l.intent.is_some()).or(lines.first());
                (line.map_or_else(String::new, |l| l.label.clone()), Some(title.to_lowercase()))
            }
            Faced::Floor(_, name) => ("Pick up".into(), Some(name.clone())),
        }
    }

    /// Is there a menu worth holding E for (more than the one thing a tap does)?
    pub fn has_more(&self) -> bool {
        match self {
            Faced::Thing { lines, .. } => lines.len() > 1,
            Faced::Floor(..) => true,
            // A car's menu: the lock, the boot and glovebox, hotwiring, repairs (stage 4).
            Faced::Exit | Faced::Enter(Some(_)) => true,
            _ => false,
        }
    }
}

/// What E faces this frame (for the prompt and the key).
#[derive(Resource, Default)]
pub struct Facing(pub Option<Faced>);

pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Facing>().add_systems(Startup, spawn_prompt).add_systems(Update, (look, press, flashlight, prompt).chain().run_if(crate::live));
    }
}

/// Work out what E faces now.
#[allow(clippy::too_many_arguments)]
fn look(
    me: Res<LocalPlayer>,
    join: Res<JoinInfo>,
    link: Res<ServerLink>,
    map: Res<ClientMap>,
    mut facing: ResMut<Facing>,
    mut sites: Local<Option<Vec<zombye_sim::containers::ContainerSite>>>,
    mut switches: Local<Option<Vec<Option<zombye_sim::power::SwitchSpot>>>>,
) {
    facing.0 = None;
    if me.dead || me.curr.climb.is_some() {
        return;
    }
    if me.car.is_some() {
        facing.0 = Some(Faced::Exit);
        return;
    }
    let (pos, dir) = (me.curr.pos, me.curr.facing);
    if let Some(i) = zombye_sim::vehicles::enterable(&map.0.vehicles, pos, dir, join.vehicles.tuning.enter_reach) {
        facing.0 = Some(Faced::Enter(map.0.vehicles.get(i).map(|b| b.id)));
        return;
    }
    if let Some(name) = crate::player::parked_car_faced(&map.0, &join.vehicles, pos, dir) {
        facing.0 = Some(Faced::Parked(name));
        return;
    }
    let sites = sites.get_or_insert_with(|| zombye_sim::containers::find_sites(&map.0, &join.containers));
    let switches = switches.get_or_insert_with(|| zombye_sim::power::switch_spots(&map.0));
    let bodies = link.curr.zombies.iter().filter(|z| z.anim == zombye_sim::ZombieAnim::Dead).map(|z| (z.id, Vec2::from(z.pos)));
    let thing = world_actions::faced(&map.0, sites, switches, &link.curr.fires, bodies, join.fire.reach, pos, me.curr.level, dir);
    let menu = |t: Target, at: Vec2| {
        let (title, lines) = world_actions::entries(t, &map.0, sites, &join.containers, &join.items, &join.builds, &join.blind_tag, join.fire.reach, me.id, pos, link.own(me.id));
        (!lines.is_empty()).then_some(Faced::Thing { title, lines, at })
    };
    // Doors, windows, fences, switches and fires before the floor; the floor before searching.
    let first = thing.filter(|(t, _)| matches!(t, Target::Edge(_) | Target::Switch(..) | Target::Fire(..)));
    if let Some((t, at)) = first {
        facing.0 = menu(t, at);
        return;
    }
    let floor = link.curr.floor_of(me.id);
    let nearest = link.curr.items.iter().filter(|it| floor.contains(&it.id)).min_by(|a, b| a.pos.distance(pos).total_cmp(&b.pos.distance(pos)));
    if let Some(it) = nearest {
        let name = join.items.get(it.stack.item as usize).map_or_else(|| "it".to_string(), |d| d.name.to_lowercase());
        let name = if it.stack.count > 1 { format!("{name} x{}", it.stack.count) } else { name };
        facing.0 = Some(Faced::Floor(it.id, name));
        return;
    }
    if let Some((t, at)) = thing {
        facing.0 = menu(t, at);
    }
}

/// E: a tap does the likeliest thing (on release), held it opens the menu.
#[allow(clippy::too_many_arguments)]
fn press(
    time: Res<Time>,
    keys_in: Res<ButtonInput<KeyCode>>,
    me: Res<LocalPlayer>,
    join: Res<JoinInfo>,
    facing: Res<Facing>,
    map: Res<ClientMap>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<crate::camera::IsoCamera>>,
    mut link: ResMut<ServerLink>,
    mut menu: ResMut<ItemMenu>,
    notice: Option<ResMut<crate::saving::Notice>>,
    (mode, mut wheel): (Res<crate::pad::InputMode>, ResMut<crate::wheel::Wheel>),
    mut down: Local<Option<f32>>,
    mut held: Local<bool>,
) {
    let now = time.elapsed_secs();
    // Beside a car the refuel key (Shift+E) comes first: pour the fuel we carry in, else siphon
    // the car's fuel into our can (the server says what's missing). E's own tap is then dropped.
    if !me.dead
        && keys::just_pressed(&keys_in, Action::Refuel)
        && let Some(Faced::Enter(vehicle)) = &facing.0
    {
        let pour = link.own(me.id).is_some_and(|v| crate::player::fuel_to_pour(&v.inventory, &join));
        let vehicle = *vehicle;
        let op = if pour { zombye_sim::VehicleOp::Refuel { vehicle } } else { zombye_sim::VehicleOp::Siphon { vehicle } };
        link.send(Intent::Vehicle { player: me.id, op });
        *down = None;
        return;
    }
    if keys::just_pressed(&keys_in, Action::Interact) {
        *down = Some(now);
        *held = false;
    }
    let Some(t) = *down else { return };
    if me.dead {
        *down = None;
        return;
    }
    // Held long enough: the menu of what we face, beside it.
    if !*held && keys::pressed(&keys_in, Action::Interact) && now - t >= HOLD_SECS {
        *held = true;
        let spot = |at: Vec2| -> Option<Vec2> {
            let (window, (camera, tf)) = (windows.single().ok()?, cams.single().ok()?);
            let ground = map.0.ground(at);
            let v = camera.world_to_viewport(tf, Vec3::new(at.x, ground + 1.3, at.y)).ok()?;
            Some(crate::render_scale::from_view(camera, window, v))
        };
        match &facing.0 {
            // On the controller, a wheel round the middle of the screen (`wheel.rs`).
            Some(Faced::Thing { title, lines, .. }) if mode.pad => wheel.show(crate::wheel::Kind::Actions, title.clone(), crate::wheel::action_entries(lines)),
            Some(Faced::Thing { title, lines, at }) => menu.show_world(title.clone(), lines.clone(), spot(*at).unwrap_or(Vec2::splat(200.0))),
            Some(Faced::Floor(id, _)) => {
                let at = link.curr.items.iter().find(|i| i.id == *id).map(|i| i.pos).unwrap_or(me.curr.pos);
                crate::inventory_ui::open_menu(&mut menu, &link, &join, me.id, Place::Ground, *id, spot(at));
            }
            Some(Faced::Exit | Faced::Enter(Some(_))) => {
                let id = match &facing.0 {
                    Some(Faced::Enter(Some(id))) => Some(*id),
                    _ => me.car.map(|c| c.id),
                };
                match id.and_then(|id| link.curr.vehicles.iter().find(|v| v.id == id)) {
                    Some(v) => {
                        let (title, lines) = world_actions::car_entries(v, &join.vehicles, &join.items, me.id, link.own(me.id));
                        menu.show_world(title, lines, spot(v.state.pos).unwrap_or(Vec2::splat(200.0)));
                    }
                    None => *held = false,
                }
            }
            // Nothing with more than one thing to do: a hold is a tap.
            _ => *held = false,
        }
    }
    if !keys::just_released(&keys_in, Action::Interact) {
        return;
    }
    *down = None;
    if *held {
        return;
    }
    if me.curr.climb.is_some() {
        return;
    }
    match &facing.0 {
        Some(Faced::Exit) => link.send(Intent::Vehicle { player: me.id, op: zombye_sim::VehicleOp::Exit }),
        Some(Faced::Enter(vehicle)) => link.send(Intent::Vehicle { player: me.id, op: zombye_sim::VehicleOp::Enter { vehicle: *vehicle } }),
        Some(Faced::Parked(name)) => {
            if let Some(mut n) = notice {
                let text = crate::player::wont_start(name, &link, &join, me.curr.pos);
                n.say(now, text);
            }
        }
        Some(Faced::Thing { lines, .. }) => match lines.iter().find_map(|l| l.intent.clone()) {
            Some(i) => link.send(i),
            // Only lines that say something (boarded up, too far): say the first.
            None => {
                if let (Some(mut n), Some(l)) = (notice, lines.first()) {
                    n.say(now, l.label.clone());
                }
            }
        },
        Some(Faced::Floor(..)) => link.send(Intent::PickUp { player: me.id }),
        // Nothing faced: ask the server, which says so once ("nothing to pick up or search here").
        None => link.send(Intent::Search { player: me.id, container: None }),
    }
}

/// F: the light in our hands on or off (the server says if there's none).
fn flashlight(keys_in: Res<ButtonInput<KeyCode>>, me: Res<LocalPlayer>, mut link: ResMut<ServerLink>) {
    if !me.dead && keys::just_pressed(&keys_in, Action::Flashlight) {
        link.send(Intent::Flashlight { player: me.id, on: None });
    }
}

#[derive(Component)]
struct Prompt;
#[derive(Component)]
struct PromptKey;
#[derive(Component)]
struct PromptDoes;
#[derive(Component)]
struct PromptWhat;
#[derive(Component)]
struct PromptMore;

/// How far above the hotbar the prompt stands: over where the current-action bar goes.
const PROMPT_UP: f32 = 84.0;

fn spawn_prompt(mut commands: Commands) {
    // A strip across the screen's width, holding the prompt in its middle.
    commands
        .spawn((
            Prompt,
            Node { position_type: PositionType::Absolute, bottom: px(crate::hotbar::REACH + PROMPT_UP), left: px(0), right: px(0), justify_content: JustifyContent::Center, ..default() },
            Visibility::Hidden,
            Pickable::IGNORE,
        ))
        .with_children(|c| {
            c.spawn((Node { align_items: AlignItems::Center, column_gap: px(10), padding: UiRect::axes(px(theme::STRIP_PAD.0), px(theme::STRIP_PAD.1 - 2.0)), ..default() }, theme::strip(), Pickable::IGNORE)).with_children(|row| {
                row.spawn((Node { padding: UiRect::axes(px(9), px(3)), justify_content: JustifyContent::Center, min_width: px(30), ..default() }, theme::button(false), Pickable::IGNORE)).with_children(|k| {
                    k.spawn((PromptKey, Text::new("E"), theme::strong(theme::BODY), TextColor(theme::INK), TextLayout::no_wrap(), Pickable::IGNORE));
                });
                row.spawn((PromptDoes, Text::new(""), theme::strong(theme::BODY), TextColor(TEXT), TextLayout::no_wrap(), Pickable::IGNORE));
                row.spawn((PromptWhat, Text::new(""), font(theme::BODY), TextColor(DIM), TextLayout::no_wrap(), Pickable::IGNORE));
                row.spawn((PromptMore, Text::new(""), font(theme::SMALL), TextColor(DIM), TextLayout::no_wrap(), Pickable::IGNORE));
            });
        });
}

/// Show what E does, or nothing.
#[allow(clippy::type_complexity)]
fn prompt(
    facing: Res<Facing>,
    panel: Res<InventoryPanel>,
    menu: Res<ItemMenu>,
    mut root: Query<&mut Visibility, With<Prompt>>,
    mut texts: ParamSet<(Query<&mut Text, With<PromptKey>>, Query<&mut Text, With<PromptDoes>>, Query<&mut Text, With<PromptWhat>>, Query<&mut Text, With<PromptMore>>)>,
    mut shown: Local<Option<(String, Option<String>, bool, String)>>,
) {
    let Ok(mut vis) = root.single_mut() else { return };
    let what = facing.0.as_ref().filter(|_| !panel.open && !menu.is_up());
    let Some(f) = what else {
        *vis = Visibility::Hidden;
        *shown = None;
        return;
    };
    let (does, thing) = f.primary();
    let more = f.has_more() && crate::help_text::keys();
    let key = keys::short(Action::Interact);
    let now = (does, thing, more, key);
    *vis = Visibility::Inherited;
    if shown.as_ref() == Some(&now) {
        return;
    }
    if let Ok(mut t) = texts.p0().single_mut() {
        t.0 = now.3.clone();
    }
    if let Ok(mut t) = texts.p1().single_mut() {
        t.0 = now.0.clone();
    }
    if let Ok(mut t) = texts.p2().single_mut() {
        t.0 = now.1.clone().unwrap_or_default();
    }
    if let Ok(mut t) = texts.p3().single_mut() {
        t.0 = if now.2 { "· hold for more".to_string() } else { String::new() };
    }
    *shown = Some(now);
}
