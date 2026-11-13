//! `zombye-reports`: the problem-report service (Linear DCM-92).
//!
//! Games and servers send reports ([`zombye_net::report`]: panics, crashes, F12, hitches, desyncs,
//! slow ticks) to `serve`, which keeps them in SQLite (`reports.db`) with their files beside it
//! (`files/<id>/`), and groups them into **problems** by signature: one row per problem, with how
//! many reports, from how many installs, first and last seen, and its triage state (new, filed,
//! fixed, ignored, regressed) and Linear issue. The other subcommands answer questions about them
//! as JSON, for an agent over ssh (docs/bug-reports.md):
//!
//! ```text
//! zombye-reports serve [--addr 0.0.0.0:7780] [--dir DIR] [--max-gb 20]
//! zombye-reports problems [--since DAYS] [--status S] [--kind K] [--limit N]
//! zombye-reports list [--since DAYS] [--kind K] [--signature SIG] [--problem N] [--limit N]
//! zombye-reports show ID
//! zombye-reports mark SIGNATURE|#N [--status new|filed|fixed|ignored|regressed] [--issue DCM-12] [--note TEXT]
//! zombye-reports sql "SELECT ..."          (read-only)
//! zombye-reports prune --days N            (reports and files older than that; problems stay)
//! ```
//!
//! `DIR` is `--dir`, else `$ZOMBYE_REPORTS_DIR`, else `~/zombye-reports`. `ZOMBYE_REPORT_KEY`, if
//! set, is the key every report must carry (`X-Zombye-Key`). No IP is stored: one is only held in
//! memory, for the rate limit (`PER_HOUR` reports an hour from one address).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params, types::ValueRef};
use serde_json::{Value, json};
use zombye_net::report::{self, Report};

const DEFAULT_ADDR: &str = "0.0.0.0:7780";
/// Reports an hour from one address.
const PER_HOUR: u32 = 40;
/// Connections at once.
const MAX_CONNECTIONS: usize = 32;
const IO_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_HEAD: usize = 16 * 1024;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS reports (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    received  INTEGER NOT NULL,      -- seconds since 1970, when the service got it
    at        INTEGER NOT NULL,      -- when it happened, by the sender's clock
    kind      TEXT NOT NULL,         -- panic, crash, f12, hitch, desync, slow_tick, error
    source    TEXT NOT NULL,         -- client or server
    title     TEXT NOT NULL,
    signature TEXT NOT NULL,         -- the problem it belongs to (problems.signature)
    message   TEXT NOT NULL,
    location  TEXT NOT NULL,
    build     TEXT NOT NULL,
    os        TEXT NOT NULL,
    arch      TEXT NOT NULL,
    install   TEXT NOT NULL,         -- anonymous id of the install
    details   TEXT NOT NULL,         -- JSON object
    files     TEXT NOT NULL,         -- JSON list of file names, in files/<id>/
    bytes     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS reports_signature ON reports(signature);
CREATE INDEX IF NOT EXISTS reports_received ON reports(received);
CREATE TABLE IF NOT EXISTS problems (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    signature  TEXT NOT NULL UNIQUE,
    kind       TEXT NOT NULL,
    title      TEXT NOT NULL,
    first_seen INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL,
    count      INTEGER NOT NULL,
    status     TEXT NOT NULL DEFAULT 'new',   -- new, filed, fixed, ignored, regressed
    issue      TEXT,                          -- the Linear issue, e.g. DCM-12
    note       TEXT,
    fixed_at   INTEGER                        -- when it was marked fixed
);
";

const STATUSES: [&str; 5] = ["new", "filed", "fixed", "ignored", "regressed"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
    let dir = value("--dir").map(PathBuf::from).or_else(|| std::env::var_os("ZOMBYE_REPORTS_DIR").map(PathBuf::from)).unwrap_or_else(|| {
        std::env::var_os("HOME").map_or_else(|| PathBuf::from("zombye-reports"), |h| PathBuf::from(h).join("zombye-reports"))
    });
    let result = match args.first().map(String::as_str) {
        Some("serve") => serve(&dir, &value("--addr").unwrap_or_else(|| DEFAULT_ADDR.into()), value("--max-gb").and_then(|v| v.parse().ok()).unwrap_or(20.0)),
        Some("problems") | Some("groups") => problems(&dir, &args),
        Some("list") => list(&dir, &args),
        Some("show") => match args.get(1).and_then(|v| v.trim_start_matches('#').parse::<i64>().ok()) {
            Some(id) => show(&dir, id),
            None => Err("show ID".into()),
        },
        Some("mark") => match args.get(1) {
            Some(sig) if !sig.starts_with("--") => mark(&dir, sig, value("--status"), value("--issue"), value("--note")),
            _ => Err("mark SIGNATURE|#N [--status S] [--issue DCM-N] [--note TEXT]".into()),
        },
        Some("sql") => match args.get(1) {
            Some(q) => sql(&dir, q),
            None => Err("sql \"SELECT ...\"".into()),
        },
        Some("prune") => match value("--days").and_then(|v| v.parse::<u64>().ok()) {
            Some(days) => prune(&dir, days),
            None => Err("prune --days N".into()),
        },
        _ => Err("zombye-reports serve | problems | list | show | mark | sql | prune (see the source's docs or docs/bug-reports.md)".into()),
    };
    if let Err(e) = result {
        eprintln!("zombye-reports: {e}");
        std::process::exit(1);
    }
}

fn open(dir: &Path) -> Result<Connection, String> {
    std::fs::create_dir_all(dir.join("files")).map_err(|e| format!("{}: {e}", dir.display()))?;
    let db = Connection::open(dir.join("reports.db")).map_err(|e| e.to_string())?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;").map_err(|e| e.to_string())?;
    db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    Ok(db)
}

fn open_read(dir: &Path) -> Result<Connection, String> {
    let path = dir.join("reports.db");
    if !path.exists() {
        // Make it (empty) so a question before the first report gets an empty answer.
        drop(open(dir)?);
    }
    let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| e.to_string())?;
    db.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;
    Ok(db)
}

/// Keep a report: its row, its files, its problem. Returns its id.
fn store(db: &Connection, dir: &Path, r: &Report, received: u64) -> Result<i64, String> {
    let m = &r.meta;
    let names: Vec<&str> = r.files.iter().map(|(n, _)| n.as_str()).collect();
    let bytes: usize = r.files.iter().map(|(_, b)| b.len()).sum();
    let details = serde_json::to_string(&m.details).unwrap_or_else(|_| "{}".into());
    let clip = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    db.execute(
        "INSERT INTO reports (received, at, kind, source, title, signature, message, location, build, os, arch, install, details, files, bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            received as i64,
            m.at as i64,
            clip(&m.kind, 32),
            clip(&m.source, 16),
            clip(&m.title, 200),
            clip(&m.signature, 300),
            clip(&m.message, 8000),
            clip(&m.location, 300),
            clip(&m.build, 80),
            clip(&m.os, 32),
            clip(&m.arch, 32),
            clip(&m.install, 32),
            details,
            serde_json::to_string(&names).unwrap_or_else(|_| "[]".into()),
            bytes as i64
        ],
    )
    .map_err(|e| e.to_string())?;
    let id = db.last_insert_rowid();
    // A player's report is a problem of its own; the rest group by what the game said.
    let signature = if m.kind == report::kind::PLAYER || m.signature.trim().is_empty() { format!("{} | #{id}", m.kind) } else { clip(&m.signature, 300) };
    db.execute("UPDATE reports SET signature = ?1 WHERE id = ?2", params![signature, id]).map_err(|e| e.to_string())?;
    let files = dir.join("files").join(id.to_string());
    if !r.files.is_empty() {
        let written = std::fs::create_dir_all(&files).and_then(|_| r.files.iter().try_for_each(|(name, b)| std::fs::write(files.join(name), b)));
        if let Err(e) = written {
            let _ = db.execute("DELETE FROM reports WHERE id = ?1", params![id]);
            let _ = std::fs::remove_dir_all(&files);
            return Err(format!("writing files: {e}"));
        }
    }
    db.execute(
        "INSERT INTO problems (signature, kind, title, first_seen, last_seen, count) VALUES (?1, ?2, ?3, ?4, ?4, 1)
         ON CONFLICT(signature) DO UPDATE SET last_seen = excluded.last_seen, count = count + 1,
           status = CASE WHEN status = 'fixed' THEN 'regressed' ELSE status END",
        params![signature, clip(&m.kind, 32), clip(&m.title, 200), received as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(id)
}

struct Shared {
    db: Mutex<Connection>,
    dir: PathBuf,
    key: String,
    max_bytes: u64,
    /// Reports from each address this hour (in memory only).
    seen: Mutex<HashMap<IpAddr, (u64, u32)>>,
    connections: AtomicUsize,
}

fn serve(dir: &Path, addr: &str, max_gb: f64) -> Result<(), String> {
    let db = open(dir)?;
    let listener = TcpListener::bind(addr).map_err(|e| format!("can't listen on {addr}: {e}"))?;
    let key = std::env::var("ZOMBYE_REPORT_KEY").unwrap_or_default();
    println!(
        "zombye-reports: listening on {} (POST /v1/report, GET /health), store {}{}",
        listener.local_addr().map_or(addr.to_string(), |a| a.to_string()),
        dir.display(),
        if key.is_empty() { ", no key asked for" } else { ", key required" }
    );
    let shared = Arc::new(Shared { db: Mutex::new(db), dir: dir.to_path_buf(), key, max_bytes: (max_gb * 1e9) as u64, seen: Mutex::new(HashMap::new()), connections: AtomicUsize::new(0) });
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
            let _ = respond(&stream, 503, "busy");
            continue;
        }
        let shared = shared.clone();
        std::thread::spawn(move || {
            let _ = connection(stream, &shared);
            shared.connections.fetch_sub(1, Ordering::SeqCst);
        });
    }
    Ok(())
}

fn respond(mut stream: &TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        411 => "Length Required",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        507 => "Insufficient Storage",
        _ => "Error",
    };
    write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
    stream.flush()
}

fn connection(stream: TcpStream, shared: &Shared) -> std::io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let ip = stream.peer_addr().map(|a| a.ip()).ok();
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut head = Vec::new();
    loop {
        let n = reader.by_ref().take(4096).read_until(b'\n', &mut head)?;
        if n == 0 || head.len() > MAX_HEAD {
            return respond(&stream, 400, "bad request");
        }
        if head.ends_with(b"\r\n\r\n") || head == b"\r\n" {
            break;
        }
    }
    let head = String::from_utf8_lossy(&head);
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (method, path) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    let mut length = None;
    let mut key = String::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse::<usize>().ok(),
                "x-zombye-key" => key = value.trim().to_string(),
                _ => {}
            }
        }
    }
    match (method, path) {
        ("GET", "/health") => return respond(&stream, 200, "ok"),
        ("POST", p) if p.ends_with("/v1/report") => {}
        _ => return respond(&stream, 404, "not found"),
    }
    if !shared.key.is_empty() && key != shared.key {
        return respond(&stream, 403, "wrong key");
    }
    let Some(length) = length else { return respond(&stream, 411, "length required") };
    if length > report::MAX_BODY {
        return respond(&stream, 413, "too big");
    }
    let now = report::now_secs();
    if let Some(ip) = ip {
        let mut seen = shared.seen.lock().unwrap();
        seen.retain(|_, (hour, _)| *hour == now / 3600);
        let entry = seen.entry(ip).or_insert((now / 3600, 0));
        entry.1 += 1;
        if entry.1 > PER_HOUR {
            return respond(&stream, 429, "too many reports for now");
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let report = match report::decode(&body) {
        Ok(r) => r,
        Err(e) => return respond(&stream, 400, &e),
    };
    let stored = {
        let db = shared.db.lock().unwrap();
        let used: i64 = db.query_row("SELECT COALESCE(SUM(bytes), 0) FROM reports", [], |r| r.get(0)).unwrap_or(0);
        if used as u64 + body.len() as u64 > shared.max_bytes {
            Err((507, "the store is full".to_string()))
        } else {
            store(&db, &shared.dir, &report, now).map_err(|e| (503, e))
        }
    };
    match stored {
        Ok(id) => {
            println!("#{id} {} from {} ({}): {}", report.meta.kind, report.meta.source, report.meta.build, report.meta.title);
            respond(&stream, 200, &format!("id {id}"))
        }
        Err((status, e)) => {
            eprintln!("refused a {} report: {e}", report.meta.kind);
            respond(&stream, status, &e)
        }
    }
}

/// The rows of a query as JSON objects.
fn rows(db: &Connection, query: &str, values: &[&dyn rusqlite::ToSql]) -> Result<Vec<Value>, String> {
    let mut stmt = db.prepare(query).map_err(|e| e.to_string())?;
    let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    let mut found = stmt.query(values).map_err(|e| e.to_string())?;
    while let Some(row) = found.next().map_err(|e| e.to_string())? {
        let mut obj = serde_json::Map::new();
        for (i, name) in names.iter().enumerate() {
            let v = match row.get_ref(i).map_err(|e| e.to_string())? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(n) => json!(n),
                ValueRef::Real(f) => json!(f),
                ValueRef::Text(t) => {
                    let s = String::from_utf8_lossy(t).to_string();
                    // JSON columns come back as JSON.
                    if (name == "details" || name == "files") && let Ok(v) = serde_json::from_str::<Value>(&s) { v } else { Value::String(s) }
                }
                ValueRef::Blob(b) => json!(format!("({} bytes)", b.len())),
            };
            obj.insert(name.clone(), v);
        }
        out.push(Value::Object(obj));
    }
    Ok(out)
}

fn print(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn since(args: &[String]) -> i64 {
    let days: f64 = arg(args, "--since").and_then(|v| v.trim_end_matches('d').parse().ok()).unwrap_or(30.0);
    report::now_secs() as i64 - (days * 86_400.0) as i64
}

fn limit(args: &[String], default: i64) -> i64 {
    arg(args, "--limit").and_then(|v| v.parse().ok()).unwrap_or(default).clamp(1, 10_000)
}

fn problems(dir: &Path, args: &[String]) -> Result<(), String> {
    let db = open_read(dir)?;
    let status = arg(args, "--status").unwrap_or_default();
    let kind = arg(args, "--kind").unwrap_or_default();
    let found = rows(
        &db,
        "SELECT p.id, p.signature, p.kind, p.title, p.status, p.issue, p.note, p.count,
                datetime(p.first_seen, 'unixepoch') AS first_seen, datetime(p.last_seen, 'unixepoch') AS last_seen,
                (SELECT COUNT(DISTINCT install) FROM reports r WHERE r.signature = p.signature) AS installs,
                (SELECT GROUP_CONCAT(DISTINCT build) FROM reports r WHERE r.signature = p.signature) AS builds,
                (SELECT GROUP_CONCAT(DISTINCT os) FROM reports r WHERE r.signature = p.signature) AS systems,
                (SELECT MAX(id) FROM reports r WHERE r.signature = p.signature) AS latest_report
         FROM problems p
         WHERE p.last_seen >= ?1 AND (?2 = '' OR p.status = ?2) AND (?3 = '' OR p.kind = ?3)
         ORDER BY CASE p.status WHEN 'regressed' THEN 0 WHEN 'new' THEN 1 ELSE 2 END, p.count DESC, p.last_seen DESC
         LIMIT ?4",
        &[&since(args), &status, &kind, &limit(args, 100)],
    )?;
    print(&Value::Array(found));
    Ok(())
}

fn list(dir: &Path, args: &[String]) -> Result<(), String> {
    let db = open_read(dir)?;
    let kind = arg(args, "--kind").unwrap_or_default();
    let mut signature = arg(args, "--signature").unwrap_or_default();
    if let Some(n) = arg(args, "--problem").and_then(|v| v.trim_start_matches('#').parse::<i64>().ok()) {
        signature = db.query_row("SELECT signature FROM problems WHERE id = ?1", params![n], |r| r.get(0)).map_err(|_| format!("no problem #{n}"))?;
    }
    let found = rows(
        &db,
        "SELECT id, datetime(received, 'unixepoch') AS received, kind, source, title, signature, location, build, os, install, details, files
         FROM reports WHERE received >= ?1 AND (?2 = '' OR kind = ?2) AND (?3 = '' OR signature = ?3)
         ORDER BY id DESC LIMIT ?4",
        &[&since(args), &kind, &signature, &limit(args, 50)],
    )?;
    print(&Value::Array(found));
    Ok(())
}

fn show(dir: &Path, id: i64) -> Result<(), String> {
    let db = open_read(dir)?;
    let mut found = rows(&db, "SELECT *, datetime(received, 'unixepoch') AS received_utc, datetime(at, 'unixepoch') AS at_utc FROM reports WHERE id = ?1", &[&id])?;
    let Some(mut r) = found.pop() else { return Err(format!("no report #{id}")) };
    let files = dir.join("files").join(id.to_string());
    if let Some(obj) = r.as_object_mut() {
        obj.insert("files_dir".into(), json!(files.display().to_string()));
        let sig = obj.get("signature").and_then(Value::as_str).unwrap_or("").to_string();
        if let Some(p) = rows(&db, "SELECT id, status, issue, note, count FROM problems WHERE signature = ?1", &[&sig])?.pop() {
            obj.insert("problem".into(), p);
        }
    }
    print(&r);
    Ok(())
}

fn mark(dir: &Path, which: &str, status: Option<String>, issue: Option<String>, note: Option<String>) -> Result<(), String> {
    if let Some(s) = &status
        && !STATUSES.contains(&s.as_str())
    {
        return Err(format!("status is one of {}", STATUSES.join(", ")));
    }
    let db = open(dir)?;
    let signature: String = match which.strip_prefix('#').and_then(|n| n.parse::<i64>().ok()) {
        Some(n) => db.query_row("SELECT signature FROM problems WHERE id = ?1", params![n], |r| r.get(0)).map_err(|_| format!("no problem #{n}"))?,
        None => which.to_string(),
    };
    let changed = db
        .execute(
            "UPDATE problems SET status = COALESCE(?2, status), issue = COALESCE(?3, issue), note = COALESCE(?4, note),
               fixed_at = CASE WHEN ?2 = 'fixed' THEN ?5 ELSE fixed_at END
             WHERE signature = ?1",
            params![signature, status, issue, note, report::now_secs() as i64],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err(format!("no problem with signature {signature:?}"));
    }
    print(&Value::Array(rows(&db, "SELECT id, signature, status, issue, note, count FROM problems WHERE signature = ?1", &[&signature])?));
    Ok(())
}

fn sql(dir: &Path, query: &str) -> Result<(), String> {
    let db = open_read(dir)?;
    print(&Value::Array(rows(&db, query, &[])?));
    Ok(())
}

fn prune(dir: &Path, days: u64) -> Result<(), String> {
    let db = open(dir)?;
    let before = report::now_secs().saturating_sub(days * 86_400) as i64;
    let ids: Vec<i64> = rows(&db, "SELECT id FROM reports WHERE received < ?1", &[&before])?.iter().filter_map(|r| r["id"].as_i64()).collect();
    for id in &ids {
        let _ = std::fs::remove_dir_all(dir.join("files").join(id.to_string()));
    }
    db.execute("DELETE FROM reports WHERE received < ?1", params![before]).map_err(|e| e.to_string())?;
    println!("pruned {} reports from before {days} days ago", ids.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zombye-reports-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn reports_group_into_problems_and_regress() {
        let dir = temp("group");
        let db = open(&dir).unwrap();
        let panic = |n: u32| Report::new(report::kind::PANIC, "client", "boom", report::signature("panic", "a.rs:1:1", &format!("boom {n}"))).install("i1").file("log.txt", b"log".to_vec());
        let a = store(&db, &dir, &panic(1), 100).unwrap();
        store(&db, &dir, &panic(2), 200).unwrap();
        assert!(dir.join("files").join(a.to_string()).join("log.txt").exists());
        let p = rows(&db, "SELECT count, status FROM problems", &[]).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["count"], 2);
        db.execute("UPDATE problems SET status = 'fixed'", []).unwrap();
        store(&db, &dir, &panic(3), 300).unwrap();
        assert_eq!(rows(&db, "SELECT status FROM problems", &[]).unwrap()[0]["status"], "regressed");
        // Each player's report is its own problem.
        store(&db, &dir, &Report::new(report::kind::PLAYER, "client", "Report from a player", "f12"), 400).unwrap();
        store(&db, &dir, &Report::new(report::kind::PLAYER, "client", "Report from a player", "f12"), 500).unwrap();
        assert_eq!(rows(&db, "SELECT id FROM problems WHERE kind = 'f12'", &[]).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_service_takes_a_report_over_http() {
        let dir = temp("http");
        let shared = Arc::new(Shared { db: Mutex::new(open(&dir).unwrap()), dir: dir.clone(), key: "k".into(), max_bytes: 1 << 30, seen: Mutex::new(HashMap::new()), connections: AtomicUsize::new(0) });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let s = shared.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let _ = connection(stream.unwrap(), &s);
            }
        });
        let body = report::encode(&Report::new(report::kind::HITCH, "client", "Frame took 2 s", "hitch").file("log.txt", b"x".to_vec()));
        // Without the key: refused for good.
        // SAFETY: tests in this binary don't read the environment at the same time.
        unsafe { std::env::remove_var("ZOMBYE_REPORT_KEY") };
        assert!(matches!(report::send(&format!("http://{addr}"), &body, Duration::from_secs(5)), Ok(report::Sent::Refused(_))));
        unsafe { std::env::set_var("ZOMBYE_REPORT_KEY", "k") };
        assert_eq!(report::send(&format!("http://{addr}"), &body, Duration::from_secs(5)), Ok(report::Sent::Taken(1)));
        let db = shared.db.lock().unwrap();
        assert_eq!(rows(&db, "SELECT kind FROM reports", &[]).unwrap()[0]["kind"], "hitch");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
