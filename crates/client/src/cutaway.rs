//! Camera cutaways: which walls drop to stubs so the player and the room they're in can be seen,
//! decided per building and per wall edge, never by a radius round the player.
//!
//! - **Inside a building:** every outside wall of that building that faces the camera drops, and
//!   so does any wall (of that building) that hides part of the room the player is in: the
//!   room's own near walls and inner walls between it and the camera. The far walls and the rest
//!   stand. Its roof and upper storeys are hidden (`world_render::hide_upper_floors`).
//! - **Never a wall the player stands in front of** ([`in_front`]: on the camera's side of its
//!   line): it is behind them on screen and hides nothing they need, so it stands at its full
//!   height, whatever the two rules above say (an alcove's wall, the leg of an L-shaped room,
//!   the camera-side wall of the next wing).
//! - **Anywhere:** a wall piece that covers the player's body from the camera drops, whichever
//!   building it belongs to. Nothing else of that building changes: it keeps its roof.
//! - What still covers the player after that (a roof, an upper storey, walls with the cutaways
//!   switched off) is said in [`Cutaway::occluded`], which shows the player's silhouette
//!   (`silhouette.rs`).
//!
//! The decision is plain functions over the map ([`room_edges`], [`body_edges`]). The plugin eases
//! each edge's cut amount over [`EASE_SECS`] and writes the amounts to a small texture the world
//! shader reads (one texel a tile: red its north edge, green its west edge), so a wall sinks to
//! its stub as one piece with a straight top. Door leaves and window panes read the same amounts
//! on the CPU ([`Cutaway::amount`]). Nothing is allocated per frame: the lists keep their room,
//! the room's set is worked out again only when the room or the camera's angle changes, and the
//! texture is uploaded only on frames where an amount moved.
//!
//! F10 switches the cutaways off and on ("walls up" / "walls cut").

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use zombye_sim::map::{Edge, OwnedEdge, RoofStyle, TileMap};

use crate::camera::IsoCamera;
use crate::player::Player;
use crate::server_link::ClientMap;

/// The cut texture is this many texels a side, one a tile, addressed by the tile's coordinates
/// modulo this (`CUT_TEXELS` in world.wesl). Only edges near the player are ever cut, and the
/// shader ignores anchors further than `CUT_NEAR` from the player, so the wrap is never seen.
pub const TEXELS: i32 = 128;
/// Seconds for a wall to sink to its stub or stand up again.
pub const EASE_SECS: f32 = 0.15;
/// How far behind a wall (on the ground, away from the camera) it can hide the floor: a 3 m wall
/// seen from 30° up covers about 5.2 m.
pub const WALL_REACH: f32 = 5.5;
/// An outside wall faces the camera when its outward normal has at least this much of the
/// camera's ground direction (0.71 for the two sides that face a fixed angle; in the middle of a
/// turn only the side the camera is square to).
const FACING: f32 = 0.1;
/// The player's body for the cut: half its width, and the top of its head.
pub const BODY_HALF: f32 = 0.3;
pub const BODY_TOP: f32 = 1.85;
/// A wall this close in front of the body's middle line (metres toward the camera) is its own
/// wall behind it, not one that covers it.
const BODY_CLEAR: f32 = 0.05;
/// Storey height (as `world_render::STOREY`).
const STOREY: f32 = 3.0;

/// A wall edge: the tile that owns it and which of its two edges.
pub type EdgeKey = (IVec2, OwnedEdge);

/// Does this edge carry wall pieces that can drop (a wall, or the wall round a window, a door or
/// an arch)?
pub fn stands(kind: Edge) -> bool {
    !matches!(kind, Edge::None | Edge::Fence)
}

/// The camera's direction on the ground (unit, toward the camera) and the tangent of its elevation.
pub fn ground_dir(camera_dir: Vec3) -> (Vec2, f32) {
    let flat = camera_dir.xz();
    let len = flat.length().max(1e-4);
    (flat / len, camera_dir.y / len)
}

/// Does the wall on this edge hide part of `room`'s floor from the camera? The floor behind it on
/// screen (away from the camera, as far as a wall's height covers) runs into the room.
pub fn hides_room(map: &TileMap, key: EdgeKey, c: Vec2, room: u16, level: u8) -> bool {
    let (a, b) = TileMap::edge_segment(key.0, key.1);
    let along = (b - a) * 0.45;
    let mid = (a + b) * 0.5;
    for s in [mid, mid - along, mid + along] {
        let mut t = 0.25;
        while t <= WALL_REACH {
            if map.room_on(level, s - c * t) == room {
                return true;
            }
            t += 0.25;
        }
    }
    false
}

/// A wall counts as further from the camera than the body when all of it is at least this far
/// (metres on the ground, away from the camera) behind the body's middle.
const BEHIND_CLEAR: f32 = 0.25;

/// Is a body at `pos` in front of this edge's wall as the camera sees it? Either it is on the
/// camera's side of the wall's line (by more than a hair), or the whole piece is further from the
/// camera than the body (up the screen from it): a wall beside the body that runs on away from
/// the camera. Such a wall is behind the body on screen, and stands.
pub fn in_front(key: EdgeKey, pos: Vec2, c: Vec2) -> bool {
    let (a, b) = TileMap::edge_segment(key.0, key.1);
    let side = if key.1 == OwnedEdge::North { (pos.y - a.y) * c.y.signum() } else { (pos.x - a.x) * c.x.signum() };
    side > BODY_CLEAR || a.dot(c).max(b.dot(c)) < pos.dot(c) - BEHIND_CLEAR
}

/// The walls of the building the player is inside that drop: its outside walls facing the camera,
/// and any of its walls hiding part of the player's room. Nothing when the player is outdoors.
/// Depends only on the building, the room and the camera's angle (not on where in the room).
///
/// `level` 1 (stairs): the same for the first upper floor's own walls, when that is the floor shown.
pub fn room_edges(map: &TileMap, player: Vec2, level: u8, camera_dir: Vec3, out: &mut Vec<EdgeKey>) {
    let room = map.room_on(level, player);
    let Some(index) = map.building_at(player) else { return };
    let (c, _) = ground_dir(camera_dir);
    let b = &map.buildings[index];
    let belongs = |r: u16| r != 0 && map.rooms[r as usize].building as usize == index;
    for y in b.min.y..=b.min.y + b.size.y {
        for x in b.min.x..=b.min.x + b.size.x {
            let owner = IVec2::new(x, y);
            // The tile's north and west edges and its room, on the floor asked for.
            let at = |t: IVec2| {
                if level == 0 {
                    let tile = map.tile(t);
                    (tile.north, tile.west, tile.room)
                } else {
                    map.upper_tile(t).map_or((Edge::None, Edge::None, 0), |u| (u.north, u.west, u.room))
                }
            };
            let (north, west, own_room) = at(owner);
            // `toward`: from the edge's north / west side to its south / east side.
            for (which, kind, other, toward) in [(OwnedEdge::North, north, owner - IVec2::Y, Vec2::Y), (OwnedEdge::West, west, owner - IVec2::X, Vec2::X)] {
                if !stands(kind) {
                    continue;
                }
                let (first, second) = (belongs(at(other).2), belongs(own_room));
                if !(first || second) {
                    continue;
                }
                // An outside wall: the building on one side only. Its outward normal points away from it.
                let faces = first != second && (if first { toward } else { -toward }).dot(c) > FACING;
                if faces || hides_room(map, (owner, which), c, room, level) {
                    out.push((owner, which));
                }
            }
        }
    }
}

/// Where a wall edge crosses the strip of ground between a body at `pos` and the camera: the
/// range of distances toward the camera (metres on the ground) over which the edge lies within
/// `half` of the body's middle line, if it does at all.
fn crossing(key: EdgeKey, pos: Vec2, c: Vec2, half: f32) -> Option<(f32, f32)> {
    let (a, b) = TileMap::edge_segment(key.0, key.1);
    let perp = Vec2::new(-c.y, c.x);
    let (a, b) = (a - pos, b - pos);
    // (toward the camera, across) for both ends; clip the edge to |across| <= half.
    let (a0, l0, a1, l1) = (a.dot(c), a.dot(perp), b.dot(c), b.dot(perp));
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let dl = l1 - l0;
    if dl.abs() < 1e-5 {
        if l0.abs() > half {
            return None;
        }
    } else {
        let (u, v) = ((-half - l0) / dl, (half - l0) / dl);
        t0 = t0.max(u.min(v));
        t1 = t1.min(u.max(v));
        if t0 > t1 {
            return None;
        }
    }
    let (p, q) = (a0 + (a1 - a0) * t0, a0 + (a1 - a0) * t1);
    Some((p.min(q), p.max(q)))
}

/// Every standing wall edge that covers a body at `pos` from the camera, whichever building it is
/// part of: the edge crosses the strip of ground between the body and the camera within the reach
/// of a wall's height. The wall right behind a body (the facade it walks along) doesn't.
pub fn body_edges(map: &TileMap, pos: Vec2, camera_dir: Vec3, mut found: impl FnMut(EdgeKey)) {
    let (c, _) = ground_dir(camera_dir);
    let end = pos + c * WALL_REACH;
    let lo = (pos.min(end) - 1.0).floor().as_ivec2();
    let hi = (pos.max(end) + 1.0).ceil().as_ivec2();
    for y in lo.y..=hi.y {
        for x in lo.x..=hi.x {
            let owner = IVec2::new(x, y);
            let tile = map.tile(owner);
            for (which, kind) in [(OwnedEdge::North, tile.north), (OwnedEdge::West, tile.west)] {
                if stands(kind) && crossing((owner, which), pos, c, BODY_HALF).is_some_and(|(near, far)| far > BODY_CLEAR && near < WALL_REACH) {
                    found((owner, which));
                }
            }
        }
    }
}

/// The open or broken window a body at `pos` is climbing through (within `CLIMB_ZONE` of it, as
/// `TileMap::window_opening`), as the edge that owns it.
pub fn crossed_window(map: &TileMap, pos: Vec2) -> Option<EdgeKey> {
    use zombye_sim::map::CLIMB_ZONE;
    let t = pos.floor().as_ivec2();
    let f = pos - t.as_vec2();
    [
        (f.y, t, OwnedEdge::North),
        (f.x, t, OwnedEdge::West),
        (1.0 - f.y, t + IVec2::Y, OwnedEdge::North),
        (1.0 - f.x, t + IVec2::X, OwnedEdge::West),
    ]
    .into_iter()
    .find(|&(d, owner, which)| {
        let tile = map.tile(owner);
        let e = if which == OwnedEdge::North { tile.north } else { tile.west };
        d < CLIMB_ZONE && e.is_window() && e.is_climbable()
    })
    .map(|(_, owner, which)| (owner, which))
}

/// Is a body at `pos` covered from the camera by the bulk of a building other than `inside` (whose
/// roof and upper storeys are hidden): its roof or an upper storey? Walls alone don't count: the
/// ones that cover the body drop. Walked along the line from the body toward the camera for as
/// long as the line from the feet is still below the tallest building.
pub fn bulk_hides(map: &TileMap, pos: Vec2, camera_dir: Vec3, inside: Option<usize>) -> bool {
    let (c, tan) = ground_dir(camera_dir);
    let mut t = 0.5;
    loop {
        let (feet, head) = (t * tan, BODY_TOP + t * tan);
        if feet > STOREY * 2.0 + ROOF_RISE {
            return false;
        }
        if let Some(i) = map.building_at(pos + c * t).filter(|i| Some(*i) != inside) {
            let b = &map.buildings[i];
            let top = STOREY * b.levels as f32 + if matches!(b.roof, RoofStyle::Flat) { 0.3 } else { ROOF_RISE };
            // Above the ground-floor walls by here, and not yet over the roof.
            if head > STOREY && feet < top {
                return true;
            }
        }
        t += 0.5;
    }
}

/// About how far a pitched roof's ridge stands over its eaves.
const ROOF_RISE: f32 = 2.5;

/// How much a building detail standing between the camera and the player is thinned out (0 not at
/// all, 1 as far as it goes): a porch roof, an awning, a canopy, a porch post, each one whole.
/// `anchor` is its middle on the ground and `radius` its half-width. It covers the body when it
/// stands in the strip between the body and the camera, no further toward the camera than its
/// height reaches. `object_fade` in world.wesl is the same rule; the edges are soft in space, so
/// it thins and thickens smoothly as the player walks. (The shader draws by its own copy; this one
/// is the rule in a form the tests can hold.)
#[cfg(test)]
pub fn object_fade(anchor: Vec2, radius: f32, player: Vec2, camera_dir: Vec3) -> f32 {
    let (c, tan) = ground_dir(camera_dir);
    let d = anchor - player;
    let (along, across) = (d.dot(c), (d.x * c.y - d.y * c.x).abs());
    let reach = FADE_TOP / tan.max(1e-3) + radius;
    let side = 1.0 - smoothstep(radius + BODY_HALF, radius + BODY_HALF + FADE_SOFT, across);
    let near = smoothstep(-radius - FADE_SOFT, -radius, along) * (1.0 - smoothstep(reach, reach + FADE_SOFT, along));
    side * near
}

/// The height the tallest of those details reaches (`FADE_TOP` in world.wesl), and the width of
/// the soft edge of the rule.
#[cfg(test)]
const FADE_TOP: f32 = 2.8;
#[cfg(test)]
const FADE_SOFT: f32 = 0.5;

#[cfg(test)]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// An edge on its way down or up.
struct Moving {
    key: EdgeKey,
    amount: f32,
    wanted: bool,
}

/// The cutaways' state: which edges are cut how far, the texture the shader reads them from, and
/// whether the player is still covered.
#[derive(Resource)]
pub struct Cutaway {
    /// `TEXELS` x `TEXELS`, Rg8: red = how far the tile's north edge is cut, green = its west edge.
    pub image: Handle<Image>,
    /// "Walls up": nothing is cut (F10).
    pub walls_up: bool,
    /// The first-person view holds every wall up too (`set_held`).
    held: bool,
    /// Something still covers the player from the camera: a roof, an upper storey, or (walls up) a wall.
    pub occluded: bool,
    /// The building the player is inside (its roof and upper storeys are hidden).
    pub inside: Option<usize>,
    moving: Vec<Moving>,
    values: Vec<u8>,
    /// The building's walls that drop for the room the player is in, and what they were worked out for.
    room: Vec<EdgeKey>,
    room_for: Option<(usize, u16, i32, u8)>,
    /// The floor whose walls the texture cuts: 0 the ground floor, 1 the first upper floor while
    /// the player is up there (stairs). The shader cuts that storey's walls only.
    pub level: u8,
    /// Those of them the player isn't in front of ([`in_front`]), this frame and last: the ones
    /// that drop.
    behind: Vec<EdgeKey>,
    behind_was: Vec<EdgeKey>,
    /// The walls covering the body, this frame and last.
    body: Vec<EdgeKey>,
    body_was: Vec<EdgeKey>,
    /// The window the player is climbing through: it stays whole whatever else would cut it, so the
    /// frame he hauls himself over doesn't sink away under him.
    keep: Option<EdgeKey>,
    /// Nothing decided yet: the first decision is taken at once, not eased (a picture of a save).
    fresh: bool,
}

impl Cutaway {
    fn new(image: Handle<Image>, walls_up: bool) -> Self {
        Self {
            image,
            walls_up,
            held: false,
            occluded: false,
            inside: None,
            moving: Vec::with_capacity(256),
            values: vec![0; (TEXELS * TEXELS * 2) as usize],
            room: Vec::with_capacity(256),
            room_for: None,
            level: 0,
            behind: Vec::with_capacity(256),
            behind_was: Vec::with_capacity(256),
            body: Vec::with_capacity(16),
            body_was: Vec::with_capacity(16),
            keep: None,
            fresh: true,
        }
    }

    fn texel(key: EdgeKey) -> usize {
        let (x, y) = (key.0.x.rem_euclid(TEXELS), key.0.y.rem_euclid(TEXELS));
        ((y * TEXELS + x) * 2) as usize + if key.1 == OwnedEdge::North { 0 } else { 1 }
    }

    /// How far this edge's wall is cut, 0 standing to 1 a stub, eased (as the shader draws it).
    pub fn amount(&self, key: EdgeKey) -> f32 {
        let a = self.values[Self::texel(key)] as f32 / 255.0;
        a * a * (3.0 - 2.0 * a)
    }

    /// Every edge that is cut at all now, with how far (as [`Cutaway::amount`]).
    pub fn cut_edges(&self) -> impl Iterator<Item = (EdgeKey, f32)> + '_ {
        self.moving.iter().filter(|m| m.amount > 0.0).map(|m| (m.key, self.amount(m.key)))
    }

    /// Take the edges wanted now (`behind` and `body`) into the moving list.
    fn retarget(&mut self) {
        for m in &mut self.moving {
            m.wanted = false;
        }
        if self.walls_up || self.held {
            return;
        }
        for i in 0..self.behind.len() + self.body.len() {
            let key = if i < self.behind.len() { self.behind[i] } else { self.body[i - self.behind.len()] };
            if Some(key) == self.keep {
                continue;
            }
            match self.moving.iter_mut().find(|m| m.key == key) {
                Some(m) => m.wanted = true,
                None => self.moving.push(Moving { key, amount: 0.0, wanted: true }),
            }
        }
    }

    /// Move every edge toward where it's wanted by `step` (1 = all the way). True if any moved.
    fn ease(&mut self, step: f32) -> bool {
        let mut moved = false;
        for m in &mut self.moving {
            let to = if m.wanted { 1.0 } else { 0.0 };
            if m.amount != to {
                m.amount = if m.wanted { (m.amount + step).min(1.0) } else { (m.amount - step).max(0.0) };
                self.values[Self::texel(m.key)] = (m.amount * 255.0).round() as u8;
                moved = true;
            }
        }
        self.moving.retain(|m| m.wanted || m.amount > 0.0);
        moved
    }

    /// Decide the cut for a player at `pos` and move toward it by `dt` seconds. True if the texture
    /// needs sending again.
    pub fn update(&mut self, map: &TileMap, pos: Vec2, level: u8, camera_dir: Vec3, dt: f32) -> bool {
        let inside = map.building_at(pos);
        self.inside = inside;
        // Upstairs only inside a building that has an upstairs here.
        let level = if inside.is_some() && map.room_on(level.min(1), pos) != 0 { level.min(1) } else { 0 };
        // The room's set: only when the room, the floor or the camera's angle changes.
        let angle = (camera_dir.x.atan2(camera_dir.z) * 200.0).round() as i32;
        let room_for = inside.map(|b| (b, map.room_on(level, pos), angle, level));
        let mut changed = self.fresh || level != self.level;
        self.level = level;
        if room_for != self.room_for {
            self.room_for = room_for;
            self.room.clear();
            room_edges(map, pos, level, camera_dir, &mut self.room);
            changed = true;
        }
        // Of those, never a wall the player stands in front of.
        std::mem::swap(&mut self.behind, &mut self.behind_was);
        self.behind.clear();
        let (c, _) = ground_dir(camera_dir);
        let (room, behind) = (&self.room, &mut self.behind);
        behind.extend(room.iter().copied().filter(|key| !in_front(*key, pos, c)));
        changed |= self.behind != self.behind_was;
        std::mem::swap(&mut self.body, &mut self.body_was);
        self.body.clear();
        // Upstairs nothing on the ground covers the body, and no window is climbed through.
        let mut keep = None;
        if level == 0 {
            let body = &mut self.body;
            body_edges(map, pos, camera_dir, |key| body.push(key));
            keep = crossed_window(map, pos);
        }
        changed |= self.body != self.body_was;
        changed |= keep != self.keep;
        self.keep = keep;
        if changed {
            self.retarget();
        }
        // Still covered: by a roof or an upper storey, or by a wall that would have dropped.
        let kept_covers = keep.is_some_and(|k| self.body.contains(&k));
        self.occluded = level == 0 && (bulk_hides(map, pos, camera_dir, inside) || (self.walls_up && !self.body.is_empty()) || kept_covers);
        let step = if self.fresh { 1.0 } else { dt / EASE_SECS };
        self.fresh = false;
        self.ease(step)
    }

    /// Hold every wall up while `held` (the first-person view: its camera stays inside the room),
    /// whatever the walls-up switch says.
    pub fn set_held(&mut self, held: bool) {
        if self.held != held {
            self.held = held;
            self.retarget();
        }
    }

    /// Switch the cutaways off or on.
    pub fn set_walls_up(&mut self, up: bool) {
        if self.walls_up != up {
            self.walls_up = up;
            self.retarget();
        }
    }
}

pub struct CutawayPlugin;

impl Plugin for CutawayPlugin {
    fn build(&self, app: &mut App) {
        let mut image = Image::new(
            Extent3d { width: TEXELS as u32, height: TEXELS as u32, depth_or_array_layers: 1 },
            TextureDimension::D2,
            vec![0; (TEXELS * TEXELS * 2) as usize],
            TextureFormat::Rg8Unorm,
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        );
        image.sampler = ImageSampler::nearest();
        let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        let walls_up = app.world().get_resource::<crate::Options>().is_some_and(|o| o.walls_up);
        app.insert_resource(Cutaway::new(handle, walls_up)).add_systems(Update, (toggle, decide).chain().in_set(CutawaySystems));
    }
}

/// The systems that decide the cut: whatever draws by it runs after.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CutawaySystems;

/// Ctrl, or Cmd on a Mac.
pub const CTRL_KEYS: [KeyCode; 4] = [KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::SuperLeft, KeyCode::SuperRight];

/// F10: walls up, or cut again. Shift+F10 is the zombies' silhouettes (`silhouette.rs`), Ctrl+F10
/// the opening hints (`opening_hints.rs`).
fn toggle(keys: Res<ButtonInput<KeyCode>>, time: Res<Time>, mut cutaway: ResMut<Cutaway>, notice: Option<ResMut<crate::saving::Notice>>) {
    if !keys.just_pressed(KeyCode::F10) || keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) || keys.any_pressed(CTRL_KEYS) {
        return;
    }
    let up = !cutaway.walls_up;
    cutaway.set_walls_up(up);
    if let Some(mut notice) = notice {
        notice.say(time.elapsed_secs(), if up { "Walls up (F10 cuts them away again)" } else { "Walls cut away (F10 stands them up)" }.to_string());
    }
}

fn decide(map: Res<ClientMap>, time: Res<Time>, view: Res<crate::player::ViewFloor>, mut cutaway: ResMut<Cutaway>, mut images: ResMut<Assets<Image>>, player: Query<&Transform, With<Player>>, cams: Query<&IsoCamera>) {
    let (Ok(p), Ok(cam)) = (player.single(), cams.single()) else { return };
    let cutaway = cutaway.as_mut();
    // First person: the camera is in the room with the survivor, so nothing is cut.
    cutaway.set_held(cam.shoulder());
    let changed = cutaway.update(&map.0, p.translation.xz(), view.level, cam.offset_dir(), time.delta_secs());
    if cam.shoulder() {
        // First person: no silhouette (it goes by the fixed view's depth, and the camera is
        // right behind the survivor).
        cutaway.occluded = false;
    }
    if !changed {
        return;
    }
    if let Some(mut image) = images.get_mut(&cutaway.image)
        && let Some(data) = image.data.as_mut()
    {
        data.copy_from_slice(&cutaway.values);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use zombye_sim::map::{Building, Room, Side};

    /// The camera at one of its four angles: 0 looks from the south-east (+x, +z), 30° up.
    pub(crate) fn camera(angle: i32) -> Vec3 {
        let yaw = std::f32::consts::FRAC_PI_4 + std::f32::consts::FRAC_PI_2 * angle as f32;
        let elev = 30f32.to_radians();
        Vec3::new(yaw.sin() * elev.cos(), elev.sin(), yaw.cos() * elev.cos())
    }

    /// A building of rooms given as rectangles (min, size), walls wherever rooms differ.
    fn add_building(map: &mut TileMap, rooms: &[(IVec2, IVec2)], levels: u8) -> usize {
        let index = map.buildings.len();
        let (mut lo, mut hi) = (IVec2::MAX, IVec2::MIN);
        for (min, size) in rooms {
            let id = map.rooms.len() as u16;
            map.rooms.push(Room { building: index as u16, name: format!("room {id}"), room_type: "living".into(), floor: Default::default() });
            for y in min.y..min.y + size.y {
                for x in min.x..min.x + size.x {
                    map.tile_mut(IVec2::new(x, y)).unwrap().room = id;
                }
            }
            lo = lo.min(*min);
            hi = hi.max(*min + *size);
        }
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let t = IVec2::new(x, y);
                for side in [Side::N, Side::W] {
                    if map.tile(t).room != map.tile(t + side.offset()).room {
                        map.set_edge(t, side, Edge::Wall);
                    }
                }
            }
        }
        map.buildings.push(Building {
            template: "test".into(),
            min: lo,
            size: hi - lo,
            levels,
            upper_floors: Vec::new(),
            upper_edges: Vec::new(),
            furniture: Vec::new(),
            roof: RoofStyle::GableX,
            siding: 0,
            roof_color: 0,
            look: None,
        });
        index
    }

    /// Two houses: A at (10, 10), 8 x 6, a west room (4 x 6) and an east room (4 x 6); B at
    /// (30, 10), 6 x 6, one room, two storeys.
    pub(crate) fn town() -> TileMap {
        let mut map = TileMap::new(4, 4);
        if map.rooms.is_empty() {
            map.rooms.push(Room { building: 0, name: "outdoors".into(), room_type: "outdoors".into(), floor: Default::default() });
        }
        add_building(&mut map, &[(IVec2::new(10, 10), IVec2::new(4, 6)), (IVec2::new(14, 10), IVec2::new(4, 6))], 1);
        add_building(&mut map, &[(IVec2::new(30, 10), IVec2::new(6, 6))], 2);
        map
    }

    fn room_set(map: &TileMap, player: Vec2, angle: i32) -> Vec<EdgeKey> {
        let mut out = Vec::new();
        room_edges(map, player, 0, camera(angle), &mut out);
        out
    }

    fn body_set(map: &TileMap, player: Vec2, angle: i32) -> Vec<EdgeKey> {
        let mut out = Vec::new();
        body_edges(map, player, camera(angle), |k| out.push(k));
        out
    }

    const N: OwnedEdge = OwnedEdge::North;
    const W: OwnedEdge = OwnedEdge::West;

    /// Inside, in the west room, camera from the south-east: the whole building's south and east
    /// outside walls drop (the other room's too), and the inner wall between the rooms (it hides
    /// the player's room); the north and west walls stand. Nothing of the other building moves.
    #[test]
    fn inside_the_buildings_camera_side_walls_drop_whole() {
        let map = town();
        let cut = room_set(&map, Vec2::new(11.5, 12.5), 0);
        for x in 10..18 {
            assert!(cut.contains(&(IVec2::new(x, 16), N)), "south wall at x {x}");
            assert!(!cut.contains(&(IVec2::new(x, 10), N)), "north wall at x {x}");
        }
        for y in 10..16 {
            assert!(cut.contains(&(IVec2::new(18, y), W)), "east wall at y {y}");
            assert!(!cut.contains(&(IVec2::new(10, y), W)), "west wall at y {y}");
            assert!(cut.contains(&(IVec2::new(14, y), W)), "the wall between the rooms at y {y}");
        }
        assert!(cut.iter().all(|(t, _)| t.x < 20), "another building's wall: {cut:?}");
        assert_eq!(cut.len(), 8 + 6 + 6);
    }

    /// In the east room the inner wall is behind the room on screen: it stands. The outside walls
    /// facing the camera still drop along the whole building.
    #[test]
    fn an_inner_wall_behind_the_room_stands() {
        let map = town();
        let cut = room_set(&map, Vec2::new(16.5, 12.5), 0);
        for y in 10..16 {
            assert!(!cut.contains(&(IVec2::new(14, y), W)), "the wall between the rooms at y {y}");
            assert!(cut.contains(&(IVec2::new(18, y), W)));
        }
        for x in 10..18 {
            assert!(cut.contains(&(IVec2::new(x, 16), N)));
        }
    }

    /// A wall the player stands in front of never drops. An L-shaped room (a 6 x 6 main part with
    /// a 3 x 3 leg off its south-west), camera from the south-east: both the leg's east wall and
    /// the main part's south wall beside the leg face the camera, so the room's set has both;
    /// standing in the main part, the leg's east wall is behind the player on screen and stands,
    /// and standing in the leg, the main part's south wall does.
    #[test]
    fn a_wall_the_player_is_in_front_of_stands() {
        let mut map = town();
        add_building(&mut map, &[(IVec2::new(40, 10), IVec2::new(6, 6)), (IVec2::new(40, 16), IVec2::new(3, 3))], 1);
        let main = map.tile(IVec2::new(40, 10)).room;
        for y in 16..19 {
            for x in 40..43 {
                map.tile_mut(IVec2::new(x, y)).unwrap().room = main;
            }
        }
        for x in 40..43 {
            map.set_edge(IVec2::new(x, 16), Side::N, Edge::None);
        }
        let leg_east = |y: i32| (IVec2::new(43, y), W);
        let main_south = |x: i32| (IVec2::new(x, 16), N);
        let (in_main, in_leg) = (Vec2::new(44.5, 12.5), Vec2::new(41.5, 17.5));
        for p in [in_main, in_leg] {
            let set = room_set(&map, p, 0);
            assert!((16..19).all(|y| set.contains(&leg_east(y))) && (43..46).all(|x| set.contains(&main_south(x))), "{set:?}");
        }
        let (c, _) = ground_dir(camera(0));
        assert!(in_front(leg_east(17), in_main, c) && !in_front(main_south(44), in_main, c));
        assert!(in_front(main_south(44), in_leg, c) && !in_front(leg_east(17), in_leg, c));
        let mut cut = Cutaway::new(Handle::default(), false);
        cut.update(&map, in_main, 0, camera(0), 0.016);
        for y in 16..19 {
            assert_eq!(cut.amount(leg_east(y)), 0.0, "the leg's east wall at y {y}, behind the player");
        }
        for x in 43..46 {
            assert_eq!(cut.amount(main_south(x)), 1.0, "the main part's south wall at x {x}");
        }
        // Into the leg: the two swap, eased.
        for _ in 0..20 {
            cut.update(&map, in_leg, 0, camera(0), 0.016);
        }
        for y in 16..19 {
            assert_eq!(cut.amount(leg_east(y)), 1.0, "the leg's east wall at y {y}");
        }
        for x in 44..46 {
            assert_eq!(cut.amount(main_south(x)), 0.0, "the main part's south wall at x {x}, behind the player");
        }
        // A wall beside the player that runs on away from the camera: the pieces further from the
        // camera than the player stand, though the player is on the far side of the wall's line.
        // In the leg, by its east wall, at the leg's camera end: all of that wall is up the screen.
        let low = Vec2::new(42.5, 18.8);
        assert!(!in_front(leg_east(18), low, c), "the piece level with the player");
        assert!(in_front(leg_east(16), low, c), "the piece two metres up the screen");
        let mut cut = Cutaway::new(Handle::default(), false);
        cut.update(&map, low, 0, camera(0), 0.016);
        assert_eq!(cut.amount(leg_east(16)), 0.0);
        assert_eq!(cut.amount(leg_east(18)), 1.0);
        // The same from the other side: nothing the player is in front of is down at any angle.
        for angle in 0..4 {
            let mut cut = Cutaway::new(Handle::default(), false);
            cut.update(&map, in_main, 0, camera(angle), 0.016);
            let (c, _) = ground_dir(camera(angle));
            let body = body_set(&map, in_main, angle);
            assert!(cut.cut_edges().all(|(k, _)| !in_front(k, in_main, c) || body.contains(&k)), "angle {angle}");
        }
    }

    /// The sides that drop follow the camera: from each of its four angles, the two outside walls
    /// facing it, and only those of the four.
    #[test]
    fn the_cut_sides_follow_the_camera() {
        let map = town();
        let p = Vec2::new(32.5, 12.5);
        let side = |cut: &[EdgeKey], s: char| match s {
            'n' => (30..36).all(|x| cut.contains(&(IVec2::new(x, 10), N))),
            's' => (30..36).all(|x| cut.contains(&(IVec2::new(x, 16), N))),
            'w' => (10..16).all(|y| cut.contains(&(IVec2::new(30, y), W))),
            _ => (10..16).all(|y| cut.contains(&(IVec2::new(36, y), W))),
        };
        // Angle 0 looks from +x +z, 1 from +x -z, 2 from -x -z, 3 from -x +z.
        for (angle, facing) in [(0, "se"), (1, "ne"), (2, "nw"), (3, "sw")] {
            let cut = room_set(&map, p, angle);
            for s in ['n', 's', 'e', 'w'] {
                assert_eq!(side(&cut, s), facing.contains(s), "angle {angle}, side {s}: {cut:?}");
            }
            assert_eq!(cut.len(), 12, "angle {angle}");
        }
    }

    /// Outdoors nothing drops for a room, and a building that doesn't cover the player is whole:
    /// in the open, in front of a house (on its camera side), and walking along its facade.
    #[test]
    fn buildings_that_dont_cover_the_player_stay_whole() {
        let map = town();
        for p in [Vec2::new(24.0, 24.0), Vec2::new(14.0, 18.5), Vec2::new(18.4, 12.5), Vec2::new(20.5, 13.0), Vec2::new(5.0, 13.0)] {
            assert!(room_set(&map, p, 0).is_empty(), "{p}");
            assert!(body_set(&map, p, 0).is_empty(), "at {p}: {:?}", body_set(&map, p, 0));
            assert!(!bulk_hides(&map, p, camera(0), None), "{p}");
        }
    }

    /// Behind a house (it stands between the player and the camera): only the wall pieces that
    /// cover the body drop, straight pieces of that building's wall, and the building's bulk (its
    /// roof) is reported as still covering the player. The other building is untouched.
    #[test]
    fn behind_a_house_only_the_covering_pieces_drop() {
        let map = town();
        // Half a metre north of house A's north wall, camera from the south-east.
        let p = Vec2::new(12.5, 9.5);
        let cut = body_set(&map, p, 0);
        assert!(cut.contains(&(IVec2::new(12, 10), N)) || cut.contains(&(IVec2::new(13, 10), N)), "{cut:?}");
        assert!(cut.len() <= 4, "{cut:?}");
        for (t, _) in &cut {
            assert!((10..=18).contains(&t.x) && (10..=16).contains(&t.y), "a wall of another building: {t}");
        }
        // Each lies between the body and the camera on screen.
        let (c, _) = ground_dir(camera(0));
        for key in &cut {
            let (near, far) = crossing(*key, p, c, BODY_HALF).unwrap();
            assert!(far > 0.0 && near < WALL_REACH, "{key:?}: {near}..{far}");
        }
        assert!(bulk_hides(&map, p, camera(0), None));
        // From the other side of the camera's view (camera from the north-west) the same spot is
        // in front of the house: nothing covers the player.
        assert!(body_set(&map, p, 2).is_empty());
        assert!(!bulk_hides(&map, p, camera(2), None));
    }

    /// The building you're inside doesn't count as covering you (its roof is hidden); a taller one
    /// across the yard toward the camera does, a one-storey one that far doesn't.
    #[test]
    fn only_bulk_that_still_stands_covers_the_player() {
        let map = town();
        let inside = Vec2::new(11.5, 12.5);
        assert!(bulk_hides(&map, inside, camera(0), None));
        assert!(!bulk_hides(&map, inside, camera(0), map.building_at(inside)));
        // 7 m north-west of the two-storey house B's corner: the line from the feet reaches its
        // footprint 4 m up, under its roof. The same distance from one-storey A (3 m + roof) too,
        // but 12 m away the line passes over A.
        assert!(bulk_hides(&map, Vec2::new(25.0, 5.0), camera(0), None));
        assert!(!bulk_hides(&map, Vec2::new(1.5, 1.5), camera(0), None));
    }

    /// The window the player is climbing through stays whole while the rest of its wall drops, on
    /// either side of it, and stands with the rest once he is through.
    #[test]
    fn the_window_being_climbed_through_stays_whole() {
        let mut map = town();
        map.set_edge(IVec2::new(12, 16), Side::N, Edge::WindowOpen);
        let window = (IVec2::new(12, 16), N);
        assert_eq!(crossed_window(&map, Vec2::new(12.5, 16.2)), Some(window));
        assert_eq!(crossed_window(&map, Vec2::new(12.5, 15.8)), Some(window));
        assert_eq!(crossed_window(&map, Vec2::new(12.5, 15.5)), None);
        let mut cut = Cutaway::new(Handle::default(), false);
        cut.update(&map, Vec2::new(12.5, 15.8), 0, camera(0), 0.016);
        assert_eq!(cut.amount(window), 0.0);
        assert_eq!(cut.amount((IVec2::new(13, 16), N)), 1.0);
        cut.update(&map, Vec2::new(12.5, 15.0), 0, camera(0), 1.0);
        assert_eq!(cut.amount(window), 1.0);
    }

    /// The state: edges ease down over `EASE_SECS` and up again, the first decision is at once,
    /// walls up cuts nothing and says the player is covered, and the amounts are what the shader
    /// reads.
    #[test]
    fn walls_ease_and_the_toggle_stands_them_up() {
        let map = town();
        let mut cut = Cutaway::new(Handle::default(), false);
        let (inside, outside) = (Vec2::new(11.5, 12.5), Vec2::new(24.0, 24.0));
        let south = (IVec2::new(12, 16), N);
        // A fresh state (a loaded save, an offscreen picture) takes its first decision at once.
        assert!(cut.update(&map, inside, 0, camera(0), 0.016));
        assert_eq!(cut.amount(south), 1.0);
        assert_eq!(cut.amount((IVec2::new(12, 10), N)), 0.0);
        // Nothing moves while nothing changes: no upload.
        assert!(!cut.update(&map, inside + Vec2::new(0.3, 0.2), 0, camera(0), 0.016));
        // Walking out: the walls stand up again over the ease, not at once.
        assert!(cut.update(&map, outside, 0, camera(0), EASE_SECS * 0.5));
        let half = cut.amount(south);
        assert!(half > 0.2 && half < 0.8, "{half}");
        assert!(cut.update(&map, outside, 0, camera(0), EASE_SECS * 0.6));
        assert_eq!(cut.amount(south), 0.0);
        assert!(cut.moving.is_empty() && cut.values.iter().all(|v| *v == 0));
        // Back in, with the walls up: nothing is cut.
        cut.set_walls_up(true);
        for _ in 0..20 {
            cut.update(&map, inside, 0, camera(0), 0.016);
        }
        assert!(cut.values.iter().all(|v| *v == 0));
        // Behind the house with the walls up, the wall that would have dropped covers the player.
        cut.update(&map, Vec2::new(18.6, 12.5), 0, camera(2), 0.016);
        assert!(cut.occluded);
        cut.set_walls_up(false);
        for _ in 0..20 {
            cut.update(&map, inside, 0, camera(0), 0.016);
        }
        assert_eq!(cut.amount(south), 1.0);
        // The camera turns: the room's set follows.
        for _ in 0..20 {
            cut.update(&map, inside, 0, camera(2), 0.016);
        }
        assert_eq!(cut.amount(south), 0.0);
        assert_eq!(cut.amount((IVec2::new(12, 10), N)), 1.0);
    }

    /// A detail (an awning, a porch post) thins out only while it stands between the player and
    /// the camera: not beside the line of sight, behind the player, or far toward the camera.
    #[test]
    fn details_thin_out_only_over_the_player() {
        let cam = camera(0);
        let (c, _) = ground_dir(cam);
        let p = Vec2::new(20.0, 20.0);
        let perp = Vec2::new(-c.y, c.x);
        assert_eq!(object_fade(p + c * 1.5, 0.8, p, cam), 1.0);
        assert_eq!(object_fade(p + c * 0.2, 0.8, p, cam), 1.0);
        assert_eq!(object_fade(p - c * 2.0, 0.8, p, cam), 0.0, "behind the player");
        assert_eq!(object_fade(p + c * 1.5 + perp * 2.0, 0.8, p, cam), 0.0, "beside the line of sight");
        assert_eq!(object_fade(p + c * 8.0, 0.8, p, cam), 0.0, "too far toward the camera to reach down to the player");
        // A thin post only right in front.
        assert_eq!(object_fade(p + c * 1.0 + perp * 1.0, 0.1, p, cam), 0.0);
        assert_eq!(object_fade(p + c * 1.0 + perp * 0.2, 0.1, p, cam), 1.0);
    }

    /// Where to take pictures of the cutaways in the default world (`cargo test -p zombye-client
    /// --release cutaway_places -- --ignored --nocapture`): the house you wake up in and its rooms,
    /// and the buildings round it with their storeys, kind and porch.
    #[test]
    #[ignore]
    fn cutaway_places() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let mut world = zombye_sim::World::new(zombye_net::content::load_config(&data, 1).expect("content"));
        // A save for the zombies' silhouettes: the survivor behind the house it woke in, three
        // zombies beside it, all under the roof as the camera sees them (`--load` it with
        // `--zombie-silhouettes`).
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/review");
        std::fs::create_dir_all(&dir).unwrap();
        let mut behind = zombye_sim::World::new(zombye_net::content::load_config(&data, 1).expect("content"));
        let house = behind.map.buildings[behind.spawn_building].min.as_vec2();
        behind.add_player(house + Vec2::new(3.5, -0.9));
        behind.spawn_zombies(3, house + Vec2::new(6.0, -1.2), 1.3);
        zombye_net::save_file::save(&behind, &dir.join("cutaway_zombies.zsave")).unwrap();
        world.add_player(world.player_spawn);
        let map = &world.map;
        let spawn = world.player_spawn;
        eprintln!("spawn {spawn}, building {}", world.spawn_building);
        let mut near: Vec<usize> = (0..map.buildings.len()).collect();
        near.sort_by(|a, b| {
            let d = |i: &usize| (map.buildings[*i].min.as_vec2() + map.buildings[*i].size.as_vec2() * 0.5).distance(spawn);
            d(a).total_cmp(&d(b))
        });
        for &i in near.iter().take(14) {
            let b = &map.buildings[i];
            eprintln!("building {i} {} min {} size {} levels {} look {:?}", b.template, b.min, b.size, b.levels, b.look);
            if i == world.spawn_building {
                let mut seen = Vec::new();
                for y in b.min.y..=b.min.y + b.size.y {
                    for x in b.min.x..=b.min.x + b.size.x {
                        let t = map.tile(IVec2::new(x, y));
                        for (which, kind) in [("north", t.north), ("west", t.west)] {
                            if kind.is_door() || kind == Edge::Arch || kind == Edge::DoorOpen {
                                eprintln!("  {kind:?} on the {which} edge of {x},{y}");
                            }
                        }
                    }
                }
                for y in b.min.y..b.min.y + b.size.y {
                    let row: String = (b.min.x..b.min.x + b.size.x).map(|x| (b'a' + (map.tile(IVec2::new(x, y)).room % 26) as u8) as char).collect();
                    eprintln!("  {row}");
                    for x in b.min.x..b.min.x + b.size.x {
                        let r = map.tile(IVec2::new(x, y)).room;
                        if r != 0 && !seen.contains(&r) {
                            seen.push(r);
                            eprintln!("  room {r} ({}) '{}' {} at {x},{y}", (b'a' + (r % 26) as u8) as char, map.rooms[r as usize].name, map.rooms[r as usize].room_type);
                        }
                    }
                }
            }
        }
    }

    /// The shipped town: wherever the player stands in a house, the room's cut is walls of that
    /// building only, includes every camera-facing outside wall of it, and never a wall that is
    /// behind the player's room on screen.
    #[test]
    fn every_house_of_the_town_cuts_only_its_own_camera_side() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let templates = zombye_net::content::load_building_templates(&data).expect("templates");
        let map = zombye_sim::worldgen::generate(&zombye_sim::worldgen::GenConfig::default(), &templates).map;
        let mut checked = 0;
        for (i, b) in map.buildings.iter().enumerate().take(40) {
            // A tile of each of its rooms.
            let mut seen = Vec::new();
            for y in b.min.y..b.min.y + b.size.y {
                for x in b.min.x..b.min.x + b.size.x {
                    let room = map.tile(IVec2::new(x, y)).room;
                    if room == 0 || map.rooms[room as usize].building as usize != i || seen.contains(&room) {
                        continue;
                    }
                    seen.push(room);
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    for angle in 0..4 {
                        let cut = room_set(&map, p, angle);
                        let (c, _) = ground_dir(camera(angle));
                        assert!(!cut.is_empty(), "{} room {room} angle {angle}", b.template);
                        for &(t, which) in &cut {
                            let other = t - if which == N { IVec2::Y } else { IVec2::X };
                            let rooms = [map.tile(t).room, map.tile(other).room];
                            assert!(rooms.iter().any(|r| *r != 0 && map.rooms[*r as usize].building as usize == i), "{}: a wall of another building", b.template);
                            // The tile on the far side of it from the camera is the building's.
                            let (a, e) = TileMap::edge_segment(t, which);
                            // (Round a patio, an outside wall facing away can still stand in front of the room.)
                            let behind = map.room_at((a + e) * 0.5 - c * 0.3);
                            let ours = behind != 0 && map.rooms[behind as usize].building as usize == i;
                            assert!(ours || hides_room(&map, (t, which), c, room, 0), "{}: a far wall dropped", b.template);
                        }
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 200, "{checked}");
    }
}
