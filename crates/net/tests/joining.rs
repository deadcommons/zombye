//! Every way a join is refused, as the join screen says it (DCM-70): each refusal a real host gives
//! (and a server that isn't there, and other game data) comes out of `joining::explain` with its
//! own title and a sentence, not the catch-all.

use std::time::Duration;

use zombye_net::host::{Ban, Host, HostOptions};
use zombye_net::identity::Identity;
use zombye_net::joining::{Server, explain};
use zombye_net::remote::{self, Remote};
use zombye_net::wire::{Agents, Camera};
use zombye_net::content;
use zombye_sim::World;

fn host(options: HostOptions) -> Host {
    let mut config = content::load_config(&content::default_data_dir(), 1).expect("the shipped content");
    config.zombies = Some(20);
    Host::bind(World::new(config), "127.0.0.1:0", options).expect("a free port")
}

fn with_host<T: Send + 'static>(host: &mut Host, work: impl FnOnce(String) -> T + Send + 'static) -> T {
    let addr = host.local_addr().unwrap().to_string();
    let client = std::thread::spawn(move || work(addr));
    while !client.is_finished() {
        host.pump();
        std::thread::sleep(Duration::from_millis(1));
    }
    client.join().unwrap()
}

/// The title the join screen shows for joining `host` as `name` (with `key`'s identity).
fn refused(host: &mut Host, name: &str, password: &str, agent: bool, key: &str) -> String {
    let (name, password, key) = (name.to_string(), password.to_string(), key.to_string());
    let addr = host.local_addr().unwrap().to_string();
    let raw = with_host(host, move |addr| Remote::connect_with(&addr, &name, &password, agent, &Identity::for_name(&key), Duration::from_secs(10)).map(|_| ()).expect_err("refused"));
    let p = explain(&raw, &Server { name: "Test", addr: &addr, version: Some(zombye_net::BUILD) });
    assert!(p.title != "Can't join", "{raw} has no words of its own: {}", p.text);
    assert!(p.text.ends_with('.'), "{}", p.text);
    p.title
}

#[test]
fn every_refusal_has_its_own_words() {
    let mut h = host(HostOptions { password: "sesame".into(), max_players: 2, agents: Agents::Refused, ..Default::default() });
    assert_eq!(refused(&mut h, "ada", "nope", false, "ada"), "Wrong password");
    assert_eq!(refused(&mut h, "bot", "sesame", true, "bot"), "No agents");

    // Ada joins; her name and her key are then taken.
    let ada = with_host(&mut h, |addr| Remote::connect(&addr, "ada", "sesame", Duration::from_secs(10))).expect("ada joins");
    assert_eq!(refused(&mut h, "ada", "sesame", false, "someone else"), "Name taken");
    assert_eq!(refused(&mut h, "ada two", "sesame", false, "ada"), "Already playing");

    // Full with two on.
    let _bob = with_host(&mut h, |addr| Remote::connect(&addr, "bob", "sesame", Duration::from_secs(10))).expect("bob joins");
    assert_eq!(refused(&mut h, "cy", "sesame", false, "cy"), "Server full");
    drop(ada);
    for _ in 0..5 {
        h.tick();
    }

    // Banned, by key, whatever the name.
    h.ban(Ban { name: "dee".into(), ip: None, key: Some(Identity::for_name("dee").key()), why: "griefing".into() });
    assert_eq!(refused(&mut h, "not dee", "sesame", false, "dee"), "Banned");

    // A first-person server, refused by this game before it joins.
    let mut fp = host(HostOptions { camera: Camera::FirstPerson, ..Default::default() });
    let info = with_host(&mut fp, |addr| remote::query(&addr, Duration::from_secs(10))).expect("an answer");
    let why = zombye_net::browser::can_join(&info, &[], false).expect_err("no first person here");
    assert_eq!(explain(&why, &Server::default()).title, "First person only");
}

#[test]
fn no_server_and_other_game_data_are_explained() {
    // Nothing listens on a port just let go.
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
    let raw = Remote::connect(&free, "ada", "", Duration::from_secs(2)).map(|_| ()).expect_err("nobody there");
    let p = explain(&raw, &Server { name: "", addr: &free, version: None });
    assert_eq!(p.title, "Can't reach the server", "{raw}");
    assert!(p.text.contains(&free) && p.retry_helps);

    // Other game data: the welcome's content against ours less a weapon.
    let mut h = host(HostOptions::default());
    let r = with_host(&mut h, |addr| Remote::connect(&addr, "ada", "", Duration::from_secs(10))).expect("ada joins");
    let mut ours = content::load_config(&content::default_data_dir(), 1).unwrap();
    ours.weapons.retain(|w| w.id != "crowbar");
    let raw = remote::build_map(&r.welcome, &ours).map(|_| ()).expect_err("other weapons");
    let p = explain(&raw, &Server { name: "Test", addr: "x", version: Some("0.1.0 (1234567)") });
    assert_eq!(p.title, "Different version", "{raw}");
    assert!(p.text.contains("Test runs Zombye 0.1.0 (1234567)") && p.text.contains(zombye_net::BUILD) && p.text.contains("weapons"), "{}", p.text);
}
