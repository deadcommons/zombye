//! MCP over streamable HTTP (#87): `zombye-server --mcp-http [ADDR[:PORT]]`, for agents that can
//! only reach a server over the network (claude.ai connectors, ChatGPT, the OpenAI Responses API,
//! a hosted server) where `--mcp` speaks stdio to the app that started it.
//!
//! **One world per process**, shared by every caller: connections only carry JSON-RPC messages to
//! the main thread, which answers them one at a time with [`mcp::handle`], exactly as the
//! stdio server does. So time is still paused between calls and the sim stays deterministic for
//! the calls it gets, whoever sends them.
//!
//! **The transport** is the MCP "streamable HTTP" one, in its simplest allowed shape, on the
//! standard library alone (a small HTTP/1.1 server; no new dependencies):
//! - `POST /mcp` with one JSON-RPC message (or a batch): a request is answered with
//!   `application/json`, a notification or a response with `202 Accepted`. `initialize` also
//!   returns an `Mcp-Session-Id`; the server keeps nothing per session (the world is the state),
//!   so it never asks for it back and a restart doesn't strand anyone.
//! - `GET /mcp` is `405`: there is no server-to-client stream (the server never sends a message
//!   of its own). `DELETE /mcp` (a client ending its session) is `200`.
//! - `GET /health` says `ok`, with no authentication, for whatever watches the process.
//!
//! **Who gets in:** a bearer token (`--mcp-token T`, or better the `ZOMBYE_MCP_TOKEN` environment
//! variable, which `ps` doesn't show; at least 16 characters) in `Authorization: Bearer T`.
//! `--mcp-token-in-url` also takes it as `/mcp/T`, for apps that can only be given a URL (weaker:
//! URLs end up in logs). Without a token the server only starts with `--mcp-no-auth`, and then
//! without the debug tools and the saves folder ([`mcp::Options::PLAY_ONLY`]); `--mcp-no-dev` (or
//! `--no-debug-tools`) leaves them out with a token too.
//! A browser's `Origin` must be this machine's or one named with `--mcp-origin ORIGIN` (DNS
//! rebinding). There is no TLS here: put a reverse proxy or a tunnel in front for HTTPS.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use serde_json::{Value, json};
use zombye_net::shutdown;
use zombye_sim::World;

use crate::Persist;
use crate::mcp::{self, Options};
use crate::screenshot::Renderer;

/// The port when `--mcp-http` names none.
pub const DEFAULT_PORT: u16 = 8787;
/// The shortest token taken: anything shorter can be guessed over the network.
pub const MIN_TOKEN: usize = 16;
/// The largest request body (a tool call is a few hundred bytes).
const MAX_BODY: usize = 1 << 20;
/// The largest request line plus headers.
const MAX_HEAD: usize = 32 * 1024;
/// Messages in one batch.
const MAX_BATCH: usize = 50;
/// Connections at once; more are told to come back later.
const MAX_CONNECTIONS: usize = 64;
/// How long a connection may say nothing (also between requests on one kept open).
const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// A wrong token is answered this late, to slow guessing down.
const REFUSAL_DELAY: Duration = Duration::from_millis(250);

/// The `--mcp-http` options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpArgs {
    /// `ADDR:PORT` to listen on.
    pub addr: String,
    pub token: Option<String>,
    /// Take the token as `/mcp/TOKEN` too.
    pub token_in_url: bool,
    /// Browser origins allowed besides this machine's (`*`: any).
    pub origins: Vec<String>,
    /// The debug tools are on.
    pub dev: bool,
}

impl HttpArgs {
    /// From the command line and `ZOMBYE_MCP_TOKEN` (`env_token`).
    pub fn parse(args: &[String], env_token: Option<String>) -> Result<Self, String> {
        let flag = |name: &str| args.iter().any(|a| a == name);
        let text = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).filter(|v| !v.starts_with("--")).cloned();
        let addr = match text("--mcp-http") {
            None => format!("127.0.0.1:{DEFAULT_PORT}"),
            Some(v) if v.parse::<u16>().is_ok() => format!("127.0.0.1:{v}"),
            Some(v) if v.rsplit_once(':').is_some_and(|(host, port)| port.parse::<u16>().is_ok() && (!host.contains(':') || host.ends_with(']'))) => v,
            // A bare IPv6 address.
            Some(v) if v.contains(':') && !v.starts_with('[') => format!("[{v}]:{DEFAULT_PORT}"),
            Some(v) => format!("{v}:{DEFAULT_PORT}"),
        };
        let token = text("--mcp-token").or(env_token).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let no_auth = flag("--mcp-no-auth");
        match &token {
            Some(_) if no_auth => return Err("--mcp-no-auth with a token: pick one".into()),
            Some(t) if t.len() < MIN_TOKEN => return Err(format!("the MCP token must be at least {MIN_TOKEN} characters")),
            Some(t) if !t.bytes().all(|b| b.is_ascii_graphic()) => return Err("the MCP token must be printable ASCII with no spaces".into()),
            None if !no_auth => {
                return Err("--mcp-http needs a token: set ZOMBYE_MCP_TOKEN (or pass --mcp-token T), which callers send as `Authorization: Bearer T`. \
                            --mcp-no-auth starts it open to anyone who can reach it, without the debug tools"
                    .into());
            }
            _ => {}
        }
        let origins = args.iter().enumerate().filter(|(_, a)| *a == "--mcp-origin").filter_map(|(i, _)| args.get(i + 1)).map(|o| o.trim_end_matches('/').to_string()).collect();
        // An open server never has the debug tools: they are the whole world.
        // `--no-debug-tools` is the stdio server's flag for the same thing.
        let no_dev = flag("--mcp-no-dev") || flag("--no-debug-tools");
        Ok(HttpArgs { addr, dev: token.is_some() && !no_dev, token_in_url: token.is_some() && flag("--mcp-token-in-url"), token, origins })
    }

    /// Which tools callers get: all of them, or (`dev` off) neither the debug tools nor the saves folder.
    pub fn options(&self) -> Options {
        if self.dev { Options::default() } else { Options::PLAY_ONLY }
    }
}

/// One JSON-RPC message for the main thread, and where its answer goes (`None`: a notification).
pub struct Job {
    pub msg: Value,
    pub reply: Sender<Option<Value>>,
}

struct Shared {
    token: Option<String>,
    token_in_url: bool,
    origins: Vec<String>,
    active: AtomicUsize,
}

/// Start listening: connections are served on their own threads and their messages arrive as
/// [`Job`]s. Returns the address actually bound (port 0 picks one).
pub fn listen(args: &HttpArgs) -> Result<(SocketAddr, Receiver<Job>), String> {
    let listener = TcpListener::bind(&args.addr).map_err(|e| format!("can't listen on {}: {e}", args.addr))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel::<Job>();
    let shared = Arc::new(Shared { token: args.token.clone(), token_in_url: args.token_in_url, origins: args.origins.clone(), active: AtomicUsize::new(0) });
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                // Out of file descriptors, say: don't spin.
                std::thread::sleep(Duration::from_millis(50));
                continue;
            };
            let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
            let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
            let _ = stream.set_nodelay(true);
            if shared.active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                shared.active.fetch_sub(1, Ordering::SeqCst);
                let _ = Response::text(503, "too many connections").with("Retry-After", "2").write(&mut stream, true);
                continue;
            }
            let (shared, tx) = (shared.clone(), tx.clone());
            std::thread::spawn(move || {
                // Counted down however the thread ends.
                struct Open(Arc<Shared>);
                impl Drop for Open {
                    fn drop(&mut self) {
                        self.0.active.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                let open = Open(shared);
                connection(stream, &open.0, &tx);
            });
        }
    });
    Ok((addr, rx))
}

/// `zombye-server --mcp-http`: serve `world` until Ctrl-C or SIGTERM, autosaving as `--mcp` does.
pub fn serve(mut world: World, persist: &mut Persist, renderer: &Renderer, args: &HttpArgs) {
    mcp::add_agent_player(&mut world);
    let (addr, jobs) = match listen(args) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "MCP over HTTP on http://{addr}/mcp ({}; debug tools {}). No TLS here: put a reverse proxy or a tunnel in front for HTTPS.",
        match (&args.token, args.token_in_url) {
            (Some(_), true) => "bearer token, also taken as /mcp/TOKEN",
            (Some(_), false) => "bearer token",
            (None, _) => "NO AUTHENTICATION: anyone who can reach it plays",
        },
        if args.dev { "on" } else { "off" },
    );
    let opts = args.options();
    let why = loop {
        match jobs.recv_timeout(Duration::from_millis(250)) {
            Ok(job) => {
                let _ = job.reply.send(mcp::handle(&mut world, persist, renderer, opts, &job.msg));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break "listener stopped",
        }
        if shutdown::requested() {
            break "interrupted";
        }
        if persist.autosave_due() {
            persist.save_now(&world, "autosave");
        }
    };
    if persist.dirty {
        persist.save_now(&world, why);
    }
    if let Some(run) = persist.benchmark.as_mut() {
        run.close(&world);
    }
    shutdown::finished();
}

struct Request {
    method: String,
    /// The path and query as sent.
    target: String,
    /// Names in lower case.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// The client wants the connection closed after this one.
    close: bool,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    fn path(&self) -> &str {
        self.target.split(['?', '#']).next().unwrap_or("")
    }
}

/// Why a request couldn't be read.
enum Bad {
    /// The connection ended or timed out: nothing to say.
    Closed,
    Status(u16, &'static str),
}

struct Response {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Response {
    fn empty(status: u16) -> Self {
        Response { status, headers: Vec::new(), body: Vec::new() }
    }

    fn text(status: u16, text: &str) -> Self {
        Response { status, headers: vec![("Content-Type", "text/plain; charset=utf-8".into())], body: format!("{text}\n").into_bytes() }
    }

    fn json(status: u16, value: &Value) -> Self {
        Response { status, headers: vec![("Content-Type", "application/json".into())], body: value.to_string().into_bytes() }
    }

    /// A JSON-RPC error with no request to pin it on.
    fn rpc_error(status: u16, code: i32, message: &str) -> Self {
        Response::json(status, &json!({"jsonrpc": "2.0", "id": null, "error": {"code": code, "message": message}}))
    }

    fn with(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_string()));
        self
    }

    fn write(&self, stream: &mut TcpStream, close: bool) -> std::io::Result<()> {
        let reason = match self.status {
            200 => "OK",
            202 => "Accepted",
            204 => "No Content",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            413 => "Content Too Large",
            431 => "Request Header Fields Too Large",
            503 => "Service Unavailable",
            _ => "Error",
        };
        let mut head = format!("HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: {}\r\n", self.status, self.body.len(), if close { "close" } else { "keep-alive" });
        for (name, value) in &self.headers {
            // Values come from our own code or from a checked Origin; never let one split the head.
            head.push_str(&format!("{name}: {}\r\n", value.replace(['\r', '\n'], " ")));
        }
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        out.extend_from_slice(&self.body);
        stream.write_all(&out)?;
        stream.flush()
    }
}

/// One line, without its line end; `None` at the end of the stream. `left` is the bytes the
/// caller will still take.
fn read_line(r: &mut impl BufRead, left: &mut usize) -> Result<Option<String>, Bad> {
    let mut buf = Vec::new();
    let n = r.take(*left as u64 + 1).read_until(b'\n', &mut buf).map_err(|_| Bad::Closed)?;
    if n == 0 {
        return Ok(None);
    }
    if !buf.ends_with(b"\n") {
        return Err(if n > *left { Bad::Status(431, "request head too large") } else { Bad::Closed });
    }
    *left = left.saturating_sub(n);
    buf.pop();
    if buf.ends_with(b"\r") {
        buf.pop();
    }
    String::from_utf8(buf).map(Some).map_err(|_| Bad::Status(400, "the request head isn't UTF-8"))
}

fn read_exact(r: &mut impl Read, n: usize) -> Result<Vec<u8>, Bad> {
    let mut buf = vec![0; n];
    r.read_exact(&mut buf).map_err(|_| Bad::Closed)?;
    Ok(buf)
}

/// A `Transfer-Encoding: chunked` body.
fn read_chunked(r: &mut impl BufRead) -> Result<Vec<u8>, Bad> {
    let mut body = Vec::new();
    loop {
        let line = read_line(r, &mut 1024)?.ok_or(Bad::Closed)?;
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).map_err(|_| Bad::Status(400, "bad chunk size"))?;
        if size > MAX_BODY - body.len() {
            return Err(Bad::Status(413, "request body too large"));
        }
        if size == 0 {
            // Trailers, to the blank line.
            let mut left = 4096;
            while !read_line(r, &mut left)?.ok_or(Bad::Closed)?.is_empty() {}
            return Ok(body);
        }
        body.extend(read_exact(r, size)?);
        if !read_line(r, &mut 2)?.ok_or(Bad::Closed)?.is_empty() {
            return Err(Bad::Status(400, "bad chunk"));
        }
    }
}

/// The next request on a connection; `None` when the client has closed it.
fn read_request(r: &mut BufReader<TcpStream>) -> Result<Option<Request>, Bad> {
    let mut left = MAX_HEAD;
    let line = loop {
        match read_line(r, &mut left)? {
            None => return Ok(None),
            // A stray blank line between requests.
            Some(l) if l.is_empty() => continue,
            Some(l) => break l,
        }
    };
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(Bad::Status(400, "bad request line"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(Bad::Status(400, "HTTP/1.1 only"));
    }
    let mut headers = Vec::new();
    loop {
        let line = read_line(r, &mut left)?.ok_or(Bad::Closed)?;
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').ok_or(Bad::Status(400, "bad header"))?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let header = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
    let has = |name: &str, token: &str| header(name).is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)));
    let close = has("connection", "close") || (version == "HTTP/1.0" && !has("connection", "keep-alive"));
    let chunked = has("transfer-encoding", "chunked");
    let length = match header("content-length") {
        None => 0,
        Some(v) => v.parse::<usize>().map_err(|_| Bad::Status(400, "bad Content-Length"))?,
    };
    if length > MAX_BODY {
        return Err(Bad::Status(413, "request body too large"));
    }
    if (chunked || length > 0) && has("expect", "100-continue") {
        r.get_mut().write_all(b"HTTP/1.1 100 Continue\r\n\r\n").map_err(|_| Bad::Closed)?;
    }
    let body = if chunked { read_chunked(r)? } else { read_exact(r, length)? };
    Ok(Some(Request { method: method.to_string(), target: target.to_string(), headers, body, close }))
}

/// Serve one connection until the client closes it, goes quiet or sends something unreadable.
fn connection(stream: TcpStream, shared: &Shared, jobs: &Sender<Job>) {
    let Ok(mut out) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    loop {
        match read_request(&mut reader) {
            Ok(Some(request)) => {
                let response = respond(&request, shared, jobs);
                if response.write(&mut out, request.close).is_err() || request.close {
                    return;
                }
            }
            Ok(None) | Err(Bad::Closed) => return,
            Err(Bad::Status(status, text)) => {
                // The rest of what was sent is unread, so the connection can't be used again.
                // Closing with unread bytes would reset it and could lose the answer on its way:
                // say we're done, then take (and drop) a little more before closing.
                let _ = Response::text(status, text).write(&mut out, true);
                let _ = out.shutdown(std::net::Shutdown::Write);
                let _ = out.set_read_timeout(Some(Duration::from_millis(500)));
                let _ = std::io::copy(&mut reader.take(4 * MAX_HEAD as u64), &mut std::io::sink());
                return;
            }
        }
    }
}

/// Equal, in time that doesn't depend on where they differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// An origin on this machine (`http://localhost:6274`): a page the person at the keyboard opened
/// from their own computer, such as the MCP inspector.
fn local_origin(origin: &str) -> bool {
    let Some(rest) = origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://")) else { return false };
    let (host, port) = match rest.strip_prefix('[') {
        Some(v6) => match v6.split_once(']') {
            Some((host, "")) => (host, None),
            Some((host, after)) => (host, Some(after.strip_prefix(':').unwrap_or("?"))),
            None => return false,
        },
        None => match rest.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        },
    };
    port.is_none_or(|p| p.parse::<u16>().is_ok()) && (host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1")
}

impl Shared {
    fn origin_ok(&self, origin: &str) -> bool {
        let origin = origin.trim_end_matches('/');
        self.origins.iter().any(|o| o == "*" || o.eq_ignore_ascii_case(origin)) || local_origin(origin)
    }

    fn authorised(&self, request: &Request) -> bool {
        let Some(token) = &self.token else { return true };
        let bearer = request.header("authorization").and_then(|h| h.split_once(' ')).filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"));
        bearer.is_some_and(|(_, sent)| same(sent.trim(), token)) || (self.token_in_url && request.path().strip_prefix("/mcp/").is_some_and(|sent| same(sent, token)))
    }
}

/// A new session id: 128 bits from the standard library's per-process random hash keys.
fn session_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let part = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(COUNT.fetch_add(1, Ordering::Relaxed));
        h.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}

/// Pass one message to the main thread and wait for its answer. `Err`: the server is stopping.
fn dispatch(msg: Value, jobs: &Sender<Job>) -> Result<Option<Value>, ()> {
    // A response to something we never asked, or anything else with no method: nothing to answer.
    if !msg.get("method").is_some_and(Value::is_string) {
        return Ok(None);
    }
    let (reply, answer) = mpsc::channel();
    jobs.send(Job { msg, reply }).map_err(|_| ())?;
    answer.recv().map_err(|_| ())
}

fn respond(request: &Request, shared: &Shared, jobs: &Sender<Job>) -> Response {
    let path = request.path();
    if path == "/health" && matches!(request.method.as_str(), "GET" | "HEAD") {
        return Response::text(200, "ok");
    }
    if !(path == "/mcp" || path == "/mcp/" || (shared.token_in_url && path.starts_with("/mcp/"))) {
        return Response::text(404, "the MCP endpoint is /mcp");
    }
    // A browser says where the page calling us came from; other clients send no Origin.
    let origin = request.header("origin");
    if origin.is_some_and(|o| !shared.origin_ok(o)) {
        return Response::text(403, "this origin isn't allowed (--mcp-origin ORIGIN allows one)");
    }
    let cors = |r: Response| match origin {
        Some(o) => r.with("Access-Control-Allow-Origin", o).with("Vary", "Origin").with("Access-Control-Expose-Headers", "Mcp-Session-Id"),
        None => r,
    };
    if request.method == "OPTIONS" {
        // A browser's preflight carries no credentials.
        return cors(Response::empty(204))
            .with("Access-Control-Allow-Methods", "POST, GET, DELETE, OPTIONS")
            .with("Access-Control-Allow-Headers", "Authorization, Content-Type, Accept, Mcp-Session-Id, Mcp-Protocol-Version, Last-Event-ID")
            .with("Access-Control-Max-Age", "600");
    }
    if !shared.authorised(request) {
        std::thread::sleep(REFUSAL_DELAY);
        return cors(Response::text(401, "a bearer token is needed: Authorization: Bearer TOKEN")).with("WWW-Authenticate", "Bearer realm=\"zombye\"");
    }
    cors(match request.method.as_str() {
        "POST" => post(request, jobs),
        // A client ending its session: there is nothing kept to end.
        "DELETE" => Response::empty(200),
        // No server-to-client stream: the spec's answer for that is 405.
        _ => Response::text(405, "POST JSON-RPC messages here").with("Allow", "POST, DELETE, OPTIONS"),
    })
}

fn post(request: &Request, jobs: &Sender<Job>) -> Response {
    let stopping = || Response::text(503, "the server is stopping");
    let Ok(msg) = serde_json::from_slice::<Value>(&request.body) else {
        return Response::rpc_error(400, -32700, "the body isn't JSON");
    };
    match msg {
        Value::Array(batch) if !batch.is_empty() && batch.len() <= MAX_BATCH => {
            let mut answers = Vec::new();
            for msg in batch {
                match dispatch(msg, jobs) {
                    Ok(Some(answer)) => answers.push(answer),
                    Ok(None) => {}
                    Err(()) => return stopping(),
                }
            }
            if answers.is_empty() { Response::empty(202) } else { Response::json(200, &Value::Array(answers)) }
        }
        Value::Object(_) => {
            let initialize = msg["method"] == "initialize";
            match dispatch(msg, jobs) {
                Ok(Some(answer)) if initialize => Response::json(200, &answer).with("Mcp-Session-Id", &session_id()),
                Ok(Some(answer)) => Response::json(200, &answer),
                Ok(None) => Response::empty(202),
                Err(()) => stopping(),
            }
        }
        _ => Response::rpc_error(400, -32600, "a JSON-RPC message, or a batch of 1 to 50, is needed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use zombye_net::content;

    const TOKEN: &str = "correct-horse-battery";

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    fn args(extra: &[&str]) -> HttpArgs {
        let mut all = vec!["--mcp-http", "127.0.0.1:0", "--mcp-token", TOKEN];
        all.extend(extra);
        HttpArgs::parse(&strings(&all), None).unwrap()
    }

    /// A listener whose messages are answered by a stand-in for the game: `{"echo": method}`.
    fn echo_server(args: &HttpArgs) -> SocketAddr {
        let (addr, jobs) = listen(args).unwrap();
        std::thread::spawn(move || {
            for job in jobs {
                let answer = job.msg.get("id").filter(|id| !id.is_null()).map(|id| json!({"jsonrpc": "2.0", "id": id, "result": {"echo": job.msg["method"]}}));
                let _ = job.reply.send(answer);
            }
        });
        addr
    }

    struct Reply {
        status: u16,
        head: String,
        body: String,
    }

    impl Reply {
        fn header(&self, name: &str) -> Option<&str> {
            self.head.lines().filter_map(|l| l.split_once(':')).find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim())
        }

        fn json(&self) -> Value {
            serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
        }
    }

    /// Read one response off a connection that stays open.
    fn read_reply(r: &mut impl BufRead) -> Reply {
        let mut head = String::new();
        loop {
            let mut line = String::new();
            assert!(r.read_line(&mut line).unwrap() > 0, "connection closed after {head:?}");
            if line == "\r\n" {
                break;
            }
            head.push_str(&line);
        }
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        let length: usize = head.lines().filter_map(|l| l.split_once(':')).find(|(n, _)| n.eq_ignore_ascii_case("content-length")).map_or(0, |(_, v)| v.trim().parse().unwrap());
        let mut body = vec![0; length];
        r.read_exact(&mut body).unwrap();
        Reply { status, head, body: String::from_utf8(body).unwrap() }
    }

    /// Send raw bytes on a new connection and read the one response.
    fn send(addr: SocketAddr, raw: &str) -> Reply {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        read_reply(&mut BufReader::new(stream))
    }

    fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> String {
        let mut raw = format!("{method} {path} HTTP/1.1\r\nHost: test\r\nContent-Length: {}\r\n", body.len());
        for (name, value) in headers {
            raw.push_str(&format!("{name}: {value}\r\n"));
        }
        raw + "\r\n" + body
    }

    fn bearer() -> (&'static str, String) {
        ("Authorization", format!("Bearer {TOKEN}"))
    }

    fn post_json(addr: SocketAddr, body: &Value) -> Reply {
        let auth = bearer();
        send(addr, &request("POST", "/mcp", &[(auth.0, &auth.1), ("Content-Type", "application/json"), ("Accept", "application/json, text/event-stream")], &body.to_string()))
    }

    #[test]
    fn the_address_defaults_to_this_machine() {
        let parse = |a: &[&str]| HttpArgs::parse(&strings(a), Some(TOKEN.into())).unwrap().addr;
        assert_eq!(parse(&["--mcp-http"]), "127.0.0.1:8787");
        assert_eq!(parse(&["--mcp-http", "--seed", "3"]), "127.0.0.1:8787");
        assert_eq!(parse(&["--mcp-http", "9000"]), "127.0.0.1:9000");
        assert_eq!(parse(&["--mcp-http", "0.0.0.0"]), "0.0.0.0:8787");
        assert_eq!(parse(&["--mcp-http", "0.0.0.0:9000"]), "0.0.0.0:9000");
        assert_eq!(parse(&["--mcp-http", "::1"]), "[::1]:8787");
        assert_eq!(parse(&["--mcp-http", "[::]:9000"]), "[::]:9000");
    }

    /// No token, no server, unless it is opened on purpose, and then without the debug tools.
    #[test]
    fn a_token_is_needed_unless_opened_on_purpose() {
        let parse = |a: &[&str], env: Option<&str>| HttpArgs::parse(&strings(a), env.map(str::to_string));
        assert!(parse(&["--mcp-http"], None).unwrap_err().contains("ZOMBYE_MCP_TOKEN"));
        assert!(parse(&["--mcp-http"], Some("  ")).is_err());
        assert!(parse(&["--mcp-http", "--mcp-token", "short"], None).unwrap_err().contains("16"));
        assert!(parse(&["--mcp-http", "--mcp-token", "has a space in it, long"], None).is_err());
        assert!(parse(&["--mcp-http", "--mcp-no-auth"], Some(TOKEN)).is_err());

        let with = parse(&["--mcp-http"], Some(TOKEN)).unwrap();
        assert_eq!((with.token.as_deref(), with.dev, with.token_in_url), (Some(TOKEN), true, false));
        // The command line's wins over the environment's.
        let both = parse(&["--mcp-http", "--mcp-token", "another-long-enough-token"], Some(TOKEN)).unwrap();
        assert_eq!(both.token.as_deref(), Some("another-long-enough-token"));
        assert!(!parse(&["--mcp-http", "--mcp-no-dev"], Some(TOKEN)).unwrap().dev);
        assert!(parse(&["--mcp-http", "--mcp-token-in-url"], Some(TOKEN)).unwrap().token_in_url);

        let open = parse(&["--mcp-http", "--mcp-no-auth", "--mcp-token-in-url"], None).unwrap();
        assert_eq!((open.token.as_deref(), open.dev, open.token_in_url), (None, false, false));
        assert!(!open.options().debug_tools && !open.options().save_tools);
        assert!(!parse(&["--mcp-http", "--no-debug-tools"], Some(TOKEN)).unwrap().dev);

        let origins = parse(&["--mcp-http", "--mcp-origin", "https://a.example/", "--mcp-origin", "https://b.example"], Some(TOKEN)).unwrap().origins;
        assert_eq!(origins, ["https://a.example", "https://b.example"]);
    }

    #[test]
    fn only_the_right_token_gets_in() {
        let addr = echo_server(&args(&[]));
        let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string();
        let none = send(addr, &request("POST", "/mcp", &[], &ping));
        assert_eq!(none.status, 401);
        assert!(none.header("WWW-Authenticate").unwrap().starts_with("Bearer"));
        assert!(!none.body.contains("echo"));
        for wrong in ["Bearer wrong-horse-battery!", "Bearer correct-horse-batter", "Bearer", "Basic Y29ycmVjdC1ob3JzZS1iYXR0ZXJ5", &format!("Bearer {TOKEN}x")] {
            let started = Instant::now();
            assert_eq!(send(addr, &request("POST", "/mcp", &[("Authorization", wrong)], &ping)).status, 401, "{wrong}");
            assert!(started.elapsed() >= REFUSAL_DELAY, "a refusal isn't instant");
        }
        // The token in the URL is nothing unless asked for.
        assert_eq!(send(addr, &request("POST", &format!("/mcp/{TOKEN}"), &[], &ping)).status, 404);
        assert_eq!(send(addr, &request("POST", &format!("/mcp?token={TOKEN}"), &[], &ping)).status, 401);

        let ok = post_json(addr, &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}));
        assert_eq!((ok.status, ok.header("Content-Type")), (200, Some("application/json")));
        assert_eq!(ok.json(), json!({"jsonrpc": "2.0", "id": 1, "result": {"echo": "ping"}}));
        // The scheme's case doesn't matter.
        assert_eq!(send(addr, &request("POST", "/mcp", &[("authorization", &format!("bearer {TOKEN}"))], &ping)).status, 200);
        // The health check needs none, and says nothing about the game.
        let health = send(addr, &request("GET", "/health", &[], ""));
        assert_eq!((health.status, health.body.as_str()), (200, "ok\n"));
        assert_eq!(send(addr, &request("GET", "/", &[], "")).status, 404);
    }

    #[test]
    fn the_token_in_the_url_works_when_asked_for() {
        let addr = echo_server(&args(&["--mcp-token-in-url"]));
        let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string();
        assert_eq!(send(addr, &request("POST", &format!("/mcp/{TOKEN}"), &[], &ping)).status, 200);
        assert_eq!(send(addr, &request("POST", "/mcp/not-the-token-at-all!", &[], &ping)).status, 401);
        assert_eq!(send(addr, &request("POST", "/mcp", &[], &ping)).status, 401);
        let auth = bearer();
        assert_eq!(send(addr, &request("POST", "/mcp", &[(auth.0, &auth.1)], &ping)).status, 200);
    }

    #[test]
    fn an_open_server_takes_anyone() {
        let addr = echo_server(&HttpArgs::parse(&strings(&["--mcp-http", "127.0.0.1:0", "--mcp-no-auth"]), None).unwrap());
        assert_eq!(send(addr, &request("POST", "/mcp", &[], &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string())).status, 200);
    }

    /// The streamable HTTP rules: requests get JSON, notifications and responses 202, a batch an
    /// array, `initialize` a session id, GET no stream.
    #[test]
    fn messages_are_answered_as_the_transport_says() {
        let addr = echo_server(&args(&[]));
        let init = post_json(addr, &json!({"jsonrpc": "2.0", "id": "a", "method": "initialize", "params": {}}));
        assert_eq!(init.status, 200);
        let session = init.header("Mcp-Session-Id").unwrap().to_string();
        assert_eq!(session.len(), 32);
        assert!(session.bytes().all(|b| b.is_ascii_hexdigit()));
        let again = post_json(addr, &json!({"jsonrpc": "2.0", "id": "a", "method": "initialize", "params": {}}));
        assert_ne!(again.header("Mcp-Session-Id"), Some(session.as_str()));
        assert_eq!(post_json(addr, &json!({"jsonrpc": "2.0", "id": 2, "method": "ping"})).header("Mcp-Session-Id"), None);

        let note = post_json(addr, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        assert_eq!((note.status, note.body.as_str()), (202, ""));
        let response = post_json(addr, &json!({"jsonrpc": "2.0", "id": 9, "result": {}}));
        assert_eq!(response.status, 202);

        let batch = post_json(addr, &json!([{"jsonrpc": "2.0", "id": 1, "method": "ping"}, {"jsonrpc": "2.0", "method": "notifications/x"}, {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}]));
        assert_eq!(batch.json(), json!([{"jsonrpc": "2.0", "id": 1, "result": {"echo": "ping"}}, {"jsonrpc": "2.0", "id": 2, "result": {"echo": "tools/list"}}]));
        assert_eq!(post_json(addr, &json!([{"jsonrpc": "2.0", "method": "notifications/x"}])).status, 202);
        assert_eq!(post_json(addr, &json!([])).status, 400);
        assert_eq!(post_json(addr, &json!("hello")).json()["error"]["code"], -32600);

        let auth = bearer();
        let headers = [(auth.0, auth.1.as_str()), ("Mcp-Session-Id", session.as_str())];
        let bad = send(addr, &request("POST", "/mcp", &headers, "{not json"));
        assert_eq!((bad.status, bad.json()["error"]["code"].as_i64()), (400, Some(-32700)));
        let get = send(addr, &request("GET", "/mcp", &headers, ""));
        assert_eq!((get.status, get.header("Allow")), (405, Some("POST, DELETE, OPTIONS")));
        assert_eq!(send(addr, &request("DELETE", "/mcp", &headers, "")).status, 200);
    }

    #[test]
    fn several_requests_share_a_connection_until_asked_to_close() {
        let addr = echo_server(&args(&[]));
        let auth = bearer();
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        for id in 1..=3 {
            let body = json!({"jsonrpc": "2.0", "id": id, "method": "ping"}).to_string();
            stream.write_all(request("POST", "/mcp", &[(auth.0, &auth.1)], &body).as_bytes()).unwrap();
            let reply = read_reply(&mut reader);
            assert_eq!((reply.status, reply.header("Connection")), (200, Some("keep-alive")));
            assert_eq!(reply.json()["id"], id);
        }
        // A chunked body, with `Expect: 100-continue` answered first.
        let body = json!({"jsonrpc": "2.0", "id": 4, "method": "ping"}).to_string();
        let (a, b) = body.split_at(10);
        let chunked = format!(
            "POST /mcp HTTP/1.1\r\nHost: test\r\nAuthorization: Bearer {TOKEN}\r\nTransfer-Encoding: chunked\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n{:x}\r\n{a}\r\n{:x};ext=1\r\n{b}\r\n0\r\nTrailer: x\r\n\r\n",
            a.len(),
            b.len()
        );
        stream.write_all(chunked.as_bytes()).unwrap();
        let mut interim = String::new();
        reader.read_line(&mut interim).unwrap();
        assert_eq!(interim, "HTTP/1.1 100 Continue\r\n");
        reader.read_line(&mut interim).unwrap();
        let reply = read_reply(&mut reader);
        assert_eq!((reply.status, reply.header("Connection")), (200, Some("close")));
        assert_eq!(reply.json()["id"], 4);
        let mut rest = Vec::new();
        assert_eq!(reader.read_to_end(&mut rest).unwrap(), 0, "closed when asked to");
    }

    #[test]
    fn oversized_and_broken_requests_are_refused() {
        let addr = echo_server(&args(&[]));
        let auth = bearer();
        let big = format!("POST /mcp HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert_eq!(send(addr, &big).status, 413);
        let chunk = format!("POST /mcp HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n", MAX_BODY + 1);
        assert_eq!(send(addr, &chunk).status, 413);
        let long = format!("POST /mcp HTTP/1.1\r\nX-Padding: {}\r\n\r\n", "a".repeat(MAX_HEAD));
        assert_eq!(send(addr, &long).status, 431);
        assert_eq!(send(addr, "nonsense\r\n\r\n").status, 400);
        assert_eq!(send(addr, "POST /mcp HTTP/2\r\n\r\n").status, 400);
        assert_eq!(send(addr, &format!("POST /mcp HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: lots\r\n\r\n")).status, 400);
        // The server is still there for the next caller.
        assert_eq!(send(addr, &request("POST", "/mcp", &[(auth.0, &auth.1)], &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string())).status, 200);
    }

    /// A page on another site can't use the browser of someone who can reach the server.
    #[test]
    fn a_browser_origin_must_be_local_or_allowed() {
        let addr = echo_server(&args(&["--mcp-origin", "https://agent.example"]));
        let auth = bearer();
        let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string();
        let from = |origin: &str| send(addr, &request("POST", "/mcp", &[(auth.0, &auth.1), ("Origin", origin)], &ping));
        for origin in ["https://evil.example", "http://localhost.evil.example", "http://127.0.0.1.evil.example:80", "https://agent.example.evil.example", "null", "http://localhost:x@evil.example"] {
            assert_eq!(from(origin).status, 403, "{origin}");
        }
        for origin in ["http://localhost:6274", "http://127.0.0.1", "http://[::1]:5173", "https://agent.example"] {
            let reply = from(origin);
            assert_eq!((reply.status, reply.header("Access-Control-Allow-Origin")), (200, Some(origin)), "{origin}");
            assert_eq!(reply.header("Access-Control-Expose-Headers"), Some("Mcp-Session-Id"));
        }
        // The preflight comes without the token.
        let preflight = send(addr, &request("OPTIONS", "/mcp", &[("Origin", "https://agent.example"), ("Access-Control-Request-Method", "POST")], ""));
        assert_eq!(preflight.status, 204);
        assert!(preflight.header("Access-Control-Allow-Headers").unwrap().contains("Authorization"));
        assert_eq!(send(addr, &request("OPTIONS", "/mcp", &[("Origin", "https://evil.example")], "")).status, 403);
        // No Origin (every client that isn't a browser): no CORS headers.
        assert_eq!(post_json(addr, &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})).header("Access-Control-Allow-Origin"), None);

        let any = echo_server(&args(&["--mcp-origin", "*"]));
        assert_eq!(send(any, &request("POST", "/mcp", &[(auth.0, &auth.1), ("Origin", "https://anywhere.example")], &ping)).status, 200);
    }

    fn world_and_persist() -> (World, Persist, Renderer) {
        let data = content::default_data_dir();
        let mut config = content::load_config(&data, 7).unwrap().with_map_chunks(8);
        config.zombies = Some(30);
        let mut world = World::new(config);
        mcp::add_agent_player(&mut world);
        let persist = Persist { path: None, dir: std::env::temp_dir(), content: world.config.clone(), autosave: None, last_save: Instant::now(), dirty: false, benchmark: None };
        (world, persist, Renderer::new(None, data, Duration::from_secs(5)))
    }

    /// Answer the listener's messages with the real game until `client` is done, as `serve` does.
    fn play<T: Send + 'static>(args: &HttpArgs, client: impl FnOnce(SocketAddr) -> T + Send + 'static) -> (T, World, Persist) {
        let (mut world, mut persist, renderer) = world_and_persist();
        let (addr, jobs) = listen(args).unwrap();
        let client = std::thread::spawn(move || client(addr));
        while !client.is_finished() {
            if let Ok(job) = jobs.recv_timeout(Duration::from_millis(5)) {
                let _ = job.reply.send(mcp::handle(&mut world, &mut persist, &renderer, args.options(), &job.msg));
            }
        }
        (client.join().unwrap(), world, persist)
    }

    fn tool(addr: SocketAddr, id: u32, name: &str, arguments: Value) -> (String, bool) {
        let reply = post_json(addr, &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})).json();
        assert_eq!(reply["id"], id, "{reply}");
        (reply["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("{reply}")).to_string(), reply["result"]["isError"] == true)
    }

    /// The whole way through: a client over a real socket plays the one world the server holds,
    /// and a second connection sees what the first did.
    #[test]
    fn an_agent_plays_the_shared_world_over_http() {
        let (tick_seen, world, persist) = play(&args(&[]), |addr| {
            let init = post_json(addr, &json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}}));
            let result = &init.json()["result"];
            assert_eq!((result["protocolVersion"].as_str(), result["serverInfo"]["name"].as_str()), (Some("2025-06-18"), Some("zombye-server")));
            assert!(init.header("Mcp-Session-Id").is_some());
            assert_eq!(post_json(addr, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).status, 202);

            let tools = post_json(addr, &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).json();
            let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
            for name in ["observe", "step_ticks", "screenshot", "spawn_zombies", "dev", "load"] {
                assert!(names.contains(&name), "{name} in {names:?}");
            }

            let (map, err) = tool(addr, 2, "observe", json!({}));
            assert!(!err && map.contains('P'), "{map}");
            let (stepped, err) = tool(addr, 3, "step_ticks", json!({"n": 25}));
            assert!(!err, "{stepped}");
            let (spawned, err) = tool(addr, 4, "spawn_zombies", json!({"count": 5, "x": 64, "y": 64}));
            assert!(!err, "{spawned}");
            // Another caller, another connection: the same world, 25 ticks on.
            let (stats, err) = tool(addr, 5, "stats", json!({}));
            assert!(!err);
            serde_json::from_str::<Value>(&stats).unwrap()["tick"].as_u64().unwrap()
        });
        assert_eq!((tick_seen, world.tick), (25, 25));
        assert!(world.zombies.len() > 30, "{} zombies", world.zombies.len());
        assert!(persist.dirty, "stepping the world over HTTP makes it need saving");
    }

    /// `--mcp-no-dev` (and any open server): the debug tools aren't listed and can't be called;
    /// playing still works.
    #[test]
    fn a_server_without_debug_tools_refuses_them() {
        let no_dev = args(&["--mcp-no-dev"]);
        assert!(!no_dev.dev);
        let ((), world, _) = play(&no_dev, |addr| {
            let tools = post_json(addr, &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).json();
            let names: Vec<String> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
            for dev in DEV_TOOLS {
                assert!(!names.iter().any(|n| n == dev), "{dev} is listed");
            }
            for play in ["observe", "step_ticks", "move_player", "attack", "inventory", "craft", "sleep", "respawn", "screenshot", "stats"] {
                assert!(names.iter().any(|n| n == play), "{play} isn't listed");
            }
            let (stats, _) = tool(addr, 2, "stats", json!({}));
            let hash = serde_json::from_str::<Value>(&stats).unwrap()["state_hash"].clone();
            // A tool that isn't on offer is, to the caller, no tool at all: the unknown-tool error.
            for (id, dev) in DEV_TOOLS.iter().enumerate() {
                let arguments = json!({"count": 50, "x": 64, "y": 64, "god": true, "hours": 5, "name": "x", "item": "baseball_bat", "radius": 80, "on": true});
                let reply = post_json(addr, &json!({"jsonrpc": "2.0", "id": 10 + id, "method": "tools/call", "params": {"name": dev, "arguments": arguments}})).json();
                assert_eq!(reply["error"]["code"], -32602, "{dev}: {reply}");
                assert!(reply["error"]["message"].as_str().unwrap().contains("without the debug tools"), "{dev}: {reply}");
            }
            let (stats, _) = tool(addr, 3, "stats", json!({}));
            assert_eq!(serde_json::from_str::<Value>(&stats).unwrap()["state_hash"], hash, "a refused tool changes nothing");
            let (map, err) = tool(addr, 4, "observe", json!({}));
            assert!(!err && map.contains('P'));
        });
        assert_eq!((world.tick, world.zombies.len(), world.players.len()), (0, 30, 1));
    }

    /// What a server others play on leaves out: the debug tools (`mcp::Kind::Debug`) and the saves folder.
    const DEV_TOOLS: [&str; 15] = ["grid", "make_noise", "set_goal", "spawn_zombies", "set_needs", "spawn_item", "wound", "add_player", "pvp", "dev", "restrain_now", "skip_time", "save", "load", "saves"];

    /// Every tool is either left out of a public server or not: a new tool has to be put on one side here.
    #[test]
    fn every_debug_tool_exists() {
        let (mut world, mut persist, renderer) = world_and_persist();
        let mut names = |opts: Options| -> Vec<String> {
            let list = mcp::handle(&mut world, &mut persist, &renderer, opts, &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).unwrap();
            list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
        };
        let all = names(Options::default());
        for dev in DEV_TOOLS {
            assert!(all.iter().any(|n| n == dev), "{dev} isn't a tool");
            assert!(Options::default().offers_tool(dev) && !Options::PLAY_ONLY.offers_tool(dev), "{dev}");
        }
        let mut play = names(Options::PLAY_ONLY);
        assert_eq!(play, all.iter().filter(|n| !DEV_TOOLS.contains(&n.as_str())).cloned().collect::<Vec<_>>());
        play.sort();
        assert_eq!(
            play,
            [
                "attack", "blind", "build", "buildings", "builds", "climb", "containers_near", "craft", "drop", "edge", "equip", "fire", "fires", "free", "gesture", "group", "health", "heard", "interact", "inventory", "items", "light", "loot", "move_item", "move_player", "observe",
                "pick_up", "recipes", "regions", "respawn", "restrain", "say", "screenshot", "search", "search_player", "shove", "sit", "skills", "sleep", "smash_window", "stats", "step_ticks", "stomp", "struggle", "take", "take_from", "treat", "unbuild", "unequip",
                "use", "weapons", "wear", "zombie",
            ],
            "a new tool: list it here if players may use it on a public server; a debug tool (Kind::Debug in mcp::TOOLS) in DEV_TOOLS"
        );
    }
}
