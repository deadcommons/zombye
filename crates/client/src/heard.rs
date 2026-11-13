//! Players heard and not seen (#66, docs/PLAN.md §4.8).
//!
//! With player against player on, the server tells us of another survivor only while ours sees
//! them. One it only hears (footsteps behind a wall or behind our back, a swing, a door) comes as
//! a sound at a position ([`ServerLink::heard`]): here that is a pale ring on the ground where the
//! sound came from, spreading and fading over a second and a half, wider the louder it was. The
//! sound itself is `audio.rs`'s.

use bevy::prelude::*;

use crate::daynight::Daylight;
use crate::fx::ClearFx;
use crate::server_link::ServerLink;

/// How long a mark lasts (seconds), and how opaque it starts.
const LIFE: f32 = 1.5;
const ALPHA: f32 = 0.7;
/// Marks alive at once (a running player is heard about twice a second).
const MAX_MARKS: usize = 24;

pub struct HeardPlugin;

impl Plugin for HeardPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup).add_systems(Update, (spawn_marks, fade_marks).chain().run_if(resource_exists::<ServerLink>));
    }
}

#[derive(Resource)]
struct HeardAssets {
    ring: Handle<Mesh>,
}

/// A ring where a sound came from: its age and the radius it spreads to (metres).
#[derive(Component)]
struct HeardMark {
    age: f32,
    reach: f32,
}

/// How wide the mark of a sound carrying `radius` metres gets: sneaking 0.6 m, walking 0.9,
/// running 1.3, breaking glass 1.6.
pub fn mark_reach(radius: f32) -> f32 {
    (0.55 + radius * 0.055).clamp(0.6, 1.6)
}

/// The ring's size and opacity `age` seconds in: out fast, then slowly, fading all the while.
pub fn mark_at(age: f32, reach: f32) -> (f32, f32) {
    let t = (age / LIFE).clamp(0.0, 1.0);
    let out = 1.0 - (1.0 - t) * (1.0 - t);
    (reach * (0.25 + 0.75 * out), ALPHA * (1.0 - t) * (1.0 - t))
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    // Flat on the ground, 1 m across at scale 1.
    let ring = Mesh::from(Annulus::new(0.88, 1.0)).rotated_by(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2));
    commands.insert_resource(HeardAssets { ring: meshes.add(ring) });
}

fn spawn_marks(
    mut commands: Commands,
    link: Res<ServerLink>,
    assets: Res<HeardAssets>,
    daylight: Option<Res<Daylight>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    marks: Query<(Entity, &HeardMark)>,
    mut cleared: MessageReader<ClearFx>,
) {
    // A quickload or a new run: these sounds belong to the world left behind.
    if cleared.read().count() > 0 {
        for (e, _) in &marks {
            commands.entity(e).despawn();
        }
    }
    if link.heard.is_empty() {
        return;
    }
    // Too many at once: the oldest go.
    let mut alive: Vec<(Entity, f32)> = marks.iter().map(|(e, m)| (e, m.age)).collect();
    alive.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (e, _) in alive.iter().take((alive.len() + link.heard.len()).saturating_sub(MAX_MARKS)) {
        commands.entity(*e).despawn();
    }
    // Paler by day, when the ground is bright; unlit, so it shows in the dark it is heard in.
    let day = daylight.map_or(1.0, |d| d.0);
    let shade = 0.55 + 0.4 * day;
    for h in &link.heard {
        let reach = mark_reach(h.radius);
        let (size, alpha) = mark_at(0.0, reach);
        let material = materials.add(StandardMaterial { base_color: Color::srgba(shade, shade, shade * 0.92, alpha), unlit: true, alpha_mode: AlphaMode::Blend, depth_bias: 60.0, ..default() });
        commands.spawn((HeardMark { age: 0.0, reach }, Mesh3d(assets.ring.clone()), MeshMaterial3d(material), Transform::from_xyz(h.pos.x, 0.07, h.pos.y).with_scale(Vec3::splat(size))));
    }
}

fn fade_marks(time: Res<Time>, mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>, mut marks: Query<(Entity, &mut HeardMark, &mut Transform, &MeshMaterial3d<StandardMaterial>)>) {
    for (e, mut mark, mut tf, material) in &mut marks {
        mark.age += time.delta_secs();
        if mark.age >= LIFE {
            materials.remove(&material.0);
            commands.entity(e).despawn();
            continue;
        }
        let (size, alpha) = mark_at(mark.age, mark.reach);
        tf.scale = Vec3::splat(size);
        if let Some(mut m) = materials.get_mut(&material.0) {
            m.base_color.set_alpha(alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_spread_and_fade() {
        assert!(mark_reach(1.2) < mark_reach(6.0) && mark_reach(6.0) < mark_reach(14.0));
        assert_eq!(mark_reach(100.0), 1.6);
        let reach = mark_reach(14.0);
        let (s0, a0) = mark_at(0.0, reach);
        let (s1, a1) = mark_at(LIFE * 0.5, reach);
        let (s2, a2) = mark_at(LIFE, reach);
        assert!(s0 < s1 && s1 < s2 && (s2 - reach).abs() < 1e-4, "{s0} {s1} {s2}");
        assert!(a0 == ALPHA && a1 < a0 && a2 == 0.0, "{a0} {a1} {a2}");
    }
}
