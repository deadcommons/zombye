//! Playtest builds: a log file, a note when the game crashed, and F12 to report a problem.
//!
//! **The log.** A packaged game (`zombye_net::install::packaged`) has nobody watching its stdout,
//! so it runs itself again as a child with stdout and stderr going to `logs/zombye.log` in the
//! player's own folder (the last run's kept as `zombye.prev.log`), and waits for it
//! ([`supervise`]). If the game ends in a crash (a panic, a signal, an exit code but 0 or 1), the
//! log is copied to `logs/crash-<time>.log` and the next start says so on the yellow line. From a
//! checkout nothing changes.
//!
//! **F12** writes a report folder, `reports/<time>/` in the player's folder: a screenshot, the log
//! so far, a save of the world (single-player) and `about.txt` (the build, the system, where the
//! survivor stands), opens it in the file manager and says on the yellow line where it is, to send
//! to us.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use crate::saving::Notice;
use crate::player::LocalPlayer;
use crate::server_link::ServerLink;

const CHILD: &str = "ZOMBYE_LOGGED";

fn secs_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `YYYY-MM-DD_HH-MM-SS` (UTC) for `secs` since 1970.
fn stamp(secs: u64) -> String {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}_{:02}-{:02}-{:02}", rest / 3600, rest / 60 % 60, rest % 60)
}

fn logs_dir() -> Option<PathBuf> {
    let dir = zombye_net::install::user_dir()?.join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// This run's log file, when there is one (a packaged game).
pub fn log_file() -> Option<PathBuf> {
    logs_dir().map(|d| d.join("zombye.log"))
}

/// In a packaged game, run the game as a child with its output to the log file and return its
/// exit code (the caller exits with it); `None` to run the game here (a checkout, the child
/// itself, or a run that only writes a file).
pub fn supervise() -> Option<i32> {
    if !zombye_net::install::packaged() || std::env::var_os(CHILD).is_some() {
        return None;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--render-to" || a == "--write-pack" || a == "--server-info" || a == "--version") {
        return None;
    }
    let log = log_file()?;
    let _ = std::fs::rename(&log, log.with_file_name("zombye.prev.log"));
    let out = std::fs::File::create(&log).ok()?;
    let err = out.try_clone().ok()?;
    let exe = std::env::current_exe().ok()?;
    let status = std::process::Command::new(exe)
        .args(&args)
        .env(CHILD, "1")
        .env("RUST_BACKTRACE", "1")
        .stdout(out)
        .stderr(err)
        .status()
        .ok()?;
    let code = status.code();
    if !matches!(code, Some(0) | Some(1)) {
        let crash = log.with_file_name(format!("crash-{}.log", stamp(secs_now())));
        let _ = std::fs::copy(&log, &crash);
        let _ = std::fs::write(log.with_file_name("last-crash.txt"), crash.display().to_string());
    }
    Some(code.unwrap_or(101))
}

/// Report folders go here.
fn reports_dir() -> PathBuf {
    zombye_net::install::user_dir().map_or_else(|| PathBuf::from("reports"), |d| d.join("reports"))
}

/// Open `dir` in the system's file manager (best effort).
fn reveal(dir: &Path) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(dir).spawn();
}

pub struct ReportPlugin;

impl Plugin for ReportPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (crashed_last_time, report_key));
    }
}

/// The yellow line, once: the last run crashed, and where its log is.
fn crashed_last_time(time: Res<Time>, mut notice: ResMut<Notice>, mut done: Local<bool>) {
    if *done || time.elapsed_secs() < 2.0 {
        return;
    }
    *done = true;
    let Some(marker) = logs_dir().map(|d| d.join("last-crash.txt")) else { return };
    if let Ok(crash) = std::fs::read_to_string(&marker) {
        let _ = std::fs::remove_file(&marker);
        notice.say(time.elapsed_secs(), format!("The game crashed last time, sorry. Its log is {crash}: please send it to us (F12 makes a report)."));
    }
}

fn report_key(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    options: Res<crate::Options>,
    mut notice: ResMut<Notice>,
) {
    if !keys.just_pressed(KeyCode::F12) || options.offscreen() {
        return;
    }
    let dir = reports_dir().join(stamp(secs_now()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return notice.say(time.elapsed_secs(), format!("Couldn't make a report: {}: {e}", dir.display()));
    }
    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.join("screenshot.png")));
    if let Some(log) = log_file() {
        let _ = std::fs::copy(&log, dir.join("zombye.log"));
    }
    let saved = match link.world() {
        Some(world) => zombye_net::save_file::save(world, &dir.join("world.zsave")).map_or_else(|e| format!("not saved: {e}"), |s| s.to_string()),
        None => "on a server: no save".to_string(),
    };
    let about = format!(
        "Zombye {}\nsystem: {} {}\nwhen: {} UTC\nsurvivor: at {:.1}, {:.1}, hp {:.0}{}\nserver: {}\nworld: {saved}\n\nWhat happened (write here):\n",
        zombye_net::install::build_name(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        stamp(secs_now()),
        me.curr.pos.x,
        me.curr.pos.y,
        me.hp,
        if me.dead { " (dead)" } else { "" },
        link.net.as_ref().map_or("single-player".to_string(), |n| n.addr.clone()),
    );
    let _ = std::fs::write(dir.join("about.txt"), about);
    reveal(&dir);
    notice.say(time.elapsed_secs(), format!("Report saved to {}: add a line to about.txt and send us the folder", dir.display()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_dates() {
        assert_eq!(stamp(0), "1970-01-01_00-00-00");
        assert_eq!(stamp(1_794_484_800), "2026-11-12_12-00-00");
    }
}
