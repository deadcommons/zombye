//! Items lying on the floor, drawn from the snapshot's full item list as grey-box shapes in each
//! item's colour: weapons as bars of their length, anything else as a small box about its size.
//! An item a release build has a model for (`release_models.rs`, `items/<item id>.glb`) is drawn as
//! that, at its own size. Hidden under the fog of war like zombies.

use std::collections::HashMap;

use bevy::prelude::*;
use zombye_sim::ItemStack;

use crate::fog::FogState;
use crate::server_link::{JoinInfo, ServerLink};

/// How items look on the ground and in the hand: one unit cube, a material and size per item, the
/// release models where there are any, and each item's look in a hand (#35).
#[derive(Resource)]
pub struct WeaponLooks {
    pub cube: Handle<Mesh>,
    /// One material per item (by index), and the size (x, y, z) it's drawn at on the floor.
    item_materials: Vec<Handle<StandardMaterial>>,
    item_sizes: Vec<Vec3>,
    /// A release build's model of an item (by index), where it has one, and the material they share.
    item_models: Vec<Option<Handle<Mesh>>>,
    /// Which items are logs (tag `log`), drawn as `log` when they have no model.
    item_logs: Vec<bool>,
    log: Handle<Mesh>,
    model_material: Handle<StandardMaterial>,
    /// How each item (by index) is drawn in a hand: its mesh, material and place in the grip frame,
    /// the item's grip from `held.ron` applied; `None` for bags (the outfit draws those) and bare hands.
    hand_looks: Vec<Option<(Handle<Mesh>, Handle<StandardMaterial>, Transform)>>,
}

/// Bar thickness and the shortest bar drawn (so a frying pan still shows up).
const THICK: f32 = 0.06;
const MIN_LENGTH: f32 = 0.2;

impl WeaponLooks {
    /// How item `item` (an index) is drawn in a hand, in the hand's grip frame (the grip at the
    /// origin, pointing along +Z, its top +Y): its held model; else, a weapon, its bar; else its
    /// ground model laid along +Z in the fist; else a box of its size. `None` for a bag (the outfit
    /// hangs those from the hand) or anything not drawn.
    pub fn in_hand(&self, item: u16) -> Option<(Handle<Mesh>, Handle<StandardMaterial>, Transform)> {
        self.hand_looks.get(item as usize)?.clone()
    }
}

/// The bounds of a mesh's positions.
fn bounds(mesh: &Mesh) -> Option<(Vec3, Vec3)> {
    let pos = mesh.attribute(Mesh::ATTRIBUTE_POSITION)?.as_float3()?;
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for p in pos {
        lo = lo.min(Vec3::from_array(*p));
        hi = hi.max(Vec3::from_array(*p));
    }
    (lo.x <= hi.x).then_some((lo, hi))
}

/// Something lying on the ground (`lo`..`hi`, its top +Y) put in the fist: its longer side along
/// the hand's +Z, its middle in the grip.
fn into_fist(lo: Vec3, hi: Vec3) -> Transform {
    let size = hi - lo;
    // Turn the longer of x and z to +Z (a quarter turn about Y takes +X to +Z).
    let turn = if size.x > size.z { Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2) } else { Quat::IDENTITY };
    let mid = turn * ((lo + hi) * 0.5);
    Transform::from_translation(-mid).with_rotation(turn)
}

/// A grip (from `held.ron`) put on top of where an item already sits in the hand.
fn gripped(id: &str, base: Transform) -> Transform {
    let g = crate::held::grip(id).transform();
    Transform::from_matrix(g.to_matrix() * base.to_matrix())
}

#[derive(Resource, Default)]
struct ItemEntities(HashMap<u32, (Entity, ItemStack)>);

pub struct ItemsRenderPlugin;

impl Plugin for ItemsRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ItemEntities>().add_systems(Startup, setup).add_systems(Update, sync_items);
    }
}

fn setup(mut commands: Commands, join: Res<JoinInfo>, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let mut mat = |c: (f32, f32, f32)| materials.add(StandardMaterial { base_color: Color::srgb(c.0, c.1, c.2), perceptual_roughness: 0.6, ..default() });
    let weapon_mats: Vec<Handle<StandardMaterial>> = join.weapons.iter().map(|w| mat(w.color)).collect();
    let item_materials: Vec<Handle<StandardMaterial>> = join.items.iter().map(|d| mat(d.color)).collect();
    let item_materials_for_hand = item_materials.clone();
    let lengths: Vec<f32> = join.weapons.iter().map(|w| w.length).collect();
    let item_sizes: Vec<Vec3> = join
        .items
        .iter()
        .map(|d| {
            let weapon = d.weapon.as_ref().and_then(|id| join.weapons.iter().position(|w| &w.id == id));
            match weapon {
                Some(w) => Vec3::new(lengths[w].max(MIN_LENGTH), THICK, THICK * 1.4),
                None if d.has_tag("log") => Vec3::ONE,
                None => {
                    // A box of about its volume, flattened a little, never too small to see.
                    let side = (d.volume_l / 1000.0).cbrt().clamp(0.08, 0.45);
                    Vec3::new(side * 1.2, side * 0.7, side)
                }
            }
        })
        .collect();
    let item_sizes_for_hand = item_sizes.clone();
    let released = crate::release_models::items();
    // A log (tagged `log`) with no model of its own lies as a length of trunk, 0.95 m by 0.26 m.
    let log = meshes.add(Cylinder::new(0.13, 0.95).mesh().resolution(7).build().rotated_by(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)).translated_by(Vec3::Y * 0.13));
    let item_models = join.items.iter().map(|d| released.mesh(&d.id).map(|m| meshes.add(m))).collect();
    let item_logs = join.items.iter().map(|d| d.has_tag("log")).collect();
    let in_hand = crate::release_models::held();
    let model_material = materials.add(StandardMaterial { base_color: Color::WHITE, perceptual_roughness: 0.8, ..default() });
    let hand_looks = join
        .items
        .iter()
        .enumerate()
        .map(|(i, d)| {
            if d.bag.is_some() {
                return None;
            }
            if let Some(m) = in_hand.mesh(&d.id) {
                return Some((meshes.add(m), model_material.clone(), gripped(&d.id, Transform::IDENTITY)));
            }
            let weapon = d.weapon.as_ref().and_then(|id| join.weapons.iter().position(|w| &w.id == id));
            if let Some(w) = weapon {
                let len = lengths[w];
                if len <= 0.0 {
                    return None;
                }
                let len = len.max(MIN_LENGTH);
                // The handle sticks out a little behind the fist.
                let bar = Transform::from_xyz(0.0, 0.0, len * 0.5 - 0.1).with_scale(Vec3::new(0.05, 0.05, len));
                return Some((cube.clone(), weapon_mats[w].clone(), gripped(&d.id, bar)));
            }
            if let Some(m) = released.mesh(&d.id) {
                let place = bounds(&m).map_or(Transform::IDENTITY, |(lo, hi)| into_fist(lo, hi));
                return Some((meshes.add(m), model_material.clone(), gripped(&d.id, place)));
            }
            // A box of its size, in the fist, held as it would lie (its long side along the hand).
            let s: Vec3 = item_sizes_for_hand[i];
            let place = into_fist(-s * 0.5, s * 0.5);
            Some((cube.clone(), item_materials_for_hand[i].clone(), gripped(&d.id, place.with_scale(place.scale * s))))
        })
        .collect();
    commands.insert_resource(WeaponLooks { cube, item_materials, item_sizes, item_models, item_logs, log, model_material, hand_looks });
}

fn sync_items(mut commands: Commands, link: Res<ServerLink>, map: Res<crate::server_link::ClientMap>, fog: Res<FogState>, view: Res<crate::player::ViewFloor>, looks: Option<Res<WeaponLooks>>, mut entities: ResMut<ItemEntities>, mut query: Query<(&mut Visibility, &mut Transform)>) {
    let Some(looks) = looks else { return };
    let items = &link.curr.items;
    // Gone (picked up) or changed into another item: despawn (and respawn with the new look).
    entities.0.retain(|id, (e, s)| {
        let keep = items.iter().any(|it| it.id == *id && it.stack.item == s.item);
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    for it in items {
        // Things lying upstairs (stairs) are drawn on that floor, and only while it is the one open.
        let up = link.curr.upstairs.binary_search(&it.id).is_ok();
        let lift = if up { zombye_sim::map::STOREY } else { 0.0 } + map.0.ground(it.pos);
        let visibility = if fog.visibility(it.pos) > 0.35 && (!up || view.level > 0) { Visibility::Inherited } else { Visibility::Hidden };
        match entities.0.get(&it.id) {
            Some(&(e, _)) => {
                if let Ok((mut v, mut tf)) = query.get_mut(e) {
                    if *v != visibility {
                        *v = visibility;
                    }
                    let at = Vec3::new(it.pos.x, tf.translation.y, it.pos.y);
                    if tf.translation != at {
                        tf.translation = at;
                    }
                }
            }
            None => {
                let e = match looks.item_models.get(it.stack.item as usize).cloned().flatten() {
                    // A model lies on the floor at its own size.
                    Some(model) => {
                        let tf = Transform::from_xyz(it.pos.x, 0.01 + lift, it.pos.y).with_rotation(Quat::from_rotation_y(-it.angle));
                        commands.spawn((Mesh3d(model), MeshMaterial3d(looks.model_material.clone()), tf, visibility)).id()
                    }
                    None if looks.item_logs.get(it.stack.item as usize).copied().unwrap_or(false) => {
                        let tf = Transform::from_xyz(it.pos.x, 0.01 + lift, it.pos.y).with_rotation(Quat::from_rotation_y(-it.angle));
                        let material = looks.item_materials.get(it.stack.item as usize).cloned().unwrap_or_default();
                        commands.spawn((Mesh3d(looks.log.clone()), MeshMaterial3d(material), tf, visibility)).id()
                    }
                    None => {
                        let size = looks.item_sizes.get(it.stack.item as usize).copied().unwrap_or(Vec3::splat(0.15));
                        let tf = Transform::from_xyz(it.pos.x, size.y * 0.5 + 0.01 + lift, it.pos.y).with_rotation(Quat::from_rotation_y(-it.angle)).with_scale(size);
                        let material = looks.item_materials.get(it.stack.item as usize).cloned().unwrap_or_default();
                        commands.spawn((Mesh3d(looks.cube.clone()), MeshMaterial3d(material), tf, visibility)).id()
                    }
                };
                entities.0.insert(it.id, (e, it.stack));
            }
        }
    }
}
