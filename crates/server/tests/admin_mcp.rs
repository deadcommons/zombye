//! `zombye-server --host --admin-mcp` as a process (#26): the real binary serving a world, a game
//! client on it over a real socket, and an operator's agent administering it over MCP.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zombye_net::remote::Remote;

const TOKEN: &str = "the-operators-own-token";

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Server {
    child: Child,
    /// Where game clients join, and where the admin endpoint listens.
    game: String,
    admin: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn command(save: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zombye-server"));
    command.args(["--host", "127.0.0.1:0", "--admin-mcp", "127.0.0.1:0", "--map-chunks", "8", "--zombies", "30", "--seed", "7", "--pvp", "off", "--save", save]);
    command.env_remove("ZOMBYE_ADMIN_TOKEN").env_remove("ZOMBYE_MCP_TOKEN").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command
}

fn start(save: &str) -> Server {
    let mut child = command(save).env("ZOMBYE_ADMIN_TOKEN", TOKEN).stderr(Stdio::null()).spawn().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let (mut game, mut seen) = (None, Vec::new());
    let admin = loop {
        let line = lines.next().unwrap_or_else(|| panic!("the server stopped before listening: {seen:?}")).unwrap();
        assert!(!line.contains(TOKEN), "the token is never printed: {line}");
        if let Some((_, rest)) = line.split_once(": listening on ") {
            game = Some(rest.split(' ').next().unwrap().to_string());
        }
        if let Some(rest) = line.strip_prefix("admin MCP on http://") {
            break rest.split("/mcp").next().unwrap().to_string();
        }
        seen.push(line);
    };
    // Keep reading what it says, so it never blocks on a full pipe.
    std::thread::spawn(move || lines.for_each(drop));
    Server { child, game: game.expect("the game's address comes first"), admin }
}

fn http(addr: &str, token: Option<&str>, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let head = format!("POST /mcp HTTP/1.1\r\nHost: {addr}\r\n{auth}Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    stream.write_all((head + body).as_bytes()).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    let (head, body) = reply.split_once("\r\n\r\n").unwrap_or_else(|| panic!("no response: {reply:?}"));
    (head.split(' ').nth(1).unwrap().parse().unwrap(), body.to_string())
}

fn rpc(addr: &str, msg: Value) -> Value {
    let (status, body) = http(addr, Some(TOKEN), &msg.to_string());
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

/// One tool call: its text.
fn tool(addr: &str, name: &str, arguments: Value) -> String {
    let reply = rpc(addr, json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": name, "arguments": arguments}}));
    assert_ne!(reply["result"]["isError"], true, "{reply}");
    reply["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no text: {reply}")).to_string()
}

#[test]
fn an_agent_administers_a_running_server() {
    let dir = Scratch(std::env::temp_dir().join(format!("zombye-admin-mcp-test-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let save = dir.0.join("server.zsave");
    let server = start(save.to_str().unwrap());

    // Nobody gets in without the operator's token, and the game's port doesn't speak MCP.
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string();
    assert_eq!(http(&server.admin, None, &list).0, 401);
    assert_eq!(http(&server.admin, Some("not-the-operators-token"), &list).0, 401);
    let tools = rpc(&server.admin, json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for name in ["status", "players", "kick", "save", "settings", "horde", "logs", "observe"] {
        assert!(names.contains(&name), "{name} in {names:?}");
    }
    assert!(!names.contains(&"step_ticks") && !names.contains(&"load"), "{names:?}");

    // Someone plays; the world runs by itself between the agent's calls.
    let mut ada = Remote::connect(&server.game, "ada", "", Duration::from_secs(20)).expect("ada joins");
    ada.wait_snapshot(Duration::from_secs(20)).expect("her first snapshot");
    let status = |addr: &str| serde_json::from_str::<Value>(&tool(addr, "status", json!({}))).unwrap();
    let first = status(&server.admin);
    assert_eq!((first["online"].as_u64(), first["rules"]["pvp"].as_bool(), first["address"].as_str()), (Some(1), Some(false), Some(server.game.as_str())), "{first}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(status(&server.admin)["tick"].as_u64() > first["tick"].as_u64(), "the server ticks on between calls");
    let players: Value = serde_json::from_str(&tool(&server.admin, "players", json!({}))).unwrap();
    assert_eq!((players["online"][0]["name"].as_str(), players["online"][0]["player"].as_u64()), (Some("ada"), Some(1)), "{players}");

    // A setting, an event, a save, a kick: each done to the live world, and logged.
    assert!(tool(&server.admin, "settings", json!({"pvp": true, "name": "Dead Commons"})).contains("pvp on"));
    assert!(tool(&server.admin, "horde", json!({"name": "ada", "count": 15})).starts_with("a horde of 15"));
    assert!(tool(&server.admin, "save", json!({})).starts_with("saved tick "));
    assert!(save.exists() && dir.0.join("server.zsave.players").exists());
    assert!(tool(&server.admin, "kick", json!({"name": "ada", "reason": "server restarting"})).starts_with("kicked ada (player 1): server restarting"));
    let start = Instant::now();
    while ada.closed.is_none() && start.elapsed() < Duration::from_secs(20) {
        ada.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(ada.closed.as_deref().is_some_and(|why| why.contains("kicked: server restarting")), "{:?}", ada.closed);
    let log = tool(&server.admin, "logs", json!({"limit": 200}));
    let mut from = 0;
    for want in ["ada joined from 127.0.0.1", "admin: settings: pvp on", "admin: a horde of 15", "admin: saved tick", "admin: kicked ada", "ada left: kicked: server restarting"] {
        from += log[from..].find(want).unwrap_or_else(|| panic!("{want:?} in order in:\n{log}"));
    }
    assert!(!log.contains(TOKEN));
    assert_eq!(status(&server.admin)["online"].as_u64(), Some(0));
}

/// With no token the server doesn't start at all, and says how to give one.
#[test]
fn no_token_no_server() {
    let out = command("unused.zsave").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("--admin-mcp needs a token") && said.contains("ZOMBYE_ADMIN_TOKEN"), "{said}");
    assert!(!std::path::Path::new("unused.zsave").exists(), "it stopped before making a world");
}
