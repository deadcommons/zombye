//! Run stats and the cause of death (#13): what one life of a player came to.
//!
//! Every player keeps a [`RunStats`] record on the server ([`Player::run`]): time alive (simulated
//! seconds, and game hours with skipped time included), zombies killed, metres walked, things taken
//! from containers and the floor, containers searched first, wounds taken and HP lost, and once it
//! is over, a [`Death`] with its [`DeathCause`]. A life ends at death; getting up again
//! ([`World::respawn_player`]) starts a fresh record, as does a new world.
//!
//! **Counting** happens where the thing happens, one line each: kills in `combat` (a strike that
//! kills), distance and seconds in `World::step`'s movement pass, game hours in `step_health`
//! (which `skip_time` runs too), takes and searches in `step_item_jobs` and `player_pick_up`,
//! wounds in `add_wound`, HP lost and the cause in [`World::hurt_player_by`], the one place a
//! player loses HP. Anything that hurts says why with `hurt_player_by`; plain
//! [`World::hurt_player`] records [`DeathCause::Other`].
//!
//! **Adding a cause** (#11's starvation and thirst, say): a variant in [`DeathCause`], its words in
//! [`DeathCause::describe`], and `hurt_player_by(pi, loss, DeathCause::Starved)` where it hurts.
//! The client and the MCP server only show `describe`'s text, so nothing else changes.
//!
//! **Not in the state hash** (nothing the future depends on reads it) and **not in `Player`'s
//! serde layout** (`#[serde(skip)]`): saves carry it in an optional `RUNS` section of its own
//! (see `crate::save`), so a save without one (format 4 from before #13) loads with empty records.
//!
//! [`Player::run`]: crate::players::Player::run

use serde::{Deserialize, Serialize};

use crate::TICK_DT;
use crate::health::{Health, HealthTuning, InfectionStage, InfectionTuning, Part, WoundKind};
use crate::world::World;

/// One life of a player, counted by the server.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunStats {
    /// Simulated seconds alive (ticks of [`TICK_DT`]; time skipped isn't played, so not counted).
    pub secs: f64,
    /// Game hours alive, time skipped included (`HealthTuning`'s hours: 150 real seconds each by default).
    pub hours: f64,
    /// Zombies this player's swings, shoves and stomps killed.
    pub kills: u32,
    /// Metres moved on foot.
    pub distance_m: f64,
    /// Things taken from containers and the floor into pockets, bags, hands or clothing (each item
    /// of a stack counts).
    pub items_taken: u32,
    /// Containers this player was the first to search.
    pub searched: u32,
    /// Wounds taken (bruises included; god mode takes none).
    pub wounds: u32,
    /// Hit points lost, all causes.
    pub hp_lost: f32,
    /// How it ended; `None` while alive (or dead in a save from before #13).
    pub death: Option<Death>,
}

/// How and when a life ended.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Death {
    pub cause: DeathCause,
    /// The tick it happened on, the game day (1 = the first) and the hour of day.
    pub tick: u64,
    pub day: u32,
    pub hour: f32,
    /// The infection will raise (or has raised) the body as a zombie.
    pub rises: bool,
}

/// What took the last hit points. #11 adds starvation and thirst here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeathCause {
    /// A zombie's strike, and the wound it left (a bruise when clothing stopped it).
    Zombie { part: Part, wound: WoundKind },
    /// Bleeding, from the wound that bled most.
    BledOut { part: Part, wound: WoundKind },
    /// A festering wound.
    Festering { part: Part, wound: WoundKind },
    /// The infection's fever or the turn, and the kind of wound it came in through (if it hasn't
    /// healed since).
    Infection { wound: Option<WoundKind> },
    /// A cut from window glass (smashing one bare-handed, climbing through a broken one).
    Glass { part: Part },
    /// A wound given by a debug tool (MCP `wound`).
    Wound { part: Part, wound: WoundKind },
    /// Anything that didn't say (`World::hurt_player`).
    Other,
}

/// "a gash", "an ..." for a wound kind; a clothing-stopped blow is "a blow".
fn a_wound(w: WoundKind) -> &'static str {
    match w {
        WoundKind::Bruise => "a blow",
        WoundKind::Scratch => "a scratch",
        WoundKind::Gash => "a gash",
        WoundKind::Bite => "a bite",
        WoundKind::GlassCut => "a glass cut",
    }
}

impl DeathCause {
    /// In plain words: "Bitten and turned", "Bled out from a gash on the left leg".
    pub fn describe(&self, rises: bool) -> String {
        match *self {
            Self::Zombie { part, wound } => format!("Killed by zombies: {} to the {}", a_wound(wound), part.name()),
            Self::BledOut { part, wound } => format!("Bled out from {} on the {}", a_wound(wound), part.name()),
            Self::Festering { part, wound } => format!("Died of {} on the {} gone bad", a_wound(wound), part.name()),
            Self::Infection { wound } => match (wound, rises) {
                (Some(WoundKind::Bite), true) => "Bitten and turned".to_string(),
                (Some(w), true) => format!("Infected through {} and turned", a_wound(w)),
                (None, true) => "Infected and turned".to_string(),
                (Some(w), false) => format!("Died of the fever from {}", a_wound(w)),
                (None, false) => "Died of the fever".to_string(),
            },
            Self::Glass { part } => format!("Cut down by broken glass: a cut to the {}", part.name()),
            Self::Wound { part, wound } => format!("Died of {} to the {}", a_wound(wound), part.name()),
            Self::Other => "Died".to_string(),
        }
    }

    /// A short `snake_case` id for tools ("zombie", "bled_out", "infection"...).
    pub fn id(&self) -> &'static str {
        match self {
            Self::Zombie { .. } => "zombie",
            Self::BledOut { .. } => "bled_out",
            Self::Festering { .. } => "festering",
            Self::Infection { .. } => "infection",
            Self::Glass { .. } => "glass",
            Self::Wound { .. } => "wound",
            Self::Other => "other",
        }
    }
}

impl Death {
    /// The cause in plain words.
    pub fn describe(&self) -> String {
        self.cause.describe(self.rises)
    }

    /// A second line when the body rises and the cause doesn't already say it turned.
    pub fn epilogue(&self) -> Option<&'static str> {
        (self.rises && !matches!(self.cause, DeathCause::Infection { .. })).then_some("The infection in you brought the body back as one of them.")
    }
}

impl RunStats {
    /// Game days and hours alive, "2 days 5 hours", "3 hours", "40 minutes".
    pub fn survived(&self) -> String {
        let total_min = (self.hours * 60.0).max(0.0) as u64;
        let (days, hours, mins) = (total_min / (24 * 60), total_min / 60 % 24, total_min % 60);
        let plural = |n: u64, what: &str| if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") };
        match (days, hours) {
            (0, 0) => plural(mins, "minute"),
            (0, h) => plural(h, "hour"),
            (d, 0) => plural(d, "day"),
            (d, h) => format!("{} {}", plural(d, "day"), plural(h, "hour")),
        }
    }

    /// Simulated time alive as m:ss (h:mm:ss past an hour).
    pub fn played(&self) -> String {
        let s = self.secs.max(0.0) as u64;
        if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
    }

    /// Distance as "850 m" or "1.2 km".
    pub fn distance(&self) -> String {
        if self.distance_m < 1000.0 { format!("{:.0} m", self.distance_m) } else { format!("{:.1} km", self.distance_m / 1000.0) }
    }

    /// The cause of death in plain words, or `None` while alive (or unknown).
    pub fn cause_of_death(&self) -> Option<String> {
        self.death.map(|d| d.describe())
    }

    /// Values a run could have: finite, not negative, not absurdly large. For loading saves.
    pub fn check(&self, dead: bool) -> Result<(), String> {
        let ok = |v: f64| v.is_finite() && (0.0..=1e12).contains(&v);
        for (name, v) in [("secs", self.secs), ("hours", self.hours), ("distance_m", self.distance_m), ("hp_lost", self.hp_lost as f64)] {
            if !ok(v) {
                return Err(format!("run {name} is {v}"));
            }
        }
        if let Some(d) = self.death {
            if !dead {
                return Err("a living player has a cause of death".into());
            }
            if !d.hour.is_finite() || !(0.0..24.0).contains(&d.hour) {
                return Err(format!("death hour {}", d.hour));
            }
        }
        Ok(())
    }
}

/// What is draining a dying player's health now, as a cause: the infection's fever or turn, the
/// worst bleeding wound or the worst festering one, whichever costs the most per hour.
pub fn health_cause(h: &Health, t: &HealthTuning, inf: &InfectionTuning) -> DeathCause {
    let mut best = (0.0f32, DeathCause::Other);
    let mut consider = |rate: f32, cause: DeathCause| {
        if rate > best.0 {
            best = (rate, cause);
        }
    };
    for w in &h.wounds {
        consider(w.bleed_per_h(t), DeathCause::BledOut { part: w.part, wound: w.kind });
        if w.festering {
            consider(t.fester_hp_per_h, DeathCause::Festering { part: w.part, wound: w.kind });
        }
    }
    if let Some(i) = &h.infection {
        let rate = match i.stage {
            InfectionStage::Hidden => 0.0,
            InfectionStage::Feverish => inf.feverish_hp_per_h,
            InfectionStage::Turning => inf.turning_hp_per_h,
        };
        let wound = h.wounds.iter().find(|w| w.id == i.wound).map(|w| w.kind);
        // The turn is the end whatever else is going on.
        let rate = if i.stage == InfectionStage::Turning { f32::INFINITY } else { rate };
        consider(rate, DeathCause::Infection { wound });
    }
    best.1
}

impl World {
    /// Player `id`'s run so far (this life).
    pub fn run_stats(&self, id: u32) -> Option<&RunStats> {
        self.player(id).map(|p| &p.run)
    }

    /// Hours on the world clock since the start of day 1 (midnight before the start hour).
    fn clock_hours(&self) -> f64 {
        self.config.start_hour as f64 + self.tick as f64 * TICK_DT as f64 / self.config.day_length_secs as f64 * 24.0 + self.clock_offset_hours as f64
    }

    /// Take `damage` off player `pi` for `cause` (see [`World::hurt_player`]): counts the HP lost
    /// and, if it dies, records how. Returns whether it died.
    pub(crate) fn hurt_player_by(&mut self, pi: usize, damage: f32, cause: DeathCause) -> bool {
        let p = &mut self.players[pi];
        if p.dev.god || p.dead {
            return false;
        }
        p.run.hp_lost += damage.clamp(0.0, p.hp.max(0.0));
        p.hp -= damage;
        if p.hp > 0.0 {
            return false;
        }
        self.kill_player(pi);
        let hours = self.clock_hours();
        let p = &mut self.players[pi];
        let rises = p.health.rise_in.is_some();
        p.run.death = Some(Death { cause, tick: self.tick, day: (hours / 24.0).floor().max(0.0) as u32 + 1, hour: hours.rem_euclid(24.0) as f32, rises });
        true
    }

    /// The cause to record if player `pi`'s health drains to nothing now ([`health_cause`]).
    pub(crate) fn health_cause(&self, pi: usize) -> DeathCause {
        health_cause(&self.players[pi].health, &self.config.health, &self.config.infection)
    }

    /// A strike by player `id` killed a zombie.
    pub(crate) fn count_kill(&mut self, id: u32) {
        if let Some(p) = self.players.iter_mut().find(|p| p.id == id) {
            p.run.kills += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::Part;
    use crate::items::{parse_items, parse_weapons};
    use crate::map::TileMap;
    use crate::players::PlayerInput;
    use crate::template::BuildingTemplate;
    use crate::world::WorldConfig;
    use crate::zombies::ZombieState;
    use glam::Vec2;

    fn content() -> WorldConfig {
        let templates = vec![BuildingTemplate::from_ron(include_str!("../../../assets/data/buildings/bungalow.ron")).unwrap()];
        let weapons = parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        let items = parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap();
        let containers = crate::containers::parse_containers(include_str!("../../../assets/data/containers.ron")).unwrap();
        let mut c = WorldConfig::new(3, 0, templates).with_weapons(weapons).with_items(items).with_containers(containers).with_map_chunks(8);
        c.start_hour = 12.0;
        c
    }

    /// Open grass at noon, no zombies, one player at (60.5, 60.5) facing east.
    fn field() -> (World, u32) {
        let mut w = World::new(content());
        w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
        w.map_changed();
        w.items.clear();
        w.sites.clear();
        let id = w.add_player(Vec2::new(60.5, 60.5));
        (w, id)
    }

    /// A zombie that stays put until it notices someone.
    fn zombie(w: &mut World, pos: Vec2, facing: f32) {
        let i = w.add_zombie(pos, facing, 1.0);
        w.zombies.wander_timer[i] = 1e9;
    }

    fn me(w: &World, id: u32) -> &crate::players::Player {
        w.player(id).unwrap()
    }

    #[test]
    fn time_and_distance_are_counted() {
        let (mut w, id) = field();
        w.set_player_input(id, 1, PlayerInput { move_dir: Vec2::X, ..Default::default() });
        for _ in 0..40 {
            w.step();
        }
        let r = me(&w, id).run.clone();
        assert!((r.secs - 2.0).abs() < 1e-6, "{}", r.secs);
        // Walking 3 m/s for 2 s.
        assert!((r.distance_m - 6.0).abs() < 0.05, "{}", r.distance_m);
        let hours = 40.0 * TICK_DT as f64 * w.game_hours_per_sec() as f64;
        assert!((r.hours - hours).abs() < 1e-6, "{} vs {hours}", r.hours);
        // Skipped time counts as hours lived, not as seconds played.
        w.skip_time(5.0);
        let r = &me(&w, id).run;
        assert!((r.hours - hours - 5.0).abs() < 1e-3, "{}", r.hours);
        assert!((r.secs - 2.0).abs() < 1e-6);
        assert_eq!(r.survived(), "5 hours");
        assert_eq!(r.distance(), "6 m");
        assert!(r.death.is_none());
    }

    #[test]
    fn kills_are_counted_for_the_killer() {
        let (mut w, id) = field();
        w.config.combat.zombie_hp = (5.0, 5.0);
        let other = w.add_player(Vec2::new(10.0, 10.0));
        assert!(w.hold_item(id, crate::inventory::Slot::Hand, "baseball_bat"));
        let at = me(&w, id).state.pos + Vec2::X * 1.0;
        zombie(&mut w, at, std::f32::consts::PI);
        w.set_player_input(id, 1, PlayerInput { facing: 0.0, ..Default::default() });
        w.step();
        for _ in 0..200 {
            if w.zombies.state.iter().all(|s| *s == ZombieState::Dead) {
                break;
            }
            w.player_attack(id);
            w.step();
        }
        assert!(w.zombies.state.iter().all(|s| *s == ZombieState::Dead), "the zombie should be dead");
        assert_eq!(me(&w, id).run.kills, 1);
        assert_eq!(me(&w, other).run.kills, 0);
    }

    /// Steps until player `id` is dead (at most `ticks`).
    fn until_dead(w: &mut World, id: u32, ticks: u32) {
        for _ in 0..ticks {
            if me(w, id).dead {
                return;
            }
            w.step();
        }
    }

    #[test]
    fn a_zombie_strike_is_recorded() {
        let (mut w, id) = field();
        w.config.combat.zombie_hit_chance = 1.0;
        w.config.combat.zombie_hit_max = 1.0;
        w.config.infection.mode = crate::health::InfectionMode::Off;
        let p = me(&w, id).state.pos;
        for k in 0..4 {
            zombie(&mut w, p + Vec2::from_angle(k as f32 * 1.57) * 0.8, k as f32 * 1.57 + std::f32::consts::PI);
        }
        until_dead(&mut w, id, 20 * 120);
        let p = me(&w, id);
        assert!(p.dead, "four zombies should kill a player standing still");
        let d = p.run.death.expect("a cause");
        assert!(matches!(d.cause, DeathCause::Zombie { .. }), "{:?}", d.cause);
        assert!(!d.rises);
        assert!(d.describe().starts_with("Killed by zombies: "), "{}", d.describe());
        assert!(p.run.wounds >= 1 && p.run.hp_lost > 99.0 && p.run.hp_lost <= 100.0, "{} wounds, {} HP", p.run.wounds, p.run.hp_lost);
        assert_eq!(d.tick + 1, w.tick);
    }

    #[test]
    fn bleeding_out_is_recorded() {
        let (mut w, id) = field();
        w.config.infection.mode = crate::health::InfectionMode::Off;
        for part in [Part::LeftLeg, Part::Torso, Part::RightArm] {
            w.inflict_wound(id, part, WoundKind::Gash, Some(false)).unwrap();
        }
        assert!(!me(&w, id).dead);
        // Three gashes bleed 30 HP an hour: dead within a few game hours.
        w.skip_time(6.0);
        let p = me(&w, id);
        assert!(p.dead);
        let d = p.run.death.unwrap();
        assert!(matches!(d.cause, DeathCause::BledOut { wound: WoundKind::Gash, .. }), "{:?}", d.cause);
        assert!(d.describe().starts_with("Bled out from a gash on the "), "{}", d.describe());
        assert_eq!(p.run.wounds, 3);
        assert!(p.run.hours > 1.0 && p.run.hours < 6.0, "{}", p.run.hours);
    }

    #[test]
    fn the_turn_is_recorded() {
        let (mut w, id) = field();
        w.inflict_wound(id, Part::LeftArm, WoundKind::Bite, Some(true)).unwrap();
        // Keep the bite from bleeding the player out first: as if dressed (#10).
        let pi = w.players.iter().position(|p| p.id == id).unwrap();
        w.players[pi].health.wounds[0].dressing = crate::health::Dressing::Clean;
        w.skip_time(60.0);
        let p = me(&w, id);
        assert!(p.dead, "the infection is fatal by default ({:?})", p.health.infection);
        let d = p.run.death.unwrap();
        assert_eq!(d.cause, DeathCause::Infection { wound: Some(WoundKind::Bite) });
        assert!(d.rises);
        assert_eq!(d.describe(), "Bitten and turned");
        assert_eq!(d.epilogue(), None);
        assert!(d.day >= 2, "a day or more later: day {}", d.day);
    }

    #[test]
    fn god_mode_keeps_the_record_empty() {
        let (mut w, id) = field();
        w.set_dev(id, Some(true), None);
        w.inflict_wound(id, Part::Torso, WoundKind::Gash, Some(false));
        w.skip_time(20.0);
        let p = me(&w, id);
        assert!(!p.dead);
        assert_eq!((p.run.hp_lost, p.run.death), (0.0, None));
    }

    #[test]
    fn getting_up_starts_a_new_run() {
        let (mut w, id) = field();
        w.set_player_input(id, 1, PlayerInput { move_dir: Vec2::Y, ..Default::default() });
        for _ in 0..20 {
            w.step();
        }
        let pi = w.players.iter().position(|p| p.id == id).unwrap();
        w.hurt_player(pi, 1000.0);
        let r = &me(&w, id).run;
        assert_eq!(r.death.map(|d| d.cause), Some(DeathCause::Other));
        assert!(r.distance_m > 2.0);
        assert!(w.respawn_player(id));
        assert_eq!(me(&w, id).run, RunStats::default());
    }

    #[test]
    fn taking_and_searching_are_counted() {
        let (mut w, id) = field();
        let p = me(&w, id).state.pos;
        w.spawn_item("tinned_beans", 3, p + Vec2::X * 0.4).unwrap();
        w.player_pick_up(id).unwrap();
        assert_eq!(me(&w, id).run.items_taken, 3);
        let g = w.spawn_item("pen", 1, p + Vec2::X * 0.4).unwrap();
        w.player_move_item(id, crate::inventory::Place::Ground, g, crate::inventory::Place::Carried, 0).unwrap();
        for _ in 0..40 {
            w.step();
        }
        assert_eq!(me(&w, id).run.items_taken, 4);
        assert_eq!(me(&w, id).run.searched, 0);
    }

    #[test]
    fn searching_is_counted_once_per_container() {
        let mut w = World::new(content());
        let id = w.add_player(w.player_spawn);
        let c = crate::save::tests::open_nearest_container(&mut w, id);
        assert_eq!(me(&w, id).run.searched, 1);
        // Opening it again isn't a new search.
        w.player_search(id, Some(c)).unwrap();
        for _ in 0..40 {
            w.step();
        }
        assert_eq!(me(&w, id).run.searched, 1);
        // Taking what's in it counts as looting.
        if let Some(n) = w.container_contents(c).and_then(|s| s.first()).map(|s| s.count) {
            w.player_move_item(id, crate::inventory::Place::Container(c), 0, crate::inventory::Place::Carried, 0).unwrap();
            for _ in 0..60 {
                w.step();
            }
            let took = me(&w, id).run.items_taken;
            assert!(took == n as u32 || took == 0, "took {took} of {n}");
        }
    }

    #[test]
    fn plain_words() {
        let r = RunStats { hours: 53.5, secs: 4000.0, distance_m: 1234.0, ..Default::default() };
        assert_eq!((r.survived().as_str(), r.played().as_str(), r.distance().as_str()), ("2 days 5 hours", "1:06:40", "1.2 km"));
        assert_eq!(RunStats { hours: 24.0, ..Default::default() }.survived(), "1 day");
        assert_eq!(RunStats { hours: 0.5, ..Default::default() }.survived(), "30 minutes");
        let d = Death { cause: DeathCause::Zombie { part: Part::Head, wound: WoundKind::Bite }, tick: 1, day: 1, hour: 8.0, rises: true };
        assert_eq!(d.describe(), "Killed by zombies: a bite to the head");
        assert!(d.epilogue().is_some());
        assert_eq!(DeathCause::Infection { wound: Some(WoundKind::Gash) }.describe(true), "Infected through a gash and turned");
        assert_eq!(DeathCause::BledOut { part: Part::LeftLeg, wound: WoundKind::Gash }.describe(false), "Bled out from a gash on the left leg");
    }
}
