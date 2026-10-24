//! Sound (#16): the client plays what it can tell happened from the snapshots, at the place it
//! happened, from the listener's point of view (our survivor; panned by the camera's right).
//!
//! - **What plays** is data: `assets/data/sounds.ron` maps each [`Cue`] to its files, volume, pitch
//!   range, reach, how much gets through a wall and how many may play at once. The files are
//!   WAVs in `assets/audio/` (placeholders from `tools/audio/synth.py` until the real ones, #78).
//! - **Where from:** footsteps from players' movement (hard or soft by the floor; quieter
//!   sneaking), zombies shuffling and groaning (more often when attacking or battering), doors and
//!   windows opening, shutting and breaking (the server's edge changes), battering (edge damage),
//!   hits, kills, shoves, stomps and wounds (combat events), swings (an action starting, timed to
//!   land with it), searching and picking up, a level-up, and ambience (wind, room tone, crickets
//!   at night, birds by day).
//! - **How:** each sound falls off with the square of the distance to its reach, is low-passed and
//!   quieter when a wall, shut door or window is in the way (`vision::clear_reach`), and is panned
//!   left or right. We decode the WAVs ourselves and play them through our own source ([`Voice`]),
//!   which does the panning, pitch, muffling and looping.
//! - Client only: nothing here touches the sim, and a quickload's door changes and cracks make no
//!   sound (nobody did them). `--mute`, `--volume V` and M (mute) control it; offscreen renders
//!   have no audio at all.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bevy::audio::{AddAudioSource, AudioSink, AudioSinkPlayback, ChannelCount, Decodable, PlaybackMode, SampleRate, Source, Volume};
use bevy::prelude::*;
use serde::Deserialize;
use zombye_sim::map::{Edge, Floor, TileMap};
use zombye_sim::{ActionKind, CombatEventKind, JobKind, Stance, WeaponCategory, ZombieAnim};

use crate::Options;
use crate::camera::IsoCamera;
use crate::fx::ClearFx;
use crate::player::LocalPlayer;
use crate::server_link::{ClientMap, CombatEventsThisFrame, EdgeDamageThisFrame, EdgeTransitionsThisFrame, JoinInfo, LevelUpsThisFrame, ServerLink};

/// Every sound the client knows how to play. `sounds.ron` gives each one its files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub enum Cue {
    FootstepHard,
    FootstepSoft,
    Shuffle,
    SwingLight,
    SwingHeavy,
    HitBlunt,
    HitBlade,
    Punch,
    BodyFall,
    Stomp,
    Shove,
    PlayerHurt,
    Groan,
    DoorOpen,
    DoorClose,
    WindowOpen,
    WindowClose,
    GlassBreak,
    DoorBreak,
    ThumpDoor,
    ThumpWindow,
    Rummage,
    Pickup,
    LevelUp,
    Bird,
    Wind,
    Room,
    Crickets,
}

impl Cue {
    pub const ALL: [Cue; 28] = [
        Cue::FootstepHard,
        Cue::FootstepSoft,
        Cue::Shuffle,
        Cue::SwingLight,
        Cue::SwingHeavy,
        Cue::HitBlunt,
        Cue::HitBlade,
        Cue::Punch,
        Cue::BodyFall,
        Cue::Stomp,
        Cue::Shove,
        Cue::PlayerHurt,
        Cue::Groan,
        Cue::DoorOpen,
        Cue::DoorClose,
        Cue::WindowOpen,
        Cue::WindowClose,
        Cue::GlassBreak,
        Cue::DoorBreak,
        Cue::ThumpDoor,
        Cue::ThumpWindow,
        Cue::Rummage,
        Cue::Pickup,
        Cue::LevelUp,
        Cue::Bird,
        Cue::Wind,
        Cue::Room,
        Cue::Crickets,
    ];

    /// The ambience loops, played for as long as the game runs at a volume that follows where you are.
    pub const LOOPS: [Cue; 3] = [Cue::Wind, Cue::Room, Cue::Crickets];
}

/// One cue's entry in `sounds.ron`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoundDef {
    /// WAV files under `assets/audio/`; one is picked at random each time.
    pub files: Vec<String>,
    #[serde(default = "one")]
    pub volume: f32,
    /// Playback speed range (pitch); picked each time.
    #[serde(default = "no_pitch")]
    pub pitch: (f32, f32),
    /// Metres at which it fades to nothing.
    pub reach: f32,
    /// How much of the volume gets through a wall, shut door or window (and it's low-passed).
    #[serde(default = "default_muffle")]
    pub muffle: f32,
    /// At most this many playing at once.
    #[serde(default = "four")]
    pub voices: u8,
}

fn one() -> f32 {
    1.0
}
fn no_pitch() -> (f32, f32) {
    (1.0, 1.0)
}
fn default_muffle() -> f32 {
    0.35
}
fn four() -> u8 {
    4
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoundData {
    pub sounds: BTreeMap<Cue, SoundDef>,
}

/// Parse and check `sounds.ron`. Every cue must be there (a missing one would be a silent hole).
pub fn parse_sounds(text: &str) -> Result<SoundData, String> {
    let data: SoundData = ron::from_str(text).map_err(|e| format!("sounds.ron: {e}"))?;
    for cue in Cue::ALL {
        if !data.sounds.contains_key(&cue) {
            return Err(format!("sounds.ron: no entry for {cue:?}"));
        }
    }
    for (cue, d) in &data.sounds {
        let bad = |what: &str| Err(format!("sounds.ron: {cue:?}: {what}"));
        if d.files.is_empty() {
            return bad("no files");
        }
        if !(0.0..=2.0).contains(&d.volume) {
            return bad("volume must be 0 to 2");
        }
        if !(0.25..=4.0).contains(&d.pitch.0) || !(0.25..=4.0).contains(&d.pitch.1) || d.pitch.0 > d.pitch.1 {
            return bad("pitch must be a range within 0.25 to 4, low first");
        }
        if !(d.reach.is_finite() && d.reach > 0.0 && d.reach <= 500.0) {
            return bad("reach must be over 0 and at most 500 m");
        }
        if !(0.0..=1.0).contains(&d.muffle) {
            return bad("muffle must be 0 to 1");
        }
        if d.voices == 0 {
            return bad("voices must be at least 1");
        }
        if d.files.iter().any(|f| f.contains("..") || Path::new(f).is_absolute()) {
            return bad("files must be paths inside assets/audio");
        }
    }
    Ok(data)
}

/// A decoded sound: mono samples at their rate.
#[derive(Clone, Debug)]
pub struct Clip {
    samples: Arc<[f32]>,
    rate: u32,
}

/// Read a 16-bit PCM WAV (mono, or stereo mixed down) into a [`Clip`].
pub fn parse_wav(bytes: &[u8]) -> Result<Clip, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let (mut fmt, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let len = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]]) as usize;
        let body = bytes.get(at + 8..at + 8 + len).ok_or("a chunk runs past the end")?;
        match &bytes[at..at + 4] {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        at += 8 + len + (len & 1);
    }
    let fmt = fmt.filter(|f| f.len() >= 16).ok_or("no fmt chunk")?;
    let u16_at = |i: usize| u16::from_le_bytes([fmt[i], fmt[i + 1]]);
    let (format, channels, bits) = (u16_at(0), u16_at(2) as usize, u16_at(14));
    let rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
    if format != 1 || bits != 16 || !(1..=2).contains(&channels) || !(8_000..=96_000).contains(&rate) {
        return Err("only 16-bit PCM, mono or stereo, 8-96 kHz".into());
    }
    let data = data.ok_or("no data chunk")?;
    let samples: Vec<f32> = data
        .chunks_exact(2 * channels)
        .map(|frame| frame.as_chunks::<2>().0.iter().map(|s| i16::from_le_bytes(*s) as f32 / 32768.0).sum::<f32>() / channels as f32)
        .collect();
    if samples.len() < 2 {
        return Err("no samples".into());
    }
    Ok(Clip { samples: samples.into(), rate })
}

/// The cues, their settings and their decoded clips.
#[derive(Resource)]
pub struct SoundBank {
    defs: BTreeMap<Cue, SoundDef>,
    clips: BTreeMap<Cue, Vec<Clip>>,
}

impl SoundBank {
    /// Load `sounds.ron` from `data` and its files from `audio`.
    pub fn load(data: &Path, audio: &Path) -> Result<Self, String> {
        let path = data.join("sounds.ron");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let defs = parse_sounds(&text)?.sounds;
        let mut clips = BTreeMap::new();
        for (cue, def) in &defs {
            let mut list = Vec::new();
            for file in &def.files {
                let bytes = std::fs::read(audio.join(file)).map_err(|e| format!("{cue:?}: {file}: {e}"))?;
                list.push(parse_wav(&bytes).map_err(|e| format!("{cue:?}: {file}: {e}"))?);
            }
            clips.insert(*cue, list);
        }
        Ok(Self { defs, clips })
    }

    pub fn def(&self, cue: Cue) -> &SoundDef {
        &self.defs[&cue]
    }
}

/// Where `sounds.ron` and the audio files are: the `--data` folder if it has its own, else the
/// game's. Files are found next to the data folder, in `audio/`.
fn bank_dirs(options: &Options) -> (PathBuf, PathBuf) {
    let data = if options.data.join("sounds.ron").exists() { options.data.clone() } else { zombye_net::content::default_data_dir() };
    let audio = data.join("..").join("audio");
    (data, audio)
}

/// One sound being played: our own source, so we choose the panning, pitch and muffling.
#[derive(Asset, TypePath)]
pub struct Voice {
    clip: Clip,
    left: f32,
    right: f32,
    speed: f32,
    muffled: bool,
    looped: bool,
}

/// Low-pass corner for a sound heard through a wall.
const MUFFLE_HZ: f32 = 700.0;

pub struct VoiceDecoder {
    samples: Arc<[f32]>,
    rate: SampleRate,
    pos: f64,
    step: f64,
    left: f32,
    right: f32,
    /// Two one-pole low-passes in a row: (coefficient, first, second).
    lowpass: Option<(f32, f32, f32)>,
    looped: bool,
    /// The right channel's sample, after the left one has gone out.
    right_next: Option<f32>,
}

impl Iterator for VoiceDecoder {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if let Some(r) = self.right_next.take() {
            return Some(r);
        }
        let last = (self.samples.len() - 1) as f64;
        if self.pos >= last {
            if !self.looped {
                return None;
            }
            self.pos %= last;
        }
        let i = self.pos as usize;
        let f = (self.pos - i as f64) as f32;
        let mut x = self.samples[i] + (self.samples[i + 1] - self.samples[i]) * f;
        self.pos += self.step;
        if let Some((a, s1, s2)) = &mut self.lowpass {
            *s1 += *a * (x - *s1);
            *s2 += *a * (*s1 - *s2);
            x = *s2;
        }
        self.right_next = Some(x * self.right);
        Some(x * self.left)
    }
}

impl Source for VoiceDecoder {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(2).expect("two")
    }

    fn sample_rate(&self) -> SampleRate {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        (!self.looped).then(|| Duration::from_secs_f64(self.samples.len() as f64 / self.step / self.rate.get() as f64))
    }
}

impl Decodable for Voice {
    type Decoder = VoiceDecoder;

    fn decoder(&self) -> VoiceDecoder {
        let rate = self.clip.rate;
        let a = 1.0 - (-2.0 * std::f32::consts::PI * MUFFLE_HZ / rate as f32).exp();
        VoiceDecoder {
            samples: self.clip.samples.clone(),
            rate: SampleRate::new(rate).expect("checked when loaded"),
            pos: 0.0,
            step: self.speed as f64,
            left: self.left,
            right: self.right,
            lowpass: self.muffled.then_some((a, 0.0, 0.0)),
            looped: self.looped,
            right_next: None,
        }
    }
}

/// How loud a sound `at` is to a listener at `ear` whose right is `right` (a unit vector on the
/// ground), and its pan (-1 left to 1 right); `None` beyond `reach`. Falls off with the square.
pub fn placement(ear: Vec2, right: Vec2, at: Vec2, reach: f32) -> Option<(f32, f32)> {
    let d = ear.distance(at);
    if d >= reach {
        return None;
    }
    let near = 1.0 - d / reach;
    // Right on top of us: in both ears. Further off: toward its side, never all the way.
    let pan = if d < 0.5 { 0.0 } else { ((at - ear) / d).dot(right).clamp(-1.0, 1.0) * 0.6 * (d / 2.0).min(1.0) };
    Some((near * near, pan))
}

/// Equal-power gains for a pan in -1..1: (1, 1) in the middle.
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let theta = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    let k = std::f32::consts::SQRT_2;
    (theta.cos() * k, theta.sin() * k)
}

/// The sound of a door or window going from `old` to `new`, if it makes one.
pub fn edge_cue(old: Edge, new: Edge) -> Option<Cue> {
    match (old, new) {
        _ if old == new => None,
        (Edge::DoorClosed, Edge::DoorOpen) => Some(Cue::DoorOpen),
        (Edge::DoorOpen, Edge::DoorClosed) => Some(Cue::DoorClose),
        (_, Edge::DoorBroken) => Some(Cue::DoorBreak),
        (Edge::Window, Edge::WindowOpen) => Some(Cue::WindowOpen),
        (Edge::WindowOpen, Edge::Window) => Some(Cue::WindowClose),
        (_, Edge::WindowBroken) => Some(Cue::GlassBreak),
        _ => None,
    }
}

/// A footstep on this floor: which cue and how loud.
pub fn footstep(floor: Floor) -> (Cue, f32) {
    match floor {
        Floor::Grass | Floor::Dirt | Floor::Path => (Cue::FootstepSoft, 1.0),
        Floor::Carpet => (Cue::FootstepSoft, 0.6),
        Floor::Asphalt | Floor::RoadLine | Floor::Sidewalk | Floor::Wood | Floor::TileFloor | Floor::Concrete => (Cue::FootstepHard, 1.0),
    }
}

/// Metres a step and how loud, by stance.
pub fn stride(stance: Stance) -> (f32, f32) {
    match stance {
        Stance::Walk => (0.8, 0.7),
        Stance::Run => (1.3, 1.0),
        Stance::Sneak => (0.55, 0.3),
    }
}

/// Distance walked since the last step: true when another stride is done. A jump of more than
/// a few metres (a respawn, a quickload) starts again without a step.
pub fn stepped(track: &mut (Vec2, f32), pos: Vec2, stride: f32) -> bool {
    let d = pos.distance(track.0);
    track.0 = pos;
    if d > 3.0 {
        track.1 = 0.0;
        return false;
    }
    track.1 += d;
    if track.1 >= stride {
        track.1 = (track.1 - stride).min(stride * 0.5);
        true
    } else {
        false
    }
}

/// Where a sound is.
#[derive(Clone, Copy, Debug)]
enum Where {
    /// In the world: falls off, may be muffled and is panned.
    At(Vec2),
    /// Around the listener (ambience, interface): as loud as it is, panned this much.
    Around(f32),
}

#[derive(Clone, Copy, Debug)]
struct Request {
    cue: Cue,
    at: Where,
    gain: f32,
    /// Seconds from now.
    delay: f32,
}

/// Sounds asked for this frame, and those waiting for their moment.
#[derive(Resource, Default)]
struct Queue {
    now: Vec<Request>,
    later: Vec<Request>,
}

impl Queue {
    fn at(&mut self, cue: Cue, pos: Vec2, gain: f32) {
        self.now.push(Request { cue, at: Where::At(pos), gain, delay: 0.0 });
    }

    fn at_later(&mut self, cue: Cue, pos: Vec2, gain: f32, delay: f32) {
        self.later.push(Request { cue, at: Where::At(pos), gain, delay });
    }

    fn around(&mut self, cue: Cue, pan: f32, gain: f32) {
        self.now.push(Request { cue, at: Where::Around(pan), gain, delay: 0.0 });
    }
}

/// Our ears: the survivor we play, and the camera's right on the ground (sim coordinates).
#[derive(Resource)]
struct Listener {
    pos: Vec2,
    right: Vec2,
    asleep: bool,
    indoors: bool,
}

/// Master volume and mute (`--volume`, `--mute`, M).
#[derive(Resource)]
pub struct AudioSettings {
    pub volume: f32,
    pub muted: bool,
}

impl AudioSettings {
    fn master(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume }
    }
}

/// The one-shots started since the HUD's last stats line, by cue (so a run's stdout shows what was
/// heard: `sounds 9 (Groan 4, FootstepHard 3, DoorOpen 1, Shuffle 1)`).
#[derive(Resource, Default)]
pub struct SoundCount {
    played: BTreeMap<Cue, u32>,
    muted: bool,
}

impl SoundCount {
    /// The stats line's part, and start counting again.
    pub fn take_line(&mut self) -> String {
        if self.muted {
            return "sound off".into();
        }
        let mut by: Vec<(Cue, u32)> = std::mem::take(&mut self.played).into_iter().collect();
        by.sort_by_key(|&(cue, n)| (std::cmp::Reverse(n), cue));
        let total: u32 = by.iter().map(|b| b.1).sum();
        if total == 0 {
            return "sounds 0".into();
        }
        let most: Vec<String> = by.iter().take(4).map(|(cue, n)| format!("{cue:?} {n}")).collect();
        format!("sounds {total} ({})", most.join(", "))
    }
}

/// A one-shot that's playing (for the voice limits).
#[derive(Component)]
struct Playing(Cue);

/// An ambience loop and its current volume (eased toward where it should be).
#[derive(Component)]
struct Ambient {
    cue: Cue,
    level: f32,
}

/// The client's own dice for sound (pitch, variants, groan timing). Not the sim's.
#[derive(Resource)]
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// What the sound systems remember between frames.
#[derive(Resource, Default)]
struct Track {
    player_steps: HashMap<u32, (Vec2, f32)>,
    zombie_steps: HashMap<u32, (Vec2, f32)>,
    groan_at: HashMap<u32, f32>,
    /// Each player's action and its seconds left at the last snapshot, to see a new one start.
    actions: HashMap<u32, (Option<ActionKind>, f32)>,
    /// Ground items at the last snapshot, to hear one being picked up.
    items: HashMap<u32, Vec2>,
    last_tick: u64,
    rummage_in: f32,
    bird_in: f32,
}

pub struct GameAudioPlugin;

impl Plugin for GameAudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_audio_source::<Voice>()
            .init_resource::<Queue>()
            .init_resource::<Track>()
            .init_resource::<SoundCount>()
            .insert_resource(Dice(0x2545_F491_4F6C_DD1D))
            .insert_resource(Listener { pos: Vec2::ZERO, right: Vec2::X, asleep: false, indoors: false })
            .add_systems(Startup, load_bank)
            .add_systems(Update, toggle_mute)
            .add_systems(
                PostUpdate,
                (listen, footsteps, zombie_voices, world_events, ambience, flush).chain().run_if(resource_exists::<SoundBank>).run_if(resource_exists::<LocalPlayer>),
            );
    }
}

fn load_bank(mut commands: Commands, options: Res<Options>) {
    let (data, audio) = bank_dirs(&options);
    commands.insert_resource(AudioSettings { volume: options.volume, muted: options.mute });
    match SoundBank::load(&data, &audio) {
        Ok(bank) => commands.insert_resource(bank),
        Err(e) => eprintln!("sound off: {e}"),
    }
}

fn toggle_mute(keys: Res<ButtonInput<KeyCode>>, settings: Option<ResMut<AudioSettings>>) {
    if let Some(mut s) = settings
        && keys.just_pressed(KeyCode::KeyM)
    {
        s.muted = !s.muted;
        println!("sound {}", if s.muted { "off" } else { "on" });
    }
}

fn listen(local: Res<LocalPlayer>, link: Res<ServerLink>, map: Res<ClientMap>, cams: Query<&IsoCamera>, mut ear: ResMut<Listener>) {
    ear.pos = local.curr.pos;
    ear.asleep = link.curr.players.iter().find(|p| p.id == local.id).is_some_and(|p| p.asleep);
    ear.indoors = map.0.room_at(ear.pos) != 0;
    if let Ok(cam) = cams.single() {
        let (_, right) = cam.ground_axes();
        ear.right = Vec2::new(right.x, right.z).normalize_or(Vec2::X);
    }
}

fn floor_at(map: &TileMap, pos: Vec2) -> Floor {
    map.tile(pos.floor().as_ivec2()).floor
}

fn footsteps(local: Res<LocalPlayer>, link: Res<ServerLink>, map: Res<ClientMap>, mut track: ResMut<Track>, mut queue: ResMut<Queue>) {
    let track = &mut *track;
    // Our own steps follow the predicted position; everyone else's the latest snapshot.
    let mut walkers: Vec<(u32, Vec2, Stance)> = vec![(local.id, local.curr.pos, local.curr.stance)];
    walkers.extend(link.curr.players.iter().filter(|p| p.id != local.id && !p.dead && !p.asleep).map(|p| (p.id, Vec2::from(p.pos), p.stance)));
    let me_down = local.dead || link.curr.players.iter().any(|p| p.id == local.id && p.asleep);
    for (id, pos, stance) in walkers {
        let (len, loud) = stride(stance);
        let t = track.player_steps.entry(id).or_insert((pos, len * 0.5));
        if stepped(t, pos, len) && !(id == local.id && me_down) && !map.0.in_window_opening(pos) {
            let (cue, floor_loud) = footstep(floor_at(&map.0, pos));
            queue.at(cue, pos, loud * floor_loud);
        }
    }
    // Zombies close by drag their feet.
    let ear = local.curr.pos;
    let mut seen = HashSet::new();
    for z in &link.curr.zombies {
        let pos = Vec2::from(z.pos);
        seen.insert(z.id);
        if z.anim != ZombieAnim::Walk || pos.distance(ear) > 16.0 {
            track.zombie_steps.remove(&z.id);
            continue;
        }
        let t = track.zombie_steps.entry(z.id).or_insert((pos, 0.4));
        if stepped(t, pos, 0.8) {
            queue.at(Cue::Shuffle, pos, 1.0);
        }
    }
    track.zombie_steps.retain(|id, _| seen.contains(id));
}

/// Seconds between groans, by what a zombie is doing; `None`: it doesn't groan now.
fn groan_gap(anim: ZombieAnim) -> Option<(f32, f32)> {
    match anim {
        ZombieAnim::Attack | ZombieAnim::Thump => Some((2.0, 5.0)),
        ZombieAnim::Walk => Some((4.0, 10.0)),
        ZombieAnim::Idle => Some((8.0, 22.0)),
        _ => None,
    }
}

fn zombie_voices(time: Res<Time>, ear: Res<Listener>, link: Res<ServerLink>, bank: Res<SoundBank>, mut track: ResMut<Track>, mut dice: ResMut<Dice>, mut queue: ResMut<Queue>) {
    let now = time.elapsed_secs();
    let reach = bank.def(Cue::Groan).reach;
    let mut seen = HashSet::new();
    for z in &link.curr.zombies {
        let pos = Vec2::from(z.pos);
        if pos.distance(ear.pos) > reach {
            continue;
        }
        seen.insert(z.id);
        let Some((lo, hi)) = groan_gap(z.anim) else {
            // Down or reeling: not now, but soon after.
            track.groan_at.entry(z.id).and_modify(|t| *t = t.max(now + 1.0));
            continue;
        };
        // First heard: somewhere in its gap, so a crowd doesn't groan in chorus.
        let first = now + dice.range(0.0, hi);
        let at = *track.groan_at.entry(z.id).or_insert(first);
        if now >= at {
            queue.at(Cue::Groan, pos, dice.range(0.75, 1.1));
            track.groan_at.insert(z.id, now + dice.range(lo, hi));
        }
    }
    track.groan_at.retain(|id, _| seen.contains(id));
}

#[allow(clippy::too_many_arguments)]
fn world_events(
    time: Res<Time>,
    map: Res<ClientMap>,
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    local: Res<LocalPlayer>,
    transitions: Res<EdgeTransitionsThisFrame>,
    damage: Res<EdgeDamageThisFrame>,
    hits: Res<CombatEventsThisFrame>,
    ups: Res<LevelUpsThisFrame>,
    mut swapped: MessageReader<ClearFx>,
    mut track: ResMut<Track>,
    mut dice: ResMut<Dice>,
    mut queue: ResMut<Queue>,
) {
    let track = &mut *track;
    let snap = &link.curr;
    let category = |player: u32| {
        snap.players.iter().find(|p| p.id == player).and_then(|p| p.weapon).and_then(|w| join.weapons.get(w as usize)).map_or(WeaponCategory::Unarmed, |w| w.category)
    };
    // A quickload (or a new run) swapped the world: what differs wasn't done by anyone.
    if swapped.read().count() > 0 {
        track.items = snap.items.iter().map(|i| (i.id, i.pos)).collect();
        track.actions.clear();
        track.player_steps.clear();
        track.last_tick = snap.tick;
        return;
    }
    for (change, old) in &transitions.0 {
        if let Some(cue) = edge_cue(*old, change.kind) {
            let (a, b) = TileMap::edge_segment(change.tile, change.edge);
            queue.at(cue, (a + b) / 2.0, 1.0);
        }
    }
    let mut thumped = HashSet::new();
    for d in &damage.0 {
        let kind = map.0.owned_edge(d.tile, d.edge);
        if d.hp < 1.0 && thumped.insert((d.tile, d.edge)) {
            let cue = if kind.is_window() { Cue::ThumpWindow } else { Cue::ThumpDoor };
            let (a, b) = TileMap::edge_segment(d.tile, d.edge);
            queue.at(cue, (a + b) / 2.0, 1.0);
        }
    }
    let mut shoved = HashSet::new();
    for e in &hits.0 {
        let stomping = snap.players.iter().any(|p| p.id == e.player && p.action == Some(ActionKind::Stomp));
        let hit = match category(e.player) {
            WeaponCategory::Unarmed => Cue::Punch,
            WeaponCategory::ShortBlade | WeaponCategory::Axe => Cue::HitBlade,
            WeaponCategory::ShortBlunt | WeaponCategory::LongBlunt => Cue::HitBlunt,
        };
        let crit = if e.crit { 1.25 } else { 1.0 };
        match e.kind {
            CombatEventKind::ZombieHit => queue.at(hit, e.pos, crit),
            CombatEventKind::ZombieKilled if stomping => queue.at(Cue::Stomp, e.pos, 1.15),
            CombatEventKind::ZombieKilled => {
                queue.at(hit, e.pos, crit);
                queue.at_later(Cue::BodyFall, e.pos, 1.0, dice.range(0.22, 0.35));
            }
            CombatEventKind::Stomp => queue.at(Cue::Stomp, e.pos, 1.0),
            CombatEventKind::Shove if shoved.insert(e.player) => queue.at(Cue::Shove, e.pos, 1.0),
            CombatEventKind::Shove => {}
            CombatEventKind::PlayerHurt => queue.at(Cue::PlayerHurt, e.pos, 1.0),
        }
    }
    if snap.tick != track.last_tick {
        track.last_tick = snap.tick;
        // A swing starting: its whoosh comes just before the blow lands.
        for p in &snap.players {
            let before = track.actions.insert(p.id, (p.action, p.action_left));
            let fresh = match before {
                Some((was, left)) => was != p.action || p.action_left > left + 0.05,
                None => true,
            };
            if p.action == Some(ActionKind::Swing) && fresh {
                let heavy = matches!(category(p.id), WeaponCategory::LongBlunt | WeaponCategory::Axe);
                let cue = if heavy { Cue::SwingHeavy } else { Cue::SwingLight };
                queue.at_later(cue, Vec2::from(p.pos), 1.0, (p.action_left - 0.12).max(0.0));
            }
        }
        track.actions.retain(|id, _| snap.players.iter().any(|p| p.id == *id));
        // An item gone from the floor next to someone: picked up.
        let now: HashMap<u32, Vec2> = snap.items.iter().map(|i| (i.id, i.pos)).collect();
        for (id, pos) in &track.items {
            if !now.contains_key(id) && snap.players.iter().any(|p| !p.dead && Vec2::from(p.pos).distance(*pos) < 2.0) {
                queue.at(Cue::Pickup, *pos, 1.0);
            }
        }
        track.items = now;
    }
    // Searching a container: rummaging until it's done.
    let me = snap.players.iter().find(|p| p.id == local.id);
    let searching = me.is_some_and(|p| p.busy > 0.0) && link.own(local.id).is_some_and(|v| matches!(v.jobs.first(), Some(JobKind::Search { .. })));
    track.rummage_in -= time.delta_secs();
    if searching && track.rummage_in <= 0.0 {
        queue.at(Cue::Rummage, local.curr.pos, 1.0);
        track.rummage_in = dice.range(0.45, 0.6);
    } else if !searching {
        track.rummage_in = 0.0;
    }
    if ups.0.iter().any(|u| u.player == local.id) {
        queue.around(Cue::LevelUp, 0.0, 1.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn ambience(
    mut commands: Commands,
    time: Res<Time>,
    ear: Res<Listener>,
    link: Res<ServerLink>,
    bank: Res<SoundBank>,
    settings: Res<AudioSettings>,
    mut voices: ResMut<Assets<Voice>>,
    mut loops: Query<(&mut Ambient, Option<&mut AudioSink>)>,
    mut track: ResMut<Track>,
    mut dice: ResMut<Dice>,
    mut queue: ResMut<Queue>,
) {
    if loops.is_empty() {
        for cue in Cue::LOOPS {
            let voice = Voice { clip: bank.clips[&cue][0].clone(), left: 1.0, right: 1.0, speed: 1.0, muffled: false, looped: true };
            let settings = PlaybackSettings { mode: PlaybackMode::Once, volume: Volume::Linear(0.0), ..default() };
            commands.spawn((AudioPlayer(voices.add(voice)), settings, Ambient { cue, level: 0.0 }));
        }
        return;
    }
    let day = zombye_sim::senses::daylight(link.curr.time_of_day);
    let night = 1.0 - day;
    let out = if ear.indoors { 0.0 } else { 1.0 };
    let sleep = if ear.asleep { 0.5 } else { 1.0 };
    let ease = (time.delta_secs() * 1.5).min(1.0);
    for (mut amb, sink) in &mut loops {
        let target = match amb.cue {
            Cue::Wind => 0.3 + 0.7 * out,
            Cue::Room => 1.0 - out,
            Cue::Crickets => night * (0.35 + 0.65 * out),
            _ => 0.0,
        } * bank.def(amb.cue).volume
            * sleep;
        amb.level += (target - amb.level) * ease;
        if let Some(mut sink) = sink {
            sink.set_volume(Volume::Linear(amb.level * settings.master()));
        }
    }
    // Birds now and then by day, outdoors, somewhere around.
    track.bird_in -= time.delta_secs();
    if track.bird_in <= 0.0 {
        track.bird_in = dice.range(5.0, 14.0);
        if day > 0.5 && !ear.indoors {
            queue.around(Cue::Bird, dice.range(-0.8, 0.8), dice.range(0.6, 1.0));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn flush(
    mut commands: Commands,
    time: Res<Time>,
    ear: Res<Listener>,
    map: Res<ClientMap>,
    bank: Res<SoundBank>,
    settings: Res<AudioSettings>,
    playing: Query<&Playing>,
    mut voices: ResMut<Assets<Voice>>,
    mut dice: ResMut<Dice>,
    mut queue: ResMut<Queue>,
    mut counted: ResMut<SoundCount>,
) {
    let queue = &mut *queue;
    counted.muted = settings.muted;
    let dt = time.delta_secs();
    let mut due = std::mem::take(&mut queue.now);
    queue.later.retain_mut(|r| {
        r.delay -= dt;
        let now = r.delay <= 0.0;
        if now {
            due.push(*r);
        }
        !now
    });
    let master = settings.master() * if ear.asleep { 0.35 } else { 1.0 };
    if master <= 0.0 {
        return;
    }
    // Work out each one's loudness, pan and muffling; the nearest go first.
    let mut ready: Vec<(f32, Cue, f32, f32, bool)> = due
        .iter()
        .filter_map(|r| {
            let def = bank.def(r.cue);
            let (dist, (near, pan), muffled) = match r.at {
                Where::At(pos) => (ear.pos.distance(pos), placement(ear.pos, ear.right, pos, def.reach)?, !zombye_sim::vision::clear_reach(&map.0, ear.pos, pos)),
                Where::Around(pan) => (0.0, (1.0, pan), false),
            };
            let volume = def.volume * r.gain * near * if muffled { def.muffle } else { 1.0 } * master;
            (volume > 0.003).then_some((dist, r.cue, volume, pan, muffled))
        })
        .collect();
    ready.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut count: HashMap<Cue, u8> = HashMap::new();
    for p in &playing {
        *count.entry(p.0).or_default() += 1;
    }
    for (_, cue, volume, pan, muffled) in ready {
        let def = bank.def(cue);
        let n = count.entry(cue).or_default();
        if *n >= def.voices {
            continue;
        }
        *n += 1;
        *counted.played.entry(cue).or_default() += 1;
        let clips = &bank.clips[&cue];
        let clip = clips[((dice.next() * clips.len() as f32) as usize).min(clips.len() - 1)].clone();
        let (left, right) = pan_gains(pan);
        let voice = Voice { clip, left, right, speed: dice.range(def.pitch.0, def.pitch.1), muffled, looped: false };
        let settings = PlaybackSettings { mode: PlaybackMode::Despawn, volume: Volume::Linear(volume), ..default() };
        commands.spawn((AudioPlayer(voices.add(voice)), settings, Playing(cue)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn the_shipped_sounds_load() {
        let bank = SoundBank::load(&repo().join("assets/data"), &repo().join("assets/audio")).unwrap();
        for cue in Cue::ALL {
            let clips = &bank.clips[&cue];
            assert!(!clips.is_empty(), "{cue:?}");
            assert!(clips.iter().all(|c| c.samples.len() > 100 && c.rate == 22_050), "{cue:?}");
        }
    }

    #[test]
    fn every_audio_file_is_attributed_and_used() {
        let attribution = std::fs::read_to_string(repo().join("ATTRIBUTION.md")).unwrap();
        let bank = parse_sounds(&std::fs::read_to_string(repo().join("assets/data/sounds.ron")).unwrap()).unwrap();
        let used: HashSet<&String> = bank.sounds.values().flat_map(|d| &d.files).collect();
        let mut n = 0;
        for dir in ["sfx", "ambience"] {
            for entry in std::fs::read_dir(repo().join("assets/audio").join(dir)).unwrap() {
                let name = entry.unwrap().file_name().into_string().unwrap();
                let rel = format!("{dir}/{name}");
                assert!(attribution.contains(&format!("`assets/audio/{rel}`")), "{rel} has no ATTRIBUTION.md row");
                assert!(used.contains(&rel), "{rel} isn't used in sounds.ron");
                n += 1;
            }
        }
        assert!(n >= Cue::ALL.len());
    }

    #[test]
    fn sounds_ron_mistakes_are_refused() {
        let good = std::fs::read_to_string(repo().join("assets/data/sounds.ron")).unwrap();
        assert!(parse_sounds(&good).is_ok());
        assert!(parse_sounds(&good.replace("Crickets:", "Cricket:")).unwrap_err().contains("Cricket"));
        assert!(parse_sounds(&good.replace("volume: 0.35", "volume: 3.0")).unwrap_err().contains("FootstepHard: volume"));
        assert!(parse_sounds(&good.replace("pitch: (0.9, 1.1), reach: 25.0", "pitch: (1.1, 0.9), reach: 25.0")).unwrap_err().contains("pitch"));
        assert!(parse_sounds(&good.replace("\"sfx/pickup.wav\"", "\"../secret.wav\"")).unwrap_err().contains("inside assets/audio"));
        assert!(parse_sounds(&good.replace("reach: 10.0, voices: 2),\n        LevelUp", "reach: 10.0, voices: 2, loud: true),\n        LevelUp")).is_err());
    }

    #[test]
    fn wav_files_are_read_and_junk_refused() {
        // A tiny hand-made WAV: mono 16-bit at 22,050 Hz, four samples.
        let data: Vec<u8> = [0i16, 16384, -16384, 32767].iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        wav.extend_from_slice(&16u32.to_le_bytes());
        for v in [1u16, 1] {
            wav.extend_from_slice(&v.to_le_bytes());
        }
        wav.extend_from_slice(&22_050u32.to_le_bytes());
        wav.extend_from_slice(&44_100u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        let clip = parse_wav(&wav).unwrap();
        assert_eq!(clip.rate, 22_050);
        assert_eq!(&clip.samples[..], &[0.0, 0.5, -0.5, 32767.0 / 32768.0]);
        assert!(parse_wav(b"RIFF....WAVE").is_err());
        assert!(parse_wav(b"not a wav at all").is_err());
        assert!(parse_wav(&wav[..wav.len() - 3]).is_err(), "a cut-off data chunk");
    }

    fn clip(n: usize) -> Clip {
        Clip { samples: (0..n).map(|i| (i as f32 * 0.9).sin()).collect::<Vec<_>>().into(), rate: 22_050 }
    }

    #[test]
    fn a_voice_plays_once_in_stereo_or_loops() {
        let once = Voice { clip: clip(1000), left: 1.0, right: 0.5, speed: 1.0, muffled: false, looped: false };
        let out: Vec<f32> = once.decoder().collect();
        assert_eq!(out.len(), 2 * 999, "interleaved left, right");
        assert!(out.chunks(2).all(|lr| (lr[1] - lr[0] * 0.5).abs() < 1e-6), "the right channel at half");
        let faster = Voice { speed: 2.0, ..once };
        assert_eq!(faster.decoder().count(), 2 * 500, "twice the pitch, half as long");
        let looped = Voice { looped: true, ..faster };
        assert_eq!(looped.decoder().take(10_000).count(), 10_000, "a loop doesn't end");
        assert!(looped.decoder().total_duration().is_none());
    }

    #[test]
    fn muffling_takes_the_top_off() {
        let buzz = Clip { samples: (0..4000).map(|i| if i % 2 == 0 { 1.0 } else { -1.0 }).collect::<Vec<_>>().into(), rate: 22_050 };
        let energy = |muffled| {
            let v = Voice { clip: buzz.clone(), left: 1.0, right: 1.0, speed: 1.0, muffled, looped: false };
            v.decoder().skip(1000).map(|x| x * x).sum::<f32>()
        };
        assert!(energy(true) < energy(false) * 0.01, "a high whine through a wall is nearly gone");
    }

    #[test]
    fn loudness_falls_off_and_pans_to_the_side() {
        let (ear, right) = (Vec2::new(10.0, 10.0), Vec2::X);
        assert_eq!(placement(ear, right, ear, 20.0), Some((1.0, 0.0)));
        assert!(placement(ear, right, ear + Vec2::new(20.0, 0.0), 20.0).is_none(), "out of reach");
        let (half, pan) = placement(ear, right, ear + Vec2::new(10.0, 0.0), 20.0).unwrap();
        assert!((half - 0.25).abs() < 1e-6, "half way out: a quarter as loud");
        assert!(pan > 0.5, "to the right");
        let (_, ahead) = placement(ear, right, ear + Vec2::new(0.0, 8.0), 20.0).unwrap();
        assert!(ahead.abs() < 1e-6, "straight ahead: in the middle");
        let (l, r) = pan_gains(pan);
        assert!(r > l * 2.0);
        assert_eq!(pan_gains(0.0).0, pan_gains(0.0).1);
        assert!((pan_gains(0.0).0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn doors_and_windows_make_the_right_sounds() {
        use Edge::*;
        assert_eq!(edge_cue(DoorClosed, DoorOpen), Some(Cue::DoorOpen));
        assert_eq!(edge_cue(DoorOpen, DoorClosed), Some(Cue::DoorClose));
        assert_eq!(edge_cue(DoorClosed, DoorBroken), Some(Cue::DoorBreak));
        assert_eq!(edge_cue(Window, WindowOpen), Some(Cue::WindowOpen));
        assert_eq!(edge_cue(WindowOpen, Window), Some(Cue::WindowClose));
        assert_eq!(edge_cue(Window, WindowBroken), Some(Cue::GlassBreak));
        assert_eq!(edge_cue(WindowOpen, WindowBroken), Some(Cue::GlassBreak));
        assert_eq!(edge_cue(DoorOpen, DoorOpen), Option::None);
        assert_eq!(edge_cue(Wall, Edge::None), Option::None);
    }

    #[test]
    fn steps_come_every_stride_and_not_on_a_jump() {
        let mut t = (Vec2::ZERO, 0.0);
        let steps = (1..=16).filter(|i| stepped(&mut t, Vec2::new(*i as f32 * 0.25, 0.0), 1.0)).count();
        assert_eq!(steps, 4, "4 m at 1 m a stride");
        assert!(!stepped(&mut t, Vec2::new(50.0, 0.0), 0.8), "a teleport isn't a step");
        assert_eq!(t.1, 0.0);
        assert_eq!(footstep(Floor::Grass).0, Cue::FootstepSoft);
        assert_eq!(footstep(Floor::Wood).0, Cue::FootstepHard);
        assert!(stride(Stance::Sneak).1 < stride(Stance::Walk).1 && stride(Stance::Walk).1 < stride(Stance::Run).1);
    }
}
