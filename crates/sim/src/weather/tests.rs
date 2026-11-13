//! The calendar, the climate over a year, the weather's changes, indoor and fire warmth, forced
//! weather and saves.

use glam::Vec2;

use super::*;
use crate::world::WorldConfig;

const WEATHER: &str = include_str!("../../../../assets/data/weather.ron");

fn config(seed: u64) -> WorldConfig {
    let mut c = WorldConfig::new(seed, 0, Vec::new());
    c.weather = parse_weather(WEATHER).unwrap();
    c
}

/// The weather every `step_h` hours over `days` days from the world's start, for `seed`.
fn sample(t: &WeatherTuning, seed: u64, days: f64, step_h: f64) -> Vec<(f64, WeatherNow)> {
    let mut out = Vec::new();
    let mut h = 0.0;
    while h < days * 24.0 {
        out.push((h, t.at(seed, h)));
        h += step_h;
    }
    out
}

#[test]
fn the_shipped_file_is_the_defaults() {
    assert_eq!(parse_weather(WEATHER).unwrap(), WeatherTuning::default());
    WeatherTuning::default().check().unwrap();
}

#[test]
fn bad_settings_are_refused_by_name() {
    let bad = |f: &dyn Fn(&mut WeatherTuning)| {
        let mut t = WeatherTuning::default();
        f(&mut t);
        t.check().unwrap_err()
    };
    assert!(bad(&|t| t.start_month = 13).contains("start_month"));
    assert!(bad(&|t| t.start_day = 31).contains("start_day"));
    assert!(bad(&|t| t.persist = 1.0).contains("persist"));
    assert!(bad(&|t| t.blend_hours = 3.0).contains("blend_hours"));
    assert!(bad(&|t| { t.months.pop(); }).contains("12 months"));
    assert!(bad(&|t| t.months[0].low_c = 5.0).contains("January"));
    assert!(bad(&|t| t.months[3].odds = Odds { clear: 0.0, cloudy: 0.0, overcast: 0.0, rain: 0.0, storm: 0.0, fog: 0.0 }).contains("April"));
    assert!(bad(&|t| t.kinds = vec![KindDef::builtin(WeatherKind::Fog), KindDef::builtin(WeatherKind::Fog)]).contains("twice"));
    assert!(parse_weather("(harshness: 1.5)").is_ok());
    assert!(parse_weather("(sunshine: 1.0)").is_err(), "unknown fields are refused");
}

#[test]
fn the_calendar_counts_days_months_and_years() {
    let t = WeatherTuning::default();
    let d = t.date_at(0.0);
    assert_eq!((d.year, d.month, d.day, d.day_number), (1, 9, 20, 0));
    assert_eq!(d.season(), Season::Autumn);
    let d = t.date_at(23.9);
    assert_eq!((d.month, d.day), (9, 20), "still the first day");
    let d = t.date_at(11.0 * 24.0 + 1.0);
    assert_eq!((d.month, d.day, d.day_number), (10, 1, 11));
    // 20 September + 102 days = 2 January of the next year (30-day months).
    let d = t.date_at(102.0 * 24.0);
    assert_eq!((d.year, d.month, d.day), (2, 1, 2));
    assert_eq!(d.season(), Season::Winter);
    let short = WeatherTuning { days_per_month: 3, start_month: 6, start_day: 1, ..Default::default() };
    let d = short.date_at(3.0 * 24.0);
    assert_eq!((d.month, d.day, d.season()), (7, 1, Season::Summer));
}

#[test]
fn a_world_starts_in_autumn_on_its_own_clock() {
    let mut c = config(3);
    c.start_hour = 14.0;
    let w = World::new(c);
    assert_eq!(w.date().month, 9);
    assert_eq!(w.month_name(), "September");
    assert_eq!(w.season(), Season::Autumn);
}

#[test]
fn the_same_seed_gives_the_same_weather_and_another_seed_other_weather() {
    let t = WeatherTuning::default();
    let a = sample(&t, 7, 30.0, 1.0);
    assert_eq!(a, sample(&t, 7, 30.0, 1.0));
    let b = sample(&t, 8, 30.0, 1.0);
    assert!(a.iter().zip(&b).filter(|(x, y)| x.1.kind != y.1.kind).count() > 50, "two seeds should differ");
}

#[test]
fn winter_is_devastating_and_summer_very_hot() {
    let t = WeatherTuning::default();
    for seed in [1, 2, 3] {
        let year = sample(&t, seed, 360.0, 1.0);
        let tr = &t;
        let in_month = |m: u8| year.iter().filter(move |(h, _)| tr.date_at(*h).month == m).map(|(_, w)| w);
        let mean = |m: u8| in_month(m).map(|w| w.temp_c).sum::<f32>() / in_month(m).count() as f32;
        let coldest = year.iter().map(|(_, w)| w.temp_c).fold(f32::INFINITY, f32::min);
        let hottest = year.iter().map(|(_, w)| w.temp_c).fold(f32::NEG_INFINITY, f32::max);
        assert!(mean(1) < -5.0, "seed {seed}: January averages {:.1} C", mean(1));
        assert!(mean(7) > 22.0, "seed {seed}: July averages {:.1} C", mean(7));
        assert!(coldest < -18.0, "seed {seed}: the coldest hour of the year was only {coldest:.1} C");
        assert!(hottest > 36.0, "seed {seed}: the hottest hour of the year was only {hottest:.1} C");
        assert!(in_month(7).all(|w| !matches!(w.kind, WeatherKind::Snow | WeatherKind::Blizzard)), "seed {seed}: snow in July");
        let snowy = in_month(1).filter(|w| matches!(w.kind, WeatherKind::Snow | WeatherKind::Blizzard)).count();
        assert!(snowy > 40, "seed {seed}: only {snowy} hours of snow in January");
        // Autumn is wet, high summer dry.
        let wet = |m: u8| in_month(m).filter(|w| matches!(w.kind, WeatherKind::Rain | WeatherKind::Storm)).count();
        assert!(wet(10) > wet(7), "seed {seed}: October {} wet hours against July's {}", wet(10), wet(7));
    }
}

#[test]
fn the_weather_changes_in_spells_and_fades() {
    let t = WeatherTuning::default();
    let days = sample(&t, 11, 60.0, 0.1);
    // Spells: the weather changes far less often than every slot.
    let changes = days.windows(2).filter(|p| p[0].1.kind != p[1].1.kind && !(p[0].1.kind.falls() && p[1].1.kind.falls())).count();
    let slots = (60.0 * 24.0 / t.slot_hours as f64) as usize;
    assert!(changes < slots * 2 / 3, "{changes} changes in {slots} slots");
    // No jumps: six game minutes apart the air moves less than a degree, the cloud a little.
    for p in days.windows(2) {
        assert!((p[0].1.temp_c - p[1].1.temp_c).abs() < 1.0, "temperature jumped at {} h: {:?} to {:?}", p[0].0, p[0].1, p[1].1);
        assert!((p[0].1.cloud - p[1].1.cloud).abs() < 0.2, "cloud jumped at {} h", p[0].0);
    }
    // Rain fades out before it stops.
    for p in days.windows(2) {
        if p[0].1.kind.falls() && !p[1].1.kind.falls() {
            assert!(p[0].1.intensity < 0.1, "stopped at full strength at {} h: {:?}", p[0].0, p[0].1);
        }
    }
    for (_, w) in &days {
        assert!((0.0..=1.0).contains(&w.intensity) && (0.0..=1.0).contains(&w.cloud) && (0.0..=1.0).contains(&w.wind));
        assert!(w.sight > 0.0 && w.sight <= 1.0 && w.hearing > 0.0 && w.hearing <= 1.0);
    }
}

#[test]
fn harshness_and_mildness_scale_the_extremes() {
    let harsh = WeatherTuning { harshness: 1.3, ..Default::default() };
    let mild = WeatherTuning { harshness: 0.6, ..Default::default() };
    let normal = WeatherTuning::default();
    let jan = 102.0 * 24.0 + 3.0;
    let (h, n, m) = (harsh.at(4, jan).temp_c, normal.at(4, jan).temp_c, mild.at(4, jan).temp_c);
    assert!(h < n && n < m, "harsh {h}, normal {n}, mild {m}");
}

#[test]
fn indoors_is_milder_and_a_fire_warms() {
    let mut c = config(5);
    c.start_hour = 3.0;
    c.weather.start_month = 1;
    c.weather.start_day = 10;
    let mut w = World::new(c);
    w.force_weather(Some((WeatherKind::Clear, 0.0, 24.0, Some(-15.0))));
    let out = Vec2::new(2.5, 2.5);
    assert_eq!(w.map.room_at(out), 0, "the corner of the map is outdoors");
    assert_eq!(w.air_temp_at(out, 0), -15.0);
    let upstairs = w.air_temp_at(out, 1);
    assert!((upstairs - (-15.0 + (16.0 + 15.0) * 0.6)).abs() < 1e-3, "indoors {upstairs}");
    w.fires.list.push(crate::fire::Fire { id: 1, pos: out + Vec2::new(1.0, 0.0), fuel_h: 2.0, lit: true });
    let by_fire = w.air_temp_at(out, 0);
    assert!((by_fire - (-15.0 + 15.0 * 0.75)).abs() < 1e-3, "a metre from the fire {by_fire}");
    w.fires.list[0].lit = false;
    assert_eq!(w.air_temp_at(out, 0), -15.0, "a cold fire warms nothing");
}

#[test]
fn forced_weather_lasts_its_hours_and_is_saved() {
    let c = config(9);
    let mut w = World::new(c.clone());
    let hash = w.state_hash();
    assert!(!w.weather().forced);
    w.force_weather(Some((WeatherKind::Blizzard, 0.8, 2.0, Some(-20.0))));
    let now = w.weather();
    assert!(now.forced && now.kind == WeatherKind::Blizzard && now.temp_c == -20.0 && now.sight < 0.5, "{now:?}");
    assert_ne!(w.state_hash(), hash, "forced weather is in the hash");
    let loaded = World::load(&w.save(), &c).unwrap();
    assert_eq!(loaded.weather_forced(), w.weather_forced());
    assert_eq!(loaded.state_hash(), w.state_hash());
    assert!(w.weather_at(w.clock_hours() + 3.0).forced == false, "over after two hours");
    w.force_weather(None);
    assert_eq!(w.state_hash(), hash, "back to the climate: hashes as before");
    let plain = World::load(&w.save(), &c).unwrap();
    assert_eq!(plain.weather_forced(), None);
}

#[test]
fn a_saved_world_keeps_its_calendar_and_climate() {
    let mut c = config(2);
    c.weather.start_month = 3;
    c.weather.harshness = 1.4;
    let w = World::new(c.clone());
    // Loaded against content with the shipped climate: the save's own wins.
    let loaded = World::load(&w.save(), &config(2)).unwrap();
    assert_eq!(loaded.config.weather, c.weather);
    assert_eq!(loaded.date(), w.date());
    assert_eq!(loaded.weather(), w.weather());
}

/// Prints a year of weather, a line a week (`cargo test -p zombye-sim --release print_year -- --ignored --nocapture`).
#[test]
#[ignore]
fn print_year() {
    let t = WeatherTuning::default();
    let seed = std::env::var("ZOMBYE_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    for week in 0..52 {
        let days = sample(&t, seed, 7.0, 1.0);
        let from = week as f64 * 7.0 * 24.0;
        let ws: Vec<WeatherNow> = days.iter().map(|(h, _)| t.at(seed, from + h)).collect();
        let lo = ws.iter().map(|w| w.temp_c).fold(f32::INFINITY, f32::min);
        let hi = ws.iter().map(|w| w.temp_c).fold(f32::NEG_INFINITY, f32::max);
        let mut counts = std::collections::BTreeMap::new();
        for w in &ws {
            *counts.entry(w.kind.id()).or_insert(0) += 1;
        }
        let d = t.date_at(from);
        println!("{:>2} {:>2}/{:<2} {:>6.1} {:>6.1}  {counts:?}", week, d.day, d.month, lo, hi);
    }
}
