//! The first night (#19): what a new survivor is working toward, and how its first night went.
//!
//! The first playable loop is: wake up in a bedroom, search the house, decide whether to stay or
//! go, and live until the first dawn. Every life keeps a [`FirstNight`] record on the server
//! ([`Player::first_night`]) that says how far along that is: the objectives done so far, when
//! night falls and when it ends, where the night was spent, and at the first dawn an [`Outcome`].
//! The client's objectives panel, hints and morning screen read it from the snapshot
//! (`net::RunView::first_night`), and the MCP `stats` tool shows it, so a person and an agent are
//! told the same thing.
//!
//! **Objectives** ([`Objective`], sticky once done): search three containers, hold a weapon, carry
//! something to eat and something to drink, be indoors at some point after dark, and be alive at
//! dawn. They are guidance, not rules: nothing is refused for not doing them, and the outcome is
//! only "alive at dawn" with what the night cost.
//!
//! **The clock:** `began`, `dusk` and `dawn` are hours on the world's clock
//! ([`World::clock_hours`]: the start hour plus game hours, never wrapping), so time skipped and
//! time slept through count. A life that begins at night is already in its first night.
//!
//! **Derived, like the run stats:** nothing the future depends on reads it, so it isn't in the
//! state hash, and it is `#[serde(skip)]` in `Player`; saves carry it in an optional `NITE`
//! section (see `crate::save`). A save without one loads with no record (`None`): the client then
//! shows no objectives.
//!
//! [`Player::first_night`]: crate::players::Player::first_night

use serde::{Deserialize, Serialize};

use crate::run::RunStats;
use crate::senses::{SUNRISE, SUNSET};
use crate::world::World;

/// Containers to search for the first objective.
pub const SEARCH_GOAL: u32 = 3;
/// The evening: this many game hours before dark the hints turn to where to spend the night.
pub const DUSK_WARNING_H: f64 = 2.0;

/// What the first day and night ask of a new survivor, in the order shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Objective {
    Search,
    Weapon,
    Supplies,
    Shelter,
    Dawn,
}

impl Objective {
    pub const ALL: [Objective; 5] = [Objective::Search, Objective::Weapon, Objective::Supplies, Objective::Shelter, Objective::Dawn];
}

/// Where a survivor is, as the night counts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shelter {
    /// The house it woke up in.
    Home,
    /// Any other building.
    Indoors,
    Outdoors,
}

/// Where the time is in the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Day,
    /// The last [`DUSK_WARNING_H`] hours of daylight.
    Evening,
    Night,
    /// The first dawn has come: there is an [`Outcome`].
    Morning,
}

/// The run's counts when night fell, so the outcome can say what the night itself cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub kills: u32,
    pub wounds: u32,
    pub hp_lost: f32,
}

/// How the first night went, fixed at the first dawn.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub tick: u64,
    /// Where most of the night was spent.
    pub shelter: Shelter,
    /// Game hours of the night asleep.
    pub slept_h: f32,
    /// Zombies killed, wounds taken and HP lost between nightfall and dawn.
    pub kills: u32,
    pub wounds: u32,
    pub hp_lost: f32,
    /// Hit points at dawn.
    pub hp: f32,
    /// Objectives done, of [`Objective::ALL`] (dawn included).
    pub objectives: u8,
}

impl Outcome {
    /// One line on where the night was spent.
    pub fn headline(&self) -> &'static str {
        match self.shelter {
            Shelter::Home => "You sat the night out at home.",
            Shelter::Indoors => "You spent the night under someone else's roof.",
            Shelter::Outdoors => "You spent the night out in the open.",
        }
    }

    /// One line on the state the survivor is in.
    pub fn verdict(&self) -> &'static str {
        if self.wounds == 0 && self.slept_h >= 4.0 {
            "Rested and unhurt. A good night, as they go now."
        } else if self.wounds == 0 {
            "Unhurt, but short of sleep."
        } else if self.hp >= 50.0 {
            "Hurt, but on your feet."
        } else {
            "Barely. See to those wounds."
        }
    }
}

/// One life's progress through the first day and night.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FirstNight {
    /// The world's clock ([`World::clock_hours`]) when this life began, when its first night
    /// falls and when that night ends.
    pub began: f64,
    pub dusk: f64,
    pub dawn: f64,
    /// The clock at the last update.
    pub now: f64,
    /// Has held a weapon; has carried food; has carried a drink; has been indoors after dark.
    pub armed: bool,
    pub food: bool,
    pub drink: bool,
    pub sheltered: bool,
    /// The run's counts when night fell (`None` until then).
    pub at_dusk: Option<Mark>,
    /// Game hours of the night spent at home, in another building, outdoors; and asleep.
    pub home_h: f32,
    pub indoors_h: f32,
    pub outdoors_h: f32,
    pub slept_h: f32,
    /// Set at the first dawn, if alive.
    pub outcome: Option<Outcome>,
}

impl FirstNight {
    /// A life beginning at `now` on the world's clock. By day, its first night is the coming one;
    /// at night, the one it is in.
    pub fn begin(now: f64) -> Self {
        let hour = now.rem_euclid(24.0);
        let midnight = now - hour;
        let (dusk, dawn) = if hour < SUNRISE as f64 {
            (now, midnight + SUNRISE as f64)
        } else if hour >= SUNSET as f64 {
            (now, midnight + 24.0 + SUNRISE as f64)
        } else {
            (midnight + SUNSET as f64, midnight + 24.0 + SUNRISE as f64)
        };
        Self { began: now, dusk, dawn, now, armed: false, food: false, drink: false, sheltered: false, at_dusk: None, home_h: 0.0, indoors_h: 0.0, outdoors_h: 0.0, slept_h: 0.0, outcome: None }
    }

    pub fn phase(&self) -> Phase {
        if self.outcome.is_some() {
            Phase::Morning
        } else if self.now >= self.dusk {
            Phase::Night
        } else if self.now >= self.dusk - DUSK_WARNING_H {
            Phase::Evening
        } else {
            Phase::Day
        }
    }

    /// Game hours until dark (0 once it is) and until dawn.
    pub fn hours_to_dusk(&self) -> f64 {
        (self.dusk - self.now).max(0.0)
    }

    pub fn hours_to_dawn(&self) -> f64 {
        (self.dawn - self.now).max(0.0)
    }

    /// Is `o` done? `run` is the same life's run stats (the searches are counted there).
    pub fn done(&self, o: Objective, run: &RunStats) -> bool {
        match o {
            Objective::Search => run.searched >= SEARCH_GOAL,
            Objective::Weapon => self.armed,
            Objective::Supplies => self.food && self.drink,
            Objective::Shelter => self.sheltered,
            Objective::Dawn => self.outcome.is_some(),
        }
    }

    /// How many of the objectives are done.
    pub fn done_count(&self, run: &RunStats) -> u8 {
        Objective::ALL.iter().filter(|&&o| self.done(o, run)).count() as u8
    }

    /// The objective in plain words, with what is still missing.
    pub fn label(&self, o: Objective, run: &RunStats) -> String {
        match o {
            Objective::Search => format!("Search the house ({}/{SEARCH_GOAL} cupboards or drawers)", run.searched.min(SEARCH_GOAL)),
            Objective::Weapon => "Take a weapon in hand".to_string(),
            Objective::Supplies => match (self.food, self.drink) {
                (true, false) => "Pack food and a drink (still no drink)".to_string(),
                (false, true) => "Pack food and a drink (still no food)".to_string(),
                _ => "Pack food and a drink".to_string(),
            },
            Objective::Shelter => format!("Be indoors after dark ({})", clock(self.dusk)),
            Objective::Dawn => format!("Live until dawn ({})", clock(self.dawn)),
        }
    }

    /// Where most of the night has been spent so far (outdoors when the hours tie at nothing).
    pub fn shelter(&self) -> Shelter {
        if self.home_h >= self.indoors_h && self.home_h >= self.outdoors_h && self.home_h > 0.0 {
            Shelter::Home
        } else if self.indoors_h >= self.outdoors_h && self.indoors_h > 0.0 {
            Shelter::Indoors
        } else {
            Shelter::Outdoors
        }
    }

    /// A value a tick couldn't have left, for a loaded save.
    pub fn check(&self) -> Result<(), String> {
        let clock = [("began", self.began), ("dusk", self.dusk), ("dawn", self.dawn), ("now", self.now)];
        if let Some((name, v)) = clock.iter().find(|(_, v)| !v.is_finite() || v.abs() > 1e9) {
            return Err(format!("first night: {name} is {v}"));
        }
        if self.dusk > self.dawn || self.began > self.dusk {
            return Err("first night: its dusk and dawn are out of order".to_string());
        }
        let mut hours = vec![("home_h", self.home_h), ("indoors_h", self.indoors_h), ("outdoors_h", self.outdoors_h), ("slept_h", self.slept_h)];
        if let Some(m) = self.at_dusk {
            hours.push(("hp lost by dusk", m.hp_lost));
        }
        if let Some(o) = self.outcome {
            hours.extend([("outcome slept_h", o.slept_h), ("outcome hp_lost", o.hp_lost), ("outcome hp", o.hp)]);
        }
        match hours.iter().find(|(_, v)| !v.is_finite() || *v < 0.0 || *v > 1e9) {
            Some((name, v)) => Err(format!("first night: {name} is {v}")),
            None => Ok(()),
        }
    }
}

/// "20:00" for an hour on the world's clock.
pub fn clock(hours: f64) -> String {
    let mins = (hours.rem_euclid(24.0) * 60.0).round() as u32 % (24 * 60);
    format!("{:02}:{:02}", mins / 60, mins % 60)
}

/// "5 h 40 min", "40 min" for a stretch of game time.
pub fn span(hours: f64) -> String {
    let mins = (hours.max(0.0) * 60.0).ceil() as u32;
    if mins >= 60 { format!("{} h {:02} min", mins / 60, mins % 60) } else { format!("{mins} min") }
}

impl World {
    /// Player `pi` starts a life now: its first night is ahead of it (or around it).
    pub(crate) fn begin_first_night(&mut self, pi: usize) {
        self.players[pi].first_night = Some(FirstNight::begin(self.clock_hours()));
    }

    /// Where a point is, as the night counts it.
    pub fn shelter_at(&self, pos: glam::Vec2) -> Shelter {
        match self.map.building_at(pos) {
            Some(b) if b == self.spawn_building => Shelter::Home,
            Some(_) => Shelter::Indoors,
            None => Shelter::Outdoors,
        }
    }

    /// Keep every living player's first-night record up to date after `hours` game hours went by
    /// (a tick's worth, or a skip's): objectives, the night's hours, and the outcome at dawn.
    pub(crate) fn step_first_night(&mut self, hours: f32) {
        let now = self.clock_hours();
        let tick = self.tick;
        for pi in 0..self.players.len() {
            let p = &self.players[pi];
            if p.dead || p.first_night.as_ref().is_none_or(|f| f.outcome.is_some()) {
                continue;
            }
            let category = |c| p.inventory.stacks().any(|s| self.config.items.get(s.item as usize).is_some_and(|d| d.category == c));
            let (food, drink) = (category(crate::items::ItemCategory::Food), category(crate::items::ItemCategory::Drink));
            let (armed, asleep, hp) = (p.weapon.is_some(), p.needs.asleep(), p.hp);
            let shelter = self.shelter_at(p.state.pos);
            let mark = Mark { kills: p.run.kills, wounds: p.run.wounds, hp_lost: p.run.hp_lost };
            let run = p.run.clone();
            let Some(f) = self.players[pi].first_night.as_mut() else { continue };
            f.now = now;
            f.armed |= armed;
            f.food |= food;
            f.drink |= drink;
            if now < f.dusk {
                continue;
            }
            let at_dusk = *f.at_dusk.get_or_insert(mark);
            // The part of this step that was night (a skip can straddle dusk or dawn).
            let night = (now.min(f.dawn) - (now - hours as f64).max(f.dusk)).max(0.0) as f32;
            match shelter {
                Shelter::Home => f.home_h += night,
                Shelter::Indoors => f.indoors_h += night,
                Shelter::Outdoors => f.outdoors_h += night,
            }
            if asleep {
                f.slept_h += night;
            }
            f.sheltered |= shelter != Shelter::Outdoors;
            if now >= f.dawn {
                let outcome = Outcome {
                    tick,
                    shelter: f.shelter(),
                    slept_h: f.slept_h,
                    kills: mark.kills.saturating_sub(at_dusk.kills),
                    wounds: mark.wounds.saturating_sub(at_dusk.wounds),
                    hp_lost: (mark.hp_lost - at_dusk.hp_lost).max(0.0),
                    hp,
                    objectives: f.done_count(&run) + 1,
                };
                f.outcome = Some(outcome);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::Slot;
    use crate::needs::NeedsTuning;
    use crate::save::tests::content;

    fn world(start_hour: f32) -> (World, u32) {
        let mut config = content();
        config.zombies = Some(0);
        config.start_hour = start_hour;
        config.needs = NeedsTuning::off();
        let mut w = World::new(config);
        let id = w.add_player(w.player_spawn);
        (w, id)
    }

    fn night(w: &World, id: u32) -> FirstNight {
        w.player(id).unwrap().first_night.clone().expect("a new player has a first night ahead")
    }

    #[test]
    fn dusk_and_dawn_follow_from_the_hour_a_life_begins() {
        let by_day = FirstNight::begin(14.0);
        assert_eq!((by_day.dusk, by_day.dawn, by_day.phase()), (20.0, 30.0, Phase::Day));
        let at_night = FirstNight::begin(22.5);
        assert_eq!((at_night.dusk, at_night.dawn, at_night.phase()), (22.5, 30.0, Phase::Night));
        let small_hours = FirstNight::begin(24.0 * 3.0 + 2.0);
        assert_eq!((small_hours.dawn, small_hours.phase()), (24.0 * 3.0 + 6.0, Phase::Night));
        let mut evening = FirstNight::begin(9.0);
        evening.now = 18.5;
        assert_eq!(evening.phase(), Phase::Evening);
        assert_eq!((clock(20.0), clock(30.0), clock(23.999)), ("20:00".to_string(), "06:00".to_string(), "00:00".to_string()));
        assert_eq!((span(5.5), span(0.25), span(0.0)), ("5 h 30 min".to_string(), "15 min".to_string(), "0 min".to_string()));
    }

    #[test]
    fn objectives_tick_off_as_the_player_does_them_and_stay_done() {
        let (mut w, id) = world(14.0);
        w.step();
        let f = night(&w, id);
        let run = w.player(id).unwrap().run.clone();
        assert_eq!(f.done_count(&run), 0);
        assert_eq!(f.label(Objective::Shelter, &run), "Be indoors after dark (20:00)");
        assert_eq!(f.label(Objective::Dawn, &run), "Live until dawn (06:00)");

        // A weapon in hand, then food, then a drink.
        let bat = w.config.items.iter().find(|i| i.weapon.as_deref() == Some("baseball_bat")).unwrap().id.clone();
        assert!(w.hold_item(id, Slot::Hand, &bat));
        let food = w.config.items.iter().find(|i| i.category == crate::items::ItemCategory::Food).unwrap().id.clone();
        assert!(w.give_item(id, &food, 1));
        w.step();
        let f = night(&w, id);
        assert!(f.armed && f.food && !f.drink);
        assert!(!f.done(Objective::Supplies, &run));
        assert_eq!(f.label(Objective::Supplies, &run), "Pack food and a drink (still no drink)");
        let drink = w.config.items.iter().find(|i| i.category == crate::items::ItemCategory::Drink).unwrap().id.clone();
        assert!(w.give_item(id, &drink, 1));
        w.step();
        assert!(night(&w, id).done(Objective::Supplies, &run));

        // Dropping it all doesn't undo them.
        let pi = 0;
        w.players[pi].inventory = Default::default();
        w.refresh_carry(pi);
        w.step();
        let f = night(&w, id);
        assert!(f.armed && f.food && f.drink);

        // Searches come from the run stats.
        let mut run = run;
        run.searched = 2;
        assert!(!f.done(Objective::Search, &run));
        run.searched = 3;
        assert!(f.done(Objective::Search, &run));
        assert_eq!(f.done_count(&run), 3);
    }

    #[test]
    fn a_night_at_home_ends_in_an_outcome_at_dawn() {
        let (mut w, id) = world(19.5);
        // Half an hour to dark: not night yet, nothing counted.
        w.skip_time(0.25);
        w.step();
        let f = night(&w, id);
        assert_eq!(f.phase(), Phase::Evening);
        assert!(f.at_dusk.is_none() && !f.sheltered && f.home_h == 0.0);
        // Into the night, in the bedroom.
        w.skip_time(1.0);
        w.step();
        let f = night(&w, id);
        assert_eq!(f.phase(), Phase::Night);
        assert!(f.sheltered && f.at_dusk.is_some());
        assert!((f.home_h - 0.75).abs() < 0.02, "only the hours after dusk count: {}", f.home_h);
        assert!(f.outcome.is_none());
        // To dawn.
        w.skip_time(9.5);
        w.step();
        let f = night(&w, id);
        let o = f.outcome.expect("alive at dawn");
        assert_eq!(f.phase(), Phase::Morning);
        assert_eq!(o.shelter, Shelter::Home);
        assert!((f.home_h - 10.0).abs() < 0.02, "the night is ten hours, whatever was skipped past dawn: {}", f.home_h);
        assert_eq!((o.kills, o.wounds, o.objectives), (0, 0, 2));
        assert_eq!(o.headline(), "You sat the night out at home.");
        // Fixed from then on.
        w.skip_time(5.0);
        w.step();
        assert_eq!(night(&w, id), f);
    }

    #[test]
    fn a_night_outdoors_says_so_and_counts_what_it_cost() {
        let (mut w, id) = world(21.0);
        // Out on open ground west of town, hurt after dark.
        let out = (0..400).map(|k| glam::Vec2::new(6.5 + k as f32, 6.5)).find(|&p| w.map.building_at(p).is_none() && !w.map.tile_blocked(p.floor().as_ivec2())).unwrap();
        w.players[0].state.pos = out;
        w.step();
        assert_eq!(w.shelter_at(out), Shelter::Outdoors);
        w.hurt_player(0, 30.0);
        w.skip_time(9.5);
        w.step();
        let o = night(&w, id).outcome.expect("alive at dawn");
        assert_eq!(o.shelter, Shelter::Outdoors);
        assert!(!night(&w, id).sheltered);
        // (Some of it has healed by dawn.)
        assert!((o.hp_lost - 30.0).abs() < 0.5 && o.hp < 95.0, "{o:?}");
        assert_eq!(o.objectives, 1);
        assert_eq!(o.headline(), "You spent the night out in the open.");
    }

    #[test]
    fn the_dead_get_no_outcome_and_a_respawn_starts_over() {
        let (mut w, id) = world(21.0);
        w.step();
        w.hurt_player(0, 1000.0);
        assert!(w.player(id).unwrap().dead);
        w.skip_time(12.0);
        w.step();
        assert!(night(&w, id).outcome.is_none(), "dead before dawn: no morning");
        // Getting up the next day: a new life, with the coming night as its first.
        assert!(w.respawn_player(id));
        let f = night(&w, id);
        assert_eq!(f.phase(), Phase::Day);
        assert!(f.dusk > f.began && f.outcome.is_none() && f.at_dusk.is_none());
    }

    #[test]
    fn the_record_is_saved_but_not_hashed() {
        let (mut w, id) = world(19.5);
        w.skip_time(3.0);
        w.step();
        let bytes = w.save();
        let loaded = World::load(&bytes, &content()).unwrap();
        assert_eq!(loaded.player(id).unwrap().first_night, w.player(id).unwrap().first_night);
        assert_eq!(loaded.state_hash(), w.state_hash());
        // Nothing reads it back: without it the world hashes the same.
        let hash = w.state_hash();
        w.players[0].first_night = None;
        assert_eq!(w.state_hash(), hash);
        // A damaged record is refused, not loaded.
        let mut bad = FirstNight::begin(10.0);
        bad.home_h = f32::NAN;
        w.players[0].first_night = Some(bad);
        assert!(matches!(World::load(&w.save(), &content()), Err(crate::LoadError::Invalid(_))));
    }
}
