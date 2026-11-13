//! Difficulty presets and a new game's world options.
//!
//! A preset is data (`assets/data/difficulty.ron`, a list of [`DifficultyDef`]): a name, a line
//! about it and a handful of numbers, each a multiplier on (or a choice over) something the world's
//! settings already hold, so "Hard" is nothing the sim knows about: it is more zombies, sharper
//! ones, less in the cupboards. A preset can also set any tuning number outright (`tuning`). A
//! modder adds a preset by adding an entry. Names and numbers are ours.
//!
//! A new game's choice is a [`Setup`]: a preset's id, and the few options the new-game screen
//! lets the player change on top of it. [`DifficultyData::apply`] puts both onto a
//! [`WorldConfig`] before the world is made.
//!
//! **What a save keeps.** Most of it lands in tunings the save stores anyway (`CONF`). The
//! numbers that multiply *content* (the population file's scale, share of fast zombies and
//! respawn; the loot file's scales) are a [`Sandbox`], kept in the config and saved with the
//! settings, and put onto the content again when the save is loaded, so the content's
//! fingerprint is the same and the game goes on with the difficulty it began with. Putting a
//! sandbox on is exact and can be done twice: it remembers the file's own values.

use serde::{Deserialize, Serialize};

use crate::health::InfectionMode;
use crate::world::WorldConfig;

/// A multiplier a preset or an option may hold.
pub const RANGE: (f32, f32) = (0.05, 10.0);

/// What becomes of someone a zombie's wound infects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Infection {
    /// Nobody catches it.
    Off,
    /// It burns out after the fever.
    Survivable,
    /// It ends in death, and the body rises.
    Deadly,
}

impl Infection {
    pub const ALL: [Infection; 3] = [Infection::Off, Infection::Survivable, Infection::Deadly];

    pub fn name(self) -> &'static str {
        match self {
            Infection::Off => "Off",
            Infection::Survivable => "Survivable",
            Infection::Deadly => "Deadly",
        }
    }

    /// As a config has it.
    pub fn of(config: &WorldConfig) -> Self {
        match (&config.infection.mode, config.infection.fatal) {
            (InfectionMode::Off, _) => Infection::Off,
            (_, false) => Infection::Survivable,
            (_, true) => Infection::Deadly,
        }
    }
}

/// The numbers that multiply content (see the module docs). The default changes nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sandbox {
    /// The preset it came from (for showing; "" none).
    pub difficulty: String,
    /// Multiplies the population's densities.
    pub zombies: f32,
    /// The share of fast zombies, in place of the population file's.
    pub fast_share: Option<f32>,
    /// Whether far regions fill up again, in place of the population file's.
    pub respawn: Option<bool>,
    /// Multiplies how much containers hold, and how often rare things turn up.
    pub loot: f32,
    pub rare_loot: f32,
    /// The content's own values, from when this was put on (not saved: a load puts it on fresh
    /// content).
    #[serde(skip)]
    base: Option<Base>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Base {
    scale: f32,
    fast_share: f32,
    respawn: bool,
    loot: f32,
    rare_loot: f32,
}

impl Default for Sandbox {
    fn default() -> Self {
        Self { difficulty: String::new(), zombies: 1.0, fast_share: None, respawn: None, loot: 1.0, rare_loot: 1.0, base: None }
    }
}

impl Sandbox {
    /// Numbers a sandbox may hold.
    pub fn check(&self) -> Result<(), String> {
        for (what, v) in [("zombies", self.zombies), ("loot", self.loot), ("rare_loot", self.rare_loot)] {
            if !v.is_finite() || v < 0.0 || v > RANGE.1 {
                return Err(format!("sandbox {what} {v} (0 to {})", RANGE.1));
            }
        }
        if self.fast_share.is_some_and(|f| !f.is_finite() || !(0.0..=1.0).contains(&f)) || self.difficulty.len() > 64 {
            return Err("sandbox fast_share or difficulty out of range".into());
        }
        Ok(())
    }

    /// Put this onto `config`'s content in place of whatever sandbox it had on.
    pub fn apply(&self, config: &mut WorldConfig) {
        let (p, l) = (&mut config.population, &mut config.loot);
        let base = config.sandbox.base.unwrap_or(Base { scale: p.scale, fast_share: p.fast_share, respawn: p.respawn.enabled, loot: l.scale, rare_loot: l.rare_scale });
        p.scale = base.scale * self.zombies;
        p.fast_share = self.fast_share.unwrap_or(base.fast_share);
        p.respawn.enabled = self.respawn.unwrap_or(base.respawn);
        l.scale = (base.loot * self.loot).min(10.0);
        l.rare_scale = (base.rare_loot * self.rare_loot).min(100.0);
        config.sandbox = Sandbox { base: Some(base), ..self.clone() };
    }
}

/// Tuning numbers a preset sets outright. Each is the whole tuning, with what the preset leaves
/// out at its default: `zombie: Some((memory_secs: 30))`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tunings {
    pub zombie: Option<crate::ai::ZombieTuning>,
    pub combat: Option<crate::combat::CombatTuning>,
    pub health: Option<crate::health::HealthTuning>,
    pub needs: Option<crate::needs::NeedsTuning>,
    pub carry: Option<crate::inventory::CarryTuning>,
    /// Speeds and endurance (`crate::players::MoveTuning`).
    pub movement: Option<crate::players::MoveTuning>,
}

/// One preset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DifficultyDef {
    /// Lower-case letters, digits and `_`.
    pub id: String,
    pub name: String,
    /// A line for the new-game screen.
    pub about: String,
    /// How many zombies (a multiplier on the population).
    pub zombies: f32,
    /// The share of them that are fast (`None`: the population file's).
    pub fast_zombies: Option<f32>,
    /// How sharp their eyes and ears are (a multiplier on their alertness).
    pub zombie_senses: f32,
    /// How much it takes to put one down (a multiplier on their hit points).
    pub zombie_toughness: f32,
    /// How often their strikes land.
    pub zombie_accuracy: f32,
    /// Do far regions fill up again (`None`: as the population file says)?
    pub respawn: Option<bool>,
    /// How much there is to find, and how often the rare things.
    pub loot: f32,
    pub rare_loot: f32,
    /// How fast skills grow.
    pub xp: f32,
    /// How fast hunger, thirst and tiredness come.
    pub needs: f32,
    pub infection: Infection,
    /// Real minutes a game day lasts (`None`: the game's own).
    pub day_minutes: Option<f32>,
    /// Any other tuning number, set outright (before the multipliers above).
    pub tuning: Tunings,
}

impl Default for DifficultyDef {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            about: String::new(),
            zombies: 1.0,
            fast_zombies: None,
            zombie_senses: 1.0,
            zombie_toughness: 1.0,
            zombie_accuracy: 1.0,
            respawn: None,
            loot: 1.0,
            rare_loot: 1.0,
            xp: 1.0,
            needs: 1.0,
            infection: Infection::Deadly,
            day_minutes: None,
            tuning: Tunings::default(),
        }
    }
}

impl DifficultyDef {
    fn check(&self) -> Result<(), String> {
        let at = format!("difficulty {:?}", self.id);
        if self.id.is_empty() || self.id.len() > 40 || !self.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
            return Err(format!("{at}: an id is lower-case letters, digits and _"));
        }
        if self.name.trim().is_empty() || self.about.trim().is_empty() {
            return Err(format!("{at}: needs a name and a line about it"));
        }
        for (what, v) in [("zombies", self.zombies), ("zombie_senses", self.zombie_senses), ("zombie_toughness", self.zombie_toughness), ("zombie_accuracy", self.zombie_accuracy), ("loot", self.loot), ("rare_loot", self.rare_loot), ("xp", self.xp), ("needs", self.needs)] {
            if !v.is_finite() || v < RANGE.0 || v > RANGE.1 {
                return Err(format!("{at}: {what} must be {} to {}, not {v}", RANGE.0, RANGE.1));
            }
        }
        if self.fast_zombies.is_some_and(|f| !f.is_finite() || !(0.0..=1.0).contains(&f)) {
            return Err(format!("{at}: fast_zombies is a share, 0 to 1"));
        }
        if self.day_minutes.is_some_and(|m| !m.is_finite() || !(5.0..=1440.0).contains(&m)) {
            return Err(format!("{at}: day_minutes must be 5 to 1440"));
        }
        if let Some(h) = &self.tuning.health {
            h.check().map_err(|e| format!("{at}: {e}"))?;
        }
        if let Some(n) = &self.tuning.needs {
            n.check().map_err(|e| format!("{at}: {e}"))?;
        }
        Ok(())
    }

    /// Put this preset onto a config that has none on yet (fresh content and settings).
    pub fn apply(&self, config: &mut WorldConfig) {
        let t = &self.tuning;
        if let Some(z) = &t.zombie {
            config.zombie = z.clone();
        }
        if let Some(c) = &t.combat {
            // Player against player is the world's own rule, not a difficulty's.
            config.combat = crate::combat::CombatTuning { pvp: config.combat.pvp, ..c.clone() };
        }
        if let Some(h) = &t.health {
            config.health = h.clone();
        }
        if let Some(n) = &t.needs {
            config.needs = *n;
        }
        if let Some(c) = &t.carry {
            config.carry = c.clone();
        }
        if let Some(m) = &t.movement {
            config.movement = *m;
        }
        config.zombie.alertness = (config.zombie.alertness.0 * self.zombie_senses, config.zombie.alertness.1 * self.zombie_senses);
        config.combat.zombie_hp = (config.combat.zombie_hp.0 * self.zombie_toughness, config.combat.zombie_hp.1 * self.zombie_toughness);
        config.combat.zombie_hit_chance = (config.combat.zombie_hit_chance * self.zombie_accuracy).min(1.0);
        config.skills.xp_scale = (config.skills.xp_scale * self.xp).min(100.0);
        let n = &mut config.needs;
        for rate in [&mut n.hunger_per_h, &mut n.hunger_asleep_per_h, &mut n.thirst_per_h, &mut n.thirst_asleep_per_h, &mut n.tired_per_h] {
            *rate *= self.needs;
        }
        match self.infection {
            Infection::Off => config.infection.mode = InfectionMode::Off,
            Infection::Survivable => config.infection.fatal = false,
            Infection::Deadly => config.infection.fatal = true,
        }
        if let Some(m) = self.day_minutes {
            config.day_length_secs = m * 60.0;
        }
        Sandbox { difficulty: self.id.clone(), zombies: self.zombies, fast_share: self.fast_zombies, respawn: self.respawn, loot: self.loot, rare_loot: self.rare_loot, base: None }.apply(config);
    }
}

/// A new game's world options: a preset, and what the player changed on top of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Setup {
    /// A preset's id ("": the first).
    pub difficulty: String,
    pub zombies: Option<f32>,
    pub loot: Option<f32>,
    pub xp: Option<f32>,
    pub respawn: Option<bool>,
    pub infection: Option<Infection>,
    pub day_minutes: Option<f32>,
    /// The hour the survivor wakes at.
    pub start_hour: Option<f32>,
    /// The month the world starts in (1-12; `weather.ron`'s `start_month` otherwise), on its first day
    /// unless that is the shipped start month.
    pub start_month: Option<u8>,
    /// How harsh the climate is (`weather.ron`'s `harshness`: 0.6 mild, 1 normal, 1.3 harsh).
    pub climate: Option<f32>,
}

impl Setup {
    /// Did the player change anything on top of the preset?
    pub fn custom(&self) -> bool {
        *self != Setup { difficulty: self.difficulty.clone(), ..Default::default() }
    }
}

/// `difficulty.ron`, checked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DifficultyData {
    pub presets: Vec<DifficultyDef>,
}

/// Read `difficulty.ron`: a list of presets, at least one, ids their own. Errors name the preset.
pub fn parse_difficulty(text: &str) -> Result<DifficultyData, String> {
    let presets: Vec<DifficultyDef> = ron::from_str(text).map_err(|e| e.to_string())?;
    if presets.is_empty() {
        return Err("no presets".into());
    }
    for (k, p) in presets.iter().enumerate() {
        p.check()?;
        if presets[..k].iter().any(|o| o.id == p.id) {
            return Err(format!("difficulty {:?}: listed twice", p.id));
        }
    }
    Ok(DifficultyData { presets })
}

impl DifficultyData {
    /// The game as it is with no file: one preset that changes nothing.
    pub fn builtin() -> Self {
        Self { presets: vec![DifficultyDef { id: "survivor".into(), name: "Survivor".into(), about: "The game as it is meant to be played.".into(), ..Default::default() }] }
    }

    pub fn get(&self, id: &str) -> Option<&DifficultyDef> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// The preset a new game starts on: the one that changes nothing if there is one, else the
    /// first.
    pub fn default_id(&self) -> &str {
        let plain = DifficultyDef::default();
        let same = |p: &&DifficultyDef| DifficultyDef { id: String::new(), name: String::new(), about: String::new(), ..(*p).clone() } == plain;
        self.presets.iter().find(same).or(self.presets.first()).map_or("", |p| &p.id)
    }

    /// The preset `setup` names with its options over it.
    pub fn resolve(&self, setup: &Setup) -> Result<DifficultyDef, String> {
        let id = if setup.difficulty.is_empty() { self.default_id() } else { &setup.difficulty };
        let mut d = self.get(id).ok_or_else(|| format!("no difficulty {id:?}"))?.clone();
        d.zombies = setup.zombies.unwrap_or(d.zombies);
        d.loot = setup.loot.unwrap_or(d.loot);
        d.xp = setup.xp.unwrap_or(d.xp);
        d.respawn = setup.respawn.or(d.respawn);
        d.infection = setup.infection.unwrap_or(d.infection);
        d.day_minutes = setup.day_minutes.or(d.day_minutes);
        d.check()?;
        if setup.start_hour.is_some_and(|h| !h.is_finite() || !(0.0..24.0).contains(&h)) {
            return Err("the start hour must be 0 to 24".into());
        }
        if setup.start_month.is_some_and(|m| !(1..=12).contains(&m)) {
            return Err("the start month must be 1 to 12".into());
        }
        if setup.climate.is_some_and(|c| !c.is_finite() || !(0.0..=3.0).contains(&c)) {
            return Err("the climate's harshness must be 0 to 3".into());
        }
        Ok(d)
    }

    /// Put `setup` onto a fresh config, before the world is made.
    pub fn apply(&self, setup: &Setup, config: &mut WorldConfig) -> Result<(), String> {
        let d = self.resolve(setup)?;
        d.apply(config);
        if let Some(h) = setup.start_hour {
            config.start_hour = h;
        }
        if let Some(m) = setup.start_month.filter(|&m| m != config.weather.start_month) {
            config.weather.start_month = m;
            config.weather.start_day = 1;
        }
        if let Some(c) = setup.climate {
            config.weather.harshness = c;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;

    fn shipped() -> DifficultyData {
        parse_difficulty(include_str!("../../../assets/data/difficulty.ron")).expect("difficulty.ron")
    }

    #[test]
    fn the_shipped_presets_are_sound_and_one_changes_nothing() {
        let data = shipped();
        assert!(data.presets.len() >= 3);
        let plain = data.get(data.default_id()).unwrap();
        let (mut a, b) = (WorldConfig::new(1, 0, Vec::new()), WorldConfig::new(1, 0, Vec::new()));
        plain.apply(&mut a);
        assert_eq!((a.population.clone(), a.loot.scale, a.zombie.clone(), a.combat.clone(), a.needs, a.skills.clone()), (b.population.clone(), b.loot.scale, b.zombie.clone(), b.combat.clone(), b.needs, b.skills.clone()));
        assert_eq!((a.day_length_secs, a.infection.clone()), (b.day_length_secs, b.infection.clone()));
        // Harder presets have more zombies and less to find than easier ones, in the file's order.
        for w in data.presets.windows(2) {
            assert!(w[0].zombies <= w[1].zombies && w[0].loot >= w[1].loot, "{} then {}", w[0].id, w[1].id);
        }
    }

    #[test]
    fn a_setup_goes_over_its_preset_and_bad_ones_are_refused() {
        let data = shipped();
        let hard = data.presets.last().unwrap();
        let mut config = WorldConfig::new(1, 0, Vec::new());
        let setup = Setup { difficulty: hard.id.clone(), loot: Some(2.0), infection: Some(Infection::Off), start_hour: Some(7.0), day_minutes: Some(30.0), ..Default::default() };
        assert!(setup.custom());
        data.apply(&setup, &mut config).unwrap();
        assert_eq!(config.loot.scale, 2.0);
        assert_eq!(config.population.scale, hard.zombies);
        assert_eq!(config.infection.mode, InfectionMode::Off);
        assert_eq!((config.start_hour, config.day_length_secs), (7.0, 1800.0));
        assert_eq!(config.sandbox.difficulty, hard.id);
        let mut winter = WorldConfig::new(1, 0, Vec::new());
        data.apply(&Setup { start_month: Some(12), climate: Some(1.3), ..Default::default() }, &mut winter).unwrap();
        assert_eq!((winter.weather.start_month, winter.weather.start_day, winter.weather.harshness), (12, 1, 1.3));
        assert!(data.apply(&Setup { start_month: Some(13), ..Default::default() }, &mut winter).is_err());
        assert!(data.apply(&Setup { climate: Some(-1.0), ..Default::default() }, &mut winter).is_err());
        assert!(data.apply(&Setup { difficulty: "no_such".into(), ..Default::default() }, &mut config).is_err());
        assert!(data.apply(&Setup { zombies: Some(1000.0), ..Default::default() }, &mut config).is_err());
        assert!(parse_difficulty("[(id: \"a\", name: \"A\", about: \"x\", gore: 2)]").is_err(), "unknown fields are refused");
        assert!(parse_difficulty("[(id: \"a\", name: \"A\", about: \"x\"), (id: \"a\", name: \"B\", about: \"x\")]").is_err());
        // A preset may set a tuning number outright.
        let d = parse_difficulty("[(id: \"a\", name: \"A\", about: \"x\", tuning: (zombie: Some((memory_secs: 99))))]").unwrap();
        let mut c = WorldConfig::new(1, 0, Vec::new());
        d.presets[0].apply(&mut c);
        assert_eq!(c.zombie.memory_secs, 99.0);
    }

    #[test]
    fn a_sandbox_goes_on_twice_the_same_and_a_save_keeps_it() {
        let mut config = WorldConfig::new(5, 40, Vec::new());
        let plain = config.clone();
        let s = Sandbox { difficulty: "x".into(), zombies: 1.7, loot: 0.3, rare_loot: 0.5, respawn: Some(true), fast_share: Some(0.5), base: None };
        s.apply(&mut config);
        let once = (config.population.clone(), config.loot.scale, config.loot.rare_scale);
        s.apply(&mut config);
        assert_eq!(once, (config.population.clone(), config.loot.scale, config.loot.rare_scale));
        assert!(config.population.respawn.enabled && config.population.fast_share == 0.5);
        // Saved, and loaded against the content as it ships: the sandbox is on again, and nothing
        // is said to have changed.
        let world = World::new(config);
        let loaded = World::load_with(&world.save(), &plain, Default::default()).expect("loads");
        assert!(loaded.notes.is_empty(), "{:?}", loaded.notes);
        assert_eq!(loaded.world.config.loot.scale, 0.3);
        assert_eq!(loaded.world.config.population.scale, world.config.population.scale);
        assert!(loaded.world.config.population.respawn.enabled);
        assert_eq!(loaded.world.config.sandbox.difficulty, "x");
        assert_eq!(loaded.world.state_hash(), world.state_hash());
        // And against the running game's own config (a quickload): the same.
        let again = World::load_with(&world.save(), &world.config, Default::default()).expect("loads");
        assert!(again.notes.is_empty() && again.world.config.loot.scale == 0.3);
    }
}
