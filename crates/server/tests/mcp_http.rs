//! `zombye-server --mcp-http` as a process (#87): the real binary, started as a service would start
//! it (no stdin, the token in the environment), called over a real socket.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

const TOKEN: &str = "a-token-for-the-process-test";

/// A folder of its own for one test, removed when dropped (pass or fail).
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("zombye-mcp-http-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// The server process, killed when dropped.
struct Server {
    child: Child,
    addr: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn server_command(extra: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zombye-server"));
    command.args(["--mcp-http", "127.0.0.1:0", "--map-chunks", "8", "--zombies", "30", "--seed", "7"]).args(extra);
    command.env_remove("ZOMBYE_MCP_TOKEN").env_remove("ZOMBYE_CLIENT").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    command
}

/// Start it and wait for the line that says where it listens (port 0: whichever was free).
fn start(extra: &[&str]) -> Server {
    let mut child = server_command(extra).env("ZOMBYE_MCP_TOKEN", TOKEN).spawn().unwrap();
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut seen = Vec::new();
    let addr = loop {
        let line = lines.next().unwrap_or_else(|| panic!("the server stopped before listening: {seen:?}")).unwrap();
        if let Some(rest) = line.strip_prefix("MCP over HTTP on http://") {
            assert!(!line.contains(TOKEN), "the token is never printed: {line}");
            break rest.split("/mcp").next().unwrap().to_string();
        }
        seen.push(line);
    };
    // Keep reading what it says, so it never blocks on a full pipe.
    std::thread::spawn(move || lines.for_each(drop));
    Server { child, addr }
}

/// One request on its own connection: the status and the body.
fn http(addr: &str, method: &str, token: Option<&str>, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let head = format!("{method} /mcp HTTP/1.1\r\nHost: {addr}\r\n{auth}Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    stream.write_all((head + body).as_bytes()).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    let (head, body) = reply.split_once("\r\n\r\n").unwrap_or_else(|| panic!("no response: {reply:?}"));
    (head.split(' ').nth(1).unwrap().parse().unwrap(), body.to_string())
}

fn rpc(addr: &str, msg: Value) -> Value {
    let (status, body) = http(addr, "POST", Some(TOKEN), &msg.to_string());
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

fn tool(addr: &str, name: &str, arguments: Value) -> Value {
    let reply = rpc(addr, json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": name, "arguments": arguments}}));
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    reply["result"]["content"].clone()
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for k in 0..4 {
            out.push(if k <= chunk.len() { ABC[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// A stand-in for the game client: copies a prepared PNG to where `--render-to` says.
#[cfg(unix)]
fn fake_client(dir: &Path, png: &[u8]) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(dir.join("picture.png"), png).unwrap();
    let path = dir.join("fake-zombye");
    let script = format!("#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in --render-to) out=\"$2\"; shift;; esac; shift; done\ncp '{}/picture.png' \"$out\"\n", dir.display());
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn it_wont_start_without_a_token() {
    let out = server_command(&[]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("ZOMBYE_MCP_TOKEN") && said.contains("--mcp-no-auth"), "{said}");
}

/// Started with no stdin (a service), it stays up, keeps strangers out, lets an agent play, and
/// carries a screenshot's image through whole.
#[cfg(unix)]
#[test]
fn the_server_process_is_played_over_http() {
    let scratch = scratch("play");
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(640u32.to_be_bytes());
    png.extend(360u32.to_be_bytes());
    png.extend((0..=255u8).cycle().take(5000));
    let client = fake_client(&scratch.0, &png);
    let server = start(&["--client", client.to_str().unwrap()]);
    let addr = server.addr.as_str();

    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string();
    assert_eq!(http(addr, "POST", None, &ping).0, 401);
    assert_eq!(http(addr, "POST", Some("not-the-token-not-the-token"), &ping).0, 401);
    assert_eq!(http(addr, "GET", Some(TOKEN), "").0, 405);

    let init = rpc(addr, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "zombye-server");
    assert_eq!(http(addr, "POST", Some(TOKEN), &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string()), (202, String::new()));
    let tools = rpc(addr, json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    assert!(tools["result"]["tools"].as_array().unwrap().len() > 40, "{tools}");

    tool(addr, "step_ticks", json!({"n": 30}));
    tool(addr, "move_player", json!({"direction": "e", "ticks": 10}));
    let stats: Value = serde_json::from_str(tool(addr, "stats", json!({}))[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(stats["tick"], 40, "one world, whichever connection asks");

    let shot = tool(addr, "screenshot", json!({"width": 640, "height": 360}));
    assert_eq!((shot[0]["type"].as_str(), shot[0]["mimeType"].as_str()), (Some("image"), Some("image/png")));
    assert_eq!(shot[0]["data"].as_str().unwrap(), base64(&png), "the picture arrives byte for byte");
    assert!(shot[1]["text"].as_str().unwrap().starts_with("screenshot 640x360"), "{shot}");
}

/// A public server: `--mcp-no-dev` with a token, and `--mcp-no-auth` whatever else is said.
#[test]
fn the_server_process_can_leave_the_debug_tools_out() {
    let server = start(&["--mcp-no-dev"]);
    let tools = rpc(&server.addr, json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"observe") && !names.contains(&"dev") && !names.contains(&"load") && !names.contains(&"spawn_zombies"), "{names:?}");
    let refused = rpc(&server.addr, json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "dev", "arguments": {"god": true}}}));
    assert_eq!(refused["error"]["code"], -32602, "{refused}");
}
