//! The mods folder: where players put mods, and which of them are on.
//!
//! ```text
//! mods/                 the game's folder of mods (zombye_net::install::mods_dir)
//!   enabled.ron         the mods that are on, in the order they load: (mods: ["alarm_clock"])
//!   alarm_clock/        a mod: a folder with a mod.ron (see the crate's docs)
//!   bigger_kitchens/
//! ```
//!
//! A mod is a folder dropped in here. [`scan`] finds every folder in it with a `mod.ron` and reads
//! `enabled.ron`; a mod that isn't listed there is off, so nothing runs that the player didn't turn
//! on. [`Folder::check`] tries the mods that are on, in order, against the game's data: one that
//! can't be read or doesn't fit (a broken file, an item another mod's recipe needs gone, a recipe
//! id taken) is left off with the reason on its [`Entry`], and the game starts without it. A mod
//! that is off is checked as if it were turned on last, so the Mods menu can say beforehand
//! whether it would load.
//!
//! The game's own `--mod DIR` folders come on top of these (for working on a mod), and a `--mod`
//! of the same id as a folder mod takes its place.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use zombye_sim::WorldConfig;

use crate::{Mod, apply};

/// The file in the mods folder that says which mods are on.
pub const ENABLED: &str = "enabled.ron";

/// `enabled.ron`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Enabled {
    /// Mod ids, in the order they load.
    #[serde(default)]
    mods: Vec<String>,
}

/// The mods that are on, in order, from the folder's `enabled.ron`. No file: none.
pub fn read_enabled(dir: &Path) -> Result<Vec<String>, String> {
    let path = dir.join(ENABLED);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let enabled: Enabled = ron::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(enabled.mods)
}

/// Write `enabled.ron` (the folder is made if it isn't there).
pub fn write_enabled(dir: &Path, ids: &[String]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(ENABLED);
    let list: Vec<String> = ids.iter().map(|id| format!("{id:?}")).collect();
    let text = format!(
        "// The mods that are on, in the order they load (later ones win where two change the same thing).\n// The game's Mods menu writes this file; a mod folder that isn't listed here is off.\n(mods: [{}])\n",
        list.join(", ")
    );
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// One mod in the folder.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Its folder.
    pub dir: PathBuf,
    /// Its id (`mod.ron`'s; the folder's name when `mod.ron` can't be read).
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    /// Listed in `enabled.ron`.
    pub enabled: bool,
    /// Why it doesn't load (or, off, wouldn't if it were turned on): it can't be read, another
    /// folder has its id, or its data doesn't fit the game's. `None`: it loads.
    pub problem: Option<String>,
    /// The mod as read, when it could be.
    pub loaded: Option<Mod>,
}

impl Entry {
    /// On, and loading.
    pub fn active(&self) -> bool {
        self.enabled && self.problem.is_none() && self.loaded.is_some()
    }

    /// What a mod has: "2 items, 1 recipe, 1 script" ([`Mod::contents`]).
    pub fn contents(&self) -> String {
        self.loaded.as_ref().map_or_else(String::new, Mod::contents)
    }
}

/// What is in the mods folder: every mod, those that are on first in their order, then the rest by
/// id; and what is wrong with the folder itself.
#[derive(Clone, Debug, Default)]
pub struct Folder {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    /// Not about one mod: `enabled.ron` unreadable, a mod listed there that isn't in the folder.
    pub problems: Vec<String>,
}

/// Read the mods folder: every folder in it with a `mod.ron`, and `enabled.ron`. Nothing is
/// checked against the game's data yet ([`Folder::check`]). A folder that isn't there has no mods.
pub fn scan(dir: &Path) -> Folder {
    let mut folder = Folder { dir: dir.to_path_buf(), ..Default::default() };
    let enabled = read_enabled(dir).unwrap_or_else(|e| {
        folder.problems.push(format!("{e}: no mods are on until it is fixed"));
        Vec::new()
    });
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.join("mod.ron").is_file()).collect()).unwrap_or_default();
    dirs.sort();
    let mut entries: Vec<Entry> = Vec::new();
    for d in dirs {
        let folder_name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut e = match Mod::load(&d) {
            Ok(m) => Entry { dir: d, id: m.manifest.id.clone(), name: m.manifest.name.clone(), version: m.manifest.version.clone(), description: m.manifest.description.clone(), enabled: false, problem: None, loaded: Some(m) },
            Err(why) => Entry { dir: d, id: folder_name.clone(), name: folder_name, version: String::new(), description: String::new(), enabled: false, problem: Some(why), loaded: None },
        };
        if let Some(first) = entries.iter().find(|o| o.id == e.id) {
            e.problem = Some(format!("another mod has the id {} ({}): ids must differ", e.id, first.dir.display()));
            e.loaded = None;
        }
        entries.push(e);
    }
    // The ones that are on, in enabled.ron's order; then the rest by id.
    let mut ordered = Vec::new();
    for id in &enabled {
        if ordered.iter().any(|e: &Entry| &e.id == id) {
            continue;
        }
        match entries.iter().position(|e| &e.id == id && (e.loaded.is_some() || e.problem.is_some())) {
            Some(k) => {
                let mut e = entries.remove(k);
                e.enabled = true;
                ordered.push(e);
            }
            None => folder.problems.push(format!("mod {id} is turned on but isn't in {}", dir.display())),
        }
    }
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    ordered.extend(entries);
    folder.entries = ordered;
    folder
}

impl Folder {
    /// Try the mods against `base` (the game's data, before any mod): those that are on in order,
    /// each with the ones before it that loaded; one that doesn't fit gets its reason and is left
    /// off. Then each that is off, as if turned on after them.
    pub fn check(&mut self, base: &WorldConfig) {
        let mut ok: Vec<Mod> = Vec::new();
        let fits = |ok: &[Mod], m: &Mod| -> Result<(), String> {
            let mut list = ok.to_vec();
            list.push(m.clone());
            apply(&mut base.clone(), &list)
        };
        for e in self.entries.iter_mut().filter(|e| e.enabled) {
            let Some(m) = e.loaded.as_ref().filter(|_| e.problem.is_none()) else { continue };
            match fits(&ok, m) {
                Ok(()) => ok.push(m.clone()),
                Err(why) => e.problem = Some(why),
            }
        }
        for e in self.entries.iter_mut().filter(|e| !e.enabled) {
            let Some(m) = e.loaded.as_ref().filter(|_| e.problem.is_none()) else { continue };
            if let Err(why) = fits(&ok, m) {
                e.problem = Some(why);
            }
        }
    }

    /// The mods that are on and load, in order.
    pub fn active(&self) -> Vec<Mod> {
        self.entries.iter().filter(|e| e.active()).filter_map(|e| e.loaded.clone()).collect()
    }

    /// The ids that are on, in order (what `enabled.ron` holds).
    pub fn enabled_ids(&self) -> Vec<String> {
        self.entries.iter().filter(|e| e.enabled).map(|e| e.id.clone()).collect()
    }

    /// Turn a mod on (it goes last) or off. False if there's no such mod.
    pub fn set_enabled(&mut self, id: &str, on: bool) -> bool {
        let Some(k) = self.entries.iter().position(|e| e.id == id) else { return false };
        let mut e = self.entries.remove(k);
        e.enabled = on;
        let at = self.entries.iter().filter(|e| e.enabled).count();
        if on {
            self.entries.insert(at, e);
        } else {
            let at = at + self.entries[at..].iter().take_while(|o| o.id < e.id).count();
            self.entries.insert(at, e);
        }
        true
    }

    /// Move a mod that is on one place earlier (`up`) or later in the load order. False if it
    /// can't go that way.
    pub fn shift(&mut self, id: &str, up: bool) -> bool {
        let on = self.entries.iter().filter(|e| e.enabled).count();
        let Some(k) = self.entries[..on].iter().position(|e| e.id == id) else { return false };
        let other = if up { k.checked_sub(1) } else { Some(k + 1).filter(|&j| j < on) };
        match other {
            Some(j) => {
                self.entries.swap(k, j);
                true
            }
            None => false,
        }
    }

    /// Write which mods are on to `enabled.ron`.
    pub fn save(&self) -> Result<(), String> {
        write_enabled(&self.dir, &self.enabled_ids())
    }

    /// Every problem in plain words: the folder's, then each mod that is on but doesn't load.
    pub fn report(&self) -> Vec<String> {
        let mut out = self.problems.clone();
        out.extend(self.entries.iter().filter(|e| e.enabled).filter_map(|e| e.problem.as_ref().map(|p| format!("mod {} is left off: {p}", e.id))));
        out
    }
}

/// What loading the mods folder gave: the mods put into the config, in order (the folder's, then
/// the `--mod` ones), and what was left off and why.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    pub mods: Vec<Mod>,
    pub problems: Vec<String>,
}

/// The game's mods: those on in the folder `dir` (if any), then the `extra` folders (`--mod`),
/// into `config`. A folder mod that doesn't load is left off and named in the problems; a `--mod`
/// folder that doesn't is an error, as it always was (it is what the person is working on).
pub fn load_folder_into(config: &mut WorldConfig, dir: Option<&Path>, extra: &[PathBuf]) -> Result<Loaded, String> {
    let extra = crate::load_mods(extra)?;
    let mut problems = Vec::new();
    let mut mods = Vec::new();
    if let Some(dir) = dir {
        let mut folder = scan(dir);
        // A `--mod` of the same id takes the folder's place.
        for e in folder.entries.iter_mut().filter(|e| e.enabled) {
            if extra.iter().any(|m| m.manifest.id == e.id) {
                e.enabled = false;
            }
        }
        folder.check(config);
        problems = folder.report();
        mods = folder.active();
    }
    mods.extend(extra);
    apply(config, &mods)?;
    Ok(Loaded { mods, problems })
}
