//! Talking to whoever is near, and a few gestures (#67, docs/PLAN.md §4.8: "alliances come from
//! play").
//!
//! **Speech** ([`World::player_say`]): a line of text at a [`Volume`]. It is heard by every other
//! living, waking survivor within that volume's radius of the speaker, by the rule zombies hear
//! by: the whole radius with a line of sight (glass doesn't block it), `wall_muffle` of it
//! through a wall or a shut door, and then [`Said::muffled`]. Who hears it is worked out the
//! moment it is said, one [`Said`] per listener (the speaker's own included, so its client shows
//! the line when the server took it), kept in [`World::speech`] until whoever replicates the
//! world takes them. Every line names its speaker and says whether the listener saw them as they
//! spoke ([`Said::seen`]); whether an unseen speaker is named to the listener is the server's
//! business (`net`: not with fair information on).
//!
//! **Speaking is a noise**: each line is also a sound at the speaker (`whisper_noise`,
//! `talk_noise`, `shout_noise` metres), a player's like its footsteps, so zombies in earshot come
//! to look, a sleeper near by wakes, and a shout (30 m, over the migration's `min_radius`) leaves
//! an attractor that draws the far horde, as breaking a door does.
//!
//! **Gestures** ([`World::player_gesture`]): wave, hands up, point. A pose and nothing more
//! ([`crate::players::Player::gesture`], in what clients are told of a player): a wave and a point
//! last `gesture_secs`, hands stay up until lowered. Starting a swing, shove or stomp, being
//! knocked about, falling asleep or dying ends one.
//!
//! The lines waiting in `World::speech` and the gestures are not saved and not in the state hash
//! (nothing in the simulation reads them); the noise of speaking is a sound like any other.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::TICK_DT;
use crate::map::TileMap;
use crate::senses::SoundSource;
use crate::world::World;

/// Lines kept for whoever replicates the world if nobody takes them.
pub const MAX_SPEECH: usize = 256;

/// How loud a line is said.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Volume {
    Whisper,
    #[default]
    Talk,
    Shout,
}

impl Volume {
    pub const ALL: [Volume; 3] = [Volume::Whisper, Volume::Talk, Volume::Shout];

    /// `whisper`, `talk` (also `say`) or `shout` (also `yell`).
    pub fn parse(s: &str) -> Option<Volume> {
        match s.trim().to_ascii_lowercase().as_str() {
            "whisper" => Some(Volume::Whisper),
            "talk" | "say" => Some(Volume::Talk),
            "shout" | "yell" => Some(Volume::Shout),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Volume::Whisper => "whisper",
            Volume::Talk => "talk",
            Volume::Shout => "shout",
        }
    }

    /// "whispers", "says", "shouts".
    pub fn verb(self) -> &'static str {
        match self {
            Volume::Whisper => "whispers",
            Volume::Talk => "says",
            Volume::Shout => "shouts",
        }
    }
}

/// Something said with the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Gesture {
    /// A raised hand waved: hello, over here.
    Wave,
    /// Both hands up and open: I'm no threat. Held until lowered.
    HandsUp,
    /// An arm held out the way the survivor faces: there.
    Point,
}

impl Gesture {
    pub const ALL: [Gesture; 3] = [Gesture::Wave, Gesture::HandsUp, Gesture::Point];

    /// `wave`, `hands_up` (also `handsup`, `hands-up`, `surrender`) or `point`.
    pub fn parse(s: &str) -> Option<Gesture> {
        match s.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "wave" => Some(Gesture::Wave),
            "hands_up" | "handsup" | "surrender" => Some(Gesture::HandsUp),
            "point" => Some(Gesture::Point),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Gesture::Wave => "wave",
            Gesture::HandsUp => "hands_up",
            Gesture::Point => "point",
        }
    }

    /// "waving", "hands up", "pointing".
    pub fn doing(self) -> &'static str {
        match self {
            Gesture::Wave => "waving",
            Gesture::HandsUp => "hands up",
            Gesture::Point => "pointing",
        }
    }
}

/// A gesture under way: which, and the seconds it has left (infinite for hands held up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gesturing {
    pub kind: Gesture,
    pub left: f32,
}

/// How far voices carry, in metres. Ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpeechTuning {
    /// How far other survivors hear each volume in the open (through a wall: `wall_muffle` of it).
    pub whisper_m: f32,
    pub talk_m: f32,
    pub shout_m: f32,
    /// The noise each makes for zombies (a sound's radius, as footsteps and doors have).
    pub whisper_noise: f32,
    pub talk_noise: f32,
    pub shout_noise: f32,
    /// The longest line, in characters; the rest is cut off.
    pub max_chars: usize,
    /// How long a wave or a point lasts (seconds).
    pub gesture_secs: f32,
}

impl Default for SpeechTuning {
    fn default() -> Self {
        Self { whisper_m: 3.0, talk_m: 12.0, shout_m: 40.0, whisper_noise: 1.5, talk_noise: 7.0, shout_noise: 30.0, max_chars: 120, gesture_secs: 2.5 }
    }
}

impl SpeechTuning {
    /// How far survivors hear a line at `volume` in the open.
    pub fn reach(&self, volume: Volume) -> f32 {
        match volume {
            Volume::Whisper => self.whisper_m,
            Volume::Talk => self.talk_m,
            Volume::Shout => self.shout_m,
        }
    }

    /// The noise a line at `volume` makes for zombies.
    pub fn noise(&self, volume: Volume) -> f32 {
        match volume {
            Volume::Whisper => self.whisper_noise,
            Volume::Talk => self.talk_noise,
            Volume::Shout => self.shout_noise,
        }
    }
}

/// A line as one survivor heard it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Said {
    /// Who hears it (the speaker itself too: its own line, as the server took it).
    pub listener: u32,
    /// Who said it. A server with fair information on tells a client only if `seen` (else 0).
    pub speaker: u32,
    /// The listener saw the speaker as it spoke (`World::player_sees`, #66; its own lines: true).
    pub seen: bool,
    /// Where the voice came from.
    pub pos: Vec2,
    pub volume: Volume,
    /// Through a wall or a shut door.
    pub muffled: bool,
    pub text: String,
    /// The world's tick when it was said.
    pub tick: u64,
}

/// `text` as a line that can be said: one line (control characters and line breaks become
/// spaces, runs of spaces one), trimmed, at most `max_chars` characters. `None` if nothing is left.
pub fn clean_text(text: &str, max_chars: usize) -> Option<String> {
    let mut out = String::new();
    let mut n = 0;
    for c in text.chars() {
        let c = if c.is_control() || c.is_whitespace() { ' ' } else { c };
        if c == ' ' && (out.is_empty() || out.ends_with(' ')) {
            continue;
        }
        if n >= max_chars {
            break;
        }
        out.push(c);
        n += 1;
    }
    let len = out.trim_end().len();
    out.truncate(len);
    (!out.is_empty()).then_some(out)
}

/// Is a voice carrying `reach` metres at `from` heard at `at`, and if so, is it muffled (no line
/// of sight: only `muffle` of the reach gets through a wall)? The zombies' hearing rule.
pub fn hears(map: &TileMap, muffle: f32, from: Vec2, at: Vec2, reach: f32) -> Option<bool> {
    let d = from.distance(at);
    if d > reach {
        return None;
    }
    let clear = crate::vision::sound_line(map, at, from);
    (clear || d <= reach * muffle).then_some(!clear)
}

impl World {
    /// Player `id` says `text` at `volume`. Returns how many other survivors heard it, or `None`
    /// if nothing was said: no such player, dead, asleep or logged out, nothing left of the text
    /// ([`clean_text`]), or it already spoke this tick.
    pub fn player_say(&mut self, id: u32, text: &str, volume: Volume) -> Option<usize> {
        let t = &self.config.speech;
        let p = self.players.iter().find(|p| p.id == id)?;
        if p.dead || p.needs.asleep() || p.away.is_some() {
            return None;
        }
        let text = clean_text(text, t.max_chars)?;
        let tick = self.tick;
        if self.speech.iter().any(|s| s.speaker == id && s.listener == id && s.tick == tick) {
            return None;
        }
        let (pos, reach, noise) = (p.state.pos, t.reach(volume), t.noise(volume));
        let muffle = self.config.zombie.wall_muffle;
        let mut heard = 0;
        // Room for this line's listeners: the oldest lines nobody took go.
        let over = (self.speech.len() + self.players.len()).saturating_sub(MAX_SPEECH);
        self.speech.drain(..over.min(self.speech.len()));
        for o in &self.players {
            let muffled = if o.id == id {
                false
            } else if o.dead || o.needs.asleep() || o.away.is_some() {
                continue;
            } else {
                match hears(&self.map, muffle, pos, o.state.pos, reach) {
                    Some(m) => m,
                    None => continue,
                }
            };
            heard += (o.id != id) as usize;
            // By the record of who sees whom; before the world's first tick with them, by the rule.
            let seen = self.awareness.sees(o.id, id) || (self.awareness.now() == 0 && self.sees_now(o.id, id));
            self.speech.push(Said { listener: o.id, speaker: id, seen, pos, volume, muffled, text: text.clone(), tick });
        }
        // A voice is a noise: zombies in earshot come to look, and a shout draws the far horde.
        self.emit_sound_from(pos, noise, SoundSource::Player(id));
        Some(heard)
    }

    /// The lines said since this was last called, each once per listener (replication).
    pub fn take_speech(&mut self) -> Vec<Said> {
        std::mem::take(&mut self.speech)
    }

    /// Player `id` makes `gesture`, or with `None` stops making one. Hands up again lowers them.
    /// Returns whether its pose changed. Not while dead, asleep, logged out, knocked about or in
    /// the middle of a swing, shove or stomp.
    pub fn player_gesture(&mut self, id: u32, gesture: Option<Gesture>) -> bool {
        let secs = self.config.speech.gesture_secs;
        let Some(p) = self.players.iter_mut().find(|p| p.id == id) else { return false };
        let Some(kind) = gesture else { return p.gesture.take().is_some() };
        // Nor with tied hands (#73).
        if p.dead || p.needs.asleep() || p.away.is_some() || p.knock.is_some() || p.action.is_some() || p.restraint.is_some() {
            return false;
        }
        p.gesture = match (kind, p.gesture) {
            (Gesture::HandsUp, Some(Gesturing { kind: Gesture::HandsUp, .. })) => None,
            (Gesture::HandsUp, _) => Some(Gesturing { kind, left: f32::INFINITY }),
            _ => Some(Gesturing { kind, left: secs }),
        };
        true
    }

    /// Gestures run out, and end with whatever takes the hands or the body.
    pub(crate) fn step_gestures(&mut self) {
        for p in &mut self.players {
            let Some(g) = &mut p.gesture else { continue };
            g.left -= TICK_DT;
            if g.left <= 0.0 || p.dead || p.needs.asleep() || p.away.is_some() || p.knock.is_some() || p.action.is_some() {
                p.gesture = None;
            }
        }
    }
}

#[cfg(test)]
mod tests;
