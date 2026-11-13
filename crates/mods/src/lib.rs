//! Zombye mods (issue #25, docs/modding.md): a folder of data and Luau scripts.
//!
//! ```text
//! mods/alarm_clock/
//!   mod.ron        id, name, version, description, scripts
//!   items.ron      more items (the format of assets/data/items/items.ron)       optional
//!   recipes.ron    more recipes (assets/data/items/recipes.ron's)               optional
//!   loot.ron       entries added to loot tables the game has                    optional
//!   buildings/     building templates (assets/data/buildings/*.ron's format)    optional
//!   *.luau         the scripts `mod.ron` lists, run by the simulation           optional
//! ```
//!
//! A server hands the **data** of its mods to the clients that join it (#24): [`Mod::pack`] is
//! what goes over the network (everything but the scripts, which run on the server alone) and
//! [`Mod::from_pack`] what a client makes of it.
//!
//! [`load_mods`] reads the folders and [`apply`] puts them into a [`WorldConfig`]: the data is
//! merged and checked as the game's own is, and the scripts become the config's
//! [`ScriptEngine`](zombye_sim::script::ScriptEngine), so every world made from it (a new game, a
//! loaded save, a new run) runs them at the end of each tick ([`luau`]). `sim` itself has no Luau
//! in it: it only knows the hook.

pub mod folder;
pub mod luau;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use zombye_net::modpack::ModPack;
use zombye_sim::WorldConfig;
use zombye_sim::template::BuildingTemplate;
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
    /// Building templates (#24): each file in `buildings/` by its name (`shed.ron`) and text,
    /// sorted by name.
    pub buildings: Vec<(String, String)>,
    /// Script file names and sources, in the manifest's order.
    pub scripts: Vec<(String, String)>,
}

use zombye_net::modpack::valid_id;

/// The folder of a mod's building templates, and of their names in a pack.
const BUILDINGS: &str = "buildings";

impl Mod {
    /// A mod with nothing in it yet (tests, and mods made in memory).
    pub fn new(id: &str) -> Self {
        Self { manifest: Manifest { id: id.to_string(), name: id.to_string(), version: String::new(), description: String::new(), scripts: Vec::new() }, items: None, recipes: None, loot: None, buildings: Vec::new(), scripts: Vec::new() }
    }

    /// With a building template, as a file `name` (`shed.ron`) in the mod's `buildings/`.
    pub fn with_building(mut self, name: &str, ron: &str) -> Self {
        self.buildings.push((name.to_string(), ron.to_string()));
        self.buildings.sort();
        self
    }

    /// The mod's data as a server hands it to clients (#24): its name and its data files,
    /// **without its scripts** (they run on the server; a client never gets or runs them). The
    /// pack's hash names exactly this.
    pub fn pack(&self) -> ModPack {
        let m = &self.manifest;
        let mut files: Vec<(String, String)> = [("items.ron", &self.items), ("recipes.ron", &self.recipes), ("loot.ron", &self.loot)].into_iter().filter_map(|(name, text)| text.clone().map(|t| (name.to_string(), t))).collect();
        files.extend(self.buildings.iter().map(|(name, text)| (format!("{BUILDINGS}/{name}"), text.clone())));
        ModPack { id: m.id.clone(), name: m.name.clone(), version: m.version.clone(), description: m.description.clone(), files }
    }

    /// The mod a pack holds: its data, and no scripts. The pack is checked first
    /// ([`ModPack::check`]: a proper id, data files only); what the files say is checked by
    /// [`apply`], as a local mod's is.
    pub fn from_pack(pack: &ModPack) -> Result<Self, String> {
        pack.check()?;
        let mut m = Self::new(&pack.id);
        (m.manifest.name, m.manifest.version, m.manifest.description) = (pack.name.clone(), pack.version.clone(), pack.description.clone());
        for (name, text) in &pack.files {
            match name.as_str() {
                "items.ron" => m.items = Some(text.clone()),
                "recipes.ron" => m.recipes = Some(text.clone()),
                "loot.ron" => m.loot = Some(text.clone()),
                other => match other.strip_prefix(BUILDINGS).and_then(|n| n.strip_prefix('/')) {
                    Some(file) => m.buildings.push((file.to_string(), text.clone())),
                    None => return Err(format!("mod {}: {name:?} is not a data file a mod may bring", pack.id)),
                },
            }
        }
        m.buildings.sort();
        Ok(m)
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

    /// What the mod brings, in words: "2 items, 1 recipe, 1 loot table change, 1 building,
    /// 1 script" ("nothing yet" for an empty one). Files that don't parse count none.
    pub fn contents(&self) -> String {
        let items = self.items.as_deref().and_then(|t| ron::from_str::<Vec<ItemDef>>(t).ok()).map_or(0, |v| v.len());
        let recipes = self.recipes.as_deref().and_then(|t| parse_recipes(t).ok()).map_or(0, |v| v.len());
        let loot = self.loot.as_deref().and_then(|t| ron::from_str::<Vec<LootAdd>>(t).ok()).map_or(0, |v| v.len());
        let plural = |n: usize, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
        let mut parts = Vec::new();
        if self.items.is_some() {
            parts.push(plural(items, "item"));
        }
        if self.recipes.is_some() {
            parts.push(plural(recipes, "recipe"));
        }
        if self.loot.is_some() {
            parts.push(plural(loot, "loot table change"));
        }
        if !self.buildings.is_empty() {
            parts.push(plural(self.buildings.len(), "building"));
        }
        if !self.scripts.is_empty() {
            parts.push(plural(self.scripts.len(), "script"));
        }
        if parts.is_empty() { "nothing yet".to_string() } else { parts.join(", ") }
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
        // Building templates: every `buildings/*.ron`, by name, so the order (and so the town
        // generated from them) is the same on every machine.
        let mut buildings = Vec::new();
        let folder = dir.join(BUILDINGS);
        if folder.is_dir() {
            let mut names: Vec<String> = std::fs::read_dir(&folder)
                .map_err(|e| format!("{}: {e}", folder.display()))?
                .filter_map(|e| e.ok().and_then(|e| e.file_name().into_string().ok()))
                .filter(|n| n.ends_with(".ron"))
                .collect();
            names.sort();
            for name in names {
                if !zombye_net::modpack::data_file(&format!("{BUILDINGS}/{name}")) {
                    return Err(format!("{}: a building's file name must be letters, digits, _ and - before .ron", folder.join(&name).display()));
                }
                let text = read(&format!("{BUILDINGS}/{name}"))?;
                buildings.push((name, text));
            }
        }
        Ok(Self { manifest, items: optional("items.ron")?, recipes: optional("recipes.ron")?, loot: optional("loot.ron")?, buildings, scripts })
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
        for (name, text) in &m.buildings {
            // A template id the game (or an earlier mod) has: this one takes its place; any other
            // is one more building the town generator may place. Either way the town changes.
            let template = BuildingTemplate::from_ron(text).map_err(|why| what(&format!("{BUILDINGS}/{name}"), why))?;
            match config.templates.iter_mut().find(|t| t.id == template.id) {
                Some(old) => *old = template,
                None => config.templates.push(template),
            }
        }
        if !m.scripts.is_empty() {
            scripts.push(ModScripts { id: id.clone(), sources: m.scripts.clone() });
        }
    }
    // A mod's container with `fills` can be filled at a tap, as the game's are.
    if !mods.is_empty() {
        let mut all = config.items.clone();
        complete_items(&mut all, &config.weapons);
        zombye_sim::crafting::add_fill_recipes(&mut config.recipes, &all);
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

/// Put the mods a server handed over (or named, and we had) into `config`, in the server's order:
/// what a client joining a modded server plays with (#24). Data only: `config` gets no scripts.
pub fn apply_packs(config: &mut WorldConfig, packs: &[ModPack]) -> Result<Vec<Mod>, String> {
    let mods = packs.iter().map(Mod::from_pack).collect::<Result<Vec<_>, _>>()?;
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
