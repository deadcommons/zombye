//! Crafting (#12): recipes as data (`assets/data/items/recipes.ron`, parsed with
//! [`parse_recipes`] and checked against the items with [`check_recipes`]).
//!
//! A [`RecipeDef`] has `inputs` (item ids and counts, used up), `tools` (an item, or any item with a
//! tag, needed but kept), the seconds it takes, and `outputs` (items made). Making one is an item job
//! (`JobKind::Craft`) like eating: checked when asked for ([`World::player_craft`]) and again when
//! it's done, taking the recipe's time (less with Handiwork, #46). Like eating, you can walk while
//! you do it; a strike landing interrupts it (with first aid's treatments), and swinging, shoving or
//! stomping drops the whole queue.
//!
//! **What counts.** Inputs and tools come from what a player carries and holds in its hands, not
//! what it wears (take the T-shirt off first), and only whole items count as inputs: the part-used
//! top one of a stack (a bottle half drunk) won't do. Asking for a recipe already queued queues
//! another only if there's enough for both; otherwise it's taken as a repeated click.
//!
//! **Outputs** are made new ([`World::fresh_stack`]: food made now is fresh), into pockets and bags
//! while they have room after the inputs are gone, the rest at the player's feet.
//!
//! The rules over an inventory ([`check`], [`check_times`]) are pure, so the inventory panel offers
//! only what the server will make. Recipes and their numbers are ours.

use serde::{Deserialize, Serialize};

use crate::firstaid::usable;
use crate::inventory::{Inventory, JobKind, merge_into};
use crate::items::{ItemDef, ItemStack};
use crate::world::World;

/// Longest a recipe can take (seconds).
pub const MAX_RECIPE_SECS: f32 = 600.0;
/// Most of one item a recipe can take or make.
pub const MAX_RECIPE_COUNT: u16 = 100;
/// Each Handiwork level (#46) makes crafting this much faster (the default of
/// `SkillTuning::craft_faster`)...
pub const HANDIWORK_FASTER_PER_LEVEL: f32 = 0.1;
/// ...down to this share of the recipe's time.
pub const HANDIWORK_MIN_MULT: f32 = 0.5;

fn one_u16() -> u16 {
    1
}

/// Some of one item a recipe takes or makes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeItem {
    /// An item id (`items.ron`).
    pub item: String,
    #[serde(default = "one_u16")]
    pub count: u16,
}

/// Something a recipe needs to hand but doesn't use up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tool {
    /// This item (an id in `items.ron`).
    Item(String),
    /// Any item with this tag (`cuts`, `fire`, ...).
    Tag(String),
}

/// One recipe. Seconds of play; counts of whole items.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeDef {
    pub id: String,
    /// What it's called ("Rip a T-shirt into rags").
    pub name: String,
    pub secs: f32,
    pub inputs: Vec<RecipeItem>,
    #[serde(default)]
    pub tools: Vec<Tool>,
    pub outputs: Vec<RecipeItem>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl RecipeDef {
    /// Check one recipe on its own: ids, counts, the time, and nothing that makes itself.
    pub fn validate(&self) -> Result<(), String> {
        let id = &self.id;
        if !valid_id(id) {
            return Err(format!("recipe id {id:?} must be lowercase letters, digits and _"));
        }
        let what = format!("recipe {id}");
        if self.name.trim().is_empty() {
            return Err(format!("{what}: name is empty"));
        }
        if !(self.secs > 0.0 && self.secs <= MAX_RECIPE_SECS) {
            return Err(format!("{what}: secs must be in (0, {MAX_RECIPE_SECS}], got {}", self.secs));
        }
        if self.inputs.is_empty() {
            return Err(format!("{what}: needs at least one input (it would make something from nothing)"));
        }
        if self.outputs.is_empty() {
            return Err(format!("{what}: makes nothing (no outputs)"));
        }
        for (list, kind) in [(&self.inputs, "input"), (&self.outputs, "output")] {
            for (k, r) in list.iter().enumerate() {
                if !valid_id(&r.item) {
                    return Err(format!("{what}: {kind} {:?} must be an item id (lowercase letters, digits and _)", r.item));
                }
                if r.count == 0 || r.count > MAX_RECIPE_COUNT {
                    return Err(format!("{what}: {kind} {} has count {}; it must be 1..{MAX_RECIPE_COUNT}", r.item, r.count));
                }
                if list[..k].iter().any(|o| o.item == r.item) {
                    return Err(format!("{what}: {kind} {} is listed twice (give it one entry with the count of both)", r.item));
                }
            }
        }
        for (k, t) in self.tools.iter().enumerate() {
            let (Tool::Item(s) | Tool::Tag(s)) = t;
            if !valid_id(s) {
                return Err(format!("{what}: tool {t:?} must name an item id or a tag (lowercase letters, digits and _)"));
            }
            if self.tools[..k].contains(t) {
                return Err(format!("{what}: tool {t:?} is listed twice"));
            }
            if let Tool::Item(s) = t
                && self.inputs.iter().any(|r| &r.item == s)
            {
                return Err(format!("{what}: {s} is both a tool (kept) and an input (used up)"));
            }
        }
        if let Some(r) = self.outputs.iter().find(|o| self.inputs.iter().any(|i| i.item == o.item)) {
            return Err(format!("{what}: {} is both an input and an output (it would make itself over and over)", r.item));
        }
        Ok(())
    }
}

/// Check a recipes file on its own: each recipe ([`RecipeDef::validate`]) and no id twice.
pub fn validate_recipes(defs: &[RecipeDef]) -> Result<(), String> {
    if defs.len() > u16::MAX as usize / 2 {
        return Err(format!("{} recipes is too many", defs.len()));
    }
    for (i, r) in defs.iter().enumerate() {
        r.validate()?;
        if defs[..i].iter().any(|o| o.id == r.id) {
            return Err(format!("recipe id {:?} is used twice", r.id));
        }
    }
    Ok(())
}

/// Check the recipes against the items they name: every input, output and tool item is in `items`,
/// and some item has each tool tag.
pub fn check_recipes(defs: &[RecipeDef], items: &[ItemDef]) -> Result<(), String> {
    let known = |id: &str| items.iter().any(|d| d.id == id);
    for r in defs {
        let what = format!("recipe {}", r.id);
        for (list, kind) in [(&r.inputs, "input"), (&r.outputs, "output")] {
            if let Some(x) = list.iter().find(|x| !known(&x.item)) {
                return Err(format!("{what}: {kind} {:?} isn't in items.ron", x.item));
            }
        }
        for t in &r.tools {
            match t {
                Tool::Item(s) if !known(s) => return Err(format!("{what}: tool {s:?} isn't in items.ron")),
                Tool::Tag(s) if !items.iter().any(|d| d.has_tag(s)) => return Err(format!("{what}: no item in items.ron has the tag {s:?} (tool Tag({s:?}))")),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Parse and validate a recipes file (a RON list of [`RecipeDef`]) on its own.
pub fn parse_recipes(text: &str) -> Result<Vec<RecipeDef>, String> {
    let defs: Vec<RecipeDef> = ron::from_str(text).map_err(|e| e.to_string())?;
    validate_recipes(&defs)?;
    Ok(defs)
}

/// Parse and validate a recipes file, and check it against `items`.
pub fn parse_recipes_for(text: &str, items: &[ItemDef]) -> Result<Vec<RecipeDef>, String> {
    let defs = parse_recipes(text)?;
    check_recipes(&defs, items)?;
    Ok(defs)
}

fn index_of(items: &[ItemDef], id: &str) -> Option<u16> {
    items.iter().position(|d| d.id == id).map(|i| i as u16)
}

/// Whole items in a stack: the part-used top one (`used` > 0) doesn't count.
pub fn whole(s: &ItemStack) -> u16 {
    s.count.saturating_sub((s.used > 0) as u16)
}

/// How many whole `item` a player has to craft with (carried and in its hands).
pub fn have(inv: &Inventory, item: u16) -> u32 {
    usable(inv).filter(|s| s.item == item).map(|s| whole(s) as u32).sum()
}

/// Does a player have tool `t` to hand (carried or held; a part-used one does)?
pub fn has_tool(inv: &Inventory, t: &Tool, items: &[ItemDef]) -> bool {
    usable(inv).any(|s| {
        items.get(s.item as usize).is_some_and(|d| match t {
            Tool::Item(id) => &d.id == id,
            Tool::Tag(tag) => d.has_tag(tag),
        })
    })
}

/// "rag x3", "t-shirt": an item by id, lowercase, and how many.
pub fn amount(items: &[ItemDef], id: &str, n: u16) -> String {
    let name = items.iter().find(|d| d.id == id).map_or(id.to_string(), |d| d.name.to_lowercase());
    if n > 1 { format!("{name} x{n}") } else { name }
}

/// A tool in words: "scissors", or for a tag the items that have it ("scissors or kitchen knife").
pub fn tool_text(items: &[ItemDef], t: &Tool) -> String {
    match t {
        Tool::Item(id) => amount(items, id, 1),
        Tool::Tag(tag) => {
            let names: Vec<String> = items.iter().filter(|d| d.has_tag(tag)).take(4).map(|d| d.name.to_lowercase()).collect();
            if names.is_empty() { format!("something tagged {tag}") } else { names.join(" or ") }
        }
    }
}

/// The recipe in brief: "t-shirt -> rag x3 (3 s)", with the tools ("with scissors or kitchen knife").
pub fn describe(r: &RecipeDef, items: &[ItemDef]) -> String {
    let list = |v: &[RecipeItem]| v.iter().map(|x| amount(items, &x.item, x.count)).collect::<Vec<_>>().join(" + ");
    let tools = if r.tools.is_empty() { String::new() } else { format!(", with {}", r.tools.iter().map(|t| tool_text(items, t)).collect::<Vec<_>>().join(" and ")) };
    format!("{} -> {} ({:.0} s{tools})", list(&r.inputs), list(&r.outputs), r.secs)
}

/// Can `r` be made `times` times over from what `inv` carries and holds (inputs for all of them,
/// and its tools)? If not, everything missing, in words. The inventory panel offers only what
/// passes; the server checks again when the craft is asked for and when it's done.
pub fn check_times(r: &RecipeDef, inv: &Inventory, items: &[ItemDef], times: u32) -> Result<(), String> {
    let mut missing = Vec::new();
    for x in &r.inputs {
        let need = x.count as u32 * times.max(1);
        let got = index_of(items, &x.item).map_or(0, |i| have(inv, i));
        if got < need {
            let worn = index_of(items, &x.item).is_some_and(|i| inv.worn.iter().any(|s| s.item == i));
            let more = if times > 1 { " for another" } else { "" };
            let hint = if worn { " (take off the one you're wearing first)" } else { "" };
            missing.push(format!("needs {}{more} (you have {got}){hint}", amount(items, &x.item, need.min(u16::MAX as u32) as u16)));
        }
    }
    for t in r.tools.iter().filter(|t| !has_tool(inv, t, items)) {
        missing.push(format!("needs {} to hand (kept)", tool_text(items, t)));
    }
    if missing.is_empty() { Ok(()) } else { Err(missing.join("; ")) }
}

/// Can `r` be made once from what `inv` carries and holds? See [`check_times`].
pub fn check(r: &RecipeDef, inv: &Inventory, items: &[ItemDef]) -> Result<(), String> {
    check_times(r, inv, items, 1)
}

/// Seconds a recipe of `secs` takes at Handiwork `level` (#46), with the built-in skill numbers
/// (a world uses its own `SkillTuning`: [`World::craft_secs`]).
pub fn craft_secs(secs: f32, level: u32) -> f32 {
    secs * crate::skills::SkillTuning::default().craft_time_mult(level)
}

/// Take `n` whole `item` from what `inv` crafts with: carried stacks first (in order), then the
/// hands. False (and `inv` part changed) if there aren't that many.
fn take_whole(inv: &mut Inventory, item: u16, mut n: u16) -> bool {
    let mut k = 0;
    while n > 0 && k < inv.carried.len() {
        let s = &mut inv.carried[k];
        if s.item == item {
            let take = whole(s).min(n);
            s.count -= take;
            n -= take;
            if s.count == 0 {
                inv.carried.remove(k);
                continue;
            }
        }
        k += 1;
    }
    for slot in [&mut inv.hand, &mut inv.off_hand] {
        if n > 0 && slot.is_some_and(|s| s.item == item && whole(&s) > 0) {
            *slot = None;
            n -= 1;
        }
    }
    n == 0
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

impl World {
    /// A recipe's index in `config.recipes`, by id.
    pub fn recipe_index(&self, id: &str) -> Option<u16> {
        self.config.recipes.iter().position(|r| r.id == id).map(|i| i as u16)
    }

    /// Seconds a recipe of `secs` takes player `pi`, with its Handiwork (#46).
    fn handiwork_secs(&self, pi: usize, secs: f32) -> f32 {
        secs * self.config.skills.craft_time_mult(self.skill_level(pi, crate::skills::Skill::Handiwork))
    }

    /// Player `pi` finished a craft: Handiwork XP (#46).
    fn on_crafted(&mut self, pi: usize) {
        self.award_xp(pi, crate::skills::Skill::Handiwork, self.config.skills.craft_xp);
    }

    /// Seconds recipe `recipe` takes player `id` (with its Handiwork, #46).
    pub fn craft_secs(&self, id: u32, recipe: u16) -> Option<f32> {
        let pi = self.players.iter().position(|p| p.id == id)?;
        let r = self.config.recipes.get(recipe as usize)?;
        Some(self.handiwork_secs(pi, r.secs))
    }

    /// Player `id` makes recipe `recipe` (an index into the recipes) from what it carries and
    /// holds. Queued like eating: it happens after the recipe's time if it still can then. Asking
    /// again for one already queued queues another only if there's enough for both (else it does
    /// nothing: a repeated click). Refusals are noted for its client and returned.
    pub fn player_craft(&mut self, id: u32, recipe: u16) -> Result<(), String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        if self.players[pi].dead {
            return self.refuse(id, "dead".into());
        }
        let Some(r) = self.config.recipes.get(recipe as usize) else { return self.refuse(id, format!("no recipe {recipe}")) };
        let p = &self.players[pi];
        let queued = p.jobs.iter().filter(|j| j.kind == JobKind::Craft { recipe }).count() as u32;
        if let Err(why) = check_times(r, &p.inventory, &self.config.items, queued + 1) {
            if queued > 0 {
                // Already on it (a repeated click), and there's not enough for another.
                return Ok(());
            }
            let text = format!("can't {}: {why}", lower(&r.name));
            return self.refuse(id, text);
        }
        let secs = self.handiwork_secs(pi, r.secs);
        self.queue_job(id, JobKind::Craft { recipe }, secs)
    }

    /// Carry out a craft (a due `JobKind::Craft`), checked again: the inputs and tools still to
    /// hand. The inputs go, the outputs come (into pockets and bags while there's room, the rest
    /// at the player's feet).
    pub(crate) fn do_craft(&mut self, pi: usize, recipe: u16) -> Result<String, String> {
        let r = self.config.recipes.get(recipe as usize).cloned().ok_or("that recipe is gone")?;
        let items = &self.config.items;
        let mut inv = self.players[pi].inventory.clone();
        check(&r, &inv, items).map_err(|why| format!("can't {}: {why}", lower(&r.name)))?;
        for x in &r.inputs {
            let i = index_of(items, &x.item).ok_or_else(|| format!("no item {}", x.item))?;
            if !take_whole(&mut inv, i, x.count) {
                return Err(format!("can't {}: the {} is gone", lower(&r.name), amount(items, &x.item, 1)));
            }
        }
        let used = r.inputs.iter().map(|x| if x.count > 1 { amount(items, &x.item, x.count) } else { format!("the {}", amount(items, &x.item, 1)) }).collect::<Vec<_>>().join(" and ");
        self.players[pi].inventory = inv;
        let (pos, facing) = (self.players[pi].state.pos, self.players[pi].state.facing);
        let mut made = Vec::new();
        let mut dropped = Vec::new();
        for x in &r.outputs {
            let i = index_of(&self.config.items, &x.item).ok_or_else(|| format!("no item {}", x.item))?;
            let def = self.item_def(i);
            let (size, vol) = (def.stack.max(1), def.volume_l);
            let like = self.fresh_stack(i, 1);
            let c = self.carry(&self.players[pi]);
            let room = (c.room_l - c.used_l).max(0.0);
            let fit = if vol > 0.0 { ((room + 1e-4) / vol).floor().min(x.count as f32) as u16 } else { x.count };
            if fit > 0 {
                merge_into(&mut self.players[pi].inventory.carried, like, fit, size);
            }
            let mut rest = x.count - fit;
            if rest > 0 {
                dropped.push(amount(&self.config.items, &x.item, rest));
            }
            while rest > 0 {
                let n = rest.min(size);
                self.put_on_floor(like.with_count(n), pos, facing);
                rest -= n;
            }
            let name = amount(&self.config.items, &x.item, 1);
            made.push(if x.count > 1 { format!("{} x {name}", x.count) } else { name });
        }
        self.on_crafted(pi);
        self.refresh_carry(pi);
        let floor = if dropped.is_empty() { String::new() } else { format!(" ({} at your feet: no room)", dropped.join(" and ")) };
        Ok(format!("made {} from {used}{floor}", made.join(" and ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TICK_DT;
    use crate::firstaid::Treatment;
    use crate::health::{Dressing, InfectionMode, Part, WoundKind};
    use crate::map::TileMap;
    use crate::players::{Player, PlayerInput};
    use crate::template::BuildingTemplate;
    use crate::world::WorldConfig;
    use crate::{Slot, items};
    use glam::Vec2;

    const RECIPES: &str = include_str!("../../../assets/data/items/recipes.ron");

    fn shipped_items() -> Vec<ItemDef> {
        let weapons = items::parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        items::parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap()
    }

    fn config(seed: u64) -> WorldConfig {
        let templates = [include_str!("../../../assets/data/buildings/bungalow.ron"), include_str!("../../../assets/data/buildings/family_two_storey.ron")]
            .iter()
            .map(|t| BuildingTemplate::from_ron(t).unwrap())
            .collect();
        let weapons = items::parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        let items = items::parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap();
        let recipes = parse_recipes_for(RECIPES, &items).unwrap();
        let mut c = WorldConfig::new(seed, 0, templates).with_weapons(weapons).with_items(items).with_recipes(recipes);
        c.start_hour = 12.0;
        c.infection.mode = InfectionMode::Off;
        c
    }

    /// An empty field at noon, one player at (100.5, 100.5).
    fn arena(seed: u64) -> (World, u32) {
        let mut w = World::new(config(seed));
        w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
        w.map_changed();
        w.items.clear();
        let id = w.add_player(Vec2::new(100.5, 100.5));
        (w, id)
    }

    fn me(w: &World, id: u32) -> &Player {
        w.player(id).unwrap()
    }

    fn count(w: &World, id: u32, it: &str) -> u32 {
        let i = w.item_index(it).unwrap();
        me(w, id).inventory.stacks().filter(|s| s.item == i).map(|s| s.count as u32).sum()
    }

    fn on_floor(w: &World, it: &str) -> u32 {
        let i = w.item_index(it).unwrap();
        w.items.iter().filter(|g| g.stack.item == i).map(|g| g.stack.count as u32).sum()
    }

    fn last_note(w: &World, id: u32) -> String {
        w.item_notes.iter().rev().find(|n| n.player == id).map(|n| n.text.clone()).unwrap_or_default()
    }

    fn recipe(w: &World, id: &str) -> u16 {
        w.recipe_index(id).unwrap()
    }

    /// Step until player `id` has nothing queued; the seconds it took.
    fn settle(w: &mut World, id: u32) -> f32 {
        for k in 0..800 {
            if me(w, id).jobs.is_empty() {
                return k as f32 * TICK_DT;
            }
            w.step();
        }
        panic!("jobs never finished");
    }

    fn craft(w: &mut World, id: u32, r: &str) -> f32 {
        let r = recipe(w, r);
        w.player_craft(id, r).unwrap_or_else(|e| panic!("refused: {e}"));
        settle(w, id)
    }

    #[test]
    fn the_shipped_recipes_load() {
        let items = shipped_items();
        let r = parse_recipes_for(RECIPES, &items).unwrap();
        assert!(r.len() >= 7);
        for id in ["rip_t_shirt", "cut_jeans", "rip_bed_sheet", "wash_rags"] {
            assert!(r.iter().any(|x| x.id == id), "{id}");
        }
        // Every recipe makes something first aid can use, or what makes it.
        assert!(r.iter().all(|x| x.outputs.iter().any(|o| o.item == "rag" || o.item == "clean_rag")));
        assert_eq!(describe(&r[0], &items), "t-shirt -> rag x3 (3 s)");
        assert!(describe(r.iter().find(|x| x.id == "cut_jeans").unwrap(), &items).contains("with scissors or kitchen knife"));
    }

    /// Mistakes in the recipes are refused with an error that names the recipe and what's wrong.
    #[test]
    fn bad_recipes_are_refused_by_name() {
        let items = shipped_items();
        let bad = |edit: fn(&mut Vec<RecipeDef>), want: &str| {
            let mut r = parse_recipes(RECIPES).unwrap();
            edit(&mut r);
            let err = validate_recipes(&r).and_then(|_| check_recipes(&r, &items)).unwrap_err();
            assert!(err.contains(want), "{err:?} should mention {want:?}");
        };
        bad(|r| r[0].inputs[0].item = "unobtainium".into(), "input \"unobtainium\" isn't in items.ron");
        bad(|r| r[0].outputs[0].item = "mithril".into(), "output \"mithril\" isn't in items.ron");
        bad(|r| r[0].tools.push(Tool::Item("sonic_screwdriver".into())), "tool \"sonic_screwdriver\" isn't in items.ron");
        bad(|r| r[0].tools.push(Tool::Tag("teleports".into())), "no item in items.ron has the tag \"teleports\"");
        bad(|r| r[0].inputs[0].count = 0, "count 0");
        bad(|r| r[0].outputs[0].count = 0, "count 0");
        bad(|r| r[0].secs = 0.0, "secs must be in (0");
        bad(|r| r[0].secs = -3.0, "secs must be in (0");
        bad(|r| r[0].secs = f32::NAN, "secs must be in (0");
        bad(|r| r[0].outputs.push(RecipeItem { item: "t_shirt".into(), count: 1 }), "both an input and an output");
        bad(|r| r[1].id = r[0].id.clone(), "used twice");
        bad(|r| r[0].inputs.clear(), "at least one input");
        bad(|r| r[0].outputs.clear(), "makes nothing");
        bad(|r| r[0].tools.push(Tool::Item("t_shirt".into())), "both a tool");
        bad(|r| r[0].inputs.push(RecipeItem { item: "t_shirt".into(), count: 2 }), "listed twice");
        bad(|r| r[0].id = "Rip It".into(), "lowercase");
        let err = bad_ron("[(id: \"x\", name: \"X\", secs: 1.0, inputs: [(item: \"rag\")], outputs: [(item: \"clean_rag\")], time: 3.0)]");
        assert!(err.contains("time"), "unknown fields are refused: {err}");
        // The error names the recipe.
        let mut r = parse_recipes(RECIPES).unwrap();
        r[2].inputs[0].item = "nope".into();
        assert!(check_recipes(&r, &items).unwrap_err().starts_with("recipe rip_hoodie:"));
    }

    fn bad_ron(text: &str) -> String {
        parse_recipes(text).unwrap_err()
    }

    /// Cutting jeans into rags: the jeans go, the scissors stay, four rags after six seconds.
    #[test]
    fn crafting_uses_inputs_keeps_tools_and_makes_outputs_in_time() {
        let (mut w, id) = arena(3);
        w.give_item(id, "jeans", 1);
        w.give_item(id, "scissors", 1);
        let r = recipe(&w, "cut_jeans");
        w.player_craft(id, r).unwrap();
        assert_eq!(w.craft_secs(id, r), Some(6.0));
        for _ in 0..100 {
            w.step();
        }
        assert_eq!((count(&w, id, "jeans"), count(&w, id, "rag")), (1, 0), "not done after 5 s");
        assert!(me(&w, id).state.mods.speed > 0.0, "you can walk while you craft (like eating)");
        let secs = settle(&mut w, id) + 5.0;
        assert!((secs - 6.0).abs() < 0.11, "took {secs}");
        assert_eq!((count(&w, id, "jeans"), count(&w, id, "scissors"), count(&w, id, "rag")), (0, 1, 4));
        assert_eq!(last_note(&w, id), "made 4 x rag from the jeans");
        // A kitchen knife cuts too (any item tagged `cuts`), held in the hand.
        w.players[0].inventory.carried.clear();
        w.hold_item(id, Slot::Hand, "kitchen_knife");
        w.give_item(id, "jeans", 1);
        craft(&mut w, id, "cut_jeans");
        assert_eq!((count(&w, id, "rag"), count(&w, id, "kitchen_knife")), (4, 1));
        // An input held in the hand counts too.
        w.hold_item(id, Slot::OffHand, "t_shirt");
        craft(&mut w, id, "rip_t_shirt");
        assert!(me(&w, id).inventory.off_hand.is_none() && count(&w, id, "rag") == 7);
        // Rags from rags would loop; the outputs are new stacks that merge with what's there.
        assert_eq!(me(&w, id).inventory.carried.iter().filter(|s| s.item == w.item_index("rag").unwrap()).count(), 1);
    }

    /// Nothing to make it from, no tool, a worn T-shirt, a half-drunk bottle: refused, saying why,
    /// and nothing queued. Gone by the time it's due: refused then, nothing used.
    #[test]
    fn missing_inputs_and_tools_are_refused() {
        let (mut w, id) = arena(5);
        let jeans = recipe(&w, "cut_jeans");
        let err = w.player_craft(id, jeans).unwrap_err();
        assert!(err.contains("needs jeans (you have 0)") && err.contains("needs scissors or kitchen knife to hand"), "{err}");
        w.give_item(id, "jeans", 1);
        let err = w.player_craft(id, jeans).unwrap_err();
        assert!(!err.contains("needs jeans") && err.contains("scissors"), "{err}");
        assert!(me(&w, id).jobs.is_empty());
        assert!(w.player_craft(id, 999).unwrap_err().contains("no recipe"));
        // Worn: take it off first.
        let shirt = w.item_index("t_shirt").unwrap();
        w.players[0].inventory.worn.push(ItemStack::one(shirt));
        let err = w.player_craft(id, recipe(&w, "rip_t_shirt")).unwrap_err();
        assert!(err.contains("take off the one you're wearing first"), "{err}");
        // Washing rags: three rags and a whole bottle of water.
        w.give_item(id, "rag", 2);
        w.give_item(id, "water_bottle", 1);
        let wash = recipe(&w, "wash_rags");
        assert!(w.player_craft(id, wash).unwrap_err().contains("needs rag x3 (you have 2)"));
        w.give_item(id, "rag", 1);
        let bottle = w.item_index("water_bottle").unwrap();
        w.players[0].inventory.carried.iter_mut().find(|s| s.item == bottle).unwrap().used = 1;
        let err = w.player_craft(id, wash).unwrap_err();
        assert!(err.contains("needs bottle of water (you have 0)"), "a half-drunk bottle won't do: {err}");
        w.players[0].inventory.carried.iter_mut().find(|s| s.item == bottle).unwrap().used = 0;
        w.player_craft(id, wash).unwrap();
        // Asking again with only enough for one: a repeated click, nothing more.
        w.player_craft(id, wash).unwrap();
        assert_eq!(me(&w, id).jobs.len(), 1);
        // The rags gone before it's done: refused then, nothing used.
        let rag = w.item_index("rag").unwrap();
        w.players[0].inventory.carried.retain(|s| s.item != rag);
        settle(&mut w, id);
        assert!(last_note(&w, id).contains("can't wash rags in clean water: needs rag x3 (you have 0)"), "{}", last_note(&w, id));
        assert_eq!((count(&w, id, "water_bottle"), count(&w, id, "clean_rag")), (1, 0));
        // Enough for two: two queue.
        w.give_item(id, "rag", 6);
        w.give_item(id, "water_bottle", 1);
        w.player_craft(id, wash).unwrap();
        w.player_craft(id, wash).unwrap();
        w.player_craft(id, wash).unwrap();
        assert_eq!(me(&w, id).jobs.len(), 2);
        settle(&mut w, id);
        // (The jeans still in the pockets leave no room for the second bottle: it's at our feet.)
        assert_eq!((count(&w, id, "clean_rag"), count(&w, id, "empty_bottle"), on_floor(&w, "empty_bottle"), count(&w, id, "rag")), (6, 1, 1, 0));
    }

    /// A strike landing interrupts crafting, and so does a swing; nothing is used up.
    #[test]
    fn being_hit_or_swinging_interrupts_crafting() {
        let (mut w, id) = arena(7);
        w.give_item(id, "bed_sheet", 1);
        let r = recipe(&w, "rip_bed_sheet");
        w.player_craft(id, r).unwrap();
        for _ in 0..20 {
            w.step();
        }
        w.wound_player(0, Part::LeftArm, WoundKind::Scratch, 5.0, Vec2::new(101.5, 100.5), 1);
        assert!(me(&w, id).jobs.is_empty() && last_note(&w, id).contains("interrupted"));
        for _ in 0..200 {
            w.step();
        }
        assert_eq!((count(&w, id, "bed_sheet"), count(&w, id, "rag")), (1, 0));
        w.player_craft(id, r).unwrap();
        w.step();
        assert!(w.player_attack(id));
        assert!(me(&w, id).jobs.is_empty());
        settle(&mut w, id);
        assert_eq!((count(&w, id, "bed_sheet"), count(&w, id, "rag")), (1, 0));
        // Left alone, it's done.
        craft(&mut w, id, "rip_bed_sheet");
        assert_eq!((count(&w, id, "bed_sheet"), count(&w, id, "rag")), (0, 6));
    }

    /// Outputs that don't fit in pockets and bags go at the player's feet.
    #[test]
    fn outputs_with_no_room_go_on_the_floor() {
        let (mut w, id) = arena(9);
        // Pockets are 3 L: two bed sheets fill them, the T-shirt is over.
        w.give_item(id, "bed_sheet", 2);
        w.give_item(id, "t_shirt", 1);
        craft(&mut w, id, "rip_t_shirt");
        assert_eq!((count(&w, id, "rag"), on_floor(&w, "rag")), (0, 3));
        assert!(last_note(&w, id).contains("rag x3 at your feet: no room"), "{}", last_note(&w, id));
        let g = w.items.iter().find(|g| g.stack.item == w.item_index("rag").unwrap()).unwrap();
        assert!(g.pos.distance(me(&w, id).state.pos) < 0.01, "at its feet");
        assert!(w.floor_ids(id).contains(&g.id), "within reach");
        // Some room: what fits goes in, the rest down.
        let sheet = w.item_index("bed_sheet").unwrap();
        w.players[0].inventory.carried.retain(|s| s.item != sheet);
        w.give_item(id, "bed_sheet", 1);
        w.give_item(id, "towel", 1);
        w.give_item(id, "jumper", 1);
        // 1.5 + 1.5 + 1.2 = 4.2 L; less the jumper 3.0 L: four rags (0.4 L) have no room at all.
        craft(&mut w, id, "rip_jumper");
        assert_eq!((count(&w, id, "rag"), on_floor(&w, "rag")), (0, 7));
        w.players[0].inventory.carried.retain(|s| s.item != sheet);
        w.give_item(id, "t_shirt", 1);
        // 1.5 (towel) + 0.5: the shirt's three rags fit.
        craft(&mut w, id, "rip_t_shirt");
        assert_eq!(count(&w, id, "rag"), 3);
    }

    /// Washed rags dress a wound clean (a plain rag goes on dirty).
    #[test]
    fn a_washed_rag_dresses_a_wound_clean() {
        let (mut w, id) = arena(11);
        w.give_item(id, "towel", 1);
        w.give_item(id, "water_bottle", 1);
        craft(&mut w, id, "rip_towel");
        craft(&mut w, id, "wash_rags");
        assert_eq!((count(&w, id, "rag"), count(&w, id, "clean_rag"), count(&w, id, "empty_bottle")), (1, 3, 1));
        w.inflict_wound(id, Part::Torso, WoundKind::Bite, Some(false)).unwrap();
        let bite = me(&w, id).health.wounds.last().unwrap().id;
        let clean = w.item_index("clean_rag").unwrap();
        w.player_treat(id, Some(bite), Treatment::Dress, Some(clean)).unwrap();
        settle(&mut w, id);
        let wd = *me(&w, id).health.wounds.iter().find(|x| x.id == bite).unwrap();
        assert_eq!(wd.dressing, Dressing::Clean, "{}", last_note(&w, id));
        assert!(!wd.bleeding);
        assert_eq!(count(&w, id, "clean_rag"), 2);
        // A dressing is the best first aid would pick with no item named: clean before dirty.
        w.inflict_wound(id, Part::LeftLeg, WoundKind::Gash, Some(false)).unwrap();
        let gash = me(&w, id).health.wounds.last().unwrap().id;
        w.player_treat(id, Some(gash), Treatment::Dress, None).unwrap();
        settle(&mut w, id);
        assert_eq!(me(&w, id).health.wounds.iter().find(|x| x.id == gash).unwrap().dressing, Dressing::Clean);
    }

    /// Handiwork (#46) makes crafting faster: 10% a level, down to half.
    #[test]
    fn handiwork_makes_it_faster() {
        assert_eq!(craft_secs(10.0, 0), 10.0);
        assert!((craft_secs(10.0, 3) - 7.0).abs() < 1e-5);
        assert_eq!(craft_secs(10.0, 9), 5.0);
    }

    /// Each craft is Handiwork XP (#46), and a higher Handiwork level crafts faster in the world.
    #[test]
    fn crafting_earns_handiwork_and_handiwork_speeds_it_up() {
        let (mut w, id) = arena(31);
        let t = w.config.skills.clone();
        let r = recipe(&w, "rip_t_shirt");
        let base = w.config.recipes[r as usize].secs;
        assert_eq!(w.craft_secs(id, r), Some(base));
        w.give_item(id, "t_shirt", 2);
        craft(&mut w, id, "rip_t_shirt");
        assert_eq!(me(&w, id).skills.xp(crate::skills::Skill::Handiwork), t.craft_xp);
        // Handiwork 3: 30% faster, asked for and done.
        w.players[0].skills.xp[crate::skills::Skill::Handiwork as usize] = t.xp_for(3);
        assert!((w.craft_secs(id, r).unwrap() - base * 0.7).abs() < 1e-4);
        let took = craft(&mut w, id, "rip_t_shirt");
        assert!((took - base * 0.7).abs() < 0.11, "took {took} s of {base}");
        assert_eq!(me(&w, id).skills.xp(crate::skills::Skill::Handiwork), t.xp_for(3) + t.craft_xp);
    }

    /// A craft under way is saved: a loaded world steps exactly like the original and finishes
    /// it; the same play twice is the same; a queued craft changes the hash.
    #[test]
    fn crafting_saves_in_lockstep_and_is_deterministic() {
        let run = || {
            let mut w = World::new(config(23));
            let id = w.add_player(w.player_spawn);
            w.give_item(id, "hoodie", 1);
            w.give_item(id, "rag", 3);
            w.give_item(id, "water_bottle", 1);
            let (a, b) = (w.recipe_index("rip_hoodie").unwrap(), w.recipe_index("wash_rags").unwrap());
            w.player_craft(id, a).unwrap();
            w.player_craft(id, b).unwrap();
            for _ in 0..40 {
                w.step();
            }
            (w, id)
        };
        let (mut w, id) = run();
        assert_eq!(me(&w, id).jobs.len(), 2, "ripping under way, washing queued");
        assert_eq!(run().0.state_hash(), w.state_hash(), "deterministic");
        let mut loaded = World::load(&w.save(), &w.config).unwrap();
        assert_eq!(loaded.players, w.players);
        assert_eq!(loaded.state_hash(), w.state_hash());
        for t in 0..400u32 {
            let input = PlayerInput { move_dir: Vec2::from_angle(t as f32 * 0.05), facing: t as f32 * 0.05, run: false, sneak: false };
            for world in [&mut w, &mut loaded] {
                world.set_player_input(id, t + 1, input);
                world.step();
            }
            assert_eq!(loaded.state_hash(), w.state_hash(), "tick {t}");
        }
        assert!(me(&w, id).jobs.is_empty());
        assert_eq!((count(&w, id, "hoodie"), count(&w, id, "rag"), count(&w, id, "clean_rag")), (0, 5, 3));
        assert_eq!(loaded.players, w.players);
        let h = w.state_hash();
        w.give_item(id, "t_shirt", 1);
        let h2 = w.state_hash();
        assert_ne!(h, h2);
        w.player_craft(id, w.recipe_index("rip_t_shirt").unwrap()).unwrap();
        assert_ne!(w.state_hash(), h2, "a queued craft is in the hash");
    }
}
