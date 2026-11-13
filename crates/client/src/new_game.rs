//! The new-game screen: who your survivor is and how the world is set, before there is a world.
//!
//! The game builds its world once, at start-up, so like the join screen this is a small Bevy app
//! of its own that `main` runs instead of the game (`--new-game`, [`wanted`]). Three tabs on one
//! sheet: **Who you were** (a Background, with the skills you start with), **Strengths and
//! Flaws** (a Strength costs points, a Flaw gives them; the Background says how many you begin
//! with) and **The world** (a difficulty preset, and the few options that go over it). Beside
//! it stands the survivor as it will wake up, in the clothes picked from the starting wardrobe
//! under it. Everything listed is data (`zombye_sim::origin`, `zombye_sim::difficulty`); the one
//! rule for what may be picked is the sim's (`OriginData::resolve`), asked here before Start and
//! again by the game.
//!
//! The footer's button (and Enter) is **Next** on the first two pages and **Start** only on the last.
//! **Start** writes the choices to `new_game.ron` in the saves folder (so the next new game
//! begins from them) and starts the game again with `--new-game-from` that file; **Back**
//! starts it again with `--main-menu`. The same choices can be given to the game as flags
//! (`--background`, `--strengths`, `--flaws`, `--outfit`, `--difficulty`: [`from_options`]).
//! After dying, R starts the same survivor again in a new run ([`StartAs`]).

use std::path::PathBuf;
use std::sync::Mutex;

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use zombye_net::newgame::{self, NewGame, Rules};
use zombye_sim::difficulty::{DifficultyDef, Infection, Setup};
use zombye_sim::origin::{BackgroundDef, Choice, PickDef, Start};
use zombye_sim::{ItemDef, Skill, SkillTuning, WorldConfig};

use crate::Options;
use crate::browser_ui::Relaunch;
use crate::join_screen::args_without;
use crate::survivor_model::{Dress, SurvivorModel};
use crate::theme::{self, BODY, SMALL, font};

/// The survivor a new game began as, for starting it again in a new run (R after dying).
#[derive(Resource, Clone, Default)]
pub struct StartAs(pub Option<Start>);

/// The file the screen writes, in the saves folder.
pub const FILE: &str = "new_game.ron";

/// The hour a survivor wakes at when nothing says otherwise (`server_link::WAKE_HOUR`).
const WAKE: f32 = crate::server_link::WAKE_HOUR;

/// Is this run the new-game screen (`--new-game`, not for a save, a server or a picture of the game)?
pub fn wanted(o: &Options) -> bool {
    o.new_game && o.connect.is_none() && o.load.is_none() && o.new_game_from.is_none()
}

/// The flags that carry a new game's choices, and those that say where it comes from.
const VALUE_FLAGS: [&str; 8] = ["--new-game-from", "--new-game-tab", "--background", "--strengths", "--flaws", "--outfit", "--difficulty", "--load"];

/// What a new world begins as, from `--new-game-from` and the flags over it: the world's options
/// go onto `config` (fresh content), and the survivor comes back to be made once it has joined.
/// `None`: nothing was asked for, a plain survivor in the game as shipped.
pub fn from_options(options: &Options, config: &mut WorldConfig) -> Result<Option<Start>, String> {
    let file = options.new_game_from.as_ref().map(|p| newgame::read(std::path::Path::new(p))).transpose()?;
    let flags = options.background.is_some() || options.strengths.is_some() || options.flaws.is_some() || options.outfit.is_some() || options.difficulty.is_some();
    if file.is_none() && !flags {
        return Ok(None);
    }
    let rules = Rules::load(&options.data, &config.items)?;
    let mut game = file.unwrap_or_default();
    with_flags(options, &rules, &mut game);
    rules.begin(&game, config)
}

/// Put the flags' choices over `game`'s.
fn with_flags(options: &Options, rules: &Rules, game: &mut NewGame) {
    if options.background.is_some() || options.strengths.is_some() || options.flaws.is_some() || options.outfit.is_some() {
        let mut c = game.survivor.take().unwrap_or_else(|| rules.origins.default_choice());
        if let Some(b) = &options.background {
            c.background = b.clone();
        }
        if let Some(s) = &options.strengths {
            c.strengths = s.clone();
        }
        if let Some(f) = &options.flaws {
            c.flaws = f.clone();
        }
        if let Some(o) = &options.outfit {
            c.outfit = o.clone();
        }
        game.survivor = Some(c);
    }
    if let Some(d) = &options.difficulty {
        game.world = Setup { difficulty: d.clone(), ..Default::default() };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Who,
    Picks,
    World,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Who, Tab::Picks, Tab::World];

    /// The page after this one; none after the last, where the game starts.
    fn next(self) -> Option<Tab> {
        Tab::ALL.iter().position(|t| *t == self).and_then(|at| Tab::ALL.get(at + 1).copied())
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Who => "Who you were",
            Tab::Picks => "Strengths and Flaws",
            Tab::World => "The world",
        }
    }
}

/// A world option the player steps through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opt {
    Zombies,
    Loot,
    Xp,
    Respawn,
    Infection,
    Day,
    Wake,
}

impl Opt {
    const ALL: [Opt; 7] = [Opt::Zombies, Opt::Loot, Opt::Xp, Opt::Respawn, Opt::Infection, Opt::Day, Opt::Wake];

    fn label(self) -> &'static str {
        match self {
            Opt::Zombies => "Zombies",
            Opt::Loot => "Things to find",
            Opt::Xp => "Skills grow",
            Opt::Respawn => "Zombies come back",
            Opt::Infection => "The infection",
            Opt::Day => "A day lasts",
            Opt::Wake => "You wake at",
        }
    }

    /// What it can be set to: the value and how it reads.
    fn steps(self) -> &'static [(f32, &'static str)] {
        match self {
            Opt::Zombies => &[(0.3, "A handful"), (0.6, "Fewer"), (1.0, "Normal"), (1.5, "More"), (2.2, "Many"), (3.5, "Swarming")],
            Opt::Loot => &[(0.25, "Almost nothing"), (0.45, "Scarce"), (0.7, "Thin"), (1.0, "Normal"), (1.5, "Plenty"), (2.5, "Abundant")],
            Opt::Xp => &[(0.5, "Slowly"), (0.8, "A little slower"), (1.0, "Normal"), (1.5, "Fast"), (2.5, "Very fast")],
            Opt::Respawn => &[(0.0, "No"), (1.0, "Yes")],
            Opt::Infection => &[(0.0, "Off"), (1.0, "Survivable"), (2.0, "Deadly")],
            Opt::Day => &[(30.0, "30 minutes"), (60.0, "1 hour"), (90.0, "90 minutes"), (120.0, "2 hours"), (180.0, "3 hours")],
            Opt::Wake => &[(7.0, "07:00"), (9.0, "09:00"), (12.0, "12:00"), (14.0, "14:00"), (17.0, "17:00"), (21.0, "21:00")],
        }
    }

    /// The step nearest `v`.
    fn nearest(self, v: f32) -> usize {
        let steps = self.steps();
        (0..steps.len()).min_by(|&a, &b| (steps[a].0 - v).abs().total_cmp(&(steps[b].0 - v).abs())).unwrap_or(0)
    }
}

fn infection_no(i: Infection) -> f32 {
    Infection::ALL.iter().position(|x| *x == i).unwrap_or(2) as f32
}

/// What an option is under preset `d` alone.
fn preset_value(opt: Opt, d: &DifficultyDef) -> f32 {
    match opt {
        Opt::Zombies => d.zombies,
        Opt::Loot => d.loot,
        Opt::Xp => d.xp,
        Opt::Respawn => d.respawn.unwrap_or(false) as u8 as f32,
        Opt::Infection => infection_no(d.infection),
        Opt::Day => d.day_minutes.unwrap_or(60.0),
        Opt::Wake => WAKE,
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Act {
    Tab(Tab),
    /// On to the page after this one (the first two pages' button; the last one's is Start).
    Next,
    Background(usize),
    Strength(usize),
    Flaw(usize),
    /// Another thing from wardrobe slot `.0`, one on or back.
    Wear(usize, i32),
    Preset(usize),
    Step(Opt, i32),
    Start,
    Back,
}

#[derive(Resource)]
struct Screen {
    options: Options,
    rules: Rules,
    items: Vec<ItemDef>,
    config: WorldConfig,
    choice: Choice,
    setup: Setup,
    tab: Tab,
    error: Option<String>,
    acts: Vec<Act>,
    dirty: bool,
    dress: bool,
}

impl Screen {
    fn new(options: Options) -> Result<Self, String> {
        let (_, config, _, _) = crate::server_link::local_content(&options)?;
        let rules = Rules::load(&options.data, &config.items)?;
        if rules.origins.backgrounds.is_empty() {
            return Err(format!("no Backgrounds in {} (backgrounds.ron): nothing to choose", options.data.display()));
        }
        // Begin from the last new game's choices, if they still stand.
        let last = newgame::read(&options.saves.join(FILE)).ok().filter(|g| rules.begin(g, &mut config.clone()).is_ok());
        let mut game = last.unwrap_or_default();
        with_flags(&options, &rules, &mut game);
        let choice = game.survivor.unwrap_or_else(|| rules.origins.default_choice());
        let mut setup = game.world;
        if rules.difficulty.get(&setup.difficulty).is_none() {
            setup = Setup { difficulty: rules.difficulty.default_id().to_string(), ..Default::default() };
        }
        let tab = match options.new_game_tab.as_deref() {
            Some("picks") => Tab::Picks,
            Some("world") => Tab::World,
            _ => Tab::Who,
        };
        Ok(Self { items: config.items.clone(), options, rules, config, choice, setup, tab, error: None, acts: Vec::new(), dirty: true, dress: true })
    }

    fn preset(&self) -> &DifficultyDef {
        self.rules.difficulty.get(&self.setup.difficulty).unwrap_or(&self.rules.difficulty.presets[0])
    }

    /// An option as it stands: the player's, else the preset's.
    fn value(&self, opt: Opt) -> f32 {
        let (s, d) = (&self.setup, self.preset());
        match opt {
            Opt::Zombies => s.zombies,
            Opt::Loot => s.loot,
            Opt::Xp => s.xp,
            Opt::Respawn => s.respawn.map(|r| r as u8 as f32),
            Opt::Infection => s.infection.map(infection_no),
            Opt::Day => s.day_minutes,
            Opt::Wake => s.start_hour,
        }
        .unwrap_or_else(|| preset_value(opt, d))
    }

    fn set(&mut self, opt: Opt, v: f32) {
        // What the preset already says isn't a change.
        let own = (v != preset_value(opt, self.preset())).then_some(v);
        let s = &mut self.setup;
        match opt {
            Opt::Zombies => s.zombies = own,
            Opt::Loot => s.loot = own,
            Opt::Xp => s.xp = own,
            Opt::Respawn => s.respawn = own.map(|v| v > 0.5),
            Opt::Infection => s.infection = own.map(|v| Infection::ALL[(v as usize).min(2)]),
            Opt::Day => s.day_minutes = own,
            Opt::Wake => s.start_hour = own,
        }
    }

    fn game(&self) -> NewGame {
        NewGame { survivor: Some(self.choice.clone()), world: self.setup.clone() }
    }

    /// Would the game start as chosen? The reason if not.
    fn check(&self) -> Result<Start, String> {
        self.rules.begin(&self.game(), &mut self.config.clone()).map(|s| s.unwrap_or_default())
    }

    /// The Strength or Flaw already picked that `p` can't go with.
    fn clash(&self, p: &PickDef) -> Option<&PickDef> {
        let o = &self.rules.origins;
        self.choice.strengths.iter().filter_map(|id| o.strength(id)).chain(self.choice.flaws.iter().filter_map(|id| o.flaw(id))).find(|x| x.id != p.id && o.excluded(&x.id, &p.id))
    }

    /// The thing worn from wardrobe slot `k`.
    fn worn_from(&self, k: usize) -> Option<&String> {
        let slot = &self.rules.origins.wardrobe[k];
        self.choice.outfit.iter().find(|id| slot.items.contains(id))
    }

    fn item_name(&self, id: &str) -> String {
        self.items.iter().find(|d| d.id == id).map_or_else(|| id.to_string(), |d| d.name.clone())
    }

    fn apply(&mut self, act: Act) -> Option<Act> {
        self.error = None;
        self.dirty = true;
        match act {
            Act::Tab(t) => self.tab = t,
            Act::Next => {
                if let Some(t) = self.tab.next() {
                    self.tab = t;
                }
            }
            Act::Background(k) => {
                if let Some(b) = self.rules.origins.backgrounds.get(k) {
                    self.choice.background = b.id.clone();
                }
            }
            Act::Strength(k) | Act::Flaw(k) => {
                let strength = matches!(act, Act::Strength(_));
                let (from, picked) = if strength { (&self.rules.origins.strengths, &mut self.choice.strengths) } else { (&self.rules.origins.flaws, &mut self.choice.flaws) };
                let Some(p) = from.get(k) else { return None };
                match picked.iter().position(|id| *id == p.id) {
                    Some(at) => {
                        picked.remove(at);
                    }
                    None if picked.len() >= zombye_sim::origin::MAX_PICKS => self.error = Some(format!("At most {} of each.", zombye_sim::origin::MAX_PICKS)),
                    None => picked.push(p.id.clone()),
                }
            }
            Act::Wear(k, dir) => {
                let Some(slot) = self.rules.origins.wardrobe.get(k) else { return None };
                // A slot with nothing worn by default may be left empty; the others always hold something.
                let n = slot.items.len() as i32 + slot.default.is_none() as i32;
                let now = self.worn_from(k).and_then(|id| slot.items.iter().position(|i| i == id)).map_or(slot.items.len() as i32, |i| i as i32);
                let next = (now + dir).rem_euclid(n) as usize;
                let (items, wear) = (slot.items.clone(), slot.items.get(next).cloned());
                self.choice.outfit.retain(|id| !items.contains(id));
                self.choice.outfit.extend(wear);
                self.dress = true;
            }
            Act::Preset(k) => {
                if let Some(d) = self.rules.difficulty.presets.get(k) {
                    // A preset is a fresh start; the hour you wake isn't the preset's to say.
                    self.setup = Setup { difficulty: d.id.clone(), start_hour: self.setup.start_hour, ..Default::default() };
                }
            }
            Act::Step(opt, dir) => {
                let steps = opt.steps();
                let next = (opt.nearest(self.value(opt)) as i32 + dir).clamp(0, steps.len() as i32 - 1) as usize;
                self.set(opt, steps[next].0);
            }
            Act::Start => match self.check() {
                Ok(_) => return Some(Act::Start),
                Err(e) => self.error = Some(e),
            },
            Act::Back => return Some(Act::Back),
        }
        None
    }
}

/// The game's arguments once the choices are made: this run's without the screen, with the file.
pub fn start_args(args: &[String], file: &str) -> Vec<String> {
    args_without(args, &VALUE_FLAGS, &["--new-game", "--main-menu"], &["--new-game-from", file])
}

/// Back to the main menu: this run's arguments without the screen.
pub fn back_args(args: &[String]) -> Vec<String> {
    args_without(args, &VALUE_FLAGS, &["--new-game", "--main-menu"], &["--main-menu"])
}

static NEXT: Mutex<Option<Relaunch>> = Mutex::new(None);

pub fn relaunch_asked() -> bool {
    NEXT.lock().is_ok_and(|n| n.is_some())
}

/// Runs the screen and then the game it ended in (or nothing, if the window was closed).
pub fn run(options: Options) -> ! {
    let screen = match Screen::new(options.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("can't start a new game: {e}");
            std::process::exit(1);
        }
    };
    let offscreen = options.offscreen();
    let mut app = App::new();
    app.insert_resource(ClearColor(flat::INK)).add_plugins(crate::window_plugins(&options)).insert_resource(options).insert_resource(screen).add_plugins((theme::ThemePlugin, crate::release_brand::BrandPlugin));
    if offscreen {
        app.add_plugins((bevy::app::ScheduleRunnerPlugin::run_loop(std::time::Duration::ZERO), crate::offscreen::OffscreenPlugin));
    }
    app.insert_resource(GlobalAmbientLight { color: Color::srgb(0.85, 0.82, 0.95), brightness: 420.0, ..default() })
        .add_systems(Startup, setup)
        .add_systems(First, crate::exit_on_signal)
        .add_systems(Update, (keys, act, draw, dress, pose).chain());
    let failed = app.run().is_error();
    if let Some(next) = NEXT.lock().ok().and_then(|mut n| n.take()) {
        crate::relaunch(next);
    }
    std::process::exit(if failed { 1 } else { 0 });
}

/// The survivor's joints, in the skeleton's order.
#[derive(Resource)]
struct Joints(Vec<Entity>);

/// Where the survivor stands, and where the camera looks from: it stands left of the middle of
/// the picture, in the space the sheet leaves.
const CAMERA_AT: Vec3 = Vec3::new(2.94, 0.6, 23.9);
const CAMERA_LOOKS_AT: Vec3 = Vec3::new(2.94, 0.19, 0.0);

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    screen: Res<Screen>,
    target: Option<Res<crate::offscreen::OffscreenTarget>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
) {
    let mut camera = commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: 0.2, ..default() }),
        Transform::from_translation(CAMERA_AT).looking_at(CAMERA_LOOKS_AT, Vec3::Y),
        Msaa::Sample4,
    ));
    if let Some(target) = target {
        camera.insert((RenderTarget::Image(target.0.clone().into()), IsDefaultUiCamera));
    }
    // A warm key light from the front left and a cool one from behind, so the figure reads.
    commands.spawn((DirectionalLight { illuminance: 9000.0, color: Color::srgb(1.0, 0.93, 0.8), shadow_maps_enabled: false, ..default() }, Transform::from_xyz(-2.5, 4.0, 5.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y)));
    commands.spawn((DirectionalLight { illuminance: 2500.0, color: Color::srgb(0.7, 0.8, 1.0), shadow_maps_enabled: false, ..default() }, Transform::from_xyz(3.0, 3.0, -4.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y)));
    // The survivor (none if the game has no modular body: the screen works without the figure).
    if let Some(model) = SurvivorModel::load(&screen.options, &mut meshes, &mut materials, &mut images, &mut bindposes) {
        let root = commands.spawn((Transform::from_rotation(Quat::from_rotation_y(0.35)), Visibility::Inherited)).id();
        let joints = crate::survivor_model::attach(&mut commands, &model, root, 1, true);
        commands.insert_resource(Joints(joints));
        commands.insert_resource(model);
    }
}

/// The idle clip on the figure.
fn pose(time: Res<Time>, model: Option<Res<SurvivorModel>>, joints: Option<Res<Joints>>, mut transforms: Query<&mut Transform>) {
    let (Some(model), Some(joints)) = (model, joints) else { return };
    let pose = model.standing(time.elapsed_secs());
    for (b, e) in joints.0.iter().take(pose.t.len()).enumerate() {
        if let Ok(mut t) = transforms.get_mut(*e) {
            t.translation = pose.t[b];
            t.rotation = pose.r[b];
        }
    }
    for (e, at) in joints.0.iter().skip(pose.t.len()).zip(model.hang_joints(&pose)) {
        if let Ok(mut t) = transforms.get_mut(*e) {
            *t = at;
        }
    }
}

/// The figure in what was picked from the wardrobe, when that changed.
fn dress(mut commands: Commands, mut screen: ResMut<Screen>, model: Option<Res<SurvivorModel>>, mut meshes: ResMut<Assets<Mesh>>, dressed: Query<&Dress>) {
    let (Some(model), Ok(d)) = (model, dressed.single()) else { return };
    if !screen.dress {
        return;
    }
    screen.dress = false;
    let worn: Vec<u16> = screen.choice.outfit.iter().filter_map(|id| screen.items.iter().position(|d| d.id == *id)).map(|i| i as u16).collect();
    let (body, outfit) = model.dressed(&screen.items, &worn);
    let (body_at, outfit_at) = d.parts();
    commands.entity(body_at).insert(Mesh3d(meshes.add(body)));
    match outfit {
        Some(m) => commands.entity(outfit_at).insert((Mesh3d(meshes.add(m)), Visibility::Inherited)),
        None => commands.entity(outfit_at).insert(Visibility::Hidden),
    };
}

fn keys(keys: Res<ButtonInput<KeyCode>>, mut screen: ResMut<Screen>) {
    if keys.just_pressed(KeyCode::Enter) {
        // Enter is the footer's button: Next until the last page, then Start.
        let act = if screen.tab.next().is_some() { Act::Next } else { Act::Start };
        screen.acts.push(act);
    }
    if keys.just_pressed(KeyCode::Escape) {
        screen.acts.push(Act::Back);
    }
    if keys.just_pressed(KeyCode::Tab) {
        let at = Tab::ALL.iter().position(|t| *t == screen.tab).unwrap_or(0);
        let next = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) { at + Tab::ALL.len() - 1 } else { at + 1 };
        screen.acts.push(Act::Tab(Tab::ALL[next % Tab::ALL.len()]));
    }
    for (k, key) in [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3].into_iter().enumerate() {
        if keys.just_pressed(key) {
            screen.acts.push(Act::Tab(Tab::ALL[k]));
        }
    }
}

fn act(mut screen: ResMut<Screen>, mut exit: MessageWriter<AppExit>) {
    if screen.acts.is_empty() {
        return;
    }
    let acts = std::mem::take(&mut screen.acts);
    let args: Vec<String> = std::env::args().skip(1).collect();
    for a in acts {
        let next = match screen.apply(a) {
            Some(Act::Start) => {
                let path: PathBuf = screen.options.saves.join(FILE);
                let written = std::fs::create_dir_all(&screen.options.saves).map_err(|e| format!("{}: {e}", screen.options.saves.display())).and_then(|_| newgame::write(&path, &screen.game()));
                if let Err(e) = written {
                    screen.error = Some(format!("Can't keep the choices: {e}"));
                    continue;
                }
                start_args(&args, &path.to_string_lossy())
            }
            Some(Act::Back) => back_args(&args),
            _ => continue,
        };
        if let Ok(mut n) = NEXT.lock() {
            *n = Some(Relaunch { args: next, password: String::new() });
        }
        exit.write(AppExit::Success);
    }
}

/// The flat chrome of docs/ui-design.md, as this screen needs it. The inventory rework brings
/// these to `theme.rs` for every panel; when it has, this module goes and the screen takes them
/// from there.
mod flat {
    use bevy::prelude::*;

    /// The ground of a menu, and text on a green button.
    pub const INK: Color = Color::srgb(0.078, 0.078, 0.078);
    pub const BONE: Color = Color::srgb(0.949, 0.902, 0.788);
    pub const DIM: Color = Color::srgb(0.725, 0.690, 0.604);
    pub const FAINT: Color = Color::srgb(0.490, 0.467, 0.416);
    // The guide's see-through tints of bone, as they come out over the opaque ink of a menu
    // (worked out in sRGB, as the guide's sheets are drawn).
    /// Bone at 11%: hairlines.
    pub const LINE: Color = Color::srgb(0.174, 0.169, 0.156);
    /// Bone at 4.5%: a tile, slot, chip or secondary button.
    pub const CELL: Color = Color::srgb(0.117, 0.115, 0.110);
    /// Bone at 9%, and the border at 50%: the thing under the pointer.
    pub const CELL_HOVER: Color = Color::srgb(0.156, 0.152, 0.142);
    pub const HOVER_LINE: Color = Color::srgb(0.514, 0.490, 0.433);
    /// Green at 10%: the fill of what is picked.
    pub const PICKED: Color = Color::srgb(0.126, 0.139, 0.112);
    /// The one accent: the open page, what is picked, the main button.
    pub const GREEN: Color = Color::srgb(0.553, 0.690, 0.420);
    pub const AMBER: Color = Color::srgb(0.878, 0.690, 0.290);
    pub const RED: Color = Color::srgb(0.843, 0.149, 0.169);

    pub const SCREEN_TITLE: f32 = 30.0;
    pub const COLUMN_TITLE: f32 = 22.0;
    pub const BUTTON: f32 = 17.0;
    pub const CHIP: f32 = 15.0;
    pub const LABEL: f32 = 14.0;
    pub const RADIUS: f32 = 6.0;
    pub const TOP_BAR: f32 = 76.0;
    pub const FOOTER: f32 = 46.0;
    /// Padding inside a column (the guide's 16, for a window under 1600 wide; the screen is laid
    /// out for 1280 x 720 and up).
    pub const PAD: f32 = 16.0;
}
use flat::{AMBER, BONE, CELL, CELL_HOVER, DIM, FAINT, GREEN, HOVER_LINE, INK, LINE, PAD, RADIUS, RED};

#[derive(Component)]
struct Root;

/// The line under the lists that says more about what the pointer is on.
#[derive(Component)]
struct Detail;

const ROW_H: f32 = 30.0;

fn draw(mut commands: Commands, mut screen: ResMut<Screen>, root: Option<Single<Entity, With<Root>>>) {
    if !screen.dirty {
        return;
    }
    screen.dirty = false;
    let root = match root {
        Some(r) => *r,
        None => commands.spawn((Root, Node { width: Val::Vw(100.0), height: Val::Vh(100.0), min_width: px(0), max_width: Val::Vw(100.0), max_height: Val::Vh(100.0), flex_direction: FlexDirection::Column, overflow: Overflow::clip(), ..default() })).id(),
    };
    let screen = &*screen;
    commands.entity(root).despawn_children();
    commands.entity(root).with_children(|c| {
        top_bar(c, screen);
        c.spawn(Node { flex_grow: 1.0, min_height: px(0), flex_direction: FlexDirection::Row, ..default() }).with_children(|body| {
            // You: the figure stands in this column, the wardrobe under it.
            body.spawn((Node { width: percent(31), flex_shrink: 0.0, flex_direction: FlexDirection::Column, justify_content: JustifyContent::SpaceBetween, padding: UiRect::all(px(PAD)), border: UiRect::right(px(1)), ..default() }, BorderColor::all(LINE))).with_children(|left| {
                column_title(left, "You");
                wardrobe(left, screen);
            });
            // The page: a column of choices, a column of detail, and a line about what is pointed at.
            body.spawn(Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), flex_direction: FlexDirection::Column, overflow: Overflow::clip(), ..default() }).with_children(|right| {
                right.spawn(Node { flex_grow: 1.0, min_height: px(0), flex_direction: FlexDirection::Row, overflow: Overflow::clip(), ..default() }).with_children(|cols| match screen.tab {
                    Tab::Who => who(cols, screen),
                    Tab::Picks => picks(cols, screen),
                    Tab::World => world(cols, screen),
                });
                detail_line(right, match screen.tab {
                    Tab::Who => "A Background is what you did before: the skills you start with, a few things in your pockets, and points to spend on Strengths.",
                    Tab::Picks => "Point at a Strength or a Flaw to read about it. Click to take it, click again to drop it.",
                    Tab::World => "The town is the same town; what is in its cupboards is rolled anew every game.",
                });
            });
        });
        footer(c, screen);
    });
}

fn text(c: &mut ChildSpawnerCommands, s: impl Into<String>, f: TextFont, colour: Color) {
    c.spawn((Text::new(s.into()), f, TextColor(colour), Pickable::IGNORE));
}

fn line(c: &mut ChildSpawnerCommands, s: impl Into<String>, f: TextFont, colour: Color) {
    c.spawn((Text::new(s.into()), f, TextColor(colour), TextLayout::no_wrap(), Pickable::IGNORE));
}

/// A column's title: GEAR, YOU, NEARBY.
fn column_title(c: &mut ChildSpawnerCommands, s: &str) {
    c.spawn((Text::new(s.to_uppercase()), theme::head(flat::COLUMN_TITLE), TextColor(BONE), TextLayout::no_wrap(), Node { margin: UiRect::bottom(px(6)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
}

/// A label over a group of things: HEAD, MAIN HAND.
fn header(c: &mut ChildSpawnerCommands, s: &str) {
    c.spawn((Text::new(s.to_uppercase()), theme::head(flat::LABEL), TextColor(DIM), TextLayout::no_wrap(), Node { margin: UiRect::bottom(px(2)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
}

/// A key to press: Barlow Medium in a rounded hairline box.
fn keycap(c: &mut ChildSpawnerCommands, key: &str, colour: Color) {
    c.spawn((Node { padding: UiRect::axes(px(6), px(0)), min_width: px(22), height: px(20), justify_content: JustifyContent::Center, align_items: AlignItems::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(4)), flex_shrink: 0.0, ..default() }, BorderColor::all(colour.with_alpha(0.45)), Pickable::IGNORE))
        .with_children(|k| line(k, key, theme::strong(SMALL), colour));
}

/// A key and the word for what it does, in the footer.
fn hint(c: &mut ChildSpawnerCommands, key: &str, does: &str) {
    c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|row| {
        keycap(row, key, BONE);
        line(row, does, font(SMALL), DIM);
    });
}

/// What a flat control looks like at rest, and under the pointer.
#[derive(Clone, Copy)]
struct Look {
    fill: Color,
    edge: Color,
    /// Lit under the pointer (not what is already picked, or can't be pressed).
    lights: bool,
}

impl Look {
    const CELL: Look = Look { fill: CELL, edge: LINE, lights: true };
    const PICKED: Look = Look { fill: flat::PICKED, edge: GREEN, lights: false };
    const PRIMARY: Look = Look { fill: GREEN, edge: GREEN, lights: false };
}

/// Lights `e` under the pointer and presses it for `act`.
fn live(e: &mut EntityCommands, look: Look, act: Option<Act>) {
    if look.lights && act.is_some() {
        e.observe(move |ev: On<PointerOver>, mut q: Query<(&mut BackgroundColor, &mut BorderColor)>| {
            if let Ok((mut bg, mut edge)) = q.get_mut(ev.entity) {
                bg.0 = CELL_HOVER;
                *edge = BorderColor::all(HOVER_LINE);
            }
        })
        .observe(move |ev: On<PointerOut>, mut q: Query<(&mut BackgroundColor, &mut BorderColor)>| {
            if let Ok((mut bg, mut edge)) = q.get_mut(ev.entity) {
                bg.0 = look.fill;
                *edge = BorderColor::all(look.edge);
            }
        });
    }
    if let Some(act) = act {
        e.observe(move |_: On<PointerPress>, mut screen: ResMut<Screen>| screen.acts.push(act.clone()));
    }
}

/// A button, 38 high: the screen's one primary (green, ink text) or a secondary (a cell), its
/// key in it as a keycap.
fn button(parent: &mut ChildSpawnerCommands, label: &str, key: Option<&str>, primary: bool, act: Act) {
    let look = if primary { Look::PRIMARY } else { Look::CELL };
    let ink = if primary { INK } else { BONE };
    let mut e = parent.spawn((Node { height: px(38), padding: UiRect::horizontal(px(14)), column_gap: px(10), flex_shrink: 0.0, justify_content: JustifyContent::Center, align_items: AlignItems::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(RADIUS)), ..default() }, BackgroundColor(look.fill), BorderColor::all(look.edge)));
    e.with_children(|b| {
        line(b, label.to_uppercase(), theme::head(flat::BUTTON), ink);
        if let Some(key) = key {
            keycap(b, key, ink);
        }
    });
    live(&mut e, look, Some(act));
}

/// A chip, 32 high: one of a few, the open one green with ink text.
fn chip(parent: &mut ChildSpawnerCommands, label: &str, key: Option<&str>, on: bool, act: Act) {
    let look = if on { Look::PRIMARY } else { Look::CELL };
    let ink = if on { INK } else { BONE };
    let mut e = parent.spawn((Node { height: px(32), padding: UiRect::horizontal(px(12)), column_gap: px(8), flex_shrink: 0.0, justify_content: JustifyContent::Center, align_items: AlignItems::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(RADIUS)), ..default() }, BackgroundColor(look.fill), BorderColor::all(look.edge)));
    e.with_children(|b| {
        if let Some(key) = key {
            keycap(b, key, ink);
        }
        line(b, label.to_uppercase(), theme::head(flat::CHIP), ink);
    });
    live(&mut e, look, Some(act));
}

/// A small square button: a stepper's arrow.
fn arrow(parent: &mut ChildSpawnerCommands, label: &str, act: Act) {
    let mut e = parent.spawn((Node { width: px(26), height: px(24), flex_shrink: 0.0, justify_content: JustifyContent::Center, align_items: AlignItems::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(RADIUS)), ..default() }, BackgroundColor(CELL), BorderColor::all(LINE)));
    e.with_children(|b| line(b, label, theme::strong(BODY), BONE));
    live(&mut e, Look::CELL, Some(act));
}

/// A row of a list: a cell, lit under the pointer, pressed for `act`, outlined green when
/// picked; pointing at it puts `about` on the detail line.
fn row<'a>(list: &'a mut ChildSpawnerCommands, picked: bool, act: Option<Act>, about: String) -> EntityCommands<'a> {
    let look = if picked { Look::PICKED } else { Look { edge: Color::NONE, ..Look::CELL } };
    let mut e = list.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(8), height: px(ROW_H), align_items: AlignItems::Center, padding: UiRect::horizontal(px(10)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(RADIUS)), flex_shrink: 0.0, overflow: Overflow::clip(), ..default() }, BackgroundColor(look.fill), BorderColor::all(look.edge)));
    e.observe(move |_: On<PointerOver>, mut detail: Query<&mut Text, With<Detail>>| {
        for mut t in &mut detail {
            if t.0 != about {
                t.0 = about.clone();
            }
        }
    });
    live(&mut e, look, act);
    e
}

/// The shape that says picked or not beside the green: a box, filled when picked (round for one
/// of several, square for any of several).
fn mark(r: &mut ChildSpawnerCommands, on: bool, round: bool) {
    r.spawn((Node { width: px(14), height: px(14), flex_shrink: 0.0, border: UiRect::all(px(1.5)), border_radius: BorderRadius::all(px(if round { 7.0 } else { 3.0 })), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }, BorderColor::all(if on { GREEN } else { FAINT }), Pickable::IGNORE)).with_children(|m| {
        if on {
            m.spawn((Node { width: px(7), height: px(7), border_radius: BorderRadius::all(px(if round { 3.5 } else { 1.5 })), ..default() }, BackgroundColor(GREEN), Pickable::IGNORE));
        }
    });
}

/// A plus or a minus, drawn: a Strength's green plus, a Flaw's amber minus.
fn sign(r: &mut ChildSpawnerCommands, plus: bool, colour: Color) {
    r.spawn((Node { width: px(10), height: px(10), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|s| {
        s.spawn((Node { position_type: PositionType::Absolute, left: px(0), top: px(4), width: px(10), height: px(2), ..default() }, BackgroundColor(colour), Pickable::IGNORE));
        if plus {
            s.spawn((Node { position_type: PositionType::Absolute, left: px(4), top: px(0), width: px(2), height: px(10), ..default() }, BackgroundColor(colour), Pickable::IGNORE));
        }
    });
}

/// A column of the page, a hairline between it and the one before.
fn column<'a>(cols: &'a mut ChildSpawnerCommands, grow: f32, divided: bool) -> EntityCommands<'a> {
    cols.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(4), flex_basis: px(0), flex_grow: grow, min_width: px(0), padding: UiRect::all(px(PAD)), border: UiRect::left(px(if divided { 1.0 } else { 0.0 })), overflow: Overflow::clip(), ..default() }, BorderColor::all(LINE)))
}

fn detail_line(c: &mut ChildSpawnerCommands, first: &str) {
    c.spawn((Node { min_height: px(46), margin: UiRect::new(px(PAD), px(PAD), px(0), px(PAD)), padding: UiRect::axes(px(12), px(6)), align_items: AlignItems::Center, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(RADIUS)), flex_shrink: 0.0, ..default() }, BackgroundColor(CELL), BorderColor::all(LINE), Pickable::IGNORE)).with_children(|d| {
        d.spawn((Text::new(first.to_string()), font(SMALL), TextColor(DIM), Detail, Pickable::IGNORE));
    });
}

fn points_word(n: i32) -> String {
    format!("{n} point{}", if n.abs() == 1 { "" } else { "s" })
}

/// The frame's top bar: the screen's title, its pages as chips, and the way back.
fn top_bar(c: &mut ChildSpawnerCommands, screen: &Screen) {
    c.spawn((Node { height: px(flat::TOP_BAR), flex_shrink: 0.0, flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect::horizontal(px(24)), border: UiRect::bottom(px(1)), ..default() }, BorderColor::all(LINE))).with_children(|bar| {
        bar.spawn((Text::new("NEW GAME"), theme::head(flat::SCREEN_TITLE), TextColor(BONE), TextLayout::no_wrap(), Node { margin: UiRect::right(px(20)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
        for (k, tab) in Tab::ALL.into_iter().enumerate() {
            chip(bar, tab.label(), Some(&format!("{}", k + 1)), tab == screen.tab, Act::Tab(tab));
        }
        bar.spawn(Node { flex_grow: 1.0, ..default() });
        button(bar, "Back", Some("Esc"), false, Act::Back);
    });
}

/// A Background's effects, a line each.
fn background_lines(b: &BackgroundDef, screen: &Screen) -> Vec<String> {
    let mut out = Vec::new();
    if !b.skills.is_empty() {
        out.push(b.skills.iter().map(|(s, n)| format!("{} +{n}", s.name())).collect::<Vec<_>>().join(",  "));
    }
    if !b.xp.is_empty() {
        out.push(format!("Learns faster: {}", b.xp.iter().map(|(s, m)| format!("{} x{m}", s.name())).collect::<Vec<_>>().join(",  ")));
    }
    if !b.items.is_empty() {
        let things: Vec<String> = b.items.iter().map(|k| if k.count > 1 { format!("{} x {}", k.count, screen.item_name(&k.item).to_lowercase()) } else { screen.item_name(&k.item).to_lowercase() }).collect();
        out.push(format!("Starts with: {}", things.join(", ")));
    }
    out
}

fn who(cols: &mut ChildSpawnerCommands, screen: &Screen) {
    let o = &screen.rules.origins;
    // The Backgrounds.
    column(cols, 0.85, false).with_children(|list| {
        column_title(list, "Background");
        for (k, b) in o.backgrounds.iter().enumerate() {
            let picked = b.id == screen.choice.background;
            row(list, picked, Some(Act::Background(k)), b.about.clone()).with_children(|r| {
                mark(r, picked, true);
                r.spawn((Node { flex_grow: 1.0, ..default() }, Pickable::IGNORE)).with_children(|n| line(n, b.name.clone(), font(BODY), BONE));
                line(r, points_word(b.points), font(SMALL), DIM);
            });
        }
    });
    // The one picked, and the skills the survivor starts with.
    column(cols, 1.0, true).with_children(|d| {
        let Some(b) = o.background(&screen.choice.background) else { return };
        column_title(d, &b.name);
        text(d, b.about.clone(), font(BODY), BONE);
        for l in background_lines(b, screen) {
            text(d, l, font(SMALL), DIM);
        }
        d.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(6), align_items: AlignItems::Baseline, flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|p| {
            line(p, format!("{}", b.points), theme::strong(BODY), BONE);
            line(p, format!("point{} to spend on Strengths", if b.points == 1 { "" } else { "s" }), font(SMALL), DIM);
        });
        d.spawn(Node { height: px(10), flex_shrink: 0.0, ..default() });
        header(d, "You start with");
        // As chosen; while the points don't add up, without the Strengths.
        let start = screen.check().or_else(|_| o.resolve(&Choice { strengths: Vec::new(), ..screen.choice.clone() }, &screen.items)).unwrap_or_default();
        let t: &SkillTuning = &screen.config.skills;
        let skills = start.origin.start_skills(t);
        let plain = zombye_sim::Skills::start(t);
        for s in Skill::ALL {
            let (level, base) = (skills.level(s, t), plain.level(s, t));
            d.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), height: px(22), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|r| {
                r.spawn((Node { width: px(96), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|n| line(n, s.name(), font(SMALL), if level > 0 { BONE } else { DIM }));
                r.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(3), ..default() }, Pickable::IGNORE)).with_children(|pips| {
                    for k in 0..t.top() {
                        // What every survivor has in bone, what this one adds in green.
                        let colour = if k < base.min(level) { BONE } else if k < level { GREEN } else { LINE };
                        pips.spawn((Node { width: px(13), height: px(8), border_radius: BorderRadius::all(px(3)), ..default() }, BackgroundColor(colour), Pickable::IGNORE));
                    }
                });
                let mult = start.origin.perks.xp[s as usize];
                line(r, format!("{level}"), theme::strong(SMALL), if level > 0 { BONE } else { DIM });
                if mult != 1.0 {
                    line(r, format!("learns x{}", (mult * 100.0).round() / 100.0), font(SMALL), if mult < 1.0 { AMBER } else { DIM });
                }
            });
        }
    });
}

fn effects_text(p: &PickDef) -> String {
    p.effects.iter().map(|e| e.text()).collect::<Vec<_>>().join(",  ")
}

fn picks(cols: &mut ChildSpawnerCommands, screen: &Screen) {
    let o = &screen.rules.origins;
    let left = o.points_left(&screen.choice);
    for strengths in [true, false] {
        let (from, picked) = if strengths { (&o.strengths, &screen.choice.strengths) } else { (&o.flaws, &screen.choice.flaws) };
        column(cols, 1.0, !strengths).with_children(|list| {
            list.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Baseline, flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|top| {
                column_title(top, if strengths { "Strengths" } else { "Flaws" });
                top.spawn(Node { flex_grow: 1.0, ..default() });
                if strengths {
                    line(top, format!("{left}"), theme::strong(flat::COLUMN_TITLE), if left < 0 { RED } else { BONE });
                    line(top, if left < 0 { "points over" } else if left == 1 { "point left" } else { "points left" }, font(SMALL), if left < 0 { RED } else { DIM });
                } else {
                    line(top, "a Flaw gives points back", font(SMALL), DIM);
                }
            });
            for (k, p) in from.iter().enumerate() {
                let on = picked.contains(&p.id);
                let clash = (!on).then(|| screen.clash(p)).flatten();
                let dear = strengths && !on && p.points > left;
                let act = clash.is_none().then(|| if strengths { Act::Strength(k) } else { Act::Flaw(k) });
                let mut about = format!("{}  {}  ({})", p.name, p.about, effects_text(p));
                if let Some(c) = clash {
                    about = format!("{about}  Can't go with {}.", c.name);
                }
                let off = clash.is_some();
                let kind = if strengths { GREEN } else { AMBER };
                row(list, on, act, about).with_children(|r| {
                    mark(r, on, false);
                    sign(r, strengths, if off { FAINT } else { kind });
                    line(r, p.name.clone(), font(BODY), if off { FAINT } else { BONE });
                    r.spawn((Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), overflow: Overflow::clip(), ..default() }, Pickable::IGNORE)).with_children(|n| {
                        line(n, clash.map_or_else(|| effects_text(p), |c| format!("not with {}", c.name)), font(SMALL), if off { FAINT } else { DIM });
                    });
                    line(r, format!("{}{}", if strengths { "" } else { "+" }, p.points), theme::strong(BODY), if dear { RED } else if off { FAINT } else { BONE });
                });
            }
        });
    }
}

fn world(cols: &mut ChildSpawnerCommands, screen: &Screen) {
    let d = screen.preset();
    column(cols, 1.0, false).with_children(|list| {
        column_title(list, "Difficulty");
        for (k, p) in screen.rules.difficulty.presets.iter().enumerate() {
            let on = p.id == d.id;
            let look = if on { Look::PICKED } else { Look::CELL };
            let mut e = list.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(10), align_items: AlignItems::Center, padding: UiRect::axes(px(12), px(8)), margin: UiRect::bottom(px(4)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(RADIUS)), flex_shrink: 0.0, ..default() }, BackgroundColor(look.fill), BorderColor::all(look.edge)));
            e.with_children(|r| {
                mark(r, on, true);
                r.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(2), flex_grow: 1.0, flex_basis: px(0), min_width: px(0), ..default() }, Pickable::IGNORE)).with_children(|t| {
                    line(t, p.name.to_uppercase(), theme::head(flat::BUTTON), BONE);
                    text(t, p.about.clone(), font(SMALL), DIM);
                });
            });
            live(&mut e, look, Some(Act::Preset(k)));
        }
        // What the preset does that has no option of its own.
        let mut also = Vec::new();
        for (what, v) in [("sharper senses", d.zombie_senses), ("tougher", d.zombie_toughness), ("surer strikes", d.zombie_accuracy)] {
            if v > 1.0 {
                also.push(format!("{what} (+{:.0}%)", (v - 1.0) * 100.0));
            } else if v < 1.0 {
                also.push(format!("{} ({:.0}%)", what.replace("sharper", "duller").replace("tougher", "weaker").replace("surer", "wilder"), (v - 1.0) * 100.0));
            }
        }
        if let Some(f) = d.fast_zombies {
            also.push(format!("{:.0}% of them fast", f * 100.0));
        }
        if d.needs != 1.0 {
            also.push(format!("hunger, thirst and tiredness x{}", d.needs));
        }
        if d.rare_loot != 1.0 {
            also.push(format!("rare finds x{}", d.rare_loot));
        }
        if !also.is_empty() {
            list.spawn(Node { height: px(4), flex_shrink: 0.0, ..default() });
            header(list, &format!("{} also means", d.name));
            text(list, format!("Zombies and needs: {}.", also.join(", ")), font(SMALL), DIM);
        }
    });
    column(cols, 1.0, true).with_children(|list| {
        column_title(list, "Your own changes");
        let mut changes = 0;
        for opt in Opt::ALL {
            let (steps, v) = (opt.steps(), screen.value(opt));
            let at = opt.nearest(v);
            // The hour you wake isn't the preset's, so it is never a change to it.
            let changed = v != preset_value(opt, d);
            changes += changed as usize;
            list.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(8), height: px(ROW_H + 4.0), align_items: AlignItems::Center, padding: UiRect::horizontal(px(10)), border_radius: BorderRadius::all(px(RADIUS)), flex_shrink: 0.0, ..default() }, BackgroundColor(CELL))).with_children(|r| {
                r.spawn((Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), overflow: Overflow::clip(), ..default() }, Pickable::IGNORE)).with_children(|n| line(n, opt.label(), font(BODY), BONE));
                arrow(r, "<", Act::Step(opt, -1));
                r.spawn((Node { width: px(132), justify_content: JustifyContent::Center, flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|n| line(n, steps[at].1, theme::strong(BODY), BONE));
                arrow(r, ">", Act::Step(opt, 1));
            });
        }
        let own = Setup { start_hour: None, ..screen.setup.clone() }.custom();
        text(list, if own { format!("Changed from {}: {changes}.", d.name) } else { format!("As {} has them.", d.name) }, font(SMALL), DIM);
    });
}

fn wardrobe(left: &mut ChildSpawnerCommands, screen: &Screen) {
    let slots = &screen.rules.origins.wardrobe;
    if slots.is_empty() {
        return;
    }
    left.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(4), width: percent(100), ..default() }).with_children(|w| {
        header(w, "What you wake up in");
        for (k, slot) in slots.iter().enumerate() {
            let worn = screen.worn_from(k);
            let one = slot.items.len() + slot.default.is_none() as usize <= 1;
            w.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(8), height: px(30), align_items: AlignItems::Center, padding: UiRect::horizontal(px(10)), border_radius: BorderRadius::all(px(RADIUS)), flex_shrink: 0.0, ..default() }, BackgroundColor(CELL), Pickable::IGNORE)).with_children(|r| {
                r.spawn((Node { width: px(58), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|n| line(n, slot.name.to_uppercase(), theme::head(flat::LABEL), DIM));
                if !one {
                    arrow(r, "<", Act::Wear(k, -1));
                }
                r.spawn((Node { flex_grow: 1.0, flex_basis: px(0), justify_content: JustifyContent::Center, min_width: px(0), overflow: Overflow::clip(), ..default() }, Pickable::IGNORE))
                    .with_children(|n| line(n, worn.map_or_else(|| "Nothing".to_string(), |id| screen.item_name(id)), font(BODY), if worn.is_some() { BONE } else { FAINT }));
                if !one {
                    arrow(r, ">", Act::Wear(k, 1));
                }
            });
        }
    });
}

/// The frame's footer: the keys that work here, who and what world it will be, and Next (Start on the last page).
fn footer(p: &mut ChildSpawnerCommands, screen: &Screen) {
    let o = &screen.rules.origins;
    let name = |ids: &[String], strengths: bool| ids.iter().filter_map(|id| if strengths { o.strength(id) } else { o.flaw(id) }).map(|p| p.name.clone()).collect::<Vec<_>>();
    let mut who: Vec<String> = o.background(&screen.choice.background).map(|b| b.name.clone()).into_iter().collect();
    who.extend(name(&screen.choice.strengths, true));
    who.extend(name(&screen.choice.flaws, false));
    let left = o.points_left(&screen.choice);
    p.spawn((Node { height: px(flat::FOOTER), flex_shrink: 0.0, flex_direction: FlexDirection::Row, column_gap: px(18), align_items: AlignItems::Center, padding: UiRect::new(px(24), px(4), px(0), px(0)), border: UiRect::top(px(1)), ..default() }, BorderColor::all(LINE))).with_children(|row| {
        hint(row, "1-3", "page");
        hint(row, "Tab", "next page");
        hint(row, "Esc", "back");
        row.spawn((Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), justify_content: JustifyContent::FlexEnd, overflow: Overflow::clip(), ..default() }, Pickable::IGNORE)).with_children(|s| {
            let world = format!("{}{}", screen.preset().name, if screen.setup.custom() { ", your own way" } else { "" });
            match &screen.error {
                Some(e) => line(s, format!("Can't start yet: {e}"), font(SMALL), RED),
                None if left < 0 => line(s, format!("{} over: drop a Strength or take a Flaw.", points_word(-left)), font(SMALL), RED),
                None => line(s, format!("{}  ·  {world}", who.join("  ·  ")), font(SMALL), DIM),
            }
        });
        // The pages are steps: Next walks through them, and only the last one starts the game.
        match screen.tab.next() {
            Some(_) => button(row, "Next", Some("Enter"), true, Act::Next),
            None => button(row, "Start", Some("Enter"), true, Act::Start),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    fn screen(line: &str) -> Screen {
        let saves = std::env::temp_dir().join(format!("zombye-new-game-test-{}-{}", std::process::id(), line.len()));
        Screen::new(Options::parse(&[args(line), vec!["--saves".into(), saves.to_string_lossy().into_owned()]].concat())).expect("the shipped data")
    }

    #[test]
    fn next_walks_the_pages_and_only_the_last_starts() {
        let mut s = screen("--new-game --seed 5");
        s.tab = Tab::Who;
        assert_eq!(s.apply(Act::Next), None);
        assert_eq!(s.tab, Tab::Picks);
        assert_eq!(s.apply(Act::Next), None);
        assert_eq!(s.tab, Tab::World);
        assert_eq!(s.tab.next(), None);
        assert_eq!(s.apply(Act::Next), None);
        assert_eq!(s.tab, Tab::World);
        assert_eq!(s.apply(Act::Start), Some(Act::Start));
    }

    #[test]
    fn the_screen_is_for_a_new_world() {
        assert!(wanted(&Options::parse(&args("--new-game --seed 4"))));
        for line in ["", "--new-game --connect a", "--new-game --load x", "--new-game-from f.ron", "--new-game --new-game-from f.ron"] {
            assert!(!wanted(&Options::parse(&args(line))), "{line}");
        }
    }

    #[test]
    fn its_ways_out_keep_the_rest_of_the_arguments() {
        let a = args("--new-game --seed 7 --no-menu --zombies 10 --new-game-tab world --background student");
        assert_eq!(start_args(&a, "saves/new_game.ron"), args("--seed 7 --no-menu --zombies 10 --new-game-from saves/new_game.ron"));
        assert_eq!(back_args(&a), args("--seed 7 --no-menu --zombies 10 --main-menu"));
        let o = Options::parse(&start_args(&a, "x.ron"));
        assert!(!wanted(&o) && o.new_game_from.as_deref() == Some("x.ron"));
    }

    #[test]
    fn picking_goes_by_the_points_and_what_cant_go_together() {
        let mut s = screen("--new-game");
        let o = s.rules.origins.clone();
        assert_eq!(s.choice, o.default_choice());
        assert!(s.check().is_ok());
        // A Background with no points: a Strength can't be afforded until a Flaw pays for it.
        let poor = o.backgrounds.iter().position(|b| b.points == 0).expect("a Background with no points");
        s.apply(Act::Background(poor));
        let strong = o.strengths.iter().position(|p| p.id == "strong_back").unwrap();
        s.apply(Act::Strength(strong));
        assert!(o.points_left(&s.choice) < 0);
        assert_eq!(s.apply(Act::Start), None);
        assert!(s.error.as_deref().is_some_and(|e| e.contains("over")), "{:?}", s.error);
        // Its opposite can't be the Flaw that pays.
        let bad_back = o.flaws.iter().find(|p| p.id == "bad_back").unwrap();
        assert_eq!(s.clash(bad_back).map(|p| p.id.as_str()), Some("strong_back"));
        let sickly = o.flaws.iter().position(|p| p.id == "sickly").unwrap();
        s.apply(Act::Flaw(sickly));
        assert!(o.points_left(&s.choice) >= 0);
        assert_eq!(s.apply(Act::Start), Some(Act::Start));
        // Clicked again, a pick is dropped.
        s.apply(Act::Flaw(sickly));
        assert!(s.choice.flaws.is_empty());
    }

    #[test]
    fn the_wardrobe_steps_round_and_only_bare_slots_go_empty() {
        let mut s = screen("--new-game --seed 2");
        let slots = s.rules.origins.wardrobe.clone();
        let (hat, shirt) = (slots.iter().position(|w| w.default.is_none()).unwrap(), slots.iter().position(|w| w.default.is_some()).unwrap());
        assert_eq!(s.worn_from(hat), None);
        s.apply(Act::Wear(hat, 1));
        assert_eq!(s.worn_from(hat), Some(&slots[hat].items[0]));
        s.apply(Act::Wear(hat, -1));
        assert_eq!(s.worn_from(hat), None);
        for _ in 0..slots[shirt].items.len() + 2 {
            s.apply(Act::Wear(shirt, 1));
            assert!(s.worn_from(shirt).is_some());
            assert!(s.check().is_ok());
        }
    }

    #[test]
    fn world_options_go_over_the_preset_and_a_preset_clears_them() {
        let mut s = screen("--new-game --seed 3");
        let plain = s.setup.difficulty.clone();
        assert!(!s.setup.custom());
        assert_eq!(Opt::Zombies.steps()[Opt::Zombies.nearest(s.value(Opt::Zombies))].1, "Normal");
        s.apply(Act::Step(Opt::Zombies, 1));
        s.apply(Act::Step(Opt::Infection, -1));
        s.apply(Act::Step(Opt::Wake, -1));
        assert_eq!((s.setup.zombies, s.setup.infection, s.setup.start_hour), (Some(1.5), Some(Infection::Survivable), Some(12.0)));
        // Back to what the preset says: not a change any more.
        s.apply(Act::Step(Opt::Zombies, -1));
        assert_eq!(s.setup.zombies, None);
        // Steps stop at their ends.
        for _ in 0..10 {
            s.apply(Act::Step(Opt::Loot, 1));
        }
        assert_eq!(s.setup.loot, Some(2.5));
        // Every preset's own numbers sit on a step of each option, so nothing reads wrong.
        for (k, p) in s.rules.difficulty.presets.clone().iter().enumerate() {
            s.apply(Act::Preset(k));
            assert_eq!((s.setup.difficulty.as_str(), s.setup.zombies, s.setup.loot), (p.id.as_str(), None, None));
            for opt in Opt::ALL {
                let v = s.value(opt);
                assert_eq!(opt.steps()[opt.nearest(v)].0, v, "{}: {opt:?} {v}", p.id);
            }
            assert!(s.check().is_ok());
        }
        assert!(s.rules.difficulty.get(&plain).is_some());
        // The game takes the same choices as flags.
        let o = Options::parse(&args("--background night_nurse --strengths strong_back --flaws big_appetite,sickly --outfit t_shirt,jeans,work_boots --difficulty hard_times"));
        let mut config = s.config.clone();
        let start = from_options(&o, &mut config).unwrap().expect("a survivor");
        assert_eq!((start.origin.background.as_str(), start.outfit.len()), ("night_nurse", 3));
        assert!(config.population.scale > 1.0 && config.sandbox.difficulty == "hard_times");
        assert_eq!(from_options(&Options::parse(&[]), &mut config).unwrap(), None);
        assert!(from_options(&Options::parse(&args("--background nobody")), &mut config).is_err());
    }
}
