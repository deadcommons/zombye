//! Right-clicking things in the world: the door, window, fence, blind or container under the
//! pointer, and what can be done with it. A quick right tap opens its menu (the inventory's item
//! menu, `inventory_ui::world_menu`); held, the right mouse aims as ever. The keys still work.
//!
//! Two pure steps, so other systems add their own things and actions in one place each:
//! [`pick`] finds the [`Target`] the pointer's ray meets first, and [`entries`] lists what can be
//! done with it, the likeliest first, as intents. Everything that goes by what the player faces
//! (doors, windows, fences, blinds) is sent wrapped in [`Intent::Toward`], which turns the player
//! to the thing first, so the server's own rules (reach, what is faced) pick that same thing.
//! Nothing here decides anything: the server checks every request as it does a key's.
//!
//! **Adding something:** a [`Target`] variant, its test in [`pick`], and its lines in [`entries`].

use bevy::prelude::*;
use zombye_net::{Intent, PrivateView};
use zombye_sim::containers::{ContainerData, ContainerSite};
use zombye_sim::map::{Edge, EdgeRef, OwnedEdge, TileMap};
use zombye_sim::{BlindOp, BuildDef, ItemDef};

/// How high a door's opening is, and a window's sill and head (docs/asset-pipeline.md).
const DOOR_TOP: f32 = 2.1;
const SILL: f32 = 0.9;
const FENCE_TOP: f32 = 1.0;
/// How tall furniture is taken to be, for the pointer to land on it.
const FURNITURE_TOP: f32 = 1.0;
/// How high a body lying on the ground is taken to be.
const BODY_TOP: f32 = 0.5;
/// How near the pointer's spot on the ground must be to a door or window cut down to a stub (the
/// cutaways) for it still to count.
const STUB_SLACK: f32 = 0.35;
/// How far from the player things are looked for (metres): further is never in reach, but the
/// menu still says so.
const LOOK: f32 = 12.0;

/// A thing in the world the pointer is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// A door, window or fence.
    Edge(EdgeRef),
    /// A container: an index into the sites [`pick`] was given.
    Container(usize),
    /// A dead zombie's body (its id, where it lies): searched like a container ([`pick_body`]).
    Body(u32, Vec2),
    /// A room's light switch (the room, where the switch is: `power::switch_spots`).
    Switch(u16, Vec2),
    /// A campfire someone built (its id, where it stands, burning or not, game minutes of fuel).
    Fire(u32, Vec2, bool, u16),
    /// A tap that isn't a container (a bath, a toilet: `power.ron`'s `water.taps`): its
    /// furniture's min corner, floor and kind ([`pick_tap`]).
    Tap(IVec2, i8, &'static str),
}

/// One line of a world menu: its label and what it sends (`None`: a line that only says
/// something, such as being too far away). `facing`: the way the player turns as it's sent.
#[derive(Clone, Debug)]
pub struct WorldEntry {
    pub label: String,
    pub intent: Option<Intent>,
    pub facing: Option<f32>,
}

impl WorldEntry {
    fn send(label: impl Into<String>, intent: Intent) -> Self {
        Self { label: label.into(), intent: Some(intent), facing: None }
    }

    pub fn say(label: impl Into<String>) -> Self {
        Self { label: label.into(), intent: None, facing: None }
    }
}

/// Where the ray from `origin` along `dir` (world space: x east, y up, z the map's y) first
/// meets a vertical rectangle on the map segment `a`..`b` between heights `lo` and `hi`; the
/// ray's parameter there.
fn ray_quad(origin: Vec3, dir: Vec3, a: Vec2, b: Vec2, lo: f32, hi: f32) -> Option<f32> {
    let along = b - a;
    let normal = Vec2::new(-along.y, along.x);
    let denom = normal.dot(Vec2::new(dir.x, dir.z));
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = normal.dot(a - Vec2::new(origin.x, origin.z)) / denom;
    if t <= 0.0 {
        return None;
    }
    let hit = origin + dir * t;
    let u = (Vec2::new(hit.x, hit.z) - a).dot(along) / along.length_squared();
    ((-0.05..=1.05).contains(&u) && (lo - 0.05..=hi + 0.05).contains(&hit.y)).then_some(t)
}

/// Where the ray first meets the box over tiles `min..min+size` up to `top`.
fn ray_box(origin: Vec3, dir: Vec3, min: Vec2, max: Vec2, top: f32) -> Option<f32> {
    let lo = Vec3::new(min.x, 0.0, min.y);
    let hi = Vec3::new(max.x, top, max.y);
    let (mut t0, mut t1) = (0.0f32, f32::INFINITY);
    for k in 0..3 {
        let (o, d) = (origin[k], dir[k]);
        if d.abs() < 1e-9 {
            if o < lo[k] || o > hi[k] {
                return None;
            }
            continue;
        }
        let (a, b) = ((lo[k] - o) / d, (hi[k] - o) / d);
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// How far `p` is from the segment `a`..`b`.
fn to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let u = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
    p.distance(a + ab * u)
}

/// The edges near `around` something can be done with: doors, windows and fences.
fn actionable(kind: Edge) -> bool {
    kind.is_door() || kind.is_window() || kind.is_boarded() || kind == Edge::DoorBroken || kind == Edge::Fence
}

/// The thing the pointer's ray (from `origin` along `dir`) meets first near the player at
/// `player`: a door, window or fence (its opening, or the ground just by it when the wall is cut
/// down), or a container's furniture. With the ray parameter where it meets it, so a caller can
/// weigh it against what else is under the pointer.
pub fn pick(map: &TileMap, sites: &[ContainerSite], player: Vec2, origin: Vec3, dir: Vec3) -> Option<(Target, f32)> {
    // Where the ray comes down; a level or rising one (first person, looking up) never does.
    let ground_t = if dir.y < -1e-4 { -origin.y / dir.y } else { 1e6 };
    let ground = origin + dir * ground_t;
    let ground = Vec2::new(ground.x, ground.z);
    let mut best: Option<(Target, f32)> = None;
    let mut take = |t: Target, at: f32| {
        if best.is_none_or(|(_, b)| at < b) {
            best = Some((t, at));
        }
    };
    // Edges round the player: only those within `LOOK` of it count, wherever the ray comes down
    // (in first person that can be far beyond the door it passes through).
    let c = player.floor().as_ivec2();
    let r = LOOK.ceil() as i32 + 1;
    for dy in -r..=r {
        for dx in -r..=r {
            let tile = c + IVec2::new(dx, dy);
            if !map.in_bounds(tile) {
                continue;
            }
            for which in [OwnedEdge::North, OwnedEdge::West] {
                let kind = map.owned_edge(tile, which);
                if !actionable(kind) {
                    continue;
                }
                let e = EdgeRef::new(tile, which);
                if e.mid().distance(player) > LOOK {
                    continue;
                }
                let (a, b) = TileMap::edge_segment(tile, which);
                let (lo, hi) = if kind.is_window() { (SILL, DOOR_TOP) } else if kind == Edge::Fence { (0.0, FENCE_TOP) } else { (0.0, DOOR_TOP) };
                if let Some(t) = ray_quad(origin, dir, a, b, lo, hi) {
                    take(Target::Edge(e), t);
                } else if to_segment(ground, a, b) < STUB_SLACK {
                    take(Target::Edge(e), ground_t);
                }
            }
        }
    }
    for (k, s) in sites.iter().enumerate() {
        if s.centre().distance(player) > LOOK {
            continue;
        }
        let (min, max) = (s.min.as_vec2(), (s.min + s.size).as_vec2());
        if let Some(t) = ray_box(origin, dir, min, max, FURNITURE_TOP) {
            take(Target::Container(k), t);
        }
    }
    best
}

/// How far off the way a body faces something may be and still be the thing it faces (E), and
/// how near it may be and count whatever the angle (it's against you).
const FACE_DEGREES: f32 = 70.0;
const TOUCHING: f32 = 0.45;

/// What a body at `pos` facing `facing` (on floor `level`) would handle with E: the door, window
/// or fence, light switch, campfire, container or body in reach and in front of it, the likeliest
/// (nearest, and most squarely in front) first, as the server's reach rules have it. With the
/// spot it is at (for a menu beside it). Things on the floor are the caller's (they go by the
/// server's own floor list).
#[allow(clippy::too_many_arguments)]
pub fn faced(
    map: &TileMap,
    sites: &[ContainerSite],
    switches: &[Option<zombye_sim::power::SwitchSpot>],
    fires: &[zombye_sim::FireView],
    bodies: impl IntoIterator<Item = (u32, Vec2)>,
    fire_reach: f32,
    pos: Vec2,
    level: u8,
    facing: f32,
) -> Option<(Target, Vec2)> {
    let ahead = Vec2::from_angle(facing);
    let cos = FACE_DEGREES.to_radians().cos();
    // Lower is likelier: distance, longer the further off square it is.
    let score = |at: Vec2| -> Option<f32> {
        let to = at - pos;
        let d = to.length();
        if d < TOUCHING {
            return Some(d);
        }
        let c = (to / d).dot(ahead);
        (c >= cos).then(|| d * (2.0 - c))
    };
    let mut best: Option<(f32, Target, Vec2)> = None;
    let mut take = |t: Target, at: Vec2, s: Option<f32>| {
        if let Some(s) = s
            && best.as_ref().is_none_or(|(b, ..)| s < *b)
        {
            best = Some((s, t, at));
        }
    };
    let room = map.room_at(pos);
    if level == 0 {
        let c = pos.floor().as_ivec2();
        for dy in -2..=2 {
            for dx in -2..=2 {
                for which in [OwnedEdge::North, OwnedEdge::West] {
                    let e = EdgeRef::new(c + IVec2::new(dx, dy), which);
                    if !map.in_bounds(e.tile) {
                        continue;
                    }
                    let kind = map.owned_edge(e.tile, which);
                    // A broken door has nothing to do with it.
                    if !actionable(kind) || kind == Edge::DoorBroken || e.mid().distance(pos) > zombye_sim::world::INTERACT_REACH {
                        continue;
                    }
                    take(Target::Edge(e), e.mid(), score(e.mid()));
                }
            }
        }
        if let Some(Some(s)) = switches.get(room as usize)
            && room != 0
            && s.pos.distance(pos) <= zombye_sim::power::SWITCH_REACH - 0.1
        {
            take(Target::Switch(room, s.pos), s.pos, score(s.pos));
        }
        for f in fires {
            let at = Vec2::from(f.pos);
            if at.distance(pos) <= fire_reach {
                take(Target::Fire(f.id, at, f.lit, f.fuel_min), at, score(at));
            }
        }
        let bodies: Vec<(u32, Vec2)> = bodies.into_iter().collect();
        if let Some(id) = zombye_sim::corpses::body_in_reach(map, pos, bodies.iter().copied())
            && let Some(&(id, at)) = bodies.iter().find(|b| b.0 == id)
        {
            take(Target::Body(id, at), at, score(at).or(Some(9.0)));
        }
    }
    for (k, s) in sites.iter().enumerate() {
        if s.level != level || s.room != room || s.distance(pos) > zombye_sim::inventory::CONTAINER_REACH {
            continue;
        }
        let near = pos.clamp(s.min.as_vec2(), (s.min + s.size).as_vec2());
        // In reach but not in front: still searched (the server takes the nearest), after the rest.
        take(Target::Container(k), near, score(near).or(Some(8.0 + s.distance(pos))));
    }
    best.map(|(_, t, at)| (t, at))
}

/// The body (a dead zombie's id and where it lies, from `bodies`) the pointer's ray meets first,
/// as a low box round where it lies, with the ray parameter there.
pub fn pick_body(bodies: impl IntoIterator<Item = (u32, Vec2)>, player: Vec2, origin: Vec3, dir: Vec3) -> Option<(Target, f32)> {
    let half = Vec2::splat(zombye_sim::corpses::BODY_RADIUS + 0.4);
    bodies
        .into_iter()
        .filter(|&(_, at)| at.distance(player) <= LOOK)
        .filter_map(|(id, at)| ray_box(origin, dir, at - half, at + half, BODY_TOP).map(|t| (Target::Body(id, at), t)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// The light switch (`spots`: `power::switch_spots`, by room) the pointer's ray meets first, as
/// its plate on the wall and a little round it, with the ray parameter there.
pub fn pick_switch(spots: &[Option<zombye_sim::power::SwitchSpot>], player: Vec2, origin: Vec3, dir: Vec3) -> Option<(Target, f32)> {
    let h = zombye_sim::power::SWITCH_HEIGHT;
    spots
        .iter()
        .enumerate()
        .filter_map(|(room, s)| s.map(|s| (room as u16, s)))
        .filter(|(_, s)| s.pos.distance(player) <= LOOK)
        .filter_map(|(room, s)| {
            let along = Vec2::new(-s.normal.y, s.normal.x) * 0.25;
            ray_quad(origin, dir, s.pos - along, s.pos + along, h - 0.3, h + 0.3).map(|t| (Target::Switch(room, s.pos), t))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// How far round a built fire the pointer may land, and how high it is taken to be.
const FIRE_HALF: f32 = 0.55;
const FIRE_TOP: f32 = 0.5;

/// The built fire (`fires`: the snapshot's) the pointer's ray meets first, as a low box round
/// it, with the ray parameter there.
pub fn pick_fire(fires: &[zombye_sim::FireView], player: Vec2, origin: Vec3, dir: Vec3) -> Option<(Target, f32)> {
    let half = Vec2::splat(FIRE_HALF);
    fires
        .iter()
        .map(|f| (f, Vec2::from(f.pos)))
        .filter(|(_, at)| at.distance(player) <= LOOK)
        .filter_map(|(f, at)| ray_box(origin, dir, at - half, at + half, FIRE_TOP).map(|t| (Target::Fire(f.id, at, f.lit, f.fuel_min), t)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// How high a tap that isn't a container (a bath, a toilet) is taken to be.
const TAP_TOP: f32 = 0.8;

/// The tap (furniture of a kind in `water.taps` or `water.cisterns`, on the player's floor, not a container: a
/// sink is one already) the pointer's ray meets first, with the ray parameter there.
pub fn pick_tap(map: &TileMap, sites: &[ContainerSite], water: &zombye_sim::power::WaterDef, player: Vec2, level: u8, origin: Vec3, dir: Vec3) -> Option<(Target, f32)> {
    map.buildings
        .iter()
        .flat_map(|b| b.furniture.iter())
        .filter(|f| f.level == level as i8 && water.taps.iter().chain(&water.cisterns.kinds).any(|t| *t == f.kind))
        .filter(|f| !sites.iter().any(|s| s.min == f.min && s.level as i8 == f.level))
        .filter(|f| (f.min.as_vec2() + f.size.as_vec2() * 0.5).distance(player) <= LOOK)
        .filter_map(|f| {
            let kind: &'static str = match f.kind.as_str() {
                "bathtub" => "Bath",
                "toilet" => "Toilet",
                "sink" => "Sink",
                _ => "Tap",
            };
            ray_box(origin, dir, f.min.as_vec2(), (f.min + f.size).as_vec2(), TAP_TOP).map(|t| (Target::Tap(f.min, f.level, kind), t))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// What a fire's menu offers: put on each kind of fuel carried, light it (with something to
/// light it with), put it out. `reach`: `FireTuning::reach`.
pub fn fire_entries(id: u32, at: Vec2, lit: bool, fuel_min: u16, items: &[ItemDef], reach: f32, me: u32, pos: Vec2, view: Option<&PrivateView>) -> (String, Vec<WorldEntry>) {
    let fuel = zombye_sim::fire::hours_text(fuel_min as f32 / 60.0);
    let title = if lit { format!("Campfire (burning, {fuel} left)") } else if fuel_min > 0 { format!("Campfire (out, {fuel} of fuel)") } else { "Campfire (out, no fuel)".to_string() };
    if at.distance(pos) > reach {
        return (title, vec![WorldEntry::say("Too far away: walk up to it")]);
    }
    let fire = |op| Intent::Fire { player: me, fire: id, op };
    let has_tag = |s: &zombye_sim::ItemStack, tag: &str| items.get(s.item as usize).is_some_and(|d| d.has_tag(tag));
    let mut lines = Vec::new();
    if lit {
        lines.push(WorldEntry::send("Put it out", fire(zombye_sim::FireOp::PutOut)));
    } else if fuel_min > 0 {
        let can = view.is_some_and(|v| v.inventory.carried.iter().chain(v.inventory.hand.iter()).chain(v.inventory.off_hand.iter()).any(|s| has_tag(s, "fire")));
        lines.push(if can { WorldEntry::send("Light it", fire(zombye_sim::FireOp::Light)) } else { WorldEntry::say("Needs matches or a lighter to light") });
    }
    // One line per kind of fuel carried (the first stack of each).
    if let Some(v) = view {
        let mut seen: Vec<u16> = Vec::new();
        for (i, s) in v.inventory.carried.iter().enumerate() {
            if !has_tag(s, "fuel") || seen.contains(&s.item) {
                continue;
            }
            seen.push(s.item);
            let name = items.get(s.item as usize).map_or("fuel".to_string(), |d| d.name.to_lowercase());
            lines.push(WorldEntry::send(format!("Put on a {name}"), fire(zombye_sim::FireOp::AddFuel { index: i as u32 })));
        }
    }
    if lines.is_empty() {
        lines.push(WorldEntry::say("Nothing to burn on you: firewood, planks or newspaper"));
    }
    (title, lines)
}

/// What the menu on `target` is called and what it offers, the likeliest first, for player `me`
/// at `pos` (`view`: our private view, for what we carry). Out of reach it says so and offers
/// nothing.
#[allow(clippy::too_many_arguments)]
pub fn entries(target: Target, map: &TileMap, sites: &[ContainerSite], data: &ContainerData, items: &[ItemDef], builds: &[BuildDef], blind_tag: &str, fire_reach: f32, me: u32, pos: Vec2, view: Option<&PrivateView>) -> (String, Vec<WorldEntry>) {
    match target {
        Target::Edge(e) => edge_entries(e, map, items, builds, blind_tag, me, pos, view),
        Target::Container(k) => {
            let Some(s) = sites.get(k) else { return (String::new(), Vec::new()) };
            let name = data.containers.get(s.def as usize).map_or("container".to_string(), |d| d.name.clone());
            let near = s.distance(pos) <= zombye_sim::inventory::CONTAINER_REACH && map.tile(pos.floor().as_ivec2()).room == s.room;
            let lines = if near {
                let open = view.and_then(|v| v.open.as_ref()).is_some_and(|c| c.id == s.id);
                vec![WorldEntry::send(if open { "Look inside" } else { "Search" }, Intent::Search { player: me, container: Some(s.id) })]
            } else {
                vec![WorldEntry::say("Too far away: walk up to it")]
            };
            (name, lines)
        }
        Target::Body(id, at) => {
            let c = zombye_sim::corpses::corpse_container(id);
            let lines = if zombye_sim::corpses::body_in_reach(map, pos, [(id, at)]).is_some() {
                let open = view.and_then(|v| v.open.as_ref()).is_some_and(|o| o.id == c);
                vec![WorldEntry::send(if open { "Look inside" } else { "Search the body" }, Intent::Search { player: me, container: Some(c) })]
            } else {
                vec![WorldEntry::say("Too far away: walk up to it")]
            };
            ("Body".to_string(), lines)
        }
        Target::Fire(id, at, lit, fuel) => fire_entries(id, at, lit, fuel, items, fire_reach, me, pos, view),
        Target::Switch(room, at) => {
            let near = at.distance(pos) <= zombye_sim::power::SWITCH_REACH && map.room_at(pos) == room;
            let lines = if near { vec![WorldEntry::send("Flick the switch", Intent::Light { player: me, on: None })] } else { vec![WorldEntry::say("Too far away: walk up to it")] };
            ("Light switch".to_string(), lines)
        }
        // The fill lines are `tap_entries`', which the caller adds.
        Target::Tap(_, _, kind) => (kind.to_string(), Vec::new()),
    }
}

/// The toilet whose cistern the player can fill from here ([`zombye_sim::power::cistern_near`],
/// with the fills taken that the snapshot says).
pub fn cistern_here(map: &TileMap, water: &zombye_sim::power::WaterDef, snap: &zombye_net::Snapshot, pos: Vec2, level: u8) -> Option<u32> {
    let taken = |k: u32| snap.cisterns.iter().find(|(c, _)| *c == k).map_or(0, |(_, n)| *n);
    zombye_sim::power::cistern_near(map, water, snap.water_on, taken, pos, level)
}

/// A tap (a sink, a bath or a toilet, by its furniture's min corner and floor; `power.ron`'s
/// `water.taps` and `water.cisterns`): the recipes that need running water (`water: true`,
/// filling a bottle) while it runs, or once it's off a toilet's cistern (`cistern: true`, dirty
/// water), each a line that makes it, or says what's missing; none when the player isn't at it
/// or there's nothing to fill from (`power::water_near`, `cistern_near`: the server's rules).
#[allow(clippy::too_many_arguments)]
pub fn tap_entries(min: IVec2, floor: i8, map: &TileMap, water: &zombye_sim::power::WaterDef, snap: &zombye_net::Snapshot, recipes: &[zombye_sim::RecipeDef], items: &[ItemDef], me: u32, pos: Vec2, level: u8, view: Option<&PrivateView>) -> Vec<WorldEntry> {
    let kind = map.buildings.iter().flat_map(|b| b.furniture.iter()).find(|f| f.min == min && f.level == floor).map(|f| f.kind.as_str());
    let Some(kind) = kind else { return Vec::new() };
    let tap = water.taps.iter().any(|t| t == kind) && zombye_sim::power::water_near(map, water, snap.water_on, pos, level);
    let cistern = !tap && water.cisterns.kinds.iter().any(|t| t == kind) && cistern_here(map, water, snap, pos, level).is_some();
    if !tap && !cistern {
        return Vec::new();
    }
    let Some(v) = view else { return Vec::new() };
    recipes
        .iter()
        .enumerate()
        .filter(|(_, r)| if tap { r.water } else { r.cistern })
        .map(|(i, r)| match zombye_sim::crafting::check(r, &v.inventory, items) {
            Ok(()) => WorldEntry::send(r.name.clone(), Intent::Craft { player: me, recipe: i as u16 }),
            Err(why) => WorldEntry::say(format!("{}: {why}", r.name)),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn edge_entries(e: EdgeRef, map: &TileMap, items: &[ItemDef], builds: &[BuildDef], blind_tag: &str, me: u32, pos: Vec2, view: Option<&PrivateView>) -> (String, Vec<WorldEntry>) {
    let kind = map.owned_edge(e.tile, e.edge);
    let glazed = map.glazed(e);
    let title = match kind {
        Edge::DoorClosed | Edge::DoorOpen if glazed => "Glass door",
        Edge::DoorClosed | Edge::DoorOpen => "Door",
        Edge::DoorBroken => "Broken door",
        Edge::Window => "Window",
        Edge::WindowOpen => "Open window",
        Edge::WindowBroken => "Broken window",
        Edge::Fence => "Fence",
        Edge::WindowBoarded => "Boarded-up window",
        Edge::DoorBoarded => "Boarded-up door",
        _ => "",
    }
    .to_string();
    let to = e.mid() - pos;
    let facing = to.y.atan2(to.x);
    if to.length() > zombye_sim::world::INTERACT_REACH {
        return (title, vec![WorldEntry::say("Too far away: walk up to it")]);
    }
    let toward = |then: Intent| Intent::Toward { player: me, facing, then: Box::new(then) };
    let mut lines = Vec::new();
    match kind {
        Edge::DoorClosed => lines.push(("Open", Intent::Interact { player: me })),
        Edge::DoorOpen => lines.push(("Close", Intent::Interact { player: me })),
        Edge::Window => {
            lines.push(("Open", Intent::Interact { player: me }));
            lines.push(("Smash the glass", Intent::SmashWindow { player: me }));
        }
        Edge::WindowOpen => {
            lines.push(("Climb through", Intent::Climb { player: me }));
            lines.push(("Close", Intent::Interact { player: me }));
            lines.push(("Smash the glass", Intent::SmashWindow { player: me }));
        }
        Edge::WindowBroken => lines.push(("Climb through", Intent::Climb { player: me })),
        Edge::Fence => lines.push(("Climb over", Intent::Climb { player: me })),
        _ => {}
    }
    // Blinds (`zombye_sim::blinds`): handled from inside only.
    if map.takes_blind(e) && zombye_sim::blinds::on_inside(map, e, pos) {
        match map.blind(e) {
            Some(down) => {
                lines.insert(lines.len().min(1), (if down { "Raise the blind" } else { "Lower the blind" }, Intent::Blind { player: me, op: BlindOp::Toggle }));
                lines.push(("Take the blind down", Intent::Blind { player: me, op: BlindOp::TakeDown }));
            }
            None => {
                let has = view.is_some_and(|v| v.inventory.carried.iter().chain(v.inventory.hand.iter()).chain(v.inventory.off_hand.iter()).any(|s| items.get(s.item as usize).is_some_and(|d| d.tags.iter().any(|t| t == blind_tag))));
                if has {
                    lines.push(("Fit a blind", Intent::Blind { player: me, op: BlindOp::Fit }));
                }
            }
        }
    }
    let mut out: Vec<WorldEntry> = lines.into_iter().map(|(label, i)| WorldEntry { facing: Some(facing), ..WorldEntry::send(label, toward(i)) }).collect();
    // The build menu's pieces that go on this edge (`assets/data/build.ron`): board it up, or one
    // plank more; prise one off. What's missing is said instead, as the server would say it.
    let inv = view.map(|v| &v.inventory);
    for (i, d) in builds.iter().enumerate() {
        if !zombye_sim::build::fits(map, d, i as u16, e) {
            continue;
        }
        let label = if kind.is_boarded() { "Nail another plank on".to_string() } else { d.name.clone() };
        match inv.map(|inv| zombye_sim::build::check_build(d, inv, items, 1)) {
            Some(Err(why)) => out.push(WorldEntry::say(format!("{label}: {why}"))),
            _ => out.push(WorldEntry { facing: Some(facing), ..WorldEntry::send(label, toward(Intent::Build { player: me, build: d.id.clone() })) }),
        }
    }
    if let Some(b) = map.barricade(e) {
        let missing = builds.get(b.build as usize).zip(inv).and_then(|(d, inv)| zombye_sim::build::check_removal(d, inv, items).err());
        match missing {
            Some(why) => out.push(WorldEntry::say(format!("Prise a plank off: {why}"))),
            None => out.push(WorldEntry { facing: Some(facing), ..WorldEntry::send("Prise a plank off", toward(Intent::Unbuild { player: me })) }),
        }
    }
    if out.is_empty() {
        out.push(WorldEntry::say(if kind == Edge::DoorBroken { "Broken: it won't shut again" } else { "Nothing to do with it" }));
    }
    (title, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ray from far up and back, as the camera looks, through the point `at` (world space).
    fn ray_through(at: Vec3) -> (Vec3, Vec3) {
        let dir = Vec3::new(0.61, -0.5, 0.61).normalize();
        (at - dir * 60.0, dir)
    }

    #[test]
    fn the_ray_meets_a_door_in_its_opening_and_not_beside_it() {
        // A north edge along x = 10..11 at z = 5.
        let (a, b) = (Vec2::new(10.0, 5.0), Vec2::new(11.0, 5.0));
        let (o, d) = ray_through(Vec3::new(10.5, 1.2, 5.0));
        assert!(ray_quad(o, d, a, b, 0.0, DOOR_TOP).is_some(), "the middle of the door");
        let (o, d) = ray_through(Vec3::new(12.0, 1.2, 5.0));
        assert!(ray_quad(o, d, a, b, 0.0, DOOR_TOP).is_none(), "a metre beside it");
        let (o, d) = ray_through(Vec3::new(10.5, 0.5, 5.0));
        assert!(ray_quad(o, d, a, b, SILL, DOOR_TOP).is_none(), "under a window's sill");
    }

    #[test]
    fn the_ray_meets_furniture_and_the_nearer_thing_wins() {
        let (o, d) = ray_through(Vec3::new(3.5, 0.5, 3.5));
        let box_t = ray_box(o, d, Vec2::new(3.0, 3.0), Vec2::new(4.0, 4.0), FURNITURE_TOP).expect("on the box");
        assert!(ray_box(o, d, Vec2::new(8.0, 8.0), Vec2::new(9.0, 9.0), FURNITURE_TOP).is_none());
        let ground_t = -o.y / d.y;
        assert!(box_t < ground_t, "the box's top is met before the ground under it");
    }

    #[test]
    fn the_ray_finds_a_body_on_the_ground() {
        let (o, d) = ray_through(Vec3::new(20.2, 0.1, 7.9));
        let bodies = [(4, Vec2::new(20.0, 8.0)), (9, Vec2::new(26.0, 8.0))];
        assert_eq!(pick_body(bodies, Vec2::new(19.0, 8.0), o, d).map(|(t, _)| t), Some(Target::Body(4, Vec2::new(20.0, 8.0))));
        let (o, d) = ray_through(Vec3::new(23.0, 0.0, 8.0));
        assert!(pick_body(bodies, Vec2::new(19.0, 8.0), o, d).is_none(), "between the two");
    }

    #[test]
    fn the_ray_finds_a_light_switch_on_its_wall() {
        use zombye_sim::map::Side;
        // A switch on the north wall of tile (10, 5), facing south into the room.
        let spot = zombye_sim::power::SwitchSpot { pos: Vec2::new(10.5, 5.1), normal: Vec2::Y, tile: IVec2::new(10, 5), side: Side::N };
        let spots = [None, Some(spot)];
        let (o, d) = ray_through(Vec3::new(10.5, zombye_sim::power::SWITCH_HEIGHT, 5.1));
        assert_eq!(pick_switch(&spots, Vec2::new(10.5, 6.0), o, d).map(|(t, _)| t), Some(Target::Switch(1, spot.pos)));
        let (o, d) = ray_through(Vec3::new(12.0, zombye_sim::power::SWITCH_HEIGHT, 5.1));
        assert!(pick_switch(&spots, Vec2::new(10.5, 6.0), o, d).is_none(), "a metre and a half along the wall");
    }
}

/// What a car's menu offers (E held beside it or at its wheel, stage 4): get in or out, the lock,
/// smashing in, the boot and glovebox, hotwiring and the key, pouring and siphoning fuel, and the
/// repairs that would do anything, each saying what's missing. The server checks it all again.
pub fn car_entries(v: &zombye_sim::VehicleView, data: &zombye_sim::VehicleData, items: &[ItemDef], me: u32, view: Option<&PrivateView>) -> (String, Vec<WorldEntry>) {
    use zombye_sim::VehicleOp as Op;
    let name = data.kinds.get(v.kind as usize).map_or_else(|| "Car".to_string(), |k| capitalise(&k.label()));
    let w = &data.work;
    let inside = v.driver == me && me != 0;
    let vehicle = Some(v.id);
    let send = |label: &str, op: Op| WorldEntry::send(label, Intent::Vehicle { player: me, op });
    let key = items.iter().position(|d| d.id == w.key_item).map(|i| i as u16);
    let has_key = view.zip(key).is_some_and(|(p, k)| zombye_sim::vehicles::holds_key(&p.inventory, k, v.id));
    let has_tag = |tag: &str| view.is_some_and(|p| p.inventory.stacks().any(|s| items.get(s.item as usize).is_some_and(|d| d.has_tag(tag))));
    let search = |label: &str, slot: u8| WorldEntry::send(label, Intent::Search { player: me, container: Some(zombye_sim::vehicles::vehicle_container(v.id, slot)) });
    let mut lines = Vec::new();
    if inside {
        lines.push(send("Get out", Op::Exit));
        if !v.start {
            lines.push(if has_tag(&w.hotwire_tag) { send("Hotwire it", Op::Hotwire) } else { WorldEntry::say("No key: a screwdriver would cross the wires") });
        }
        if v.key_in {
            lines.push(send("Take the key", Op::TakeKey));
        }
        lines.push(send(if v.locked { "Unlock the doors" } else { "Lock the doors" }, Op::Lock { vehicle }));
        lines.push(search("Open the glovebox", zombye_sim::vehicles::GLOVEBOX));
    } else {
        if v.driver == 0 {
            lines.push(send("Get in", Op::Enter { vehicle }));
        }
        if v.locked {
            if has_key {
                lines.push(send("Unlock it", Op::Lock { vehicle }));
            }
            lines.push(send("Smash a window to get in", Op::BreakIn { vehicle }));
        } else {
            if has_key {
                lines.push(send("Lock it", Op::Lock { vehicle }));
            }
            lines.push(search("Open the boot", zombye_sim::vehicles::BOOT));
            lines.push(search("Open the glovebox", zombye_sim::vehicles::GLOVEBOX));
        }
        let can = items.iter().position(|d| d.id == data.tuning.fuel_item).map(|i| i as u16);
        if view.zip(can).is_some_and(|(p, c)| p.inventory.stacks().any(|s| s.item == c)) {
            lines.push(send("Pour in fuel", Op::Refuel { vehicle }));
            if has_tag(&data.tuning.siphon_tag) && v.fuel_dl > 0 {
                lines.push(send("Siphon fuel out", Op::Siphon { vehicle }));
            }
        }
    }
    // Repairs that would mend something (from inside, only the windows).
    let level = view.and_then(|p| p.skills.iter().find(|s| s.skill == zombye_sim::skills::Skill::Mechanics)).map_or(0, |s| s.level);
    for (k, r) in w.repairs.iter().enumerate() {
        let now = v.parts[r.part.index()] as f32 / 100.0;
        if now >= r.up_to - 1e-3 || (inside && r.part != zombye_sim::vehicles::Part::Windows) {
            continue;
        }
        let label = format!("{} ({} {}%)", r.name, r.part.label(), v.parts[r.part.index()]);
        let missing = if level < r.level {
            Some(format!("needs Mechanics {}", r.level))
        } else {
            view.and_then(|p| zombye_sim::build::check_have(&r.inputs, &r.tools, &p.inventory, items, 1).err())
        };
        lines.push(match missing {
            None => send(&label, Op::Repair { vehicle, repair: k as u8 }),
            Some(why) => WorldEntry::say(format!("{label}: {why}")),
        });
    }
    (name, lines)
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().collect::<String>() + c.as_str())
}
