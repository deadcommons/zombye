//! What a survivor has seen of the town, for the map screen.
//!
//! Every player keeps an [`Explored`] record on the server ([`Player::explored`]): one bit for
//! each [`CELL`] m square of the map it has been near, and the buildings it has been inside. The
//! client's map screen draws the street plan everywhere and inks in buildings, gardens and trees
//! only where a cell is marked, and a building's rooms only once it has been entered.
//!
//! **The rule** ([`World::step_explore`], every [`ExploreTuning::every_ticks`]): a living player
//! marks the cells within its sight of the moment, `night_m` in the dark to `day_m` in daylight,
//! and no further than `indoors_m` while it stands in a building (walls are in the way). No rays:
//! a map is what you'd sketch walking past, not what you can see this second. A new survivor (a
//! respawn, a new run) starts with a blank map.
//!
//! **Not in the state hash** (nothing in the simulation reads it) and **not in `Player`'s serde
//! layout**: saves carry it in an optional `XPLR` section (see `crate::save`), so a save from
//! before the map loads with nothing explored. Snapshots carry each player's record whole
//! (`net::Snapshot::maps`, a client only its own); over the network it goes out when it changes.
//!
//! [`Player::explored`]: crate::players::Player::explored

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::world::World;

/// The side of one explored cell (metres, and tiles).
pub const CELL: i32 = 4;

/// How far a survivor's map fills in round it. Ours.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExploreTuning {
    /// Reach in full daylight and in the dark (metres), in between by the daylight.
    pub day_m: f32,
    pub night_m: f32,
    /// The most it reaches from inside a building.
    pub indoors_m: f32,
    /// Ticks between looks (10: twice a second).
    pub every_ticks: u32,
}

impl Default for ExploreTuning {
    fn default() -> Self {
        Self { day_m: 22.0, night_m: 9.0, indoors_m: 9.0, every_ticks: 10 }
    }
}

impl ExploreTuning {
    /// The reach at a daylight (0 night to 1 day), indoors or out.
    pub fn reach(&self, daylight: f32, indoors: bool) -> f32 {
        let out = self.night_m + (self.day_m - self.night_m) * daylight.clamp(0.0, 1.0);
        if indoors { out.min(self.indoors_m) } else { out }
    }
}

/// What one survivor has seen of the map.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explored {
    /// The grid's size in cells (0 × 0: not begun; [`Explored::fit`] sizes it to the map).
    pub w: u16,
    pub h: u16,
    /// One bit a cell, row by row, 64 to a word.
    pub cells: Vec<u64>,
    /// Buildings entered (indices into the map's buildings), sorted.
    pub entered: Vec<u16>,
    /// The tile this survivor stood on at its first look: where it woke up, or joined.
    pub home: Option<(i16, i16)>,
}

impl Explored {
    /// A blank record for a map `map_w` × `map_h` tiles.
    pub fn new(map_w: i32, map_h: i32) -> Self {
        let (w, h) = (cells_across(map_w), cells_across(map_h));
        Self { w, h, cells: vec![0; (w as usize * h as usize).div_ceil(64)], entered: Vec::new(), home: None }
    }

    /// Makes this a record for a map of that size: kept if it already is one, else blank.
    pub fn fit(&mut self, map_w: i32, map_h: i32) {
        if self.w != cells_across(map_w) || self.h != cells_across(map_h) || self.cells.len() != (self.w as usize * self.h as usize).div_ceil(64) {
            *self = Self::new(map_w, map_h);
        }
    }

    /// Is the cell (`cx`, `cy`) marked? Off the grid: no.
    pub fn cell(&self, cx: i32, cy: i32) -> bool {
        if cx < 0 || cy < 0 || cx >= self.w as i32 || cy >= self.h as i32 {
            return false;
        }
        let i = cy as usize * self.w as usize + cx as usize;
        self.cells.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 == 1)
    }

    /// Is the cell holding this point (metres) marked?
    pub fn at(&self, pos: Vec2) -> bool {
        self.cell((pos.x / CELL as f32).floor() as i32, (pos.y / CELL as f32).floor() as i32)
    }

    /// Marks every cell whose middle is within `reach` metres of `pos` (and the one `pos` is in).
    /// True if any was new.
    pub fn reveal(&mut self, pos: Vec2, reach: f32) -> bool {
        if self.w == 0 || self.h == 0 || !pos.is_finite() || !reach.is_finite() {
            return false;
        }
        let c = CELL as f32;
        let (x0, x1) = (((pos.x - reach) / c).floor() as i32, ((pos.x + reach) / c).floor() as i32);
        let (y0, y1) = (((pos.y - reach) / c).floor() as i32, ((pos.y + reach) / c).floor() as i32);
        let own = ((pos.x / c).floor() as i32, (pos.y / c).floor() as i32);
        let mut new = false;
        for cy in y0.max(0)..=y1.min(self.h as i32 - 1) {
            for cx in x0.max(0)..=x1.min(self.w as i32 - 1) {
                let mid = Vec2::new((cx as f32 + 0.5) * c, (cy as f32 + 0.5) * c);
                if (cx, cy) != own && mid.distance_squared(pos) > reach * reach {
                    continue;
                }
                let i = cy as usize * self.w as usize + cx as usize;
                let bit = 1u64 << (i % 64);
                if self.cells[i / 64] & bit == 0 {
                    self.cells[i / 64] |= bit;
                    new = true;
                }
            }
        }
        new
    }

    /// Notes a building as entered. True if it wasn't yet.
    pub fn enter(&mut self, building: usize) -> bool {
        let Ok(b) = u16::try_from(building) else { return false };
        match self.entered.binary_search(&b) {
            Ok(_) => false,
            Err(at) => {
                self.entered.insert(at, b);
                true
            }
        }
    }

    pub fn has_entered(&self, building: usize) -> bool {
        u16::try_from(building).is_ok_and(|b| self.entered.binary_search(&b).is_ok())
    }

    /// Cells marked.
    pub fn count(&self) -> u32 {
        self.cells.iter().map(|w| w.count_ones()).sum()
    }

    /// The share of the map marked, 0 to 1.
    pub fn share(&self) -> f32 {
        let all = self.w as u32 * self.h as u32;
        if all == 0 { 0.0 } else { self.count() as f32 / all as f32 }
    }

    /// A loaded record against the map it is for: the grid's size, no bit past its end, buildings
    /// that exist, sorted, each once.
    pub fn check(&self, map_w: i32, map_h: i32, buildings: usize) -> Result<(), String> {
        let n = self.w as usize * self.h as usize;
        if self.w != cells_across(map_w) || self.h != cells_across(map_h) || self.cells.len() != n.div_ceil(64) {
            return Err(format!("a map record of {} x {} cells in {} words, for a map {map_w} x {map_h} tiles", self.w, self.h, self.cells.len()));
        }
        if n % 64 != 0 && self.cells.last().is_some_and(|w| w >> (n % 64) != 0) {
            return Err("a map record with cells past its end".into());
        }
        if self.entered.windows(2).any(|p| p[0] >= p[1]) || self.entered.last().is_some_and(|&b| b as usize >= buildings) {
            return Err(format!("a map record with buildings entered out of order or past the {buildings} there are"));
        }
        if self.home.is_some_and(|(x, y)| x < 0 || y < 0 || x as i32 >= map_w || y as i32 >= map_h) {
            return Err("a map record that began off the map".into());
        }
        Ok(())
    }
}

fn cells_across(tiles: i32) -> u16 {
    (tiles.max(0) as u32).div_ceil(CELL as u32).min(u16::MAX as u32) as u16
}

impl World {
    /// Fills in each living player's map round where it stands (see the module docs).
    pub(crate) fn step_explore(&mut self) {
        let t = self.config.explore;
        if t.every_ticks == 0 || self.tick % t.every_ticks as u64 != 0 {
            return;
        }
        let daylight = self.daylight();
        let (w, h) = (self.map.width(), self.map.height());
        for p in &mut self.players {
            if p.dead {
                continue;
            }
            p.explored.fit(w, h);
            if p.explored.home.is_none() {
                let t = p.state.pos.floor().as_ivec2().clamp(glam::IVec2::ZERO, glam::IVec2::new(w - 1, h - 1).min(glam::IVec2::splat(i16::MAX as i32)));
                p.explored.home = Some((t.x as i16, t.y as i16));
            }
            let inside = self.map.building_at(p.state.pos);
            p.explored.reveal(p.state.pos, t.reach(daylight, inside.is_some()));
            if let Some(b) = inside {
                p.explored.enter(b);
            }
        }
    }

    /// What a player has seen of the map (blank for one that hasn't looked yet, or isn't there).
    pub fn explored(&self, player: u32) -> Explored {
        let mut e = self.players.iter().find(|p| p.id == player).map(|p| p.explored.clone()).unwrap_or_default();
        e.fit(self.map.width(), self.map.height());
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revealing_marks_a_disc_of_cells_once() {
        let mut e = Explored::new(480, 480);
        assert_eq!((e.w, e.h, e.cells.len()), (120, 120, 225));
        assert!(e.reveal(Vec2::new(100.0, 100.0), 22.0));
        assert!(!e.reveal(Vec2::new(100.0, 100.0), 22.0), "nothing new the second time");
        assert!(e.at(Vec2::new(100.0, 100.0)) && e.at(Vec2::new(118.0, 100.0)) && e.at(Vec2::new(100.0, 82.0)));
        assert!(!e.at(Vec2::new(130.0, 100.0)) && !e.at(Vec2::new(118.0, 118.0)));
        // About the disc's area in cells.
        let disc = std::f32::consts::PI * 22.0 * 22.0 / (CELL * CELL) as f32;
        assert!((e.count() as f32 - disc).abs() < disc * 0.15, "{} cells for a disc of {disc}", e.count());
        e.check(480, 480, 10).unwrap();
    }

    #[test]
    fn the_edge_of_the_map_and_bad_numbers_are_safe() {
        let mut e = Explored::new(50, 30);
        assert_eq!((e.w, e.h), (13, 8));
        assert!(e.reveal(Vec2::new(-3.0, 500.0), 600.0));
        assert_eq!(e.count(), 13 * 8);
        e.check(50, 30, 0).unwrap();
        assert!(!e.reveal(Vec2::new(f32::NAN, 0.0), 5.0) && !e.reveal(Vec2::ZERO, f32::INFINITY));
        assert!(!Explored::default().reveal(Vec2::ZERO, 5.0));
        assert!(!e.cell(-1, 0) && !e.cell(13, 0) && !e.cell(0, 8));
    }

    #[test]
    fn buildings_entered_stay_sorted_and_checked() {
        let mut e = Explored::new(64, 64);
        assert!(e.enter(7) && e.enter(2) && !e.enter(7) && e.enter(5));
        assert_eq!(e.entered, vec![2, 5, 7]);
        assert!(e.has_entered(5) && !e.has_entered(6));
        e.check(64, 64, 8).unwrap();
        assert!(e.check(64, 64, 7).is_err(), "building 7 of 7");
        assert!(e.check(128, 64, 8).is_err(), "another map's size");
        let mut bad = e.clone();
        bad.entered = vec![5, 2];
        assert!(bad.check(64, 64, 8).is_err());
        let mut lost = e.clone();
        lost.home = Some((64, 3));
        assert!(lost.check(64, 64, 8).is_err(), "began off the map");
        lost.home = Some((63, 3));
        lost.check(64, 64, 8).unwrap();
        let mut past = Explored::new(40, 40);
        *past.cells.last_mut().unwrap() |= 1 << 63;
        assert!(past.check(40, 40, 0).is_err(), "a bit past the last cell");
    }

    fn world() -> (World, u32) {
        let mut config = crate::save::tests::content();
        config.zombies = Some(0);
        let mut w = World::new(config);
        let id = w.add_player(w.player_spawn);
        (w, id)
    }

    #[test]
    fn a_survivor_maps_what_it_walks_past() {
        let (mut w, id) = world();
        assert_eq!(w.explored(id).count(), 0);
        for _ in 0..10 {
            w.step();
        }
        let woke = w.explored(id);
        let home = w.player_spawn;
        assert!(woke.at(home) && woke.count() > 4, "the bedroom and round it: {} cells", woke.count());
        assert!(!woke.at(home + Vec2::new(40.0, 0.0)), "indoors it reaches {} m", w.config.explore.indoors_m);
        assert_eq!(woke.entered, vec![w.map.building_at(home).unwrap() as u16]);
        assert_eq!(woke.home, Some((home.x.floor() as i16, home.y.floor() as i16)));
        // Further on, more of the map; what was seen stays.
        w.players[0].state.pos = home + Vec2::new(40.0, 0.0);
        for _ in 0..10 {
            w.step();
        }
        let later = w.explored(id);
        assert!(later.at(home) && later.at(home + Vec2::new(40.0, 0.0)) && later.count() > woke.count());
        assert_eq!(later.home, woke.home, "where it began doesn't move");
        // The dead add nothing, and a new life starts a blank map.
        w.players[0].dead = true;
        w.players[0].state.pos = home + Vec2::new(100.0, 0.0);
        for _ in 0..10 {
            w.step();
        }
        assert_eq!(w.explored(id), later);
        w.respawn_player(id);
        assert_eq!(w.player(id).unwrap().explored.count(), 0);
    }

    #[test]
    fn the_map_is_saved_but_not_hashed() {
        let (mut w, id) = world();
        for _ in 0..10 {
            w.step();
        }
        let loaded = World::load(&w.save(), &crate::save::tests::content()).unwrap();
        assert_eq!(loaded.explored(id), w.explored(id));
        assert!(loaded.explored(id).count() > 0);
        assert_eq!(loaded.state_hash(), w.state_hash());
        let hash = w.state_hash();
        w.players[0].explored = Explored::default();
        assert_eq!(w.state_hash(), hash, "nothing reads it back");
        // A record that doesn't fit the map is refused.
        w.players[0].explored = Explored::new(64, 64);
        w.players[0].explored.reveal(Vec2::new(10.0, 10.0), 5.0);
        assert!(matches!(World::load(&w.save(), &crate::save::tests::content()), Err(crate::LoadError::Invalid(_))));
    }

    #[test]
    fn the_reach_follows_the_daylight_and_stops_at_walls() {
        let t = ExploreTuning::default();
        assert_eq!(t.reach(1.0, false), t.day_m);
        assert_eq!(t.reach(0.0, false), t.night_m);
        assert_eq!(t.reach(1.0, true), t.indoors_m);
        assert!(t.reach(0.5, false) > t.night_m && t.reach(0.5, false) < t.day_m);
    }
}
