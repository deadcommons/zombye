//! The controller (#12): playing with a gamepad as well as, or instead of, the keyboard and mouse.
//!
//! **Its buttons are data,** as the keys are: `assets/data/pad.ron` (the data folder's, else the
//! one built in) says which button does each action, with what the player picked on the settings
//! screen's Controls tab over it (`ClientSettings::pad`). An action is one of the keyboard's
//! ([`crate::keys::Action`]) or one only a controller needs ([`Only`]: attack, aim, the camera's
//! turns and zoom, first person, pause), and a binding is a button, pressed or held
//! ([`Bind`]). A button with a hold action does its plain one when let go sooner.
//!
//! **How the rest of the game hears it:** each frame [`read_pad`] (in `PreUpdate`, before any
//! other key handler) works out which actions the pad started, holds and let go of, and
//! [`crate::keys::just_pressed`] / [`pressed`](crate::keys::pressed) /
//! [`just_released`](crate::keys::just_released) answer for the pad as well as the keys, so every
//! system that asks for an action follows the pad with no change. The keys that aren't actions
//! (Esc, `,` `.` `;`) are pressed for the game by the pad ([`Injected`]): the pause button and B
//! on a screen are Esc, the bumpers turn the camera, a held View is `;`. What only a controller
//! does (the sticks, the triggers, aiming, zoom) is the [`Pad`] resource, which `player.rs` and
//! `camera.rs` read.
//!
//! **Which is in use** ([`InputMode`]): the Input setting (Auto, Keyboard and mouse, Controller;
//! `ClientSettings::input`, `--input`). On Auto the game follows the last thing touched: a pad
//! button or a stick pushed past its dead zone makes the controller the one in use, a key or a
//! mouse click the keyboard (moving the mouse alone never does). A pad plugged in says so on the
//! yellow line. Nothing of it is in the simulation, a save or a snapshot: the pad sends the same
//! intents the keys and mouse do.

use crate::keys::Action;
use bevy::input::InputSystems;
use bevy::input::gamepad::{Gamepad, GamepadButton, GamepadConnection, GamepadConnectionEvent};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

const SHIPPED: &str = include_str!("../../../assets/data/pad.ron");

/// How long a button is held before its hold action happens (seconds).
pub const HOLD_SECS: f32 = 0.35;
/// A stick's dead zone: pushed less than this far it reads as untouched.
pub const DEAD_ZONE: f32 = 0.2;
/// How far a stick must go to count as the player picking up the controller (Auto).
const WAKE_STICK: f32 = 0.5;

/// A controller's buttons, by where they are on the common pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Button {
    /// The four on the right face: bottom, right, left, top.
    A,
    B,
    X,
    Y,
    LB,
    RB,
    LT,
    RT,
    /// The sticks pressed in.
    L3,
    R3,
    Up,
    Down,
    Left,
    Right,
    /// The two small buttons in the middle: left, right.
    View,
    Menu,
}

impl Button {
    pub const ALL: [Button; 16] = [
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::LB,
        Button::RB,
        Button::LT,
        Button::RT,
        Button::L3,
        Button::R3,
        Button::Up,
        Button::Down,
        Button::Left,
        Button::Right,
        Button::View,
        Button::Menu,
    ];

    /// Its name in `pad.ron` and `settings.ron`.
    pub fn id(self) -> &'static str {
        match self {
            Button::A => "A",
            Button::B => "B",
            Button::X => "X",
            Button::Y => "Y",
            Button::LB => "LB",
            Button::RB => "RB",
            Button::LT => "LT",
            Button::RT => "RT",
            Button::L3 => "L3",
            Button::R3 => "R3",
            Button::Up => "Up",
            Button::Down => "Down",
            Button::Left => "Left",
            Button::Right => "Right",
            Button::View => "View",
            Button::Menu => "Menu",
        }
    }

    pub fn from_id(id: &str) -> Option<Button> {
        Button::ALL.into_iter().find(|b| b.id().eq_ignore_ascii_case(id))
    }

    pub fn gamepad(self) -> GamepadButton {
        match self {
            Button::A => GamepadButton::South,
            Button::B => GamepadButton::East,
            Button::X => GamepadButton::West,
            Button::Y => GamepadButton::North,
            Button::LB => GamepadButton::LeftTrigger,
            Button::RB => GamepadButton::RightTrigger,
            Button::LT => GamepadButton::LeftTrigger2,
            Button::RT => GamepadButton::RightTrigger2,
            Button::L3 => GamepadButton::LeftThumb,
            Button::R3 => GamepadButton::RightThumb,
            Button::Up => GamepadButton::DPadUp,
            Button::Down => GamepadButton::DPadDown,
            Button::Left => GamepadButton::DPadLeft,
            Button::Right => GamepadButton::DPadRight,
            Button::View => GamepadButton::Select,
            Button::Menu => GamepadButton::Start,
        }
    }

    /// What it is called on a pad of this family ("A", "Cross", "B" for the bottom one on a
    /// Nintendo pad).
    pub fn label(self, family: Family) -> &'static str {
        use self::Button::*;
        match family {
            Family::Xbox => match self {
                A => "A",
                B => "B",
                X => "X",
                Y => "Y",
                LB => "LB",
                RB => "RB",
                LT => "LT",
                RT => "RT",
                L3 => "LS",
                R3 => "RS",
                Up => "D-pad up",
                Down => "D-pad down",
                Left => "D-pad left",
                Right => "D-pad right",
                View => "View",
                Menu => "Menu",
            },
            Family::PlayStation => match self {
                A => "Cross",
                B => "Circle",
                X => "Square",
                Y => "Triangle",
                LB => "L1",
                RB => "R1",
                LT => "L2",
                RT => "R2",
                L3 => "L3",
                R3 => "R3",
                Up => "D-pad up",
                Down => "D-pad down",
                Left => "D-pad left",
                Right => "D-pad right",
                View => "Create",
                Menu => "Options",
            },
            // Named by where they are: Nintendo's bottom face button is B, its right one A.
            Family::Nintendo => match self {
                A => "B",
                B => "A",
                X => "Y",
                Y => "X",
                LB => "L",
                RB => "R",
                LT => "ZL",
                RT => "ZR",
                L3 => "L stick",
                R3 => "R stick",
                Up => "D-pad up",
                Down => "D-pad down",
                Left => "D-pad left",
                Right => "D-pad right",
                View => "-",
                Menu => "+",
            },
        }
    }
}

/// Which names a pad's buttons go by (from its maker; anything else: the common names).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Family {
    #[default]
    Xbox,
    PlayStation,
    Nintendo,
}

impl Family {
    /// By the USB vendor, else by words in the name the system gives it.
    pub fn of(vendor: Option<u16>, name: &str) -> Family {
        match vendor {
            Some(0x054c) => return Family::PlayStation,
            Some(0x057e) => return Family::Nintendo,
            Some(0x045e) | Some(0x28de) => return Family::Xbox,
            _ => {}
        }
        let n = name.to_lowercase();
        if ["playstation", "dualshock", "dualsense", "ps4", "ps5", "wireless controller"].iter().any(|w| n.contains(w)) {
            Family::PlayStation
        } else if ["nintendo", "switch", "pro controller", "joy-con"].iter().any(|w| n.contains(w)) {
            Family::Nintendo
        } else {
            Family::Xbox
        }
    }
}

/// What only a controller needs a button for: the keyboard has the mouse and fixed keys for these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Only {
    /// Swing or shoot (aiming by itself when [`Only::Aim`] isn't held).
    Attack,
    /// Aim, held: the right stick turns the aim.
    Aim,
    TurnLeft,
    TurnRight,
    ZoomIn,
    ZoomOut,
    FirstPerson,
    Pause,
    /// The item wheel, held (`wheel.rs`).
    ItemWheel,
    /// The make wheel, held.
    MakeWheel,
    /// The chat line, with the on-screen keyboard (`osk.rs`).
    Chat,
}

impl Only {
    pub const ALL: [Only; 11] = [Only::Attack, Only::Aim, Only::TurnLeft, Only::TurnRight, Only::ZoomIn, Only::ZoomOut, Only::FirstPerson, Only::Pause, Only::ItemWheel, Only::MakeWheel, Only::Chat];

    pub fn id(self) -> &'static str {
        match self {
            Only::Attack => "attack",
            Only::Aim => "aim",
            Only::TurnLeft => "turn_left",
            Only::TurnRight => "turn_right",
            Only::ZoomIn => "zoom_in",
            Only::ZoomOut => "zoom_out",
            Only::FirstPerson => "first_person",
            Only::Pause => "pause",
            Only::ItemWheel => "item_wheel",
            Only::MakeWheel => "make_wheel",
            Only::Chat => "chat",
        }
    }
}

/// An action a button can do: one of the keyboard's, or one only a controller needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PadAction {
    Key(Action),
    Only(Only),
}

impl PadAction {
    pub fn id(self) -> &'static str {
        match self {
            PadAction::Key(a) => a.id(),
            PadAction::Only(o) => o.id(),
        }
    }

    pub fn from_id(id: &str) -> Option<PadAction> {
        Action::from_id(id).map(PadAction::Key).or_else(|| Only::ALL.into_iter().find(|o| o.id() == id).map(PadAction::Only))
    }
}

/// `pad.ron`: each action's buttons, any of which does it. A button is its name on the common pad
/// (A, B, X, Y, LB, RB, LT, RT, L3, R3, Up, Down, Left, Right, View, Menu); "Hold " in front means
/// held for a moment. Left out: no button.
#[derive(serde::Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct PadFile {
    /// Walk up the screen (away from the camera).
    pub move_up: Option<Vec<String>>,
    /// Walk down the screen.
    pub move_down: Option<Vec<String>>,
    /// Walk left.
    pub move_left: Option<Vec<String>>,
    /// Walk right.
    pub move_right: Option<Vec<String>>,
    /// Run, until you stop (a toggle; the keyboard's is held).
    pub run: Option<Vec<String>>,
    /// Sneak, and stop sneaking.
    pub sneak: Option<Vec<String>>,
    /// Lie down to sleep, and get up.
    pub sleep: Option<Vec<String>>,
    /// Sit down, and get up.
    pub sit: Option<Vec<String>>,
    /// The likeliest thing to do with what you face: open or shut a door or window, climb through or over, get in or out of a car, flick a light switch, tend a fire, pick up, search. Held: the menu of everything you can do with it.
    pub interact: Option<Vec<String>>,
    /// Smash the window you face.
    pub smash: Option<Vec<String>>,
    /// The flashlight or candle in your hands, on and off.
    pub flashlight: Option<Vec<String>>,
    /// Beside a car: pour the fuel you carry into it, or with none siphon its fuel into a can.
    pub refuel: Option<Vec<String>>,
    /// At the wheel: the horn.
    pub horn: Option<Vec<String>>,
    /// Shove, or stomp one that's down; the handbrake at the wheel.
    pub shove: Option<Vec<String>>,
    /// Reload the gun in your hand.
    pub reload: Option<Vec<String>>,
    /// Stop what you're doing (with Shift: everything queued).
    pub stop: Option<Vec<String>>,
    /// The inventory screen.
    pub inventory: Option<Vec<String>>,
    /// The Health screen.
    pub health: Option<Vec<String>>,
    /// The Skills screen.
    pub skills: Option<Vec<String>>,
    /// The Make screen (crafting).
    pub make: Option<Vec<String>>,
    /// The build menu.
    pub build: Option<Vec<String>>,
    /// The map.
    pub map: Option<Vec<String>>,
    /// Hide or show the objectives and hints.
    pub objectives: Option<Vec<String>>,
    /// Swing or shoot. Without aim held it aims by itself, at what is in front of you.
    pub attack: Option<Vec<String>>,
    /// Aim, held: the right stick turns the aim, and the aim assist turns onto what is near it.
    pub aim: Option<Vec<String>>,
    /// Turn the camera a quarter left.
    pub turn_left: Option<Vec<String>>,
    /// Turn the camera a quarter right.
    pub turn_right: Option<Vec<String>>,
    /// Zoom in, held.
    pub zoom_in: Option<Vec<String>>,
    /// Zoom out, held.
    pub zoom_out: Option<Vec<String>>,
    /// First person, and back to the view from above.
    pub first_person: Option<Vec<String>>,
    /// The pause menu (Esc).
    pub pause: Option<Vec<String>>,
    /// The item wheel, held: the stick picks, letting go uses it.
    pub item_wheel: Option<Vec<String>>,
    /// The make wheel, held: what you can make here; letting go makes it.
    pub make_wheel: Option<Vec<String>>,
    /// The chat line, typed on the on-screen keyboard.
    pub chat: Option<Vec<String>>,
}

impl PadFile {
    /// Each action and its buttons, where the file gives them.
    pub fn entries(self) -> BTreeMap<String, Vec<String>> {
        let all = [
            (PadAction::Key(Action::MoveUp), self.move_up),
            (PadAction::Key(Action::MoveDown), self.move_down),
            (PadAction::Key(Action::MoveLeft), self.move_left),
            (PadAction::Key(Action::MoveRight), self.move_right),
            (PadAction::Key(Action::Run), self.run),
            (PadAction::Key(Action::Sneak), self.sneak),
            (PadAction::Key(Action::Sleep), self.sleep),
            (PadAction::Key(Action::Sit), self.sit),
            (PadAction::Key(Action::Interact), self.interact),
            (PadAction::Key(Action::Smash), self.smash),
            (PadAction::Key(Action::Flashlight), self.flashlight),
            (PadAction::Key(Action::Refuel), self.refuel),
            (PadAction::Key(Action::Horn), self.horn),
            (PadAction::Key(Action::Shove), self.shove),
            (PadAction::Key(Action::Reload), self.reload),
            (PadAction::Key(Action::Stop), self.stop),
            (PadAction::Key(Action::Inventory), self.inventory),
            (PadAction::Key(Action::Health), self.health),
            (PadAction::Key(Action::Skills), self.skills),
            (PadAction::Key(Action::Make), self.make),
            (PadAction::Key(Action::Build), self.build),
            (PadAction::Key(Action::Map), self.map),
            (PadAction::Key(Action::Objectives), self.objectives),
            (PadAction::Only(Only::Attack), self.attack),
            (PadAction::Only(Only::Aim), self.aim),
            (PadAction::Only(Only::TurnLeft), self.turn_left),
            (PadAction::Only(Only::TurnRight), self.turn_right),
            (PadAction::Only(Only::ZoomIn), self.zoom_in),
            (PadAction::Only(Only::ZoomOut), self.zoom_out),
            (PadAction::Only(Only::FirstPerson), self.first_person),
            (PadAction::Only(Only::Pause), self.pause),
            (PadAction::Only(Only::ItemWheel), self.item_wheel),
            (PadAction::Only(Only::MakeWheel), self.make_wheel),
            (PadAction::Only(Only::Chat), self.chat),
        ];
        all.into_iter().filter_map(|(a, b)| b.map(|b| (a.id().to_string(), b))).collect()
    }
}

/// A button, pressed or held for [`HOLD_SECS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Bind {
    pub button: Button,
    pub hold: bool,
}

impl Bind {
    /// "X", "Hold X" (case doesn't matter).
    pub fn parse(text: &str) -> Result<Bind, String> {
        let t = text.trim();
        let (hold, name) = match t.split_once(char::is_whitespace) {
            Some((h, rest)) if h.eq_ignore_ascii_case("hold") => (true, rest.trim()),
            _ => (false, t),
        };
        let button = Button::from_id(name).ok_or_else(|| format!("\"{text}\" is not a button (A, B, X, Y, LB, RB, LT, RT, L3, R3, Up, Down, Left, Right, View, Menu)"))?;
        Ok(Bind { button, hold })
    }

    /// As the file writes it.
    #[cfg(test)]
    pub fn text(self) -> String {
        if self.hold { format!("Hold {}", self.button.id()) } else { self.button.id().to_string() }
    }

    /// As a player reads it on a pad of this family: "Hold Square".
    pub fn label(self, family: Family) -> String {
        if self.hold { format!("Hold {}", self.button.label(family)) } else { self.button.label(family).to_string() }
    }
}

/// Each action's buttons.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PadBindings {
    map: BTreeMap<PadAction, Vec<Bind>>,
}

impl PadBindings {
    /// A `pad.ron` ([`PadFile`]). Unknown actions and buttons are errors naming them.
    pub fn parse(text: &str) -> Result<Self, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let file = options.from_str::<PadFile>(text).map_err(|e| format!("pad.ron: {e}"))?.entries();
        let mut b = PadBindings::default();
        b.apply(&file)?;
        Ok(b)
    }

    /// Put these (action id to buttons) over what is there; an empty list leaves the action with none.
    pub fn apply(&mut self, over: &BTreeMap<String, Vec<String>>) -> Result<(), String> {
        for (id, binds) in over {
            let a = PadAction::from_id(id).ok_or_else(|| format!("\"{id}\" is not an action"))?;
            let parsed = binds.iter().map(|t| Bind::parse(t).map_err(|e| format!("{id}: {e}"))).collect::<Result<Vec<_>, _>>()?;
            self.map.insert(a, parsed);
        }
        Ok(())
    }

    pub fn binds(&self, a: PadAction) -> &[Bind] {
        self.map.get(&a).map_or(&[], Vec::as_slice)
    }

    /// Does any action take a hold of this button (so its plain action waits for it to be let go)?
    pub fn has_hold(&self, b: Button) -> bool {
        self.map.values().flatten().any(|x| x.button == b && x.hold)
    }

    /// The actions this binding does.
    pub fn actions_of(&self, bind: Bind) -> impl Iterator<Item = PadAction> + '_ {
        self.map.iter().filter(move |(_, v)| v.contains(&bind)).map(|(a, _)| *a)
    }

    /// "A", "Hold X or Y" (nothing bound: "none").
    pub fn label(&self, a: PadAction, family: Family) -> String {
        let all: Vec<String> = self.binds(a).iter().map(|b| b.label(family)).collect();
        if all.is_empty() { "none".to_string() } else { all.join(" or ") }
    }
}

/// The game's own buttons: the shipped file (tested to parse).
pub fn shipped() -> PadBindings {
    PadBindings::parse(SHIPPED).expect("the shipped pad.ron parses")
}

static CURRENT: RwLock<Option<PadBindings>> = RwLock::new(None);
static BASE: RwLock<Option<PadBindings>> = RwLock::new(None);

/// The buttons in force now (a copy).
pub fn current() -> PadBindings {
    if let Some(b) = CURRENT.read().unwrap().as_ref() {
        return b.clone();
    }
    shipped()
}

/// Read the data folder's `pad.ron` (missing or broken: the shipped one, a broken one with a
/// warning) and put the player's picks over it.
pub fn init(data: Option<&std::path::Path>, overrides: &BTreeMap<String, Vec<String>>) {
    let base = data
        .map(|d| d.join("pad.ron"))
        .filter(|p| p.is_file())
        .and_then(|p| match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| PadBindings::parse(&t)) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("{}: {e}; using the game's own buttons", p.display());
                None
            }
        })
        .unwrap_or_else(shipped);
    *BASE.write().unwrap() = Some(base);
    set_overrides(overrides);
}

/// The player's picks over the base buttons (ones that don't parse left out, with a warning).
pub fn set_overrides(overrides: &BTreeMap<String, Vec<String>>) {
    let mut b = BASE.read().unwrap().clone().unwrap_or_else(shipped);
    for (id, binds) in overrides {
        if let Err(e) = b.apply(&BTreeMap::from([(id.clone(), binds.clone())])) {
            eprintln!("settings.ron pad {e}");
        }
    }
    *CURRENT.write().unwrap() = Some(b);
}

/// The keyboard actions the pad started, holds and let go of this frame, for `keys.rs`.
#[derive(Clone, Debug, Default)]
struct KeyFrame {
    just: Vec<Action>,
    held: Vec<Action>,
    released: Vec<Action>,
}

static KEYS: RwLock<KeyFrame> = RwLock::new(KeyFrame { just: Vec::new(), held: Vec::new(), released: Vec::new() });

/// Did the pad start this action this frame? (`keys::just_pressed` asks.)
pub(crate) fn just(a: Action) -> bool {
    KEYS.read().unwrap().just.contains(&a)
}

pub(crate) fn held(a: Action) -> bool {
    KEYS.read().unwrap().held.contains(&a)
}

pub(crate) fn released(a: Action) -> bool {
    KEYS.read().unwrap().released.contains(&a)
}

/// The Input setting: follow whichever was touched last, or keep to one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputSetting {
    #[default]
    Auto,
    Keyboard,
    Controller,
}

impl InputSetting {
    pub const ALL: [InputSetting; 3] = [InputSetting::Auto, InputSetting::Keyboard, InputSetting::Controller];

    pub fn name(self) -> &'static str {
        match self {
            InputSetting::Auto => "Auto",
            InputSetting::Keyboard => "Keyboard and mouse",
            InputSetting::Controller => "Controller",
        }
    }

    pub fn parse(text: &str) -> Option<InputSetting> {
        match text.to_lowercase().as_str() {
            "auto" => Some(InputSetting::Auto),
            "keyboard" | "kbm" | "mouse" => Some(InputSetting::Keyboard),
            "controller" | "pad" | "gamepad" => Some(InputSetting::Controller),
            _ => None,
        }
    }
}

/// Which the game is using now, and the pad it reads.
#[derive(Resource, Clone, Debug, Default)]
pub struct InputMode {
    pub setting: InputSetting,
    /// The controller is the one in use: prompts name its buttons, the pointer is hidden in play.
    pub pad: bool,
    /// The names the pad in use (or the last one plugged in) gives its buttons.
    pub family: Family,
    /// The pad read (the one last touched).
    gamepad: Option<Entity>,
}

impl InputMode {
    /// The pad read: the one last touched, if any has been.
    pub fn gamepad(&self) -> Option<Entity> {
        self.gamepad
    }

    /// The setting and what was touched last: is the controller the one in use?
    pub fn follow(setting: InputSetting, was_pad: bool, pad_touched: bool, keys_touched: bool) -> bool {
        match setting {
            InputSetting::Keyboard => false,
            InputSetting::Controller => true,
            InputSetting::Auto if pad_touched => true,
            InputSetting::Auto if keys_touched => false,
            InputSetting::Auto => was_pad,
        }
    }
}

/// What only a controller gives, this frame (read by `player.rs` and `camera.rs`). All zero
/// while the keyboard is in use, a screen holds the pad, or there is none.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct Pad {
    /// The left stick past its dead zone, scaled from it (0..1 long), y up the stick.
    pub stick: Vec2,
    /// The right stick, the same.
    pub look: Vec2,
    /// The triggers, 0..1 (driving).
    pub lt: f32,
    pub rt: f32,
    /// Aim held, and the attack started this frame.
    pub aim: bool,
    pub attack: bool,
    /// The run toggle (the left stick pressed in): on until the stick comes back to the middle.
    pub run: bool,
    /// Zoom, -1 out to 1 in, held.
    pub zoom: f32,
    /// The wheels' buttons (`wheel.rs`): started this frame, and held.
    pub items_start: bool,
    pub items_held: bool,
    pub make_start: bool,
    pub make_held: bool,
    /// While a wheel is up: the stick pushed further (it picks), A pressed, B pressed. The
    /// sticks then walk and look nowhere.
    pub select: Vec2,
    pub confirm: bool,
    pub cancel: bool,
    /// In the pause menu and the settings (`focus.rs`): the d-pad pressed this frame as a step
    /// (x right, y up), the left stick held, and the shoulder buttons tapped (a tab either way).
    pub nav_step: IVec2,
    pub nav_stick: Vec2,
    pub nav_tab: i32,
    /// The buttons pressed this frame, a bit each by [`Button`]'s order (for screens that take a
    /// button whatever it is bound to: the death and morning screens).
    pub pressed: u16,
}

impl Pad {
    /// Was this button pressed this frame?
    pub fn just(&self, b: Button) -> bool {
        self.pressed & (1 << b as u16) != 0
    }
}

/// Keys the pad pressed for the game this frame, let go of next frame.
#[derive(Resource, Default)]
pub struct Injected(Vec<KeyCode>);

/// Per button: since when it has been down, and whether its hold action has happened.
#[derive(Default)]
struct Held {
    since: BTreeMap<Button, (f32, bool)>,
    /// Tap actions that happened on a let-go, let go of the frame after.
    tapped: Vec<PadAction>,
}

/// A stick past the dead zone, rescaled so the dead zone's edge is 0 and full over is 1.
pub fn dead_zone(v: Vec2) -> Vec2 {
    let len = v.length();
    if len <= DEAD_ZONE {
        return Vec2::ZERO;
    }
    v / len * ((len - DEAD_ZONE) / (1.0 - DEAD_ZONE)).min(1.0)
}

pub struct PadPlugin;

impl Plugin for PadPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().get_resource::<crate::Options>().cloned();
        let settings = crate::ui_scale::SettingsStore::ensure(app);
        init(options.as_ref().map(|o| o.data.as_path()), &settings.pad);
        let setting = options.as_ref().and_then(|o| o.input).unwrap_or(settings.input);
        app.insert_resource(InputMode { setting, pad: setting == InputSetting::Controller, ..default() })
            .init_resource::<Pad>()
            .init_resource::<Injected>()
            .add_systems(
                PreUpdate,
                read_pad
                    .after(InputSystems)
                    .before(crate::settings_ui::capture)
                    .before(crate::browser_ui::keys)
                    .before(crate::chat::chat_keys)
                    .before(crate::inventory_ui::TextEntry)
                    .run_if(crate::live),
            )
            .init_resource::<Toast>()
            .add_systems(Startup, spawn_toast)
            .add_systems(Update, (connected, follow_setting, publish, show_toast).chain());
    }
}

// --- Prompts: what the UI calls a key, while the controller is in use ---------------------------

/// The controller is the one in use (a copy of [`InputMode::pad`], for the small spawn helpers
/// that every panel shares, as `help_text.rs` keeps its level).
static SHOWING: AtomicBool = AtomicBool::new(false);
static FAMILY: AtomicU8 = AtomicU8::new(0);

/// Keep the copies in step with [`InputMode`].
fn publish(mode: Res<InputMode>) {
    if mode.is_changed() {
        show(mode.pad, mode.family);
    }
}

/// Set what the prompts follow (also for tests and pictures).
pub fn show(pad: bool, family: Family) {
    SHOWING.store(pad, Ordering::Relaxed);
    FAMILY.store(family as u8, Ordering::Relaxed);
}

/// Prompts name the controller's buttons: it is the one in use.
pub fn showing() -> bool {
    SHOWING.load(Ordering::Relaxed)
}

/// The names the pad in use gives its buttons.
pub fn family() -> Family {
    match FAMILY.load(Ordering::Relaxed) {
        1 => Family::PlayStation,
        2 => Family::Nintendo,
        _ => Family::Xbox,
    }
}

/// A button by its common name, as the pad in use calls it ("B" is "Circle" on a PlayStation pad).
pub fn button(b: Button) -> String {
    b.label(family()).to_string()
}

/// The first button that does this action, as the pad in use calls it ("Hold LB"); none bound: None.
pub fn button_for(a: PadAction) -> Option<String> {
    current().binds(a).first().map(|b| b.label(family()))
}

/// What to show for a key the UI names (as the keyboard calls it: "Esc", "Enter", "E", "Shift +
/// B"): itself while the keyboard is in use; with the controller, the button that does the same,
/// or None when no button does (the hint is then left out). On the screens before and over the
/// game the pad's B is Esc and A is Enter (`focus.rs`).
pub fn prompt(key: &str) -> Option<String> {
    if !showing() {
        return Some(key.to_string());
    }
    prompt_for(&current(), &crate::keys::current(), family(), key)
}

/// [`prompt`] with what it reads handed in.
pub fn prompt_for(pad: &PadBindings, keys: &crate::keys::Bindings, family: Family, key: &str) -> Option<String> {
    let k = key.trim();
    match k.to_lowercase().as_str() {
        "esc" | "escape" => return Some(Button::B.label(family).to_string()),
        "enter" | "return" | "click" => return Some(Button::A.label(family).to_string()),
        // Tabs and pages: the bumpers.
        "tab" => return Some(Button::RB.label(family).to_string()),
        "shift+tab" | "shift + tab" => return Some(Button::LB.label(family).to_string()),
        _ => {}
    }
    // An action's key: that action's button.
    let a = Action::ALL.into_iter().find(|&a| keys.chords(a).iter().any(|c| c.text.replace('+', " + ").eq_ignore_ascii_case(k) || c.text.eq_ignore_ascii_case(k)))?;
    pad.binds(PadAction::Key(a)).first().map(|b| b.label(family))
}

/// What to show for an action's key: its keyboard key, or with the controller its button ("Y");
/// an action no button does: None.
pub fn prompt_action(a: Action) -> Option<String> {
    if showing() { button_for(PadAction::Key(a)) } else { Some(crate::keys::short(a)) }
}

/// The setting changed on the settings screen: follow it.
fn follow_setting(store: Res<crate::ui_scale::SettingsStore>, options: Res<crate::Options>, mut mode: ResMut<InputMode>) {
    if !store.is_changed() || options.input.is_some() {
        return;
    }
    if mode.setting != store.settings.input {
        mode.setting = store.settings.input;
        mode.pad = InputMode::follow(mode.setting, mode.pad, false, false);
    }
}

/// A pad plugged in or out: say so on the yellow line.
fn connected(mut events: MessageReader<GamepadConnectionEvent>, time: Res<Time>, real: Res<Time<Real>>, mut mode: ResMut<InputMode>, notice: Option<ResMut<crate::saving::Notice>>, mut toast: ResMut<Toast>) {
    let mut said = None;
    for ev in events.read() {
        match &ev.connection {
            GamepadConnection::Connected { name, vendor_id, .. } => {
                mode.family = Family::of(*vendor_id, name);
                mode.gamepad.get_or_insert(ev.gamepad);
                said = Some(format!("Controller connected: {name}"));
                println!("controller connected: {name} ({:?} buttons)", mode.family);
            }
            GamepadConnection::Disconnected => {
                if mode.gamepad == Some(ev.gamepad) {
                    mode.gamepad = None;
                }
                said = Some("Controller disconnected".to_string());
                println!("controller disconnected");
            }
        }
    }
    if let Some(text) = said {
        toast.say(real.elapsed_secs(), text.clone());
        if let Some(mut n) = notice {
            n.say(time.elapsed_secs(), text);
        }
    }
}

/// A short line in the top right corner, over every screen (the menus, the new-game screen, the
/// game): a controller plugged in or out, and which the game now follows when that changes.
#[derive(Resource, Default)]
pub struct Toast {
    text: String,
    until: f32,
    changed: bool,
    /// The controller in use, as last said (None: nothing said yet, so the start says nothing).
    was_pad: Option<bool>,
}

impl Toast {
    pub fn say(&mut self, now: f32, text: String) {
        (self.text, self.until, self.changed) = (text, now + TOAST_SECS, true);
    }
}

/// How long the corner line stays up (seconds).
const TOAST_SECS: f32 = 4.0;

#[derive(Component)]
struct ToastNode;

fn spawn_toast(mut commands: Commands) {
    use crate::menu_skin as skin;
    commands
        .spawn((
            ToastNode,
            Node { position_type: PositionType::Absolute, top: px(18), right: px(18), padding: UiRect::axes(px(14), px(8)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(6)), display: Display::None, ..default() },
            BackgroundColor(skin::GROUND.with_alpha(0.94)),
            BorderColor::all(skin::ACCENT),
            GlobalZIndex(60),
            Pickable::IGNORE,
        ))
        .with_children(|t| {
            t.spawn((Text::new(""), crate::theme::strong(crate::theme::BODY), TextColor(skin::BONE), TextLayout::no_wrap(), Pickable::IGNORE));
        });
}

fn show_toast(time: Res<Time<Real>>, mode: Res<InputMode>, mut toast: ResMut<Toast>, mut node: Query<(&mut Node, &Children), With<ToastNode>>, mut texts: Query<&mut Text>) {
    let now = time.elapsed_secs();
    // The game follows the other now: say so (not at the start).
    if toast.was_pad != Some(mode.pad) {
        if toast.was_pad.is_some() {
            let text = if mode.pad { format!("Controller in use: {} buttons", family_name(mode.family)) } else { "Keyboard and mouse in use".to_string() };
            toast.say(now, text);
        }
        toast.was_pad = Some(mode.pad);
    }
    let Ok((mut n, children)) = node.single_mut() else { return };
    let up = now < toast.until;
    let display = if up { Display::Flex } else { Display::None };
    if n.display != display {
        n.display = display;
    }
    if toast.changed {
        toast.changed = false;
        if let Some(mut t) = children.first().and_then(|c| texts.get_mut(*c).ok()) {
            t.0 = toast.text.clone();
        }
    }
}

fn family_name(f: Family) -> &'static str {
    match f {
        Family::Xbox => "Xbox",
        Family::PlayStation => "PlayStation",
        Family::Nintendo => "Nintendo",
    }
}

/// Is a screen up that B goes back from (Esc), and that keeps the pad's play actions to itself?
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct Screens<'w> {
    inventory: Option<Res<'w, crate::inventory_ui::InventoryPanel>>,
    map: Option<Res<'w, crate::map_ui::MapUi>>,
    pause: Option<Res<'w, crate::pause_menu::PauseMenu>>,
    settings: Option<Res<'w, crate::settings_ui::SettingsUi>>,
    browser: Option<Res<'w, crate::browser_ui::BrowserUi>>,
    graphics: Option<Res<'w, crate::graphics_ui::GraphicsUi>>,
    item_menu: Option<Res<'w, crate::inventory_ui::ItemMenu>>,
    chat: Option<Res<'w, crate::chat::ChatEntry>>,
    find: Option<Res<'w, crate::inventory_ui::Find>>,
    wheel: Option<Res<'w, crate::wheel::Wheel>>,
    menu: Option<Res<'w, crate::main_menu::MenuLayer>>,
    front: Option<Res<'w, crate::new_game::Front>>,
}

impl Screens<'_> {
    /// A screen that B closes (the inventory's frame, the map, the item menu, the browser, F8).
    fn open(&self) -> bool {
        self.inventory.as_ref().is_some_and(|i| i.open)
            || self.map.as_ref().is_some_and(|m| m.open)
            || self.browser.as_ref().is_some_and(|b| b.open)
            || self.graphics.as_ref().is_some_and(|g| g.open)
            || self.item_menu.as_ref().is_some_and(|m| m.is_up())
            || self.menus()
    }

    /// The main menu, the new-game screen, the pause menu or the settings: they take the pad whole
    /// (Menu and B are Esc there).
    fn menus(&self) -> bool {
        self.pause.as_ref().is_some_and(|p| p.open)
            || self.settings.as_ref().is_some_and(|s| s.open)
            || self.menu.as_ref().is_some_and(|m| m.up)
            || self.front.as_ref().is_some_and(|f| f.up())
    }

    /// A wheel is up: the sticks pick and A and B are its own.
    fn wheel(&self) -> bool {
        self.wheel.as_ref().is_some_and(|w| w.is_up())
    }

    /// A line being typed: the pad does nothing meanwhile.
    fn typing(&self) -> bool {
        self.chat.as_ref().is_some_and(|c| c.typing) || self.find.as_ref().is_some_and(|f| f.typing) || self.browser.as_ref().is_some_and(|b| b.typing())
    }
}

/// Read the pad: which is in use, the actions it did, the keys it presses for the game, and the
/// sticks and triggers ([`Pad`]).
#[allow(clippy::too_many_arguments)]
fn read_pad(
    gamepads: Query<(Entity, &Gamepad)>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time>,
    screens: Screens,
    mut mode: ResMut<InputMode>,
    mut pad: ResMut<Pad>,
    mut injected: ResMut<Injected>,
    mut held: Local<Held>,
) {
    let now = time.elapsed_secs();
    // Last frame's keys come up (a real key held down at the same time is let go with them).
    for k in injected.0.drain(..) {
        keys.release(k);
    }
    // Which pad: the one last touched (any button or a stick), else the first.
    let touched = |g: &Gamepad| g.get_just_pressed().next().is_some() || g.left_stick().length() > WAKE_STICK || g.right_stick().length() > WAKE_STICK;
    if let Some((e, _)) = gamepads.iter().find(|(_, g)| touched(g)) {
        mode.gamepad = Some(e);
    }
    let gamepad = mode.gamepad.and_then(|e| gamepads.get(e).ok()).or_else(|| gamepads.iter().next()).map(|(_, g)| g);
    // Which is in use: real keys and clicks (the pad's own presses come after this).
    // (The on-screen keyboard types as keys no keyboard has: `osk.rs`. They aren't the keyboard.)
    let keys_touched = keys.get_just_pressed().any(|k| !matches!(k, KeyCode::Unidentified(_))) || mouse.get_just_pressed().next().is_some();
    let pad_touched = gamepad.is_some_and(touched);
    let was = mode.pad;
    let now_pad = InputMode::follow(mode.setting, was, pad_touched, keys_touched);
    if mode.pad != now_pad {
        mode.pad = now_pad;
    }
    let mut frame = KeyFrame::default();
    let mut out = Pad::default();
    let listening = mode.setting != InputSetting::Keyboard && !screens.typing();
    let Some(g) = gamepad.filter(|_| listening) else {
        *held = Held::default();
        *KEYS.write().unwrap() = frame;
        if *pad != out {
            *pad = out;
        }
        return;
    };
    let b = current();
    // Taps of last frame are let go of now.
    for a in std::mem::take(&mut held.tapped) {
        frame.released.extend(key_of(a));
    }
    let mut started: Vec<PadAction> = Vec::new();
    let mut holding: Vec<PadAction> = Vec::new();
    let mut ended: Vec<PadAction> = Vec::new();
    for button in Button::ALL {
        let gb = button.gamepad();
        let plain = Bind { button, hold: false };
        let long = Bind { button, hold: true };
        let waits = b.has_hold(button);
        if g.just_pressed(gb) {
            held.since.insert(button, (now, false));
            if !waits {
                started.extend(b.actions_of(plain));
            }
        }
        if g.pressed(gb) {
            let (since, fired) = held.since.get(&button).copied().unwrap_or((now, false));
            if !waits {
                holding.extend(b.actions_of(plain));
            } else if fired {
                holding.extend(b.actions_of(long));
            } else if now - since >= HOLD_SECS {
                held.since.insert(button, (since, true));
                started.extend(b.actions_of(long));
                holding.extend(b.actions_of(long));
            }
        }
        if g.just_released(gb) {
            let fired = held.since.remove(&button).is_some_and(|(_, f)| f);
            if !waits {
                ended.extend(b.actions_of(plain));
            } else if fired {
                ended.extend(b.actions_of(long));
            } else {
                // Let go before the hold: the plain action, started and held this frame, let go the next.
                let taps: Vec<PadAction> = b.actions_of(plain).collect();
                started.extend(taps.iter().copied());
                holding.extend(taps.iter().copied());
                held.tapped.extend(taps);
            }
        }
    }
    out.items_start = started.contains(&PadAction::Only(Only::ItemWheel));
    out.items_held = holding.contains(&PadAction::Only(Only::ItemWheel));
    out.make_start = started.contains(&PadAction::Only(Only::MakeWheel));
    out.make_held = holding.contains(&PadAction::Only(Only::MakeWheel));
    out.pressed = Button::ALL.iter().filter(|b| g.just_pressed(b.gamepad())).fold(0, |m, b| m | 1 << *b as u16);
    // A wheel up: the sticks pick (the one pushed further), A and B are its own, nothing else.
    if screens.wheel() {
        let (l, r) = (dead_zone(g.left_stick()), dead_zone(g.right_stick()));
        out.select = if r.length() > l.length() { r } else { l };
        out.confirm = g.just_pressed(Button::A.gamepad());
        out.cancel = g.just_pressed(Button::B.gamepad()) || started.contains(&PadAction::Only(Only::Pause));
        *KEYS.write().unwrap() = KeyFrame::default();
        if *pad != out {
            *pad = out;
        }
        return;
    }
    // B on a screen, and Menu anywhere, are Esc; in the pause menu and the settings the pad does
    // nothing else but move the focus (below).
    let back = g.just_pressed(Button::B.gamepad()) && screens.open();
    let menus = screens.menus();
    let mut press = |k: KeyCode, keys: &mut ButtonInput<KeyCode>| {
        keys.press(k);
        injected.0.push(k);
    };
    if back || started.contains(&PadAction::Only(Only::Pause)) {
        press(KeyCode::Escape, &mut keys);
    }
    if menus {
        // The pause menu and the settings go by focus (`focus.rs`): the d-pad or the left stick
        // moves it, A presses what has it, the bumpers change tab.
        let step = |b: Button| g.just_pressed(b.gamepad()) as i32;
        out.nav_step = IVec2::new(step(Button::Right) - step(Button::Left), step(Button::Up) - step(Button::Down));
        out.nav_stick = dead_zone(g.left_stick());
        out.confirm = g.just_pressed(Button::A.gamepad());
        out.nav_tab = step(Button::RB) - step(Button::LB);
    }
    if menus || back {
        *KEYS.write().unwrap() = KeyFrame::default();
        if *pad != out {
            *pad = out;
        }
        return;
    }
    for (only, key) in [(Only::TurnLeft, KeyCode::Comma), (Only::TurnRight, KeyCode::Period), (Only::FirstPerson, KeyCode::Semicolon), (Only::Chat, KeyCode::Enter)] {
        if started.contains(&PadAction::Only(only)) {
            press(key, &mut keys);
        }
    }
    frame.just = started.iter().filter_map(|&a| key_of(a)).collect();
    frame.held = holding.iter().filter_map(|&a| key_of(a)).collect();
    frame.released.extend(ended.iter().filter_map(|&a| key_of(a)));
    out.stick = dead_zone(g.left_stick());
    out.look = dead_zone(g.right_stick());
    out.lt = g.get(GamepadButton::LeftTrigger2).unwrap_or(0.0);
    out.rt = g.get(GamepadButton::RightTrigger2).unwrap_or(0.0);
    out.aim = holding.contains(&PadAction::Only(Only::Aim));
    out.attack = started.contains(&PadAction::Only(Only::Attack));
    out.zoom = holding.contains(&PadAction::Only(Only::ZoomIn)) as i32 as f32 - holding.contains(&PadAction::Only(Only::ZoomOut)) as i32 as f32;
    // Run (the left stick pressed in) stays on until the stick comes back to the middle. It is
    // the pad's own: the keyboard's run is held.
    out.run = (pad.run != frame.just.contains(&Action::Run)) && out.stick != Vec2::ZERO;
    frame.just.retain(|a| *a != Action::Run);
    frame.held.retain(|a| *a != Action::Run);
    *KEYS.write().unwrap() = frame;
    if *pad != out {
        *pad = out;
    }
}

/// The keyboard action a pad action is, if any.
fn key_of(a: PadAction) -> Option<Action> {
    match a {
        PadAction::Key(k) => Some(k),
        PadAction::Only(_) => None,
    }
}

/// Aim assist (#12): of `targets` (where each stands), the one a swing or shot from `from` facing
/// `facing` should turn to: within `reach` metres and `cone` radians either side of the facing,
/// the one nearest the facing's line (by angle, then distance). `None`: nobody there.
pub fn soft_lock(from: Vec2, facing: f32, reach: f32, cone: f32, targets: impl Iterator<Item = Vec2>) -> Option<Vec2> {
    let ahead = Vec2::from_angle(facing);
    let mut best: Option<(f32, Vec2)> = None;
    for t in targets {
        let to = t - from;
        let d = to.length();
        if d > reach || d < 1e-3 {
            continue;
        }
        let off = ahead.angle_to(to / d).abs();
        if off > cone {
            continue;
        }
        // An angle's worth of a metre: nearer the line first, then nearer.
        let score = off + d * 0.02;
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, t));
        }
    }
    best.map(|(_, t)| t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_name_the_button_that_does_the_same() {
        let (pad, keys) = (shipped(), crate::keys::shipped());
        let p = |f: Family, k: &str| prompt_for(&pad, &keys, f, k);
        assert_eq!(p(Family::Xbox, "Esc").as_deref(), Some("B"));
        assert_eq!(p(Family::PlayStation, "Enter").as_deref(), Some("Cross"));
        assert_eq!(p(Family::Xbox, "E").as_deref(), Some("A"));
        assert_eq!(p(Family::Xbox, "Tab").as_deref(), Some("RB"));
        assert_eq!(p(Family::Xbox, "Shift+Tab").as_deref(), Some("LB"));
        // An action the pad has a button for, through its key; keys no button does: no prompt.
        assert_eq!(p(Family::Xbox, "Space").as_deref(), Some("B"));
        assert_eq!(p(Family::Xbox, "1-3"), None);
        assert_eq!(p(Family::Xbox, "Alt"), None);
    }

    #[test]
    fn the_shipped_buttons_parse_and_name_their_actions() {
        let b = shipped();
        let a = |id: &str| PadAction::from_id(id).unwrap();
        assert_eq!(b.binds(a("interact")), &[Bind { button: Button::A, hold: false }]);
        assert_eq!(b.binds(a("sleep")), &[Bind { button: Button::Down, hold: true }]);
        assert_eq!(b.binds(a("attack")), &[Bind { button: Button::RT, hold: false }]);
        assert!(b.has_hold(Button::Down) && b.has_hold(Button::X) && !b.has_hold(Button::A));
        assert_eq!(b.label(a("stop"), Family::PlayStation), "Hold Square");
        assert_eq!(b.label(a("interact"), Family::Nintendo), "B", "Nintendo's bottom button is B");
        assert_eq!(b.label(a("smash"), Family::Xbox), "none");
        // Every pad-only action has a button, and no two plain actions on foot share one.
        for o in Only::ALL {
            assert!(!b.binds(PadAction::Only(o)).is_empty(), "{} has no button", o.id());
        }
        let mut seen = std::collections::BTreeMap::new();
        for (act, binds) in &b.map {
            if matches!(act, PadAction::Key(k) if k.at_wheel()) {
                continue;
            }
            for bind in binds {
                assert!(seen.insert(*bind, *act).is_none(), "{} is on two actions", bind.text());
            }
        }
    }

    #[test]
    fn bad_buttons_and_actions_are_named() {
        assert!(PadBindings::parse("(jump: [\"A\"])").unwrap_err().contains("jump"));
        assert!(PadBindings::parse("(interact: [\"Z\"])").unwrap_err().contains("\"Z\""));
        assert_eq!(Bind::parse("hold x"), Ok(Bind { button: Button::X, hold: true }));
        assert_eq!(Bind::parse("Hold X").unwrap().text(), "Hold X");
    }

    #[test]
    fn auto_follows_what_was_touched_last() {
        use InputSetting::*;
        assert!(InputMode::follow(Auto, false, true, false));
        assert!(!InputMode::follow(Auto, true, false, true));
        assert!(InputMode::follow(Auto, true, false, false), "nothing touched: as it was");
        assert!(!InputMode::follow(Keyboard, true, true, false));
        assert!(InputMode::follow(Controller, false, false, true));
    }

    #[test]
    fn families_go_by_maker_then_name() {
        assert_eq!(Family::of(Some(0x054c), ""), Family::PlayStation);
        assert_eq!(Family::of(Some(0x057e), ""), Family::Nintendo);
        assert_eq!(Family::of(None, "DualSense Wireless Controller"), Family::PlayStation);
        assert_eq!(Family::of(None, "Steam Deck"), Family::Xbox);
    }

    #[test]
    fn the_dead_zone_reads_as_still_and_full_over_as_one() {
        assert_eq!(dead_zone(Vec2::new(0.15, 0.0)), Vec2::ZERO);
        assert!((dead_zone(Vec2::new(1.0, 0.0)).length() - 1.0).abs() < 1e-5);
        assert!((dead_zone(Vec2::new(0.6, 0.0)).x - 0.5).abs() < 1e-5, "half way past the dead zone walks at half");
    }

    #[test]
    fn the_soft_lock_takes_the_one_in_front() {
        let ahead = soft_lock(Vec2::ZERO, 0.0, 2.0, 0.5, [Vec2::new(1.5, 0.2), Vec2::new(1.0, 0.9), Vec2::new(-1.0, 0.0)].into_iter());
        assert_eq!(ahead, Some(Vec2::new(1.5, 0.2)));
        assert_eq!(soft_lock(Vec2::ZERO, 0.0, 2.0, 0.5, [Vec2::new(3.0, 0.0), Vec2::new(0.0, 1.0)].into_iter()), None, "out of reach and to the side");
    }
}
