//! Loading game content (data packs) from disk. The sim itself never does I/O; server and client
//! load content here and hand it to the sim.

use std::path::{Path, PathBuf};

use zombye_sim::BuildingTemplate;
use zombye_sim::containers::{ContainerData, parse_containers};
use zombye_sim::crafting::{RecipeDef, parse_recipes_for};
use zombye_sim::items::{ItemDef, WeaponDef, parse_items, parse_weapons};
use zombye_sim::loot::LootData;
use zombye_sim::population::{PopulationDef, parse_population};
use zombye_sim::status::{StatusData, parse_status};
use zombye_sim::street_props::{StreetPropsDef, parse_street_props};

/// The game's `assets/data` folder: the checkout's when running from one, else a packaged
/// game's beside its executable (`crate::install::assets_dir`).
pub fn default_data_dir() -> PathBuf {
    crate::install::assets_dir().join("data")
}

/// The data folder of a server that may be running away from a checkout (an installed binary, a
/// Docker image; #20): `$ZOMBYE_DATA` if set, else the checkout's folder if it is there, else
/// `data` (or `assets/data`) beside the executable. With none of them, the checkout's, so the
/// error names a path.
pub fn installed_data_dir() -> PathBuf {
    let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    data_dir_from(std::env::var_os("ZOMBYE_DATA").map(PathBuf::from), default_data_dir(), beside.as_deref())
}

fn data_dir_from(env: Option<PathBuf>, checkout: PathBuf, beside_exe: Option<&Path>) -> PathBuf {
    if let Some(dir) = env.filter(|d| !d.as_os_str().is_empty()) {
        return dir;
    }
    if checkout.is_dir() {
        return checkout;
    }
    beside_exe.into_iter().flat_map(|dir| [dir.join("data"), dir.join("assets").join("data")]).find(|d| d.is_dir()).unwrap_or(checkout)
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

/// The grid and the lights in `<data>/power.ron` (`zombye_sim::power`); without the file, the
/// defaults (the grid on, about one room in five lit).
pub fn load_power(data_dir: &Path) -> Result<zombye_sim::PowerDef, String> {
    let path = data_dir.join("power.ron");
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(zombye_sim::PowerDef::default()) };
    zombye_sim::power::parse_power(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Fire and cooking in `<data>/fire.ron` (`zombye_sim::fire`); without the file, the defaults.
pub fn load_fire(data_dir: &Path) -> Result<zombye_sim::FireTuning, String> {
    let path = data_dir.join("fire.ron");
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(zombye_sim::FireTuning::default()) };
    zombye_sim::fire::parse_fire(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The calendar, climate and weather in `<data>/weather.ron` (`zombye_sim::weather`); without the
/// file, the defaults.
pub fn load_weather(data_dir: &Path) -> Result<zombye_sim::WeatherTuning, String> {
    let path = data_dir.join("weather.ron");
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(zombye_sim::WeatherTuning::default()) };
    zombye_sim::weather::parse_weather(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// `--biomes FILE` and `--biome ID`, as the program was started ([`set_biome_flags`]).
static BIOME_FLAGS: std::sync::OnceLock<(Option<PathBuf>, Option<String>)> = std::sync::OnceLock::new();

/// Tell [`load_biomes`] what the command line asked for: a biomes file to read in place of the
/// data folder's (`--biomes FILE`) and one biome to grow everywhere (`--biome ID`). Once, before
/// any content is loaded. Without it (or with `None`s) the environment's `ZOMBYE_BIOMES` and
/// `ZOMBYE_BIOME` say the same, so a program started by this one (the MCP `screenshot` tool's
/// client) can be told too: [`biome_args`].
pub fn set_biome_flags(file: Option<PathBuf>, everywhere: Option<String>) {
    let _ = BIOME_FLAGS.set((file, everywhere));
}

/// The biomes file and the one biome asked for: the flags', else the environment's.
pub fn biome_flags() -> (Option<PathBuf>, Option<String>) {
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let (file, everywhere) = BIOME_FLAGS.get().cloned().unwrap_or_default();
    (file.or_else(|| env("ZOMBYE_BIOMES").map(PathBuf::from)), everywhere.or_else(|| env("ZOMBYE_BIOME")))
}

/// [`biome_flags`] as arguments for another run of the game, so it grows the same world.
pub fn biome_args() -> Vec<String> {
    let (file, everywhere) = biome_flags();
    let mut out = Vec::new();
    if let Some(f) = file {
        out.extend(["--biomes".to_string(), f.to_string_lossy().into_owned()]);
    }
    if let Some(id) = everywhere {
        out.extend(["--biome".to_string(), id]);
    }
    out
}

/// The biomes file in use: `--biomes FILE` (or `ZOMBYE_BIOMES`), else `<data>/biomes.ron`.
pub fn biomes_path(data_dir: &Path) -> PathBuf {
    biome_flags().0.unwrap_or_else(|| data_dir.join("biomes.ron"))
}

/// What grows on the wild ground ([`zombye_sim::biomes`]): the file at [`biomes_path`], validated,
/// with `--biome ID` (or `ZOMBYE_BIOME`) grown everywhere if asked. A data folder without the file
/// has the shipped biomes; a file named by `--biomes` must be there.
pub fn load_biomes(data_dir: &Path) -> Result<zombye_sim::biomes::BiomeData, String> {
    let (file, everywhere) = biome_flags();
    let path = biomes_path(data_dir);
    let data = if file.is_none() && !path.exists() {
        zombye_sim::biomes::BiomeData::default()
    } else {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        zombye_sim::biomes::parse_biomes(&text).map_err(|e| format!("{}: {e}", path.display()))?
    };
    match everywhere {
        Some(id) => data.with_everywhere(&id).map_err(|e| format!("--biome {id}: {e}")),
        None => Ok(data),
    }
}

/// World events in `<data>/events.ron` (`zombye_sim::events`): the water and power going off and
/// the like. Without the file, none.
pub fn load_events(data_dir: &Path) -> Result<zombye_sim::EventsDef, String> {
    let path = data_dir.join("events.ron");
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(zombye_sim::EventsDef::default()) };
    zombye_sim::events::parse_events(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The street props in `<data>/street_props.ron` (#61: mailboxes, hydrants, bins and where the town
/// generator puts them), validated. A data folder without the file has none.
pub fn load_street_props(data_dir: &Path) -> Result<StreetPropsDef, String> {
    let path = data_dir.join("street_props.ron");
    if !path.exists() {
        return Ok(StreetPropsDef::default());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_street_props(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The status icons in `<data>/status.ron` (Linear DCM-57: which there are, their levels, labels
/// and tips), validated. A data folder without the file gets the shipped ones. Only what a client
/// shows: the world never reads it.
pub fn load_status(data_dir: &Path) -> Result<StatusData, String> {
    let path = data_dir.join("status.ron");
    if !path.exists() {
        return Ok(StatusData::builtin().clone());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_status(&text).map_err(|e| format!("{}: {e}", path.display()))
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
    let mut all = items.to_vec();
    zombye_sim::items::complete_items(&mut all, weapons);
    let mut recipes = if path.exists() {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        parse_recipes_for(&text, &all).map_err(|e| format!("{}: {e}", path.display()))?
    } else {
        Vec::new()
    };
    // Filling a bottle (or anything with `fills`) at a tap is a recipe of its own.
    zombye_sim::crafting::add_fill_recipes(&mut recipes, &all);
    Ok(recipes)
}

/// What can be built (the build menu, `zombye_sim::build`): `<data>/build.ron`, validated, every
/// item id and tool tag checked against `items` (and the plain items weapons without one get). A
/// data folder without the file has no builds.
pub fn load_builds(data_dir: &Path, items: &[ItemDef], weapons: &[WeaponDef]) -> Result<Vec<zombye_sim::BuildDef>, String> {
    let path = data_dir.join("build.ron");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut all = items.to_vec();
    zombye_sim::items::complete_items(&mut all, weapons);
    zombye_sim::build::parse_builds_for(&text, &all).map_err(|e| format!("{}: {e}", path.display()))
}

/// What a new survivor on a server starts with, from `<data>/start_kit.ron` (#68), checked against
/// the items. A data folder without the file gives nothing.
pub fn load_start_kit(data_dir: &Path, items: &[ItemDef]) -> Result<zombye_sim::logout::StartKit, String> {
    let path = data_dir.join("start_kit.ron");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    zombye_sim::logout::parse_start_kit(&text, items).map_err(|e| format!("{}: {e}", path.display()))
}

/// Backgrounds, Strengths and Flaws and the starting wardrobe (`<data>/backgrounds.ron` and
/// `<data>/strengths_and_flaws.ron`, [`zombye_sim::origin`]), checked against `items`. A data
/// folder with neither has none (every survivor is a plain one); with one of the two, the other
/// is empty.
pub fn load_origins(data_dir: &Path, items: &[ItemDef]) -> Result<zombye_sim::origin::OriginData, String> {
    use zombye_sim::origin;
    let (b, p) = (data_dir.join("backgrounds.ron"), data_dir.join("strengths_and_flaws.ron"));
    let read = |path: &Path| std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()));
    let backgrounds = if b.exists() { origin::parse_backgrounds(&read(&b)?).map_err(|e| format!("{}: {e}", b.display()))? } else { Vec::new() };
    let picks = if p.exists() { origin::parse_picks(&read(&p)?).map_err(|e| format!("{}: {e}", p.display()))? } else { Default::default() };
    origin::OriginData::new(backgrounds, picks, items).map_err(|e| format!("{} / {}: {e}", b.display(), p.display()))
}

/// The difficulty presets (`<data>/difficulty.ron`, [`zombye_sim::difficulty`]). A data folder
/// without the file has the one built in, which changes nothing.
pub fn load_difficulty(data_dir: &Path) -> Result<zombye_sim::difficulty::DifficultyData, String> {
    let path = data_dir.join("difficulty.ron");
    if !path.exists() {
        return Ok(zombye_sim::difficulty::DifficultyData::builtin());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    zombye_sim::difficulty::parse_difficulty(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The benchmark scenarios in `<data>/scenarios.ron` (#29), validated, the items they name checked
/// against `items` (and the plain items weapons without one get). A data folder without the file
/// has none.
pub fn load_scenarios(data_dir: &Path, items: &[ItemDef], weapons: &[WeaponDef]) -> Result<Vec<zombye_sim::scenario::ScenarioDef>, String> {
    let path = data_dir.join("scenarios.ron");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let defs = zombye_sim::scenario::parse_scenarios(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut all = items.to_vec();
    zombye_sim::items::complete_items(&mut all, weapons);
    zombye_sim::scenario::check_scenarios(&defs, &all).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(defs)
}

/// Everything a world is made from in `<data>`: building templates, weapons, items, containers,
/// loot tables, recipes, the population file, the street props and the power, as a config for `seed` (the zombie count as the
/// population file gives).
/// `vehicles.ron` (`zombye_sim::vehicles`): none if the file isn't there. Every kind must be parked
/// as a street prop kind `street_props` has.
pub fn load_vehicles(data_dir: &Path, street_props: &zombye_sim::street_props::StreetPropsDef, items: &[zombye_sim::ItemDef]) -> Result<zombye_sim::VehicleData, String> {
    let path = data_dir.join("vehicles.ron");
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(zombye_sim::VehicleData::default()) };
    let data = zombye_sim::parse_vehicles(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    // The fuel can, if there are items at all, must be one that holds fuel.
    let t = &data.tuning;
    if !data.kinds.is_empty() && !items.is_empty() && !items.iter().any(|i| i.id == t.fuel_item && i.fuel_l.is_some()) {
        return Err(format!("{}: tuning.fuel_item {:?} isn't an item with fuel_l in items.ron", path.display(), t.fuel_item));
    }
    // Keys, hotwiring and repairs name items that must be there (stage 4).
    if !data.kinds.is_empty() && !items.is_empty() {
        data.work.check_items(items).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    for k in &data.kinds {
        if !street_props.kinds.iter().any(|p| p.id == k.prop) {
            return Err(format!("{}: vehicle {:?} is parked as {:?}, which street_props.ron doesn't have", path.display(), k.id, k.prop));
        }
    }
    Ok(data)
}

pub fn load_config(data_dir: &Path, seed: u64) -> Result<zombye_sim::WorldConfig, String> {
    let templates = load_building_templates(data_dir)?;
    let weapons = load_weapons(data_dir)?;
    let items = load_items(data_dir, &weapons)?;
    let containers = load_containers(data_dir)?;
    let loot = load_loot(data_dir, &items, &weapons, &containers)?;
    let recipes = load_recipes(data_dir, &items, &weapons)?;
    let builds = load_builds(data_dir, &items, &weapons)?;
    let population = load_population(data_dir)?;
    // What zombies wear must be clothing there is.
    zombye_sim::corpses::check_outfits(&population.outfits, &items).map_err(|e| format!("{}: {e}", data_dir.join("population.ron").display()))?;
    let street_props = load_street_props(data_dir)?;
    let biomes = load_biomes(data_dir)?;
    let power = load_power(data_dir)?;
    let fire = load_fire(data_dir)?;
    let vehicles = load_vehicles(data_dir, &street_props, &items)?;
    let weather = load_weather(data_dir)?;
    let events = load_events(data_dir)?;
    let mut config = zombye_sim::WorldConfig::new(seed, 0, templates)
        .with_weapons(weapons)
        .with_items(items)
        .with_containers(containers)
        .with_loot(loot)
        .with_recipes(recipes)
        .with_builds(builds)
        .with_population(population)
        .with_street_props(street_props)
        .with_biomes(biomes)
        .with_power(power)
        .with_fire(fire)
        .with_vehicles(vehicles)
        .with_weather(weather)
        .with_events(events);
    config.zombies = None;
    Ok(config)
}

/// A seed for a new game's loot ([`zombye_sim::WorldConfig::loot_seed`]): different every
/// call, from the process's random hasher keys and the clock. Kept under 2^48 so it reads back
/// from the settings' RON exactly.
pub fn fresh_loot_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.finish() & ((1 << 48) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where an installed server finds its data (#20): the environment's first, the checkout's
    /// while there is one, then beside the binary.
    #[test]
    fn an_installed_server_finds_its_data() {
        let scratch = std::env::temp_dir().join(format!("zombye-data-dir-test-{}", std::process::id()));
        let (bin, gone) = (scratch.join("bin"), scratch.join("no-checkout"));
        std::fs::create_dir_all(bin.join("data")).unwrap();
        assert_eq!(data_dir_from(Some("/opt/data".into()), default_data_dir(), Some(&bin)), PathBuf::from("/opt/data"));
        assert_eq!(data_dir_from(Some("".into()), default_data_dir(), Some(&bin)), default_data_dir());
        assert_eq!(data_dir_from(None, gone.clone(), Some(&bin)), bin.join("data"));
        std::fs::remove_dir_all(bin.join("data")).unwrap();
        std::fs::create_dir_all(bin.join("assets/data")).unwrap();
        assert_eq!(data_dir_from(None, gone.clone(), Some(&bin)), bin.join("assets/data"));
        std::fs::remove_dir_all(&scratch).unwrap();
        assert_eq!(data_dir_from(None, gone.clone(), Some(&bin)), gone, "nothing anywhere: the path the error will name");
    }

    #[test]
    fn population_loads_and_falls_back_to_defaults() {
        let def = load_population(&default_data_dir()).unwrap();
        assert!(def.density.town_core > def.density.deep_forest);
        let none = load_population(Path::new("/nonexistent-zombye-data")).unwrap();
        assert_eq!(none, PopulationDef::default());
    }

    /// The street props load (#61), a mistake names the file and the kind or rule, and with no file
    /// there are none.
    #[test]
    fn status_icons_load_from_the_data_folder() {
        // The shipped file is the built-in one; a folder without it gets that too.
        assert_eq!(&load_status(&default_data_dir()).unwrap(), StatusData::builtin());
        assert_eq!(&load_status(Path::new("/nonexistent-zombye-data")).unwrap(), StatusData::builtin());
        let dir = std::env::temp_dir().join(format!("zombye-status-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("status.ron"), r#"(icons: [(kind: Thirst, levels: [(at: 0.5, label: "Dry"), (at: 0.4, label: "Drier")])])"#).unwrap();
        let err = load_status(&dir).unwrap_err();
        std::fs::remove_dir_all(&dir).ok();
        assert!(err.contains("status.ron") && err.contains("Thirst") && err.contains("higher than the last"), "{err}");
    }

    #[test]
    fn street_props_load_and_name_their_mistakes() {
        let def = load_street_props(&default_data_dir()).unwrap();
        for id in ["mailbox", "fire_hydrant", "trash_can"] {
            assert!(def.kinds.iter().any(|k| k.id == id), "no {id}");
        }
        assert!(!def.rules.is_empty());
        assert_eq!(load_street_props(Path::new("/nonexistent-zombye-data")).unwrap(), StreetPropsDef::default());
        let dir = std::env::temp_dir().join(format!("zombye-props-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |text: &str| {
            std::fs::write(dir.join("street_props.ron"), text).unwrap();
            load_street_props(&dir).unwrap_err()
        };
        let kind = |extra: &str| format!("(id: \"bin\", size: (0.5, 0.5, 1.0), color: (0.5, 0.5, 0.5){extra})");
        let errs = [
            (write(&format!("(kinds: [{}], rules: [(kind: \"bench\", place: Kerb, spacing: Some((40.0, 60.0)))])", kind(""))), "no kind \"bench\""),
            (write(&format!("(kinds: [{}])", kind(", radius: Some(0.8)"))), "radius must be"),
            (write(&format!("(kinds: [{}, {}])", kind(""), kind(""))), "used twice"),
            (write(&format!("(kinds: [{}], rules: [(kind: \"bin\", place: Kerb)])", kind(""))), "Kerb needs spacing"),
            (write(&format!("(kinds: [{}], rules: [(kind: \"bin\", place: GardenPath, chance: 1.5)])", kind(""))), "chance must be"),
            (write(&format!("(kinds: [{}])", kind(", wobble: 2"))), "wobble"),
        ];
        let _ = std::fs::remove_dir_all(&dir);
        for (err, want) in errs {
            assert!(err.contains("street_props.ron") && err.contains(want), "{err} should mention {want}");
        }
    }

    /// The shipped scenarios load (#29), each starts its world, and a mistake names the file and
    /// the scenario; with no file there are none.
    #[test]
    fn scenarios_load_and_name_their_mistakes() {
        let c = load_config(&default_data_dir(), 1).unwrap();
        let defs = load_scenarios(&default_data_dir(), &c.items, &c.weapons).unwrap();
        for id in ["first_night", "scavenger", "last_stand", "siege"] {
            assert!(defs.iter().any(|d| d.id == id), "no {id}");
        }
        assert!(load_scenarios(Path::new("/nonexistent-zombye-data"), &c.items, &c.weapons).unwrap().is_empty());
        let dir = std::env::temp_dir().join(format!("zombye-scenarios-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("scenarios.ron"), "[(id: \"x\", name: \"X\", brief: \"Live.\", seed: 1, hours: 1.0, hold: Some(\"unobtainium\"))]").unwrap();
        let err = load_scenarios(&dir, &c.items, &c.weapons).unwrap_err();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(err.contains("scenarios.ron") && err.contains("\"x\"") && err.contains("unobtainium"), "{err}");
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
        let none = load_recipes(Path::new("/nonexistent-zombye-data"), &c.items, &c.weapons).unwrap();
        assert!(!none.is_empty() && none.iter().all(|r| (r.water || r.cistern) && r.id.starts_with("fill_")), "no file: only the made-up fill recipes");
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
