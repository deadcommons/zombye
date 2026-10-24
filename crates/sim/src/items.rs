//! Items in the world. Every item is an [`ItemDef`] (`assets/data/items/items.ron`, parsed with
//! [`parse_items`]); melee weapons keep their combat numbers in [`WeaponDef`]s
//! (`assets/data/items/weapons.ron`, [`parse_weapons`]) and an item swings like one when it names it
//! (`weapon: Some("baseball_bat")`). Things are carried, held and worn as [`ItemStack`]s (an item and
//! a count; see [`crate::inventory`]), lie in furniture ([`crate::containers`]) or on the floor as
//! [`GroundItem`]s. A few weapons lie on house floors from the start ([`place_weapons`]).

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::map::{Feature, TileMap};
use crate::rng::Rng;
use crate::skills::Skill;

/// What kind of weapon it is: how it hits, which decides its skill (#46) and its swing animation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WeaponCategory {
    /// Bare hands. Exactly one weapon must be this; it's what a player without a weapon uses.
    Unarmed,
    ShortBlade,
    ShortBlunt,
    LongBlunt,
    Axe,
}

impl WeaponCategory {
    /// The skill weapons of this category use unless the data says otherwise: Bludgeon for blunt
    /// ones, Edged for knives and axes, Brawl for bare hands.
    pub fn skill(self) -> Skill {
        match self {
            Self::Unarmed => Skill::Brawl,
            Self::ShortBlade | Self::Axe => Skill::Edged,
            Self::ShortBlunt | Self::LongBlunt => Skill::Bludgeon,
        }
    }
}

/// One melee weapon. Distances in metres (reach is measured to the target's body), times in
/// seconds, chances in 0..1. All numbers are ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WeaponDef {
    pub id: String,
    pub name: String,
    pub category: WeaponCategory,
    /// The skill it trains and that makes it better (#46): Bludgeon, Edged or Brawl. Left out, its
    /// category's ([`WeaponCategory::skill`]); every weapon of a category must name the same one.
    #[serde(default)]
    pub skill: Option<Skill>,
    /// Damage per hit, rolled uniformly (zombies have about 100 hit points).
    pub damage: (f32, f32),
    /// Reach from the player's centre to the edge of a zombie's body.
    pub range: f32,
    /// Width of the swing in front of the player.
    pub arc_degrees: f32,
    /// From starting the swing to the moment it lands.
    pub windup_seconds: f32,
    /// From starting the swing until the next action can start (≥ windup).
    pub swing_seconds: f32,
    /// Zombies one swing can hit, nearest first.
    pub max_targets: u32,
    /// How far a hit pushes a zombie back.
    pub knockback: f32,
    pub stagger_chance: f32,
    pub knockdown_chance: f32,
    pub crit_chance: f32,
    pub crit_multiplier: f32,
    /// Endurance (0..1) one swing costs.
    pub endurance_cost: f32,
    /// Radius of the noise a swing makes.
    pub noise_radius: f32,
    /// Relative chance of being placed in a house (0 = never).
    #[serde(default)]
    pub spawn_weight: f32,
    /// Room types (`kitchen`, `bedroom`, ...) where it's three times as likely to turn up.
    #[serde(default)]
    pub found_in: Vec<String>,
    /// Grey-box look: length in metres and colour (linear-ish RGB 0..1).
    #[serde(default = "default_length")]
    pub length: f32,
    #[serde(default = "default_color")]
    pub color: (f32, f32, f32),
}

fn default_length() -> f32 {
    0.5
}

fn default_color() -> (f32, f32, f32) {
    (0.5, 0.5, 0.5)
}

impl WeaponDef {
    /// The skill it uses (#46).
    pub fn skill(&self) -> Skill {
        self.skill.unwrap_or(self.category.skill())
    }

    /// Built-in bare hands, used when the content has no `Unarmed` weapon (tests, bare configs).
    pub fn fists() -> Self {
        Self {
            id: "fists".into(),
            name: "Bare hands".into(),
            category: WeaponCategory::Unarmed,
            skill: Some(Skill::Brawl),
            damage: (4.0, 7.0),
            range: 0.6,
            arc_degrees: 60.0,
            windup_seconds: 0.15,
            swing_seconds: 0.45,
            max_targets: 1,
            knockback: 0.15,
            stagger_chance: 0.25,
            knockdown_chance: 0.03,
            crit_chance: 0.05,
            crit_multiplier: 1.5,
            endurance_cost: 0.02,
            noise_radius: 3.0,
            spawn_weight: 0.0,
            found_in: Vec::new(),
            length: 0.0,
            color: (0.8, 0.65, 0.5),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let id = &self.id;
        let chance = |name: &str, v: f32| if (0.0..=1.0).contains(&v) { Ok(()) } else { Err(format!("{id}: {name} must be in 0..1, got {v}")) };
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return Err(format!("weapon id {id:?} must be lowercase letters, digits and _"));
        }
        let (lo, hi) = self.damage;
        if !(lo >= 0.0 && hi >= lo && hi.is_finite()) {
            return Err(format!("{id}: damage must be (min, max) with 0 <= min <= max, got ({lo}, {hi})"));
        }
        if !(self.range > 0.0 && self.range <= 4.0) {
            return Err(format!("{id}: range must be in (0, 4] m, got {}", self.range));
        }
        if !(self.arc_degrees > 0.0 && self.arc_degrees <= 360.0) {
            return Err(format!("{id}: arc_degrees must be in (0, 360], got {}", self.arc_degrees));
        }
        if !(self.windup_seconds >= 0.0 && self.swing_seconds >= self.windup_seconds && self.swing_seconds >= 0.1 && self.swing_seconds <= 5.0) {
            return Err(format!("{id}: need 0 <= windup_seconds <= swing_seconds, swing_seconds in [0.1, 5], got {} and {}", self.windup_seconds, self.swing_seconds));
        }
        if self.max_targets == 0 || self.max_targets > 8 {
            return Err(format!("{id}: max_targets must be 1..8, got {}", self.max_targets));
        }
        if !(0.0..=1.5).contains(&self.knockback) {
            return Err(format!("{id}: knockback must be in 0..1.5 m, got {}", self.knockback));
        }
        chance("stagger_chance", self.stagger_chance)?;
        chance("knockdown_chance", self.knockdown_chance)?;
        chance("crit_chance", self.crit_chance)?;
        chance("endurance_cost", self.endurance_cost)?;
        if !(self.crit_multiplier >= 1.0 && self.crit_multiplier <= 10.0) {
            return Err(format!("{id}: crit_multiplier must be in 1..10, got {}", self.crit_multiplier));
        }
        if !(0.0..=100.0).contains(&self.noise_radius) {
            return Err(format!("{id}: noise_radius must be in 0..100 m, got {}", self.noise_radius));
        }
        if !(self.spawn_weight >= 0.0 && self.spawn_weight.is_finite()) {
            return Err(format!("{id}: spawn_weight must be >= 0"));
        }
        if !self.skill().is_weapon_skill() {
            return Err(format!("{id}: skill {:?} isn't a weapon skill (Bludgeon, Edged or Brawl)", self.skill()));
        }
        if self.category == WeaponCategory::Unarmed && self.skill() != Skill::Brawl {
            return Err(format!("{id}: bare hands are Brawl, not {:?}", self.skill()));
        }
        if self.category == WeaponCategory::Unarmed && self.spawn_weight > 0.0 {
            return Err(format!("{id}: an Unarmed weapon can't lie about (spawn_weight must be 0)"));
        }
        if !(0.0..=3.0).contains(&self.length) {
            return Err(format!("{id}: length must be in 0..3 m"));
        }
        Ok(())
    }
}

/// Parse and validate a weapons file: a RON list of [`WeaponDef`]. Ids must be unique and exactly
/// one weapon must be `Unarmed`.
pub fn parse_weapons(text: &str) -> Result<Vec<WeaponDef>, String> {
    let defs: Vec<WeaponDef> = ron::from_str(text).map_err(|e| e.to_string())?;
    validate_weapons(&defs)?;
    Ok(defs)
}

pub fn validate_weapons(defs: &[WeaponDef]) -> Result<(), String> {
    for (i, d) in defs.iter().enumerate() {
        d.validate()?;
        if defs[..i].iter().any(|o| o.id == d.id) {
            return Err(format!("weapon id {:?} is used twice", d.id));
        }
        // A category hits one way, so it trains one skill.
        if let Some(o) = defs[..i].iter().find(|o| o.category == d.category && o.skill() != d.skill()) {
            return Err(format!("{}: {:?} weapons use {:?} (as {} does), not {:?}", d.id, d.category, o.skill(), o.id, d.skill()));
        }
    }
    match defs.iter().filter(|d| d.category == WeaponCategory::Unarmed).count() {
        1 => Ok(()),
        n => Err(format!("need exactly one Unarmed weapon (bare hands), found {n}")),
    }
}

/// What an item is, broadly: drives which slot it goes to and what using it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ItemCategory {
    Food,
    Drink,
    Medical,
    Tool,
    Material,
    Clothing,
    Bag,
    Weapon,
    Light,
    /// Everyday clutter: light and mostly useless.
    Junk,
}

impl ItemCategory {
    pub const ALL: [ItemCategory; 10] = [Self::Food, Self::Drink, Self::Medical, Self::Tool, Self::Material, Self::Clothing, Self::Bag, Self::Weapon, Self::Light, Self::Junk];
}

/// Something eaten or drunk (`food` / `drink`). Hunger and thirst are on a 0..1 scale (taken off
/// the need per go; negative makes it worse; see [`crate::needs`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consumable {
    #[serde(default)]
    pub hunger: f32,
    #[serde(default)]
    pub thirst: f32,
    /// How many goes one item lasts.
    #[serde(default = "one_u8")]
    pub uses: u8,
    /// Game hours until it goes off (`None`: keeps).
    #[serde(default)]
    pub spoils_hours: Option<f32>,
}

/// A medical item. What it treats is in its tags (`bandage`, `clean`, `disinfects`, ...); the
/// treatments come with #9 and #10.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MedicalDef {
    #[serde(default = "one_u8")]
    pub uses: u8,
}

/// How a bag is carried: which slot it goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BagCarry {
    Back,
    Shoulder,
    /// In a hand (either).
    Hand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BagDef {
    /// Room it adds (litres).
    pub capacity_l: f32,
    pub carried: BagCarry,
    /// Share of speed lost while carrying it (an awkward strap), 0..1.
    #[serde(default)]
    pub slow: f32,
}

/// A region of the body clothing covers: both arms at once, say. Wounds land on finer parts
/// (`crate::health::Part`, left and right apart), each in one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BodyPart {
    Head,
    Torso,
    Arms,
    Hands,
    Legs,
    Feet,
}

/// Which layer of clothing: two worn items can't share a body part on the same layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Layer {
    #[default]
    Base,
    Mid,
    Outer,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClothingDef {
    pub covers: Vec<BodyPart>,
    /// Chance (0..1) a strike on a covered part only bruises (#9); layers combine.
    #[serde(default)]
    pub protection: f32,
    #[serde(default)]
    pub layer: Layer,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightDef {
    pub radius_m: f32,
    /// Game hours of light from one charge (batteries, a candle's length).
    pub hours: f32,
}

fn one_u8() -> u8 {
    1
}

fn one_u16() -> u16 {
    1
}

fn grey() -> (f32, f32, f32) {
    (0.5, 0.5, 0.5)
}

/// One kind of item. Weights in kg (they count in full wherever the item is carried), volumes in
/// litres (room taken in pockets, bags and furniture). All numbers are ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemDef {
    pub id: String,
    pub name: String,
    pub category: ItemCategory,
    pub weight_kg: f32,
    pub volume_l: f32,
    /// How many share one slot.
    #[serde(default = "one_u16")]
    pub stack: u16,
    /// For rules that recipes and actions check (`opens_tins`, `rippable`, `fire`, ...).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Held in both hands.
    #[serde(default)]
    pub two_handed: bool,
    /// The weapon (id in `weapons.ron`) it swings as.
    #[serde(default)]
    pub weapon: Option<String>,
    #[serde(default)]
    pub food: Option<Consumable>,
    #[serde(default)]
    pub drink: Option<Consumable>,
    #[serde(default)]
    pub medical: Option<MedicalDef>,
    #[serde(default)]
    pub bag: Option<BagDef>,
    #[serde(default)]
    pub clothing: Option<ClothingDef>,
    #[serde(default)]
    pub light: Option<LightDef>,
    /// Grey-box colour (linear-ish RGB 0..1).
    #[serde(default = "grey")]
    pub color: (f32, f32, f32),
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl ItemDef {
    /// A plain item for a weapon that has none in the item data (tests, bare configs, a mod that
    /// adds a weapon but no item): 1 kg, 1.5 L.
    pub fn for_weapon(w: &WeaponDef) -> Self {
        Self {
            id: w.id.clone(),
            name: w.name.clone(),
            category: ItemCategory::Weapon,
            weight_kg: 1.0,
            volume_l: 1.5,
            stack: 1,
            tags: Vec::new(),
            two_handed: w.length >= 0.8,
            weapon: Some(w.id.clone()),
            food: None,
            drink: None,
            medical: None,
            bag: None,
            clothing: None,
            light: None,
            color: w.color,
        }
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Something to eat or drink.
    pub fn consumable(&self) -> Option<&Consumable> {
        self.food.as_ref().or(self.drink.as_ref())
    }

    /// Check one definition against the weapons it may name.
    pub fn validate(&self, weapons: &[WeaponDef]) -> Result<(), String> {
        let id = &self.id;
        if !valid_id(id) {
            return Err(format!("item id {id:?} must be lowercase letters, digits and _"));
        }
        if self.name.trim().is_empty() {
            return Err(format!("{id}: name is empty"));
        }
        if !(self.weight_kg > 0.0 && self.weight_kg <= 100.0) {
            return Err(format!("{id}: weight_kg must be in (0, 100], got {}", self.weight_kg));
        }
        if !(self.volume_l > 0.0 && self.volume_l <= 200.0) {
            return Err(format!("{id}: volume_l must be in (0, 200], got {}", self.volume_l));
        }
        if self.stack == 0 || self.stack > 100 {
            return Err(format!("{id}: stack must be 1..100, got {}", self.stack));
        }
        if let Some(t) = self.tags.iter().find(|t| !valid_id(t)) {
            return Err(format!("{id}: tag {t:?} must be lowercase letters, digits and _"));
        }
        use ItemCategory as C;
        let only = |field: &str, present: bool, cat: C| {
            if present && self.category != cat {
                Err(format!("{id}: `{field}` is only for {cat:?} items (this is {:?})", self.category))
            } else if !present && self.category == cat {
                Err(format!("{id}: a {cat:?} item needs `{field}`"))
            } else {
                Ok(())
            }
        };
        only("food", self.food.is_some(), C::Food)?;
        only("drink", self.drink.is_some(), C::Drink)?;
        only("bag", self.bag.is_some(), C::Bag)?;
        only("clothing", self.clothing.is_some(), C::Clothing)?;
        if self.medical.is_some() && self.category != C::Medical {
            return Err(format!("{id}: `medical` is only for Medical items"));
        }
        if self.light.is_some() && self.category != C::Light {
            return Err(format!("{id}: `light` is only for Light items"));
        }
        if self.category == C::Weapon && self.weapon.is_none() {
            return Err(format!("{id}: a Weapon item needs `weapon: Some(\"<id in weapons.ron>\")`"));
        }
        if let Some(w) = &self.weapon {
            match weapons.iter().find(|d| &d.id == w) {
                None => return Err(format!("{id}: weapon {w:?} isn't in weapons.ron")),
                Some(d) if d.category == WeaponCategory::Unarmed => return Err(format!("{id}: weapon {w:?} is bare hands")),
                _ => {}
            }
            if self.stack != 1 {
                return Err(format!("{id}: weapons don't stack (stack must be 1)"));
            }
        }
        if let Some(c) = self.consumable() {
            if !(-1.0..=1.0).contains(&c.hunger) || !(-1.0..=1.0).contains(&c.thirst) {
                return Err(format!("{id}: hunger and thirst must be in -1..1"));
            }
            if c.uses == 0 || c.uses > 20 {
                return Err(format!("{id}: uses must be 1..20, got {}", c.uses));
            }
            if c.spoils_hours.is_some_and(|h| !(h > 0.0 && h.is_finite())) {
                return Err(format!("{id}: spoils_hours must be positive"));
            }
        }
        if let Some(m) = &self.medical
            && (m.uses == 0 || m.uses > 50)
        {
            return Err(format!("{id}: medical uses must be 1..50, got {}", m.uses));
        }
        if let Some(b) = &self.bag {
            if !(b.capacity_l > 0.0 && b.capacity_l <= 200.0) {
                return Err(format!("{id}: bag capacity_l must be in (0, 200], got {}", b.capacity_l));
            }
            if !(0.0..1.0).contains(&b.slow) {
                return Err(format!("{id}: bag slow must be in 0..1, got {}", b.slow));
            }
        }
        if let Some(c) = &self.clothing {
            if c.covers.is_empty() {
                return Err(format!("{id}: clothing must cover at least one body part"));
            }
            if !(0.0..=1.0).contains(&c.protection) {
                return Err(format!("{id}: protection must be in 0..1, got {}", c.protection));
            }
        }
        if let Some(l) = &self.light
            && !(l.radius_m > 0.0 && l.radius_m <= 100.0 && l.hours > 0.0 && l.hours.is_finite())
        {
            return Err(format!("{id}: light radius_m must be in (0, 100] and hours positive"));
        }
        Ok(())
    }
}

/// Parse and validate an items file: a RON list of [`ItemDef`]. Ids must be unique and every
/// `weapon` must name one of `weapons`.
pub fn parse_items(text: &str, weapons: &[WeaponDef]) -> Result<Vec<ItemDef>, String> {
    let defs: Vec<ItemDef> = ron::from_str(text).map_err(|e| e.to_string())?;
    validate_items(&defs, weapons)?;
    Ok(defs)
}

pub fn validate_items(defs: &[ItemDef], weapons: &[WeaponDef]) -> Result<(), String> {
    if defs.len() > u16::MAX as usize / 2 {
        return Err(format!("{} items is too many", defs.len()));
    }
    for (i, d) in defs.iter().enumerate() {
        d.validate(weapons)?;
        if defs[..i].iter().any(|o| o.id == d.id) {
            return Err(format!("item id {:?} is used twice", d.id));
        }
    }
    Ok(())
}

/// Give every weapon (but bare hands) an item if none names it ([`ItemDef::for_weapon`]), as a
/// world does on creation. Returns, per weapon, the item that swings it (the first that names it).
pub fn complete_items(items: &mut Vec<ItemDef>, weapons: &[WeaponDef]) -> Vec<Option<u16>> {
    for w in weapons {
        if w.category == WeaponCategory::Unarmed || items.iter().any(|it| it.weapon.as_deref() == Some(w.id.as_str())) {
            continue;
        }
        let mut def = ItemDef::for_weapon(w);
        while items.iter().any(|it| it.id == def.id) {
            def.id.push_str("_weapon");
        }
        items.push(def);
    }
    weapons.iter().map(|w| items.iter().position(|it| it.weapon.as_deref() == Some(w.id.as_str())).map(|i| i as u16)).collect()
}

/// Some of one kind of item: `item` indexes `WorldConfig::items`; `count` is at least 1.
///
/// Food and drink carry two more things (#11): when it was made (`born`, game minutes from the
/// world's start, `World::game_minute`; negative for food from before it, 0 for anything that
/// doesn't spoil), which says how fresh it is ([`crate::needs::Freshness`]), and how many goes of
/// the top one have been had (`used`, below its `uses`). Stacks only merge when all three agree
/// ([`ItemStack::same_kind`]), so a stale loaf never hides among fresh ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStack {
    pub item: u16,
    pub count: u16,
    pub born: i32,
    pub used: u8,
}

impl ItemStack {
    pub fn new(item: u16, count: u16) -> Self {
        Self { item, count, born: 0, used: 0 }
    }

    pub fn one(item: u16) -> Self {
        Self::new(item, 1)
    }

    /// The same kind of thing (item, age and goes had), `n` of them.
    pub fn with_count(self, count: u16) -> Self {
        Self { count, ..self }
    }

    /// Can these share a stack: the same item, made at the same time, with the same goes had.
    pub fn same_kind(&self, o: &ItemStack) -> bool {
        self.item == o.item && self.born == o.born && self.used == o.used
    }
}

/// Items lying on the floor: any stack. The floor has unlimited room.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroundItem {
    /// Stable id, never reused within a world.
    pub id: u32,
    pub stack: ItemStack,
    pub pos: Vec2,
    /// Facing angle it lies at (radians), for the renderer.
    pub angle: f32,
}

/// Scatter weapons over house floors: 0–2 per building on free ground-floor tiles, at least one in
/// `spawn_building`, chosen by `spawn_weight` (three times as likely in a room of a `found_in`
/// type), each as the item that swings it (`weapon_items`, from [`complete_items`]). Deterministic
/// for the same map, weapons and seed; doesn't touch the world RNG.
pub fn place_weapons(map: &TileMap, weapons: &[WeaponDef], weapon_items: &[Option<u16>], spawn_building: usize, seed: u64, first_id: u32) -> Vec<GroundItem> {
    let mut out = Vec::new();
    let placeable = |wi: usize, w: &WeaponDef| w.spawn_weight > 0.0 && weapon_items.get(wi).copied().flatten().is_some();
    if !weapons.iter().enumerate().any(|(wi, w)| placeable(wi, w)) {
        return out;
    }
    let mut rng = Rng::new(seed ^ 0x17E5_CA77_E2ED);
    let mut tiles: Vec<IVec2> = Vec::new();
    for (bi, b) in map.buildings.iter().enumerate() {
        tiles.clear();
        for y in b.min.y..b.min.y + b.size.y {
            for x in b.min.x..b.min.x + b.size.x {
                let t = IVec2::new(x, y);
                let tile = map.tile(t);
                if tile.room != 0 && map.rooms[tile.room as usize].building as usize == bi && tile.feature == Feature::None {
                    tiles.push(t);
                }
            }
        }
        let mut count = rng.below(3);
        if bi == spawn_building {
            count = count.max(1);
        }
        for _ in 0..count.min(tiles.len() as u32) {
            let k = rng.below(tiles.len() as u32) as usize;
            let t = tiles.swap_remove(k);
            let room_type = &map.rooms[map.tile(t).room as usize].room_type;
            let weight = |wi: usize, w: &WeaponDef| if placeable(wi, w) { w.spawn_weight * if w.found_in.iter().any(|r| r == room_type) { 3.0 } else { 1.0 } } else { 0.0 };
            let total: f32 = weapons.iter().enumerate().map(|(wi, w)| weight(wi, w)).sum();
            let mut roll = rng.f32() * total;
            let mut pick = None;
            for (wi, w) in weapons.iter().enumerate() {
                let wt = weight(wi, w);
                if wt <= 0.0 {
                    continue;
                }
                pick = Some(wi);
                if roll < wt {
                    break;
                }
                roll -= wt;
            }
            let Some(item) = pick.and_then(|wi| weapon_items[wi]) else { continue };
            let pos = t.as_vec2() + Vec2::new(rng.range(0.3, 0.7), rng.range(0.3, 0.7));
            out.push(GroundItem { id: first_id + out.len() as u32, stack: ItemStack::one(item), pos, angle: rng.range(0.0, std::f32::consts::TAU) });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weapons() -> Vec<WeaponDef> {
        parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap()
    }

    #[test]
    fn weapons_file_is_valid() {
        let w = weapons();
        assert!(w.len() >= 9, "only {} weapons", w.len());
        assert_eq!(w.iter().filter(|d| d.category == WeaponCategory::Unarmed).count(), 1);
        assert!(w.iter().filter(|d| d.spawn_weight > 0.0).count() >= 8);
        // Every weapon names its skill (#46), and it is the one its category hits with.
        for d in &w {
            assert_eq!(d.skill, Some(d.category.skill()), "{} names {:?}", d.id, d.skill);
        }
    }

    #[test]
    fn validation_explains_mistakes() {
        let mut w = weapons();
        w[1].damage = (10.0, 5.0);
        assert!(validate_weapons(&w).unwrap_err().contains("damage"));
        let mut w = weapons();
        w[2].id = w[1].id.clone();
        assert!(validate_weapons(&w).unwrap_err().contains("twice"));
        let mut w = weapons();
        w[1].stagger_chance = 1.5;
        assert!(validate_weapons(&w).unwrap_err().contains("stagger_chance"));
        let w: Vec<WeaponDef> = weapons().into_iter().filter(|d| d.category != WeaponCategory::Unarmed).collect();
        assert!(validate_weapons(&w).unwrap_err().contains("Unarmed"));
        assert!(parse_weapons("[(id: \"x\")]").is_err());
        // Skills (#46): only weapon skills, bare hands are Brawl, one skill per category.
        let mut w = weapons();
        w[1].skill = Some(Skill::Stealth);
        assert!(validate_weapons(&w).unwrap_err().contains("isn't a weapon skill"));
        let mut w = weapons();
        w[0].skill = Some(Skill::Edged);
        assert!(validate_weapons(&w).unwrap_err().contains("bare hands are Brawl"));
        let mut w = weapons();
        let k = w.iter().rposition(|d| d.category == WeaponCategory::LongBlunt).unwrap();
        w[k].skill = Some(Skill::Edged);
        assert!(validate_weapons(&w).unwrap_err().contains("LongBlunt weapons use Bludgeon"));
        // Left out, a weapon uses its category's.
        let mut w = weapons();
        w.iter_mut().for_each(|d| d.skill = None);
        validate_weapons(&w).unwrap();
        assert_eq!(w.iter().find(|d| d.category == WeaponCategory::Axe).unwrap().skill(), Skill::Edged);
    }

    fn items() -> Vec<ItemDef> {
        parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons()).unwrap()
    }

    #[test]
    fn items_file_is_valid_and_covers_every_weapon() {
        let w = weapons();
        let mut it = items();
        assert!(it.len() >= 50, "only {} items", it.len());
        for cat in ItemCategory::ALL {
            assert!(it.iter().any(|d| d.category == cat), "no {cat:?} item");
        }
        let before = it.len();
        let map = complete_items(&mut it, &w);
        assert_eq!(it.len(), before, "items.ron should have an item for every weapon");
        for (wi, d) in w.iter().enumerate() {
            assert_eq!(map[wi].is_some(), d.category != WeaponCategory::Unarmed, "{}", d.id);
        }
        assert!(it.iter().any(|d| d.bag.as_ref().is_some_and(|b| b.carried == BagCarry::Back)), "a backpack");
    }

    #[test]
    fn item_validation_explains_mistakes() {
        let w = weapons();
        let bad = |edit: fn(&mut Vec<ItemDef>), want: &str| {
            let mut it = items();
            edit(&mut it);
            let err = validate_items(&it, &w).unwrap_err();
            assert!(err.contains(want), "{err:?} should mention {want:?}");
        };
        bad(|it| it[0].weight_kg = 0.0, "weight_kg");
        bad(|it| it[1].id = it[0].id.clone(), "twice");
        bad(|it| it[0].food = None, "needs `food`");
        bad(|it| it.iter_mut().find(|d| d.weapon.is_some()).unwrap().weapon = Some("lightsaber".into()), "isn't in weapons.ron");
        bad(|it| it.iter_mut().find(|d| d.bag.is_some()).unwrap().bag.as_mut().unwrap().capacity_l = -1.0, "capacity_l");
        bad(|it| it.iter_mut().find(|d| d.clothing.is_some()).unwrap().clothing.as_mut().unwrap().covers.clear(), "body part");
        bad(|it| it[0].stack = 0, "stack");
        assert!(parse_items("[(id: \"x\", name: \"X\", category: Junk, weight_kg: 1.0, volume_l: 1.0, colour: (1.0, 1.0, 1.0))]", &w).unwrap_err().contains("colour"), "unknown fields are refused");
        // A bare config: every weapon gets a plain item.
        let mut none = Vec::new();
        let map = complete_items(&mut none, &w);
        assert_eq!(none.len(), w.len() - 1);
        assert!(map.iter().flatten().all(|&i| none[i as usize].weapon.is_some()));
    }
}
