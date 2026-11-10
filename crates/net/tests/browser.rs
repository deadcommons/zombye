//! The server browser (#27) against real servers on loopback: a list from a file and from an
//! http address, each server asked for its rules, players, mods and ping, the featured section,
//! servers that don't answer, servers added by hand, and joining the one picked.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use zombye_net::browser::{self, Browser, ListState, Status};
use zombye_net::content;
use zombye_net::host::{Host, HostOptions};
use zombye_net::remote::Remote;
use zombye_net::wire::ModInfo;
use zombye_sim::World;

fn host(options: HostOptions) -> Host {
    let mut config = content::load_config(&content::default_data_dir(), 1).expect("the shipped content");
    config.zombies = Some(20);
    Host::bind(World::new(config), "127.0.0.1:0", options).expect("a free port")
}

/// Keep `hosts` answering on another thread while `work` runs; they come back afterwards.
fn serving<T>(hosts: Vec<Host>, work: impl FnOnce() -> T) -> (T, Vec<Host>) {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let pump = std::thread::spawn(move || {
        let mut hosts = hosts;
        while !flag.load(Ordering::Relaxed) {
            for h in &mut hosts {
                h.tick();
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        hosts
    });
    let out = work();
    stop.store(true, Ordering::Relaxed);
    (out, pump.join().unwrap())
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("zombye-browser-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A port nothing listens on.
fn dead_addr() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

#[test]
fn a_list_file_becomes_rows_with_players_ping_and_mods() {
    let official = host(HostOptions { name: "Official co-op".into(), max_players: 16, ..Default::default() });
    let modded = host(HostOptions { name: "Modded".into(), password: "secret".into(), mods: vec![ModInfo { id: "alarm_clock".into(), name: "Alarm clock".into(), version: "1.0".into(), ..Default::default() }], ..Default::default() });
    let (a, b, dead) = (official.local_addr().unwrap().to_string(), modded.local_addr().unwrap().to_string(), dead_addr());
    let dir = temp_dir("file");
    let file = dir.join("servers.ron");
    std::fs::write(&file, format!(r#"(servers: [(address: "{dead}", name: "Gone away"), (address: "{b}"), (address: "{a}", featured: true, note: "hosted by us")])"#)).unwrap();

    let (mut browser, hosts) = serving(vec![official, modded], || {
        // Someone is playing on the first.
        let on = Remote::connect(&a, "ann", "", Duration::from_secs(10)).expect("joins");
        let mut browser = Browser::open(file.to_str().unwrap(), &[]);
        assert_eq!(browser.list, ListState::Loading);
        assert!(browser.wait(Duration::from_secs(20)), "every server answered or was given up on");
        drop(on);
        browser
    });
    assert_eq!(browser.list, ListState::Ready);

    // Featured first; then those that answered, then the one that didn't.
    let shown: Vec<&str> = browser.shown().iter().map(|r| r.name()).collect();
    assert_eq!(shown, ["Official co-op", "Modded", "Gone away"]);
    assert_eq!(browser.featured().len(), 1);
    let row = browser.row(&a).unwrap();
    let Status::Online { info, ping_ms } = &row.status else { panic!("{:?}", row.status) };
    assert_eq!((info.online, info.rules.max_players, info.password, info.mods.len()), (1, 16, false, 0));
    assert!(*ping_ms < 1000, "loopback: {ping_ms} ms");
    assert_eq!(row.entry.note, "hosted by us");
    assert_eq!(browser::can_join(info, &[], false), Ok(()));

    let info = browser.row(&b).unwrap().info().unwrap();
    assert!(info.password);
    assert_eq!(browser::mods_text(&info.mods), "Alarm clock 1.0");
    // A mod we haven't got is no reason not to join: it is downloaded on the way in (#24).
    assert_eq!(browser::can_join(info, &[], false), Ok(()));

    let Status::Offline(why) = &browser.row(&dead).unwrap().status else { panic!("nothing listens there") };
    assert!(why.contains("can't connect"), "{why}");
    assert_eq!(browser.summary(), "3 servers, 2 up, 1 playing");

    // Asking never joined anyone: no survivor was made by the browser.
    assert_eq!(hosts[1].world.players.len(), 0);

    // With the servers gone, a refresh says so; the rows stay.
    drop(hosts);
    browser.refresh();
    assert!(browser.wait(Duration::from_secs(20)));
    assert!(browser.shown().iter().all(|r| matches!(r.status, Status::Offline(_))));
    assert_eq!(browser.shown().len(), 3);
    let _ = std::fs::remove_dir_all(dir);
}

/// A web server for one file: answers every request with `body` (or `status` and no body).
fn web(status: &'static str, body: String, extra: &'static str) -> (String, std::thread::JoinHandle<()>, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let serve = std::thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            let Ok((mut s, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            };
            s.set_nonblocking(false).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut buf) {
                    Ok(n) if n > 0 => request.extend_from_slice(&buf[..n]),
                    _ => break,
                }
            }
            let request = String::from_utf8_lossy(&request);
            assert!(request.starts_with("GET /lists/zombye.ron HTTP/1.0\r\n"), "{request}");
            assert!(request.contains("\r\nHost: 127.0.0.1:"), "{request}");
            let _ = s.write_all(format!("HTTP/1.0 {status}\r\nContent-Type: text/plain\r\n{extra}Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
        }
    });
    (addr, serve, stop)
}

#[test]
fn a_list_comes_from_an_http_address() {
    let server = host(HostOptions { name: "From the web".into(), ..Default::default() });
    let a = server.local_addr().unwrap().to_string();
    let (web_addr, web_thread, web_stop) = web("200 OK", format!(r#"(servers: [(address: "{a}", featured: true)])"#), "");
    let (browser, _hosts) = serving(vec![server], || {
        let mut browser = Browser::open(&format!("http://{web_addr}/lists/zombye.ron"), &[]);
        assert!(browser.wait(Duration::from_secs(20)));
        browser
    });
    web_stop.store(true, Ordering::Relaxed);
    web_thread.join().unwrap();
    assert_eq!(browser.list, ListState::Ready);
    assert_eq!(browser.featured().iter().map(|r| r.name()).collect::<Vec<_>>(), ["From the web"]);

    // What can go wrong with a list is said, and isn't fatal.
    for (status, body, extra, why) in [
        ("404 Not Found", String::new(), "", "said 404"),
        ("301 Moved Permanently", String::new(), "Location: https://elsewhere.example/list.ron\r\n", "https lists can't be read yet"),
        ("200 OK", "<html>not a list</html>".to_string(), "", "not a server list"),
    ] {
        let (web_addr, web_thread, web_stop) = web(status, body, extra);
        let mut browser = Browser::open(&format!("http://{web_addr}/lists/zombye.ron"), &[]);
        assert!(browser.wait(Duration::from_secs(20)));
        web_stop.store(true, Ordering::Relaxed);
        web_thread.join().unwrap();
        let ListState::Failed(e) = &browser.list else { panic!("{status}: {:?}", browser.list) };
        assert!(e.contains(why), "{status}: {e}");
        assert!(browser.shown().is_empty());
    }
    for (source, why) in [("https://lists.example.org/zombye.ron", "no TLS"), ("no/such/servers.ron", "can't read")] {
        let mut browser = Browser::open(source, &[]);
        assert!(browser.wait(Duration::from_secs(20)));
        let ListState::Failed(e) = &browser.list else { panic!("{source}") };
        assert!(e.contains(why), "{source}: {e}");
    }
}

#[test]
fn servers_are_added_by_hand_and_the_one_picked_is_joined() {
    let listed = host(HostOptions { name: "Listed".into(), ..Default::default() });
    let own = host(HostOptions { name: "A friend's".into(), password: "pass".into(), ..Default::default() });
    let (a, b) = (listed.local_addr().unwrap().to_string(), own.local_addr().unwrap().to_string());
    let dir = temp_dir("mine");
    let file = dir.join("servers.ron");
    std::fs::write(&file, format!(r#"(servers: [(address: "{a}")])"#)).unwrap();

    let ((), hosts) = serving(vec![listed, own], || {
        // No list at all: only what you add.
        let mut browser = Browser::open("", &[]);
        assert!(browser.settled() && browser.shown().is_empty());
        assert!(browser.add("not an address").is_err());
        assert_eq!(browser.add(&b).unwrap(), b);
        assert!(browser.wait(Duration::from_secs(20)));
        assert_eq!(browser.shown()[0].name(), "A friend's");
        assert_eq!(browser.mine(), [b.clone()]);

        // The next session starts with it, beside the list's; it can be taken off, the list's can't.
        let mut browser = Browser::open(file.to_str().unwrap(), &[b.clone(), b.clone()]);
        assert!(browser.wait(Duration::from_secs(20)));
        let mut names: Vec<&str> = browser.shown().iter().map(|r| r.name()).collect();
        names.sort();
        assert_eq!(names, ["A friend's", "Listed"]);
        assert!(browser.row(&b).unwrap().mine && !browser.row(&a).unwrap().mine);
        assert!(!browser.remove(&a));

        // Joining what the browser shows: the row's address, the password it said it needs.
        let row = browser.row(&b).unwrap();
        let info = row.info().unwrap();
        assert!(info.password && browser::can_join(info, &[], false).is_ok());
        let wrong = Remote::connect(&row.addr, "ann", "nope", Duration::from_secs(10));
        assert!(wrong.is_err_and(|e| e.contains("password")));
        let joined = Remote::connect(&row.addr, "ann", "pass", Duration::from_secs(10)).expect("joins with the password");
        assert_eq!(joined.welcome.rules.name, "A friend's");

        // Asked again, it has someone on.
        browser.refresh();
        assert!(browser.wait(Duration::from_secs(20)));
        assert_eq!(browser.row(&b).unwrap().info().unwrap().online, 1);
        assert!(browser.remove(&b) && browser.mine().is_empty() && browser.row(&b).is_none());
    });
    assert_eq!(hosts[1].world.players.len(), 1, "one survivor: the one who joined");
    let _ = std::fs::remove_dir_all(dir);
}
