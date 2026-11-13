//! Keeping the window's full screen across a start-again.
//!
//! The main menu, the new-game screen, the join screen and the game are each a run of this program
//! that hands over to the next (`crate::relaunch`), so each has a window of its own. A Mac puts a
//! full-screen window (the green button, or the Fullscreen setting) in a Space of its own, and a
//! window that was still full screen when its run ended left that Space behind with the old screen
//! on it, while the next screen opened as a small window on the desktop.
//!
//! So an exit that leads to a start-again is held back while the window is full screen: the window
//! leaves full screen first, the exit goes on once it has (or after [`GIVE_UP`]), and the next run
//! is told to open full screen ([`VAR`]), so the next screen comes up as the last one was.
//!
//! Only on a Mac: elsewhere a full-screen window goes with its run. `ZOMBYE_WINDOW_HANDOFF=1`
//! does it on any system, for trying it out.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bevy::app::OnAppExitSystems;
use bevy::ecs::message::Messages;
use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

/// Set for the next run when this one's window was full screen.
pub const VAR: &str = "ZOMBYE_WINDOW_FULLSCREEN";
/// Where this run's window stood (its outer top left corner, physical pixels: `x,y`), so the next
/// run's opens on the same monitor, in the same place, rather than wherever the system puts a new
/// window (with two monitors, often the other one).
pub const AT_VAR: &str = "ZOMBYE_WINDOW_AT";
/// Frames the window has been out of full screen before the exit goes on.
const SETTLED_FRAMES: u32 = 3;
/// The longest an exit is held back.
const GIVE_UP: Duration = Duration::from_secs(3);

/// Whether this run's window was full screen when it handed over.
static WAS_FULLSCREEN: AtomicBool = AtomicBool::new(false);

/// Where this run's window stood when it handed over.
static WAS_AT: std::sync::Mutex<Option<IVec2>> = std::sync::Mutex::new(None);

/// Where the run before this one left its window, if it said.
pub fn position_asked() -> Option<IVec2> {
    parse_at(&std::env::var(AT_VAR).ok()?)
}

fn parse_at(s: &str) -> Option<IVec2> {
    let (x, y) = s.split_once(',')?;
    Some(IVec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// The mode a new window is made in. A window made full screen can't tell which monitor is the
/// current one yet (and a Mac opened the game's on a Space of its own that never came to the
/// front, so it drew nothing); so it is made on the main monitor, or as a plain window where the
/// last run's stood that goes full screen once it is up ([`arrive`]).
pub fn first_mode(fullscreen: bool) -> bevy::window::WindowMode {
    use bevy::window::{MonitorSelection, WindowMode};
    if fullscreen_asked() {
        WindowMode::Windowed
    } else if fullscreen {
        WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
    } else {
        WindowMode::Windowed
    }
}

/// Where a new window opens: where the last run's was, else in the middle of the main monitor.
pub fn first_position() -> bevy::window::WindowPosition {
    match position_asked() {
        Some(at) => bevy::window::WindowPosition::At(at),
        None => bevy::window::WindowPosition::Centered(bevy::window::MonitorSelection::Primary),
    }
}

/// Whether this run was started by one whose window was full screen.
pub fn fullscreen_asked() -> bool {
    std::env::var_os(VAR).is_some_and(|v| v == "1")
}

/// Tell the run `command` starts how this one's window was.
pub fn pass_on(command: &mut std::process::Command) {
    if WAS_FULLSCREEN.load(Ordering::Relaxed) {
        command.env(VAR, "1");
    } else {
        command.env_remove(VAR);
    }
    match WAS_AT.lock().ok().and_then(|a| *a) {
        Some(at) => command.env(AT_VAR, format!("{},{}", at.x, at.y)),
        None => command.env_remove(AT_VAR),
    };
}

fn enabled() -> bool {
    cfg!(target_os = "macos") || std::env::var_os("ZOMBYE_WINDOW_HANDOFF").is_some_and(|v| v == "1")
}

/// An exit being held back.
#[derive(Default)]
struct Held {
    since: Option<Instant>,
    /// Frames in a row the window has been out of full screen.
    out: u32,
    /// The exit went on: hold nothing more.
    let_go: bool,
}

/// Whether a held exit may go on.
fn may_go(out_frames: u32, held_for: Duration) -> bool {
    out_frames >= SETTLED_FRAMES || held_for >= GIVE_UP
}

pub struct HandoffPlugin;

impl Plugin for HandoffPlugin {
    fn build(&self, app: &mut App) {
        // Before anything that acts on the exit (the autosave), so a held exit is seen by
        // nobody until it goes on. (Everywhere it notes where the window is; only where
        // `enabled` does it hold the exit for full screen.)
        app.add_systems(Last, hold_exit.before(OnAppExitSystems));
        app.add_systems(Update, arrive);
    }
}

/// A window that follows another run's: brought to the front (a program started again in its own
/// place isn't, and a window behind others on a Mac draws nothing), then made full screen if the
/// last one was, on the monitor it opened on. Once, a few frames in.
fn arrive(_main: NonSendMarker, mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>, mut frames: Local<u32>) {
    if *frames > ARRIVE_FRAMES {
        return;
    }
    *frames += 1;
    if *frames != ARRIVE_FRAMES || (position_asked().is_none() && !fullscreen_asked()) {
        return;
    }
    let Ok((entity, mut window)) = windows.single_mut() else { return };
    bevy::winit::WINIT_WINDOWS.with_borrow(|w| {
        if let Some(w) = w.get_window(entity) {
            w.focus_window();
        }
    });
    if fullscreen_asked() && window.mode == bevy::window::WindowMode::Windowed {
        window.mode = bevy::window::WindowMode::BorderlessFullscreen(bevy::window::MonitorSelection::Current);
    }
}

/// Frames after its first that a following window is brought forward and made full screen.
const ARRIVE_FRAMES: u32 = 2;

fn hold_exit(_main: NonSendMarker, mut exits: ResMut<Messages<AppExit>>, window: Query<Entity, With<PrimaryWindow>>, mut held: Local<Held>) {
    if held.let_go {
        return;
    }
    let Ok(entity) = window.single() else { return };
    let asked = !exits.is_empty();
    if held.since.is_none() && !(asked && crate::relaunch_pending()) {
        return;
    }
    // Is the window full screen? Leave it, if this is where the hold begins.
    let begin = held.since.is_none();
    let full = bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
        let window = windows.get_window(entity)?;
        if begin && let Ok(p) = window.outer_position() {
            // (Full screen, the monitor's own corner: the next window opens on that monitor.)
            if let Ok(mut at) = WAS_AT.lock() {
                *at = Some(IVec2::new(p.x, p.y));
            }
        }
        let full = window.fullscreen().is_some();
        if !enabled() {
            return Some(false);
        }
        if full && begin {
            window.set_fullscreen(None);
        }
        Some(full)
    });
    let Some(full) = full else { return };
    if begin {
        if !full {
            // A plain window goes with its run.
            return;
        }
        WAS_FULLSCREEN.store(true, Ordering::Relaxed);
        held.since = Some(Instant::now());
    }
    // (Anything asking again meanwhile is held with the first.)
    exits.drain().for_each(drop);
    held.out = if full { 0 } else { held.out + 1 };
    if may_go(held.out, held.since.map_or(GIVE_UP, |s| s.elapsed())) {
        held.let_go = true;
        exits.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_the_window_stood_is_read_back() {
        assert_eq!(parse_at("-1920,25"), Some(IVec2::new(-1920, 25)));
        assert_eq!(parse_at(" 40 , 60"), Some(IVec2::new(40, 60)));
        assert_eq!(parse_at("40"), None);
        assert_eq!(parse_at("a,b"), None);
    }

    #[test]
    fn a_held_exit_goes_on_once_the_window_is_out_or_time_is_up() {
        let ms = Duration::from_millis;
        // Still full screen, or only just out.
        assert!(!may_go(0, ms(500)));
        assert!(!may_go(SETTLED_FRAMES - 1, ms(500)));
        assert!(may_go(SETTLED_FRAMES, ms(500)));
        // A window that never leaves full screen doesn't hold the game for ever.
        assert!(may_go(0, GIVE_UP));
    }
}
