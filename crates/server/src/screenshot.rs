//! The MCP `screenshot` tool's renderer: a PNG of the world around a player.
//!
//! The server has no renderer, so it saves the world to a temporary file and runs the game client
//! offscreen on it (`zombye --render-to PNG --load SAVE ...`, which never opens a window), waits
//! for it with a timeout and reads the PNG back. The client renders from the player's point of view
//! (fog of war, cutaways, the night lamp) without ticking. Saving only reads the world, so taking
//! a screenshot changes nothing: no ticks, no RNG, the same state hash before and after.
//!
//! The client binary is `--client PATH`, else `$ZOMBYE_CLIENT`, else `zombye` next to this
//! server's binary (`cargo build --release -p zombye-client` puts it there).

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use zombye_net::save_file;
use zombye_sim::World;
use zombye_sim::save::EXTENSION;

/// Environment variable naming the client binary (after `--client`).
pub const CLIENT_ENV: &str = "ZOMBYE_CLIENT";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// Image size: default, smallest and largest (each side).
pub const DEFAULT_SIZE: [u32; 2] = [1280, 720];
pub const MIN_SIDE: u32 = 64;
pub const MAX_SIZE: [u32; 2] = [1920, 1080];
/// Metres of world visible vertically: default and range (the game's zoom limits).
pub const DEFAULT_ZOOM: f32 = 26.0;
pub const ZOOM_RANGE: (f32, f32) = (8.0, 90.0);

/// How the server runs the client.
pub struct Renderer {
    /// `--client PATH`.
    pub client: Option<PathBuf>,
    /// The game data folder, passed on (`--data`): the save only loads against the same content.
    pub data: PathBuf,
    pub timeout: Duration,
    /// Where the temporary save and PNG go.
    pub temp_dir: PathBuf,
}

impl Renderer {
    pub fn new(client: Option<PathBuf>, data: PathBuf, timeout: Duration) -> Self {
        Self { client, data, timeout, temp_dir: std::env::temp_dir() }
    }

    /// The client binary: `--client`, else `$ZOMBYE_CLIENT`, else `zombye` next to this binary.
    pub fn client_path(&self) -> Result<PathBuf, String> {
        let (path, from) = match (&self.client, std::env::var_os(CLIENT_ENV).filter(|v| !v.is_empty())) {
            (Some(p), _) => (p.clone(), "--client"),
            (None, Some(p)) => (PathBuf::from(p), CLIENT_ENV),
            (None, None) => {
                let exe = std::env::current_exe().map_err(|e| format!("can't find this server's binary: {e}"))?;
                (exe.with_file_name(format!("zombye{}", std::env::consts::EXE_SUFFIX)), "next to zombye-server")
            }
        };
        if path.is_file() {
            Ok(path)
        } else {
            Err(format!(
                "no game client at {} ({from}): build it with `cargo build --release -p zombye-client`, or point --client PATH or ${CLIENT_ENV} at the `zombye` binary",
                path.display()
            ))
        }
    }
}

/// What to render.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// Whose view: fog of war, cutaways and the camera (unless `center`).
    pub player: u32,
    /// Centre the picture here instead of on the player.
    pub center: Option<[f32; 2]>,
    pub zoom: f32,
    /// One of the four fixed camera angles, 0..3.
    pub angle: i32,
    pub size: [u32; 2],
}

impl Request {
    /// From the tool's arguments: `player` (default 1), `x` and `y` (both or neither), `zoom`,
    /// `angle`, `width`, `height`. Out-of-range numbers are clamped; wrong types are errors.
    pub fn from_args(args: &Value) -> Result<Request, String> {
        let num = |k: &str| -> Result<Option<f64>, String> {
            match args.get(k) {
                None | Some(Value::Null) => Ok(None),
                Some(v) => v.as_f64().filter(|n| n.is_finite()).map(Some).ok_or_else(|| format!("{k} must be a number, not {v}")),
            }
        };
        let player = match num("player")? {
            None => 1,
            Some(n) if n >= 1.0 && n.fract() == 0.0 && n <= u32::MAX as f64 => n as u32,
            Some(n) => return Err(format!("player must be a player id (1, 2, ...), not {n}")),
        };
        let center = match (num("x")?, num("y")?) {
            (Some(x), Some(y)) => Some([x as f32, y as f32]),
            (None, None) => None,
            _ => return Err("give both x and y (the point to centre on), or neither (centre on the player)".into()),
        };
        let zoom = num("zoom")?.map_or(DEFAULT_ZOOM, |z| (z as f32).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1));
        let angle = num("angle")?.map_or(0, |a| (a.round() as i64).rem_euclid(4) as i32);
        let side = |k: &str, default: u32, max: u32| -> Result<u32, String> { Ok(num(k)?.map_or(default, |v| (v.round().max(0.0) as u32).clamp(MIN_SIDE, max))) };
        let size = [side("width", DEFAULT_SIZE[0], MAX_SIZE[0])?, side("height", DEFAULT_SIZE[1], MAX_SIZE[1])?];
        Ok(Request { player, center, zoom, angle, size })
    }

    /// The client's command line for this request.
    pub fn client_args(&self, save: &Path, png: &Path, data: &Path) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["--render-to".into(), png.into(), "--load".into(), save.into(), "--data".into(), data.into()];
        for (flag, value) in [
            ("--player", self.player.to_string()),
            ("--size", format!("{}x{}", self.size[0], self.size[1])),
            ("--zoom", self.zoom.to_string()),
            ("--angle", self.angle.to_string()),
        ] {
            args.extend([flag.into(), value.into()]);
        }
        if let Some([x, y]) = self.center {
            args.extend(["--center".into(), format!("{x},{y}").into()]);
        }
        args
    }
}

/// Files removed when dropped (also on errors and timeouts).
struct TempFiles(Vec<PathBuf>);

impl Drop for TempFiles {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Temporary file paths for one screenshot, unique within this process and across processes.
fn temp_paths(dir: &Path) -> (PathBuf, PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stem = format!("zombye-shot-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    (dir.join(format!("{stem}.{}", EXTENSION)), dir.join(format!("{stem}.png")))
}

/// Render `req` from `world`: the PNG's bytes and how long it took (ms). Only reads the world.
pub fn render(world: &World, renderer: &Renderer, req: &Request) -> Result<(Vec<u8>, f32), String> {
    let start = Instant::now();
    let client = renderer.client_path()?;
    let (save, png) = temp_paths(&renderer.temp_dir);
    let _cleanup = TempFiles(vec![save.clone(), png.clone()]);
    save_file::save(world, &save)?;
    let mut command = Command::new(&client);
    command.args(req.client_args(&save, &png, &renderer.data)).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if std::env::var_os("RUST_LOG").is_none() {
        // Only warnings and errors from the engine: they're what an error report needs.
        command.env("RUST_LOG", "warn");
    }
    let mut child = command.spawn().map_err(|e| format!("can't run the game client {}: {e}", client.display()))?;
    // Drain both pipes on their own threads so a chatty client can't block on a full pipe (and
    // never inherit stdout: that's the MCP channel). Read what's arrived without waiting for the
    // pipes to close: a stopped client's children may hold them open.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        let text = Arc::new(Mutex::new(String::new()));
        let sink = text.clone();
        let reader = std::thread::spawn(move || {
            let Some(mut p) = pipe else { return };
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = p.read(&mut buf) {
                sink.lock().expect("log lock").push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
        (text, reader)
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let deadline = start + renderer.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(format!("the game client took longer than {} s and was stopped", renderer.timeout.as_secs_f32()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => break Err(format!("lost the game client: {e}")),
        }
    };
    // The pipes close when the client exits; wait a little for the readers to see that.
    let readers_done = Instant::now() + Duration::from_millis(200);
    while !(out.1.is_finished() && err.1.is_finished()) && Instant::now() < readers_done {
        std::thread::sleep(Duration::from_millis(2));
    }
    let err = err.0.lock().expect("log lock").clone();
    let report = |what: String| match log_tail(&err, 8) {
        tail if tail.is_empty() => what,
        tail => format!("{what}:\n{tail}"),
    };
    let status = status.map_err(report)?;
    if !status.success() {
        return Err(report(format!("the game client failed to render ({status})")));
    }
    let bytes = std::fs::read(&png).map_err(|e| report(format!("the game client wrote no picture ({}: {e})", png.display())))?;
    if png_size(&bytes).is_none() {
        return Err(report(format!("{} isn't a PNG", png.display())));
    }
    Ok((bytes, start.elapsed().as_secs_f32() * 1000.0))
}

/// The last `n` non-empty lines of a log, without terminal colour codes.
pub fn log_tail(log: &str, n: usize) -> String {
    let lines: Vec<String> = log.lines().map(strip_ansi).filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ parameters, ending in a letter.
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Width and height from a PNG's header, if `bytes` starts like a PNG.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    Some((be(16), be(20)))
}

/// Standard base64 (RFC 4648, with padding), as MCP image content wants.
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// MCP tool content for a screenshot: the PNG as image content, then a line of text.
pub fn content(png: &[u8], text: &str) -> Value {
    serde_json::json!([
        {"type": "image", "data": base64(png), "mimeType": "image/png"},
        {"type": "text", "text": text}
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn base64_matches_the_rfc_examples() {
        for (raw, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(raw.as_bytes()), enc);
        }
        assert_eq!(base64(&[0xff, 0xfe, 0x00, 0x3f]), "//4APw==");
    }

    #[test]
    fn content_is_an_image_then_text() {
        let c = content(b"\x89PNG", "hello");
        assert_eq!(c[0], json!({"type": "image", "data": "iVBORw==", "mimeType": "image/png"}));
        assert_eq!(c[1], json!({"type": "text", "text": "hello"}));
    }

    #[test]
    fn requests_take_defaults_clamp_and_refuse_nonsense() {
        let r = Request::from_args(&json!({})).unwrap();
        assert_eq!(r, Request { player: 1, center: None, zoom: DEFAULT_ZOOM, angle: 0, size: DEFAULT_SIZE });
        let r = Request::from_args(&json!({"player": 2, "x": 10.5, "y": 20, "zoom": 500, "angle": 7, "width": 99999, "height": 10})).unwrap();
        assert_eq!(r, Request { player: 2, center: Some([10.5, 20.0]), zoom: ZOOM_RANGE.1, angle: 3, size: [MAX_SIZE[0], MIN_SIDE] });
        assert_eq!(Request::from_args(&json!({"angle": -1})).unwrap().angle, 3);
        for bad in [json!({"x": 3}), json!({"player": 0}), json!({"player": 1.5}), json!({"zoom": "far"}), json!({"width": [1]})] {
            assert!(Request::from_args(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn client_args_carry_the_request() {
        let r = Request { player: 3, center: Some([1.5, -2.0]), zoom: 30.0, angle: 2, size: [640, 360] };
        let args: Vec<String> = r.client_args(Path::new("/t/w.zsave"), Path::new("/t/w.png"), Path::new("/data")).into_iter().map(|a| a.into_string().unwrap()).collect();
        assert_eq!(
            args.join(" "),
            "--render-to /t/w.png --load /t/w.zsave --data /data --player 3 --size 640x360 --zoom 30 --angle 2 --center 1.5,-2"
        );
        let plain = Request::from_args(&json!({})).unwrap().client_args(Path::new("s"), Path::new("p"), Path::new("d"));
        assert!(!plain.iter().any(|a| a == "--center"));
    }

    #[test]
    fn temp_files_are_unique_and_cleaned_up() {
        let dir = std::env::temp_dir();
        let (a, b) = (temp_paths(&dir), temp_paths(&dir));
        assert_ne!(a, b);
        assert!(a.0.extension().is_some_and(|e| e == EXTENSION) && a.1.extension().is_some_and(|e| e == "png"));
        std::fs::write(&a.0, b"x").unwrap();
        drop(TempFiles(vec![a.0.clone(), a.1.clone()]));
        assert!(!a.0.exists());
    }

    #[test]
    fn png_headers_and_logs() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(1280u32.to_be_bytes());
        png.extend(720u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((1280, 720)));
        assert_eq!(png_size(b"GIF89a, definitely not a png"), None);
        assert_eq!(log_tail("a\n\n\u{1b}[33m WARN\u{1b}[0m b\nc\n", 2), " WARN b\nc");
    }

    #[test]
    fn a_missing_client_says_how_to_get_one() {
        let r = Renderer::new(Some("/nowhere/zombye".into()), PathBuf::from("."), DEFAULT_TIMEOUT);
        let e = r.client_path().unwrap_err();
        assert!(e.contains("/nowhere/zombye") && e.contains("cargo build --release -p zombye-client"), "{e}");
    }
}
