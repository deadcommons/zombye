//! The Luau side: one sandboxed VM a mod, the `game` object its hooks are handed, and the calls.
//!
//! What keeps scripts from breaking determinism (docs/modding.md has the modder's view):
//! - a VM has only Luau's pure libraries (`table`, `string`, `math`, `utf8`, `bit32`, `buffer`):
//!   no `os` (clocks), no `io`, no `require`, `loadstring`, `collectgarbage`, `getfenv`/`setfenv`,
//!   and no `math.random` (random numbers come from the world: `game:random`);
//! - the world is reached only through `game`, a userdata that is valid for the length of one
//!   hook call and whose methods are `World`'s own checked functions;
//! - a call is cut off after [`BUDGET`] interrupts (Luau raises one at every loop iteration and
//!   function call: a count, not a clock, so the same on every machine and every run);
//! - hooks run in a fixed order: mods in load order; for each, the tick's events in the order
//!   they happened, then `on_tick`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use glam::Vec2;
use mlua::{Function, Lua, LuaOptions, StdLib, Table, Value as LuaValue, Variadic, VmState};
use zombye_sim::script::{self, Need, ScriptEngine, ScriptEvent, ScriptHost, Value};
use zombye_sim::{TICK_RATE, World};

/// Interrupts one hook call (or loading one mod's scripts) may use before it is cut off: about
/// that many loop iterations and function calls, a millisecond or two of Luau.
pub const BUDGET: u32 = 100_000;
/// Memory one mod's VM may hold.
pub const MEMORY: usize = 16 * 1024 * 1024;
/// Entries a table handed to `game:set` may have in all.
const MAX_ENTRIES: usize = 4096;
/// Errors a mod prints to stderr before it goes quiet (they are still kept in the world).
const LOUD_ERRORS: u32 = 5;

/// The hooks a script may define, as global functions.
pub const HOOKS: [&str; 6] = ["on_tick", "on_use", "on_hit", "on_craft", "on_death", "on_spawn"];

/// One mod's scripts: file names and sources, in load order.
#[derive(Clone, Debug, PartialEq)]
pub struct ModScripts {
    pub id: String,
    pub sources: Vec<(String, String)>,
}

impl ModScripts {
    /// FNV-1a of the names and sources.
    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for &b in bytes {
                h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
            }
            h = (h ^ 0xff).wrapping_mul(0x0100_0000_01b3);
        };
        for (name, source) in &self.sources {
            eat(name.as_bytes());
            eat(source.as_bytes());
        }
        h
    }
}

/// The loaded mods' scripts ([`ScriptEngine`]): makes a host with fresh VMs for each world.
#[derive(Debug)]
pub struct LuauEngine {
    mods: Vec<ModScripts>,
}

impl LuauEngine {
    /// Checks that every mod's scripts load (in a VM that is then thrown away).
    pub fn new(mods: Vec<ModScripts>) -> Result<Self, String> {
        for m in &mods {
            Vm::build(m)?;
        }
        Ok(Self { mods })
    }
}

impl ScriptEngine for LuauEngine {
    fn start(&self) -> Box<dyn ScriptHost> {
        let mut host = LuauHost { vms: Vec::new(), failed: Vec::new() };
        for m in &self.mods {
            match Vm::build(m) {
                Ok(vm) => host.vms.push(vm),
                // They loaded when the engine was made, so this is not expected.
                Err(why) => host.failed.push(why),
            }
        }
        Box::new(host)
    }

    fn mods(&self) -> Vec<(String, u64)> {
        self.mods.iter().map(|m| (m.id.clone(), m.hash())).collect()
    }
}

struct LuauHost {
    vms: Vec<Vm>,
    failed: Vec<String>,
}

impl ScriptHost for LuauHost {
    fn run(&mut self, world: &mut World, events: &[ScriptEvent]) {
        for why in self.failed.drain(..) {
            world.script_error(why);
        }
        for vm in &self.vms {
            vm.run(world, events);
        }
    }
}

struct Hooks {
    on_tick: Option<Function>,
    on_use: Option<Function>,
    on_hit: Option<Function>,
    on_craft: Option<Function>,
    on_death: Option<Function>,
    on_spawn: Option<Function>,
}

struct Vm {
    id: String,
    lua: Lua,
    hooks: Hooks,
    budget: Arc<AtomicU32>,
    errors: AtomicU32,
}

/// A hook call with its arguments worked out (the world is lent to `game` while hooks run).
enum Call<'a> {
    Use(&'a Function, u32, String),
    Hit(&'a Function, u32, u32, f32, bool),
    Craft(&'a Function, u32, String),
    Player(&'a Function, &'static str, u32),
    Tick(&'a Function),
}

fn err(text: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(text.into())
}

impl Vm {
    fn build(m: &ModScripts) -> Result<Vm, String> {
        let fail = |what: &str, e: mlua::Error| format!("mod {}: {what}: {e}", m.id);
        let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::BIT | StdLib::BUFFER;
        let lua = Lua::new_with(libs, LuaOptions::default()).map_err(|e| fail("starting Luau", e))?;
        lua.set_memory_limit(MEMORY).map_err(|e| fail("memory limit", e))?;
        let budget = Arc::new(AtomicU32::new(BUDGET));
        let left = budget.clone();
        lua.set_interrupt(move |_| match left.load(Relaxed) {
            0 => Err(err(format!("ran too long (a call may use {BUDGET} loop iterations and function calls)"))),
            n => {
                left.store(n - 1, Relaxed);
                Ok(VmState::Continue)
            }
        });
        let setup = || -> mlua::Result<()> {
            let globals = lua.globals();
            // Anything that reads the machine, the clock or files, or loads more code.
            for name in ["require", "loadstring", "collectgarbage", "getfenv", "setfenv", "newproxy", "os", "io", "debug", "package"] {
                globals.raw_set(name, LuaValue::Nil)?;
            }
            let math: Table = globals.get("math")?;
            math.raw_set("random", LuaValue::Nil)?;
            math.raw_set("randomseed", LuaValue::Nil)?;
            let id = m.id.clone();
            let print = lua.create_function(move |_, args: Variadic<LuaValue>| {
                let line: Vec<String> = args.iter().map(|v| v.to_string().unwrap_or_else(|_| "?".into())).collect();
                eprintln!("[mod {id}] {}", line.join("\t"));
                Ok(())
            })?;
            globals.raw_set("print", print)?;
            register_game(&lua, &m.id)?;
            // Libraries and globals read-only from here; the scripts' own globals go to a table of
            // their own in front of them.
            lua.sandbox(true)
        };
        setup().map_err(|e| fail("setting up the sandbox", e))?;
        for (name, source) in &m.sources {
            budget.store(BUDGET, Relaxed);
            lua.load(source.as_str()).set_name(format!("={}/{name}", m.id)).exec().map_err(|e| format!("mod {}: {name}: {e}", m.id))?;
        }
        let hook = |name: &str| -> Result<Option<Function>, String> {
            match lua.globals().get::<LuaValue>(name) {
                Ok(LuaValue::Function(f)) => Ok(Some(f)),
                Ok(LuaValue::Nil) => Ok(None),
                Ok(other) => Err(format!("mod {}: {name} must be a function, not a {}", m.id, other.type_name())),
                Err(e) => Err(fail(name, e)),
            }
        };
        let hooks = Hooks { on_tick: hook("on_tick")?, on_use: hook("on_use")?, on_hit: hook("on_hit")?, on_craft: hook("on_craft")?, on_death: hook("on_death")?, on_spawn: hook("on_spawn")? };
        Ok(Vm { id: m.id.clone(), lua, hooks, budget, errors: AtomicU32::new(0) })
    }

    fn run(&self, world: &mut World, events: &[ScriptEvent]) {
        let h = &self.hooks;
        let mut calls: Vec<Call> = Vec::new();
        for e in events {
            match *e {
                ScriptEvent::Used { player, item } => {
                    if let Some(f) = &h.on_use {
                        calls.push(Call::Use(f, player, world.item_def(item).id.clone()));
                    }
                }
                ScriptEvent::Hit { player, zombie, damage, killed } => {
                    if let Some(f) = &h.on_hit {
                        calls.push(Call::Hit(f, player, zombie, damage, killed));
                    }
                }
                ScriptEvent::Crafted { player, recipe } => {
                    if let (Some(f), Some(r)) = (&h.on_craft, world.config.recipes.get(recipe as usize)) {
                        calls.push(Call::Craft(f, player, r.id.clone()));
                    }
                }
                ScriptEvent::Died { player } => {
                    if let Some(f) = &h.on_death {
                        calls.push(Call::Player(f, "on_death", player));
                    }
                }
                ScriptEvent::Spawned { player } => {
                    if let Some(f) = &h.on_spawn {
                        calls.push(Call::Player(f, "on_spawn", player));
                    }
                }
            }
        }
        if let Some(f) = &h.on_tick {
            calls.push(Call::Tick(f));
        }
        if calls.is_empty() {
            return;
        }
        let mut errors: Vec<String> = Vec::new();
        let scoped = self.lua.scope(|scope| {
            let game = scope.create_any_userdata_ref_mut(world)?;
            for call in &calls {
                self.budget.store(BUDGET, Relaxed);
                let (name, result) = match call {
                    Call::Use(f, player, item) => ("on_use", f.call::<()>((game.clone(), *player, item.as_str()))),
                    Call::Hit(f, player, zombie, damage, killed) => ("on_hit", f.call::<()>((game.clone(), *player, *zombie, *damage, *killed))),
                    Call::Craft(f, player, recipe) => ("on_craft", f.call::<()>((game.clone(), *player, recipe.as_str()))),
                    Call::Player(f, name, player) => (*name, f.call::<()>((game.clone(), *player))),
                    Call::Tick(f) => ("on_tick", f.call::<()>(game.clone())),
                };
                if let Err(e) = result {
                    errors.push(format!("mod {}: {name}: {}", self.id, first_lines(&e.to_string())));
                }
            }
            Ok(())
        });
        if let Err(e) = scoped {
            errors.push(format!("mod {}: {e}", self.id));
        }
        for text in errors {
            let n = self.errors.fetch_add(1, Relaxed);
            if n < LOUD_ERRORS {
                eprintln!("{text}");
                if n + 1 == LOUD_ERRORS {
                    eprintln!("mod {}: more errors are kept in the world but not printed", self.id);
                }
            }
            world.script_error(text);
        }
    }
}

/// An error without its stack traceback: what went wrong and where.
fn first_lines(text: &str) -> String {
    text.lines().take_while(|l| !l.starts_with("stack traceback")).collect::<Vec<_>>().join(" ").trim().to_string()
}

fn pos_of(x: f32, y: f32) -> Vec2 {
    Vec2::new(x, y)
}

/// A whole number of things asked for, from whatever number the script gave.
fn whole(n: Option<f64>, default: u32) -> mlua::Result<u32> {
    match n {
        None => Ok(default),
        Some(n) if n.is_finite() && n >= 0.0 => Ok(n.min(1e6) as u32),
        Some(n) => Err(err(format!("a count can't be {n}"))),
    }
}

/// The `game` object: methods on the world, for the length of a hook call.
fn register_game(lua: &Lua, mod_id: &str) -> mlua::Result<()> {
    let (get_id, set_id) = (mod_id.to_string(), mod_id.to_string());
    lua.register_userdata_type::<World>(move |reg| {
        use mlua::UserDataMethods;
        // The clock.
        reg.add_method("tick", |_, w, ()| Ok(w.tick));
        reg.add_method("seconds", |_, w, ()| Ok(w.tick as f64 / TICK_RATE as f64));
        reg.add_method("hour", |_, w, ()| Ok(w.time_of_day()));
        reg.add_method("day", |_, w, ()| Ok((w.clock_hours() / 24.0).floor().max(0.0) as u32 + 1));
        reg.add_method("daylight", |_, w, ()| Ok(w.daylight()));
        // Random numbers, from the world's stream for scripts.
        reg.add_method_mut("random", |_, w, (a, b): (Option<f64>, Option<f64>)| {
            let x = w.script_random() as f64;
            let (lo, hi) = match (a, b) {
                (None, _) => return Ok(x),
                (Some(n), None) => (1.0, n.floor()),
                (Some(a), Some(b)) => (a.ceil(), b.floor()),
            };
            if !(lo.is_finite() && hi.is_finite()) || hi < lo {
                return Err(err("random(n) wants n >= 1, random(a, b) wants a <= b"));
            }
            Ok((lo + (x * (hi - lo + 1.0)).floor()).min(hi))
        });
        // What the mod keeps between calls, saved with the world.
        reg.add_method("get", move |lua, w, key: String| match w.script_get(&get_id, &key) {
            Some(v) => to_lua(lua, v),
            None => Ok(LuaValue::Nil),
        });
        reg.add_method_mut("set", move |_, w, (key, value): (String, LuaValue)| {
            let value = match value {
                LuaValue::Nil => None,
                v => Some(from_lua(&v, 1, &mut 0)?),
            };
            w.script_set(&set_id, &key, value).map_err(|why| err(format!("set({key:?}): {why}")))
        });
        // Players.
        reg.add_method("players", |_, w, ()| Ok(w.players.iter().map(|p| p.id).collect::<Vec<u32>>()));
        reg.add_method("player", |lua, w, id: u32| {
            let Some(p) = w.script_player(id) else { return Ok(LuaValue::Nil) };
            let t = lua.create_table()?;
            t.raw_set("id", p.id)?;
            t.raw_set("x", p.pos.x)?;
            t.raw_set("y", p.pos.y)?;
            t.raw_set("facing", p.facing)?;
            t.raw_set("hp", p.hp)?;
            t.raw_set("max_hp", p.max_hp)?;
            t.raw_set("dead", p.dead)?;
            t.raw_set("asleep", p.asleep)?;
            t.raw_set("indoors", p.indoors)?;
            t.raw_set("endurance", p.endurance)?;
            t.raw_set("hunger", p.hunger)?;
            t.raw_set("thirst", p.thirst)?;
            t.raw_set("tiredness", p.tiredness)?;
            Ok(LuaValue::Table(t))
        });
        reg.add_method_mut("heal", |_, w, (id, hp): (u32, f32)| Ok(w.script_heal(id, hp)));
        reg.add_method_mut("hurt", |_, w, (id, hp): (u32, f32)| Ok(w.script_hurt(id, hp)));
        reg.add_method_mut("set_need", |_, w, (id, need, value): (u32, String, f32)| {
            let need = match need.as_str() {
                "hunger" => Need::Hunger,
                "thirst" => Need::Thirst,
                "tiredness" => Need::Tiredness,
                other => return Err(err(format!("no need {other:?} (hunger, thirst, tiredness)"))),
            };
            Ok(w.script_set_need(id, need, value))
        });
        reg.add_method_mut("set_endurance", |_, w, (id, value): (u32, f32)| Ok(w.script_set_endurance(id, value)));
        reg.add_method_mut("tell", |_, w, (id, text): (u32, String)| Ok(w.script_tell(id, &text)));
        // Items.
        reg.add_method("item", |lua, w, id: String| {
            let Some(d) = w.item_index(&id).map(|i| w.item_def(i)) else { return Ok(LuaValue::Nil) };
            let t = lua.create_table()?;
            t.raw_set("id", d.id.as_str())?;
            t.raw_set("name", d.name.as_str())?;
            t.raw_set("category", format!("{:?}", d.category).to_lowercase())?;
            t.raw_set("weight_kg", d.weight_kg)?;
            t.raw_set("tags", d.tags.clone())?;
            Ok(LuaValue::Table(t))
        });
        reg.add_method("count", |_, w, (id, item): (u32, String)| Ok(w.script_count(id, &item)));
        reg.add_method_mut("give", |_, w, (id, item, count): (u32, String, Option<f64>)| Ok(w.script_give(id, &item, whole(count, 1)?)));
        reg.add_method_mut("take", |_, w, (id, item, count): (u32, String, Option<f64>)| Ok(w.script_take(id, &item, whole(count, 1)?)));
        reg.add_method_mut("spawn_item", |_, w, (item, x, y, count): (String, f32, f32, Option<f64>)| Ok(w.spawn_item(&item, whole(count, 1)?.min(1000) as u16, pos_of(x, y))));
        reg.add_method("ground_item", |lua, w, id: u32| {
            let Some(it) = w.items().iter().find(|it| it.id == id) else { return Ok(LuaValue::Nil) };
            let t = lua.create_table()?;
            t.raw_set("id", it.id)?;
            t.raw_set("item", w.item_def(it.stack.item).id.as_str())?;
            t.raw_set("count", it.stack.count)?;
            t.raw_set("x", it.pos.x)?;
            t.raw_set("y", it.pos.y)?;
            Ok(LuaValue::Table(t))
        });
        reg.add_method_mut("remove_item", |_, w, id: u32| Ok(w.script_remove_ground(id)));
        // The world.
        reg.add_method_mut("noise", |_, w, (x, y, radius, lasting): (f32, f32, f32, Option<bool>)| Ok(w.script_noise(pos_of(x, y), radius, lasting.unwrap_or(false))));
        reg.add_method("zombies_near", |_, w, (x, y, radius): (f32, f32, f32)| Ok(w.script_zombies_near(pos_of(x, y), radius)));
        reg.add_method_mut("spawn_zombies", |_, w, (x, y, count, radius): (f32, f32, Option<f64>, Option<f32>)| Ok(w.script_spawn_zombies(pos_of(x, y), whole(count, 1)?, radius.unwrap_or(3.0))));
    })
}

/// A Luau value as something the world stores: booleans, finite numbers, strings, and tables that
/// are a list (keys 1..n) or a record (string keys) of the same.
fn from_lua(v: &LuaValue, depth: usize, entries: &mut usize) -> mlua::Result<Value> {
    Ok(match v {
        LuaValue::Boolean(b) => Value::Bool(*b),
        LuaValue::Integer(n) => Value::Num(*n as f64),
        LuaValue::Number(n) if n.is_finite() => Value::Num(*n),
        LuaValue::Number(n) => return Err(err(format!("{n} can't be stored"))),
        LuaValue::String(s) => Value::Str(s.to_str().map_err(|_| err("a stored string must be UTF-8"))?.to_string()),
        LuaValue::Table(t) => {
            if depth > script::MAX_DEPTH {
                return Err(err(format!("tables nested deeper than {} can't be stored", script::MAX_DEPTH)));
            }
            let len = t.raw_len();
            let mut record = BTreeMap::new();
            let mut numbered = 0usize;
            for pair in t.pairs::<LuaValue, LuaValue>() {
                let (k, v) = pair?;
                *entries += 1;
                if *entries > MAX_ENTRIES {
                    return Err(err(format!("more than {MAX_ENTRIES} entries can't be stored at once")));
                }
                match k {
                    LuaValue::String(s) => {
                        let key = s.to_str().map_err(|_| err("a stored key must be UTF-8"))?.to_string();
                        record.insert(key, from_lua(&v, depth + 1, entries)?);
                    }
                    LuaValue::Integer(_) | LuaValue::Number(_) => numbered += 1,
                    other => return Err(err(format!("a stored table's keys are strings or 1..n, not a {}", other.type_name()))),
                }
            }
            if numbered == 0 {
                if record.is_empty() { Value::List(Vec::new()) } else { Value::Map(record) }
            } else if !record.is_empty() || numbered != len {
                return Err(err("a stored table is a list (keys 1..n, no gaps) or a record (string keys), not both"));
            } else {
                let mut list = Vec::with_capacity(len);
                for i in 1..=len {
                    list.push(from_lua(&t.raw_get::<LuaValue>(i)?, depth + 1, entries)?);
                }
                Value::List(list)
            }
        }
        other => return Err(err(format!("a {} can't be stored", other.type_name()))),
    })
}

fn to_lua(lua: &Lua, v: &Value) -> mlua::Result<LuaValue> {
    Ok(match v {
        Value::Bool(b) => LuaValue::Boolean(*b),
        Value::Num(n) => LuaValue::Number(*n),
        Value::Str(s) => LuaValue::String(lua.create_string(s)?),
        Value::List(l) => {
            let t = lua.create_table_with_capacity(l.len(), 0)?;
            for (i, x) in l.iter().enumerate() {
                t.raw_set(i + 1, to_lua(lua, x)?)?;
            }
            LuaValue::Table(t)
        }
        Value::Map(m) => {
            let t = lua.create_table_with_capacity(0, m.len())?;
            for (k, x) in m {
                t.raw_set(k.as_str(), to_lua(lua, x)?)?;
            }
            LuaValue::Table(t)
        }
    })
}
