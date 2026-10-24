//! Loading game content (data packs) from disk. The sim itself never does I/O; server and client
//! load content here and hand it to the sim.

use std::path::{Path, PathBuf};

use zombye_sim::BuildingTemplate;
use zombye_sim::containers::{ContainerData, parse_containers};
use zombye_sim::crafting::{RecipeDef, parse_recipes_for};
use zombye_sim::items::{ItemDef, WeaponDef, parse_items, parse_weapons};
use zombye_sim::loot::LootData;
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

/// What containers hold: `<data>/loot.ron`, validated, every item id checked against `items` (and
/// the plain items weapons without one get) and every container kind against `containers`. A data
/// folder without the file has no loot tables: every container holds nothing.
pub fn load_loot(data_dir: &Path, items: &[ItemDef], weapons: &[WeaponDef], containers: &ContainerData) -> Result<LootData, String> {
    let path = data_dir.join("loot.ron");
    if !path.exists() {
        return Ok(LootData::default());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut all = items.to_vec();
    zombye_sim::items::complete_items(&mut all, weapons);
    LootData::parse_for(&text, &all, containers).map_err(|e| format!("{}: {e}", path.display()))
}

/// What can be made from what: `<data>/items/recipes.ron` (#12), validated, every item id checked
/// against `items` (and the plain items weapons without one get) and every tool tag against the
/// items' tags. A data folder without the file has no recipes.
pub fn load_recipes(data_dir: &Path, items: &[ItemDef], weapons: &[WeaponDef]) -> Result<Vec<RecipeDef>, String> {
    let path = data_dir.join("items/recipes.ron");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut all = items.to_vec();
    zombye_sim::items::complete_items(&mut all, weapons);
    parse_recipes_for(&text, &all).map_err(|e| format!("{}: {e}", path.display()))
}

/// Everything a world is made from in `<data>`: building templates, weapons, items, containers,
/// loot tables, recipes and the population file, as a config for `seed` (the zombie count as the
/// population file gives).
pub fn load_config(data_dir: &Path, seed: u64) -> Result<zombye_sim::WorldConfig, String> {
    let templates = load_building_templates(data_dir)?;
    let weapons = load_weapons(data_dir)?;
    let items = load_items(data_dir, &weapons)?;
    let containers = load_containers(data_dir)?;
    let loot = load_loot(data_dir, &items, &weapons, &containers)?;
    let recipes = load_recipes(data_dir, &items, &weapons)?;
    let population = load_population(data_dir)?;
    let mut config = zombye_sim::WorldConfig::new(seed, 0, templates)
        .with_weapons(weapons)
        .with_items(items)
        .with_containers(containers)
        .with_loot(loot)
        .with_recipes(recipes)
        .with_population(population);
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

    /// The loot tables load against the items and containers, and a mistake in them names the file
    /// and the table.
    #[test]
    fn loot_loads_and_names_its_mistakes() {
        let c = load_config(&default_data_dir(), 1).unwrap();
        assert!(c.loot.tables.len() >= 20 && c.loot.find("kitchen", "fridge").is_some());
        assert_eq!(load_loot(Path::new("/nonexistent-zombye-data"), &c.items, &c.weapons, &c.containers).unwrap(), LootData::default());
        let dir = std::env::temp_dir().join(format!("zombye-loot-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("loot.ron"), "(tables: [(room: \"kitchen\", container: \"fridge\", entries: [(item: \"unobtainium\")])])").unwrap();
        let err = load_loot(&dir, &c.items, &c.weapons, &c.containers).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains("loot.ron") && err.contains("(kitchen, fridge)") && err.contains("unobtainium"), "{err}");
    }

    /// The recipes load against the items (#12), and a mistake in them names the file and the
    /// recipe; with no file there are none.
    #[test]
    fn recipes_load_and_name_their_mistakes() {
        let c = load_config(&default_data_dir(), 1).unwrap();
        assert!(c.recipes.len() >= 7 && c.recipes.iter().any(|r| r.id == "rip_t_shirt"));
        assert!(load_recipes(Path::new("/nonexistent-zombye-data"), &c.items, &c.weapons).unwrap().is_empty());
        let dir = std::env::temp_dir().join(format!("zombye-recipes-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("items")).unwrap();
        let write = |text: &str| {
            std::fs::write(dir.join("items/recipes.ron"), text).unwrap();
            load_recipes(&dir, &c.items, &c.weapons).unwrap_err()
        };
        let rec = |inputs: &str, secs: &str, outputs: &str| format!("(id: \"sling\", name: \"Tie a sling\", secs: {secs}, inputs: [{inputs}], outputs: [{outputs}])");
        let one = |inputs: &str, secs: &str, outputs: &str| format!("[{}]", rec(inputs, secs, outputs));
        let shirt = rec("(item: \"t_shirt\")", "3.0", "(item: \"rag\")");
        let errs = [
            (write(&one("(item: \"unobtainium\")", "3.0", "(item: \"rag\")")), "input \"unobtainium\" isn't in items.ron"),
            (write(&one("(item: \"t_shirt\", count: 0)", "3.0", "(item: \"rag\")")), "count 0"),
            (write(&one("(item: \"t_shirt\")", "0.0", "(item: \"rag\")")), "secs must be in (0"),
            (write(&one("(item: \"rag\")", "3.0", "(item: \"rag\", count: 2)")), "both an input and an output"),
            (write(&format!("[{shirt}, {shirt}]")), "used twice"),
        ];
        let _ = std::fs::remove_dir_all(&dir);
        for (err, want) in errs {
            assert!(err.contains("recipes.ron") && err.contains(want), "{err} should mention {want}");
        }
    }
}
