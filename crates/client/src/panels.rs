//! Panels remember where they were and whether they were open (#97).
//!
//! The inventory, health and skills panels ([`PanelId`], on each panel's root beside
//! `hud::TopPanel`) can be **dragged by their title**: press on the strip along a panel's top
//! ([`TITLE_H`] UI pixels: its title line) and move. Where it's let go is kept ([`PanelPlaces`], the
//! top-left corner in UI pixels, so it means the same at any UI scale) and `hud::fit_top_panels`
//! puts the panel there, kept on the screen and above the vitals whatever the window's size
//! ([`placed`]). A **double-click on the title** puts a panel back where it's laid out by default.
//! A press that doesn't move isn't a drag (the controls on the title line still take their clicks),
//! and a panel scrolled down has no title on show, so it can't be dragged until scrolled back.
//!
//! Each panel's place and whether it's open go into the client settings file (`settings.ron` in the
//! saves folder, `ui_scale::SettingsStore`) when they change, and come back at the next start:
//! `--inventory`, `--skills` and `--health` still open theirs whatever the file says. An offscreen
//! render (`--render-to`) never reads or writes the file; `--panel-at inventory:X,Y[;skills:X,Y]`
//! puts panels somewhere for a picture.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::health_ui::HealthPanel;
use crate::inventory_ui::InventoryPanel;
use crate::skills_ui::SkillsPanel;
use crate::ui_scale::{PanelMemory, PanelsMemory, SettingsStore};

/// A panel that can be dragged and is remembered.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelId {
    Inventory,
    Health,
    Skills,
}

impl PanelId {
    pub const ALL: [PanelId; 3] = [PanelId::Inventory, PanelId::Health, PanelId::Skills];

    /// Where the panel is laid out when it hasn't been dragged: its `left` and `right`.
    pub fn home(self) -> (Val, Val) {
        match self {
            PanelId::Inventory => (Val::Auto, px(16)),
            PanelId::Health => (px(16), Val::Auto),
            PanelId::Skills => (px(340), Val::Auto),
        }
    }

    fn memory(self, m: &mut PanelsMemory) -> &mut PanelMemory {
        match self {
            PanelId::Inventory => &mut m.inventory,
            PanelId::Health => &mut m.health,
            PanelId::Skills => &mut m.skills,
        }
    }
}

/// The height of the strip along a panel's top that drags it (UI pixels): the padding and the
/// title line.
pub const TITLE_H: f32 = 40.0;
/// How far a press has to move before it's a drag (UI pixels).
const DRAG_START: f32 = 4.0;
/// Two presses on a title within this long are a double-click (seconds).
const DOUBLE_CLICK_SECS: f32 = 0.35;
/// A panel is never dragged so low that less than this of it shows above the vitals (UI pixels).
const KEEP_H: f32 = 80.0;

/// Where each panel was dragged to (its top-left corner, UI pixels), by `PanelId as usize`.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct PanelPlaces(pub [Option<Vec2>; 3]);

impl PanelPlaces {
    pub fn of(&self, id: PanelId) -> Option<Vec2> {
        self.0[id as usize]
    }

    fn from_memory(mut m: PanelsMemory) -> Self {
        Self(PanelId::ALL.map(|id| id.memory(&mut m).at.map(Vec2::from)))
    }

    /// `--panel-at inventory:300,90;skills:40,300`: those panels there, whatever was remembered.
    fn with_flag(mut self, text: &str) -> Self {
        for part in text.split(';') {
            let Some((name, at)) = part.split_once(':') else { continue };
            if let (Some(id), Some(at)) = (PanelId::parse(name), parse_xy(at)) {
                self.0[id as usize] = Some(at);
            }
        }
        self
    }
}

/// `"300, 90.5"` → (300, 90.5).
fn parse_xy(text: &str) -> Option<Vec2> {
    text.split_once(',').and_then(|(x, y)| Some(Vec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))).filter(|v| v.is_finite())
}

/// `--drag-panel skills:200,120@6`: which panel, how far (UI pixels) and when (seconds).
fn parse_drag(text: &str) -> Option<(PanelId, Vec2, f32)> {
    let (what, after) = text.split_once('@').unwrap_or((text, "0"));
    let (name, by) = what.split_once(':')?;
    let after: f32 = after.trim().parse().ok().filter(|s: &f32| s.is_finite() && *s >= 0.0)?;
    Some((PanelId::parse(name)?, parse_xy(by)?, after))
}

impl PanelId {
    fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "inventory" => Some(PanelId::Inventory),
            "health" => Some(PanelId::Health),
            "skills" => Some(PanelId::Skills),
            _ => None,
        }
    }
}

/// The drag under way, and the last press on a title (for double-clicks).
#[derive(Resource, Default)]
pub struct PanelDrag {
    active: Option<Dragging>,
    last_press: Option<(PanelId, f32)>,
}

impl PanelDrag {
    /// A panel is being moved (not just pressed).
    pub fn moving(&self) -> bool {
        self.active.is_some_and(|d| d.moved)
    }
}

#[derive(Clone, Copy, Debug)]
struct Dragging {
    id: PanelId,
    /// From the panel's top-left corner to the pointer where it was pressed.
    grab: Vec2,
    pressed_at: Vec2,
    moved: bool,
}

/// Is `cursor` on the title strip of a panel whose top-left corner is `corner` and which is
/// `width` wide (all UI pixels)? Not while the panel is scrolled: its title is out of sight then.
pub fn on_title(cursor: Vec2, corner: Vec2, width: f32, scrolled: f32) -> bool {
    scrolled <= 0.5 && cursor.x >= corner.x && cursor.x <= corner.x + width && cursor.y >= corner.y && cursor.y <= corner.y + TITLE_H
}

/// Where a panel asked to be at `at` goes: all of it on a `view` wide screen while it fits, never
/// off the top or left, and never so low that it's lost under the bottom HUD (which reaches
/// `bottom_reach` up from the bottom). All UI pixels.
pub fn placed(at: Vec2, size: Vec2, view: Vec2, bottom_reach: f32) -> Vec2 {
    let x = at.x.clamp(0.0, (view.x - size.x).max(0.0));
    let y = at.y.clamp(0.0, (view.y - bottom_reach - KEEP_H).max(0.0));
    Vec2::new(x.round(), y.round())
}

pub struct PanelsPlugin;

impl Plugin for PanelsPlugin {
    fn build(&self, app: &mut App) {
        let settings = SettingsStore::ensure(app);
        let flag = app.world().get_resource::<crate::Options>().and_then(|o| o.panel_at.clone()).unwrap_or_default();
        app.insert_resource(PanelPlaces::from_memory(settings.panels).with_flag(&flag))
            .init_resource::<PanelDrag>()
            .add_systems(Update, (script_drag, drag).chain().before(crate::hud::fit_top_panels))
            .add_systems(Update, remember.after(crate::hud::fit_top_panels));
    }
}

/// How long `--drag-panel`'s drag takes (seconds).
const SCRIPT_DRAG_SECS: f32 = 1.2;

/// `--drag-panel PANEL:DX,DY[@SECS]`, for recordings and for checking the real thing: after SECS,
/// puts the pointer on that panel's title, presses the left button, moves it DX,DY UI pixels over
/// a little more than a second and lets go, through the window's pointer and the mouse buttons
/// [`drag`] reads, so it goes the way a hand's drag does.
#[allow(clippy::too_many_arguments)]
fn script_drag(
    time: Res<Time>,
    options: Option<Res<crate::Options>>,
    ui: Res<UiScale>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    panels: Panels,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut going: Local<Option<(f32, Vec2)>>,
    mut held: Local<u32>,
    mut done: Local<bool>,
) {
    let Some((id, by, after)) = options.as_ref().and_then(|o| o.drag_panel.as_deref()).and_then(parse_drag) else { return };
    let (now, scale) = (time.elapsed_secs(), ui.0.max(0.01));
    let Ok(mut window) = window.single_mut() else { return };
    if *done || now < after {
        return;
    }
    match *going {
        None => {
            // Onto its title, a little in from the corner (UI pixels), and press.
            let Some(corner) = panels.iter().find(|(p, .., vis, _)| **p == id && vis.get()).map(|(_, node, _, at, ..)| (at.translation - node.size() / 2.0) * node.inverse_scale_factor()) else { return };
            let start = corner + Vec2::new(70.0, 16.0);
            window.set_cursor_position(Some(start * scale));
            mouse.press(MouseButton::Left);
            *going = Some((now, start));
        }
        Some((began, start)) => {
            let k = ((now - began) / SCRIPT_DRAG_SECS).min(1.0);
            window.set_cursor_position(Some((start + by * k) * scale));
            // Held at the end for a few frames before letting go: the window system reports the
            // pointer a frame or two behind where it was put.
            if k >= 1.0 {
                *held += 1;
            }
            if *held > 3 {
                mouse.release(MouseButton::Left);
                *done = true;
            }
        }
    }
}

/// The panels a press could land on.
type Panels<'w, 's> = Query<'w, 's, (&'static PanelId, &'static ComputedNode, &'static bevy::ui::ComputedStackIndex, &'static UiGlobalTransform, &'static InheritedVisibility, &'static ScrollPosition)>;

fn drag(time: Res<Time>, mouse: Res<ButtonInput<MouseButton>>, ui: Res<UiScale>, window: Query<&Window, With<PrimaryWindow>>, panels: Panels, mut state: ResMut<PanelDrag>, mut places: ResMut<PanelPlaces>) {
    let Some(cursor) = window.single().ok().and_then(|w| w.cursor_position()).map(|c| c / ui.0.max(0.01)) else {
        if !mouse.pressed(MouseButton::Left) {
            state.active = None;
        }
        return;
    };
    // On a press: the shown panel drawn on top under the pointer, if the pointer is on its title.
    let hit = mouse.just_pressed(MouseButton::Left).then(|| {
        panels
            .iter()
            .filter(|(.., vis, _)| vis.get())
            .filter_map(|(id, node, stack, at, _, scroll)| {
                let k = node.inverse_scale_factor();
                let size = node.size() * k;
                let corner = at.translation * k - size / 2.0;
                let inside = cursor.cmpge(corner).all() && cursor.cmple(corner + size).all();
                inside.then_some((*id, stack.0, corner, size.x, scroll.y))
            })
            .max_by_key(|&(_, stack, ..)| stack)
            .filter(|&(_, _, corner, width, scrolled)| on_title(cursor, corner, width, scrolled))
            .map(|(id, _, corner, ..)| (id, corner))
    });
    let mut next = *places;
    state.step(cursor, hit, mouse.pressed(MouseButton::Left), time.elapsed_secs(), &mut next);
    if next != *places {
        *places = next;
    }
}

impl PanelDrag {
    /// One frame of the pointer: at `cursor` (UI pixels), the left button `down` or not, and
    /// `pressed` when it went down this frame: the panel whose title it landed on with that
    /// panel's top-left corner, or `None` for a press anywhere else.
    fn step(&mut self, cursor: Vec2, pressed: Option<Option<(PanelId, Vec2)>>, down: bool, now: f32, places: &mut PanelPlaces) {
        if let Some(hit) = pressed {
            self.active = None;
            match hit {
                // Double-click: back to where it's laid out.
                Some((id, _)) if self.last_press.is_some_and(|(was, at)| was == id && now - at < DOUBLE_CLICK_SECS) => {
                    places.0[id as usize] = None;
                    self.last_press = None;
                }
                Some((id, corner)) => {
                    self.active = Some(Dragging { id, grab: cursor - corner, pressed_at: cursor, moved: false });
                    self.last_press = Some((id, now));
                }
                None => self.last_press = None,
            }
        }
        if !down {
            self.active = None;
            return;
        }
        if let Some(d) = &mut self.active {
            if !d.moved && cursor.distance(d.pressed_at) >= DRAG_START {
                d.moved = true;
                // A drag isn't half a double-click.
                self.last_press = None;
            }
            if d.moved {
                places.0[d.id as usize] = Some((cursor - d.grab).round());
            }
        }
    }
}

/// Keep the settings file up with the panels: where they are (once let go) and which are open.
fn remember(drag: Res<PanelDrag>, places: Res<PanelPlaces>, inventory: Res<InventoryPanel>, health: Res<HealthPanel>, skills: Res<SkillsPanel>, mut was_open: Local<Option<[bool; 3]>>, mut store: ResMut<SettingsStore>) {
    if drag.moving() {
        return;
    }
    let open = [inventory.open, health.open, skills.open];
    // What a flag opened at the start isn't a choice to remember; a change since is.
    let toggled = was_open.is_some_and(|was| was != open);
    *was_open = Some(open);
    let result = store.change(|s| {
        for id in PanelId::ALL {
            let m = id.memory(&mut s.panels);
            m.at = places.of(id).map(Into::into);
            if toggled {
                m.open = open[id as usize];
            }
        }
    });
    if let Err(e) = result {
        eprintln!("panels not remembered: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_title_strip_drags_and_not_while_scrolled() {
        let corner = Vec2::new(100.0, 70.0);
        assert!(on_title(Vec2::new(150.0, 80.0), corner, 300.0, 0.0));
        assert!(on_title(Vec2::new(100.0, 70.0 + TITLE_H), corner, 300.0, 0.0));
        // Below the strip, beside the panel, above it.
        assert!(!on_title(Vec2::new(150.0, 70.0 + TITLE_H + 1.0), corner, 300.0, 0.0));
        assert!(!on_title(Vec2::new(401.0, 80.0), corner, 300.0, 0.0));
        assert!(!on_title(Vec2::new(150.0, 69.0), corner, 300.0, 0.0));
        // Scrolled, the rows are under the pointer there, not the title.
        assert!(!on_title(Vec2::new(150.0, 80.0), corner, 300.0, 40.0));
    }

    #[test]
    fn a_dragged_panel_stays_on_the_screen() {
        let (size, view) = (Vec2::new(400.0, 300.0), Vec2::new(1280.0, 720.0));
        // Where it was put, when that's on the screen.
        assert_eq!(placed(Vec2::new(200.4, 90.6), size, view, 140.0), Vec2::new(200.0, 91.0));
        // Not off the left or the top, nor past the right edge.
        assert_eq!(placed(Vec2::new(-50.0, -20.0), size, view, 140.0), Vec2::ZERO);
        assert_eq!(placed(Vec2::new(5000.0, 90.0), size, view, 140.0).x, 1280.0 - 400.0);
        // Never under the vitals: some of it stays above them.
        assert_eq!(placed(Vec2::new(200.0, 5000.0), size, view, 140.0).y, 720.0 - 140.0 - KEEP_H);
        // A place kept from a bigger window comes back onto a smaller one, and a panel wider than
        // the screen starts at its left edge.
        assert_eq!(placed(Vec2::new(1500.0, 800.0), size, Vec2::new(1280.0, 720.0), 140.0), Vec2::new(880.0, 500.0));
        assert_eq!(placed(Vec2::new(300.0, 90.0), Vec2::new(1400.0, 300.0), view, 140.0).x, 0.0);
    }

    #[test]
    fn a_press_on_a_title_drags_and_a_double_click_puts_it_back() {
        let (mut drag, mut places) = (PanelDrag::default(), PanelPlaces::default());
        let corner = Vec2::new(340.0, 70.0);
        let at = |x: f32, y: f32| Vec2::new(x, y);
        // Pressed on the skills title and held still: a click, nothing moves.
        drag.step(at(400.0, 80.0), Some(Some((PanelId::Skills, corner))), true, 1.0, &mut places);
        drag.step(at(401.0, 81.0), None, true, 1.02, &mut places);
        assert!(!drag.moving());
        assert_eq!(places, PanelPlaces::default());
        // Moved: the panel follows the pointer by where it was grabbed, and only that panel.
        drag.step(at(500.0, 180.0), None, true, 1.1, &mut places);
        assert!(drag.moving());
        assert_eq!(places.of(PanelId::Skills), Some(corner + at(100.0, 100.0)));
        drag.step(at(520.4, 60.0), None, true, 1.2, &mut places);
        assert_eq!(places.of(PanelId::Skills), Some(at(460.0, 50.0)));
        assert_eq!((places.of(PanelId::Inventory), places.of(PanelId::Health)), (None, None));
        // Let go: it stays, and moving the pointer on doesn't drag it.
        drag.step(at(520.0, 60.0), None, false, 1.3, &mut places);
        drag.step(at(900.0, 600.0), None, false, 1.4, &mut places);
        assert!(!drag.moving());
        assert_eq!(places.of(PanelId::Skills), Some(at(460.0, 50.0)));
        // A press somewhere else with the button held and moved drags nothing.
        drag.step(at(900.0, 600.0), Some(None), true, 2.0, &mut places);
        drag.step(at(700.0, 300.0), None, true, 2.1, &mut places);
        assert_eq!(places.of(PanelId::Skills), Some(at(460.0, 50.0)));
        drag.step(at(700.0, 300.0), None, false, 2.2, &mut places);
        // Two quick presses on its title: back to where it's laid out. Two slow ones: not.
        let moved = at(460.0, 50.0);
        drag.step(at(470.0, 60.0), Some(Some((PanelId::Skills, moved))), true, 3.0, &mut places);
        drag.step(at(470.0, 60.0), None, false, 3.05, &mut places);
        drag.step(at(470.0, 60.0), Some(Some((PanelId::Skills, moved))), true, 3.0 + DOUBLE_CLICK_SECS + 0.2, &mut places);
        drag.step(at(470.0, 60.0), None, false, 3.7, &mut places);
        assert_eq!(places.of(PanelId::Skills), Some(moved));
        drag.step(at(470.0, 60.0), Some(Some((PanelId::Skills, moved))), true, 3.8, &mut places);
        assert_eq!(places.of(PanelId::Skills), None);
        // A press on one panel then another isn't a double-click.
        places.0[PanelId::Health as usize] = Some(at(30.0, 200.0));
        drag.step(at(40.0, 210.0), Some(Some((PanelId::Inventory, at(0.0, 200.0)))), true, 5.0, &mut places);
        drag.step(at(40.0, 210.0), None, false, 5.05, &mut places);
        drag.step(at(40.0, 210.0), Some(Some((PanelId::Health, at(30.0, 200.0)))), true, 5.1, &mut places);
        assert_eq!(places.of(PanelId::Health), Some(at(30.0, 200.0)));
    }

    #[test]
    fn places_come_from_the_settings() {
        let mut m = PanelsMemory::default();
        m.health.at = Some((30.0, 200.0));
        let places = PanelPlaces::from_memory(m);
        assert_eq!(places.of(PanelId::Health), Some(Vec2::new(30.0, 200.0)));
        assert_eq!((places.of(PanelId::Inventory), places.of(PanelId::Skills)), (None, None));
        // The flag moves the ones it names, and bad parts of it are passed over.
        let places = places.with_flag("skills:40,300; Inventory: 300 , 90.5;health:x,2;bogus:1,2;nonsense");
        assert_eq!(places.of(PanelId::Skills), Some(Vec2::new(40.0, 300.0)));
        assert_eq!(places.of(PanelId::Inventory), Some(Vec2::new(300.0, 90.5)));
        assert_eq!(places.of(PanelId::Health), Some(Vec2::new(30.0, 200.0)));
        assert_eq!(parse_drag("skills:200,-120@6.5"), Some((PanelId::Skills, Vec2::new(200.0, -120.0), 6.5)));
        assert_eq!(parse_drag("Health:10,20"), Some((PanelId::Health, Vec2::new(10.0, 20.0), 0.0)));
        for bad in ["", "skills", "skills:1", "door:1,2", "skills:1,2@soon", "skills:1,2@-1"] {
            assert_eq!(parse_drag(bad), None, "{bad:?}");
        }
    }

    /// The panels' open state and places are written when they change, and a flag's opening at the
    /// start isn't taken for a choice.
    #[test]
    fn open_panels_and_places_are_remembered() {
        use crate::ui_scale::ClientSettings;
        let d = std::env::temp_dir().join(format!("zombye-panels-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let mut app = App::new();
        app.insert_resource(crate::Options::parse(&["--saves".to_string(), d.display().to_string(), "--skills".to_string()]))
            .insert_resource(InventoryPanel::default())
            .insert_resource(HealthPanel::default())
            .insert_resource(SkillsPanel { open: true })
            .init_resource::<PanelDrag>();
        let settings = SettingsStore::ensure(&mut app);
        app.insert_resource(PanelPlaces::from_memory(settings.panels)).add_systems(Update, remember);
        let file = ClientSettings::path(&d);
        app.update();
        app.update();
        // Nothing chosen yet: `--skills` alone writes nothing.
        assert!(!file.exists());
        app.world_mut().resource_mut::<InventoryPanel>().open = true;
        app.update();
        let read = ClientSettings::load(&file).0.panels;
        assert!(read.inventory.open && read.skills.open && !read.health.open);
        app.world_mut().resource_mut::<InventoryPanel>().open = false;
        app.world_mut().resource_mut::<PanelPlaces>().0[PanelId::Skills as usize] = Some(Vec2::new(40.0, 90.0));
        app.update();
        let read = ClientSettings::load(&file).0.panels;
        assert_eq!((read.inventory.open, read.skills), (false, PanelMemory { open: true, at: Some((40.0, 90.0)) }));
        // And the next session starts from it.
        let next = SettingsStore::from_options(&crate::Options::parse(&["--saves".to_string(), d.display().to_string()])).0;
        assert_eq!(PanelPlaces::from_memory(next.settings.panels).of(PanelId::Skills), Some(Vec2::new(40.0, 90.0)));
        let _ = std::fs::remove_dir_all(&d);
    }
}
