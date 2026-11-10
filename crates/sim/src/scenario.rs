//! Benchmark scenarios (#29): fixed starts to compare agents (or people) on, and the score.
//!
//! A scenario is data (`assets/data/scenarios.ron`, a list of [`ScenarioDef`]): the seed, the map
//! size, how many zombies, the hour, where the survivor starts and with what, zombies put near it
//! and a noise to draw them, how many game hours the run lasts, and what the score counts.
//! [`ScenarioDef::start`] makes the world: the same one every time, since everything in it comes
//! from the seed. [`ScenarioDef::standing`] reads how a run is going, or how it ended, from the
//! world alone: it keeps nothing in the world, so nothing here is saved or in the state hash, and
//! a world started from a scenario is a world like any other.
//!
//! A run ends when its survivor dies or the scenario's hours are up. **The score** is
//! `survive x (hours survived / hours)` + `house x houses searched` + `kill x zombies killed`,
//! with the three weights from the scenario ([`Points`]). A house counts once a container in it
//! has been searched (any building: a shop or an office too).
//!
//! The server runs them (`zombye-server --benchmark`, `crates/server/src/benchmark.rs`).

use std::collections::BTreeSet;

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::inventory::Slot;
use crate::items::ItemDef;
use crate::save::ContentPrint;
use crate::world::{World, WorldConfig};

/// The world's own zombies within this of the start are taken out as a scenario begins (metres;
/// a scenario's `hordes` are put where it says, however near).
pub const START_CLEAR: f32 = 12.0;
/// The longest a scenario may last (game hours).
pub const MAX_HOURS: f32 = 72.0;

/// One scenario, as `scenarios.ron` lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioDef {
    /// What it is asked for by: lower-case letters, digits and `_`.
    pub id: String,
    pub name: String,
    /// What whoever plays it is told: the situation and what counts.
    pub brief: String,
    pub seed: u64,
    /// The map is this many chunks of 16 m each way (default 30, as the game's).
    #[serde(default = "default_chunks")]
    pub map_chunks: i32,
    /// Zombies in the world at the start; `None`: as many as the population file gives the map.
    #[serde(default)]
    pub zombies: Option<u32>,
    /// The hour of day the run starts at (default 14, as a new game's).
    #[serde(default = "default_hour")]
    pub start_hour: f32,
    #[serde(default)]
    pub start: Start,
    /// An item in the survivor's hand at the start (a weapon, usually).
    #[serde(default)]
    pub hold: Option<String>,
    /// Items in its pockets at the start.
    #[serde(default)]
    pub kit: Vec<KitEntry>,
    /// Zombies put near the start, on top of the world's own.
    #[serde(default)]
    pub hordes: Vec<Horde>,
    /// A noise at the start point as the run begins.
    #[serde(default)]
    pub noise: Option<Noise>,
    /// How long the run lasts (game hours; one is 150 s of simulation at the default day length).
    pub hours: f32,
    #[serde(default)]
    pub points: Points,
}

fn default_chunks() -> i32 {
    30
}

fn default_hour() -> f32 {
    14.0
}

fn one() -> u16 {
    1
}

/// Where the survivor starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Start {
    /// The bedroom a single player wakes up in.
    #[default]
    Bedroom,
    /// Open ground outdoors nearest the middle of the town.
    Town,
    /// This point (metres; pushed out of walls and furniture).
    At(f32, f32),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KitEntry {
    pub item: String,
    #[serde(default = "one")]
    pub count: u16,
}

/// `count` zombies on open tiles within `radius` of the point `distance` metres from the start
/// toward `bearing` (degrees: 0 north, 90 east).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Horde {
    pub count: u32,
    pub distance: f32,
    #[serde(default)]
    pub bearing: f32,
    pub radius: f32,
}

/// A noise heard `radius` metres off; `lasting` keeps drawing the far horde in (an alarm).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Noise {
    pub radius: f32,
    #[serde(default)]
    pub lasting: bool,
}

/// What the score counts: `survive` for lasting the whole run (in proportion for part of it),
/// `house` for each building with a container searched, `kill` for each zombie killed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Points {
    #[serde(default = "default_survive")]
    pub survive: u32,
    #[serde(default = "default_house")]
    pub house: u32,
    #[serde(default = "default_kill")]
    pub kill: u32,
}

fn default_survive() -> u32 {
    1000
}

fn default_house() -> u32 {
    20
}

fn default_kill() -> u32 {
    5
}

impl Default for Points {
    fn default() -> Self {
        Points { survive: default_survive(), house: default_house(), kill: default_kill() }
    }
}

/// Read `scenarios.ron`: a list of scenarios, every number in range, no id twice, unknown fields
/// refused. Errors name the scenario.
pub fn parse_scenarios(text: &str) -> Result<Vec<ScenarioDef>, String> {
    let defs: Vec<ScenarioDef> = ron::from_str(text).map_err(|e| e.to_string())?;
    for (i, d) in defs.iter().enumerate() {
        d.validate().map_err(|e| format!("scenario {:?}: {e}", d.id))?;
        if defs[..i].iter().any(|o| o.id == d.id) {
            return Err(format!("scenario {:?}: id used twice", d.id));
        }
    }
    Ok(defs)
}

/// The items scenarios name (in the hand, in the kit) against the game's: errors name the scenario.
pub fn check_scenarios(defs: &[ScenarioDef], items: &[ItemDef]) -> Result<(), String> {
    for d in defs {
        for id in d.hold.iter().chain(d.kit.iter().map(|k| &k.item)) {
            if !items.iter().any(|i| i.id == *id) {
                return Err(format!("scenario {:?}: {id:?} isn't in items.ron", d.id));
            }
        }
    }
    Ok(())
}

impl ScenarioDef {
    pub fn validate(&self) -> Result<(), String> {
        let within = |name: &str, v: f32, lo: f32, hi: f32| if v.is_finite() && v >= lo && v <= hi { Ok(()) } else { Err(format!("{name} must be {lo} to {hi}, not {v}")) };
        if self.id.is_empty() || !self.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return Err("id must be lower-case letters, digits and _".into());
        }
        if self.name.trim().is_empty() || self.brief.trim().is_empty() {
            return Err("needs a name and a brief".into());
        }
        if !(8..=128).contains(&self.map_chunks) {
            return Err(format!("map_chunks must be 8 to 128, not {}", self.map_chunks));
        }
        if self.zombies.is_some_and(|n| n > 50_000) {
            return Err("at most 50000 zombies".into());
        }
        within("start_hour", self.start_hour, 0.0, 23.99)?;
        if !(self.hours.is_finite() && self.hours > 0.0 && self.hours <= MAX_HOURS) {
            return Err(format!("hours must be over 0 and at most {MAX_HOURS}, not {}", self.hours));
        }
        if let Start::At(x, y) = self.start {
            within("start x", x, 0.0, 1e5)?;
            within("start y", y, 0.0, 1e5)?;
        }
        if self.kit.len() > 12 {
            return Err(format!("{} kit entries: at most 12", self.kit.len()));
        }
        for (i, k) in self.kit.iter().enumerate() {
            if !(1..=20).contains(&k.count) {
                return Err(format!("kit {}: count {} (1 to 20)", k.item, k.count));
            }
            if self.kit[..i].iter().any(|o| o.item == k.item) {
                return Err(format!("kit {}: listed twice", k.item));
            }
        }
        if self.hordes.len() > 8 {
            return Err(format!("{} hordes: at most 8", self.hordes.len()));
        }
        for h in &self.hordes {
            if !(1..=2000).contains(&h.count) {
                return Err(format!("a horde of {} (1 to 2000)", h.count));
            }
            within("a horde's distance", h.distance, 0.0, 300.0)?;
            within("a horde's bearing", h.bearing, -360.0, 360.0)?;
            within("a horde's radius", h.radius, 1.0, 60.0)?;
        }
        if let Some(n) = self.noise {
            within("the noise's radius", n.radius, 1.0, 300.0)?;
        }
        let p = self.points;
        if p.survive.max(p.house).max(p.kill) > 100_000 || p.survive + p.house + p.kill == 0 {
            return Err("points must be at most 100000 each, and not all 0".into());
        }
        Ok(())
    }

    /// The world's settings: `content` (the game data, with its tunings) on this scenario's seed,
    /// map, zombie count and hour, player against player off.
    pub fn config(&self, content: &WorldConfig) -> WorldConfig {
        let mut config = content.clone().with_map_chunks(self.map_chunks);
        config.generation.seed = self.seed;
        config.zombies = self.zombies;
        config.start_hour = self.start_hour;
        config.combat.pvp = false;
        config
    }

    /// The scenario's world at tick 0, and its survivor's player id. The same world every time
    /// for the same game data.
    pub fn start(&self, content: &WorldConfig) -> (World, u32) {
        let mut world = World::new(self.config(content));
        let at = match self.start {
            Start::Bedroom => world.player_spawn,
            Start::Town => open_ground_near(&world, (world.town_min + world.town_max).as_vec2() * 0.5),
            Start::At(x, y) => Vec2::new(x, y),
        };
        let id = world.add_player(at);
        let at = world.player(id).expect("just added").state.pos;
        // Nobody stands on top of the survivor as the run begins (the bedroom is clear already).
        let near: Vec<bool> = world.zombies.pos.iter().map(|z| z.distance(at) < START_CLEAR).collect();
        world.zombies.remove_where(|i| near[i]);
        if let Some(item) = &self.hold {
            world.hold_item(id, Slot::Hand, item);
        }
        for k in &self.kit {
            world.give_item(id, &k.item, k.count);
        }
        for h in &self.hordes {
            // North is -y (the map's y runs south).
            let b = h.bearing.to_radians();
            world.spawn_zombies(h.count, at + Vec2::new(b.sin(), -b.cos()) * h.distance, h.radius);
        }
        if let Some(n) = self.noise {
            world.make_noise(at, n.radius, n.lasting);
        }
        (world, id)
    }

    /// How player `player`'s run of this scenario stands in `world`, or how it ended.
    pub fn standing(&self, world: &World, player: u32) -> Standing {
        let limit = self.hours as f64;
        let Some(p) = world.player(player) else {
            return Standing { outcome: Outcome::Died, hours: 0.0, limit, hp: 0.0, kills: 0, houses: 0, containers: 0, items_taken: 0, distance_m: 0.0, wounds: 0, hp_lost: 0.0, cause_of_death: None, points: [0; 3] };
        };
        let run = &p.run;
        let (outcome, hours) = if p.dead && run.hours < limit {
            (Outcome::Died, run.hours)
        } else if world.game_hours() >= limit {
            (Outcome::Survived, limit)
        } else {
            (Outcome::Running, world.game_hours())
        };
        // Only this survivor searches in a scenario's world, so every opened container is its own.
        let houses = world.containers().iter().filter(|s| world.container_contents(s.id).is_some()).map(|s| s.building).collect::<BTreeSet<_>>().len() as u32;
        let points = [(self.points.survive as f64 * (hours / limit).clamp(0.0, 1.0)).round() as u32, self.points.house * houses, self.points.kill * run.kills];
        Standing {
            outcome,
            hours,
            limit,
            hp: if p.dead { 0.0 } else { p.hp },
            kills: run.kills,
            houses,
            containers: run.searched,
            items_taken: run.items_taken,
            distance_m: run.distance_m,
            wounds: run.wounds,
            hp_lost: run.hp_lost,
            cause_of_death: if p.dead { run.cause_of_death_with(&world.config.weapons) } else { None },
            points,
        }
    }

    /// A hash of the scenario and of the game data its world is made from: results are only
    /// comparable between runs with the same one.
    pub fn fingerprint(&self, world: &World) -> u64 {
        let print = ContentPrint::of(&world.config);
        let mut h = fnv(0xcbf2_9ce4_8422_2325, ron::to_string(self).unwrap_or_default().as_bytes());
        for part in [print.templates, print.weapons, print.items, print.containers, print.population, print.recipes, world.map_base_hash()] {
            h = fnv(h, &part.to_le_bytes());
        }
        h
    }
}

fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The open outdoor tile nearest `p` (its middle); the bedroom if there is none within 80 m.
fn open_ground_near(world: &World, p: Vec2) -> Vec2 {
    let c = p.floor().as_ivec2();
    for r in 0..80 {
        for dy in -r..=r {
            for dx in -r..=r {
                let t = c + IVec2::new(dx, dy);
                if dx.abs().max(dy.abs()) == r && world.map.in_bounds(t) && !world.map.tile_blocked(t) && world.map.tile(t).room == 0 {
                    return t.as_vec2() + Vec2::splat(0.5);
                }
            }
        }
    }
    world.player_spawn
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Alive, with hours to go.
    Running,
    /// Alive when the hours were up.
    Survived,
    Died,
}

impl Outcome {
    pub fn over(self) -> bool {
        self != Outcome::Running
    }

    pub fn name(self) -> &'static str {
        match self {
            Outcome::Running => "running",
            Outcome::Survived => "survived",
            Outcome::Died => "died",
        }
    }
}

/// A run so far, or as it ended ([`ScenarioDef::standing`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Standing {
    pub outcome: Outcome,
    /// Game hours survived (at most `limit`), of the scenario's `limit`.
    pub hours: f64,
    pub limit: f64,
    /// Hit points left (0 dead).
    pub hp: f32,
    pub kills: u32,
    /// Buildings with at least one container searched.
    pub houses: u32,
    pub containers: u32,
    pub items_taken: u32,
    pub distance_m: f64,
    pub wounds: u32,
    pub hp_lost: f32,
    pub cause_of_death: Option<String>,
    /// The score's three parts: for surviving, for houses, for kills.
    pub points: [u32; 3],
}

impl Standing {
    pub fn score(&self) -> u32 {
        self.points.iter().sum()
    }

    /// One line: "survived 2.0 of 2.0 h, 3 houses searched, 12 kills: 1120 points".
    pub fn line(&self) -> String {
        let how = match self.outcome {
            Outcome::Running => "alive after",
            Outcome::Survived => "survived",
            Outcome::Died => "died after",
        };
        let cause = self.cause_of_death.as_ref().map(|c| format!(" ({c})")).unwrap_or_default();
        format!(
            "{how} {:.2} of {:.2} game hours{cause}, {} house{} searched, {} kill{}: {} points ({} + {} + {})",
            self.hours,
            self.limit,
            self.houses,
            if self.houses == 1 { "" } else { "s" },
            self.kills,
            if self.kills == 1 { "" } else { "s" },
            self.score(),
            self.points[0],
            self.points[1],
            self.points[2],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::PlayerInput;

    fn content() -> WorldConfig {
        // No templates: a town of streets and empty lots is world enough here.
        WorldConfig::new(1, 0, Vec::new())
    }

    fn def(extra: &str) -> String {
        format!("[(id: \"t\", name: \"Test\", brief: \"Live.\", seed: 5, map_chunks: 12, zombies: Some(40), hours: 0.02{extra})]")
    }

    #[test]
    fn scenarios_parse_with_defaults_and_name_their_mistakes() {
        let d = &parse_scenarios(&def("")).unwrap()[0];
        assert_eq!((d.start, d.start_hour, d.points), (Start::Bedroom, 14.0, Points::default()));
        assert!(d.hold.is_none() && d.kit.is_empty() && d.hordes.is_empty() && d.noise.is_none());
        let full = &parse_scenarios(&def(
            ", start: Town, start_hour: 21.5, hold: Some(\"bat\"), kit: [(item: \"rag\", count: 2)], hordes: [(count: 5, distance: 20.0, bearing: 90.0, radius: 4.0)], \
             noise: Some((radius: 50.0, lasting: true)), points: (kill: 10)",
        ))
        .unwrap()[0];
        assert_eq!(full.points, Points { kill: 10, ..Points::default() });
        assert_eq!(full.hordes[0].count, 5);
        for (extra, want) in [
            (", hours: 0.0", "duplicate field"),
            (", wobble: 1", "wobble"),
            (", map_chunks: 2", "map_chunks"),
            (", start_hour: 25.0", "start_hour"),
            (", hordes: [(count: 0, distance: 5.0, radius: 4.0)]", "a horde of 0"),
            (", kit: [(item: \"rag\"), (item: \"rag\")]", "listed twice"),
            (", points: (survive: 0, house: 0, kill: 0)", "not all 0"),
        ] {
            let err = parse_scenarios(&def(extra)).unwrap_err();
            assert!(err.contains(want), "{err} should mention {want}");
        }
        let two = format!("[{0}, {0}]", def("").trim_start_matches('[').trim_end_matches(']'));
        assert!(parse_scenarios(&two).unwrap_err().contains("used twice"));
        assert!(parse_scenarios(&def("").replace("hours: 0.02", "hours: 100.0")).unwrap_err().contains("hours must be"));
        assert!(parse_scenarios(&def("").replace("\"t\"", "\"Bad Id\"")).unwrap_err().contains("id must be"));
        // Items are checked against the game's.
        let err = check_scenarios(std::slice::from_ref(full), &[]).unwrap_err();
        assert!(err.contains("\"bat\"") && err.contains("\"t\""), "{err}");
    }

    /// The same scenario starts the same world, and the same inputs end it the same way.
    #[test]
    fn a_scenario_starts_and_runs_the_same_every_time() {
        let d = parse_scenarios(&def(", start: Town, hordes: [(count: 12, distance: 15.0, bearing: 90.0, radius: 5.0)], noise: Some((radius: 40.0))")).unwrap().remove(0);
        let run = || {
            let (mut world, id) = d.start(&content());
            let start = world.state_hash();
            for t in 0..300u32 {
                world.set_player_input(id, t + 1, PlayerInput { move_dir: Vec2::from_angle(t as f32 * 0.05), facing: 0.0, run: false, sneak: false, aim: false });
                world.step();
            }
            (start, world.state_hash(), d.standing(&world, id), d.fingerprint(&world))
        };
        let (a, b) = (run(), run());
        assert_eq!(a, b);
        assert_ne!(a.0, a.1);
        // Another seed is another world and another fingerprint.
        let other = ScenarioDef { seed: 6, ..d.clone() };
        let (world, _) = other.start(&content());
        assert_ne!(world.state_hash(), a.0);
        assert_ne!(other.fingerprint(&world), a.3);
    }

    #[test]
    fn the_start_is_as_the_scenario_says() {
        let d = parse_scenarios(&def(", start: Town, start_hour: 21.0, hordes: [(count: 12, distance: 15.0, bearing: 90.0, radius: 5.0)]")).unwrap().remove(0);
        let (world, id) = d.start(&content());
        let p = world.player(id).unwrap();
        assert_eq!(world.map.room_at(p.state.pos), 0, "outdoors");
        let mid = (world.town_min + world.town_max).as_vec2() * 0.5;
        assert!(p.state.pos.distance(mid) < 40.0, "{} from the middle of town", p.state.pos.distance(mid));
        // The world's 40 but those right by the start, and the horde's 12.
        assert!((40..=52).contains(&world.zombies.len()), "{} zombies", world.zombies.len());
        let apart = ScenarioDef { hordes: Vec::new(), ..d.clone() }.start(&content()).0;
        assert!(apart.zombies.pos.iter().all(|z| z.distance(p.state.pos) >= START_CLEAR));
        assert!((world.time_of_day() - 21.0).abs() < 0.01);
        // The horde is east of the start (bearing 90).
        let east = world.zombies.pos.iter().filter(|z| (**z - (p.state.pos + Vec2::new(15.0, 0.0))).length() <= 5.5).count();
        assert!(east >= 12, "{east} zombies in the horde's circle");
        // A fixed point, and the bedroom.
        let at = ScenarioDef { start: Start::At(p.state.pos.x, p.state.pos.y), hordes: Vec::new(), ..d.clone() };
        assert!(at.start(&content()).0.players[0].state.pos.distance(p.state.pos) < 0.01);
        let bed = ScenarioDef { start: Start::Bedroom, ..d.clone() };
        let (world, id) = bed.start(&content());
        assert_eq!(world.player(id).unwrap().state.pos, world.map.resolve_circle(world.player_spawn, crate::players::PLAYER_RADIUS));
    }

    /// Running until the hours are up, then survived with the full survival points; dying before
    /// keeps the share earned; kills count.
    #[test]
    fn the_standing_follows_the_run() {
        let mut d = parse_scenarios(&def("")).unwrap().remove(0);
        d.zombies = Some(0);
        let (mut world, id) = d.start(&content());
        let s = d.standing(&world, id);
        assert_eq!((s.outcome, s.score(), s.houses, s.kills), (Outcome::Running, 0, 0, 0));
        // 0.02 game hours at 150 s an hour: 3 s, 60 ticks.
        for _ in 0..30 {
            world.step();
        }
        let half = d.standing(&world, id);
        assert_eq!(half.outcome, Outcome::Running);
        assert_eq!(half.points, [500, 0, 0]);
        let mut dying = World::load(&world.save(), &content()).unwrap();
        for _ in 0..30 {
            world.step();
        }
        let s = d.standing(&world, id);
        assert_eq!((s.outcome, s.points), (Outcome::Survived, [1000, 0, 0]));
        assert!(s.line().starts_with("survived 0.02 of 0.02 game hours"), "{}", s.line());
        // Later ticks add nothing: the hours are capped at the scenario's.
        for _ in 0..40 {
            world.step();
        }
        assert_eq!(d.standing(&world, id).points, [1000, 0, 0]);

        dying.players[0].run.kills = 3;
        dying.hurt_player_by(0, 1e6, crate::run::DeathCause::Other);
        for _ in 0..40 {
            dying.step();
        }
        let s = d.standing(&dying, id);
        assert_eq!((s.outcome, s.points, s.hp), (Outcome::Died, [500, 0, 15], 0.0));
        assert!(s.cause_of_death.is_some() && s.line().starts_with("died after 0.01 of"), "{}", s.line());
    }
}
