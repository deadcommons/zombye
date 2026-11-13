//! The game's keys, as data: each [`Action`] and the keys that do it, from `assets/data/keys.ron`
//! (the data folder's, else the one built in), with what the player picked on the settings
//! screen's Controls tab over it (`ClientSettings::keys`).
//!
//! Systems ask [`just_pressed`] / [`pressed`] with an action instead of a `KeyCode`, and text that
//! names a key asks [`label`], so a rebound key is followed everywhere at once. The bindings are
//! global (like `help_text`), set once at start ([`init`]) and again when the player rebinds
//! ([`set_overrides`]).
//!
//! **Adding an action:** a variant, its id and name in [`Action::ALL`] and [`Action::id`] /
//! [`Action::name`], and its keys in `keys.ron`.

use bevy::prelude::*;
use std::collections::BTreeMap;
use std::sync::RwLock;

const SHIPPED: &str = include_str!("../../../assets/data/keys.ron");

/// Something the player does with a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    Run,
    Sneak,
    Sleep,
    Sit,
    Interact,
    Smash,
    Flashlight,
    Shove,
    Reload,
    Stop,
    Inventory,
    Health,
    Skills,
    Make,
    Build,
    Map,
    Objectives,
}

impl Action {
    pub const ALL: [Action; 21] = [
        Action::MoveUp,
        Action::MoveDown,
        Action::MoveLeft,
        Action::MoveRight,
        Action::Run,
        Action::Sneak,
        Action::Sleep,
        Action::Sit,
        Action::Interact,
        Action::Smash,
        Action::Flashlight,
        Action::Shove,
        Action::Reload,
        Action::Stop,
        Action::Inventory,
        Action::Health,
        Action::Skills,
        Action::Make,
        Action::Build,
        Action::Map,
        Action::Objectives,
    ];

    /// Its name in `keys.ron` and `settings.ron`.
    pub fn id(self) -> &'static str {
        match self {
            Action::MoveUp => "move_up",
            Action::MoveDown => "move_down",
            Action::MoveLeft => "move_left",
            Action::MoveRight => "move_right",
            Action::Run => "run",
            Action::Sneak => "sneak",
            Action::Sleep => "sleep",
            Action::Sit => "sit",
            Action::Interact => "interact",
            Action::Smash => "smash",
            Action::Flashlight => "flashlight",
            Action::Shove => "shove",
            Action::Reload => "reload",
            Action::Stop => "stop",
            Action::Inventory => "inventory",
            Action::Health => "health",
            Action::Skills => "skills",
            Action::Make => "make",
            Action::Build => "build",
            Action::Map => "map",
            Action::Objectives => "objectives",
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.id() == id)
    }
}

/// `keys.ron`: each action's keys, any of which does it. A key is its name ("E", "Space",
/// "F5", ",", "Shift"), a chord a modifier before it ("Shift+B"). Left out: the game's own.
#[derive(serde::Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct KeysFile {
    /// Walk up the screen (away from the camera).
    pub move_up: Option<Vec<String>>,
    /// Walk down the screen.
    pub move_down: Option<Vec<String>>,
    /// Walk left.
    pub move_left: Option<Vec<String>>,
    /// Walk right.
    pub move_right: Option<Vec<String>>,
    /// Run while held.
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
    /// Shove, or stomp one that's down; the brake at the wheel.
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
}

impl KeysFile {
    /// Each action and its keys, where the file gives them.
    pub fn entries(self) -> BTreeMap<String, Vec<String>> {
        let all = [
            (Action::MoveUp, self.move_up),
            (Action::MoveDown, self.move_down),
            (Action::MoveLeft, self.move_left),
            (Action::MoveRight, self.move_right),
            (Action::Run, self.run),
            (Action::Sneak, self.sneak),
            (Action::Sleep, self.sleep),
            (Action::Sit, self.sit),
            (Action::Interact, self.interact),
            (Action::Smash, self.smash),
            (Action::Flashlight, self.flashlight),
            (Action::Shove, self.shove),
            (Action::Reload, self.reload),
            (Action::Stop, self.stop),
            (Action::Inventory, self.inventory),
            (Action::Health, self.health),
            (Action::Skills, self.skills),
            (Action::Make, self.make),
            (Action::Build, self.build),
            (Action::Map, self.map),
            (Action::Objectives, self.objectives),
        ];
        all.into_iter().filter_map(|(a, k)| k.map(|k| (a.id().to_string(), k))).collect()
    }
}

/// One way to press an action: a key (any of `keys`: Shift is either side) with modifiers held.
#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    pub keys: Vec<KeyCode>,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// As written ("Shift+B"), for showing.
    pub text: String,
}

const SHIFT: [KeyCode; 2] = [KeyCode::ShiftLeft, KeyCode::ShiftRight];
const CTRL: [KeyCode; 4] = [KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::SuperLeft, KeyCode::SuperRight];
const ALT: [KeyCode; 2] = [KeyCode::AltLeft, KeyCode::AltRight];

/// The keys a name stands for (`None`: not a key we know).
fn key_of(name: &str) -> Option<Vec<KeyCode>> {
    use KeyCode::*;
    let n = name.trim();
    let one = |k| Some(vec![k]);
    if n.len() == 1 {
        let c = n.chars().next()?.to_ascii_uppercase();
        let letters = [KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ];
        let digits = [Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9];
        return match c {
            'A'..='Z' => one(letters[(c as u8 - b'A') as usize]),
            '0'..='9' => one(digits[(c as u8 - b'0') as usize]),
            ',' => one(Comma),
            '.' => one(Period),
            ';' => one(Semicolon),
            '/' => one(Slash),
            '\'' => one(Quote),
            '[' => one(BracketLeft),
            ']' => one(BracketRight),
            '-' => one(Minus),
            '=' => one(Equal),
            '`' => one(Backquote),
            '\\' => one(Backslash),
            _ => None,
        };
    }
    let f = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    if let Some(k) = n.strip_prefix('F').and_then(|d| d.parse::<usize>().ok()).filter(|d| (1..=12).contains(d)) {
        return one(f[k - 1]);
    }
    match n.to_ascii_lowercase().as_str() {
        "space" => one(Space),
        "tab" => one(Tab),
        "enter" | "return" => one(Enter),
        "esc" | "escape" => one(Escape),
        "backspace" => one(Backspace),
        "shift" => Some(SHIFT.to_vec()),
        "ctrl" | "control" => Some(CTRL.to_vec()),
        "alt" | "option" => Some(ALT.to_vec()),
        "up" => one(ArrowUp),
        "down" => one(ArrowDown),
        "left" => one(ArrowLeft),
        "right" => one(ArrowRight),
        _ => None,
    }
}

/// The name of a key, as `keys.ron` writes it ("E", "Space", "Shift"), for a key the player just
/// pressed to rebind (`None`: one we don't take).
pub fn name_of(key: KeyCode) -> Option<String> {
    use KeyCode::*;
    let s = match key {
        ShiftLeft | ShiftRight => "Shift",
        ControlLeft | ControlRight | SuperLeft | SuperRight => "Ctrl",
        AltLeft | AltRight => "Alt",
        Space => "Space",
        Tab => "Tab",
        Enter => "Enter",
        Backspace => "Backspace",
        Comma => ",",
        Period => ".",
        Semicolon => ";",
        Slash => "/",
        Quote => "'",
        BracketLeft => "[",
        BracketRight => "]",
        Minus => "-",
        Equal => "=",
        Backquote => "`",
        Backslash => "\\",
        ArrowUp => "Up",
        ArrowDown => "Down",
        ArrowLeft => "Left",
        ArrowRight => "Right",
        _ => {
            let d = format!("{key:?}");
            if let Some(l) = d.strip_prefix("Key") {
                return Some(l.to_string());
            }
            if let Some(n) = d.strip_prefix("Digit") {
                return Some(n.to_string());
            }
            if d.starts_with('F') && d[1..].parse::<u8>().is_ok() {
                return Some(d);
            }
            return None;
        }
    };
    Some(s.to_string())
}

/// "Shift+B" into a chord.
pub fn parse_chord(text: &str) -> Result<Chord, String> {
    let parts: Vec<&str> = text.split('+').map(str::trim).collect();
    // A chord ending in "+" means the plus key: not one we bind.
    let (last, mods) = parts.split_last().ok_or_else(|| format!("\"{text}\": no key"))?;
    let keys = key_of(last).ok_or_else(|| format!("\"{text}\": {last} isn't a key the game knows"))?;
    let mut c = Chord { keys, shift: false, ctrl: false, alt: false, text: text.trim().to_string() };
    for m in mods {
        match m.to_ascii_lowercase().as_str() {
            "shift" => c.shift = true,
            "ctrl" | "control" | "cmd" => c.ctrl = true,
            "alt" | "option" => c.alt = true,
            _ => return Err(format!("\"{text}\": {m} isn't Shift, Ctrl or Alt")),
        }
    }
    Ok(c)
}

/// Every action's chords.
#[derive(Clone, Debug, Default)]
pub struct Bindings {
    map: BTreeMap<Action, Vec<Chord>>,
}

impl Bindings {
    /// `keys.ron`'s text ([`KeysFile`]). Unknown actions and keys are refused, naming them.
    pub fn parse(text: &str) -> Result<Self, String> {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        let raw = options.from_str::<KeysFile>(text).map_err(|e| format!("keys.ron: {e}"))?.entries();
        let mut b = Bindings::default();
        b.apply(&raw)?;
        Ok(b)
    }

    /// Put `over` (action id to keys) over these.
    pub fn apply(&mut self, over: &BTreeMap<String, Vec<String>>) -> Result<(), String> {
        for (id, keys) in over {
            let a = Action::from_id(id).ok_or_else(|| format!("keys: \"{id}\" isn't an action"))?;
            let chords = keys.iter().map(|k| parse_chord(k)).collect::<Result<Vec<_>, _>>().map_err(|e| format!("keys: {id}: {e}"))?;
            self.map.insert(a, chords);
        }
        Ok(())
    }

    pub fn chords(&self, a: Action) -> &[Chord] {
        self.map.get(&a).map_or(&[], |v| v.as_slice())
    }

    /// Is `c` a key with no modifier whose key some action also binds with Shift? Then it wants
    /// Shift up (B sleeps, Shift+B builds); otherwise Shift may be held (E while running).
    fn shift_taken(&self, c: &Chord) -> bool {
        !c.shift && self.map.values().flatten().any(|o| o.shift && o.keys == c.keys)
    }

    fn mods_ok(&self, input: &ButtonInput<KeyCode>, c: &Chord) -> bool {
        let shift = input.any_pressed(SHIFT);
        let ctrl = input.any_pressed(CTRL);
        let alt = input.any_pressed(ALT);
        // A modifier bound alone (Shift to run) is its own key.
        let alone = c.keys.iter().any(|k| SHIFT.contains(k) || CTRL.contains(k) || ALT.contains(k));
        if alone {
            return true;
        }
        if c.shift != shift && (c.shift || self.shift_taken(c)) {
            return false;
        }
        c.ctrl == ctrl && c.alt == alt
    }

    pub fn just_pressed(&self, input: &ButtonInput<KeyCode>, a: Action) -> bool {
        self.chords(a).iter().any(|c| input.any_just_pressed(c.keys.iter().copied()) && self.mods_ok(input, c))
    }

    pub fn pressed(&self, input: &ButtonInput<KeyCode>, a: Action) -> bool {
        self.chords(a).iter().any(|c| input.any_pressed(c.keys.iter().copied()) && self.mods_ok(input, c))
    }

    pub fn just_released(&self, input: &ButtonInput<KeyCode>, a: Action) -> bool {
        self.chords(a).iter().any(|c| input.any_just_released(c.keys.iter().copied()))
    }

    /// "E", "Tab or I", "Shift + B" (nothing bound: "unbound").
    pub fn label(&self, a: Action) -> String {
        let all: Vec<String> = self.chords(a).iter().map(|c| c.text.replace('+', " + ")).collect();
        if all.is_empty() { "unbound".to_string() } else { all.join(" or ") }
    }

    /// The other actions that share a chord with `a` (to warn of on the Controls tab).
    pub fn clashes(&self, a: Action) -> Vec<Action> {
        let mine = self.chords(a);
        Action::ALL
            .into_iter()
            .filter(|&o| o != a && self.chords(o).iter().any(|c| mine.iter().any(|m| m.keys == c.keys && m.shift == c.shift && m.ctrl == c.ctrl && m.alt == c.alt)))
            .collect()
    }
}

/// The game's own bindings: the shipped file. (It is tested to parse.)
pub fn shipped() -> Bindings {
    Bindings::parse(SHIPPED).expect("the shipped keys.ron parses")
}

static CURRENT: RwLock<Option<Bindings>> = RwLock::new(None);
/// The data folder's keys (or the shipped ones), before the player's own picks.
static BASE: RwLock<Option<Bindings>> = RwLock::new(None);

fn with<T>(f: impl FnOnce(&Bindings) -> T) -> T {
    if let Some(b) = CURRENT.read().unwrap().as_ref() {
        return f(b);
    }
    let b = shipped();
    let out = f(&b);
    *CURRENT.write().unwrap() = Some(b);
    out
}

/// The bindings in force now (a copy).
pub fn current() -> Bindings {
    with(Clone::clone)
}

pub fn just_pressed(input: &ButtonInput<KeyCode>, a: Action) -> bool {
    with(|b| b.just_pressed(input, a))
}

pub fn pressed(input: &ButtonInput<KeyCode>, a: Action) -> bool {
    with(|b| b.pressed(input, a))
}

pub fn just_released(input: &ButtonInput<KeyCode>, a: Action) -> bool {
    with(|b| b.just_released(input, a))
}

pub fn label(a: Action) -> String {
    with(|b| b.label(a))
}

/// The first key of an action alone, for a short hint ("E").
pub fn short(a: Action) -> String {
    with(|b| b.chords(a).first().map_or_else(|| "unbound".to_string(), |c| c.text.replace('+', " + ")))
}

/// Read the data folder's `keys.ron` (a missing or broken one: the shipped keys, with a warning
/// for a broken one) and put the player's picks over it.
pub fn init(data: Option<&std::path::Path>, overrides: &BTreeMap<String, Vec<String>>) {
    let base = data
        .map(|d| d.join("keys.ron"))
        .filter(|p| p.is_file())
        .and_then(|p| match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| Bindings::parse(&t)) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("{}: {e}; using the game's own keys", p.display());
                None
            }
        })
        .map(|mut b| {
            // An action the data folder's file leaves out keeps the game's own key.
            for a in Action::ALL {
                if b.chords(a).is_empty() {
                    b.map.insert(a, shipped().chords(a).to_vec());
                }
            }
            b
        })
        .unwrap_or_else(shipped);
    *BASE.write().unwrap() = Some(base);
    set_overrides(overrides);
}

/// The player's picks (action id to keys) over the base bindings. Ones that don't parse are left
/// out, with a warning.
pub fn set_overrides(overrides: &BTreeMap<String, Vec<String>>) {
    let mut b = BASE.read().unwrap().clone().unwrap_or_else(shipped);
    for (id, keys) in overrides {
        let one = BTreeMap::from([(id.clone(), keys.clone())]);
        if let Err(e) = b.apply(&one) {
            eprintln!("settings.ron {e}");
        }
    }
    *CURRENT.write().unwrap() = Some(b);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(held: &[KeyCode], just: &[KeyCode]) -> ButtonInput<KeyCode> {
        let mut i = ButtonInput::default();
        for k in held {
            i.press(*k);
        }
        i.clear();
        for k in just {
            i.press(*k);
        }
        i
    }

    #[test]
    fn the_shipped_keys_bind_every_action_once() {
        let b = shipped();
        for a in Action::ALL {
            assert!(!b.chords(a).is_empty(), "{} has no key", a.id());
            assert!(b.clashes(a).is_empty(), "{} shares a key with {:?}", a.id(), b.clashes(a));
        }
        assert_eq!(b.label(Action::Inventory), "Tab or I");
        assert_eq!(b.label(Action::Build), "Shift + B");
        assert_eq!(b.label(Action::Interact), "E");
    }

    #[test]
    fn shift_is_ignored_unless_another_action_takes_it() {
        let b = shipped();
        let shift = KeyCode::ShiftLeft;
        // E while running still interacts.
        assert!(b.just_pressed(&input(&[shift], &[KeyCode::KeyE]), Action::Interact));
        // B sleeps, Shift+B builds, never both.
        assert!(b.just_pressed(&input(&[], &[KeyCode::KeyB]), Action::Sleep));
        assert!(!b.just_pressed(&input(&[], &[KeyCode::KeyB]), Action::Build));
        assert!(b.just_pressed(&input(&[shift], &[KeyCode::KeyB]), Action::Build));
        assert!(!b.just_pressed(&input(&[shift], &[KeyCode::KeyB]), Action::Sleep));
        // Ctrl+M is mute's, not the map's.
        assert!(!b.just_pressed(&input(&[KeyCode::ControlLeft], &[KeyCode::KeyM]), Action::Map));
        // Shift alone runs.
        assert!(b.pressed(&input(&[shift], &[]), Action::Run));
    }

    #[test]
    fn picks_go_over_the_file_and_bad_ones_are_named() {
        let mut b = shipped();
        b.apply(&BTreeMap::from([("interact".to_string(), vec!["F".to_string()])])).unwrap();
        assert!(b.just_pressed(&input(&[], &[KeyCode::KeyF]), Action::Interact));
        assert!(!b.just_pressed(&input(&[], &[KeyCode::KeyE]), Action::Interact));
        let e = b.apply(&BTreeMap::from([("jump".to_string(), vec!["J".to_string()])])).unwrap_err();
        assert!(e.contains("jump"), "{e}");
        let e = b.apply(&BTreeMap::from([("sit".to_string(), vec!["Hyper+Q".to_string()])])).unwrap_err();
        assert!(e.contains("Hyper"), "{e}");
        assert!(parse_chord("Numpad7").is_err());
    }

    #[test]
    fn a_pressed_key_names_itself_as_the_file_writes_it() {
        for k in [KeyCode::KeyE, KeyCode::Digit4, KeyCode::F7, KeyCode::Space, KeyCode::Comma, KeyCode::ShiftRight] {
            let name = name_of(k).expect("a key we take");
            assert!(parse_chord(&name).unwrap().keys.contains(&k), "{name}");
        }
        assert_eq!(name_of(KeyCode::NumpadAdd), None);
    }
}
