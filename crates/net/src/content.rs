//! Loading game content (data packs) from disk. The sim itself never does I/O; server and client
//! load content here and hand it to the sim.

use std::path::{Path, PathBuf};

use zombye_sim::BuildingTemplate;
use zombye_sim::containers::{ContainerData, parse_containers};
use zombye_sim::items::{ItemDef, WeaponDef, parse_items, parse_weapons};
use zombye_sim::population::{PopulationDef, parse_population};

/// The repo's `assets/data` folder (used when running from a checkout).
pub fn default_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data")
}

/// Every `*.ron` building template in `<data>/buildings`, sorted by file name so load order (and
/// therefore generation) is stable.
pub fn load_building_templates(data_dir: &Path) -> Result<Vec<BuildingTemplate>, String> {
    let dir = data_dir.join("buildings");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "ron"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|p| {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            BuildingTemplate::from_ron(&text).map_err(|e| format!("{}: {e}", p.display()))
        })
        .collect()
}

/// The population settings in `<data>/population.ron` (zombie density per zone, respawn,
/// migration toward noise), validated. A data folder without the file gets the built-in defaults.
pub fn load_population(data_dir: &Path) -> Result<PopulationDef, String> {
    let path = data_dir.join("population.ron");
    if !path.exists() {
        return Ok(PopulationDef::default());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_population(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The melee weapons in `<data>/items/weapons.ron`, validated.
pub fn load_weapons(data_dir: &Path) -> Result<Vec<WeaponDef>, String> {
    let path = data_dir.join("items/weapons.ron");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_weapons(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The items in `<data>/items/items.ron`, validated against `weapons` (each `weapon` must name one).
/// A data folder without the file has no items but the plain ones the world makes for weapons.
pub fn load_items(data_dir: &Path, weapons: &[WeaponDef]) -> Result<Vec<ItemDef>, String> {
    let path = data_dir.join("items/items.ron");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_items(&text, weapons).map_err(|e| format!("{}: {e}", path.display()))
}

/// Which furniture holds things: `<data>/containers.ron`, validated. A data folder without the file
/// has no containers.
pub fn load_containers(data_dir: &Path) -> Result<ContainerData, String> {
    let path = data_dir.join("containers.ron");
    if !path.exists() {
        return Ok(ContainerData::default());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_containers(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Everything a world is made from in `<data>`: building templates, weapons, items, containers and
/// the population file, as a config for `seed` (the zombie count as the population file gives).
pub fn load_config(data_dir: &Path, seed: u64) -> Result<zombye_sim::WorldConfig, String> {
    let templates = load_building_templates(data_dir)?;
    let weapons = load_weapons(data_dir)?;
    let items = load_items(data_dir, &weapons)?;
    let containers = load_containers(data_dir)?;
    let population = load_population(data_dir)?;
    let mut config = zombye_sim::WorldConfig::new(seed, 0, templates).with_weapons(weapons).with_items(items).with_containers(containers).with_population(population);
    config.zombies = None;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn population_loads_and_falls_back_to_defaults() {
        let def = load_population(&default_data_dir()).unwrap();
        assert!(def.density.town_core > def.density.deep_forest);
        let none = load_population(Path::new("/nonexistent-zombye-data")).unwrap();
        assert_eq!(none, PopulationDef::default());
    }

    #[test]
    fn items_and_containers_load() {
        let c = load_config(&default_data_dir(), 1).unwrap();
        assert!(c.items.len() >= 50 && !c.containers.containers.is_empty());
        assert!(load_items(Path::new("/nonexistent-zombye-data"), &c.weapons).unwrap().is_empty());
        assert_eq!(load_containers(Path::new("/nonexistent-zombye-data")).unwrap(), ContainerData::default());
    }
}
