//! The calendar, the seasons, the weather and the air temperature.
//!
//! **The calendar** runs on the world's one clock ([`World::clock_hours`]: the start hour plus the
//! game hours gone, sleep and skips included), so every player of a world has the same date. A year is
//! twelve months of [`WeatherTuning::days_per_month`] days; a world starts on
//! [`WeatherTuning::start_month`] / [`WeatherTuning::start_day`] (early autumn by default).
//!
//! **The weather is worked out, not stored:** time is cut into slots of `slot_hours`; each slot either
//! keeps the last one's weather (`persist`) or draws a new one from the month's odds (`months`, seeded
//! by the world seed), so spells run from hours to days and some months are wetter, stormier or drier
//! than others. Rain falls as snow and a storm is a blizzard when the air is below `snow_below_c`.
//! The weather fades in and out over `blend_hours` at a change. Because it follows from the seed and
//! the clock alone it costs nothing to save or hash, and any client could work it out the same way.
//! The one stored part is [`Forced`] weather (MCP `set_weather`, for tests and pictures): saved in the
//! optional `WTHR` section and in the state hash while set.
//!
//! **The air temperature** is the month's low and high (blended day by day between mid-months), a
//! daily swing (coldest about 3:00, warmest about 15:00, wider under a clear sky, narrower under
//! cloud), the weather's own offset, and a slow seeded drift of up to `swing_c` either way over a few
//! days (cold snaps, heatwaves), all scaled about `mild_c` by `harshness`. Indoors is milder
//! (`indoor_c`, `indoor_mix`), and a lit campfire warms what is within `fire_radius` of it.
//!
//! The default climate is a hard one: winter nights fall below -20 C in a cold snap and summer days
//! pass 40 C in a heatwave. Nothing in the simulation reads the weather yet (no body temperature,
//! no effect on the senses): those come next, and read [`World::weather`] and [`World::air_temp_at`].
//!
//! Every number is in [`WeatherTuning`] (`assets/data/weather.ron`, `WorldConfig::weather`, saved with
//! the settings). All ours.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::rng::hash01;
use crate::world::World;

/// Salt for the weather's seeded draws, so they don't follow any other stream of the seed.
const SALT: u64 = 0x5745_4154_4845_52;

/// What the sky is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum WeatherKind {
    #[default]
    Clear,
    Cloudy,
    Overcast,
    Rain,
    Storm,
    Snow,
    Blizzard,
    Fog,
}

impl WeatherKind {
    pub const ALL: [WeatherKind; 8] = [Self::Clear, Self::Cloudy, Self::Overcast, Self::Rain, Self::Storm, Self::Snow, Self::Blizzard, Self::Fog];

    /// Its id, as in `weather.ron` and MCP (`"clear"`, `"blizzard"`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Cloudy => "cloudy",
            Self::Overcast => "overcast",
            Self::Rain => "rain",
            Self::Storm => "storm",
            Self::Snow => "snow",
            Self::Blizzard => "blizzard",
            Self::Fog => "fog",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }

    /// Whether something falls from the sky.
    pub fn falls(self) -> bool {
        matches!(self, Self::Rain | Self::Storm | Self::Snow | Self::Blizzard)
    }

    /// Rain and storms fall as snow and blizzards in the cold, and back.
    fn for_temp(self, temp_c: f32, snow_below_c: f32) -> Self {
        let cold = temp_c < snow_below_c;
        match self {
            Self::Rain | Self::Snow => if cold { Self::Snow } else { Self::Rain },
            Self::Storm | Self::Blizzard => if cold { Self::Blizzard } else { Self::Storm },
            k => k,
        }
    }
}

/// The seasons, by month: spring March to May, summer June to August, autumn September to November,
/// winter December to February.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

impl Season {
    pub fn of_month(month: u8) -> Self {
        match month {
            3..=5 => Self::Spring,
            6..=8 => Self::Summer,
            9..=11 => Self::Autumn,
            _ => Self::Winter,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Spring => "spring",
            Self::Summer => "summer",
            Self::Autumn => "autumn",
            Self::Winter => "winter",
        }
    }
}

/// A day of the calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Date {
    /// From 1, the year the world began in.
    pub year: u32,
    /// 1 to 12.
    pub month: u8,
    /// From 1 to `days_per_month`.
    pub day: u8,
    /// Days since the world began (0 its first day).
    pub day_number: u32,
}

impl Date {
    pub fn season(&self) -> Season {
        Season::of_month(self.month)
    }
}

/// One month's climate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MonthClimate {
    /// Its name, for people ("October").
    pub name: String,
    /// The usual coldest of a night (degrees C, before the weather and drift).
    pub low_c: f32,
    /// The usual warmest of a day (degrees C).
    pub high_c: f32,
    /// How usual each kind of weather is this month (relative weights; rain and storms fall as snow and
    /// blizzards when it is cold enough, so there are no separate snow weights).
    pub odds: Odds,
    /// How windy it usually is, 0 still to 1 a gale (storms add `storm_wind`).
    pub wind: f32,
}

impl Default for MonthClimate {
    fn default() -> Self {
        Self { name: String::new(), low_c: 5.0, high_c: 15.0, odds: Odds::default(), wind: 0.3 }
    }
}

/// Relative weights of the kinds of weather a slot can draw.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Odds {
    pub clear: f32,
    pub cloudy: f32,
    pub overcast: f32,
    /// Rain, or snow in the cold.
    pub rain: f32,
    /// A storm, or a blizzard in the cold.
    pub storm: f32,
    pub fog: f32,
}

impl Default for Odds {
    fn default() -> Self {
        Self { clear: 4.0, cloudy: 3.0, overcast: 2.0, rain: 2.0, storm: 0.5, fog: 0.5 }
    }
}

impl Odds {
    fn list(&self) -> [(WeatherKind, f32); 6] {
        [
            (WeatherKind::Clear, self.clear),
            (WeatherKind::Cloudy, self.cloudy),
            (WeatherKind::Overcast, self.overcast),
            (WeatherKind::Rain, self.rain),
            (WeatherKind::Storm, self.storm),
            (WeatherKind::Fog, self.fog),
        ]
    }

    fn lerp(&self, o: &Odds, t: f32) -> Odds {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Odds { clear: l(self.clear, o.clear), cloudy: l(self.cloudy, o.cloudy), overcast: l(self.overcast, o.overcast), rain: l(self.rain, o.rain), storm: l(self.storm, o.storm), fog: l(self.fog, o.fog) }
    }
}

/// What a kind of weather does.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KindDef {
    /// Which kind.
    pub kind: WeatherKind,
    /// How much of the sky is cloud, 0 to 1 (for the light, later).
    pub cloud: f32,
    /// Degrees C it adds to the air (rain cools, a clear day's sun doesn't: that is `swing`).
    pub temp_c: f32,
    /// The daily swing between low and high times this (a clear sky widens it, cloud narrows it).
    pub swing: f32,
    /// How far one sees in it, as a share of a clear day's sight, at full strength (for the senses, later).
    pub sight: f32,
    /// How far sounds carry in it, as a share, at full strength (for hearing, later).
    pub hearing: f32,
}

impl Default for KindDef {
    fn default() -> Self {
        Self { kind: WeatherKind::Clear, cloud: 0.1, temp_c: 0.0, swing: 1.0, sight: 1.0, hearing: 1.0 }
    }
}

impl KindDef {
    fn builtin(kind: WeatherKind) -> Self {
        let k = |cloud, temp_c, swing, sight, hearing| KindDef { kind, cloud, temp_c, swing, sight, hearing };
        match kind {
            WeatherKind::Clear => k(0.05, 0.0, 1.25, 1.0, 1.0),
            WeatherKind::Cloudy => k(0.45, 0.0, 0.9, 1.0, 1.0),
            WeatherKind::Overcast => k(0.9, -0.5, 0.5, 0.95, 1.0),
            WeatherKind::Rain => k(0.85, -2.0, 0.4, 0.8, 0.85),
            WeatherKind::Storm => k(1.0, -3.0, 0.35, 0.6, 0.7),
            WeatherKind::Snow => k(0.9, -1.0, 0.4, 0.7, 0.8),
            WeatherKind::Blizzard => k(1.0, -5.0, 0.3, 0.3, 0.6),
            WeatherKind::Fog => k(0.9, -1.0, 0.5, 0.4, 0.95),
        }
    }
}

/// `assets/data/weather.ron`. Every field is optional (these defaults).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WeatherTuning {
    /// The month a new world starts in (1 January to 12 December).
    pub start_month: u8,
    /// The day of that month it starts on.
    pub start_day: u8,
    /// Days in a month (twelve months a year). Fewer makes the seasons come round faster.
    pub days_per_month: u32,
    /// How often the weather may change (game hours).
    pub slot_hours: f32,
    /// The chance a slot keeps the weather of the one before (so spells last).
    pub persist: f32,
    /// Game hours the weather takes to fade in or out at a change.
    pub blend_hours: f32,
    /// Below this (degrees C) rain falls as snow and a storm is a blizzard.
    pub snow_below_c: f32,
    /// How far temperatures stray from `mild_c`: 1 as the months say, less a gentler climate, more a harsher.
    pub harshness: f32,
    /// The temperature `harshness` scales about (degrees C).
    pub mild_c: f32,
    /// Most degrees C a slow drift moves the temperature either way (cold snaps and heatwaves)...
    pub swing_c: f32,
    /// ...changing over this many days.
    pub swing_days: f32,
    /// What storms and blizzards add to the month's wind (0 to 1).
    pub storm_wind: f32,
    /// What the air indoors tends to (degrees C)...
    pub indoor_c: f32,
    /// ...and how much of the way there it goes (0 indoors is outdoors, 1 always `indoor_c`).
    pub indoor_mix: f32,
    /// Degrees C a lit campfire adds at its middle...
    pub fire_c: f32,
    /// ...falling to nothing this far off (metres).
    pub fire_radius: f32,
    /// The twelve months' climates, January first.
    pub months: Vec<MonthClimate>,
    /// What each kind of weather does; a kind left out keeps its built-in numbers.
    pub kinds: Vec<KindDef>,
}

impl Default for WeatherTuning {
    fn default() -> Self {
        // (name, low, high, clear, cloudy, overcast, rain, storm, fog, wind)
        const MONTHS: [(&str, f32, f32, [f32; 6], f32); 12] = [
            ("January", -14.0, -4.0, [3.0, 2.0, 3.0, 3.0, 2.0, 1.0], 0.45),
            ("February", -12.0, -2.0, [3.0, 2.0, 3.0, 3.0, 2.0, 1.0], 0.45),
            ("March", -5.0, 5.0, [3.0, 3.0, 3.0, 3.0, 1.0, 2.0], 0.4),
            ("April", 2.0, 13.0, [3.0, 3.0, 2.0, 4.0, 1.0, 1.0], 0.35),
            ("May", 8.0, 20.0, [4.0, 3.0, 2.0, 3.0, 1.0, 1.0], 0.3),
            ("June", 15.0, 28.0, [5.0, 3.0, 1.0, 2.0, 1.0, 0.5], 0.25),
            ("July", 20.0, 34.0, [7.0, 2.0, 1.0, 1.0, 1.5, 0.2], 0.2),
            ("August", 19.0, 33.0, [7.0, 2.0, 1.0, 1.0, 1.5, 0.3], 0.2),
            ("September", 12.0, 25.0, [4.0, 3.0, 2.0, 2.0, 1.0, 1.0], 0.3),
            ("October", 5.0, 16.0, [3.0, 3.0, 3.0, 4.0, 1.0, 2.0], 0.35),
            ("November", -2.0, 7.0, [2.0, 3.0, 4.0, 4.0, 1.5, 2.0], 0.4),
            ("December", -10.0, -1.0, [3.0, 2.0, 3.0, 3.0, 2.0, 1.0], 0.45),
        ];
        let months = MONTHS
            .iter()
            .map(|&(name, low_c, high_c, o, wind)| MonthClimate {
                name: name.into(),
                low_c,
                high_c,
                odds: Odds { clear: o[0], cloudy: o[1], overcast: o[2], rain: o[3], storm: o[4], fog: o[5] },
                wind,
            })
            .collect();
        Self {
            start_month: 9,
            start_day: 20,
            days_per_month: 30,
            slot_hours: 4.0,
            persist: 0.65,
            blend_hours: 1.0,
            snow_below_c: 1.0,
            harshness: 1.0,
            mild_c: 12.0,
            swing_c: 5.0,
            swing_days: 3.0,
            storm_wind: 0.45,
            indoor_c: 16.0,
            indoor_mix: 0.6,
            fire_c: 15.0,
            fire_radius: 4.0,
            months,
            kinds: WeatherKind::ALL.into_iter().map(KindDef::builtin).collect(),
        }
    }
}

impl WeatherTuning {
    /// Settings a tick can't work with, by name (checked on load and when the file is read).
    pub fn check(&self) -> Result<(), String> {
        let ok = |v: f32, lo: f32, hi: f32| v.is_finite() && v >= lo && v <= hi;
        if !(1..=12).contains(&self.start_month) {
            return Err(format!("weather.start_month must be from 1 to 12, got {}", self.start_month));
        }
        if !(1..=360).contains(&self.days_per_month) {
            return Err(format!("weather.days_per_month must be from 1 to 360, got {}", self.days_per_month));
        }
        if self.start_day == 0 || self.start_day as u32 > self.days_per_month {
            return Err(format!("weather.start_day must be from 1 to days_per_month ({}), got {}", self.days_per_month, self.start_day));
        }
        for (name, v, lo, hi) in [
            ("slot_hours", self.slot_hours, 0.25, 240.0),
            ("persist", self.persist, 0.0, 0.99),
            ("blend_hours", self.blend_hours, 0.0, 24.0),
            ("snow_below_c", self.snow_below_c, -30.0, 30.0),
            ("harshness", self.harshness, 0.0, 3.0),
            ("mild_c", self.mild_c, -50.0, 50.0),
            ("swing_c", self.swing_c, 0.0, 30.0),
            ("swing_days", self.swing_days, 0.25, 365.0),
            ("storm_wind", self.storm_wind, 0.0, 1.0),
            ("indoor_c", self.indoor_c, -50.0, 50.0),
            ("indoor_mix", self.indoor_mix, 0.0, 1.0),
            ("fire_c", self.fire_c, 0.0, 100.0),
            ("fire_radius", self.fire_radius, 0.0, 50.0),
        ] {
            if !ok(v, lo, hi) {
                return Err(format!("weather.{name} must be from {lo} to {hi}, got {v}"));
            }
        }
        if self.blend_hours * 2.0 > self.slot_hours {
            return Err(format!("weather.blend_hours ({}) must be at most half of slot_hours ({})", self.blend_hours, self.slot_hours));
        }
        if self.months.len() != 12 {
            return Err(format!("weather.months must list 12 months, January first, got {}", self.months.len()));
        }
        for m in &self.months {
            let o = &m.odds;
            let w = o.list();
            if !ok(m.low_c, -80.0, 60.0) || !ok(m.high_c, -80.0, 60.0) || m.low_c > m.high_c {
                return Err(format!("weather month {:?}: low_c and high_c must be from -80 to 60, low no higher than high", m.name));
            }
            if w.iter().any(|&(_, v)| !ok(v, 0.0, 1000.0)) || w.iter().map(|&(_, v)| v).sum::<f32>() <= 0.0 {
                return Err(format!("weather month {:?}: odds must be from 0 to 1000 and not all 0", m.name));
            }
            if !ok(m.wind, 0.0, 1.0) {
                return Err(format!("weather month {:?}: wind must be from 0 to 1, got {}", m.name, m.wind));
            }
        }
        for (i, k) in self.kinds.iter().enumerate() {
            if self.kinds[..i].iter().any(|o| o.kind == k.kind) {
                return Err(format!("weather.kinds lists {} twice", k.kind.id()));
            }
            for (name, v, lo, hi) in [("cloud", k.cloud, 0.0, 1.0), ("temp_c", k.temp_c, -30.0, 30.0), ("swing", k.swing, 0.0, 3.0), ("sight", k.sight, 0.05, 1.0), ("hearing", k.hearing, 0.05, 1.0)] {
                if !ok(v, lo, hi) {
                    return Err(format!("weather kind {}: {name} must be from {lo} to {hi}, got {v}", k.kind.id()));
                }
            }
        }
        Ok(())
    }

    /// What a kind of weather does (the file's entry, else the built-in one).
    pub fn kind(&self, kind: WeatherKind) -> KindDef {
        self.kinds.iter().find(|k| k.kind == kind).copied().unwrap_or_else(|| KindDef::builtin(kind))
    }

    fn year_days(&self) -> u32 {
        12 * self.days_per_month
    }

    /// Days into the year (from 0, 1 January) the world's first day is.
    fn start_offset(&self) -> u32 {
        (self.start_month as u32 - 1) * self.days_per_month + (self.start_day as u32 - 1)
    }

    /// The date at `clock_hours` on the world's clock.
    pub fn date_at(&self, clock_hours: f64) -> Date {
        let day_number = (clock_hours.max(0.0) / 24.0).floor() as u32;
        let since_jan = self.start_offset() + day_number;
        let in_year = since_jan % self.year_days();
        Date {
            year: 1 + since_jan / self.year_days(),
            month: (in_year / self.days_per_month) as u8 + 1,
            day: (in_year % self.days_per_month) as u8 + 1,
            day_number,
        }
    }

    /// Where in the year `clock_hours` is, in months from the middle of January (0..12, wrapping).
    fn month_pos(&self, clock_hours: f64) -> f32 {
        let days = self.start_offset() as f64 + clock_hours.max(0.0) / 24.0;
        let year = self.year_days() as f64;
        let pos = (days.rem_euclid(year) / self.days_per_month as f64 - 0.5) as f32;
        pos.rem_euclid(12.0)
    }

    /// The two months either side of a month position and how far from the first.
    fn months_at(&self, clock_hours: f64) -> (&MonthClimate, &MonthClimate, f32) {
        let pos = self.month_pos(clock_hours);
        let a = pos.floor() as usize % 12;
        (&self.months[a], &self.months[(a + 1) % 12], pos - pos.floor())
    }

    /// The kind a slot draws before the cold turns rain to snow.
    fn slot_kind(&self, seed: u64, slot: i64) -> WeatherKind {
        // Walk back over the slots that kept the weather (at most 16, so a spell can't run forever
        // and this stays cheap) to the one that drew it.
        let mut s = slot;
        for _ in 0..16 {
            if hash01(seed, SALT, s as u64, 1) >= self.persist {
                break;
            }
            s -= 1;
        }
        let (a, b, t) = self.months_at((s as f64 + 0.5) * self.slot_hours as f64);
        let odds = a.odds.lerp(&b.odds, t).list();
        let total: f32 = odds.iter().map(|o| o.1).sum();
        let mut roll = hash01(seed, SALT, s as u64, 2) * total;
        for (k, w) in odds {
            if roll < w {
                return k;
            }
            roll -= w;
        }
        WeatherKind::Clear
    }

    /// The slow drift of the temperature (cold snaps, heatwaves), -1 to 1, smooth over `swing_days`.
    fn drift(&self, seed: u64, clock_hours: f64) -> f32 {
        let x = clock_hours / 24.0 / self.swing_days as f64;
        let i = x.floor();
        let f = (x - i) as f32;
        let at = |n: f64| hash01(seed, SALT, n as i64 as u64, 3) * 2.0 - 1.0;
        let s = f * f * (3.0 - 2.0 * f);
        at(i) + (at(i + 1.0) - at(i)) * s
    }

    /// The weather outdoors at `clock_hours` for a world of `seed`, with nothing forced.
    pub fn at(&self, seed: u64, clock_hours: f64) -> WeatherNow {
        let slot_h = self.slot_hours as f64;
        let slot = (clock_hours / slot_h).floor() as i64;
        let kinds = [self.slot_kind(seed, slot - 1), self.slot_kind(seed, slot), self.slot_kind(seed, slot + 1)];
        let defs = kinds.map(|k| self.kind(k));
        let start = slot as f64 * slot_h;
        let blend = self.blend_hours.max(1e-3) as f64;
        let into = ((clock_hours - start) / blend).min(1.0) as f32;
        let out = ((start + slot_h - clock_hours) / blend).min(1.0) as f32;
        // How much of the slot's own weather shows, 0.5 at a change, 1 inside the slot; and the
        // neighbour it is shared with.
        let (other, mix) = if into < 1.0 { (0, 0.5 + 0.5 * into) } else if out < 1.0 { (2, 0.5 + 0.5 * out) } else { (1, 1.0) };
        let mix = if kinds[other] == kinds[1] { 1.0 } else { mix * mix * (3.0 - 2.0 * mix) };
        let blend = |f: fn(&KindDef) -> f32| f(&defs[other]) + (f(&defs[1]) - f(&defs[other])) * mix;

        // Intensity: a seeded strength for the spell, fading out at a change of kind.
        let strength = 0.45 + 0.55 * hash01(seed, SALT, slot as u64, 4);
        let fade = if kinds[other] == kinds[1] { 1.0 } else { (2.0 * mix - 1.0).clamp(0.0, 1.0) };
        let intensity = if kinds[1] == WeatherKind::Clear { 0.0 } else { strength * fade };

        // Temperature.
        let (a, b, t) = self.months_at(clock_hours);
        let low = a.low_c + (b.low_c - a.low_c) * t;
        let high = a.high_c + (b.high_c - a.high_c) * t;
        let hour = clock_hours.rem_euclid(24.0) as f32;
        let daily = ((hour - 15.0) / 24.0 * std::f32::consts::TAU).cos();
        let swing = blend(|d| d.swing);
        let raw = (low + high) * 0.5 + (high - low) * 0.5 * swing * daily + blend(|d| d.temp_c) + self.swing_c * self.drift(seed, clock_hours);
        let temp_c = self.mild_c + (raw - self.mild_c) * self.harshness;

        let kind = kinds[1].for_temp(temp_c, self.snow_below_c);
        let def = self.kind(kind);
        let wind_base = a.wind + (b.wind - a.wind) * t;
        let gust = hash01(seed, SALT, slot as u64, 5) * 0.3 - 0.1;
        let stormy = if matches!(kinds[1], WeatherKind::Storm) { self.storm_wind * intensity } else { 0.0 };
        WeatherNow {
            kind,
            intensity,
            cloud: blend(|d| d.cloud),
            wind: (wind_base + gust + stormy).clamp(0.0, 1.0),
            wind_dir: hash01(seed, SALT, slot as u64 / 3, 6) * std::f32::consts::TAU,
            temp_c,
            sight: 1.0 - (1.0 - def.sight) * intensity,
            hearing: 1.0 - (1.0 - def.hearing) * intensity,
            forced: false,
        }
    }
}

/// Parse and check `weather.ron`.
pub fn parse_weather(text: &str) -> Result<WeatherTuning, String> {
    let t: WeatherTuning = ron::from_str(text).map_err(|e| e.to_string())?;
    t.check()?;
    Ok(t)
}

/// The weather at one moment, outdoors.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WeatherNow {
    pub kind: WeatherKind,
    /// How strong it is, 0 (nothing, or a clear sky) to 1.
    pub intensity: f32,
    /// How much of the sky is cloud, 0 to 1.
    pub cloud: f32,
    /// 0 still to 1 a gale.
    pub wind: f32,
    /// The way the wind blows toward, radians (0 east, as `facing`).
    pub wind_dir: f32,
    /// The air outdoors, degrees C.
    pub temp_c: f32,
    /// How far one sees in it, as a share of a clear day's sight.
    pub sight: f32,
    /// How far sounds carry in it, as a share.
    pub hearing: f32,
    /// Set by [`World::force_weather`] rather than the climate.
    pub forced: bool,
}

/// Weather set by hand ([`World::force_weather`]): until `until_h` on the world's clock.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Forced {
    pub kind: WeatherKind,
    pub intensity: f32,
    /// The air temperature outdoors while it lasts, if set (else the climate's).
    pub temp_c: Option<f32>,
    /// On the world's clock ([`World::clock_hours`]).
    pub until_h: f64,
}

impl Forced {
    pub fn check(&self) -> Result<(), String> {
        let ok = |v: f32, lo: f32, hi: f32| v.is_finite() && v >= lo && v <= hi;
        if !ok(self.intensity, 0.0, 1.0) || !self.until_h.is_finite() || self.temp_c.is_some_and(|t| !ok(t, -80.0, 60.0)) {
            return Err(format!("forced weather out of range: {self:?}"));
        }
        Ok(())
    }

    /// Fold into the state hash.
    pub fn hash(&self, mix: &mut impl FnMut(u32)) {
        mix(0x5754_0000 | self.kind as u32);
        mix(self.intensity.to_bits());
        mix(self.temp_c.map_or(u32::MAX, f32::to_bits));
        mix((self.until_h.to_bits() >> 32) as u32);
        mix(self.until_h.to_bits() as u32);
    }
}

impl World {
    /// Today's date on the world's calendar.
    pub fn date(&self) -> Date {
        self.config.weather.date_at(self.clock_hours())
    }

    /// The season now.
    pub fn season(&self) -> Season {
        self.date().season()
    }

    /// The month's name now ("October").
    pub fn month_name(&self) -> &str {
        let m = self.date().month as usize - 1;
        self.config.weather.months.get(m).map_or("", |m| m.name.as_str())
    }

    /// The weather outdoors now.
    pub fn weather(&self) -> WeatherNow {
        self.weather_at(self.clock_hours())
    }

    /// The weather outdoors at `clock_hours` on the world's clock (a forecast, for tests and agents;
    /// forced weather counts while it lasts).
    pub fn weather_at(&self, clock_hours: f64) -> WeatherNow {
        let t = &self.config.weather;
        let mut w = t.at(self.config.generation.seed, clock_hours);
        if let Some(f) = self.weather_forced.filter(|f| clock_hours < f.until_h) {
            let def = t.kind(f.kind);
            w.kind = f.kind;
            w.intensity = f.intensity;
            w.cloud = def.cloud;
            w.sight = 1.0 - (1.0 - def.sight) * f.intensity;
            w.hearing = 1.0 - (1.0 - def.hearing) * f.intensity;
            if matches!(f.kind, WeatherKind::Storm | WeatherKind::Blizzard) {
                w.wind = (w.wind + t.storm_wind * f.intensity).min(1.0);
            }
            if let Some(c) = f.temp_c {
                w.temp_c = c;
            }
            w.forced = true;
        }
        w
    }

    /// The air temperature where a body stands (degrees C): outdoors the weather's; indoors (a room,
    /// or any floor upstairs) milder; warmer near a lit campfire.
    pub fn air_temp_at(&self, pos: Vec2, level: u8) -> f32 {
        let t = &self.config.weather;
        let outside = self.weather().temp_c;
        let mut temp = if level > 0 || self.map.room_at(pos) != 0 { outside + (t.indoor_c - outside) * t.indoor_mix } else { outside };
        if level == 0 && t.fire_radius > 0.0 {
            let near = self.fires.list.iter().filter(|f| f.lit).map(|f| f.pos.distance(pos)).fold(f32::INFINITY, f32::min);
            if near < t.fire_radius {
                temp += t.fire_c * (1.0 - near / t.fire_radius);
            }
        }
        temp
    }

    /// Force the weather (debug, MCP `set_weather`): `kind` at `intensity` for `hours` game hours, and the
    /// air at `temp_c` if given. `None` goes back to the climate's weather.
    pub fn force_weather(&mut self, forced: Option<(WeatherKind, f32, f32, Option<f32>)>) {
        self.weather_forced = forced.map(|(kind, intensity, hours, temp_c)| Forced {
            kind,
            intensity: if kind == WeatherKind::Clear { 0.0 } else { intensity.clamp(0.0, 1.0) },
            temp_c: temp_c.filter(|t| t.is_finite()).map(|t| t.clamp(-80.0, 60.0)),
            until_h: self.clock_hours() + hours.clamp(0.0, 24.0 * 365.0) as f64,
        });
    }

    /// The forced weather, while it lasts.
    pub fn weather_forced(&self) -> Option<Forced> {
        self.weather_forced.filter(|f| self.clock_hours() < f.until_h)
    }
}

#[cfg(test)]
mod tests;
