//! Opening hints: the doors and windows a building hides from the camera, outlined through it.
//!
//! A door or window on an outside wall that faces away from the camera is behind its own building
//! on screen, so a player walking round a house can't tell where its back door is without turning
//! the camera. While the player is within [`RANGE`] of such an opening, its outline is drawn
//! through the building: doors and windows in one blue (a shut window's outline has a cross, an open or
//! broken one a single bar across), and ones that are barred (locked, barricaded, boarded: nothing in the map is yet)
//! dimmer.
//!
//! **Fair information:** only the building's own openings, from the client's copy of the map,
//! which every client holds whole. Nothing of zombies, players or loot.
//!
//! **Drawn** the way the player's silhouette is (`silhouette.rs`): a flat tint in the transparent
//! phase of the main pass with the depth test turned round, so only where something stands in
//! front of the opening. Two small shared meshes (a door's outline, a window's), one material, and
//! an entity an opening with a `MeshTag` for its colour and how far it has faded in, so Bevy draws
//! each mesh's instances as one batch. No pass of its own, nothing full-screen.
//!
//! **Cost:** which openings are hinted is worked out again only when the player has moved half a
//! metre, the camera has turned, the map changed or the player went in or out of a building (a
//! walk over the tiles within [`RANGE`]); a frame in which nothing fades writes nothing. Nothing
//! is allocated per frame: the lists keep their room and the entities are kept for the next hint.
//!
//! **Doors in cut-away walls** get the same outline, the other way round: a door whose wall has
//! dropped to its stub (`cutaway.rs`) has no leaf drawn (it sinks away with the wall), so its outline is
//! drawn at the door's full size in its place, in the doors' one blue (shut, open or broken alike), by the wall's
//! own eased amount, and only where nothing stands in front of it ([`cut_door`], `draw_cut_doors`).
//! Always on: it is how a cut door is drawn, not a hint through anything.
//!
//! Ctrl+F10 switches the hints off and on, kept in the settings file (`opening_hints`; on by
//! default); `--opening-hints on|off` for a session.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshTag, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use zombye_sim::map::{Edge, OwnedEdge, TileMap};

use crate::camera::IsoCamera;
use crate::cutaway::{Cutaway, CutawaySystems, EdgeKey, ground_dir};
use crate::daynight::Daylight;
use crate::player::Player;
use crate::server_link::ClientMap;

/// An opening is hinted while the player is within this many metres of its middle.
pub const RANGE: f32 = 10.0;
/// Seconds for a hint to come and go.
pub const FADE_SECS: f32 = 0.2;
/// A wall faces away from the camera when its outward normal has at least this much against the
/// camera's ground direction (as the cutaways' rule for a wall that faces it: 0.71 for the two far
/// sides at a fixed angle; in the middle of a turn only the side square to the camera).
const AWAY: f32 = 0.1;
/// How far toward the camera an outline's depth is moved before it is compared (metres): the
/// wall it is in, its own door leaf or pane and its frame don't count as covering it.
const CLEAR_M: f32 = 0.5;
const CLEAR: f32 = CLEAR_M / (crate::camera::FAR - crate::camera::NEAR);
/// The one tint of every outline (linear, straight alpha): doors and windows, shut, open or broken.
const DOOR: Vec4 = Vec4::new(0.16, 0.66, 1.0, 0.9);
/// A cut-away door's outline: this share of a hint's alpha (it is in plain view, in your room).
pub const CUT_DOOR: f32 = 0.8;
/// A barred opening's share of its tint's alpha.
const BARRED: f32 = 0.45;
/// The tints' brightness at night (a shape, not a lamp).
const NIGHT: f32 = 0.16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Door,
    Window,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Shut and whole.
    Shut,
    /// Locked, barricaded or boarded up: drawn dimmer. No edge in the map is any of these yet, so
    /// nothing gives this state today; an edge kind that is must be given it in [`classify`].
    #[allow(dead_code)]
    Barred,
    /// Open or broken: a way in.
    Open,
}

/// What kind of opening an edge is and the state it is in, from the map alone. Walls, arches and
/// fences are none (a fence is no building wall). Every edge kind is named here, so a new one
/// (a locked door, a boarded window) doesn't compile until it is given its hint.
pub fn classify(edge: Edge) -> Option<(Kind, State)> {
    match edge {
        Edge::DoorClosed => Some((Kind::Door, State::Shut)),
        Edge::DoorOpen | Edge::DoorBroken => Some((Kind::Door, State::Open)),
        Edge::Window => Some((Kind::Window, State::Shut)),
        Edge::WindowOpen | Edge::WindowBroken => Some((Kind::Window, State::Open)),
        Edge::None | Edge::Wall | Edge::Arch | Edge::Fence => None,
    }
}

/// A hinted opening.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hint {
    pub key: EdgeKey,
    pub kind: Kind,
    pub state: State,
}

/// The building a tile is inside of, walls round it: its room's, unless that room is the
/// building's open porch (outdoors as far as an outside wall goes).
fn enclosed(map: &TileMap, tile: IVec2) -> Option<usize> {
    let room = map.tile(tile).room;
    if room == 0 {
        return None;
    }
    let building = map.rooms[room as usize].building as usize;
    let porch = map.buildings.get(building).and_then(|b| b.look).and_then(|l| l.porch).is_some_and(|p| p.room == room);
    (!porch).then_some(building)
}

/// If this edge is part of a building's outside wall: the building, and the wall's outward normal
/// on the ground.
pub fn outside_wall(map: &TileMap, key: EdgeKey) -> Option<(usize, Vec2)> {
    // `toward`: from the edge's north / west side to its south / east side.
    let (other, toward) = if key.1 == OwnedEdge::North { (key.0 - IVec2::Y, Vec2::Y) } else { (key.0 - IVec2::X, Vec2::X) };
    match (enclosed(map, other), enclosed(map, key.0)) {
        (Some(b), None) => Some((b, toward)),
        (None, Some(b)) => Some((b, -toward)),
        _ => None,
    }
}

/// Is the opening on this edge hinted, for a player at `player` and a camera whose ground
/// direction (unit, toward the camera) is `c`? It must be a door or a window on a building's
/// outside wall that faces away from the camera, within [`RANGE`] of the player, and not of the
/// building the player is inside (`inside`: that one is cut open, its far walls in plain view).
pub fn hint_for(map: &TileMap, key: EdgeKey, player: Vec2, c: Vec2, inside: Option<usize>) -> Option<Hint> {
    let tile = map.tile(key.0);
    let (kind, state) = classify(if key.1 == OwnedEdge::North { tile.north } else { tile.west })?;
    let (a, b) = TileMap::edge_segment(key.0, key.1);
    if ((a + b) * 0.5).distance_squared(player) > RANGE * RANGE {
        return None;
    }
    let (building, outward) = outside_wall(map, key)?;
    (Some(building) != inside && outward.dot(c) < -AWAY).then_some(Hint { key, kind, state })
}

/// Every opening hinted for a player at `player`, by the tiles within [`RANGE`].
pub fn find(map: &TileMap, player: Vec2, camera_dir: Vec3, inside: Option<usize>, mut found: impl FnMut(Hint)) {
    let (c, _) = ground_dir(camera_dir);
    let lo = (player - RANGE - 1.0).floor().as_ivec2();
    let hi = (player + RANGE + 1.0).ceil().as_ivec2();
    for y in lo.y..=hi.y {
        for x in lo.x..=hi.x {
            let owner = IVec2::new(x, y);
            if !map.in_bounds(owner) {
                continue;
            }
            for which in [OwnedEdge::North, OwnedEdge::West] {
                if let Some(hint) = hint_for(map, (owner, which), player, c, inside) {
                    found(hint);
                }
            }
        }
    }
}

/// The outline of the door on this edge while its wall is cut `amount` (0 standing, 1 a stub):
/// what it is drawn as and its mesh tag. Doors only (a window keeps its lip of glass on the
/// stub), shut, open or battered down.
pub fn cut_door(map: &TileMap, key: EdgeKey, amount: f32) -> Option<(Hint, u32)> {
    let tile = map.tile(key.0);
    let (kind, state) = classify(if key.1 == OwnedEdge::North { tile.north } else { tile.west })?;
    let hint = Hint { key, kind, state };
    (kind == Kind::Door && amount > 0.0).then(|| (hint, tint_bits(hint) | (amount.clamp(0.0, 1.0) * CUT_DOOR * 255.0).round() as u32))
}

/// A hint on its way in or out.
struct Slot {
    hint: Hint,
    wanted: bool,
    /// 0 gone to 1 all there (linear in time; drawn smoothed).
    shown: f32,
    /// The tag its entity was last given.
    tag: u32,
    entity: Option<Entity>,
}

/// The mesh tag of a hint: bits 0-7 how much of it shows, 8 an open or broken window (its mesh differs; every outline is one blue),
/// bit 10 barred (`opening_hint.wesl`).
pub fn tag_of(hint: Hint, shown: f32) -> u32 {
    let s = shown.clamp(0.0, 1.0);
    let alpha = (s * s * (3.0 - 2.0 * s) * 255.0).round() as u32;
    alpha | tint_bits(hint)
}

/// A hint's tint and barred bits of its mesh tag.
fn tint_bits(hint: Hint) -> u32 {
    // Doors and windows are the same blue shut, open or broken. An open window's bit is for
    // its mesh (the tag must change when a window opens, so the mesh is swapped).
    let open = hint.kind == Kind::Window && hint.state == State::Open;
    (open as u32) << 8 | if hint.state == State::Barred { 1 << 10 } else { 0 }
}

/// The outlines of doors in cut-away walls: an entity a door, kept (hidden) once its wall is up.
#[derive(Resource, Default)]
struct CutDoors {
    shown: Vec<CutDoor>,
    spare: Vec<Entity>,
}

struct CutDoor {
    key: EdgeKey,
    /// Drawn swung open (the leaf itself is hidden in a cut wall, so the outline says which).
    open: bool,
    entity: Entity,
    tag: u32,
    seen: bool,
}

/// The hints' state: which are on their way in or out, and the setting.
#[derive(Resource)]
pub struct OpeningHints {
    /// The setting (Ctrl+F10).
    pub on: bool,
    slots: Vec<Slot>,
    /// Entities of hints that have gone, hidden, for the next ones.
    spare: Vec<Entity>,
    /// What the hints were last worked out for: the player's place in half metres, the camera's
    /// angle, the building the player is in, the setting.
    decided_for: Option<(IVec2, i32, Option<usize>, bool)>,
}

impl OpeningHints {
    fn new(on: bool) -> Self {
        Self { on, slots: Vec::with_capacity(32), spare: Vec::with_capacity(32), decided_for: None }
    }

    /// Work out which openings are hinted now, if anything they depend on changed (`map_changed`:
    /// a door or window did). True if it was worked out again.
    pub fn decide(&mut self, map: &TileMap, map_changed: bool, player: Vec2, camera_dir: Vec3, inside: Option<usize>) -> bool {
        let angle = (camera_dir.x.atan2(camera_dir.z) * 200.0).round() as i32;
        let decided_for = Some(((player * 2.0).floor().as_ivec2(), angle, inside, self.on));
        if decided_for == self.decided_for && !map_changed {
            return false;
        }
        self.decided_for = decided_for;
        for s in &mut self.slots {
            s.wanted = false;
        }
        if self.on {
            let slots = &mut self.slots;
            find(map, player, camera_dir, inside, |hint| match slots.iter_mut().find(|s| s.hint.key == hint.key) {
                // A door opened or a window broke while hinted: its tag is written again at once.
                Some(s) => (s.hint, s.wanted) = (hint, true),
                None => slots.push(Slot { hint, wanted: true, shown: 0.0, tag: u32::MAX, entity: None }),
            });
        }
        true
    }

    /// Fade every hint toward where it's wanted by `dt` seconds. True if any moved.
    pub fn ease(&mut self, dt: f32) -> bool {
        let step = dt / FADE_SECS;
        let mut moved = false;
        for s in &mut self.slots {
            let to = if s.wanted { 1.0 } else { 0.0 };
            if s.shown != to {
                s.shown = if s.wanted { (s.shown + step).min(1.0) } else { (s.shown - step).max(0.0) };
                moved = true;
            }
        }
        moved
    }

    /// How much of the hint on this edge shows (0 none, 1 all), if there is one.
    #[cfg(test)]
    fn shown(&self, key: EdgeKey) -> Option<f32> {
        self.slots.iter().find(|s| s.hint.key == key).map(|s| s.shown)
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(HintKey)]
pub struct HintMaterial {
    #[uniform(0)]
    pub look: HintLook,
    /// Drawn only where something stands in front of it (a hint through a building), or only
    /// where nothing does (a cut-away door's outline).
    pub behind: bool,
}

/// The pipeline key of a [`HintMaterial`].
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq)]
pub struct HintKey {
    behind: bool,
}

impl From<&HintMaterial> for HintKey {
    fn from(m: &HintMaterial) -> Self {
        HintKey { behind: m.behind }
    }
}

/// Must match `HintLook` in opening_hint.wesl.
#[derive(ShaderType, Debug, Clone, PartialEq)]
pub struct HintLook {
    pub tint: Vec4,
    /// x = how far toward the camera an outline's depth is moved (depth-buffer units), y = a
    /// barred opening's share of the alpha, z = brightness (dim at night).
    pub params: Vec4,
}

/// The look at a daylight (0 night to 1 day), in 16 steps so the material changes a few times at
/// dusk and dawn, not every frame.
fn look_at(daylight: f32) -> HintLook {
    let d = (daylight.clamp(0.0, 1.0) * 16.0).round() / 16.0;
    HintLook { tint: DOOR, params: Vec4::new(CLEAR, BARRED, NIGHT + (1.0 - NIGHT) * d, 0.0) }
}

impl Material for HintMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/opening_hint.wesl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    // Only the main pass: a hint is in no depth prepass and casts no shadow.
    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // As the silhouettes: only where something nearer is drawn. Seen from either side.
        if key.bind_group_data.behind {
            crate::silhouette::behind_only(descriptor);
        }
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

#[derive(Resource)]
struct HintAssets {
    door: Handle<Mesh>,
    window: Handle<Mesh>,
    /// An open or broken window: one bar across in place of the cross.
    window_open: Handle<Mesh>,
    material: Handle<HintMaterial>,
    /// The same look, drawn where nothing is in front: doors in cut-away walls.
    in_view: Handle<HintMaterial>,
    look: HintLook,
}

/// An outline in the plane of a wall edge: x along the edge (0 to 1), y up, as bars only, with
/// nothing between them (the vertex colour's alpha is each bar's share of the tint).
fn outline(lo: Vec2, hi: Vec2, bar: f32, bottom: bool, cross: Option<bool>) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut quad = |a: Vec2, b: Vec2, alpha: f32| {
        let n = positions.len() as u32;
        positions.extend([[a.x, a.y, 0.0], [b.x, a.y, 0.0], [b.x, b.y, 0.0], [a.x, b.y, 0.0]]);
        colors.extend([[1.0, 1.0, 1.0, alpha]; 4]);
        indices.extend([n, n + 1, n + 2, n, n + 2, n + 3]);
    };
    let (ilo, ihi) = (Vec2::new(lo.x + bar, if bottom { lo.y + bar } else { lo.y }), hi - bar);
    quad(lo, Vec2::new(ilo.x, hi.y), 1.0);
    quad(Vec2::new(ihi.x, lo.y), hi, 1.0);
    quad(Vec2::new(ilo.x, ihi.y), Vec2::new(ihi.x, hi.y), 1.0);
    if bottom {
        quad(Vec2::new(ilo.x, lo.y), Vec2::new(ihi.x, ilo.y), 1.0);
    }
    // `Some(true)`: a cross (a shut window's panes); `Some(false)`: only the bar across (an open one).
    if let Some(upright) = cross {
        let (mid, half) = ((lo + hi) * 0.5, bar * 0.3);
        if upright {
            quad(Vec2::new(mid.x - half, ilo.y), Vec2::new(mid.x + half, ihi.y), 0.7);
        }
        quad(Vec2::new(ilo.x, mid.y - half), Vec2::new(ihi.x, mid.y + half), 0.7);
    }
    let n = positions.len();
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32, 0.0, 1.0]; n])
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(indices))
}

/// Where an edge's outline stands: at the edge's first corner, turned along it.
fn place(key: EdgeKey) -> Transform {
    let at = Transform::from_xyz(key.0.x as f32, 0.0, key.0.y as f32);
    if key.1 == OwnedEdge::North { at } else { at.with_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)) }
}

/// A door standing open on this edge (its leaf swung in; a battered-down door has none).
pub fn stands_open(map: &TileMap, key: EdgeKey) -> bool {
    let tile = map.tile(key.0);
    (if key.1 == OwnedEdge::North { tile.north } else { tile.west }) == Edge::DoorOpen
}

/// Where the outline of a door in a cut wall stands: in its doorway, or, standing open, where
/// its leaf is (`world_render::door_leaf_at_rest`), so an open door reads as open.
fn place_cut(key: EdgeKey, open: bool) -> Transform {
    crate::world_render::door_leaf_at_rest(key, 0.0, open)
}

pub struct OpeningHintsPlugin;

impl Plugin for OpeningHintsPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().get_resource::<crate::Options>().cloned();
        // The flag, else (never offscreen) the settings file, else on.
        let on = match options.as_ref().and_then(|o| o.opening_hints) {
            Some(on) => on,
            None if options.as_ref().is_some_and(|o| !o.offscreen()) => crate::ui_scale::SettingsStore::ensure(app).opening_hints,
            None => true,
        };
        app.add_plugins(MaterialPlugin::<HintMaterial>::default())
            .insert_resource(OpeningHints::new(on))
            .init_resource::<CutDoors>()
            .add_systems(Startup, setup)
            .add_systems(Update, (toggle, (draw, draw_cut_doors).chain().after(CutawaySystems)));
    }
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<HintMaterial>>) {
    let look = look_at(1.0);
    commands.insert_resource(HintAssets {
        // A door leaf is 0.92 x 2.05 m; a window runs from the sill (0.9 m) to the head (2.1 m).
        door: meshes.add(outline(Vec2::new(0.04, 0.0), Vec2::new(0.96, 2.08), 0.07, false, None)),
        window: meshes.add(outline(Vec2::new(0.08, 0.9), Vec2::new(0.92, 2.1), 0.06, true, Some(true))),
        window_open: meshes.add(outline(Vec2::new(0.08, 0.9), Vec2::new(0.92, 2.1), 0.06, true, Some(false))),
        material: materials.add(HintMaterial { look: look.clone(), behind: true }),
        in_view: materials.add(HintMaterial { look: look.clone(), behind: false }),
        look,
    });
}

/// Ctrl+F10 (or Cmd): the hints on or off, kept in the settings file.
fn toggle(keys: Res<ButtonInput<KeyCode>>, time: Res<Time>, mut hints: ResMut<OpeningHints>, store: Option<ResMut<crate::ui_scale::SettingsStore>>, notice: Option<ResMut<crate::saving::Notice>>) {
    if !keys.just_pressed(KeyCode::F10) || !keys.any_pressed(crate::cutaway::CTRL_KEYS) {
        return;
    }
    hints.on = !hints.on;
    let value = hints.on;
    if let Some(mut store) = store
        && let Err(e) = store.change(|s| s.opening_hints = value)
    {
        eprintln!("{e}");
    }
    if let Some(mut notice) = notice {
        notice.say(time.elapsed_secs(), if value { "Opening hints on: doors and windows behind a building show through it" } else { "Opening hints off" }.to_string());
    }
}

/// Decide, fade, and bring each hint's entity up to date.
#[allow(clippy::too_many_arguments)]
fn draw(
    mut commands: Commands,
    time: Res<Time>,
    map: Res<ClientMap>,
    cutaway: Res<Cutaway>,
    daylight: Option<Res<Daylight>>,
    assets: Option<ResMut<HintAssets>>,
    mut hints: ResMut<OpeningHints>,
    mut materials: ResMut<Assets<HintMaterial>>,
    player: Query<&Transform, With<Player>>,
    cams: Query<&IsoCamera>,
    mut drawn: Query<(&mut Transform, &mut Mesh3d, &mut MeshTag, &mut Visibility), Without<Player>>,
) {
    let (Some(mut assets), Ok(p), Ok(cam)) = (assets, player.single(), cams.single()) else { return };
    let look = look_at(daylight.map_or(1.0, |d| d.0));
    if look != assets.look {
        assets.look = look.clone();
        for handle in [&assets.material, &assets.in_view] {
            if let Some(mut m) = materials.get_mut(handle) {
                m.look = look.clone();
            }
        }
    }
    // Only a frame in which something is decided anew or fades goes on (and counts as a change).
    let hints = hints.bypass_change_detection();
    // Not in first person: the outlines go by the fixed view's depth.
    let on = hints.on;
    hints.on = on && !cam.shoulder();
    let decided = hints.decide(&map.0, map.is_changed(), p.translation.xz(), cam.offset_dir(), cutaway.inside);
    hints.on = on;
    let moved = hints.ease(time.delta_secs());
    if !(decided || moved) {
        return;
    }
    let OpeningHints { slots, spare, .. } = hints;
    slots.retain_mut(|s| {
        if !s.wanted && s.shown <= 0.0 {
            // Gone: its entity is hidden and kept for the next hint.
            if let Some(e) = s.entity {
                if let Ok((_, _, _, mut visibility)) = drawn.get_mut(e) {
                    *visibility = Visibility::Hidden;
                }
                spare.push(e);
            }
            return false;
        }
        let tag = tag_of(s.hint, s.shown);
        let mesh = match (s.hint.kind, s.hint.state) {
            (Kind::Door, _) => &assets.door,
            (Kind::Window, State::Open) => &assets.window_open,
            (Kind::Window, _) => &assets.window,
        };
        match s.entity {
            Some(e) if tag != s.tag => {
                if let Ok((_, mut m, mut t, _)) = drawn.get_mut(e) {
                    *t = MeshTag::new(tag);
                    if m.0 != *mesh {
                        m.0 = mesh.clone();
                    }
                }
            }
            Some(_) => {}
            None => match spare.pop() {
                Some(e) => {
                    if let Ok((mut tf, mut m, mut t, mut visibility)) = drawn.get_mut(e) {
                        *tf = place(s.hint.key);
                        m.0 = mesh.clone();
                        *t = MeshTag::new(tag);
                        *visibility = Visibility::Inherited;
                    }
                    s.entity = Some(e);
                }
                None => {
                    s.entity = Some(commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(assets.material.clone()), place(s.hint.key), MeshTag::new(tag), NotShadowCaster, Visibility::Inherited)).id());
                }
            },
        }
        s.tag = tag;
        true
    });
}

/// Keep an outline on every door whose wall is cut, by the wall's amount. A frame in which no
/// wall moves and no door changes writes nothing; nothing is allocated once the lists have room.
fn draw_cut_doors(
    mut commands: Commands,
    map: Res<ClientMap>,
    cutaway: Res<Cutaway>,
    assets: Option<Res<HintAssets>>,
    mut doors: ResMut<CutDoors>,
    mut drawn: Query<(&mut Transform, &mut MeshTag, &mut Visibility), Without<Player>>,
    player: Query<&Transform, With<Player>>,
) {
    let Some(assets) = assets else { return };
    let eye = player.single().map(|p| p.translation.xz()).ok();
    let CutDoors { shown, spare } = doors.bypass_change_detection();
    for d in shown.iter_mut() {
        d.seen = false;
    }
    for (key, amount) in cutaway.cut_edges() {
        let Some((_, tag)) = cut_door(&map.0, key, amount) else { continue };
        // Only the doors of the room we're in: other rooms stay dark, their doors with them.
        if eye.is_some_and(|eye| !crate::fog::on_own_room(&map.0, key, eye)) {
            continue;
        }
        let open = stands_open(&map.0, key);
        match shown.iter_mut().find(|d| d.key == key) {
            Some(d) => {
                d.seen = true;
                if d.open != open {
                    d.open = open;
                    if let Ok((mut tf, _, _)) = drawn.get_mut(d.entity) {
                        *tf = place_cut(key, open);
                    }
                }
                if d.tag != tag {
                    d.tag = tag;
                    if let Ok((_, mut t, _)) = drawn.get_mut(d.entity) {
                        *t = MeshTag::new(tag);
                    }
                }
            }
            None => {
                let entity = match spare.pop() {
                    Some(e) => {
                        if let Ok((mut tf, mut t, mut visibility)) = drawn.get_mut(e) {
                            *tf = place_cut(key, open);
                            *t = MeshTag::new(tag);
                            *visibility = Visibility::Inherited;
                        }
                        e
                    }
                    None => commands.spawn((Mesh3d(assets.door.clone()), MeshMaterial3d(assets.in_view.clone()), place_cut(key, open), MeshTag::new(tag), NotShadowCaster, Visibility::Inherited)).id(),
                };
                shown.push(CutDoor { key, open, entity, tag, seen: true });
            }
        }
    }
    shown.retain(|d| {
        if !d.seen {
            if let Ok((_, _, mut visibility)) = drawn.get_mut(d.entity) {
                *visibility = Visibility::Hidden;
            }
            spare.push(d.entity);
        }
        d.seen
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cutaway::tests::{camera, town};
    use zombye_sim::map::Side;

    const N: OwnedEdge = OwnedEdge::North;
    const W: OwnedEdge = OwnedEdge::West;

    /// House A of the cutaways' test town (10..18 x 10..16, a west and an east room) with a door
    /// and a window on its north wall, a door on its south wall, a window on its west and its
    /// east wall, and a door between its two rooms; house B (30..36 x 10..16) with a north door.
    fn openings() -> TileMap {
        let mut map = town();
        map.set_edge(IVec2::new(12, 10), Side::N, Edge::DoorClosed);
        map.set_edge(IVec2::new(15, 10), Side::N, Edge::Window);
        map.set_edge(IVec2::new(13, 16), Side::N, Edge::DoorClosed);
        map.set_edge(IVec2::new(10, 12), Side::W, Edge::Window);
        map.set_edge(IVec2::new(18, 13), Side::W, Edge::Window);
        map.set_edge(IVec2::new(14, 12), Side::W, Edge::DoorClosed);
        map.set_edge(IVec2::new(32, 10), Side::N, Edge::DoorClosed);
        // A fence along the yard north of A, and an arch on B's south wall: neither is an opening.
        map.set_edge(IVec2::new(12, 6), Side::N, Edge::Fence);
        map.set_edge(IVec2::new(33, 16), Side::N, Edge::Arch);
        map
    }

    fn found(map: &TileMap, player: Vec2, angle: i32, inside: Option<usize>) -> Vec<Hint> {
        let mut out = Vec::new();
        find(map, player, camera(angle), inside, |h| out.push(h));
        out
    }

    fn keys(hints: &[Hint]) -> Vec<EdgeKey> {
        let mut keys: Vec<EdgeKey> = hints.iter().map(|h| h.key).collect();
        keys.sort_by_key(|k| (k.0.x, k.0.y, k.1 == W));
        keys
    }

    /// Facing: from each of the camera's four angles, exactly the doors and windows on the two
    /// outside walls that face away from it are hinted. Never the inner door, the fence or the arch.
    #[test]
    fn only_openings_on_walls_facing_away_are_hinted() {
        let map = openings();
        let p = Vec2::new(14.0, 18.0);
        let (north_door, north_window, south_door) = ((IVec2::new(12, 10), N), (IVec2::new(15, 10), N), (IVec2::new(13, 16), N));
        let (west_window, east_window) = ((IVec2::new(10, 12), W), (IVec2::new(18, 13), W));
        // Angle 0 looks from the south-east: the north and west walls are behind the house.
        assert_eq!(keys(&found(&map, p, 0, None)), vec![west_window, north_door, north_window]);
        // 1 from the north-east (south and west hidden), 2 from the north-west, 3 from the south-west.
        assert_eq!(keys(&found(&map, p, 1, None)), vec![west_window, south_door]);
        assert_eq!(keys(&found(&map, p, 2, None)), vec![south_door, east_window]);
        assert_eq!(keys(&found(&map, p, 3, None)), vec![north_door, north_window, east_window]);
        for angle in 0..4 {
            for h in found(&map, p, angle, None) {
                assert_ne!(h.key, (IVec2::new(14, 12), W), "the door between the rooms");
                assert!(h.key.0.y != 6 && h.key.0 != IVec2::new(33, 16), "a fence or an arch: {h:?}");
            }
        }
    }

    /// Mid-turn (the camera square to the east wall), only the wall square to it on the far side
    /// counts as facing away: the two it sees edge-on are neither hidden nor hinted.
    #[test]
    fn a_wall_edge_on_to_the_camera_is_not_hinted() {
        let map = openings();
        let east = Vec3::new(0.866, 0.5, 0.0);
        let mut out = Vec::new();
        find(&map, Vec2::new(14.0, 18.0), east, None, |h| out.push(h.key));
        assert_eq!(out, vec![(IVec2::new(10, 12), W)]);
    }

    /// Distance: within 10 m of the opening's middle, not beyond; so the other house's door is
    /// only hinted from near it.
    #[test]
    fn hints_reach_ten_metres() {
        let map = openings();
        let door = (IVec2::new(12, 10), N);
        let mid = Vec2::new(12.5, 10.0);
        let c = ground_dir(camera(0)).0;
        for (d, want) in [(0.5, true), (5.0, true), (9.9, true), (10.1, false), (14.0, false)] {
            let p = mid + Vec2::new(0.0, -d);
            assert_eq!(hint_for(&map, door, p, c, None).is_some(), want, "{d} m north of it");
            assert_eq!(found(&map, p, 0, None).iter().any(|h| h.key == door), want, "{d} m (find)");
        }
        let b_door = (IVec2::new(32, 10), N);
        assert!(!found(&map, Vec2::new(14.0, 18.0), 0, None).iter().any(|h| h.key == b_door));
        assert_eq!(keys(&found(&map, Vec2::new(33.0, 7.0), 0, None)), vec![b_door]);
    }

    /// State: shut, open and broken doors and windows by what the map holds, and nothing else is
    /// an opening. A barred one is dimmer than a shut one, in the same tint.
    #[test]
    fn the_state_comes_from_the_map() {
        assert_eq!(classify(Edge::DoorClosed), Some((Kind::Door, State::Shut)));
        assert_eq!(classify(Edge::DoorOpen), Some((Kind::Door, State::Open)));
        assert_eq!(classify(Edge::DoorBroken), Some((Kind::Door, State::Open)));
        assert_eq!(classify(Edge::Window), Some((Kind::Window, State::Shut)));
        assert_eq!(classify(Edge::WindowOpen), Some((Kind::Window, State::Open)));
        assert_eq!(classify(Edge::WindowBroken), Some((Kind::Window, State::Open)));
        for e in [Edge::None, Edge::Wall, Edge::Arch, Edge::Fence] {
            assert_eq!(classify(e), None, "{e:?}");
        }
        let mut map = openings();
        let door = (IVec2::new(12, 10), N);
        let p = Vec2::new(12.5, 7.0);
        let state = |map: &TileMap| found(map, p, 0, None).iter().find(|h| h.key == door).map(|h| (h.kind, h.state));
        assert_eq!(state(&map), Some((Kind::Door, State::Shut)));
        map.set_edge(door.0, Side::N, Edge::DoorOpen);
        assert_eq!(state(&map), Some((Kind::Door, State::Open)));
        map.set_edge(door.0, Side::N, Edge::Wall);
        assert_eq!(state(&map), None);
        // The tag: bit 8 an open window (for its mesh), bit 10 barred; the low byte how much shows.
        let hint = |kind, state| Hint { key: door, kind, state };
        assert_eq!(tag_of(hint(Kind::Door, State::Shut), 1.0), 255);
        assert_eq!(tag_of(hint(Kind::Window, State::Shut), 1.0), 255);
        assert_eq!(tag_of(hint(Kind::Window, State::Open), 1.0), 255 | 1 << 8);
        assert_eq!(tag_of(hint(Kind::Door, State::Open), 0.0), 0);
        assert_eq!(tag_of(hint(Kind::Door, State::Open), 1.0), tag_of(hint(Kind::Door, State::Shut), 1.0));
        assert_eq!(tag_of(hint(Kind::Door, State::Barred), 1.0), 255 | 1 << 10);
        assert_eq!(tag_of(hint(Kind::Window, State::Barred), 1.0), 255 | 1 << 10);
        assert!(BARRED < 0.6);
    }

    /// A door in a cut-away wall is outlined by how far its wall is cut, the same blue shut, open
    /// or battered down; a window, a wall and a standing wall's door are not.
    #[test]
    fn a_door_in_a_cut_wall_is_outlined() {
        let mut map = openings();
        let (door, window, wall) = ((IVec2::new(13, 16), N), (IVec2::new(15, 10), N), (IVec2::new(11, 10), N));
        assert_eq!(cut_door(&map, door, 0.0), None, "its wall stands");
        assert_eq!(cut_door(&map, window, 1.0), None);
        assert_eq!(cut_door(&map, wall, 1.0), None);
        let (hint, tag) = cut_door(&map, door, 1.0).unwrap();
        assert_eq!((hint.kind, hint.state, tag >> 8), (Kind::Door, State::Shut, 0));
        assert_eq!(tag & 255, (CUT_DOOR * 255.0).round() as u32);
        let half = cut_door(&map, door, 0.5).unwrap().1 & 255;
        assert!(half > 0 && half < tag & 255, "{half}");
        assert!(!stands_open(&map, door));
        for edge in [Edge::DoorOpen, Edge::DoorBroken] {
            map.set_edge(door.0, Side::N, edge);
            let (hint, tag) = cut_door(&map, door, 1.0).unwrap();
            assert_eq!((hint.state, tag >> 8), (State::Open, 0), "{edge:?}");
            // An open door's outline is drawn where its leaf is; a battered-down one has no leaf.
            assert_eq!(stands_open(&map, door), edge == Edge::DoorOpen);
            assert_eq!(place_cut(door, stands_open(&map, door)) == place(door), edge == Edge::DoorBroken);
        }
        // The inner door between the rooms too: any door, not only outside ones.
        assert!(cut_door(&map, (IVec2::new(14, 12), W), 1.0).is_some());
    }

    /// The building the player is inside gets no hints (it is cut open); the house across the
    /// yard still does. An open porch's room counts as outdoors, so the door from it into the
    /// house is on an outside wall.
    #[test]
    fn the_building_you_are_in_gets_none() {
        let mut map = openings();
        let inside = Vec2::new(11.5, 12.5);
        let a = map.building_at(inside);
        assert!(a.is_some());
        assert!(found(&map, inside, 0, a).is_empty(), "{:?}", found(&map, inside, 0, a));
        assert!(!found(&map, inside, 0, None).is_empty());
        // From inside B, A's hidden openings 14 m off are out of range; B's own north door isn't hinted.
        let in_b = Vec2::new(33.0, 13.0);
        assert!(found(&map, in_b, 0, map.building_at(in_b)).is_empty());
        // A's east room as its porch: the door between the rooms is now an outside door, facing east.
        let east_room = map.room_at(Vec2::new(16.5, 12.5));
        let porch = zombye_sim::template::Porch { min: IVec2::new(14, 10), size: IVec2::new(4, 6), facing: Side::E, room: east_room };
        map.buildings[0].look = Some(zombye_sim::template::BuildingLook { kind: zombye_sim::template::BuildingKind::House, front: Side::S, porch: Some(porch) });
        let inner = (IVec2::new(14, 12), W);
        assert_eq!(outside_wall(&map, inner), Some((0, Vec2::X)));
        // Hidden from the west (angles 2 and 3), in view from the east.
        assert!(found(&map, Vec2::new(20.0, 12.0), 2, None).iter().any(|h| h.key == inner));
        assert!(!found(&map, Vec2::new(20.0, 12.0), 0, None).iter().any(|h| h.key == inner));
    }

    /// Hints fade in over `FADE_SECS` and out again, are worked out again only when the player
    /// has moved, the camera turned or the map changed, and the setting switches them all off.
    #[test]
    fn hints_fade_in_and_out() {
        let mut map = openings();
        let mut hints = OpeningHints::new(true);
        let door = (IVec2::new(12, 10), N);
        let (near, far) = (Vec2::new(12.5, 7.0), Vec2::new(12.5, -8.0));
        assert!(hints.decide(&map, false, near, camera(0), None));
        assert_eq!(hints.shown(door), Some(0.0));
        assert!(hints.ease(FADE_SECS * 0.5));
        let half = hints.shown(door).unwrap();
        assert!((half - 0.5).abs() < 1e-4, "{half}");
        assert!(hints.ease(FADE_SECS * 0.6));
        assert_eq!(hints.shown(door), Some(1.0));
        // Standing still: nothing is decided again and nothing moves.
        assert!(!hints.decide(&map, false, near + Vec2::new(0.1, 0.1), camera(0), None));
        assert!(!hints.ease(0.016));
        // The door opens: decided again at once, it stays shown.
        map.set_edge(door.0, Side::N, Edge::DoorOpen);
        assert!(hints.decide(&map, true, near, camera(0), None));
        let slot = hints.slots.iter().find(|s| s.hint.key == door).unwrap();
        assert_eq!((slot.hint.state, slot.shown, slot.wanted), (State::Open, 1.0, true));
        // The camera turns to look at that wall: it fades out over the same time.
        assert!(hints.decide(&map, false, near, camera(2), None));
        assert!(hints.ease(FADE_SECS * 0.5));
        assert!((hints.shown(door).unwrap() - 0.5).abs() < 1e-4);
        // Turned back before it has gone, it comes back from where it was.
        assert!(hints.decide(&map, false, near, camera(0), None));
        assert!(hints.ease(FADE_SECS * 0.25));
        assert!((hints.shown(door).unwrap() - 0.75).abs() < 1e-4);
        // Walking away: out again; switched off: every hint goes.
        assert!(hints.decide(&map, false, far, camera(0), None));
        assert!(hints.slots.iter().all(|s| !s.wanted));
        hints.decide(&map, false, near, camera(0), None);
        hints.ease(1.0);
        hints.on = false;
        assert!(hints.decide(&map, false, near, camera(0), None));
        hints.ease(FADE_SECS);
        assert!(hints.slots.iter().all(|s| !s.wanted && s.shown == 0.0));
    }

    /// The outline meshes are small, and the look dims at night without changing every frame.
    #[test]
    fn the_outlines_are_small_and_dim_at_night() {
        for (mesh, quads) in [(outline(Vec2::ZERO, Vec2::new(1.0, 2.0), 0.07, false, None), 3), (outline(Vec2::ZERO, Vec2::ONE, 0.06, true, Some(true)), 6), (outline(Vec2::ZERO, Vec2::ONE, 0.06, true, Some(false)), 5)] {
            assert_eq!(mesh.count_vertices(), quads * 4);
        }
        let (day, night) = (look_at(1.0), look_at(0.0));
        assert!(night.params.z < day.params.z * 0.4 && day.params.z == 1.0);
        assert_eq!(look_at(0.50), look_at(0.51));
        assert!(CLEAR > 0.0 && CLEAR < 0.01);
        // West edges run along +z, north edges along +x.
        let along = |key| place(key).transform_point(Vec3::X);
        assert!(along((IVec2::new(3, 5), N)).distance(Vec3::new(4.0, 0.0, 5.0)) < 1e-5);
        assert!(along((IVec2::new(3, 5), W)).distance(Vec3::new(3.0, 0.0, 6.0)) < 1e-5);
    }

    /// The shipped town: every hinted opening is a door or window between a building and the
    /// outdoors, and from each angle a house's hinted and unhinted outside openings split by side.
    #[test]
    fn the_towns_houses_hint_only_their_hidden_outside_openings() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let templates = zombye_net::content::load_building_templates(&data).expect("templates");
        let map = zombye_sim::worldgen::generate(&zombye_sim::worldgen::GenConfig::default(), &templates).map;
        let (mut hinted, mut doors) = (0, 0);
        for b in map.buildings.iter().take(40) {
            let p = b.min.as_vec2() + b.size.as_vec2() * 0.5;
            for angle in 0..4 {
                let c = ground_dir(camera(angle)).0;
                for h in found(&map, p, angle, None) {
                    let (_, outward) = outside_wall(&map, h.key).expect("an outside wall");
                    assert!(outward.dot(c) < 0.0, "{}: a wall facing the camera", b.template);
                    let tile = map.tile(h.key.0);
                    let edge = if h.key.1 == N { tile.north } else { tile.west };
                    assert!(edge.is_door() || edge.is_window(), "{edge:?}");
                    hinted += 1;
                    doors += (h.kind == Kind::Door) as usize;
                }
            }
        }
        assert!(hinted > 300 && doors > 40, "{hinted} hints, {doors} doors");
    }
}
