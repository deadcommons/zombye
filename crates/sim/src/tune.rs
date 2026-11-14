//! Every tuning number by name: `zombie.memory_secs`, `combat.zombie_hp.1`,
//! `population.respawn.enabled`. The Sandbox (a new game's "set anything" page, a server's
//! `sandbox` block, `assets/data/sandbox.ron`) sets them through here, so a tuning field added to
//! any of the sections below can be changed with no code.
//!
//! How: a section is serialised to a small tree ([`Node`], which keeps what RON needs: field
//! names, enum variants, options), the one value is replaced, and the tree is printed as RON and
//! read back into the section's own type. So the section's serde rules (defaults, unknown fields,
//! enums) still decide what is a value it takes, and a section with a `check` is checked after.
//!
//! A path is `section.field[.field...]`: a tuple's or list's parts by number (`alertness.0`), a
//! map's entries by their key as written (`xp_mult.Fitness`), an option's value through it
//! (`Some` is passed over). A value is written as RON writes it: `30`, `0.25`, `true`, `Off`.
//! Which paths there are: [`leaves`].

use serde::de::DeserializeOwned;
use serde::ser::{self, Serialize};

use crate::world::WorldConfig;

/// Where a section's values are kept, which decides what a save does with a change to them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// In the world's saved settings (`CONF`): a change is the world's for good.
    Saved,
    /// Content (fingerprinted): kept by the [`crate::difficulty::Sandbox`] and put on the content
    /// again on load.
    Content,
    /// A rule of the game not saved with the world: kept by the sandbox too.
    Rule,
}

/// A section of [`WorldConfig`] the sandbox can change, with a word about it.
pub struct Section {
    pub id: &'static str,
    pub name: &'static str,
    pub kept: Kept,
}

/// The sections, in the order the Sandbox page shows them.
pub const SECTIONS: &[Section] = &[
    Section { id: "zombie", name: "Zombies: senses and behaviour", kept: Kept::Saved },
    Section { id: "population", name: "Zombies: how many, where, respawn and migration", kept: Kept::Content },
    Section { id: "combat", name: "Fighting", kept: Kept::Saved },
    Section { id: "health", name: "Wounds and illness", kept: Kept::Saved },
    Section { id: "infection", name: "The infection", kept: Kept::Saved },
    Section { id: "needs", name: "Hunger, thirst and sleep", kept: Kept::Saved },
    Section { id: "movement", name: "Speeds and endurance", kept: Kept::Saved },
    Section { id: "carry", name: "Carrying", kept: Kept::Saved },
    Section { id: "skills", name: "Skills", kept: Kept::Saved },
    Section { id: "loot", name: "Loot", kept: Kept::Content },
    Section { id: "fire", name: "Fire and cooking", kept: Kept::Saved },
    Section { id: "weather", name: "Calendar and climate", kept: Kept::Saved },
    Section { id: "chop", name: "Chopping trees", kept: Kept::Saved },
    Section { id: "blinds", name: "Blinds", kept: Kept::Saved },
    Section { id: "tiers", name: "Simulation tiers", kept: Kept::Saved },
    Section { id: "player_sight", name: "How survivors see each other", kept: Kept::Rule },
    Section { id: "speech", name: "Voices", kept: Kept::Rule },
    Section { id: "sit", name: "Sitting down", kept: Kept::Rule },
    Section { id: "bonds", name: "Tying hands", kept: Kept::Rule },
    Section { id: "groups", name: "Groups", kept: Kept::Rule },
    Section { id: "explore", name: "The map", kept: Kept::Rule },
];

/// Paths the sandbox never changes: they belong to the world or the server, not a difficulty.
pub const FIXED: &[&str] = &["combat.pvp"];

/// The largest a number may be set to (as saves check settings).
pub const LIMIT: f64 = 1e6;

pub fn section(id: &str) -> Option<&'static Section> {
    SECTIONS.iter().find(|s| s.id == id)
}

/// A number or a switch the sandbox can show and change.
#[derive(Clone, Debug, PartialEq)]
pub struct Leaf {
    pub path: String,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Num(f64),
    /// A whole number (counts, ticks).
    Whole(i64),
    Bool(bool),
}

impl Value {
    /// As RON writes it, and as [`set`] takes it.
    pub fn text(&self) -> String {
        match *self {
            Value::Num(v) => float_text(v),
            Value::Whole(v) => v.to_string(),
            Value::Bool(b) => b.to_string(),
        }
    }
}

/// Every number and switch in `config`'s sections, by path, in each section's field order. Lists
/// (loot tables, the zones' lists) aren't gone into: they are data, not tuning.
pub fn leaves(config: &WorldConfig) -> Vec<Leaf> {
    let mut out = Vec::new();
    for s in SECTIONS {
        with_section(config, s.id, |n| collect(n, s.id.to_string(), &mut out));
    }
    out.retain(|l| !FIXED.contains(&l.path.as_str()));
    out
}

/// The value at `path` as RON writes it, or `None` if there is no such value.
pub fn get(config: &WorldConfig, path: &str) -> Option<String> {
    let (id, rest) = split(path)?;
    let mut found = None;
    with_section(config, id, |n| found = find(n, &rest).map(print));
    found
}

/// Set the value at `path` to `text` (RON), checking the result as the section's own data would be.
pub fn set(config: &mut WorldConfig, path: &str, text: &str) -> Result<(), String> {
    let (id, rest) = split(path).ok_or_else(|| format!("{path:?}: a path is section.field"))?;
    if section(id).is_none() {
        return Err(format!("{path:?}: no section {id:?} (there are {})", SECTIONS.iter().map(|s| s.id).collect::<Vec<_>>().join(", ")));
    }
    if FIXED.contains(&path) {
        return Err(format!("{path:?} isn't a difficulty's to change"));
    }
    if rest.is_empty() {
        return Err(format!("{path:?}: name a value in the section"));
    }
    let text = text.trim();
    if text.is_empty() || text.len() > 200 {
        return Err(format!("{path:?}: no value"));
    }
    // Numbers that can't be below 0 as shipped stay at 0 or more (the weather's temperatures may go anywhere).
    if id != "weather" {
        if let (Some(old), Ok(new)) = (get(config, path).and_then(|t| t.parse::<f64>().ok()), text.parse::<f64>()) {
            if old >= 0.0 && new < 0.0 {
                return Err(format!("{path} can't be below 0"));
            }
        }
    }
    let r = match id {
        "zombie" => checked(&mut config.zombie, &rest, text, |_| Ok(())),
        "population" => checked(&mut config.population, &rest, text, |v| v.validate()),
        "combat" => checked(&mut config.combat, &rest, text, |_| Ok(())),
        "health" => checked(&mut config.health, &rest, text, |v| v.check()),
        "infection" => checked(&mut config.infection, &rest, text, |v| v.check()),
        "needs" => checked(&mut config.needs, &rest, text, |v| v.check()),
        "movement" => checked(&mut config.movement, &rest, text, |v| v.check()),
        "carry" => checked(&mut config.carry, &rest, text, |_| Ok(())),
        "skills" => checked(&mut config.skills, &rest, text, |v| v.check()),
        "loot" => checked(&mut config.loot, &rest, text, |v| v.validate()),
        "fire" => checked(&mut config.fire, &rest, text, |v| v.check()),
        "weather" => checked(&mut config.weather, &rest, text, |v| v.check()),
        "chop" => checked(&mut config.chop, &rest, text, |v| v.check()),
        "blinds" => checked(&mut config.blinds, &rest, text, |v| v.check()),
        "tiers" => checked(&mut config.tiers, &rest, text, |_| Ok(())),
        "player_sight" => checked(&mut config.player_sight, &rest, text, |_| Ok(())),
        "speech" => checked(&mut config.speech, &rest, text, |_| Ok(())),
        "sit" => checked(&mut config.sit, &rest, text, |_| Ok(())),
        "bonds" => checked(&mut config.bonds, &rest, text, |_| Ok(())),
        "groups" => checked(&mut config.groups, &rest, text, |_| Ok(())),
        "explore" => checked(&mut config.explore, &rest, text, |_| Ok(())),
        _ => unreachable!("every section is matched"),
    };
    r.map_err(|e| format!("{path} = {text}: {e}"))
}

fn split(path: &str) -> Option<(&str, Vec<&str>)> {
    let mut parts = path.trim().split('.');
    let id = parts.next().filter(|s| !s.is_empty())?;
    Some((id, parts.collect()))
}

fn with_section(config: &WorldConfig, id: &str, f: impl FnOnce(&Node)) {
    let n = match id {
        "zombie" => tree(&config.zombie),
        "population" => tree(&config.population),
        "combat" => tree(&config.combat),
        "health" => tree(&config.health),
        "infection" => tree(&config.infection),
        "needs" => tree(&config.needs),
        "movement" => tree(&config.movement),
        "carry" => tree(&config.carry),
        "skills" => tree(&config.skills),
        "loot" => tree(&config.loot),
        "fire" => tree(&config.fire),
        "weather" => tree(&config.weather),
        "chop" => tree(&config.chop),
        "blinds" => tree(&config.blinds),
        "tiers" => tree(&config.tiers),
        "player_sight" => tree(&config.player_sight),
        "speech" => tree(&config.speech),
        "sit" => tree(&config.sit),
        "bonds" => tree(&config.bonds),
        "groups" => tree(&config.groups),
        "explore" => tree(&config.explore),
        _ => None,
    };
    if let Some(n) = n {
        f(&n);
    }
}

/// [`edit`] a copy, check it, and keep it only if it passes: a refused value changes nothing.
fn checked<T: Serialize + DeserializeOwned + Clone>(value: &mut T, path: &[&str], text: &str, check: impl Fn(&T) -> Result<(), String>) -> Result<(), String> {
    let mut copy = value.clone();
    edit(&mut copy, path, text)?;
    check(&copy)?;
    *value = copy;
    Ok(())
}

/// Replace the value at `path` in `value` with `text`, through RON.
pub fn edit<T: Serialize + DeserializeOwned>(value: &mut T, path: &[&str], text: &str) -> Result<(), String> {
    let mut n = tree(value).ok_or("can't be changed by name")?;
    let slot = find_mut(&mut n, path).ok_or("no such value")?;
    *slot = parse_like(slot, text)?;
    let ron_text = print(&n);
    *value = ron::from_str(&ron_text).map_err(|e| format!("not a value it takes ({e})"))?;
    Ok(())
}

/// What a value is written as, given what was there before.
fn parse_like(old: &Node, text: &str) -> Result<Node, String> {
    let num = || -> Result<f64, String> {
        let v: f64 = text.parse().map_err(|_| format!("{text:?} isn't a number"))?;
        if !v.is_finite() || v.abs() > LIMIT {
            return Err(format!("{text} is out of range (at most {LIMIT} either way)"));
        }
        Ok(v)
    };
    Ok(match old {
        Node::Float(_) => Node::Float(num()?),
        Node::Int(_) => {
            let v = num()?;
            if v.fract() != 0.0 {
                return Err(format!("{text} isn't a whole number"));
            }
            Node::Int(v as i128)
        }
        Node::Bool(_) => match text {
            "true" | "on" | "yes" => Node::Bool(true),
            "false" | "off" | "no" => Node::Bool(false),
            _ => return Err(format!("{text:?} isn't true or false")),
        },
        Node::Some(inner) if text != "None" => Node::Some(Box::new(parse_like(inner, text)?)),
        Node::Variant(_) if text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') => Node::Variant(text.to_string()),
        _ => Node::Raw(text.to_string()),
    })
}

fn collect(n: &Node, path: String, out: &mut Vec<Leaf>) {
    match n {
        Node::Float(v) => out.push(Leaf { path, value: Value::Num(*v) }),
        Node::Int(v) => out.push(Leaf { path, value: Value::Whole((*v).clamp(i64::MIN as i128, i64::MAX as i128) as i64) }),
        Node::Bool(b) => out.push(Leaf { path, value: Value::Bool(*b) }),
        Node::Some(inner) | Node::Newtype(inner) | Node::NewtypeVariant(_, inner) => collect(inner, path, out),
        Node::Tuple(items) | Node::TupleVariant(_, items) => {
            for (k, item) in items.iter().enumerate() {
                collect(item, format!("{path}.{k}"), out);
            }
        }
        Node::Struct(fields) | Node::StructVariant(_, fields) => {
            for (name, item) in fields {
                collect(item, format!("{path}.{name}"), out);
            }
        }
        Node::Map(entries) => {
            for (key, item) in entries {
                if let Some(k) = key_text(key) {
                    collect(item, format!("{path}.{k}"), out);
                }
            }
        }
        // Lists are data (loot tables, zones), not tuning.
        _ => {}
    }
}

fn key_text(key: &Node) -> Option<String> {
    match key {
        Node::Str(s) if !s.contains('.') => Some(s.clone()),
        Node::Variant(v) => Some(v.clone()),
        Node::Int(v) => Some(v.to_string()),
        _ => None,
    }
}

fn find<'a>(n: &'a Node, path: &[&str]) -> Option<&'a Node> {
    let Some((first, rest)) = path.split_first() else { return Some(n) };
    match n {
        Node::Some(inner) | Node::Newtype(inner) | Node::NewtypeVariant(_, inner) => find(inner, path),
        Node::Struct(fields) | Node::StructVariant(_, fields) => fields.iter().find(|(name, _)| name == first).and_then(|(_, v)| find(v, rest)),
        Node::Tuple(items) | Node::TupleVariant(_, items) | Node::Seq(items) => first.parse::<usize>().ok().and_then(|k| items.get(k)).and_then(|v| find(v, rest)),
        Node::Map(entries) => entries.iter().find(|(k, _)| key_text(k).as_deref() == Some(*first)).and_then(|(_, v)| find(v, rest)),
        _ => None,
    }
}

fn find_mut<'a>(n: &'a mut Node, path: &[&str]) -> Option<&'a mut Node> {
    let Some((first, rest)) = path.split_first() else { return Some(n) };
    match n {
        Node::Some(inner) | Node::Newtype(inner) | Node::NewtypeVariant(_, inner) => find_mut(inner, path),
        Node::Struct(fields) | Node::StructVariant(_, fields) => fields.iter_mut().find(|(name, _)| name == first).and_then(|(_, v)| find_mut(v, rest)),
        Node::Tuple(items) | Node::TupleVariant(_, items) | Node::Seq(items) => first.parse::<usize>().ok().and_then(|k| items.get_mut(k)).and_then(|v| find_mut(v, rest)),
        Node::Map(entries) => entries.iter_mut().find(|(k, _)| key_text(k).as_deref() == Some(*first)).and_then(|(_, v)| find_mut(v, rest)),
        _ => None,
    }
}

fn float_text(v: f64) -> String {
    let t = format!("{v}");
    if t.contains(['.', 'e', 'E']) || t.contains("inf") || t.contains("NaN") {
        t
    } else {
        format!("{t}.0")
    }
}

/// A value as serde describes it, with what RON needs to read it back.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    Char(char),
    Bytes(Vec<u8>),
    Unit,
    None,
    Some(Box<Node>),
    /// A unit variant, by name.
    Variant(String),
    Newtype(Box<Node>),
    NewtypeVariant(String, Box<Node>),
    Seq(Vec<Node>),
    Tuple(Vec<Node>),
    TupleVariant(String, Vec<Node>),
    Map(Vec<(Node, Node)>),
    Struct(Vec<(String, Node)>),
    StructVariant(String, Vec<(String, Node)>),
    /// RON text as given (a value [`set`] couldn't place by kind; RON reading it back checks it).
    Raw(String),
}

/// `value` as a tree, or `None` if it doesn't serialise.
pub fn tree<T: Serialize>(value: &T) -> Option<Node> {
    value.serialize(TreeSer).ok()
}

/// A tree as RON text.
pub fn print(n: &Node) -> String {
    let mut out = String::new();
    write(n, &mut out);
    out
}

fn write(n: &Node, out: &mut String) {
    let list = |items: &[Node], open: char, close: char, out: &mut String| {
        out.push(open);
        for (k, item) in items.iter().enumerate() {
            if k > 0 {
                out.push(',');
            }
            write(item, out);
        }
        out.push(close);
    };
    let fields = |fields: &[(String, Node)], out: &mut String| {
        out.push('(');
        for (k, (name, item)) in fields.iter().enumerate() {
            if k > 0 {
                out.push(',');
            }
            out.push_str(name);
            out.push(':');
            write(item, out);
        }
        out.push(')');
    };
    match n {
        Node::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Node::Int(v) => out.push_str(&v.to_string()),
        Node::Float(v) => out.push_str(&float_text(*v)),
        Node::Str(s) => out.push_str(&format!("{s:?}")),
        Node::Char(c) => out.push_str(&format!("{c:?}")),
        Node::Bytes(b) => list(&b.iter().map(|&x| Node::Int(x as i128)).collect::<Vec<_>>(), '[', ']', out),
        Node::Unit => out.push_str("()"),
        Node::None => out.push_str("None"),
        Node::Some(inner) => {
            out.push_str("Some(");
            write(inner, out);
            out.push(')');
        }
        Node::Variant(name) => out.push_str(name),
        Node::Newtype(inner) => {
            out.push('(');
            write(inner, out);
            out.push(')');
        }
        Node::NewtypeVariant(name, inner) => {
            out.push_str(name);
            out.push('(');
            write(inner, out);
            out.push(')');
        }
        Node::Seq(items) => list(items, '[', ']', out),
        Node::Tuple(items) => list(items, '(', ')', out),
        Node::TupleVariant(name, items) => {
            out.push_str(name);
            list(items, '(', ')', out);
        }
        Node::Map(entries) => {
            out.push('{');
            for (k, (key, item)) in entries.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write(key, out);
                out.push(':');
                write(item, out);
            }
            out.push('}');
        }
        Node::Struct(f) => fields(f, out),
        Node::StructVariant(name, f) => {
            out.push_str(name);
            fields(f, out);
        }
        Node::Raw(text) => out.push_str(text),
    }
}

#[derive(Debug)]
pub struct TreeError(String);

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TreeError {}

impl ser::Error for TreeError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        TreeError(msg.to_string())
    }
}

struct TreeSer;

macro_rules! int {
    ($($f:ident $t:ty),*) => { $(fn $f(self, v: $t) -> Result<Node, TreeError> { Ok(Node::Int(v as i128)) })* };
}

impl ser::Serializer for TreeSer {
    type Ok = Node;
    type Error = TreeError;
    type SerializeSeq = Items;
    type SerializeTuple = Items;
    type SerializeTupleStruct = Items;
    type SerializeTupleVariant = Items;
    type SerializeMap = Entries;
    type SerializeStruct = Fields;
    type SerializeStructVariant = Fields;

    int!(serialize_i8 i8, serialize_i16 i16, serialize_i32 i32, serialize_i64 i64, serialize_u8 u8, serialize_u16 u16, serialize_u32 u32, serialize_u64 u64);

    fn serialize_bool(self, v: bool) -> Result<Node, TreeError> {
        Ok(Node::Bool(v))
    }
    fn serialize_i128(self, v: i128) -> Result<Node, TreeError> {
        Ok(Node::Int(v))
    }
    fn serialize_u128(self, v: u128) -> Result<Node, TreeError> {
        Ok(Node::Int(v.min(i128::MAX as u128) as i128))
    }
    fn serialize_f32(self, v: f32) -> Result<Node, TreeError> {
        // Through its shortest text, so 0.1 stays 0.1 (not 0.100000001...).
        Ok(Node::Float(format!("{v}").parse().unwrap_or(v as f64)))
    }
    fn serialize_f64(self, v: f64) -> Result<Node, TreeError> {
        Ok(Node::Float(v))
    }
    fn serialize_char(self, v: char) -> Result<Node, TreeError> {
        Ok(Node::Char(v))
    }
    fn serialize_str(self, v: &str) -> Result<Node, TreeError> {
        Ok(Node::Str(v.to_string()))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Node, TreeError> {
        Ok(Node::Bytes(v.to_vec()))
    }
    fn serialize_none(self) -> Result<Node, TreeError> {
        Ok(Node::None)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<Node, TreeError> {
        Ok(Node::Some(Box::new(v.serialize(TreeSer)?)))
    }
    fn serialize_unit(self) -> Result<Node, TreeError> {
        Ok(Node::Unit)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Node, TreeError> {
        Ok(Node::Unit)
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<Node, TreeError> {
        Ok(Node::Variant(variant.to_string()))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, _: &'static str, v: &T) -> Result<Node, TreeError> {
        Ok(Node::Newtype(Box::new(v.serialize(TreeSer)?)))
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(self, _: &'static str, _: u32, variant: &'static str, v: &T) -> Result<Node, TreeError> {
        Ok(Node::NewtypeVariant(variant.to_string(), Box::new(v.serialize(TreeSer)?)))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Items, TreeError> {
        Ok(Items { items: Vec::with_capacity(len.unwrap_or(0)), kind: ItemsKind::Seq })
    }
    fn serialize_tuple(self, len: usize) -> Result<Items, TreeError> {
        Ok(Items { items: Vec::with_capacity(len), kind: ItemsKind::Tuple })
    }
    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<Items, TreeError> {
        Ok(Items { items: Vec::with_capacity(len), kind: ItemsKind::Tuple })
    }
    fn serialize_tuple_variant(self, _: &'static str, _: u32, variant: &'static str, len: usize) -> Result<Items, TreeError> {
        Ok(Items { items: Vec::with_capacity(len), kind: ItemsKind::Variant(variant.to_string()) })
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Entries, TreeError> {
        Ok(Entries { entries: Vec::new(), key: None })
    }
    fn serialize_struct(self, _: &'static str, len: usize) -> Result<Fields, TreeError> {
        Ok(Fields { fields: Vec::with_capacity(len), variant: None })
    }
    fn serialize_struct_variant(self, _: &'static str, _: u32, variant: &'static str, len: usize) -> Result<Fields, TreeError> {
        Ok(Fields { fields: Vec::with_capacity(len), variant: Some(variant.to_string()) })
    }
}

enum ItemsKind {
    Seq,
    Tuple,
    Variant(String),
}

struct Items {
    items: Vec<Node>,
    kind: ItemsKind,
}

impl Items {
    fn push<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), TreeError> {
        self.items.push(v.serialize(TreeSer)?);
        Ok(())
    }
    fn done(self) -> Node {
        match self.kind {
            ItemsKind::Seq => Node::Seq(self.items),
            ItemsKind::Tuple => Node::Tuple(self.items),
            ItemsKind::Variant(name) => Node::TupleVariant(name, self.items),
        }
    }
}

impl ser::SerializeSeq for Items {
    type Ok = Node;
    type Error = TreeError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), TreeError> {
        self.push(v)
    }
    fn end(self) -> Result<Node, TreeError> {
        Ok(self.done())
    }
}

impl ser::SerializeTuple for Items {
    type Ok = Node;
    type Error = TreeError;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), TreeError> {
        self.push(v)
    }
    fn end(self) -> Result<Node, TreeError> {
        Ok(self.done())
    }
}

impl ser::SerializeTupleStruct for Items {
    type Ok = Node;
    type Error = TreeError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), TreeError> {
        self.push(v)
    }
    fn end(self) -> Result<Node, TreeError> {
        Ok(self.done())
    }
}

impl ser::SerializeTupleVariant for Items {
    type Ok = Node;
    type Error = TreeError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), TreeError> {
        self.push(v)
    }
    fn end(self) -> Result<Node, TreeError> {
        Ok(self.done())
    }
}

struct Entries {
    entries: Vec<(Node, Node)>,
    key: Option<Node>,
}

impl ser::SerializeMap for Entries {
    type Ok = Node;
    type Error = TreeError;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, k: &T) -> Result<(), TreeError> {
        self.key = Some(k.serialize(TreeSer)?);
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), TreeError> {
        let key = self.key.take().ok_or_else(|| TreeError("a map value with no key".into()))?;
        self.entries.push((key, v.serialize(TreeSer)?));
        Ok(())
    }
    fn end(self) -> Result<Node, TreeError> {
        Ok(Node::Map(self.entries))
    }
}

struct Fields {
    fields: Vec<(String, Node)>,
    variant: Option<String>,
}

impl Fields {
    fn push<T: ?Sized + Serialize>(&mut self, name: &'static str, v: &T) -> Result<(), TreeError> {
        self.fields.push((name.to_string(), v.serialize(TreeSer)?));
        Ok(())
    }
    fn done(self) -> Node {
        match self.variant {
            None => Node::Struct(self.fields),
            Some(name) => Node::StructVariant(name, self.fields),
        }
    }
}

impl ser::SerializeStruct for Fields {
    type Ok = Node;
    type Error = TreeError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, name: &'static str, v: &T) -> Result<(), TreeError> {
        self.push(name, v)
    }
    fn skip_field(&mut self, _: &'static str) -> Result<(), TreeError> {
        Ok(())
    }
    fn end(self) -> Result<Node, TreeError> {
        Ok(self.done())
    }
}

impl ser::SerializeStructVariant for Fields {
    type Ok = Node;
    type Error = TreeError;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, name: &'static str, v: &T) -> Result<(), TreeError> {
        self.push(name, v)
    }
    fn end(self) -> Result<Node, TreeError> {
        Ok(self.done())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_reads_back_as_it_was() {
        // Printing a section and reading it back changes nothing: so an edit changes only the one value.
        let c = WorldConfig::new(3, 20, Vec::new());
        let mut d = c.clone();
        for s in SECTIONS {
            let leaves: Vec<_> = leaves(&c).into_iter().filter(|l| l.path.starts_with(&format!("{}.", s.id))).collect();
            assert!(!leaves.is_empty(), "{} has nothing to change", s.id);
            // Set its first value to what it is: the section must come back the same.
            let first = &leaves[0];
            set(&mut d, &first.path, &first.value.text()).unwrap_or_else(|e| panic!("{e}"));
        }
        assert_eq!(format!("{:?}", tree(&c.zombie)), format!("{:?}", tree(&d.zombie)));
        assert_eq!(crate::save::ContentPrint::of(&c).population, crate::save::ContentPrint::of(&d).population);
        assert_eq!(c.combat, d.combat);
        assert_eq!(c.health, d.health);
        assert_eq!(c.weather, d.weather);
        assert_eq!(c.loot, d.loot);
        let n = leaves(&c).len();
        assert!(n > 200, "only {n} values");
    }

    #[test]
    fn values_are_set_by_path_and_bad_ones_are_refused() {
        let mut c = WorldConfig::new(3, 20, Vec::new());
        set(&mut c, "zombie.memory_secs", "42").unwrap();
        assert_eq!(c.zombie.memory_secs, 42.0);
        set(&mut c, "combat.zombie_hp.1", "300").unwrap();
        assert_eq!(c.combat.zombie_hp.1, 300.0);
        set(&mut c, "population.respawn.enabled", "true").unwrap();
        assert!(c.population.respawn.enabled);
        set(&mut c, "zombie.sight_interval_ticks", "6").unwrap();
        assert_eq!(c.zombie.sight_interval_ticks, 6);
        assert_eq!(get(&c, "zombie.memory_secs").as_deref(), Some("42.0"));
        set(&mut c, "zombie.hearing", "0.1").unwrap();
        assert_eq!(get(&c, "zombie.hearing").as_deref(), Some("0.1"));
        assert_eq!(c.zombie.hearing, 0.1);
        for (path, text) in [
            ("zombie.no_such", "1"),
            ("nowhere.x", "1"),
            ("zombie", "1"),
            ("zombie.memory_secs", "lots"),
            ("zombie.memory_secs", "inf"),
            ("zombie.memory_secs", "1e9"),
            ("zombie.sight_interval_ticks", "1.5"),
            ("zombie.sight_interval_ticks", "-1"),
            ("population.respawn.enabled", "maybe"),
            ("combat.pvp", "true"),
        ] {
            let before = c.clone();
            assert!(set(&mut c, path, text).is_err(), "{path} = {text} was taken");
            assert_eq!(format!("{:?}", tree(&before.zombie)), format!("{:?}", tree(&c.zombie)));
        }
    }
}
