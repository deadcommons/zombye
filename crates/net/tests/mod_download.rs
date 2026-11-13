//! Mods downloaded on joining (#24) against a real server on loopback: a client that hasn't a
//! server's mod asks for it by hash, takes only what hashes right, and keeps it; one that has it
//! (its own copy, or an earlier download) downloads nothing; and what goes wrong is said.

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use zombye_net::content;
use zombye_net::host::{Host, HostOptions};
use zombye_net::modpack::{self, Cache, ModPack, Source};
use zombye_net::remote;
use zombye_net::wire::{self, ClientMsg, ServerMsg};
use zombye_sim::World;

const WAIT: Duration = Duration::from_secs(10);

fn pack(id: &str, items: &str) -> ModPack {
    ModPack { id: id.into(), name: format!("The {id} mod"), version: "1.2".into(), description: "A test.".into(), files: vec![("items.ron".into(), items.into())] }
}

fn host(packs: Vec<ModPack>, password: &str) -> Host {
    let mut config = content::load_config(&content::default_data_dir(), 1).expect("the shipped content");
    config = config.with_map_chunks(8);
    config.zombies = Some(5);
    let mut options = HostOptions { password: password.into(), ..Default::default() };
    assert!(options.with_mods(packs).is_empty());
    Host::bind(World::new(config), "127.0.0.1:0", options).expect("a free port")
}

/// Keep `host` ticking on another thread while `work` runs; it comes back afterwards.
fn serving<T>(host: Host, work: impl FnOnce() -> T) -> (T, Host) {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let pump = std::thread::spawn(move || {
        let mut host = host;
        while !flag.load(Ordering::Relaxed) {
            host.tick();
            std::thread::sleep(Duration::from_millis(2));
        }
        host
    });
    let out = work();
    stop.store(true, Ordering::Relaxed);
    (out, pump.join().unwrap())
}

fn temp_cache(name: &str) -> Cache {
    let dir = std::env::temp_dir().join(format!("zombye-mod-download-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Cache::in_saves(&dir)
}

#[test]
fn a_client_downloads_what_it_lacks_and_keeps_it() {
    let (tins, big) = (pack("tins", "[]"), pack("big", &format!("// {}\n[]", "x".repeat(600_000))));
    let host = host(vec![tins.clone(), big.clone()], "");
    let addr = host.local_addr().unwrap().to_string();
    let cache = temp_cache("keeps");
    let (_, host) = serving(host, || {
        // The server names its mods by hash to whoever asks.
        let info = remote::query(&addr, WAIT).unwrap();
        assert_eq!(info.mods, [tins.info(), big.info()]);
        assert_eq!(info.mods[0].hash, tins.hash());
        assert!(remote::describe(&info, &addr).iter().any(|l| l.contains(&modpack::hex(&tins.hash()))));
        assert_eq!(modpack::to_download(&info.mods, &cache.listed()), (2, tins.size() + big.size()));

        // With nothing: both are downloaded, whole, in the server's order.
        let got = modpack::gather(&addr, "", &info.mods, &[], Some(&cache), WAIT).unwrap();
        assert_eq!(got.packs, [(tins.clone(), Source::Downloaded), (big.clone(), Source::Downloaded)]);
        assert_eq!(got.downloaded(), 2);
        assert!(got.lines()[0].starts_with("mod tins 1.2 (downloaded, "), "{:?}", got.lines());
        assert_eq!(modpack::to_download(&info.mods, &cache.listed()), (0, 0));

        // Again: from the cache.
        let again = modpack::gather(&addr, "", &info.mods, &[], Some(&cache), WAIT).unwrap();
        assert_eq!(again.packs, [(tins.clone(), Source::Cached), (big.clone(), Source::Cached)]);

        // With our own copy of one, an own mod the server doesn't run, and an own mod of the
        // same id but other data: ours where it is the same, the server's where it isn't.
        let (mine, other_big) = (pack("mine", "[]"), pack("big", "[ ]"));
        let mixed = modpack::gather(&addr, "", &info.mods, &[tins.clone(), mine, other_big], None, WAIT).unwrap();
        assert_eq!(mixed.packs, [(tins.clone(), Source::Own), (big.clone(), Source::Downloaded)]);
        assert_eq!(mixed.left_out, ["mine"]);
        assert!(mixed.lines().last().unwrap().contains("left out on this server"));

        // A server with no mods: nothing to get, ours are left out.
        let none = modpack::gather(&addr, "", &[], &[tins.clone()], None, WAIT).unwrap();
        assert!(none.packs.is_empty() && none.left_out == ["tins"]);
    });
    // Two downloads of two and one of one; the second gather asked for nothing.
    assert_eq!((host.stats.mods_sent, host.stats.queries), (3, 1));
    assert!(host.stats.mod_bytes > 1_200_000);
    assert!(host.log.iter().any(|l| l.starts_with("sent 2 mods (")), "{:?}", host.log);
    let _ = std::fs::remove_dir_all(cache.dir.parent().unwrap());
}

#[test]
fn only_those_who_may_join_get_the_mods() {
    let tins = pack("tins", "[]");
    let host = host(vec![tins.clone()], "sesame");
    let addr = host.local_addr().unwrap().to_string();
    serving(host, || {
        let info = remote::query(&addr, WAIT).unwrap();
        let e = remote::fetch_mods(&addr, "", &info.mods, WAIT).unwrap_err();
        assert!(e.contains("wrong password"), "{e}");
        assert_eq!(remote::fetch_mods(&addr, "sesame", &info.mods, WAIT).unwrap(), [tins.clone()]);
        // A hash the server doesn't have (it restarted with other mods, say).
        let other = pack("tins", "[ ]").info();
        let e = remote::fetch_mods(&addr, "sesame", &[other], WAIT).unwrap_err();
        assert!(e.contains("no mod with the hash"), "{e}");
        // Another version of the game.
        let mut stream = std::net::TcpStream::connect(&addr).unwrap();
        wire::write_frame(&mut stream, &ClientMsg::Fetch(wire::Fetch { protocol: wire::PROTOCOL + 1, password: "sesame".into(), mods: vec![tins.hash()] })).unwrap();
        assert!(matches!(wire::read_frame::<ServerMsg>(&mut stream, wire::MAX_SERVER_FRAME).unwrap().0, ServerMsg::Refused(why) if why.contains("protocol")));
        // And the players already on are not held up: one joins and plays through a download.
        let mut on = remote::Remote::connect(&addr, "ann", "sesame", WAIT).unwrap();
        let first = on.wait_snapshot(WAIT).unwrap();
        remote::fetch_mods(&addr, "sesame", &info.mods, WAIT).unwrap();
        assert!(on.wait_tick(first.tick + 5, WAIT).is_ok());
    });
}

/// A server that names one mod and sends another: the game takes nothing that doesn't hash to
/// what was named, whatever it is called.
#[test]
fn a_mod_that_isnt_what_was_named_is_not_taken() {
    let named = pack("tins", "[]");
    let cases: Vec<(ModPack, &str)> = vec![
        // Other data under the same name.
        (pack("tins", r#"[(id: "evil")]"#), "isn't the tins it named"),
        // A script smuggled in: not a data file, whatever the hash.
        (ModPack { files: vec![("run.luau".into(), "while true do end".into())], ..named.clone() }, "not a data file"),
        // A file that would leave the folder.
        (ModPack { files: vec![("../../settings.ron".into(), "()".into())], ..named.clone() }, "not a data file"),
        // Another mod altogether.
        (pack("other", "[]"), "isn't the tins it named"),
    ];
    for (sent, why) in cases {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let liar = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let asked = wire::read_frame::<ClientMsg>(&mut stream, wire::MAX_CLIENT_FRAME).unwrap().0;
            assert!(matches!(asked, ClientMsg::Fetch(_)));
            wire::write_frame(&mut stream, &ServerMsg::Mod(Box::new(sent))).unwrap();
        });
        let cache = temp_cache("liar");
        let e = modpack::gather(&addr, "", &[named.info()], &[], Some(&cache), WAIT).unwrap_err();
        assert!(e.contains(why), "{why}: {e}");
        assert!(cache.listed().is_empty(), "nothing is kept of it");
        liar.join().unwrap();
        let _ = std::fs::remove_dir_all(cache.dir.parent().unwrap());
    }
    // More than a join downloads unasked.
    let heavy: Vec<wire::ModInfo> = (0..20).map(|k| wire::ModInfo { id: format!("m{k}"), size: 2 << 20, hash: [k as u8; 32], ..Default::default() }).collect();
    assert!(modpack::gather("127.0.0.1:1", "", &heavy, &[], None, WAIT).unwrap_err().contains("at most 32.0 MB"));
    // And a list no server of ours would send.
    let twice = [named.info(), named.info()];
    assert!(modpack::gather("127.0.0.1:1", "", &twice, &[], None, WAIT).unwrap_err().contains("twice"));
    let bad = wire::ModInfo { id: "../x".into(), ..named.info() };
    assert!(modpack::gather("127.0.0.1:1", "", &[bad], &[], None, WAIT).unwrap_err().contains("id"));
}
