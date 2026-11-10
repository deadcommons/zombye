//! Music (#78): a score that follows how things stand, sparse by design. The town is meant to feel
//! empty, so most of the time nothing plays: a track now and then by day or at night with long
//! silences between, something that builds when zombies close in, a short piece at dawn and a sting
//! when you die.
//!
//! - **What plays** is data: `assets/data/music.ron` gives each [`Mood`] its files (under
//!   `assets/audio/`, OGG Vorbis only: this Bevy build has no WAV decoder, so a WAV would panic
//!   when played), volume, the silence between tracks and the fades. A mood with no files is
//!   silent, so the game runs the same before the tracks exist.
//! - **When:** dead → `Death` (once); zombies on us (one attacking within 6 m, or three coming within
//!   12 m; held 8 s after) → `Danger` (loops while it lasts); the night turning to day → `Dawn`
//!   (once); else `Night` or `Day` by the daylight, a track then a silence of `gap` seconds.
//! - **How:** Bevy's own player (no panning or muffling: music is around you), faded in and out by
//!   us; a change of mood fades the old track out over its `fade_out` while the new one comes in.
//!   Under the master volume and mute (M), times `--music V` (0..2, default 1; 0 turns it off).
//!   Asleep, quieter. Client only.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use bevy::audio::{AudioSink, AudioSinkPlayback, PlaybackMode, Volume};
use bevy::prelude::*;
use serde::Deserialize;
use zombye_sim::ZombieAnim;

use crate::Options;
use crate::audio::AudioSettings;
use crate::player::LocalPlayer;
use crate::server_link::ServerLink;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub enum Mood {
    Day,
    Night,
    Danger,
    Dawn,
    Death,
}

impl Mood {
    pub const ALL: [Mood; 5] = [Mood::Day, Mood::Night, Mood::Danger, Mood::Dawn, Mood::Death];

    /// Played once when it comes (not a track and a silence, not a loop).
    fn once(self) -> bool {
        matches!(self, Mood::Dawn | Mood::Death)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoodDef {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default = "one")]
    pub volume: f32,
    /// Seconds of silence after a track before the next (`Day` and `Night`).
    #[serde(default)]
    pub gap: (f32, f32),
    #[serde(default = "three")]
    pub fade_in: f32,
    #[serde(default = "three")]
    pub fade_out: f32,
}

fn one() -> f32 {
    1.0
}
fn three() -> f32 {
    3.0
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MusicData {
    /// The music's own level under the master volume.
    #[serde(default = "one")]
    pub volume: f32,
    pub moods: BTreeMap<Mood, MoodDef>,
}

pub fn parse_music(text: &str) -> Result<MusicData, String> {
    let data: MusicData = ron::from_str(text).map_err(|e| format!("music.ron: {e}"))?;
    if !(0.0..=2.0).contains(&data.volume) {
        return Err("music.ron: volume must be 0 to 2".into());
    }
    for mood in Mood::ALL {
        let Some(d) = data.moods.get(&mood) else {
            return Err(format!("music.ron: {mood:?} is missing"));
        };
        let bad = |what: &str| Err(format!("music.ron: {mood:?}: {what}"));
        if !(0.0..=2.0).contains(&d.volume) {
            return bad("volume must be 0 to 2");
        }
        if !(d.gap.0 >= 0.0 && d.gap.1 >= d.gap.0 && d.gap.1 <= 3600.0) {
            return bad("gap must be (min, max) seconds, 0 <= min <= max <= 3600");
        }
        if d.files.iter().any(|f| f.contains("..") || f.starts_with('/') || f.contains('\\') || !f.ends_with(".ogg")) {
            return bad("files must be .ogg (OGG Vorbis) inside assets/audio");
        }
        if !(d.fade_in > 0.0 && d.fade_in <= 60.0 && d.fade_out > 0.0 && d.fade_out <= 60.0) {
            return bad("fades must be over 0 and at most 60 seconds");
        }
    }
    Ok(data)
}

/// The tracks, loaded once.
#[derive(Resource)]
struct Music {
    data: MusicData,
    tracks: BTreeMap<Mood, Vec<Handle<AudioSource>>>,
    /// `--music`, 0..2.
    level: f32,
}

impl Music {
    /// Load the moods from `path` and their files from `audio_dir`, or from the release audio
    /// folder `overlay` where it has them (#154).
    fn load(path: &Path, audio_dir: &Path, overlay: Option<&Path>, sources: &mut Assets<AudioSource>, level: f32) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let data = parse_music(&text)?;
        let mut tracks = BTreeMap::new();
        for (mood, def) in &data.moods {
            let mut list = Vec::new();
            for file in &def.files {
                match crate::release_audio::read(overlay, audio_dir, file) {
                    Ok(bytes) => list.push(sources.add(AudioSource { bytes: Arc::from(bytes) })),
                    Err(e) => eprintln!("music: {mood:?}: {file}: {e} (left out)"),
                }
            }
            tracks.insert(*mood, list);
        }
        Ok(Self { data, tracks, level })
    }

    fn has(&self, mood: Mood) -> bool {
        self.tracks.get(&mood).is_some_and(|t| !t.is_empty())
    }
}

/// A track playing, and how far it's faded in (0..1).
#[derive(Component)]
struct Track {
    mood: Mood,
    level: f32,
    /// Fading out, to be despawned at 0.
    leaving: bool,
}

/// What the music is doing between frames.
#[derive(Resource)]
struct State {
    mood: Mood,
    /// When the next `Day` / `Night` track may start (seconds of app time).
    next_at: f32,
    danger_until: f32,
    /// Daylight at the last look, to hear dawn come.
    last_day: Option<f32>,
    /// Dawn came and its piece hasn't started yet.
    dawn_due: bool,
    /// A `Dawn` or `Death` piece is playing: it keeps the floor (but for danger) until it ends.
    once_playing: Option<Mood>,
    /// The death piece has played for this death.
    death_played: bool,
    last_pick: Option<usize>,
    dice: u64,
}

impl State {
    fn roll(&mut self) -> f32 {
        self.dice ^= self.dice << 13;
        self.dice ^= self.dice >> 7;
        self.dice ^= self.dice << 17;
        (self.dice >> 40) as f32 / (1u64 << 24) as f32
    }
}

pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(State { mood: Mood::Day, next_at: 0.0, danger_until: f32::NEG_INFINITY, last_day: None, dawn_due: false, once_playing: None, death_played: false, last_pick: None, dice: 0x9E37_79B9_7F4A_7C15 })
            .add_systems(Startup, load_music)
            .add_systems(PostUpdate, (choose, play).chain().run_if(resource_exists::<Music>).run_if(resource_exists::<LocalPlayer>));
    }
}

fn load_music(mut commands: Commands, options: Res<Options>, mut sources: ResMut<Assets<AudioSource>>, mut state: ResMut<State>, time: Res<Time>) {
    if options.music <= 0.0 {
        return;
    }
    let own = options.data.join("music.ron").exists();
    let data = if own { options.data.clone() } else { zombye_net::content::default_data_dir() };
    let audio = data.join("..").join("audio");
    let overlay = crate::release_audio::root();
    let game_data = data == zombye_net::content::default_data_dir();
    let path = crate::release_audio::data_file(overlay, &data, game_data, "music.ron");
    match Music::load(&path, &audio, overlay, &mut sources, options.music) {
        Ok(m) => {
            let n: usize = m.tracks.values().map(Vec::len).sum();
            if n > 0 {
                println!("music: {n} tracks");
            }
            commands.insert_resource(m);
        }
        Err(e) => eprintln!("music off: {e}"),
    }
    // A little quiet before the first piece.
    state.next_at = time.elapsed_secs() + 20.0;
}

/// Zombies on us: one attacking within 6 m, or three coming within 12 m.
pub fn in_danger(ear: Vec2, zombies: impl Iterator<Item = (Vec2, ZombieAnim)>) -> bool {
    let mut coming = 0;
    for (pos, anim) in zombies {
        let d = pos.distance(ear);
        match anim {
            ZombieAnim::Attack if d <= 6.0 => return true,
            ZombieAnim::Walk | ZombieAnim::Attack if d <= 12.0 => coming += 1,
            _ => {}
        }
    }
    coming >= 3
}

/// The mood that should be playing now, from what's going on and what the music last did.
fn choose(time: Res<Time>, local: Res<LocalPlayer>, link: Res<ServerLink>, music: Res<Music>, mut state: ResMut<State>) {
    let now = time.elapsed_secs();
    let day = zombye_sim::senses::daylight(link.curr.time_of_day);
    if !local.dead && in_danger(local.curr.pos, link.curr.zombies.iter().map(|z| (Vec2::from(z.pos), z.anim))) {
        state.danger_until = now + 8.0;
    }
    if state.last_day.is_some_and(|d| d < 0.5 && day >= 0.5) && link.curr.time_of_day < 12.0 && !local.dead {
        state.dawn_due = music.has(Mood::Dawn);
    }
    state.last_day = Some(day);
    if !local.dead {
        state.death_played = false;
    }
    let want = if local.dead {
        Mood::Death
    } else if now < state.danger_until && music.has(Mood::Danger) {
        Mood::Danger
    } else if state.dawn_due || state.once_playing == Some(Mood::Dawn) {
        Mood::Dawn
    } else if day < 0.35 {
        Mood::Night
    } else {
        Mood::Day
    };
    if want != state.mood {
        // Back to the ambient score after something else: a silence first.
        if matches!(want, Mood::Day | Mood::Night) && !matches!(state.mood, Mood::Day | Mood::Night) {
            let (lo, hi) = music.data.moods[&want].gap;
            let r = state.roll();
            state.next_at = now + (lo + (hi - lo) * r) * 0.5;
        }
        state.mood = want;
    }
}

#[allow(clippy::too_many_arguments)]
fn play(
    mut commands: Commands,
    time: Res<Time>,
    music: Res<Music>,
    settings: Option<Res<AudioSettings>>,
    local: Res<LocalPlayer>,
    link: Res<ServerLink>,
    mut state: ResMut<State>,
    mut tracks: Query<(Entity, &mut Track, Option<&mut AudioSink>)>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    let mood = state.mood;
    let asleep = link.curr.players.iter().any(|p| p.id == local.id && p.asleep);
    let master = settings.map_or(1.0, |s| s.master()) * music.level * music.data.volume * if asleep { 0.6 } else { 1.0 };

    // Fade, end and clear out.
    let mut current = None;
    for (e, mut t, sink) in &mut tracks {
        let def = &music.data.moods[&t.mood];
        if t.mood != mood && !t.leaving {
            t.leaving = true;
            if state.once_playing == Some(t.mood) {
                state.once_playing = None;
            }
        }
        let ended = sink.as_ref().is_some_and(|s| s.empty());
        if t.leaving {
            t.level -= dt / def.fade_out;
        } else {
            t.level = (t.level + dt / def.fade_in).min(1.0);
        }
        if t.level <= 0.0 || ended {
            commands.entity(e).despawn();
            if state.once_playing == Some(t.mood) {
                state.once_playing = None;
            }
            if !t.leaving {
                finished(&music, &mut state, t.mood, now);
            }
            continue;
        }
        if let Some(mut s) = sink {
            let v = t.level * def.volume * master;
            if (s.volume().to_linear() - v).abs() > 1e-4 {
                s.set_volume(Volume::Linear(v));
            }
        }
        if !t.leaving {
            current = Some(e);
        }
    }

    // Start one.
    if current.is_some() || !music.has(mood) {
        return;
    }
    let start = match mood {
        Mood::Danger => true,
        Mood::Dawn => state.dawn_due,
        Mood::Death => !state.death_played,
        Mood::Day | Mood::Night => now >= state.next_at,
    };
    if !start {
        return;
    }
    let list = &music.tracks[&mood];
    let mut pick = (state.roll() * list.len() as f32) as usize % list.len();
    if list.len() > 1 && state.last_pick == Some(pick) {
        pick = (pick + 1) % list.len();
    }
    state.last_pick = Some(pick);
    let mode = if mood == Mood::Danger { PlaybackMode::Loop } else { PlaybackMode::Once };
    commands.spawn((
        AudioPlayer(list[pick].clone()),
        PlaybackSettings { mode, volume: Volume::Linear(0.0), ..default() },
        Track { mood, level: 0.0, leaving: false },
    ));
    match mood {
        Mood::Dawn => state.dawn_due = false,
        Mood::Death => state.death_played = true,
        _ => {}
    }
    if mood.once() {
        state.once_playing = Some(mood);
    }
    if matches!(mood, Mood::Day | Mood::Night) {
        state.next_at = f32::INFINITY;
    }
}

/// A track came to its end: a silence before the next.
fn finished(music: &Music, state: &mut State, mood: Mood, now: f32) {
    let after = if mood.once() { Mood::Day } else { mood };
    let (lo, hi) = music.data.moods[&after].gap;
    let r = state.roll();
    state.next_at = now + lo + (hi - lo) * r;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> String {
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data/music.ron")).unwrap()
    }

    #[test]
    fn the_shipped_music_file_parses_and_its_files_exist() {
        let data = parse_music(&shipped()).unwrap();
        let audio = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/audio");
        let attribution = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ATTRIBUTION.md")).unwrap();
        for (mood, def) in &data.moods {
            for f in &def.files {
                assert!(audio.join(f).exists(), "{mood:?}: {f} missing");
                assert!(attribution.contains(&format!("`assets/audio/{f}`")), "{f} has no ATTRIBUTION.md row");
            }
        }
    }

    #[test]
    fn bad_music_files_are_refused() {
        let good = shipped();
        assert!(parse_music(&good.replace("Death:", "Deaths:")).is_err());
        assert!(parse_music(&good.replacen("fade_in: 4.0", "fade_in: 0.0", 1)).unwrap_err().contains("fades"));
        assert!(parse_music(&good.replacen("gap: (60.0, 180.0)", "gap: (90.0, 30.0)", 1)).unwrap_err().contains("gap"));
        assert!(parse_music(&good.replacen("volume: 0.7", "volume: 0.7, loud: true", 1)).is_err());
        assert!(parse_music(&good.replacen("files: []", "files: [\"../secret.ogg\"]", 1)).unwrap_err().contains("inside assets/audio"));
        assert!(parse_music(&good.replacen("files: []", "files: [\"music/a.mp3\"]", 1)).unwrap_err().contains("must be .ogg"));
        assert!(parse_music(&good.replacen("files: []", "files: [\"music/a.wav\"]", 1)).unwrap_err().contains("must be .ogg"));
        assert!(parse_music(&good.replacen("files: []", "files: [\"music/night_1.ogg\"]", 1)).is_ok());
    }

    #[test]
    fn danger_is_one_attacking_close_or_three_coming() {
        let ear = Vec2::ZERO;
        assert!(in_danger(ear, [(Vec2::new(5.0, 0.0), ZombieAnim::Attack)].into_iter()));
        assert!(!in_danger(ear, [(Vec2::new(8.0, 0.0), ZombieAnim::Attack)].into_iter()));
        let two = [(Vec2::new(10.0, 0.0), ZombieAnim::Walk), (Vec2::new(0.0, 9.0), ZombieAnim::Walk)];
        assert!(!in_danger(ear, two.into_iter()));
        let three = [(Vec2::new(10.0, 0.0), ZombieAnim::Walk), (Vec2::new(0.0, 9.0), ZombieAnim::Walk), (Vec2::new(-11.0, 0.0), ZombieAnim::Attack)];
        assert!(in_danger(ear, three.into_iter()));
        let idle = [(Vec2::new(1.0, 0.0), ZombieAnim::Idle), (Vec2::new(2.0, 0.0), ZombieAnim::Down), (Vec2::new(3.0, 0.0), ZombieAnim::Dead)];
        assert!(!in_danger(ear, idle.into_iter()));
    }
}
