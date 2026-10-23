//! Line of sight on the tile map. Walls and closed doors block sight; windows, fences, open doors
//! and archways don't. Used for the client's fog of war and zombie senses. Also a "clear walking
//! line" test (anything that blocks movement blocks it), for steering straight at a target.

use glam::{IVec2, Vec2};

use crate::map::{Side, TileMap};

/// Distance a ray from `origin` along unit `dir` travels before hitting something opaque, capped at `max`.
pub fn cast_ray(map: &TileMap, origin: Vec2, dir: Vec2, max: f32) -> f32 {
    cast(map, origin, dir, max, |map, tile, side| map.edge(tile, side).blocks_sight())
}

/// Can something walk in a straight line from `a` to `b`? Walls, windows, closed doors, fences
/// and blocked tiles (trees, furniture) in the way say no.
pub fn clear_path(map: &TileMap, a: Vec2, b: Vec2) -> bool {
    let d = b - a;
    let len = d.length();
    len < 1e-4 || cast(map, a, d / len, len, |map, tile, side| !map.can_step(tile, side)) >= len - 1e-3
}

/// Walk the tiles a ray crosses (DDA) until `blocks(map, tile, side)` says the crossing out of
/// `tile` through `side` is blocked. Returns the distance travelled, capped at `max`.
fn cast(map: &TileMap, origin: Vec2, dir: Vec2, max: f32, blocks: impl Fn(&TileMap, IVec2, Side) -> bool) -> f32 {
    let mut tile = origin.floor().as_ivec2();
    let step = IVec2::new(if dir.x >= 0.0 { 1 } else { -1 }, if dir.y >= 0.0 { 1 } else { -1 });
    let next_boundary = |o: f32, t: i32, s: i32| if s > 0 { (t + 1) as f32 - o } else { o - t as f32 };
    let inv = Vec2::new(if dir.x.abs() > 1e-9 { 1.0 / dir.x.abs() } else { f32::INFINITY }, if dir.y.abs() > 1e-9 { 1.0 / dir.y.abs() } else { f32::INFINITY });
    let mut t_max = Vec2::new(next_boundary(origin.x, tile.x, step.x) * inv.x, next_boundary(origin.y, tile.y, step.y) * inv.y);
    loop {
        let (t, side) = if t_max.x < t_max.y {
            (t_max.x, if step.x > 0 { Side::E } else { Side::W })
        } else {
            (t_max.y, if step.y > 0 { Side::S } else { Side::N })
        };
        if t >= max {
            return max;
        }
        if blocks(map, tile, side) {
            return t;
        }
        tile += side.offset();
        if !map.in_bounds(tile) {
            return t;
        }
        match side {
            Side::E | Side::W => t_max.x += inv.x,
            _ => t_max.y += inv.y,
        }
    }
}

/// `n` ray distances evenly spaced around `origin`, starting along +x and turning toward +y.
pub fn visibility_rays(map: &TileMap, origin: Vec2, max: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            cast_ray(map, origin, Vec2::from_angle(a), max)
        })
        .collect()
}

pub fn line_of_sight(map: &TileMap, a: Vec2, b: Vec2) -> bool {
    let d = b - a;
    let len = d.length();
    len < 1e-4 || cast_ray(map, a, d / len, len) >= len - 1e-3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::Edge;

    #[test]
    fn walls_block_sight_windows_do_not() {
        let mut m = TileMap::new(2, 2);
        for y in 0..32 {
            m.set_edge(IVec2::new(10, y), Side::W, Edge::Wall);
        }
        let eye = Vec2::new(5.5, 5.5);
        assert!((cast_ray(&m, eye, Vec2::X, 50.0) - 4.5).abs() < 1e-3);
        assert!(!line_of_sight(&m, eye, Vec2::new(15.5, 5.5)));
        m.set_edge(IVec2::new(10, 5), Side::W, Edge::Window);
        assert!(line_of_sight(&m, eye, Vec2::new(15.5, 5.5)));
        m.set_edge(IVec2::new(10, 5), Side::W, Edge::DoorClosed);
        assert!(!line_of_sight(&m, eye, Vec2::new(15.5, 5.5)));
    }

    #[test]
    fn windows_block_walking_but_not_sight() {
        let mut m = TileMap::new(2, 2);
        let (a, b) = (Vec2::new(5.5, 5.5), Vec2::new(15.5, 5.5));
        assert!(clear_path(&m, a, b));
        m.set_edge(IVec2::new(10, 5), Side::W, Edge::Window);
        assert!(line_of_sight(&m, a, b));
        assert!(!clear_path(&m, a, b));
        m.set_edge(IVec2::new(10, 5), Side::W, Edge::DoorOpen);
        assert!(clear_path(&m, a, b));
    }
}
