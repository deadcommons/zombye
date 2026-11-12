//! What goes over the network between a client and a multiplayer server (#21): the messages, how
//! they are framed, and snapshots cut down to what one client needs.
//!
//! **Transport:** one TCP connection per client (`TCP_NODELAY`), carrying frames: a `u32`
//! little-endian length, then that many bytes of postcard (the save files' encoding). TCP because
//! it needs nothing but the standard library, crosses home routers outward without help, and its
//! ordered, reliable delivery lets every snapshot be a *delta* against the one before with no
//! acknowledgements. Its cost is head-of-line blocking on a lossy link; the messages
//! ([`ClientMsg`], [`ServerMsg`]) don't care what carries them, so UDP or QUIC can replace it
//! behind [`crate::remote::Remote`] and [`crate::host::Host`] (docs/PLAN.md §2).
//!
//! **Joining:** the client sends [`Hello`]; the server answers [`ServerMsg::Welcome`] or
//! [`ServerMsg::Refused`]. The map isn't sent (a 1 km map is a million tiles): the client builds
//! it from the seed with its own copy of the game data, as loading a save does, checks its hash
//! against the server's, and applies the doors and windows changed since ([`Welcome::map_edits`]).
//!
//! **Snapshots (interest management, #22):** each tick the server captures one [`Snapshot`], cuts
//! it down per client ([`Snapshot::for_client`]: what's within the interest radius of that
//! client's survivor, and only its own private parts; [`MapInterest`]: map changes only for the
//! chunks near it, with what it missed when it comes near) and sends what the client hasn't got
//! ([`DeltaEncoder`] / [`DeltaDecoder`]). Zombies, the bulk of it, are carried on between updates
//! at both ends ([`Tracked`]) and corrected only when they are too far off for their distance,
//! most pressing first within a byte budget ([`Limits`]); other players go packed and only when
//! they change ([`PlayerPacked`]). With the server's fair-sight rule on, the view is first cut to
//! what the client's survivor could see or hear (#66, [`crate::visibility`]).

use std::collections::BTreeMap;
use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zombye_sim::map::{CHUNK_SIZE, EdgeChange, EdgeRef};
use zombye_sim::save::ContentPrint;
use zombye_sim::worldgen::GenConfig;
use zombye_sim::{
    ActionKind, CarryTuning, CombatEvent, CombatTuning, DevFlags, Gesture, GroundItem, HandsAct, Said, HealthTags, HealthTuning, InfectionTuning, ItemNote, Knock, LevelUp, MoveMods, NeedsTuning, SkillTuning, Stance, World, ZombieAnim,
};

use crate::{EdgeDamage, FloorView, Intent, OutfitView, PlayerView, PrivateView, RunView, Snapshot, ZombieView};

/// Bumped whenever a message changes shape; a server refuses any other. (2: player-vs-player
/// combat, #65: `PlayerView::knock`, `CombatEvent::by`, players killed in the run stats. 3: the
/// first night, #19: `RunView::first_night`, together with 2's changes. 4: interest management,
/// #22: zombies carried on between updates, other players packed. 5: fair information, #66:
/// `SnapshotDelta::heard`, `Rules::fair_sight`. 6: lag-compensated melee, #23: `Intent::Seen`,
/// `CombatTuning::lag_comp_max_secs` in the tunings. 7: #68: `PlayerView::protected`, the logout
/// and spawn protection times in `Rules`. 8: proximity chat and gestures, #67: `Intent::Say`,
/// `Intent::Gesture`, `PlayerView::gesture`, `SnapshotDelta::speech`. 9: the server's rules, #69:
/// `Hello::agent`, the camera, agents, spawns, start kit and world in `Rules`, `PlayerView::agent`,
/// `ClientMsg::Query` and `ServerMsg::Info`. 10: hold-ups and groups, #73: the `Restrain`, `Free`,
/// `Frisk`, `TakeFrom`, `StopHands`, `Struggle` and `Group` intents, tied hands in `PlayerView`
/// (`PlayerPacked::bits` is a `u32` for them), `PrivateView::frisk`, `SnapshotDelta::group`.
/// 11: the server browser, #27: `ServerInfo::mods`. 12: mods downloaded on joining, #24: a hash
/// and a size in `ModInfo`, `ClientMsg::Fetch` and `ServerMsg::Mod`. 13: guns, #72: `Intent::Aim`,
/// `CombatEventKind::Shot`, `JobKind::Reload`, the weapons' `ranged`. 14: `ZombieAnim::Lunge` and `Climb`.
/// 15: weapons that wear: `durability`, `wear_chance` and `close_range` in the weapons, `one_hand`
/// in the items, the wear numbers in `CombatTuning`. 16: ten skill levels and XP multipliers in `SkillTuning`.
/// 17: climbing with E: `Intent::Climb`, `PlayerView::climb`, `Tunings::climb`.
/// 18: broken weapons stay as items: `CombatTuning::broken_damage`. 19: electricity and room
/// lights: `Intent::Light`, `Intent::Grid`, `SnapshotDelta::lights`. 20: blinds: `Intent::Blind`,
/// `Snapshot::blind_changes`, `Welcome::blinds` and `Welcome::glazed`, `JobKind::Blind`.
/// 21: right-click actions on things in the world: `Intent::Toward`.
/// 22: player identity (DCM-66): `Hello::key`, `ServerMsg::Challenge` and `ClientMsg::Proof`.
/// 23: shoves that add up and stomps to the head: the shove and stomp numbers in `CombatTuning`.
/// 24: electricity (19) merged after 20-23.
/// 25: stairs and the upper floor: `PlayerView::level` (bit 17 of `PlayerPacked::bits`),
/// `SnapshotDelta::upstairs`, `Tunings::stairs`. 26: the map screen: `SnapshotDelta::map`, what
/// the client's survivor has seen of the map. 27: bags with their own room: `Place::Pouch`,
/// `PrivateView::homes`. 28: chopping trees: `Snapshot::tree_changes`, `Welcome::stumps`,
/// `CombatEventKind::Chop`, `Feature::Stump`. 29: fire and cooking: `Intent::Fire`, `Snapshot::fires`
/// (`SnapshotDelta::fires`), `Tunings::fire`, `RecipeDef::heat` and `places`, `Consumable::poison`, `WoundKind::Burn`,
/// `DeathCause::Burned`; sitting down: `Intent::Sit`, `PlayerView::seated` (bits 18-20 of `PlayerPacked::bits`).)
pub const PROTOCOL: u32 = 29;
/// The port a server listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 7777;
/// The most players a server takes (the design size, docs/PLAN.md §4.8), and the default.
pub const MAX_PLAYERS: usize = 64;
pub const DEFAULT_PLAYERS: usize = 32;

/// The largest frame each side accepts from the other. A client's messages are tiny.
pub const MAX_CLIENT_FRAME: usize = 4 * 1024;
pub const MAX_SERVER_FRAME: usize = 8 * 1024 * 1024;

/// Client to server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// First, once.
    Hello(Hello),
    /// Something the client's player wants to do. The server makes it that player's whatever
    /// `player` it names.
    Intent(Intent),
    /// Answered with [`ServerMsg::Pong`] carrying the same number (the client's clock, for ping).
    Ping(u32),
    /// Leaving.
    Bye,
    /// Instead of a hello (#69): "what server are you?", with the asker's protocol. Answered with
    /// [`ServerMsg::Info`] and the connection is closed: nobody joins, no password is needed. What
    /// a server browser asks, and `zombye --server-info HOST`.
    Query(u32),
    /// Instead of a hello (#24): "send me these mods of yours". Answered with a
    /// [`ServerMsg::Mod`] for each, in the order asked, or one [`ServerMsg::Refused`], and the
    /// connection is closed: nobody joins.
    Fetch(Fetch),
    /// The answer to [`ServerMsg::Challenge`] (DCM-66): the challenge and the name signed with
    /// the key the hello named ([`crate::identity::Identity::sign`]).
    Proof(Vec<u8>),
}

/// A request for mods a server runs, by the hashes its [`ServerInfo::mods`] named (#24).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fetch {
    pub protocol: u32,
    /// The server's join password, if it has one: its mods go to those who may join.
    pub password: String,
    /// At most [`crate::modpack::MAX_MODS`].
    pub mods: Vec<crate::modpack::Hash>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    /// Who this is. A name whose survivor is already in the world and not being played takes that
    /// survivor up again. Empty: the server makes one up.
    pub name: String,
    /// The server's join password, if it has one.
    pub password: String,
    /// Played by a program, not a person (#69): the bots, an AI agent's client. A server's
    /// [`Rules::agents`] says whether it may join and whether others are told. It is the client's
    /// own word: an honest agent says so, and a server can't tell one that doesn't.
    pub agent: bool,
    /// Who this is, for the server (DCM-66): the player's public key, made once per install. Its
    /// survivors and any ban go by it, not by `name`; the hello is answered with a
    /// [`ServerMsg::Challenge`] the key must sign before anything else.
    pub key: crate::identity::Key,
}

/// Server to client.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome(Box<Welcome>),
    /// Not let in, and why (wrong version or password, full, name in use).
    Refused(String),
    Snapshot(Box<SnapshotDelta>),
    Pong(u32),
    /// The server is closing this connection, and why.
    Bye(String),
    /// The answer to [`ClientMsg::Query`].
    Info(Box<ServerInfo>),
    /// One of the mods a [`ClientMsg::Fetch`] asked for: its data, never its scripts (#24).
    Mod(Box<crate::modpack::ModPack>),
    /// The answer to a hello (DCM-66): sign this with the hello's key ([`ClientMsg::Proof`]),
    /// then the hello is let in or refused.
    Challenge(crate::identity::Nonce),
}

/// What a server says of itself to anyone who asks, before joining (#69: the server browser's row).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    /// The server's protocol (a client of another can't join) and game version.
    pub protocol: u32,
    pub version: String,
    pub rules: Rules,
    /// Players on now.
    pub online: u32,
    /// Joining needs a password.
    pub password: bool,
    /// The mods the server runs, in order (#27). A client plays with the same data: what it
    /// hasn't got by these hashes it downloads before joining (#24, [`crate::modpack`]).
    pub mods: Vec<ModInfo>,
}

/// A mod a server runs, as its `mod.ron` names it (#27: the server browser's mods column).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModInfo {
    pub id: String,
    pub name: String,
    /// May be empty.
    pub version: String,
    /// What names its data (#24): the SHA-256 of its pack ([`crate::modpack::ModPack::hash`]).
    /// A client downloads the pack with this hash, and takes nothing that doesn't hash to it.
    pub hash: crate::modpack::Hash,
    /// The pack's size in bytes, for saying what a join will download.
    pub size: u32,
}

/// The camera a server's players play with (docs/PLAN.md §5). The isometric camera sees over
/// walls and round corners and a first-person one doesn't, so a server has one or the other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Camera {
    #[default]
    Isometric,
    /// First person only. No client has that camera yet (#71): one without it doesn't join.
    FirstPerson,
}

impl Camera {
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
            "isometric" | "iso" => Some(Self::Isometric),
            "firstperson" | "first" | "fp" => Some(Self::FirstPerson),
            _ => None,
        }
    }

    /// As the `--camera` flag and the admin tools write it.
    pub fn word(self) -> &'static str {
        match self {
            Self::Isometric => "isometric",
            Self::FirstPerson => "first-person",
        }
    }
}

/// Whether programs (bots, AI agents: clients whose hello says `agent`) may play on a server, and
/// whether the others are told which survivors they are (docs/PLAN.md §4.8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Agents {
    /// They are refused at the door.
    Refused,
    /// They join like anyone, and nobody is told.
    Allowed,
    /// They join, and whoever sees one sees that it is one ([`PlayerView::agent`]).
    #[default]
    Marked,
}

impl Agents {
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "off" | "no" | "false" | "refused" => Some(Self::Refused),
            "on" | "yes" | "true" | "allowed" | "unmarked" => Some(Self::Allowed),
            "marked" => Some(Self::Marked),
            _ => None,
        }
    }

    /// As the `--agents` flag and the admin tools write it.
    pub fn word(self) -> &'static str {
        match self {
            Self::Refused => "off",
            Self::Allowed => "on",
            Self::Marked => "marked",
        }
    }
}

/// A server's rules (docs/PLAN.md §4.8, #69): what makes one server different from another. From
/// its config, told to every client that joins ([`Welcome::rules`]) and to anyone who asks before
/// joining ([`ServerInfo`]). [`Rules::lines`] says them in words.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    /// The server's name.
    pub name: String,
    /// Players can hurt each other. Off: co-op.
    pub pvp: bool,
    pub max_players: u32,
    /// Dev toggles and debug intents are taken.
    pub allow_dev: bool,
    /// Seconds a survivor stays in the world, asleep, after its player leaves (#68), and the
    /// shorter time when it was left somewhere safe.
    pub logout_secs: f32,
    pub logout_safe_secs: f32,
    /// Seconds a new survivor can't be hurt by other players (0: none).
    pub spawn_protection_secs: f32,
    /// Fair information (#66): a client is told of another player only while its survivor sees
    /// them (or hears them, as a sound). Off: everyone within the interest radius.
    pub fair_sight: bool,
    /// The camera everyone plays with.
    pub camera: Camera,
    /// Whether programs may play here, and whether they are marked.
    pub agents: Agents,
    /// New survivors start spread round the edge of the town (else together, by one house).
    pub spread_spawns: bool,
    /// What a new survivor starts with: item names and counts.
    pub start_kit: Vec<(String, u16)>,
    /// The map's width and height in metres, and the zombies the world holds now.
    pub map_m: [u32; 2],
    pub zombies: u32,
}

impl Rules {
    /// The rules in words, a label and a sentence each, in the order a player wants them: what the
    /// client shows on joining, what `--server-info` prints and what the server says when it starts.
    pub fn lines(&self) -> Vec<(&'static str, String)> {
        let secs = |s: f32| if s.fract() == 0.0 { format!("{s:.0} s") } else { format!("{s:.1} s") };
        let mut lines = vec![
            ("Players", format!("up to {}", self.max_players)),
            ("PvP", if self.pvp { "on: other survivors can hurt you".to_string() } else { "off: co-op, survivors can't hurt each other".to_string() }),
            (
                "Camera",
                match self.camera {
                    Camera::Isometric => "isometric".to_string(),
                    Camera::FirstPerson => "first person only".to_string(),
                },
            ),
            (
                "Agents",
                match self.agents {
                    Agents::Refused => "not allowed: every survivor is played by a person".to_string(),
                    Agents::Allowed => "allowed, and not marked: a survivor may be played by a program".to_string(),
                    Agents::Marked => "allowed, and marked as agents".to_string(),
                },
            ),
            ("Other players", if self.fair_sight { "shown only while your survivor sees or hears them".to_string() } else { "always shown when near".to_string() }),
            (
                "Logging out",
                if self.logout_secs <= 0.0 {
                    "your survivor leaves the world at once".to_string()
                } else if self.logout_safe_secs < self.logout_secs {
                    format!("your survivor stays in the world for {} ({} somewhere safe)", secs(self.logout_secs), secs(self.logout_safe_secs))
                } else {
                    format!("your survivor stays in the world for {}", secs(self.logout_secs))
                },
            ),
            (
                "New survivors",
                format!(
                    "start {}{}",
                    if self.spread_spawns { "apart, round the edge of the town" } else { "together, by one house" },
                    if self.pvp && self.spawn_protection_secs > 0.0 { format!("; other players can't hurt them for {}", secs(self.spawn_protection_secs)) } else { String::new() }
                ),
            ),
            (
                "Start kit",
                if self.start_kit.is_empty() { "nothing".to_string() } else { self.start_kit.iter().map(|(name, n)| if *n == 1 { name.clone() } else { format!("{n} x {name}") }).collect::<Vec<_>>().join(", ") },
            ),
            ("World", format!("{} x {} m, {} zombies", self.map_m[0], self.map_m[1], self.zombies)),
        ];
        if self.allow_dev {
            lines.push(("Dev tools", "on: god mode and the debug tools are taken from players".to_string()));
        }
        lines
    }
}

/// The numbers a client needs to predict and to label things as the server does.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tunings {
    pub combat: CombatTuning,
    pub health: HealthTuning,
    pub infection: InfectionTuning,
    pub needs: NeedsTuning,
    pub skills: SkillTuning,
    pub carry: CarryTuning,
    /// When E climbs, so the client sends a climb only where the server takes one.
    pub climb: zombye_sim::ClimbTuning,
    /// Walking between floors: the speed on a flight of stairs, which movement is predicted with.
    pub stairs: zombye_sim::StairTuning,
    /// Fire and cooking (`zombye_sim::fire`): where the Make list says a cooking recipe has heat.
    pub fire: zombye_sim::FireTuning,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Welcome {
    pub protocol: u32,
    /// The client's player and the name it goes by.
    pub player: u32,
    pub name: String,
    pub rules: Rules,
    /// The map: generated from this with the client's own game data...
    pub generation: GenConfig,
    /// ...which must be the server's (the fingerprint saves use) and give this map hash...
    pub content: ContentPrint,
    pub map_hash: u64,
    /// ...then the doors and windows opened, shut or broken since, and the cracks.
    pub map_edits: Vec<EdgeChange>,
    pub edge_damage: Vec<EdgeDamage>,
    /// Every blind as it is now, which replace the ones the client's map was generated with, and
    /// the doors with glass in them (the server's settings place them).
    pub blinds: Vec<zombye_sim::map::BlindChange>,
    pub glazed: Vec<zombye_sim::map::EdgeRef>,
    /// Every tree felled so far (stumps now, `zombye_sim::trees`).
    pub stumps: Vec<zombye_sim::trees::TreeChange>,
    pub tunings: Tunings,
}

/// A zombie as sent: position in 1/64 m, facing in 1/256 turns, speed in 1/48 m/s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZombiePacked {
    pub id: u32,
    pub x: u32,
    pub y: u32,
    pub facing: u8,
    pub speed: u8,
    pub anim: ZombieAnim,
}

/// A zombie the client already has that isn't where the client takes it to be (see [`Tracked`]):
/// about 4 bytes. `gap` is its id less the id of the move before it in the list (the first: its
/// id), ids rising; `dx`, `dy` are how far off it is, in [`ZombiePacked`]'s position steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZombieMove {
    pub gap: u32,
    pub dx: i8,
    pub dy: i8,
    pub facing: u8,
}

const POS_STEPS: f32 = 64.0;
const SPEED_STEPS: f32 = 48.0;

impl ZombiePacked {
    pub fn pack(z: &ZombieView) -> Self {
        let turn = z.facing.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        Self {
            id: z.id,
            x: (z.pos[0].max(0.0) * POS_STEPS).round() as u32,
            y: (z.pos[1].max(0.0) * POS_STEPS).round() as u32,
            facing: ((turn * 256.0).round() as u32 & 255) as u8,
            speed: (z.speed.max(0.0) * SPEED_STEPS).round().min(255.0) as u8,
            anim: z.anim,
        }
    }

    pub fn unpack(&self) -> ZombieView {
        ZombieView {
            id: self.id,
            pos: [self.x as f32 / POS_STEPS, self.y as f32 / POS_STEPS],
            facing: self.facing as f32 / 256.0 * std::f32::consts::TAU,
            speed: self.speed as f32 / SPEED_STEPS,
            anim: self.anim,
        }
    }
}

/// Fixed-point parts of a position step ([`ZombiePacked`]'s 1/64 m) that both ends carry a zombie's
/// position and speed in between updates.
const SUB: i32 = 256;
/// A correction is taken to have built up over at most this many ticks.
const MAX_AGE: u8 = 20;
/// The fastest a zombie is carried on between updates (steps × `SUB` a tick: 20 m/s).
const MAX_DRIFT: i32 = 64 * SUB;

/// What both ends keep of a zombie the client has (#22): what it was last told, and where it is
/// taken to be now. Between updates it is **carried on** at the speed its last corrections showed
/// (dead reckoning), so one walking steadily needs no updates at all; the server does exactly what
/// the client does (integers only: the same on every machine), sees how far that is from the
/// truth, and sends a correction once it is too far for how far away the zombie is.
#[derive(Clone, Copy, Debug)]
struct Tracked {
    z: ZombiePacked,
    /// Position and speed per tick in 1/`SUB` steps.
    px: i32,
    py: i32,
    vx: i32,
    vy: i32,
    /// Ticks since it was last told.
    age: u8,
}

impl Tracked {
    /// Told whole: there, standing.
    fn whole(z: ZombiePacked) -> Self {
        Self { z, px: z.x as i32 * SUB, py: z.y as i32 * SUB, vx: 0, vy: 0, age: 0 }
    }

    /// A tick passes with no word of it.
    fn advance(&mut self) {
        self.px = (self.px + self.vx).max(0);
        self.py = (self.py + self.vy).max(0);
        self.age = self.age.saturating_add(1);
    }

    /// Told it is `dx`, `dy` steps from where it was taken to be: put there, and half of what
    /// that says about its speed taken up (all of it overshoots for a zombie jostled in a crowd).
    fn moved(&mut self, dx: i8, dy: i8, facing: u8) {
        let n = self.age.clamp(1, MAX_AGE) as i32;
        let (cx, cy) = (dx as i32 * SUB, dy as i32 * SUB);
        self.px = (self.px + cx).max(0);
        self.py = (self.py + cy).max(0);
        self.vx = (self.vx + cx / (2 * n)).clamp(-MAX_DRIFT, MAX_DRIFT);
        self.vy = (self.vy + cy / (2 * n)).clamp(-MAX_DRIFT, MAX_DRIFT);
        self.z.facing = facing;
        self.age = 0;
    }

    /// As the client draws it now.
    fn shown(&self) -> ZombiePacked {
        ZombiePacked { x: ((self.px + SUB / 2) / SUB) as u32, y: ((self.py + SUB / 2) / SUB) as u32, ..self.z }
    }
}

/// Another player as sent (#22): what drawing it needs, about 14 bytes, and only when it changes.
/// A client's own player goes whole every tick (prediction needs it exact); of the others it
/// learns no more than it could see: not their endurance, load, inputs or dev toggles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerPacked {
    pub id: u32,
    /// As [`ZombiePacked`]: 1/64 m, 1/256 turn.
    pub x: u32,
    pub y: u32,
    pub facing: u8,
    /// Bits 0-1 the stance, then dead, asleep, aiming, bleeding, limping, sick, injured arm, and
    /// in bits 9-10 its gesture (#67: 0 none, 1 wave, 2 hands up, 3 point); #73: bit 11 its hands
    /// are tied, 12 struggling, 13-15 what its hands do to another (0 nothing, 1 tying, 2 freeing,
    /// 3 searching, 4 taking); in bit 16 that it is a marked agent (#69); in bit 17 that it is
    /// upstairs; in bits 18-20 what it sits on (`SeatKind::bits`, 0 standing).
    pub bits: u32,
    /// Hit points, rounded up.
    pub hp: u8,
    pub weapon: Option<u16>,
    pub action: Option<ActionKind>,
    /// Hundredths of a second (at most 2.55 s).
    pub action_left: u8,
    pub ready_in: u8,
    /// Reeling or (`true`) on the ground after another player's blow, with the tenths of a second
    /// left (at most 25.5 s).
    pub knock: Option<(bool, u8)>,
}

impl PlayerPacked {
    pub fn pack(p: &PlayerView) -> Self {
        let turn = p.facing.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        let bit = |on: bool, n: u32| (on as u32) << n;
        let h = p.health;
        let centi = |secs: f32| (secs.max(0.0) * 100.0).round().min(255.0) as u8;
        let gesture: u32 = match p.gesture {
            None => 0,
            Some(Gesture::Wave) => 1,
            Some(Gesture::HandsUp) => 2,
            Some(Gesture::Point) => 3,
        };
        let hands: u32 = match p.hands {
            None => 0,
            Some(HandsAct::Tie) => 1,
            Some(HandsAct::Free) => 2,
            Some(HandsAct::Frisk) => 3,
            Some(HandsAct::Take) => 4,
        };
        Self {
            id: p.id,
            x: (p.pos[0].max(0.0) * POS_STEPS).round() as u32,
            y: (p.pos[1].max(0.0) * POS_STEPS).round() as u32,
            facing: ((turn * 256.0).round() as u32 & 255) as u8,
            bits: p.stance as u32 | bit(p.dead, 2) | bit(p.asleep, 3) | bit(p.aiming, 4) | bit(h.bleeding, 5) | bit(h.limping, 6) | bit(h.sick, 7) | bit(h.injured_arm, 8) | gesture << 9 | bit(p.restrained, 11) | bit(p.struggling, 12) | hands << 13 | bit(p.agent, 16) | bit(p.level > 0, 17) | zombye_sim::SeatKind::bits(p.seated) << 18,
            hp: p.hp.clamp(0.0, 255.0).ceil() as u8,
            weapon: p.weapon,
            action: p.action,
            action_left: centi(p.action_left),
            ready_in: centi(p.ready_in),
            knock: p.knock.map(|k| (k.down, (k.timer.max(0.0) * 10.0).ceil().min(255.0) as u8)),
        }
    }

    /// The view a client draws it from; what wasn't sent is as a rested, unburdened survivor's.
    pub fn unpack(&self) -> PlayerView {
        let bit = |n: u32| self.bits >> n & 1 == 1;
        PlayerView {
            id: self.id,
            pos: [self.x as f32 / POS_STEPS, self.y as f32 / POS_STEPS],
            facing: self.facing as f32 / 256.0 * std::f32::consts::TAU,
            stance: match self.bits & 3 {
                1 => Stance::Run,
                2 => Stance::Sneak,
                _ => Stance::Walk,
            },
            endurance: 1.0,
            winded: false,
            climb: None,
            level: bit(17) as u8,
            last_input_seq: 0,
            hp: self.hp as f32,
            dead: bit(2),
            weapon: self.weapon,
            action: self.action,
            action_left: self.action_left as f32 / 100.0,
            ready_in: self.ready_in as f32 / 100.0,
            mods: MoveMods::default(),
            busy: 0.0,
            jobs: 0,
            dev: DevFlags::default(),
            health: HealthTags { bleeding: bit(5), limping: bit(6), sick: bit(7), injured_arm: bit(8) },
            asleep: bit(3),
            seated: zombye_sim::SeatKind::from_bits(self.bits >> 18 & 7),
            aiming: bit(4),
            knock: self.knock.map(|(down, tenths)| Knock { down, timer: tenths as f32 / 10.0 }),
            // Not sent of others (#68): you can't see that a stranger is protected, only our own
            // player's view (which goes whole) says so.
            protected: false,
            gesture: match self.bits >> 9 & 3 {
                1 => Some(Gesture::Wave),
                2 => Some(Gesture::HandsUp),
                3 => Some(Gesture::Point),
                _ => None,
            },
            agent: bit(16),
            restrained: bit(11),
            struggling: bit(12),
            struggle: 0.0,
            hands: match self.bits >> 13 & 7 {
                1 => Some(HandsAct::Tie),
                2 => Some(HandsAct::Free),
                3 => Some(HandsAct::Frisk),
                4 => Some(HandsAct::Take),
                _ => None,
            },
            hands_left: 0.0,
            hands_total: 0.0,
        }
    }
}

/// A [`Snapshot`] for one client, as the changes since the last one sent to it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SnapshotDelta {
    pub tick: u64,
    pub time_of_day: f32,
    pub time_scale: f32,
    pub game_minute: i32,
    /// The client's own player, whole (it must be exact for prediction).
    pub players: Vec<PlayerView>,
    /// The other players in view that are new to this client or changed, and the ids of those no
    /// longer in view.
    pub others: Vec<PlayerPacked>,
    pub others_gone: Vec<u32>,
    /// Zombies new to this client or changed, the ids of those no longer in view, and those that
    /// only moved (applied after the others).
    pub zombies: Vec<ZombiePacked>,
    pub zombies_gone: Vec<u32>,
    pub zombie_moves: Vec<ZombieMove>,
    /// Ground items new or changed, and the ids of those gone.
    pub items: Vec<GroundItem>,
    pub items_gone: Vec<u32>,
    // Things that happened (each once).
    pub edge_changes: Vec<EdgeChange>,
    pub edge_damage: Vec<EdgeDamage>,
    pub blind_changes: Vec<zombye_sim::map::BlindChange>,
    pub tree_changes: Vec<zombye_sim::trees::TreeChange>,
    pub hits: Vec<CombatEvent>,
    pub private: Vec<PrivateView>,
    pub item_notes: Vec<ItemNote>,
    pub level_ups: Vec<LevelUp>,
    /// Players heard and not seen (#66): where, and how loud (the radius, metres).
    pub heard: Vec<(glam::Vec2, f32)>,
    /// What this client's survivor heard said (#67), its own lines included.
    pub speech: Vec<Said>,
    /// Sent when they change (the run about once a second while only its clocks move).
    pub floor: Option<Vec<u32>>,
    /// Which of the client's ground items lie upstairs, by id, when that changes.
    pub upstairs: Option<Vec<u32>>,
    /// The run with the first night's record (#19).
    pub run: Option<RunView>,
    pub outfits: Option<Vec<OutfitView>>,
    pub looks: Option<Vec<OutfitView>>,
    /// The client's group and invitations (#73), when they change; `Some(None)`: none any more.
    pub group: Option<Option<zombye_sim::GroupView>>,
    /// The rooms lit and whether the grid is on (`zombye_sim::power`), when they change.
    pub lights: Option<(Vec<u16>, bool)>,
    /// What the client's survivor has seen of the map (`zombye_sim::explore`), when it changes.
    pub map: Option<zombye_sim::Explored>,
    /// The campfires (`zombye_sim::fire`), when they change.
    pub fires: Option<Vec<zombye_sim::FireView>>,
}

impl Snapshot {
    /// This snapshot as the client playing `player` gets it over the network: its own private
    /// parts ([`Snapshot::for_player`]), and of the world only what is within `radius` metres of
    /// its survivor: zombies, other players, items on the ground, hits. Map changes all stay in:
    /// a server cuts those by chunk with the client's [`MapInterest`], which remembers what it
    /// held back. With no such player, nothing is cut by distance.
    pub fn for_client(&self, player: u32, radius: f32) -> Snapshot {
        let me: Option<glam::Vec2> = self.players.iter().find(|p| p.id == player).map(|p| p.pos.into());
        let near = |pos: glam::Vec2| me.is_none_or(|m| m.distance_squared(pos) <= radius * radius);
        let players: Vec<PlayerView> = self.players.iter().filter(|p| p.id == player || near(p.pos.into())).copied().collect();
        let zombies: Vec<ZombieView> = self.zombies.iter().filter(|z| near(z.pos.into())).copied().collect();
        let seen = |id: u32| players.iter().any(|p| p.id == id);
        Snapshot {
            tick: self.tick,
            time_of_day: self.time_of_day,
            time_scale: self.time_scale,
            game_minute: self.game_minute,
            outfits: self.outfits.iter().filter(|o| seen(o.id)).cloned().collect(),
            looks: self.looks.iter().filter(|o| zombies.binary_search_by_key(&o.id, |z| z.id).is_ok()).cloned().collect(),
            players,
            zombies,
            edge_changes: self.edge_changes.clone(),
            edge_damage: self.edge_damage.clone(),
            blind_changes: self.blind_changes.clone(),
            tree_changes: self.tree_changes.clone(),
            items: self.items.iter().filter(|i| near(i.pos)).copied().collect(),
            upstairs: self.upstairs.clone(),
            hits: self.hits.iter().filter(|h| near(h.pos)).copied().collect(),
            private: self.private.iter().filter(|v| v.player == player).cloned().collect(),
            item_notes: self.item_notes.iter().filter(|n| n.player == player).cloned().collect(),
            floor: self.floor.iter().filter(|f| f.player == player).cloned().collect(),
            runs: self.runs.iter().filter(|r| r.player == player).cloned().collect(),
            level_ups: self.level_ups.iter().filter(|l| l.player == player).copied().collect(),
            heard: self.heard.iter().filter(|h| h.listener == player).copied().collect(),
            speech: self.speech.iter().filter(|s| s.listener == player).cloned().collect(),
            groups: self.groups.iter().filter(|g| g.player == player).cloned().collect(),
            maps: self.maps.iter().filter(|m| m.player == player).cloned().collect(),
            lit_rooms: self.lit_rooms.clone(),
            grid_on: self.grid_on,
            fires: self.fires.clone(),
        }
    }
}

/// Which doors and windows one client is behind on (#22). Map changes go to a client only for
/// the chunks near its survivor; one that happens further off is noted here, and when its chunk
/// comes into the client's interest the edge's state *as it is then* is sent (however often it
/// changed meanwhile). So a client's map copy is right wherever it can walk or see, and a town
/// full of doors being battered costs each client only the ones near it.
#[derive(Default)]
pub struct MapInterest {
    edges: BTreeMap<(i32, i32, u8), EdgeRef>,
    damage: BTreeMap<(i32, i32, u8), EdgeRef>,
}

impl MapInterest {
    /// Cut `view`'s map changes down to the chunks within `radius` metres of `me` (measured to
    /// the nearest point of the edge's 16 m chunk), and add what is owed for the chunks that have
    /// come within it. With no `me`, everything goes.
    pub fn cut(&mut self, view: &mut Snapshot, me: Option<glam::Vec2>, radius: f32, world: &World) {
        let near = |tile: glam::IVec2| {
            me.is_none_or(|m| {
                let min = (tile.div_euclid(glam::IVec2::splat(CHUNK_SIZE)) * CHUNK_SIZE).as_vec2();
                m.distance_squared(m.clamp(min, min + glam::Vec2::splat(CHUNK_SIZE as f32))) <= radius * radius
            })
        };
        let mut changes = Vec::new();
        for c in view.edge_changes.drain(..) {
            let e = EdgeRef::new(c.tile, c.edge);
            if near(c.tile) {
                self.edges.remove(&e.key());
                changes.push(c);
            } else {
                self.edges.insert(e.key(), e);
            }
        }
        self.edges.retain(|_, e| {
            let due = near(e.tile);
            if due {
                changes.push(EdgeChange { tile: e.tile, edge: e.edge, kind: world.map.owned_edge(e.tile, e.edge) });
            }
            !due
        });
        view.edge_changes = changes;

        let mut damage = Vec::new();
        for d in view.edge_damage.drain(..) {
            let e = EdgeRef::new(d.tile, d.edge);
            if near(d.tile) {
                self.damage.remove(&e.key());
                damage.push(d);
            } else {
                self.damage.insert(e.key(), e);
            }
        }
        self.damage.retain(|_, e| {
            let due = near(e.tile);
            if due {
                // Broken, mended or no longer a door: no cracks.
                let hp = world.edge_hp(*e).map_or(1.0, |(hp, max)| (hp / max).clamp(0.0, 1.0));
                damage.push(EdgeDamage { tile: e.tile, edge: e.edge, hp });
            }
            !due
        });
        view.edge_damage = damage;
    }

    /// Edges this client is behind on (tests, reports).
    pub fn owed(&self) -> usize {
        self.edges.len() + self.damage.len()
    }
}

/// How often a run whose clocks alone moved is sent again (ticks).
const RUN_EVERY: u64 = 20;

/// What one snapshot for one client may cost (#22).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Zombies new to the client are taken up within this many metres of its survivor; ones it
    /// has stay until they leave its view (a little further out, so one walking along the edge
    /// of the view doesn't come and go).
    pub admit: f32,
    /// Bytes for zombie updates this tick. When they don't all fit, the most pressing go (see
    /// [`DeltaEncoder::encode`]) and the rest wait, carried on meanwhile.
    pub zombie_bytes: usize,
}

impl Default for Limits {
    /// No limits.
    fn default() -> Self {
        Self { admit: f32::INFINITY, zombie_bytes: usize::MAX }
    }
}

/// What an encode did with the zombies in a client's view.
#[derive(Clone, Copy, Debug, Default)]
pub struct EncodeStats {
    /// Zombies the client has after this snapshot.
    pub known: u32,
    /// Sent whole (new, or doing something else), as a move, and as gone.
    pub whole: u32,
    pub moves: u32,
    pub gone: u32,
    /// Updates that were due but didn't fit the bytes allowed: they wait.
    pub held: u32,
}

/// About what each kind of zombie update takes on the wire.
const WHOLE_BYTES: usize = 12;
const MOVE_BYTES: usize = 5;
const GONE_BYTES: usize = 3;
/// How far off (in steps, per metre from the client's survivor) a carried-on zombie may be before
/// it is corrected: 1/400 of its distance (2.5 cm at 10 m, 20 cm at 80 m), and never under 3/4 of
/// a step. At the default zoom that is under two pixels at the edge of the screen.
const OFF_PER_M: f32 = POS_STEPS / 400.0;
const OFF_MIN: f32 = 0.75;
/// What a zombie that is new to the client, or doing something else, counts as being off (steps).
const WHOLE_OFF: f32 = POS_STEPS * 0.5;
/// The same for its facing, in 1/256 turns: 4° close up, 40° at the edge of the view.
const TURN_MIN: f32 = 3.0;
const TURN_PER_M: f32 = 0.3;

enum Update {
    Whole,
    Move(i8, i8),
}

/// A zombie update that is due: how pressing, and where the zombie is in the new list.
struct Due {
    press: f32,
    at: usize,
    update: Update,
}

/// The server's memory of what one client has: turns each snapshot for it into a delta.
#[derive(Default)]
pub struct DeltaEncoder {
    started: bool,
    zombies: Vec<Tracked>,
    others: Vec<PlayerPacked>,
    /// What the last [`DeltaEncoder::encode`] did with the zombies.
    pub stats: EncodeStats,
    items: Vec<GroundItem>,
    floor: Vec<u32>,
    upstairs: Vec<u32>,
    run: Option<RunView>,
    outfits: Vec<OutfitView>,
    looks: Vec<OutfitView>,
    group: Option<zombye_sim::GroupView>,
    lights: (Vec<u16>, bool),
    map: zombye_sim::Explored,
    fires: Vec<zombye_sim::FireView>,
}

/// The entries of `next` that are new or differ from `last`'s (both sorted by `key`), and the keys
/// of `last` missing from `next`.
fn diff<T: Copy + PartialEq>(last: &[T], next: &[T], key: impl Fn(&T) -> u32) -> (Vec<T>, Vec<u32>) {
    let (mut changed, mut gone) = (Vec::new(), Vec::new());
    let (mut i, mut j) = (0, 0);
    while i < last.len() || j < next.len() {
        match (last.get(i), next.get(j)) {
            (Some(a), Some(b)) if key(a) == key(b) => {
                if a != b {
                    changed.push(*b);
                }
                i += 1;
                j += 1;
            }
            (Some(a), Some(b)) if key(a) < key(b) => {
                gone.push(key(a));
                i += 1;
            }
            (Some(a), None) => {
                gone.push(key(a));
                i += 1;
            }
            (_, Some(b)) => {
                changed.push(*b);
                j += 1;
            }
            (None, None) => break,
        }
    }
    (changed, gone)
}

/// `last` with `changed` put in (replacing by key, or added in order) and `gone` taken out.
fn patch<T: Copy>(last: &mut Vec<T>, changed: &[T], gone: &[u32], key: impl Fn(&T) -> u32) {
    if !gone.is_empty() {
        last.retain(|t| gone.binary_search(&key(t)).is_err());
    }
    for c in changed {
        match last.binary_search_by_key(&key(c), &key) {
            Ok(k) => last[k] = *c,
            Err(k) => last.insert(k, *c),
        }
    }
}

impl DeltaEncoder {
    /// The delta that takes this client from the last snapshot encoded to `view` (already cut
    /// down for it by [`Snapshot::for_client`]). The first is everything.
    ///
    /// **Zombies** (#22) are the bulk of it, so they are sent by need. Each one the client has is
    /// carried on between updates ([`Tracked`]), here as there; one is *due* when it is new (and
    /// within `limits.admit`), does something else (its animation), or is further from where the
    /// client takes it to be than its distance from the client's survivor allows (`OFF_PER_M`).
    /// Due updates go most pressing first (by how many times over what its distance allows a
    /// zombie is, a new or changed one counting as half a metre off) until `limits.zombie_bytes` is spent; the rest wait, growing more pressing.
    /// So a crowd costs what its changes of pace and direction cost, close ones are exact, and a
    /// horde bigger than the budget gets coarser with distance instead of the link falling behind.
    pub fn encode(&mut self, view: Snapshot, player: u32, limits: Limits) -> SnapshotDelta {
        let first = !std::mem::replace(&mut self.started, true);
        let me: Option<glam::Vec2> = view.players.iter().find(|p| p.id == player).map(|p| p.pos.into());
        let mut actual: Vec<(ZombiePacked, &ZombieView)> = view.zombies.iter().map(|z| (ZombiePacked::pack(z), z)).collect();
        actual.sort_by_key(|(z, _)| z.id);
        for t in &mut self.zombies {
            t.advance();
        }
        // The new list: every zombie in view the client has or could get, in id order. `live`
        // says which it will have after this snapshot.
        let mut next: Vec<(Tracked, bool)> = Vec::with_capacity(actual.len());
        let mut due: Vec<Due> = Vec::new();
        let mut z_gone: Vec<u32> = Vec::new();
        let mut old = self.zombies.iter().peekable();
        for (z, at) in &actual {
            while old.peek().is_some_and(|t| t.z.id < z.id) {
                z_gone.push(old.next().expect("peeked").z.id);
            }
            let dist = me.map_or(0.0, |m| m.distance(at.pos.into()));
            // How pressing: how many times over what its distance allows it is. A new one, or one
            // doing something else, counts as half a metre off.
            let allowed = OFF_MIN + dist * OFF_PER_M;
            let whole = WHOLE_OFF / allowed;
            match old.next_if(|t| t.z.id == z.id) {
                None if dist <= limits.admit => {
                    due.push(Due { press: whole, at: next.len(), update: Update::Whole });
                    next.push((Tracked::whole(*z), false));
                }
                None => {}
                Some(t) if t.z.anim != z.anim || t.z.speed != z.speed => {
                    due.push(Due { press: whole, at: next.len(), update: Update::Whole });
                    next.push((*t, true));
                }
                Some(t) => {
                    let (ex, ey) = (z.x as i32 * SUB - t.px, z.y as i32 * SUB - t.py);
                    let off = ex.abs().max(ey.abs()) as f32 / SUB as f32;
                    let turn = z.facing.wrapping_sub(t.z.facing);
                    let turn = turn.min(turn.wrapping_neg()) as f32;
                    let press = (off / allowed).max(turn / (TURN_MIN + dist * TURN_PER_M));
                    if press > 1.0 {
                        let step = |e: i32| (e + if e < 0 { -SUB / 2 } else { SUB / 2 }) / SUB;
                        let (dx, dy) = (step(ex), step(ey));
                        let update = if dx.abs() <= 127 && dy.abs() <= 127 { Update::Move(dx as i8, dy as i8) } else { Update::Whole };
                        due.push(Due { press, at: next.len(), update });
                    }
                    next.push((*t, true));
                }
            }
        }
        z_gone.extend(old.map(|t| t.z.id));
        // What fits, most pressing first (ties by id, so the same every run).
        let mut order: Vec<usize> = (0..due.len()).collect();
        order.sort_by(|&a, &b| due[b].press.total_cmp(&due[a].press).then(due[a].at.cmp(&due[b].at)));
        let mut left = limits.zombie_bytes.saturating_sub(z_gone.len() * GONE_BYTES);
        let mut send = vec![false; due.len()];
        for k in order {
            let cost = if matches!(due[k].update, Update::Whole) { WHOLE_BYTES } else { MOVE_BYTES };
            if cost <= left {
                left -= cost;
                send[k] = true;
            }
        }
        let (mut z_changed, mut z_moves) = (Vec::new(), Vec::<ZombieMove>::new());
        let mut last_id = 0;
        for (d, _) in due.iter().zip(&send).filter(|(_, s)| **s) {
            let z = actual[..].binary_search_by_key(&next[d.at].0.z.id, |(z, _)| z.id).map(|k| actual[k].0).expect("a due zombie is in view");
            let (t, live) = &mut next[d.at];
            match d.update {
                Update::Whole => {
                    (*t, *live) = (Tracked::whole(z), true);
                    z_changed.push(z);
                }
                Update::Move(dx, dy) => {
                    t.moved(dx, dy, z.facing);
                    z_moves.push(ZombieMove { gap: z.id - last_id, dx, dy, facing: z.facing });
                    last_id = z.id;
                }
            }
        }
        self.zombies = next.into_iter().filter(|(_, live)| *live).map(|(t, _)| t).collect();
        self.stats = EncodeStats { known: self.zombies.len() as u32, whole: z_changed.len() as u32, moves: z_moves.len() as u32, gone: z_gone.len() as u32, held: send.iter().filter(|s| !**s).count() as u32 };

        // Our own player whole; the others packed, and only when they change.
        let (own, rest): (Vec<PlayerView>, Vec<PlayerView>) = view.players.iter().partition(|p| p.id == player);
        let mut others: Vec<PlayerPacked> = rest.iter().map(PlayerPacked::pack).collect();
        others.sort_by_key(|p| p.id);
        let (o_changed, o_gone) = diff(&self.others, &others, |p| p.id);
        self.others = others;

        let mut items = view.items;
        items.sort_by_key(|i| i.id);
        let (i_changed, i_gone) = diff(&self.items, &items, |i| i.id);
        // Of the items this client has, the ones upstairs.
        let upstairs: Vec<u32> = view.upstairs.iter().copied().filter(|id| items.binary_search_by_key(id, |i| i.id).is_ok()).collect();
        let upstairs = (first || upstairs != self.upstairs).then(|| {
            self.upstairs = upstairs.clone();
            upstairs
        });
        self.items = items;

        let floor = view.floor.into_iter().next().map(|f| f.items).unwrap_or_default();
        let floor = (first || floor != self.floor).then(|| {
            self.floor = floor.clone();
            floor
        });
        let run = view.runs.into_iter().next();
        // What should show at once: the counts, a death, and the first night's objectives and outcome.
        let counts = |v: &RunView| {
            let (r, f) = (&v.run, v.first_night.as_ref());
            let night = f.map(|f| (f.armed, f.food, f.drink, f.sheltered, f.outcome.is_some(), f.began.to_bits()));
            (r.kills, r.items_taken, r.searched, r.wounds, r.death.is_some(), night)
        };
        let run_due = match (&self.run, &run) {
            (Some(a), Some(b)) => counts(a) != counts(b) || (a != b && view.tick % RUN_EVERY == 0),
            (None, None) => false,
            _ => true,
        };
        let run = if first || run_due {
            self.run = run.clone();
            run
        } else {
            None
        };
        let outfits = (first || view.outfits != self.outfits).then(|| {
            self.outfits = view.outfits.clone();
            view.outfits
        });
        let looks = (first || view.looks != self.looks).then(|| {
            self.looks = view.looks.clone();
            view.looks
        });
        let group = view.groups.into_iter().next();
        let group = (group != self.group).then(|| {
            self.group = group.clone();
            group
        });
        let lights = (view.lit_rooms.clone(), view.grid_on);
        let lights = (first || lights != self.lights).then(|| {
            self.lights = lights.clone();
            lights
        });
        let map = view.maps.into_iter().next().map(|m| m.explored).unwrap_or_default();
        let map = (first || map != self.map).then(|| {
            self.map = map.clone();
            map
        });
        let fires = (first || view.fires != self.fires).then(|| {
            self.fires = view.fires.clone();
            view.fires
        });
        SnapshotDelta {
            group,
            lights,
            map,
            fires,
            tick: view.tick,
            time_of_day: view.time_of_day,
            time_scale: view.time_scale,
            game_minute: view.game_minute,
            players: own,
            others: o_changed,
            others_gone: o_gone,
            zombies: z_changed,
            zombie_moves: z_moves,
            zombies_gone: z_gone,
            items: i_changed,
            items_gone: i_gone,
            edge_changes: view.edge_changes,
            edge_damage: view.edge_damage,
            blind_changes: view.blind_changes,
            tree_changes: view.tree_changes,
            hits: view.hits,
            private: view.private,
            item_notes: view.item_notes,
            level_ups: view.level_ups,
            heard: view.heard.iter().map(|h| (h.pos, h.radius)).collect(),
            speech: view.speech,
            floor,
            upstairs,
            run,
            outfits,
            looks,
        }
    }
}

/// The client's side of [`DeltaEncoder`]: keeps the last whole snapshot and rebuilds the next.
#[derive(Default)]
pub struct DeltaDecoder {
    zombies: Vec<Tracked>,
    others: Vec<PlayerPacked>,
    last: Snapshot,
}

impl DeltaDecoder {
    /// The whole snapshot `delta` stands for, for the client playing `player`. Things that
    /// happened (edge changes, hits, notes) are in the snapshot they came with only.
    pub fn apply(&mut self, delta: SnapshotDelta, player: u32) -> Snapshot {
        let s = &mut self.last;
        (s.tick, s.time_of_day, s.time_scale, s.game_minute) = (delta.tick, delta.time_of_day, delta.time_scale, delta.game_minute);
        patch(&mut self.others, &delta.others, &delta.others_gone, |p| p.id);
        s.players = delta.players;
        s.players.extend(self.others.iter().map(PlayerPacked::unpack));
        s.players.sort_by_key(|p| p.id);
        // Zombies: carried on a tick, as the server carried them, then what it sent.
        for t in &mut self.zombies {
            t.advance();
        }
        if !delta.zombies_gone.is_empty() {
            self.zombies.retain(|t| delta.zombies_gone.binary_search(&t.z.id).is_err());
        }
        for z in &delta.zombies {
            match self.zombies.binary_search_by_key(&z.id, |t| t.z.id) {
                Ok(k) => self.zombies[k] = Tracked::whole(*z),
                Err(k) => self.zombies.insert(k, Tracked::whole(*z)),
            }
        }
        let mut id = 0u32;
        for m in &delta.zombie_moves {
            id = id.wrapping_add(m.gap);
            if let Ok(k) = self.zombies.binary_search_by_key(&id, |t| t.z.id) {
                self.zombies[k].moved(m.dx, m.dy, m.facing);
            }
        }
        s.zombies = self.zombies.iter().map(|t| t.shown().unpack()).collect();
        patch(&mut s.items, &delta.items, &delta.items_gone, |i| i.id);
        s.edge_changes = delta.edge_changes;
        s.edge_damage = delta.edge_damage;
        s.blind_changes = delta.blind_changes;
        s.tree_changes = delta.tree_changes;
        s.hits = delta.hits;
        s.private = delta.private;
        for v in &mut s.private {
            v.restore_homes();
        }
        s.item_notes = delta.item_notes;
        s.level_ups = delta.level_ups;
        s.heard = delta.heard.iter().map(|&(pos, radius)| zombye_sim::Heard { listener: player, pos, radius, at: delta.tick }).collect();
        s.speech = delta.speech;
        if let Some(items) = delta.floor {
            s.floor = vec![FloorView { player, items }];
        }
        if let Some(up) = delta.upstairs {
            s.upstairs = up;
        }
        if let Some((rooms, grid)) = delta.lights {
            (s.lit_rooms, s.grid_on) = (rooms, grid);
        }
        if let Some(fires) = delta.fires {
            s.fires = fires;
        }
        if let Some(run) = delta.run {
            s.runs = vec![RunView { player, ..run }];
        }
        if let Some(outfits) = delta.outfits {
            s.outfits = outfits;
        }
        if let Some(looks) = delta.looks {
            s.looks = looks;
        }
        if let Some(group) = delta.group {
            s.groups = group.into_iter().collect();
        }
        if let Some(explored) = delta.map {
            s.maps = if explored.w == 0 { Vec::new() } else { vec![crate::MapView { player, explored }] };
        }
        s.clone()
    }
}

/// `msg` as a frame: its length, then its postcard bytes.
pub fn encode_frame<T: Serialize>(msg: &T) -> Vec<u8> {
    let mut frame = vec![0u8; 4];
    frame = postcard::to_extend(msg, frame).expect("messages serialize");
    let len = (frame.len() - 4) as u32;
    frame[..4].copy_from_slice(&len.to_le_bytes());
    frame
}

/// Read one frame of at most `max` bytes and decode it. Returns the message and the bytes it took
/// on the wire. A longer frame, or one that doesn't decode, is `InvalidData`.
pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read, max: usize) -> io::Result<(T, usize)> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a frame of {len} bytes (at most {max})")));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let msg = postcard::from_bytes(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("a frame that doesn't decode: {e}")))?;
    Ok((msg, len + 4))
}

/// Write `msg` as one frame. Returns the bytes written.
pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<usize> {
    let frame = encode_frame(msg);
    w.write_all(&frame)?;
    Ok(frame.len())
}

impl Intent {
    /// The player this intent says it is from; `None` for the debug and admin intents that name
    /// none. A network server overwrites it with the sender's own player.
    pub fn player_mut(&mut self) -> Option<&mut u32> {
        match self {
            Intent::PlayerInput { player, .. }
            | Intent::Toward { player, .. }
            | Intent::Interact { player }
            | Intent::Climb { player }
            | Intent::Blind { player, .. }
            | Intent::SmashWindow { player }
            | Intent::Light { player, .. }
            | Intent::Fire { player, .. }
            | Intent::Attack { player }
            | Intent::Shove { player }
            | Intent::Stomp { player }
            | Intent::PickUp { player }
            | Intent::MoveItem { player, .. }
            | Intent::Equip { player, .. }
            | Intent::Wear { player, .. }
            | Intent::Unequip { player, .. }
            | Intent::Drop { player, .. }
            | Intent::Use { player, .. }
            | Intent::Search { player, .. }
            | Intent::Treat { player, .. }
            | Intent::Craft { player, .. }
            | Intent::Sleep { player, .. }
            | Intent::Sit { player, .. }
            | Intent::CancelJobs { player, .. }
            | Intent::Seen { player, .. }
            | Intent::Respawn { player }
            | Intent::Say { player, .. }
            | Intent::Gesture { player, .. }
            | Intent::Restrain { player, .. }
            | Intent::Free { player, .. }
            | Intent::Frisk { player, .. }
            | Intent::TakeFrom { player, .. }
            | Intent::StopHands { player }
            | Intent::Struggle { player, .. }
            | Intent::Group { player, .. }
            | Intent::Aim { player, .. }
            | Intent::Dev { player, .. } => Some(player),
            Intent::SpawnItem { .. } | Intent::SetGoal { .. } | Intent::SpawnZombies { .. } | Intent::SkipTime { .. } | Intent::Grid { .. } | Intent::MakeNoise { .. } => None,
        }
    }

    /// Only for developers: the dev toggles and the debug and admin intents. A multiplayer server
    /// takes them only with `--allow-dev`.
    pub fn dev_only(&self) -> bool {
        matches!(self, Intent::Dev { .. } | Intent::SpawnItem { .. } | Intent::SetGoal { .. } | Intent::SpawnZombies { .. } | Intent::SkipTime { .. } | Intent::Grid { .. } | Intent::MakeNoise { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LocalServer, content};

    /// A client that applies every delta has, each tick, the snapshot the server cut for it:
    /// players and everything of its own exactly, zombies to the wire's precision; and the deltas
    /// are a fraction of the size of whole snapshots.
    #[test]
    fn deltas_rebuild_each_clients_snapshot() {
        let mut config = content::load_config(&content::default_data_dir(), 3).unwrap();
        config.zombies = Some(1500);
        let mut server = LocalServer::new(config);
        let a = server.join(None);
        let b = server.join(Some((server.world.player_spawn + glam::Vec2::new(30.0, 0.0)).to_array()));
        let radius = server.world.config.tiers.interest_radius;
        let (mut encoder, mut decoder) = (DeltaEncoder::default(), DeltaDecoder::default());
        let (mut whole, mut sent, mut moves, mut in_view, mut worst_near) = (0usize, 0usize, 0usize, 0usize, 0.0f32);
        let bytes = |s: &Snapshot| postcard::to_allocvec(s).unwrap().len();
        for t in 0..300u32 {
            let dir = glam::Vec2::from_angle(t as f32 * 0.02);
            server.send(Intent::PlayerInput { player: a, seq: t + 1, move_dir: dir.to_array(), facing: 0.0, run: true, sneak: false, aim: false });
            if t == 40 {
                // Something for the floor, the items and a note.
                let at = server.world.player(a).unwrap().state.pos;
                server.send(Intent::SpawnItem { item: "tinned_beans".into(), count: 2, pos: at.to_array() });
            }
            if t == 60 {
                server.send(Intent::PickUp { player: a });
                server.send(Intent::MakeNoise { pos: server.world.player(a).unwrap().state.pos.to_array(), radius: 60.0, lasting: false });
            }
            let full = server.tick();
            let view = full.for_client(a, radius);
            assert!(view.players.iter().any(|p| p.id == a));
            assert!(view.private.iter().all(|v| v.player == a) && view.runs.len() == 1 && view.floor.len() == 1);
            assert!(view.zombies.len() <= full.zombies.len());
            let frame = encode_frame(&ServerMsg::Snapshot(Box::new(encoder.encode(view.clone(), a, Limits::default()))));
            let (msg, n) = read_frame::<ServerMsg>(&mut frame.as_slice(), MAX_SERVER_FRAME).unwrap();
            assert_eq!(n, frame.len());
            let ServerMsg::Snapshot(delta) = msg else { panic!("a snapshot") };
            moves += delta.zombie_moves.len();
            assert!(delta.players.len() == 1 && delta.players[0].id == a, "our own player whole, the others packed");
            let got = decoder.apply(*delta, a);
            whole += bytes(&view);
            sent += frame.len();

            let own = |s: &Snapshot| s.players.iter().find(|p| p.id == a).copied().unwrap();
            let others = |s: &Snapshot| s.players.iter().filter(|p| p.id != a).map(PlayerPacked::pack).collect::<Vec<_>>();
            let exact = |s: &Snapshot| postcard::to_allocvec(&(s.tick, s.time_of_day, s.game_minute, own(s), others(s), &s.items_sorted(), &s.floor, &s.outfits, &s.looks, &s.private, &s.item_notes, &s.hits, &s.level_ups, &s.speech, &s.maps)).unwrap();
            assert_eq!(exact(&got), exact(&view), "tick {t}");
            // The map screen's record: only our own, whole (it is sent only when it grows).
            assert!(got.maps.len() == 1 && got.maps[0].player == a && got.maps[0].explored.count() > 0 || t < 10, "tick {t}");
            assert!(others(&got).len() == 1 && got.players.iter().all(|p| p.id == a || p.last_input_seq == 0), "nothing of another player's that can't be seen");
            assert_eq!(got.zombies.len(), view.zombies.len(), "tick {t}");
            let me = glam::Vec2::from(own(&view).pos);
            for (g, v) in got.zombies.iter().zip(&view.zombies) {
                assert_eq!((g.id, g.anim), (v.id, v.anim), "tick {t}");
                // As close as its distance from us asks (and the wire's steps allow: half a step
                // in what the server compares, half in what the client shows).
                let dist = me.distance(v.pos.into());
                let (off, allowed) = (glam::Vec2::from(g.pos) - glam::Vec2::from(v.pos), (OFF_MIN + dist * OFF_PER_M + 1.01) / POS_STEPS);
                assert!(off.abs().max_element() <= allowed, "zombie {} {dist:.0} m off at tick {t}: {:?} for {:?}", g.id, g.pos, v.pos);
                let turn = (g.facing - v.facing).rem_euclid(std::f32::consts::TAU);
                assert!(turn.min(std::f32::consts::TAU - turn) <= (TURN_MIN + dist * TURN_PER_M + 1.0) / 256.0 * std::f32::consts::TAU, "zombie {} faces {} for {}", g.id, g.facing, v.facing);
                assert!((g.speed - v.speed).abs() < 0.011);
            }
            worst_near = got.zombies.iter().zip(&view.zombies).filter(|(_, v)| me.distance(v.pos.into()) < 10.0).map(|(g, v)| glam::Vec2::from(g.pos).distance(v.pos.into())).fold(worst_near, f32::max);
            in_view += view.zombies.len();
            let (g, v) = (got.run_of(a).unwrap(), view.run_of(a).unwrap());
            assert_eq!((g.kills, g.items_taken, g.death.is_some()), (v.kills, v.items_taken, v.death.is_some()));
            assert!((g.secs - v.secs).abs() <= 1.0, "the run's clock is at most a second old");
        }
        let _ = b;
        println!("{} zombies in view a tick: {sent} bytes sent ({:.1} KB/s) for {whole} whole, {moves} moves ({:.2} a zombie a tick); within 10 m at most {:.1} cm off", in_view / 300, sent as f32 / 15.0 / 1000.0, moves as f32 / in_view as f32, worst_near * 100.0);
        assert!(moves > 300, "walking zombies go as moves ({moves})");
        assert!(moves * 3 < in_view, "a zombie carried on needs a correction only now and then: {moves} moves for {in_view} zombie-ticks");
        assert!(worst_near < 0.06, "zombies close by are where the server has them ({worst_near} m)");
        assert!(sent * 10 < whole, "deltas took {sent} bytes, whole snapshots {whole}");
    }

    /// A horde far bigger than the bytes allowed: every snapshot stays within them, the zombies
    /// close to the survivor are still exact, the rest are coarser but all there, and once the
    /// rush of new ones is through, nothing waits long.
    #[test]
    fn a_horde_over_budget_gets_coarser_with_distance() {
        const BYTES: usize = 500;
        let mut config = content::load_config(&content::default_data_dir(), 3).unwrap();
        config.zombies = Some(0);
        let mut server = LocalServer::new(config);
        let at = server.world.player_spawn + glam::Vec2::new(40.0, 0.0);
        let a = server.join(Some(at.to_array()));
        server.send(Intent::Dev { player: a, god: Some(true), ignored: None });
        server.send(Intent::SpawnZombies { count: 1500, center: at.to_array(), radius: 70.0 });
        let radius = server.world.config.tiers.interest_radius;
        let (mut encoder, mut decoder) = (DeltaEncoder::default(), DeltaDecoder::default());
        let (mut held, mut most, mut near_off, mut far_off, mut seen) = (0u32, 0usize, 0.0f32, 0.0f32, 0usize);
        for t in 0..400u32 {
            let dir = glam::Vec2::from_angle(t as f32 * 0.03);
            server.send(Intent::PlayerInput { player: a, seq: t + 1, move_dir: dir.to_array(), facing: 0.0, run: false, sneak: false, aim: false });
            let view = server.tick().for_client(a, radius);
            let delta = encoder.encode(view.clone(), a, Limits { admit: radius, zombie_bytes: BYTES });
            let zombie_bytes = postcard::to_allocvec(&(&delta.zombies, &delta.zombie_moves, &delta.zombies_gone)).unwrap().len();
            most = most.max(zombie_bytes);
            held += encoder.stats.held;
            let got = decoder.apply(delta, a);
            assert_eq!(got.zombies.len() as u32, encoder.stats.known);
            if t < 200 {
                continue;
            }
            // Settled: everyone in view is there.
            assert_eq!(got.zombies.len(), view.zombies.len(), "tick {t}");
            seen = seen.max(got.zombies.len());
            let me = glam::Vec2::from(view.players[0].pos);
            for (g, v) in got.zombies.iter().zip(&view.zombies) {
                assert_eq!(g.id, v.id);
                let (dist, off) = (me.distance(v.pos.into()), glam::Vec2::from(g.pos).distance(v.pos.into()));
                if dist < 8.0 {
                    near_off = near_off.max(off);
                    assert_eq!(g.anim, v.anim, "zombie {} {dist:.1} m away at tick {t}", g.id);
                } else {
                    far_off = far_off.max(off);
                }
            }
        }
        println!("{seen} zombies in view on {BYTES} bytes a tick: at most {most} bytes of zombies, {held} updates held back; off by at most {:.1} cm within 8 m, {:.2} m beyond", near_off * 100.0, far_off);
        assert!(seen > 1000, "a horde in view ({seen})");
        assert!(most <= BYTES + BYTES / 5, "zombie updates took {most} bytes of {BYTES}");
        assert!(held > 1000, "the budget was the limit ({held} held)");
        assert!(near_off < 0.08, "zombies in reach are where the server has them ({near_off} m)");
        assert!(far_off < 3.0, "and none is left far behind ({far_off} m)");
    }

    /// Map changes go only to clients with the chunk in their interest; one that comes near later
    /// gets each edge as it is by then, once.
    #[test]
    fn map_changes_wait_until_a_client_is_near() {
        use zombye_sim::map::Edge;
        let mut config = content::load_config(&content::default_data_dir(), 3).unwrap();
        config.zombies = Some(0);
        let mut server = LocalServer::new(config);
        let a = server.join(None);
        let far = server.world.player_spawn + glam::Vec2::new(200.0, 0.0);
        let b = server.join(Some(far.to_array()));
        let radius = server.world.config.tiers.interest_radius;
        // Two doors beside A: one opened then broken, one battered.
        let home = server.world.player_spawn;
        let mut doors: Vec<EdgeRef> = Vec::new();
        let t0 = home.floor().as_ivec2();
        for dy in -12..=12 {
            for dx in -12..=12 {
                for edge in [zombye_sim::map::OwnedEdge::North, zombye_sim::map::OwnedEdge::West] {
                    let e = EdgeRef::new(t0 + glam::IVec2::new(dx, dy), edge);
                    if server.world.map.owned_edge(e.tile, e.edge) == Edge::DoorClosed {
                        doors.push(e);
                    }
                }
            }
        }
        assert!(doors.len() >= 2, "the spawn house has doors");
        let (mut ia, mut ib) = (MapInterest::default(), MapInterest::default());
        let views = |server: &mut LocalServer, ia: &mut MapInterest, ib: &mut MapInterest| {
            let full = server.tick();
            let mut out = Vec::new();
            for (id, interest) in [(a, ia), (b, ib)] {
                let mut view = full.for_client(id, radius);
                let me = server.world.player(id).map(|p| p.state.pos);
                interest.cut(&mut view, me, radius, &server.world);
                out.push(view);
            }
            out
        };
        server.world.set_edge(doors[0], Edge::DoorOpen);
        server.world.damage_edge(doors[1], 10.0);
        let v = views(&mut server, &mut ia, &mut ib);
        assert_eq!((v[0].edge_changes.len(), v[0].edge_damage.len()), (1, 1), "A is beside them");
        assert_eq!((v[1].edge_changes.len(), v[1].edge_damage.len()), (0, 0), "B is 200 m off");
        assert_eq!((ia.owed(), ib.owed()), (0, 2));
        server.world.set_edge(doors[0], Edge::DoorBroken);
        let v = views(&mut server, &mut ia, &mut ib);
        assert_eq!((v[0].edge_changes.len(), v[1].edge_changes.len(), ib.owed()), (1, 0, 2), "changed again: still one owed for it");
        // B comes over: each edge once, as it is now.
        server.world.players.iter_mut().find(|p| p.id == b).unwrap().state.pos = home + glam::Vec2::new(30.0, 0.0);
        let v = views(&mut server, &mut ia, &mut ib);
        assert_eq!(v[1].edge_changes.len(), 1);
        assert_eq!((v[1].edge_changes[0].tile, v[1].edge_changes[0].kind), (doors[0].tile, Edge::DoorBroken));
        assert_eq!(v[1].edge_damage.len(), 1);
        let (hp, max) = server.world.edge_hp(doors[1]).unwrap();
        assert!(v[1].edge_damage[0].tile == doors[1].tile && (v[1].edge_damage[0].hp - hp / max).abs() < 1e-6 && hp < max);
        assert_eq!(ib.owed(), 0);
        let v = views(&mut server, &mut ia, &mut ib);
        assert!(v[1].edge_changes.is_empty() && v[1].edge_damage.is_empty(), "and not again");
    }

    impl Snapshot {
        fn items_sorted(&self) -> Vec<GroundItem> {
            let mut items = self.items.clone();
            items.sort_by_key(|i| i.id);
            items
        }
    }

    #[test]
    fn frames_are_checked() {
        let frame = encode_frame(&ClientMsg::Ping(7));
        let (msg, n) = read_frame::<ClientMsg>(&mut frame.as_slice(), MAX_CLIENT_FRAME).unwrap();
        assert!(matches!(msg, ClientMsg::Ping(7)) && n == frame.len());
        // Too long, cut short, and not a message.
        let long = [&(MAX_CLIENT_FRAME as u32 + 1).to_le_bytes()[..], &[0; 8]].concat();
        assert_eq!(read_frame::<ClientMsg>(&mut long.as_slice(), MAX_CLIENT_FRAME).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert!(read_frame::<ClientMsg>(&mut &frame[..frame.len() - 1], MAX_CLIENT_FRAME).is_err());
        let junk = [&3u32.to_le_bytes()[..], &[250, 250, 250]].concat();
        assert_eq!(read_frame::<ClientMsg>(&mut junk.as_slice(), MAX_CLIENT_FRAME).unwrap_err().kind(), io::ErrorKind::InvalidData);
        // Every intent says whose it is, or that it is for developers.
        let mut i = Intent::Attack { player: 3 };
        *i.player_mut().unwrap() = 9;
        assert!(matches!(i, Intent::Attack { player: 9 }) && !i.dev_only());
        assert!(Intent::SkipTime { hours: 1.0 }.dev_only() && Intent::SkipTime { hours: 1.0 }.player_mut().is_none());
    }
}
