//! Loading game content (data packs) from disk. The sim itself never does I/O; server and client
//! load content here and hand it to the sim.

use std::path::{Path, PathBuf};

use zombye_sim::BuildingTemplate;
use zombye_sim::items::{WeaponDef, parse_weapons};

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

/// The melee weapons in `<data>/items/weapons.ron`, validated.
pub fn load_weapons(data_dir: &Path) -> Result<Vec<WeaponDef>, String> {
    let path = data_dir.join("items/weapons.ron");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_weapons(&text).map_err(|e| format!("{}: {e}", path.display()))
}
