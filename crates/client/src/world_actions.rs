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
use zombye_sim::{BlindOp, ItemDef};

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

    fn say(label: impl Into<String>) -> Self {
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
    kind.is_door() || kind.is_window() || kind == Edge::DoorBroken || kind == Edge::Fence
}

/// The thing the pointer's ray (from `origin` along `dir`) meets first near the player at
/// `player`: a door, window or fence (its opening, or the ground just by it when the wall is cut
/// down), or a container's furniture. With the ray parameter where it meets it, so a caller can
/// weigh it against what else is under the pointer.
pub fn pick(map: &TileMap, sites: &[ContainerSite], player: Vec2, origin: Vec3, dir: Vec3) -> Option<(Target, f32)> {
    if dir.y.abs() < 1e-4 {
        return None;
    }
    let ground_t = -origin.y / dir.y;
    let ground = origin + dir * ground_t;
    let ground = Vec2::new(ground.x, ground.z);
    let mut best: Option<(Target, f32)> = None;
    let mut take = |t: Target, at: f32| {
        if best.is_none_or(|(_, b)| at < b) {
            best = Some((t, at));
        }
    };
    // Edges round where the pointer is on the ground: what stands in front of it on screen is
    // within a few metres of there (the camera looks down at 30 degrees and more).
    let c = ground.floor().as_ivec2();
    for dy in -4..=4 {
        for dx in -4..=4 {
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

/// What the menu on `target` is called and what it offers, the likeliest first, for player `me`
/// at `pos` (`view`: our private view, for what we carry). Out of reach it says so and offers
/// nothing.
pub fn entries(target: Target, map: &TileMap, sites: &[ContainerSite], data: &ContainerData, items: &[ItemDef], blind_tag: &str, me: u32, pos: Vec2, view: Option<&PrivateView>) -> (String, Vec<WorldEntry>) {
    match target {
        Target::Edge(e) => edge_entries(e, map, items, blind_tag, me, pos, view),
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
        Target::Switch(room, at) => {
            let near = at.distance(pos) <= zombye_sim::power::SWITCH_REACH && map.room_at(pos) == room;
            let lines = if near { vec![WorldEntry::send("Flick the switch", Intent::Light { player: me, on: None })] } else { vec![WorldEntry::say("Too far away: walk up to it")] };
            ("Light switch".to_string(), lines)
        }
    }
}

fn edge_entries(e: EdgeRef, map: &TileMap, items: &[ItemDef], blind_tag: &str, me: u32, pos: Vec2, view: Option<&PrivateView>) -> (String, Vec<WorldEntry>) {
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
                lines.insert(0, (if down { "Raise the blind" } else { "Lower the blind" }, Intent::Blind { player: me, op: BlindOp::Toggle }));
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
