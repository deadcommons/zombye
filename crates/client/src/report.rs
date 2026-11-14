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
//! survivor stands). Once the screenshot is taken a small box asks "What happened?" ([`ReportBox`]):
//! a line of the player's own (optional), which goes into `about.txt` and the report's details
//! (`what`). Enter sends it, Esc sends without a note; while the box is up every other key is held
//! back. Then the folder is sent (below), or, when nothing is sent, opened in the file manager.
//!
//! **Sent to us** (Linear DCM-92, `zombye_net::report`): when the game has a report address
//! (`report::endpoint`, compiled into packaged builds) and "Send problem reports" is on (Settings,
//! Game; on unless turned off, and the yellow line says so once), the F12 folder is also uploaded,
//! a crash or panic is sent by the supervising parent as soon as the game has fallen over (the
//! child's panic hook leaves `logs/panic.zrpt` for it), and the game sends a report of its own when
//! a frame takes over [`HITCH_SECS`] or the prediction of our survivor is corrected by more than a
//! metre [`DESYNC_COUNT`] times in a minute (each at most once every [`REPORT_GAP_SECS`]). Reports
//! wait in `reports/pending/` until the report service has them (`report::queue`, `flush`).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::input::InputSystems;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};

use std::sync::{Arc, Mutex};

use bevy::window::PrimaryWindow;
use zombye_net::report::{self, Report, kind};

use crate::player::{LocalPlayer, Prediction};
use crate::saving::Notice;
use crate::server_link::ServerLink;
use crate::ui_scale::{ClientSettings, SettingsStore};

const CHILD: &str = "ZOMBYE_LOGGED";
/// A frame longer than this is a hitch worth a report.
pub const HITCH_SECS: f32 = 1.0;
/// Corrections of over a metre within a minute that make a desync report.
pub const DESYNC_COUNT: usize = 3;
/// At most one hitch report and one desync report this often.
pub const REPORT_GAP_SECS: f32 = 15.0 * 60.0;
/// Nothing is watched for this long after the start (loading).
const SETTLE_SECS: f32 = 30.0;
/// The most of the log a report carries.
const LOG_TAIL: usize = 512 * 1024;
/// A save bigger than this stays out of an F12 upload.
const MAX_SAVE: u64 = 12 << 20;

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
    let panic_file = log.with_file_name(PANIC_FILE);
    let _ = std::fs::remove_file(&panic_file);
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
        let sent = report_crash(&args, &log, &panic_file, &status);
        let _ = std::fs::write(log.with_file_name("last-crash.txt"), format!("{}\n{sent}", crash.display()));
    }
    let _ = std::fs::remove_file(&panic_file);
    Some(code.unwrap_or(101))
}

/// The child's panic hook leaves its report here for the parent.
const PANIC_FILE: &str = "panic.zrpt";

/// In the packaged game's child: a panic leaves its report (message, place, backtrace) in the logs
/// folder before the usual hook prints it, for [`supervise`] to send.
pub fn capture_panics() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let Some(file) = logs_dir().map(|d| d.join(PANIC_FILE)) else { return };
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The first panic is the one that matters; later ones (unwinding elsewhere) don't overwrite it.
        if !file.exists() {
            let _ = std::fs::write(&file, report::encode(&report::panic_report("client", info)));
        }
        previous(info);
    }));
}

/// The saves folder a run with `args` uses (`--saves DIR`, else the default).
fn saves_of(args: &[String]) -> PathBuf {
    args.windows(2).find(|w| w[0] == "--saves").map_or_else(zombye_net::install::default_saves_dir, |w| PathBuf::from(&w[1]))
}

/// The anonymous id of this install (a hash of its identity key), or empty.
fn install_of(saves: &Path) -> String {
    zombye_net::identity::Identity::load_or_create(&saves.join(zombye_net::identity::FILE)).map_or(String::new(), |i| report::install_id(&i.key()))
}

/// Reports may be sent: there is somewhere to send them and the player hasn't said no.
fn allowed(settings: &ClientSettings) -> bool {
    settings.send_reports && report::endpoint().is_some()
}

/// After a crash, in the parent: queue a report (the child's panic, else how it ended, with the
/// log) and send it now if allowed. Says what became of it, for the next start's yellow line.
fn report_crash(args: &[String], log: &Path, panic_file: &Path, status: &std::process::ExitStatus) -> String {
    let ended = ended(status);
    let report = std::fs::read(panic_file).ok().and_then(|b| report::decode(&b).ok()).unwrap_or_else(|| {
        Report::new(kind::CRASH, "client", format!("The game ended with {ended}"), format!("crash | {ended}")).message(format!("The game ended with {ended}, without a panic."))
    });
    let saves = saves_of(args);
    let report = report.install(install_of(&saves)).detail("ended", &ended).file_tail("zombye.log", log, LOG_TAIL);
    let settings = ClientSettings::load(&ClientSettings::path(&saves)).0;
    if !allowed(&settings) {
        return "not sent".into();
    }
    if report::queue(&reports_dir(), &report).is_err() {
        return "not sent".into();
    }
    let flushed = report::flush(&reports_dir());
    if flushed.taken.is_empty() { "queued".into() } else { "sent".into() }
}

/// How a process ended, in words: an exit code, or on Unix the signal.
fn ended(status: &std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit code {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    "no exit code".into()
}

/// Report folders go here.
fn reports_dir() -> PathBuf {
    zombye_net::install::user_dir().map_or_else(|| PathBuf::from("reports"), |d| d.join("reports"))
}

/// Open `dir` in the system's file manager (best effort).
pub(crate) fn reveal(dir: &Path) {
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
        app.init_resource::<Uploads>()
            .init_resource::<ReportBox>()
            .add_systems(Startup, box_for_picture)
            .add_systems(PreUpdate, box_keys.after(InputSystems).before(crate::pause_menu::PauseKeys).before(crate::chat::chat_keys).before(crate::inventory_ui::TextEntry))
            .add_systems(Update, (crashed_last_time, report_key, send_pending, uploaded, draw_box))
            .add_systems(Last, (watch_frames, watch_prediction));
    }
}

/// Uploads under way in other threads: each says on the yellow line what came of it.
#[derive(Resource, Default)]
struct Uploads(Arc<Mutex<Vec<String>>>);

/// Queue `report` and send what's waiting, in another thread. `say`: put what came of it on the
/// yellow line (an F12 report).
fn send_soon(report: Report, uploads: Option<&Uploads>) {
    let said = uploads.map(|u| u.0.clone());
    std::thread::spawn(move || {
        let id = report.meta.kind.clone();
        let line = match report::queue(&reports_dir(), &report) {
            Ok(_) => {
                let f = report::flush(&reports_dir());
                match (f.taken.last(), &f.error) {
                    (Some(n), _) => format!("Report sent to us as #{n}: thanks! Mention #{n} if you tell us what happened."),
                    (None, Some(e)) => format!("Couldn't send the report ({e}): it goes next time the game starts."),
                    (None, None) => "The report will be sent next time the game starts.".into(),
                }
            }
            Err(e) => format!("Couldn't keep the {id} report to send: {e}"),
        };
        println!("report: {line}");
        if let Some(said) = said {
            said.lock().unwrap().push(line);
        }
    });
}

fn uploaded(time: Res<Time>, uploads: Res<Uploads>, mut notice: ResMut<Notice>) {
    let Some(line) = uploads.0.lock().unwrap().pop() else { return };
    notice.say(time.elapsed_secs(), line);
}

/// A few seconds in: send what earlier runs left waiting, and say once that reports are sent.
fn send_pending(time: Res<Time>, options: Res<crate::Options>, store: Option<ResMut<SettingsStore>>, mut notice: ResMut<Notice>, mut done: Local<bool>) {
    if *done || time.elapsed_secs() < 8.0 || options.offscreen() {
        return;
    }
    *done = true;
    let Some(mut store) = store else { return };
    if !allowed(&store.settings) {
        return;
    }
    std::thread::spawn(|| {
        let f = report::flush(&reports_dir());
        if !f.taken.is_empty() || f.error.is_some() {
            println!("report: sent {} waiting from before{}", f.taken.len(), f.error.map_or(String::new(), |e| format!(" ({e}; {} still waiting)", f.left)));
        }
    });
    if zombye_net::install::packaged() && !store.settings.reports_notice_seen {
        notice.say(time.elapsed_secs(), "Crashes, F12 reports and bad stutters are sent to the developers to fix, with no name or address. Settings, Game turns it off.".into());
        let _ = store.change(|s| s.reports_notice_seen = true);
    }
}

/// What every report from the game says about where it was.
fn facts(report: Report, link: &ServerLink, me: &LocalPlayer, options: &crate::Options) -> Report {
    report
        .install(install_of(&options.saves))
        .detail("mode", if link.net.is_some() { "multiplayer" } else { "single-player" })
        .detail("tick", link.curr.tick)
        .detail("zombies_in_view", link.curr.zombies.len())
        .detail("at", format!("{:.1},{:.1}", me.curr.pos.x, me.curr.pos.y))
        .detail("hp", format!("{:.0}", me.hp))
}

/// Frames that take far too long, while the window has focus.
#[allow(clippy::too_many_arguments)]
fn watch_frames(
    real: Res<Time<Real>>,
    options: Res<crate::Options>,
    store: Option<Res<SettingsStore>>,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut last: Local<Option<f32>>,
) {
    let (dt, now) = (real.delta_secs(), real.elapsed_secs());
    if dt < HITCH_SECS || now < SETTLE_SECS || options.offscreen() || !store.is_some_and(|s| allowed(&s.settings)) {
        return;
    }
    if !windows.single().is_ok_and(|w| w.focused) || last.is_some_and(|t| now - t < REPORT_GAP_SECS) {
        return;
    }
    *last = Some(now);
    let mut r = Report::new(kind::HITCH, "client", format!("A frame took {dt:.1} s"), format!("hitch | {}", std::env::consts::OS)).detail("frame_ms", format!("{:.0}", dt * 1000.0));
    if let Some(log) = log_file() {
        r = r.file_tail("zombye.log", &log, LOG_TAIL);
    }
    send_soon(facts(r, &link, &me, &options), None);
}

/// Our survivor's prediction corrected by over a metre [`DESYNC_COUNT`] times within a minute.
#[allow(clippy::too_many_arguments)]
fn watch_prediction(
    real: Res<Time<Real>>,
    options: Res<crate::Options>,
    store: Option<Res<SettingsStore>>,
    prediction: Res<Prediction>,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    mut seen: Local<(u32, Vec<f32>, Option<f32>)>,
) {
    let now = real.elapsed_secs();
    let (counted, times, last) = &mut *seen;
    if prediction.big == *counted {
        return;
    }
    *counted = prediction.big;
    times.push(now);
    times.retain(|t| now - t < 60.0);
    if times.len() < DESYNC_COUNT || now < SETTLE_SECS || options.offscreen() || !store.is_some_and(|s| allowed(&s.settings)) || last.is_some_and(|t| now - t < REPORT_GAP_SECS) {
        return;
    }
    *last = Some(now);
    let mode = if link.net.is_some() { "multiplayer" } else { "single-player" };
    let mut r = Report::new(kind::DESYNC, "client", format!("Prediction corrected {} times in a minute (up to {:.1} m)", times.len(), prediction.worst), format!("desync | prediction | {mode}"))
        .detail("corrections", times.len())
        .detail("worst_m", format!("{:.2}", prediction.worst))
        .detail("last_m", format!("{:.2}", prediction.last_error));
    times.clear();
    if let Some(log) = log_file() {
        r = r.file_tail("zombye.log", &log, LOG_TAIL);
    }
    send_soon(facts(r, &link, &me, &options), None);
}

/// In another thread once the F12 screenshot is on disk: send the report folder.
fn upload_folder(dir: PathBuf, report: Report, uploads: &Uploads) {
    let shot = dir.join("screenshot.png");
    let said = Uploads(uploads.0.clone());
    std::thread::spawn(move || {
        // The screenshot is written a frame or two later: wait for it to stop growing (6 s at most).
        let mut size = 0;
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let now = std::fs::metadata(&shot).map_or(0, |m| m.len());
            if now > 0 && now == size {
                break;
            }
            size = now;
        }
        let mut report = report;
        for name in ["about.txt", "screenshot.png"] {
            if let Ok(bytes) = std::fs::read(dir.join(name)) {
                report = report.file(name, bytes);
            }
        }
        report = report.file_tail("zombye.log", &dir.join("zombye.log"), LOG_TAIL);
        let save = dir.join("world.zsave");
        if std::fs::metadata(&save).is_ok_and(|m| m.len() <= MAX_SAVE)
            && let Ok(bytes) = std::fs::read(&save)
        {
            report = report.file("world.zsave", bytes);
        }
        send_soon(report, Some(&said));
    });
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
        let (crash, sent) = crash.split_once('\n').unwrap_or((crash.as_str(), ""));
        notice.say(
            time.elapsed_secs(),
            match sent {
                "sent" => format!("The game crashed last time, sorry. A report went to us. Its log is {crash}."),
                "queued" => format!("The game crashed last time, sorry. A report goes to us when it can. Its log is {crash}."),
                _ => format!("The game crashed last time, sorry. Its log is {crash}: please send it to us (F12 makes a report)."),
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn report_key(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    options: Res<crate::Options>,
    store: Option<Res<SettingsStore>>,
    mut report_box: ResMut<ReportBox>,
    mut notice: ResMut<Notice>,
) {
    if !keys.just_pressed(KeyCode::F12) || options.offscreen() || report_box.open {
        return;
    }
    let dir = reports_dir().join(stamp(secs_now()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return notice.say(time.elapsed_secs(), format!("Couldn't make a report: {}: {e}", dir.display()));
    }
    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.join("screenshot.png"))).observe(|_: On<ScreenshotCaptured>, mut b: ResMut<ReportBox>| b.shot = true);
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
    let report = store.is_some_and(|s| allowed(&s.settings)).then(|| facts(Report::new(kind::PLAYER, "client", "Report from a player (F12)", "f12"), &link, &me, &options));
    *report_box = ReportBox { open: true, shot: false, opened_at: time.elapsed_secs(), text: String::new(), pending: Some(Pending { dir, report }) };
}

/// The most a player's note on an F12 report holds.
pub const MAX_NOTE: usize = 400;
/// The box shows this long after F12 at the latest, even if the screenshot never says it's done.
const SHOT_WAIT_SECS: f32 = 1.5;

/// The F12 box: "What happened?", with the report made and waiting for the player's line.
#[derive(Resource, Default)]
pub struct ReportBox {
    /// Up and taking the keys.
    pub open: bool,
    /// The screenshot has been taken, so the box can be drawn without being in it.
    shot: bool,
    opened_at: f32,
    pub text: String,
    pending: Option<Pending>,
}

impl ReportBox {
    /// Up and taking typed letters (the on-screen keyboard asks).
    pub fn typing(&self) -> bool {
        self.open
    }
}

struct Pending {
    dir: PathBuf,
    /// The report to send; none when reports aren't sent (the folder is opened instead).
    report: Option<Report>,
}

/// `about.txt` with the player's note in place of the line asking for it.
pub fn with_note(about: &str, note: &str) -> String {
    let note = note.trim();
    let shown = if note.is_empty() { "(nothing written)" } else { note };
    match about.rfind("What happened (write here):") {
        Some(at) => format!("{}What happened:\n{shown}\n", &about[..at]),
        None => format!("{about}\nWhat happened:\n{shown}\n"),
    }
}

/// The box's typing: letters into the note, Enter sends, Esc sends without a note. Every other key
/// and button is held back while it is up.
#[allow(clippy::too_many_arguments)]
fn box_keys(
    mut typed: MessageReader<KeyboardInput>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut report_box: ResMut<ReportBox>,
    options: Res<crate::Options>,
    uploads: Res<Uploads>,
    time: Res<Time>,
    mut notice: ResMut<Notice>,
) {
    if !report_box.open || options.offscreen() {
        typed.clear();
        return;
    }
    let mut send = None;
    for k in typed.read().filter(|k| k.state.is_pressed()) {
        match &k.logical_key {
            Key::Character(c) => {
                let room = MAX_NOTE.saturating_sub(report_box.text.chars().count());
                report_box.text.extend(c.chars().filter(|c| !c.is_control()).take(room));
            }
            Key::Space if report_box.text.chars().count() < MAX_NOTE => report_box.text.push(' '),
            Key::Backspace => {
                report_box.text.pop();
            }
            Key::Enter => send = Some(report_box.text.trim().to_string()),
            Key::Escape => send = Some(String::new()),
            _ => {}
        }
    }
    keys.reset_all();
    mouse.reset_all();
    let Some(note) = send else { return };
    let pending = report_box.pending.take();
    *report_box = ReportBox::default();
    let Some(Pending { dir, report }) = pending else { return };
    let about = dir.join("about.txt");
    let _ = std::fs::write(&about, with_note(&std::fs::read_to_string(&about).unwrap_or_default(), &note));
    match report {
        Some(mut r) => {
            if !note.is_empty() {
                r = r.detail("what", &note);
            }
            upload_folder(dir.clone(), r, &uploads);
            notice.say(time.elapsed_secs(), format!("Sending the report to us (a copy is in {})...", dir.display()));
        }
        None => {
            reveal(&dir);
            notice.say(time.elapsed_secs(), format!("Report saved to {}: send us the folder", dir.display()));
        }
    }
}

/// `--report-box TEXT`: the box up with that note, for pictures.
fn box_for_picture(options: Res<crate::Options>, mut report_box: ResMut<ReportBox>) {
    if let Some(text) = &options.report_box {
        *report_box = ReportBox { open: true, shot: true, text: text.clone(), ..default() };
    }
}

#[derive(Component)]
struct BoxRoot;
#[derive(Component)]
struct BoxText;
#[derive(Component)]
struct BoxCount;

/// Draw the box while it is up (once the screenshot is taken), the note as typed.
fn draw_box(
    mut commands: Commands,
    report_box: Res<ReportBox>,
    time: Res<Time>,
    root: Query<Entity, With<BoxRoot>>,
    mut text: Query<&mut Text, (With<BoxText>, Without<BoxCount>)>,
    mut count: Query<&mut Text, (With<BoxCount>, Without<BoxText>)>,
) {
    use crate::theme::flat::*;
    use crate::theme::{font, head};
    let show = report_box.open && (report_box.shot || time.elapsed_secs() - report_box.opened_at > SHOT_WAIT_SECS);
    let shown = root.iter().next();
    if !show {
        if let Some(e) = shown {
            commands.entity(e).despawn();
        }
        return;
    }
    let typed = format!("{}|", report_box.text);
    let left = format!("{} / {MAX_NOTE}", report_box.text.chars().count());
    if shown.is_some() {
        for mut t in &mut text {
            if t.0 != typed {
                t.0 = typed.clone();
            }
        }
        for mut t in &mut count {
            if t.0 != left {
                t.0 = left.clone();
            }
        }
        return;
    }
    commands
        .spawn((
            BoxRoot,
            Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
            GlobalZIndex(40),
        ))
        .with_children(|r| {
            r.spawn((
                Node { width: px(560), flex_direction: FlexDirection::Column, row_gap: px(12), padding: UiRect::all(px(22)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(RADIUS)), ..default() },
                BackgroundColor(SOLID),
                BorderColor::all(LINE),
            ))
            .with_children(|p| {
                p.spawn((Text::new("WHAT HAPPENED?"), head(TITLE), TextColor(BONE)));
                p.spawn((Text::new("A report with a screenshot and the log is ready. A line about what went wrong helps us find it. You can leave it empty."), font(BODY), TextColor(DIM)));
                p.spawn((
                    Node { min_height: px(84), padding: UiRect::all(px(10)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(RADIUS)), ..default() },
                    BackgroundColor(CELL),
                    BorderColor::all(GREEN),
                ))
                .with_children(|f| {
                    f.spawn((BoxText, Text::new(typed.clone()), font(BODY), TextColor(BONE), Node { width: percent(100), ..default() }));
                });
                p.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(16), align_items: AlignItems::Center, ..default() }).with_children(|h| {
                    hint(h, &["Enter"], "send");
                    hint(h, &["Esc"], "send without a note");
                    h.spawn(Node { flex_grow: 1.0, ..default() });
                    h.spawn((BoxCount, Text::new(left.clone()), font(SMALL), TextColor(FAINT)));
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_takes_the_place_of_the_ask() {
        let about = "Zombye x\nserver: single-player\n\nWhat happened (write here):\n";
        assert_eq!(with_note(about, "  fell through the floor "), "Zombye x\nserver: single-player\n\nWhat happened:\nfell through the floor\n");
        assert!(with_note(about, "").ends_with("What happened:\n(nothing written)\n"));
        assert!(with_note("old", "x").ends_with("What happened:\nx\n"));
    }

    #[test]
    fn stamps_are_dates() {
        assert_eq!(stamp(0), "1970-01-01_00-00-00");
        assert_eq!(stamp(1_794_484_800), "2026-11-12_12-00-00");
    }
}
