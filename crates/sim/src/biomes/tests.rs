use glam::IVec2;

use super::*;
use crate::map::{Feature, Floor, TileMap};
use crate::street_props::StreetPropsDef;
use crate::template::tests::shipped_templates;
use crate::worldgen::{GenConfig, Generated, generate_full};

fn town(seed: u64, biomes: &BiomeData) -> Generated {
    generate_full(&GenConfig { seed, ..Default::default() }, &shipped_templates(), &StreetPropsDef::default(), biomes)
}

fn is_tree(f: Feature) -> bool {
    matches!(f, Feature::Tree | Feature::PineTree)
}

/// Trees per 100 m² over the tiles of the map that have a biome.
fn trees_per_100m2(map: &TileMap) -> f32 {
    let (mut trees, mut tiles) = (0, 0);
    for y in 0..map.height() {
        for x in 0..map.width() {
            let p = IVec2::new(x, y);
            if map.biomes.at(p).is_some() {
                tiles += 1;
                trees += is_tree(map.tile(p).feature) as i32;
            }
        }
    }
    trees as f32 * 100.0 / tiles as f32
}

#[test]
fn the_shipped_biomes_are_sound() {
    let data = parse_biomes(SHIPPED).unwrap();
    assert!(data.biomes.len() >= 5);
    assert_eq!(&data, shipped());
    assert_eq!(data.everywhere, None, "the shipped file lays every biome out");
    for b in &data.biomes {
        assert!(!b.name.is_empty(), "{} has no name", b.id);
    }
}

#[test]
fn mistakes_name_the_biome_and_the_field() {
    let with = |from: &str, to: &str| {
        assert!(SHIPPED.contains(from), "the shipped file has no {from:?}");
        parse_biomes(&SHIPPED.replacen(from, to, 1)).unwrap_err()
    };
    let e = with("id: \"meadow\"", "id: \"wood_edge\"");
    assert!(e.contains("wood_edge") && e.contains("twice"), "{e}");
    let e = with("everywhere: None", "everywhere: Some(\"swamp\")");
    assert!(e.contains("swamp") && e.contains("meadow"), "{e}");
    let e = with("thick: 14.0", "thick: 70.0");
    assert!(e.contains("mixed_woods") && e.contains("trees.thick"), "{e}");
    let e = with("spacing: 3", "spacing: 0");
    assert!(e.contains("meadow") && e.contains("spacing"), "{e}");
    let e = with("\"#6e7345\"", "\"green\"");
    assert!(e.contains("green"), "{e}");
    let e = with("ragged_m: 3.0", "ragged_m: 3.0, wobble: 2");
    assert!(e.contains("wobble"), "{e}");
    // A biome with trees must say which.
    let e = parse_biomes("(biomes: [(id: \"bare\", trees: (thick: 2.0))])").unwrap_err();
    assert!(e.contains("bare") && e.contains("kinds"), "{e}");
    // Nearly everything may be left out.
    let d = parse_biomes("(biomes: [(id: \"bare\")])").unwrap();
    assert_eq!(d.cell_m, 4);
    assert_eq!(d.biomes[0].label(), "bare");
}

#[test]
fn the_town_has_no_biome_and_all_the_wild_ground_has_one() {
    let g = town(1, shipped());
    let b = &g.map.biomes;
    assert!(!b.is_empty());
    let mut seen = vec![false; b.defs().len()];
    for y in 0..g.map.height() {
        for x in 0..g.map.width() {
            let p = IVec2::new(x, y);
            let outside = p.x < g.town_min.x || p.y < g.town_min.y || p.x >= g.town_max.x || p.y >= g.town_max.y;
            match b.index_at(p) {
                Some(i) => seen[i] = true,
                None => assert!(!outside, "no biome at {p}, outside town"),
            }
            // Nothing built or paved is in a biome's hands.
            if g.map.tile(p).room != 0 {
                assert!(b.at(p).is_none(), "a room at {p} is in a biome");
            }
        }
    }
    // Every biome but the hillside's (the default map has few steep faces) turns up.
    for (i, def) in b.defs().iter().enumerate() {
        assert!(seen[i] || def.id == "rocky_slope", "{} is nowhere on the default map", def.id);
    }
    assert_eq!(b.at(IVec2::new(-1, 0)), None);
    assert_eq!(b.counts().iter().sum::<usize>(), (0..g.map.height()).flat_map(|y| (0..g.map.width()).map(move |x| IVec2::new(x, y))).filter(|&p| b.at(p).is_some()).count());
}

#[test]
fn the_same_seed_grows_the_same_and_the_town_ignores_the_biomes() {
    let a = town(3, shipped());
    let b = town(3, shipped());
    assert_eq!(a.map.biomes, b.map.biomes);
    // Another file's forest: the town, its houses and where you wake up are the same.
    let mut other = shipped().clone();
    for biome in &mut other.biomes {
        biome.trees.thick = (biome.trees.thick * 0.5).max(0.5);
        biome.bushes.per_100m2 = 4.0;
    }
    let c = town(3, &other);
    assert_eq!(a.player_spawn, c.player_spawn);
    assert_eq!(a.map.buildings.len(), c.map.buildings.len());
    let (mut same, mut differ) = (0, 0);
    for y in 0..a.map.height() {
        for x in 0..a.map.width() {
            let p = IVec2::new(x, y);
            let (ta, tb, tc) = (a.map.tile(p), b.map.tile(p), c.map.tile(p));
            assert_eq!(ta, tb, "the same seed and biomes differ at {p}");
            if a.map.biomes.at(p).is_none() {
                assert_eq!(ta, tc, "the town changed with the biomes at {p}");
                same += 1;
            } else if ta != tc {
                differ += 1;
            }
        }
    }
    assert!(same > 50_000 && differ > 500, "{same} town tiles, {differ} wild tiles changed");
}

#[test]
fn one_biome_everywhere_and_biomes_differ() {
    let only = |id: &str| town(2, &shipped().clone().with_everywhere(id).unwrap());
    let meadow = only("meadow");
    let n = meadow.map.biomes.counts();
    let i = shipped().biomes.iter().position(|b| b.id == "meadow").unwrap();
    assert!(n[i] > 50_000 && n.iter().enumerate().all(|(k, &c)| k == i || c == 0), "{n:?}");
    let deep = only("deep_woods");
    let (open, close) = (trees_per_100m2(&meadow.map), trees_per_100m2(&deep.map));
    assert!(open < 0.6, "a meadow has {open} trees per 100 m2");
    assert!(close > 3.0 && close > open * 8.0, "deep woods have {close} trees per 100 m2, a meadow {open}");
    // A pine wood is conifers, a birch grove isn't.
    let share = |g: &Generated| {
        let (mut pines, mut all) = (0, 0);
        for y in 0..g.map.height() {
            for x in 0..g.map.width() {
                let f = g.map.tile(IVec2::new(x, y)).feature;
                if g.map.biomes.at(IVec2::new(x, y)).is_some() && is_tree(f) {
                    all += 1;
                    pines += (f == Feature::PineTree) as i32;
                }
            }
        }
        pines as f32 / all as f32
    };
    assert!(share(&only("pine_wood")) > 0.85);
    assert!(share(&only("birch_grove")) < 0.01);
    assert!(shipped().clone().with_everywhere("swamp").is_err());
}

#[test]
fn trees_keep_their_spacing_and_their_kind_fits_the_map() {
    let g = town(1, shipped());
    let map = &g.map;
    let mut checked = 0;
    for y in 0..map.height() {
        for x in 0..map.width() {
            let p = IVec2::new(x, y);
            let Some(b) = map.biomes.at(p) else { continue };
            let f = map.tile(p).feature;
            if !is_tree(f) {
                continue;
            }
            checked += 1;
            // No other trunk on the eight tiles round it, whatever the biomes on either side
            // of a border say: bodies always have a way between two trees.
            for d in [IVec2::X, IVec2::Y, IVec2::ONE, IVec2::new(1, -1)] {
                assert!(!is_tree(map.tile(p + d).feature) || map.biomes.at(p + d).is_none(), "trunks side by side at {p}");
            }
            // The kind the client draws is one of the biome's, of the map's feature.
            if let Some(k) = b.tree_kind(p, f == Feature::PineTree) {
                assert_eq!(k.kind.feature(), f);
                assert!(b.trees.kinds.iter().any(|o| o.kind == k.kind));
            }
            assert_eq!(map.tile(p).floor, Floor::Grass);
        }
    }
    assert!(checked > 1500, "only {checked} trees in the biomes");
    // Clumps: some ground is inside one and some between.
    let clumps: Vec<f32> = (0..400).map(|i| map.biomes.clump(IVec2::new(4 + i % 20, 4 + i / 20 * 3))).collect();
    assert!(clumps.iter().all(|c| (0.0..=1.0).contains(c)));
}

#[test]
fn a_biome_keeps_to_its_area() {
    // A biome only far from town, another only near it.
    let data = parse_biomes(
        "(ragged_m: 0.0, blend_distance_m: 0.0, biomes: [
            (id: \"near\", area: (town_distance: (0.0, 30.0))),
            (id: \"far\", area: (town_distance: (30.0, 100000.0), in_town: false)),
        ])",
    )
    .unwrap();
    let g = town(1, &data);
    for y in 0..g.map.height() {
        for x in 0..g.map.width() {
            let p = IVec2::new(x, y);
            let Some(b) = g.map.biomes.at(p) else { continue };
            let out = IVec2::new((g.town_min.x - x).max(x - g.town_max.x).max(0), (g.town_min.y - y).max(y - g.town_max.y).max(0)).max_element();
            // Chosen per 4 m square, by its middle.
            if out > 36 {
                assert_eq!(b.id, "far", "at {p}, {out} m out");
            } else if out < 24 {
                assert_eq!(b.id, "near", "at {p}, {out} m out");
            }
        }
    }
}
