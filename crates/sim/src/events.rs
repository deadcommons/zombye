//! World events (`assets/data/events.ron`): one-off things that happen to the whole town on a
//! day and change it for good, such as the water or the power going off.
//!
//! Each event ([`EventDef`]) names a range of game days, a chance of happening at all and an
//! effect from a fixed list the simulation understands ([`EventEffect`]); when it happens every
//! survivor gets its `message`, and optionally a warning some hours before (`warn`). Which events
//! happen and when is worked out from the world's loot seed ([`crate::WorldConfig::loot_seed`]: a
//! new game, a new schedule) and the event's id, never stored: [`EventsDef::when`]. What has
//! happened is the world's ([`EventState`]: the events fired and warned, the water cut off),
//! saved in the optional `EVNT` section and in the state hash only while there is any.
//!
//! The file is saved with the settings, so a world keeps its schedule. MCP `events` lists it,
//! `trigger_event` fires one now (the game's debug tools, and the admin endpoint of a hosted
//! server). Weather is not an event: it follows from the clock (`crate::weather`).

use serde::{Deserialize, Serialize};

use crate::world::World;

/// What an event does. A new kind of event is a variant here and its line in
/// [`World::apply_event`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventEffect {
    /// Only the message.
    Notice,
    /// The town's water stops: no tap runs (toilet cisterns still hold a few fills).
    WaterOff,
    /// The water comes back (an admin's undo).
    WaterOn,
    /// The grid fails: every building on it goes dark.
    PowerOff,
    /// The grid comes back.
    PowerOn,
}

/// One event in `events.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventDef {
    /// Unique, what MCP and saves call it.
    pub id: String,
    /// A short name for lists.
    pub name: String,
    /// What every survivor is told when it happens ("" for nothing).
    #[serde(default)]
    pub message: String,
    /// Game days after the world began, from and to: it happens at a time in between, the same
    /// for the same world.
    pub days: (f32, f32),
    /// The chance it happens at all in a world (1: always).
    #[serde(default = "one")]
    pub chance: f32,
    pub effect: EventEffect,
    /// Game hours before it a warning is given, and the warning's words.
    #[serde(default)]
    pub warn: Option<(f32, String)>,
}

fn one() -> f32 {
    1.0
}

/// `assets/data/events.ron`. No file: no events.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EventsDef {
    pub events: Vec<EventDef>,
}

/// At most this many events in a file.
pub const MAX_EVENTS: usize = 64;

fn id_hash(id: &str) -> u64 {
    id.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01B3))
}

impl EventsDef {
    /// Every value in range; errors name the event.
    pub fn check(&self) -> Result<(), String> {
        if self.events.len() > MAX_EVENTS {
            return Err(format!("at most {MAX_EVENTS} events"));
        }
        for (i, e) in self.events.iter().enumerate() {
            let at = if e.id.is_empty() { format!("event {}", i + 1) } else { format!("event {:?}", e.id) };
            if e.id.is_empty() || e.id.len() > 64 || !e.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(format!("{at}: id must be 1-64 letters, digits or _"));
            }
            if self.events[..i].iter().any(|o| o.id == e.id) {
                return Err(format!("{at}: listed twice"));
            }
            if e.name.trim().is_empty() || e.name.len() > 80 {
                return Err(format!("{at}: name must be 1-80 characters"));
            }
            if e.message.len() > 200 {
                return Err(format!("{at}: message over 200 characters"));
            }
            let (a, b) = e.days;
            if !(a.is_finite() && b.is_finite() && 0.0 <= a && a <= b && b <= 3650.0) {
                return Err(format!("{at}: days must be (from, to) with 0 <= from <= to <= 3650"));
            }
            if !(0.0..=1.0).contains(&e.chance) {
                return Err(format!("{at}: chance must be 0 to 1"));
            }
            if let Some((h, text)) = &e.warn {
                if !(h.is_finite() && *h > 0.0 && *h <= 240.0) {
                    return Err(format!("{at}: warn hours must be over 0 and at most 240"));
                }
                if text.trim().is_empty() || text.len() > 200 {
                    return Err(format!("{at}: warning must be 1-200 characters"));
                }
            }
        }
        Ok(())
    }

    /// When event `i` happens in a world of this loot seed, in game hours after the world began;
    /// None: not in this world (its chance didn't come up).
    pub fn when(&self, seed: u64, i: usize) -> Option<f64> {
        let e = self.events.get(i)?;
        let h = id_hash(&e.id);
        if crate::rng::hash01(seed, 0x0E7E_4E75, h, 0) >= e.chance {
            return None;
        }
        let u = crate::rng::hash01(seed, 0x0E7E_4E75, h, 1);
        Some((e.days.0 as f64 + (e.days.1 - e.days.0) as f64 * u as f64) * 24.0)
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.events.iter().position(|e| e.id == id)
    }
}

/// Read and check `events.ron`.
pub fn parse_events(text: &str) -> Result<EventsDef, String> {
    let def: EventsDef = ron::from_str(text).map_err(|e| e.to_string())?;
    def.check()?;
    Ok(def)
}

/// What has happened: saved (the optional `EVNT` section), in the hash while there is any.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventState {
    /// Events that happened (by id), with the game hour they did, in order.
    pub fired: Vec<(String, f64)>,
    /// Events whose warning has been given.
    pub warned: Vec<String>,
    /// The water cut off by an event (`EventEffect::WaterOff`).
    pub water_off: bool,
    /// Events moved by hand (MCP `schedule_event`): the game hour now due, or None: called off.
    pub moved: Vec<(String, Option<f64>)>,
    /// Fills taken from toilet cisterns since the water went off, by the toilet's furniture index
    /// (`crate::power::cistern_near`).
    pub cisterns: std::collections::BTreeMap<u32, u8>,
}

impl EventState {
    pub fn is_empty(&self) -> bool {
        self.fired.is_empty() && self.warned.is_empty() && !self.water_off && self.moved.is_empty() && self.cisterns.is_empty()
    }

    pub fn has_fired(&self, id: &str) -> bool {
        self.fired.iter().any(|(f, _)| f == id)
    }

    pub(crate) fn hash(&self, mix: &mut impl FnMut(u32)) {
        if self.is_empty() {
            return;
        }
        mix(0x4556_4E54 ^ self.water_off as u32);
        for (id, h) in &self.fired {
            let k = id_hash(id);
            mix(k as u32);
            mix((k >> 32) as u32);
            mix((*h * 60.0) as i64 as u32);
        }
        for id in &self.warned {
            mix(id_hash(id) as u32 ^ 0x5741_524E);
        }
        for (id, h) in &self.moved {
            mix(id_hash(id) as u32 ^ 0x4D4F_5645);
            mix(h.map_or(u32::MAX, |h| (h * 60.0) as i64 as u32));
        }
        for (k, n) in &self.cisterns {
            mix(0x4349_0000 ^ *k);
            mix(*n as u32);
        }
    }

    /// A loaded state: finite hours, ids that look like ids.
    pub fn check(&self) -> Result<(), String> {
        let ok = |id: &str| !id.is_empty() && id.len() <= 64;
        if self.fired.iter().any(|(id, h)| !ok(id) || !h.is_finite() || *h < 0.0) || self.warned.iter().any(|id| !ok(id)) || self.moved.iter().any(|(id, h)| !ok(id) || h.is_some_and(|h| !h.is_finite() || h < 0.0)) || self.moved.len() > MAX_EVENTS || self.fired.len() > 4 * MAX_EVENTS || self.warned.len() > MAX_EVENTS {
            return Err("events: bad record".into());
        }
        Ok(())
    }
}

/// One line of [`World::event_schedule`].
#[derive(Clone, Debug, PartialEq)]
pub struct Scheduled {
    pub id: String,
    pub name: String,
    pub effect: EventEffect,
    /// When it is due in this world (game hours after it began); None: not in this world.
    pub due_h: Option<f64>,
    /// When it happened, if it has.
    pub fired_h: Option<f64>,
}

impl World {
    /// The world's events (`events.ron`, as saved with its settings).
    pub fn events_def(&self) -> &EventsDef {
        &self.config.events
    }

    /// What has happened.
    pub fn event_state(&self) -> &EventState {
        &self.events
    }

    /// When event `i` is due in this world: moved by hand, else its own time from the seed.
    pub fn event_due(&self, i: usize) -> Option<f64> {
        let e = self.config.events.events.get(i)?;
        match self.events.moved.iter().find(|(id, _)| *id == e.id) {
            Some((_, at)) => *at,
            None => self.config.events.when(self.config.loot_seed(), i),
        }
    }

    /// Move event `id` to `at` game hours after the world began, or call it off (None), whether
    /// or not it has happened before (MCP `schedule_event`, `cancel_event`). A time already passed
    /// makes it happen at the next check.
    pub fn schedule_event(&mut self, id: &str, at: Option<f64>) -> Result<(), String> {
        self.event_index(id)?;
        if at.is_some_and(|h| !h.is_finite() || h < 0.0) {
            return Err("the time must be 0 or more game hours".into());
        }
        self.events.moved.retain(|(m, _)| m != id);
        self.events.moved.push((id.to_string(), at));
        self.events.fired.retain(|(f, _)| f != id);
        self.events.warned.retain(|w| w != id);
        Ok(())
    }

    fn event_index(&self, id: &str) -> Result<usize, String> {
        self.config.events.index_of(id).ok_or_else(|| {
            let ids: Vec<&str> = self.config.events.events.iter().map(|e| e.id.as_str()).collect();
            format!("no event {id:?} (events: {})", if ids.is_empty() { "none".to_string() } else { ids.join(", ") })
        })
    }

    /// Every event with when it is due here and when it happened.
    pub fn event_schedule(&self) -> Vec<Scheduled> {
        self.config.events.events
            .iter()
            .enumerate()
            .map(|(i, e)| Scheduled {
                id: e.id.clone(),
                name: e.name.clone(),
                effect: e.effect,
                due_h: self.event_due(i),
                fired_h: self.events.fired.iter().rev().find(|(f, _)| *f == e.id).map(|(_, h)| *h),
            })
            .collect()
    }

    /// Each second (and after a skip): events whose time has come happen, warnings whose time has
    /// come are given.
    pub(crate) fn step_events(&mut self, every_tick: bool) {
        if self.config.events.events.is_empty() || (!every_tick && !self.tick.is_multiple_of(20)) {
            return;
        }
        let now = self.game_hours();
        for i in 0..self.config.events.events.len() {
            let Some(at) = self.event_due(i) else { continue };
            let e = &self.config.events.events[i];
            if self.events.has_fired(&e.id) {
                continue;
            }
            if now >= at {
                self.fire_event(i, now);
            } else if let Some((h, text)) = &e.warn
                && now >= at - *h as f64
                && !self.events.warned.contains(&e.id)
            {
                let (id, text) = (e.id.clone(), text.clone());
                self.events.warned.push(id);
                self.tell_everyone(&text);
            }
        }
    }

    /// Make event `id` happen now (MCP `trigger_event`): whether or not it was due, or has
    /// happened before. Returns what everyone was told.
    pub fn trigger_event(&mut self, id: &str) -> Result<String, String> {
        let i = self.event_index(id)?;
        let now = self.game_hours();
        self.fire_event(i, now);
        let e = &self.config.events.events[i];
        Ok(if e.message.is_empty() { format!("{} happened", e.name) } else { format!("{} happened: {}", e.name, e.message) })
    }

    fn fire_event(&mut self, i: usize, now: f64) {
        let e = self.config.events.events[i].clone();
        self.events.fired.retain(|(f, _)| *f != e.id);
        self.events.fired.push((e.id.clone(), now));
        self.apply_event(e.effect);
        if !e.message.is_empty() {
            self.tell_everyone(&e.message);
        }
    }

    fn apply_event(&mut self, effect: EventEffect) {
        match effect {
            EventEffect::Notice => {}
            EventEffect::WaterOff => self.events.water_off = true,
            EventEffect::WaterOn => {
                // Running again: the cisterns fill up too.
                self.events.water_off = false;
                self.events.cisterns.clear();
            }
            EventEffect::PowerOff => {
                self.power.grid_failed = true;
                self.set_grid(false);
            }
            EventEffect::PowerOn => {
                self.power.grid_failed = false;
                self.set_grid(true);
            }
        }
    }

    /// A line every living survivor in the world is told (as an item note: the yellow line).
    fn tell_everyone(&mut self, text: &str) {
        let ids: Vec<u32> = self.players.iter().filter(|p| !p.dead).map(|p| p.id).collect();
        for id in ids {
            self.note(id, true, text.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::WorldConfig;

    const FILE: &str = include_str!("../../../assets/data/events.ron");

    fn world(events: EventsDef) -> World {
        let mut c = WorldConfig::new(7, 0, crate::worldgen::tests::templates());
        c.events = events;
        // Days go by in these tests: nobody should die of thirst before the water goes off.
        c.needs = crate::needs::NeedsTuning::off();
        World::new(c)
    }

    #[test]
    fn the_shipped_file_parses_and_mistakes_are_named() {
        let def = parse_events(FILE).unwrap();
        assert!(def.index_of("water_off").is_some() && def.index_of("power_off").is_some());
        let bad = |t: &str| parse_events(t).unwrap_err();
        assert!(bad("(events: [(id: \"a\", name: \"A\", days: (5.0, 3.0), effect: Notice)])").contains("\"a\": days"));
        assert!(bad("(events: [(id: \"a\", name: \"A\", days: (1.0, 3.0), effect: Notice), (id: \"a\", name: \"B\", days: (1.0, 3.0), effect: Notice)])").contains("twice"));
        assert!(bad("(events: [(id: \"a\", name: \"A\", days: (1.0, 3.0), effect: Flood)])").contains("Flood"));
        assert!(bad("(events: [(id: \"a\", name: \"A\", days: (1.0, 3.0), effect: Notice, chance: 2.0)])").contains("chance"));
    }

    #[test]
    fn events_happen_on_their_day_the_same_for_the_same_world() {
        let def = parse_events(FILE).unwrap();
        let i = def.index_of("water_off").unwrap();
        let (a, b) = def.events[i].days;
        let mut days = Vec::new();
        for seed in 0..40 {
            let h = def.when(seed, i).expect("the water always goes off");
            assert!(h >= a as f64 * 24.0 && h <= b as f64 * 24.0);
            assert_eq!(def.when(seed, i), Some(h));
            days.push((h / 24.0) as i32);
        }
        days.sort();
        days.dedup();
        assert!(days.len() >= 4, "different worlds, different days: {days:?}");
    }

    #[test]
    fn the_water_and_power_go_off_with_a_warning_and_a_message() {
        let mut w = world(parse_events(FILE).unwrap());
        let id = w.add_player(glam::Vec2::new(20.5, 20.5));
        w.set_dev(id, Some(true), Some(true));
        // The power may go off before the water in this world: keep it on until it is cut by hand below.
        w.schedule_event("power_off", None).unwrap();
        let water = w.event_schedule().into_iter().find(|s| s.id == "water_off").unwrap().due_h.unwrap();
        assert!(w.water_on() && w.event_state().is_empty());
        let warn = w.config.events.events[w.config.events.index_of("water_off").unwrap()].warn.clone().unwrap();
        w.skip_time((water - warn.0 as f64 + 0.1) as f32);
        assert!(w.water_on() && w.event_state().warned.contains(&"water_off".to_string()));
        assert!(w.item_notes.iter().any(|n| n.player == id && n.text == warn.1), "warned");
        w.skip_time(warn.0);
        assert!(!w.water_on() && w.event_state().has_fired("water_off"));
        let hash = w.state_hash();
        w.skip_time(0.0);
        assert_eq!(w.state_hash(), hash, "an event happens once");
        // An admin's undo, and a power cut by hand.
        w.trigger_event("water_on").unwrap();
        assert!(w.water_on());
        assert!(w.power().grid_on());
        let said = w.trigger_event("power_off").unwrap();
        assert!(!w.power().grid_on() && said.contains("power"), "{said}");
        assert!(w.trigger_event("flood").unwrap_err().contains("no event \"flood\""));
        // Moved and called off by hand.
        w.trigger_event("power_on").unwrap();
        w.schedule_event("power_off", None).unwrap();
        w.skip_time(24.0 * 30.0);
        assert!(w.power().grid_on(), "called off");
        let now = w.game_hours();
        w.schedule_event("power_off", Some(now + 2.0)).unwrap();
        w.skip_time(1.0);
        assert!(w.power().grid_on());
        w.skip_time(1.5);
        assert!(!w.power().grid_on(), "moved");
        // Saved and loaded: what has happened stays.
        let content = w.config.clone();
        let back = World::load(&w.save(), &content).unwrap();
        assert_eq!(back.state_hash(), w.state_hash());
        assert_eq!(back.event_state(), w.event_state());
        assert!(back.water_on() && !back.power().grid_on());
    }

    #[test]
    fn a_world_without_events_hashes_as_before() {
        let a = world(EventsDef::default());
        let b = world(parse_events(FILE).unwrap());
        assert_eq!(a.state_hash(), b.state_hash());
    }
}
