//! Street props (#61): the mailboxes, hydrants and bins the town generator placed
//! (`TileMap::street_props`), merged into a chunk's props mesh, so they get the world material (fog
//! of war, lighting, shadows) and cost no draw call of their own. Each is its kind's model from the
//! furniture models (`FurnitureModels`, `ZOMBYE_EXTRA_MODELS` included), at its own size in the
//! middle of its tile (of the tiles it covers, for a kind bigger than one: a parked car, a shed),
//! facing the street; a kind with no model is a grey-box of its size and colour from
//! `street_props.ron`. A prop is drawn by the chunk its first tile is in.

use bevy::prelude::*;
use zombye_sim::map::{CHUNK_SIZE, Side, StreetProp, TileMap};

use super::{CutTag, MeshBuilder, rgb, same, shade};
use crate::furniture_models::FurnitureModels;

/// The street props standing in chunk `c`, in tile order.
fn in_chunk(map: &TileMap, c: IVec2) -> impl Iterator<Item = &StreetProp> {
    let (lo, hi) = (c * CHUNK_SIZE, (c + IVec2::ONE) * CHUNK_SIZE);
    let props = &map.street_props;
    let start = props.partition_point(|p| (p.tile.y, p.tile.x) < (lo.y, lo.x));
    props[start..].iter().take_while(move |p| p.tile.y < hi.y).filter(move |p| p.tile.x >= lo.x && p.tile.x < hi.x)
}

/// Add the street props of chunk `c` to its props mesh.
pub fn add(mb: &mut MeshBuilder, map: &TileMap, c: IVec2, models: &FurnitureModels) {
    mb.set_tag(CutTag::default());
    for p in in_chunk(map, c) {
        let Some(kind) = map.prop_kinds.get(p.kind as usize) else { continue };
        let centre = centre(map, p);
        // Worn as its kind is (decay.ron): dust, rust, and for vehicles a paint of their own.
        let wear = super::decay::Wear::of(super::decay::look(), &kind.id, centre, models.body_colour(kind.model(), centre));
        if !models.add_at_worn(mb, kind.model(), centre, p.facing, 0.0, wear.as_ref()) {
            grey_box(mb, centre, p.facing, kind.size, kind.color);
        }
    }
}

/// The middle of the tiles a prop covers.
fn centre(map: &TileMap, p: &StreetProp) -> Vec2 {
    let (min, size) = map.street_prop_rect(p);
    min.as_vec2() + size.as_vec2() * 0.5
}

/// A box of the kind's size and colour, its front (south when facing south) a little darker.
fn grey_box(mb: &mut MeshBuilder, centre: Vec2, facing: Side, (w, d, h): (f32, f32, f32), (r, g, b): (f32, f32, f32)) {
    let (w, d) = if matches!(facing, Side::E | Side::W) { (d, w) } else { (w, d) };
    let c = rgb(r, g, b);
    let mut faces = same(c);
    faces[2] = shade(c, 1.1);
    // Face order: +x, -x, +y, -y, +z, -z.
    let front = match facing {
        Side::E => 0,
        Side::W => 1,
        Side::S => 4,
        Side::N => 5,
    };
    faces[front] = shade(c, 0.8);
    let half = Vec2::new(w, d) * 0.5;
    mb.add_box(Vec3::new(centre.x - half.x, 0.0, centre.y - half.y), Vec3::new(centre.x + half.x, h, centre.y + half.y), faces);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshgen::landscape::build_chunk;
    use zombye_sim::worldgen::{GenConfig, generate_with};

    fn data() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data")
    }

    fn town() -> TileMap {
        let templates = zombye_net::content::load_building_templates(&data()).expect("templates");
        let props = zombye_net::content::load_street_props(&data()).expect("street props");
        generate_with(&GenConfig::default(), &templates, &props).map
    }

    /// The furniture models, plus `ZOMBYE_EXTRA_MODELS` as the client takes them.
    fn models() -> FurnitureModels {
        let mut m = FurnitureModels::load(&data().join("../models/furniture"));
        m.extend(FurnitureModels::load(&data().join("../models/street")));
        for dir in std::env::var("ZOMBYE_EXTRA_MODELS").unwrap_or_default().split(':').filter(|d| !d.is_empty()) {
            m.extend(FurnitureModels::load(std::path::Path::new(dir)));
        }
        m
    }

    /// Every prop is drawn by the chunk it stands in, once, inside the tiles it covers, on the
    /// ground; a kind with no model is a grey-box of its size.
    #[test]
    fn every_prop_is_drawn_once_in_its_tile() {
        let map = town();
        assert!(!map.street_props.is_empty());
        let mut seen = 0;
        for cy in 0..map.chunks_y {
            for cx in 0..map.chunks_x {
                for p in in_chunk(&map, IVec2::new(cx, cy)) {
                    assert_eq!(p.tile.div_euclid(IVec2::splat(CHUNK_SIZE)), IVec2::new(cx, cy));
                    seen += 1;
                }
            }
        }
        assert_eq!(seen, map.street_props.len());
        let none = FurnitureModels::default();
        // Some of every kind, the big ones among them.
        let mut left = vec![4; map.prop_kinds.len()];
        for p in &map.street_props {
            if left[p.kind as usize] == 0 {
                continue;
            }
            left[p.kind as usize] -= 1;
            let kind = &map.prop_kinds[p.kind as usize];
            let mut mb = MeshBuilder::default();
            add(&mut mb, &map, p.tile.div_euclid(IVec2::splat(CHUNK_SIZE)), &none);
            // The chunk's props: find this one's box among them by its tile.
            let mut one = MeshBuilder::default();
            grey_box(&mut one, centre(&map, p), p.facing, kind.size, kind.color);
            let (lo, hi) = one.bounds();
            let (min, size) = map.street_prop_rect(p);
            let (min, max) = (min.as_vec2(), (min + size).as_vec2());
            assert!(lo.x >= min.x && lo.z >= min.y && hi.x <= max.x && hi.z <= max.y, "{} at {}: {lo} {hi}", kind.id, p.tile);
            assert!(lo.y.abs() < 1e-4 && (hi.y - kind.size.2).abs() < 1e-4);
            assert!(!mb.is_empty());
        }
    }

    /// The street props with models of their own (assets/models/street) stand at their own size, inside
    /// their tile, whichever way they face. The rest are grey-box until they have one.
    #[test]
    fn models_keep_their_own_size() {
        let models = models();
        let map = town();
        for kind in map.prop_kinds.iter().filter(|k| ["mailbox", "fire_hydrant", "trash_can"].contains(&k.id.as_str())) {
            for facing in Side::ALL {
                let mut mb = MeshBuilder::default();
                assert!(models.add_at(&mut mb, kind.model(), Vec2::new(10.5, 20.5), facing, 0.0), "no model for {}", kind.id);
                let (lo, hi) = mb.bounds();
                let size = hi - lo;
                let (w, d) = if matches!(facing, Side::E | Side::W) { (kind.size.1, kind.size.0) } else { (kind.size.0, kind.size.1) };
                assert!((size.x - w).abs() < 0.1 && (size.z - d).abs() < 0.1 && (size.y - kind.size.2).abs() < 0.1, "{} {facing:?}: {size} vs {:?}", kind.id, kind.size);
                assert!(lo.x >= 10.0 && lo.z >= 20.0 && hi.x <= 11.0 && hi.z <= 21.0 && lo.y.abs() < 0.02, "{} {facing:?}: {lo} {hi}", kind.id);
            }
        }
    }

    /// What drawing the street props adds to building a chunk
    /// (`cargo test -p zombye-client --release street_props_build_time -- --ignored --nocapture`,
    /// with `ZOMBYE_EXTRA_MODELS` for the models still in the art repo).
    #[test]
    #[ignore]
    fn street_props_build_time() {
        let map = town();
        let models = models();
        let (mut base, mut with, mut tris) = (Vec::new(), Vec::new(), 0);
        for _ in 0..3 {
            for cy in 0..map.chunks_y {
                for cx in 0..map.chunks_x {
                    let c = IVec2::new(cx, cy);
                    let start = std::time::Instant::now();
                    let mut m = build_chunk(&map, c);
                    base.push(start.elapsed().as_secs_f64() * 1000.0);
                    let before = m.props.tri_count();
                    add(&mut m.props, &map, c, &models);
                    with.push(start.elapsed().as_secs_f64() * 1000.0);
                    tris += m.props.tri_count() - before;
                }
            }
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        eprintln!(
            "{} street props ({} models); chunk build mean {:.3} ms without drawing them, {:.3} ms with; {} triangles of street props a chunk",
            map.street_props.len(),
            models.len(),
            mean(&base),
            mean(&with),
            tris / 3 / (map.chunks_x * map.chunks_y) as usize
        );
    }
}
