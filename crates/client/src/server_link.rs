//! Connection to the authoritative server. In single-player that's an in-process
//! [`LocalServer`] ticked at the sim rate in `FixedUpdate`; rendering interpolates between the last
//! two snapshots. With `--connect HOST[:PORT]` it is a multiplayer server over the network
//! instead (#21, [`zombye_net::remote::Remote`]) behind the same resource: intents go out once a
//! tick, and the snapshots that have arrived are taken up one a tick (see [`RemoteLink`]).
//!
//! On "join" the client gets its player id, a copy of the map ([`ClientMap`]) and the weapon
//! definitions, and from then on only the map edits carried in snapshots, exactly as a remote
//! client will.

use bevy::prelude::*;
use std::collections::HashMap;

use std::collections::VecDeque;
use std::time::Duration;

use zombye_net::remote::{Remote, build_map};
use zombye_net::{EdgeDamage, Intent, LocalServer, PrivateView, Snapshot, content, save_file};
use zombye_sim::map::{Edge, EdgeChange, OwnedEdge, Side, TileMap};
use zombye_sim::{CombatEvent, CombatTuning, ItemDef, ItemNote, LevelUp, TICK_RATE, WeaponDef};

use crate::Options;

/// Where the world is: in this process (single-player), or on a server.
enum Backend {
    Local(LocalServer),
    Remote(Box<RemoteLink>),
}

/// A multiplayer server's end of the link. Snapshots arrive when the network brings them (none
/// this tick, two the next), so they wait in a short queue and each client tick takes up one:
/// rendering then interpolates between snapshots a tick apart, as in single-player. One is left
/// waiting as a cushion against jitter; a longer queue (we fell behind) is taken up faster.
struct RemoteLink {
    remote: Remote,
    waiting: VecDeque<Snapshot>,
    /// When the bytes received were last counted, and the count then (for the HUD's rate).
    counted: (std::time::Instant, u64),
}

impl RemoteLink {
    /// Send what was queued this tick and return the snapshots to take up now, oldest first.
    fn advance(&mut self) -> Vec<Snapshot> {
        self.remote.flush();
        self.waiting.extend(self.remote.poll());
        let take = take_now(self.waiting.len());
        self.waiting.drain(..take).collect()
    }
}

/// How many of `waiting` snapshots a client tick takes up.
fn take_now(waiting: usize) -> usize {
    match waiting {
        0 => 0,
        1 | 2 => 1,
        3..=6 => 2,
        n => n - 1,
    }
}

/// What the HUD says about a multiplayer server we're on.
#[derive(Clone, Debug, Default)]
pub struct NetInfo {
    pub addr: String,
    /// The server's name and whether players can hurt each other there.
    pub server: String,
    pub pvp: bool,
    /// The name we go by.
    pub name: String,
    pub ping_ms: Option<f32>,
    /// Kilobytes a second coming down from the server, over the last second or so (#22).
    pub down_kbps: Option<f32>,
    /// Why the connection ended, once it has.
    pub closed: Option<String>,
}

#[derive(Resource)]
pub struct ServerLink {
    server: Backend,
    /// The multiplayer server we're on (`--connect`); `None` in single-player.
    pub net: Option<NetInfo>,
    pub prev: Snapshot,
    pub curr: Snapshot,
    /// Last tick's sim time and chasing count, and zombies per simulation tier (active, near,
    /// virtual), for the HUD.
    pub sim_ms: f32,
    pub chasing: u32,
    pub tiers: [u32; 3],
    /// The latest private view (inventory, load, open container) of each player we've heard about;
    /// snapshots only carry one when it changed.
    pub private: Vec<PrivateView>,
    /// Players we heard without seeing them, from the snapshots taken up this frame (#66): each a
    /// sound at a position, for a mark on the ground and a footstep there.
    pub heard: Vec<zombye_sim::Heard>,
    /// Single-player with PvP on: every player in the world, seen by ours or not, for the scenario
    /// bot that plays another survivor from this process (`duel.rs`: its own client would always
    /// have itself). Empty otherwise.
    pub everyone: Vec<zombye_net::PlayerView>,
}

impl ServerLink {
    /// Our player's latest private view.
    pub fn own(&self, player: u32) -> Option<&PrivateView> {
        self.private.iter().find(|v| v.player == player)
    }

    fn keep_private(&mut self, views: &[PrivateView]) {
        for v in views {
            match self.private.iter_mut().find(|o| o.player == v.player) {
                Some(o) => *o = v.clone(),
                None => self.private.push(v.clone()),
            }
        }
    }

    pub fn send(&mut self, intent: Intent) {
        match &mut self.server {
            Backend::Local(s) => s.send(intent),
            Backend::Remote(r) => r.remote.send(intent),
        }
    }
}

/// The client's copy of the map, kept in sync from snapshots.
#[derive(Resource)]
pub struct ClientMap(pub TileMap);

/// What the client was told when it joined.
#[derive(Resource, Clone)]
pub struct JoinInfo {
    /// The player this client controls.
    pub player_id: u32,
    pub player_spawn: Vec2,
    /// The weapon definitions; snapshots name weapons by index into this.
    pub weapons: Vec<WeaponDef>,
    /// The item definitions; stacks name items by index into this.
    pub items: Vec<ItemDef>,
    /// The crafting recipes (#12); `Intent::Craft` names one by index into this.
    pub recipes: Vec<zombye_sim::RecipeDef>,
    /// Combat reach and arcs, for choosing between shove and stomp.
    pub combat: CombatTuning,
    /// The infection's settings (night sight while turning).
    pub infection: zombye_sim::InfectionTuning,
    /// First aid's numbers (treatment times shown in the health panel).
    pub first_aid: zombye_sim::FirstAidTuning,
    /// The needs' numbers (status tags, the view cone while tired).
    pub needs: zombye_sim::NeedsTuning,
    /// The skills' numbers, and the carry limit Strength adds to (the skills panel, #46).
    pub skills: zombye_sim::SkillTuning,
    pub carry: zombye_sim::CarryTuning,
}

/// The `FixedUpdate` system that ticks the server. Input is sent before it and reconciled after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerTick;

/// Map edits received this frame, for renderers (doors).
#[derive(Resource, Default)]
pub struct EdgeChangesThisFrame(pub Vec<EdgeChange>);

/// The map edits the server sent this frame, each with what the edge was before (a door opening,
/// a window breaking), for sound. A quickload's differences aren't here: nobody did them.
#[derive(Resource, Default)]
pub struct EdgeTransitionsThisFrame(pub Vec<(EdgeChange, Edge)>);

/// Combat events (hits, kills, shoves, stomps, wounds) received this frame, for hit feedback.
#[derive(Resource, Default)]
pub struct CombatEventsThisFrame(pub Vec<CombatEvent>);

/// What came of our item requests this frame (done, or why not), for the HUD.
#[derive(Resource, Default)]
pub struct ItemNotesThisFrame(pub Vec<ItemNote>);

/// Our skill level-ups this frame (#46), for the HUD's line.
#[derive(Resource, Default)]
pub struct LevelUpsThisFrame(pub Vec<LevelUp>);

/// How battered each damaged door and window is (fraction of its hit points left; missing =
/// undamaged), kept from the join and snapshots, for drawing cracks.
#[derive(Resource, Default)]
pub struct EdgeDamageMap(pub HashMap<(IVec2, OwnedEdge), f32>);

/// Edge damage received this frame, for renderers.
#[derive(Resource, Default)]
pub struct EdgeDamageThisFrame(pub Vec<EdgeDamage>);

/// `--fight-demo`: zombies spawned this far ahead of the player, and how many.
const DEMO_DISTANCE: f32 = 7.0;
const DEMO_ZOMBIES: u32 = 6;

/// The hour a new single-player world starts at without `--time` (the sim's own default, 9:00,
/// stays for the server and tests).
pub const WAKE_HOUR: f32 = 14.0;

pub struct ServerLinkPlugin;

impl Plugin for ServerLinkPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().resource::<Options>().clone();
        let mut config = content::load_config(&options.data, options.seed).unwrap_or_else(|e| panic!("failed to load content: {e}"));
        if let Some(addr) = &options.connect {
            return join_remote(app, addr, &options, &config);
        }
        // Without --zombies, as many as the population file's densities give for this map.
        config.zombies = options.zombies;
        if let Some(chunks) = options.map_chunks {
            config = config.with_map_chunks(chunks);
        }
        // You wake in the early afternoon (#19): six game hours of daylight to search the house
        // before the first night.
        config.start_hour = options.start_hour.unwrap_or(WAKE_HOUR);
        // Player against player (a new world; a loaded one keeps its own setting).
        config.combat.pvp = options.pvp;
        // `--load`: the saved world, with its own seed, map and players (the world options and
        // the demo setups don't apply).
        let loaded = options.load.as_ref().map(|name| {
            let loaded = save_file::resolve(&options.saves, name).and_then(|path| save_file::load(&path, &config, options.load_options()).map_err(|e| e.hint("--load-anyway")));
            match loaded {
                Ok((loaded, ms)) => {
                    for note in &loaded.notes {
                        println!("note: {note}");
                    }
                    let world = loaded.world;
                    println!("loaded {name} in {ms:.0} ms: tick {}, {} zombies", world.tick, world.zombies.len());
                    world
                }
                Err(e) => {
                    eprintln!("can't load {name}: {e}");
                    std::process::exit(1);
                }
            }
        });
        let resumed = loaded.is_some();
        let mut server = match loaded {
            Some(world) => LocalServer::from_world(world),
            None => LocalServer::new(config),
        };
        if resumed {
            // `--player ID` plays (or renders) as that player; otherwise the save's first.
            let player_id = match (options.player, server.world.players.first()) {
                (Some(id), _) if server.world.player(id).is_some() => id,
                (Some(id), _) => {
                    let ids: Vec<String> = server.world.players.iter().map(|p| p.id.to_string()).collect();
                    eprintln!("no player {id} in {}: it has players [{}]", options.load.as_deref().unwrap_or("the save"), ids.join(", "));
                    std::process::exit(1);
                }
                (None, Some(p)) => p.id,
                (None, None) => server.join(options.at),
            };
            return join_local(app, server, player_id);
        }
        // The fight demo wants room to swing: open ground near the house you wake up in.
        let demo_at = if (options.fight_demo || options.duel) && options.at.is_none() { open_ground_near(&server.world.map, server.world.player_spawn).map(|p| p.to_array()) } else { None };
        let siege_at = if options.siege { siege_spot(&server.world.map, server.world.spawn_building) } else { None };
        if options.siege {
            match siege_at {
                Some((at, tile, out)) => {
                    let side = Side::ALL.into_iter().find(|s| s.offset() == out).unwrap_or(Side::S);
                    let what = if server.world.map.edge(tile, side).is_door() { "door" } else { "window" };
                    println!("--siege: {:.0} m out from the {what} of {tile} ({side:?} wall)", at.distance(tile.as_vec2() + Vec2::splat(0.5)));
                }
                None => println!("--siege: the house you wake up in has no shut door or window with {SIEGE_MIN_DISTANCE} m of open ground in front; starting inside instead"),
            }
        }
        let player_id = server.join(options.at.or(siege_at.map(|s| s.0.to_array())).or(demo_at));
        if options.fight_demo {
            // A bat in hand and a small group just ahead (the player faces +x), called over by a noise.
            let me = server.world.player(player_id).map(|p| p.state.pos).unwrap_or(server.world.player_spawn);
            server.send(Intent::SpawnItem { item: "baseball_bat".into(), count: 1, pos: me.to_array() });
            server.send(Intent::PickUp { player: player_id });
            server.send(Intent::SpawnZombies { count: DEMO_ZOMBIES, center: (me + Vec2::X * DEMO_DISTANCE).to_array(), radius: 2.0 });
            server.send(Intent::MakeNoise { pos: me.to_array(), radius: 12.0, lasting: false });
        }
        if let Some((_, door, out)) = siege_at {
            siege(&mut server, player_id, door, out);
        }
        // `--skip-hours`: for pictures of a later hour of this life (the first morning, say).
        if let Some(hours) = options.skip_hours {
            server.world.skip_time(hours);
        }
        if options.duel {
            // A hammer for you (unless the fight demo gave you its bat), and another survivor with a crowbar a
            // few metres ahead, who joins like any player and is driven through intents (`duel.rs`).
            let me = server.world.player(player_id).map(|p| p.state.pos).unwrap_or(server.world.player_spawn);
            if !options.fight_demo {
                // A hammer, not the fight demo's bat: the bat's reach and stagger would end it
                // before the other got a blow in.
                server.send(Intent::SpawnItem { item: "claw_hammer".into(), count: 1, pos: me.to_array() });
                server.send(Intent::PickUp { player: player_id });
            }
            let foe = server.join(Some((me + Vec2::X * crate::duel::START_DISTANCE).to_array()));
            let at = server.world.player(foe).map_or(me, |p| p.state.pos);
            server.tick();
            server.send(Intent::SpawnItem { item: "crowbar".into(), count: 1, pos: at.to_array() });
            server.send(Intent::PickUp { player: foe });
            println!("--duel: PvP on; player {foe} with a crowbar {:.0} m east of you", at.distance(me));
            app.insert_resource(crate::duel::Duel { foe, seq: 0 });
        }
        join_local(app, server, player_id);
    }
}

/// What the client keeps from the config of the world it joined.
fn join_info(config: &zombye_sim::WorldConfig, player_id: u32, player_spawn: Vec2) -> JoinInfo {
    JoinInfo {
        player_id,
        player_spawn,
        weapons: config.weapons.clone(),
        items: config.items.clone(),
        recipes: config.recipes.clone(),
        combat: config.combat.clone(),
        infection: config.infection.clone(),
        first_aid: config.health.first_aid.clone(),
        needs: config.needs,
        skills: config.skills.clone(),
        carry: config.carry.clone(),
    }
}

/// `--connect`: join a multiplayer server (#21). The same things arrive another way: our player
/// and the rules in the server's welcome, the map built here from the server's seed with our own
/// game data (and checked against the server's), the doors and windows changed since and every
/// cracked one, and a first snapshot. Anything wrong (no server, refused, other game data) is said
/// on stderr and the game doesn't start.
fn join_remote(app: &mut App, addr: &str, options: &Options, content: &zombye_sim::WorldConfig) {
    let fail = |e: String| -> ! {
        eprintln!("can't join {addr}: {e}");
        std::process::exit(1);
    };
    let mut remote = Remote::connect(addr, &options.name, &options.password, Duration::from_secs(10)).unwrap_or_else(|e| fail(e));
    if options.lag_ms > 0 {
        // `--lag MS`: a made-up slow line, for trying lag compensation on one machine (#23).
        remote.set_lag(Duration::from_millis(options.lag_ms as u64));
        println!("--lag: playing as if {} ms of ping away", options.lag_ms);
    }
    let joined = build_map(&remote.welcome, content).unwrap_or_else(|e| fail(e));
    let first = remote.wait_snapshot(Duration::from_secs(10)).unwrap_or_else(|e| fail(e));
    let w = &remote.welcome;
    let player_id = w.player;
    println!(
        "joined {} at {addr} as {} (player {player_id}): pvp {}, up to {} players, tick {}",
        w.rules.name,
        w.name,
        if w.rules.pvp { "on" } else { "off (co-op)" },
        w.rules.max_players,
        first.tick
    );
    let damage = EdgeDamageMap(w.edge_damage.iter().map(|d| ((d.tile, d.edge), d.hp)).collect());
    let spawn = first.players.iter().find(|p| p.id == player_id).map(|p| Vec2::from(p.pos)).unwrap_or(joined.player_spawn);
    let join = join_info(&joined.config, player_id, spawn);
    let net = NetInfo { addr: addr.to_string(), server: w.rules.name.clone(), pvp: w.rules.pvp, name: w.name.clone(), ping_ms: None, down_kbps: None, closed: None };
    if options.god || options.ignored {
        remote.send(Intent::Dev { player: player_id, god: options.god.then_some(true), ignored: options.ignored.then_some(true) });
    }
    let counted = (std::time::Instant::now(), remote.bytes_in());
    let link = RemoteLink { remote, waiting: VecDeque::new(), counted };
    finish_join(app, Backend::Remote(Box::new(link)), Some(net), first, ClientMap(joined.map), damage, join);
}

/// Single-player: what a client gets on joining (its map copy, the weapons, every damaged edge, a
/// first snapshot), from the world in this process.
fn join_local(app: &mut App, mut server: LocalServer, player_id: u32) {
    // `--god` / `--ignored`: as if F1 / F2 were pressed at once.
    let options = app.world().resource::<Options>();
    if options.god || options.ignored {
        server.send(Intent::Dev { player: player_id, god: options.god.then_some(true), ignored: options.ignored.then_some(true) });
    }
    let first = Snapshot::capture(&mut server.world);
    // Player against player: other survivors only as ours sees them, as on a server (#66).
    let first = if server.world.config.combat.pvp { first.through_senses(player_id, &server.world) } else { first };
    let map = ClientMap(server.world.map.clone());
    let damage = EdgeDamageMap(EdgeDamage::all(&server.world).into_iter().map(|d| ((d.tile, d.edge), d.hp)).collect());
    let spawn = first.players.iter().find(|p| p.id == player_id).map(|p| Vec2::from(p.pos)).unwrap_or(server.world.player_spawn);
    let join = join_info(&server.world.config, player_id, spawn);
    finish_join(app, Backend::Local(server), None, first, map, damage, join);
}

/// The resources a joined client has, and the systems that tick the server (or take up what it sent).
fn finish_join(app: &mut App, server: Backend, net: Option<NetInfo>, first: Snapshot, map: ClientMap, damage: EdgeDamageMap, join: JoinInfo) {
    let private = first.private.clone();
    app.insert_resource(ServerLink { server, net, prev: first.clone(), curr: first, sim_ms: 0.0, chasing: 0, tiers: [0; 3], private, heard: Vec::new(), everyone: Vec::new() })
        .insert_resource(map)
        .insert_resource(join)
        .insert_resource(damage)
        .init_resource::<EdgeChangesThisFrame>()
        .init_resource::<EdgeTransitionsThisFrame>()
        .init_resource::<EdgeDamageThisFrame>()
        .init_resource::<CombatEventsThisFrame>()
        .init_resource::<ItemNotesThisFrame>()
        .init_resource::<LevelUpsThisFrame>()
        .insert_resource(Time::<Fixed>::from_hz(TICK_RATE as f64))
        .add_systems(First, clear_frame_changes)
        .add_systems(FixedUpdate, tick_server.in_set(ServerTick).run_if(crate::live));
}

impl ServerLink {
    /// Tests: a link to a small world (the bungalow alone, a 4 x 4 chunk map) with one player
    /// joined, and that player's id.
    #[cfg(test)]
    pub fn for_test() -> (Self, u32) {
        use zombye_sim::{BuildingTemplate, WorldConfig};
        let templates = vec![BuildingTemplate::from_ron(include_str!("../../../assets/data/buildings/bungalow.ron")).unwrap()];
        let weapons = zombye_sim::items::parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        let items = zombye_sim::items::parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap();
        let containers = zombye_sim::containers::parse_containers(include_str!("../../../assets/data/containers.ron")).unwrap();
        let mut server = LocalServer::new(WorldConfig::new(4, 0, templates).with_weapons(weapons).with_items(items).with_containers(containers));
        let id = server.join(None);
        let first = server.tick();
        let private = first.private.clone();
        (Self { server: Backend::Local(server), net: None, prev: first.clone(), curr: first, sim_ms: 0.0, chasing: 0, tiers: [0; 3], private, heard: Vec::new(), everyone: Vec::new() }, id)
    }

    /// The world being served, when it is in this process (to save it, or start it again); `None`
    /// on a multiplayer server, whose world is the server's.
    pub fn world(&self) -> Option<&zombye_sim::World> {
        match &self.server {
            Backend::Local(s) => Some(&s.world),
            Backend::Remote(_) => None,
        }
    }

    /// Serve `world` (a loaded save) from now on: drops queued intents and starts the snapshots
    /// afresh from it. Returns the first snapshot. Single-player only: on a multiplayer server
    /// nothing changes.
    pub fn replace_world(&mut self, world: zombye_sim::World) -> &Snapshot {
        let Backend::Local(server) = &mut self.server else { return &self.curr };
        server.replace_world(world);
        let first = Snapshot::capture(&mut server.world);
        let s = server.world.stats;
        (self.sim_ms, self.chasing, self.tiers) = (0.0, 0, [s.active, s.near, s.virtual_]);
        self.private = first.private.clone();
        self.prev = first.clone();
        self.curr = first;
        &self.curr
    }
}

fn clear_frame_changes(
    mut changes: ResMut<EdgeChangesThisFrame>,
    mut transitions: ResMut<EdgeTransitionsThisFrame>,
    mut damage: ResMut<EdgeDamageThisFrame>,
    mut hits: ResMut<CombatEventsThisFrame>,
    mut notes: ResMut<ItemNotesThisFrame>,
    mut ups: ResMut<LevelUpsThisFrame>,
    mut link: ResMut<ServerLink>,
) {
    if !link.heard.is_empty() {
        link.heard.clear();
    }
    ups.0.clear();
    changes.0.clear();
    transitions.0.clear();
    damage.0.clear();
    hits.0.clear();
    notes.0.clear();
}

#[allow(clippy::too_many_arguments)]
fn tick_server(
    mut link: ResMut<ServerLink>,
    mut map: ResMut<ClientMap>,
    mut changes: ResMut<EdgeChangesThisFrame>,
    mut transitions: ResMut<EdgeTransitionsThisFrame>,
    mut damage_map: ResMut<EdgeDamageMap>,
    mut damage: ResMut<EdgeDamageThisFrame>,
    mut hits: ResMut<CombatEventsThisFrame>,
    mut notes: ResMut<ItemNotesThisFrame>,
    mut ups: ResMut<LevelUpsThisFrame>,
    join: Res<JoinInfo>,
) {
    let link = &mut *link;
    let snapshots = match &mut link.server {
        Backend::Local(server) => {
            // A remote server sends us only our own private view and notes, and with PvP on other
            // players only as our survivor sees or hears them (#66); do as it does.
            let full = server.tick();
            link.everyone = if server.world.config.combat.pvp { full.players.clone() } else { Vec::new() };
            let snapshot = full.for_local(join.player_id, &server.world);
            let s = server.world.stats;
            (link.sim_ms, link.chasing, link.tiers) = (s.tick_ms, s.chasing, [s.active, s.near, s.virtual_]);
            vec![snapshot]
        }
        Backend::Remote(remote) => {
            let snapshots = remote.advance();
            if let Some(net) = &mut link.net {
                net.ping_ms = remote.remote.ping_ms;
                let (since, bytes) = remote.counted;
                if since.elapsed().as_secs_f32() >= 1.0 {
                    let now = remote.remote.bytes_in();
                    net.down_kbps = Some((now - bytes) as f32 / since.elapsed().as_secs_f32() / 1000.0);
                    remote.counted = (std::time::Instant::now(), now);
                }
                if net.closed.is_none() && remote.remote.closed.is_some() {
                    net.closed = remote.remote.closed.clone();
                    eprintln!("disconnected from {}: {}", net.addr, net.closed.as_deref().unwrap_or_default());
                }
            }
            // Nothing new this tick: stand still rather than run the last step again.
            if snapshots.is_empty() && link.prev.tick != link.curr.tick {
                link.prev = link.curr.clone();
            }
            snapshots
        }
    };
    for snapshot in snapshots {
        take_up(link, snapshot, &mut map, &mut changes, &mut transitions, &mut damage_map, &mut damage, &mut hits, &mut notes, &mut ups);
    }
}

/// Take up one snapshot: keep what it brought (our private view, notes, level-ups, map changes,
/// cracks, hits) and make it the current one.
#[allow(clippy::too_many_arguments)]
fn take_up(
    link: &mut ServerLink,
    snapshot: Snapshot,
    map: &mut ClientMap,
    changes: &mut EdgeChangesThisFrame,
    transitions: &mut EdgeTransitionsThisFrame,
    damage_map: &mut EdgeDamageMap,
    damage: &mut EdgeDamageThisFrame,
    hits: &mut CombatEventsThisFrame,
    notes: &mut ItemNotesThisFrame,
    ups: &mut LevelUpsThisFrame,
) {
    link.keep_private(&snapshot.private);
    notes.0.extend(snapshot.item_notes.iter().cloned());
    ups.0.extend_from_slice(&snapshot.level_ups);
    for c in &snapshot.edge_changes {
        transitions.0.push((*c, map.0.owned_edge(c.tile, c.edge)));
        map.0.set_owned_edge(c.tile, c.edge, c.kind);
        changes.0.push(*c);
    }
    for d in &snapshot.edge_damage {
        if d.hp >= 1.0 {
            damage_map.0.remove(&(d.tile, d.edge));
        } else {
            damage_map.0.insert((d.tile, d.edge), d.hp);
        }
        damage.0.push(*d);
    }
    hits.0.extend_from_slice(&snapshot.hits);
    link.heard.extend_from_slice(&snapshot.heard);
    link.prev = std::mem::replace(&mut link.curr, snapshot);
}

/// `--siege`: where you start (outside the house you woke up in, a few metres out from one of its
/// shut doors, else one of its intact windows), the door's tile inside, and the way out through it.
/// The way out must be outdoors and walkable in a straight line, at least `SIEGE_MIN_DISTANCE`
/// tiles; of those, the one with the most room up to `SIEGE_DISTANCE`, the south and east walls
/// first (those face the default camera).
fn siege_spot(map: &TileMap, house: usize) -> Option<(Vec2, IVec2, IVec2)> {
    let b = &map.buildings[house];
    let ours = |t: IVec2| map.tile(t).room != 0 && map.rooms[map.tile(t).room as usize].building as usize == house;
    // How many tiles straight out from `t` through `side` are open outdoor ground.
    let run = |t: IVec2, side: Side| {
        let out = side.offset();
        (1..=SIEGE_DISTANCE)
            .take_while(|&k| {
                let c = t + out * k;
                map.in_bounds(c) && map.tile(c).room == 0 && !map.tile_blocked(c) && (k == 1 || map.can_step(c - out, side))
            })
            .count() as i32
    };
    let mut best: Option<(i32, IVec2, Side)> = None;
    for wanted in [Edge::DoorClosed, Edge::Window] {
        for side in [Side::S, Side::E, Side::N, Side::W] {
            for y in b.min.y..b.min.y + b.size.y {
                for x in b.min.x..b.min.x + b.size.x {
                    let t = IVec2::new(x, y);
                    if !ours(t) || map.edge(t, side) != wanted {
                        continue;
                    }
                    let n = run(t, side);
                    if n >= SIEGE_MIN_DISTANCE && best.is_none_or(|(m, ..)| n > m) {
                        best = Some((n, t, side));
                    }
                }
            }
        }
        // Any door that leaves room to stand back beats every window.
        if best.is_some() {
            break;
        }
    }
    let (n, t, side) = best?;
    let out = side.offset();
    Some(((t + out * n).as_vec2() + Vec2::splat(0.5), t, out))
}

/// `--siege`, after joining: a bat in hand, and a few zombies between you and the door drawn to a
/// noise just inside it (not yours), so they batter it with their backs to you.
fn siege(server: &mut LocalServer, player_id: u32, door_tile: IVec2, out: IVec2) {
    let me = server.world.player(player_id).map(|p| p.state.pos).unwrap_or(server.world.player_spawn);
    server.send(Intent::SpawnItem { item: "baseball_bat".into(), count: 1, pos: me.to_array() });
    server.send(Intent::PickUp { player: player_id });
    let near_door = (door_tile + out * 2).as_vec2() + Vec2::splat(0.5);
    server.send(Intent::SpawnZombies { count: SIEGE_ZOMBIES, center: near_door.to_array(), radius: 1.0 });
    server.send(Intent::MakeNoise { pos: (door_tile.as_vec2() + Vec2::splat(0.5)).to_array(), radius: 30.0, lasting: false });
}

/// `--siege`: how far out from the door you start (tiles; at least `SIEGE_MIN_DISTANCE` where
/// nothing has that much room), and how many zombies batter it.
const SIEGE_DISTANCE: i32 = 10;
const SIEGE_MIN_DISTANCE: i32 = 4;
const SIEGE_ZOMBIES: u32 = 5;

/// The centre of the nearest outdoor tile (searching outward from `from`) with a clear square of
/// open ground around it: no walls, fences, furniture or trees within a few metres.
fn open_ground_near(map: &TileMap, from: Vec2) -> Option<Vec2> {
    const CLEAR: i32 = 5;
    let start = from.floor().as_ivec2();
    let open = |t: IVec2| map.in_bounds(t) && map.tile(t).room == 0 && !map.tile_blocked(t) && Side::ALL.iter().all(|&s| map.edge(t, s) == Edge::None);
    for r in 0..60i32 {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dy.abs() != r {
                    continue;
                }
                let c = start + IVec2::new(dx, dy);
                if (-CLEAR..=CLEAR).all(|y| (-CLEAR..=CLEAR).all(|x| open(c + IVec2::new(x, y)))) {
                    return Some(c.as_vec2() + Vec2::splat(0.5));
                }
            }
        }
    }
    None
}


#[cfg(test)]
mod tests {
    use super::*;

    /// On a multiplayer server one snapshot is taken up a tick, one left waiting as a cushion, and
    /// a pile is worked off faster without ever skipping what a snapshot brought.
    #[test]
    fn waiting_snapshots_are_taken_up_one_a_tick() {
        assert_eq!([0, 1, 2, 3, 6, 7, 30].map(take_now), [0, 1, 1, 2, 2, 6, 29]);
        // Arriving one a tick, the queue settles; arriving in bursts of three, it never grows.
        let mut waiting = 0;
        for tick in 0..60 {
            waiting += if tick % 3 == 0 { 3 } else { 0 };
            waiting -= take_now(waiting);
            assert!(waiting <= 3, "{waiting} waiting at tick {tick}");
        }
    }

    /// #79: the shipped town gives a siege on the default seed and the next few, from a shut door
    /// or window of the spawn house with open ground in front.
    #[test]
    fn siege_has_a_door_to_batter_on_the_shipped_town() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        for seed in 1..=4u64 {
            let mut config = content::load_config(&data, seed).unwrap();
            config.zombies = Some(0);
            let world = zombye_sim::World::new(config);
            let map = &world.map;
            let (at, tile, out) = siege_spot(map, world.spawn_building).unwrap_or_else(|| panic!("seed {seed}: no siege spot"));
            let side = Side::ALL.into_iter().find(|s| s.offset() == out).unwrap();
            assert!(map.edge(tile, side).is_breakable(), "seed {seed}: {tile} {side:?} is {:?}", map.edge(tile, side));
            let stand = at.floor().as_ivec2();
            assert!(map.tile(stand).room == 0 && !map.tile_blocked(stand), "seed {seed}: standing at {stand}");
            assert!(at.distance(tile.as_vec2() + Vec2::splat(0.5)) >= SIEGE_MIN_DISTANCE as f32, "seed {seed}");
        }
    }

    /// The siege itself: the zombies set down between you and the door batter it (or the window
    /// beside it).
    #[test]
    fn siege_zombies_batter_the_door() {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let mut config = content::load_config(&data, 1).unwrap();
        config.zombies = Some(0);
        let mut server = LocalServer::new(config);
        let (at, tile, out) = siege_spot(&server.world.map, server.world.spawn_building).unwrap();
        let id = server.join(Some(at.to_array()));
        siege(&mut server, id, tile, out);
        let mut thumps = 0;
        for _ in 0..200 {
            server.tick();
            thumps += server.world.thump_events.iter().filter(|t| (t.edge.tile - tile).abs().max_element() <= 3).count();
        }
        let p = server.world.player(id).unwrap();
        assert_eq!(p.weapon.map(|w| server.world.config.weapons[w as usize].id.as_str()), Some("baseball_bat"), "a bat in hand");
        assert!(thumps >= 3, "only {thumps} thumps by the door in 10 s");
    }
}
