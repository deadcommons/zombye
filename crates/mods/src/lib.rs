//! Zombye mods (issue #25, docs/modding.md): a folder of data and Luau scripts.
//!
//! ```text
//! mods/alarm_clock/
//!   mod.ron        id, name, version, description, scripts
//!   items.ron      more items (the format of assets/data/items/items.ron)       optional
//!   recipes.ron    more recipes (assets/data/items/recipes.ron's)               optional
//!   loot.ron       entries added to loot tables the game has                    optional
//!   *.luau         the scripts `mod.ron` lists, run by the simulation           optional
//! ```
//!
//! [`load_mods`] reads the folders and [`apply`] puts them into a [`WorldConfig`]: the data is
//! merged and checked as the game's own is, and the scripts become the config's
//! [`ScriptEngine`](zombye_sim::script::ScriptEngine), so every world made from it (a new game, a
//! loaded save, a new run) runs them at the end of each tick ([`luau`]). `sim` itself has no Luau
//! in it: it only knows the hook.

pub mod luau;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use zombye_sim::WorldConfig;
use zombye_sim::crafting::{RecipeDef, check_recipes, parse_recipes};
use zombye_sim::items::{ItemDef, complete_items, validate_items};
use zombye_sim::loot::LootEntry;

pub use luau::{LuauEngine, ModScripts};

/// `mod.ron`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Lowercase letters, digits and `_`: names the mod in saves, logs and its store.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Script files in the mod's folder, run in this order in one VM.
    #[serde(default)]
    pub scripts: Vec<String>,
}

/// Entries a mod adds to one of the game's loot tables (`loot.ron`: a list of these).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LootAdd {
    /// The table's key, as in `assets/data/loot.ron`: a room type or `any`, a container kind or `any`.
    pub room: String,
    pub container: String,
    pub entries: Vec<LootEntry>,
}

/// A mod as read: its manifest and the text of its files.
#[derive(Clone, Debug, PartialEq)]
pub struct Mod {
    pub manifest: Manifest,
    pub items: Option<String>,
    pub recipes: Option<String>,
    pub loot: Option<String>,
    /// Script file names and sources, in the manifest's order.
    pub scripts: Vec<(String, String)>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 48 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl Mod {
    /// A mod with nothing in it yet (tests, and mods made in memory).
    pub fn new(id: &str) -> Self {
        Self { manifest: Manifest { id: id.to_string(), name: id.to_string(), version: String::new(), description: String::new(), scripts: Vec::new() }, items: None, recipes: None, loot: None, scripts: Vec::new() }
    }

    pub fn with_items(mut self, ron: &str) -> Self {
        self.items = Some(ron.to_string());
        self
    }

    pub fn with_recipes(mut self, ron: &str) -> Self {
        self.recipes = Some(ron.to_string());
        self
    }

    pub fn with_loot(mut self, ron: &str) -> Self {
        self.loot = Some(ron.to_string());
        self
    }

    pub fn with_script(mut self, name: &str, source: &str) -> Self {
        self.manifest.scripts.push(name.to_string());
        self.scripts.push((name.to_string(), source.to_string()));
        self
    }

    /// Read a mod's folder. Errors name the file.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let read = |name: &str| -> Result<String, String> {
            let path = dir.join(name);
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
        };
        let optional = |name: &str| -> Result<Option<String>, String> { if dir.join(name).exists() { read(name).map(Some) } else { Ok(None) } };
        let manifest: Manifest = ron::from_str(&read("mod.ron")?).map_err(|e| format!("{}: {e}", dir.join("mod.ron").display()))?;
        if !valid_id(&manifest.id) {
            return Err(format!("{}: mod id {:?} must be lowercase letters, digits and _ (at most 48)", dir.join("mod.ron").display(), manifest.id));
        }
        let mut scripts = Vec::new();
        for name in &manifest.scripts {
            // A plain file name in the mod's own folder: a mod can't read anything else.
            if name.contains(['/', '\\']) || name.starts_with('.') || !name.ends_with(".luau") {
                return Err(format!("{}: script {name:?} must be a .luau file in the mod's folder", dir.join("mod.ron").display()));
            }
            scripts.push((name.clone(), read(name)?));
        }
        Ok(Self { manifest, items: optional("items.ron")?, recipes: optional("recipes.ron")?, loot: optional("loot.ron")?, scripts })
    }
}

/// Read each folder as a mod, in order.
pub fn load_mods(dirs: &[PathBuf]) -> Result<Vec<Mod>, String> {
    dirs.iter().map(|d| Mod::load(d)).collect()
}

/// Put `mods` into `config`, in order: their items, recipes and loot entries join the game's
/// (checked as the game's own are; an item with an id the game has replaces it, a recipe with
/// one is an error), and their
/// scripts become the config's script engine (each loaded once here, so a script that doesn't
/// compile is an error now, not in the middle of a game). With no mods the config is untouched.
pub fn apply(config: &mut WorldConfig, mods: &[Mod]) -> Result<(), String> {
    let mut scripts = Vec::new();
    for (k, m) in mods.iter().enumerate() {
        let id = &m.manifest.id;
        let what = |file: &str, why: String| format!("mod {id}: {file}: {why}");
        if !valid_id(id) {
            return Err(format!("mod id {id:?} must be lowercase letters, digits and _ (at most 48)"));
        }
        if mods[..k].iter().any(|o| &o.manifest.id == id) {
            return Err(format!("mod {id} is listed twice"));
        }
        if let Some(text) = &m.items {
            let more: Vec<ItemDef> = ron::from_str(text).map_err(|e| what("items.ron", e.to_string()))?;
            for def in more {
                // An id the game (or an earlier mod) has: this one takes its place, so a mod can
                // give an existing thing a tag, another weight, a use.
                match config.items.iter_mut().find(|d| d.id == def.id) {
                    Some(old) => *old = def,
                    None => config.items.push(def),
                }
            }
            validate_items(&config.items, &config.weapons).map_err(|why| what("items.ron", why))?;
        }
        // Ids as the world will know them: with the plain items weapons without one get.
        let mut all = config.items.clone();
        complete_items(&mut all, &config.weapons);
        if let Some(text) = &m.recipes {
            let more: Vec<RecipeDef> = parse_recipes(text).map_err(|why| what("recipes.ron", why))?;
            if let Some(r) = more.iter().find(|r| config.recipes.iter().any(|o| o.id == r.id)) {
                return Err(what("recipes.ron", format!("recipe {} is already defined", r.id)));
            }
            check_recipes(&more, &all).map_err(|why| what("recipes.ron", why))?;
            config.recipes.extend(more);
        }
        if let Some(text) = &m.loot {
            let adds: Vec<LootAdd> = ron::from_str(text).map_err(|e| what("loot.ron", e.to_string()))?;
            for add in adds {
                let Some(table) = config.loot.tables.iter_mut().find(|t| t.room == add.room && t.container == add.container) else {
                    return Err(what("loot.ron", format!("the game has no loot table ({}, {})", add.room, add.container)));
                };
                table.entries.extend(add.entries);
            }
            config.loot.validate().map_err(|why| what("loot.ron", why))?;
            config.loot.check(&all, &config.containers).map_err(|why| what("loot.ron", why))?;
        }
        if !m.scripts.is_empty() {
            scripts.push(ModScripts { id: id.clone(), sources: m.scripts.clone() });
        }
    }
    if !scripts.is_empty() {
        config.scripts = Some(Arc::new(LuauEngine::new(scripts)?));
    }
    Ok(())
}

/// [`load_mods`] then [`apply`]: what `--mod DIR` does. Returns the mods, for saying what loaded.
pub fn load_into(config: &mut WorldConfig, dirs: &[PathBuf]) -> Result<Vec<Mod>, String> {
    let mods = load_mods(dirs)?;
    apply(config, &mods)?;
    Ok(mods)
}

/// One line saying what loaded, for a server's or the game's start-up output.
pub fn describe(mods: &[Mod]) -> String {
    let list: Vec<String> = mods
        .iter()
        .map(|m| {
            let v = if m.manifest.version.is_empty() { String::new() } else { format!(" {}", m.manifest.version) };
            format!("{}{v} ({} script{})", m.manifest.id, m.scripts.len(), if m.scripts.len() == 1 { "" } else { "s" })
        })
        .collect();
    format!("mods: {}", list.join(", "))
}
