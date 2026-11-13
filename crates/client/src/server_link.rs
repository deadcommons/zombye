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

use zombye_net::modpack::{self, ModPack};
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
    /// The server takes the dev tools (`--allow-dev`): the debug stats window opens there too.
    pub allow_dev: bool,
    /// Every rule the server told us on joining (#69), for the rules panel (`rules_ui.rs`).
    pub rules: zombye_net::wire::Rules,
    /// The server's mods and how we came by each (#24), for the rules panel; empty: none.
    pub mods: String,
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
    /// What our survivor heard said, from the snapshots taken up this frame (#67; our own lines
    /// too), for `chat.rs`.
    pub speech: Vec<zombye_sim::Said>,
    /// Tiles whose tree came down (stumps now) in the snapshots taken up this frame, or in a
    /// world taken up: their chunks are built again (`world_render`).
    pub felled: Vec<IVec2>,
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
    /// The build catalog (`build.ron`): barricades now, walls and upgrades later. `Intent::Build`
    /// names an entry by id; a job names it by index into this.
    pub builds: Vec<zombye_sim::BuildDef>,
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
    /// When E climbs a fence or window (sent only where the server takes one).
    pub climb: zombye_sim::ClimbTuning,
    /// Fires and cooking: a fire's reach, what heats a pot (the Make list, a fire's menu).
    pub fire: zombye_sim::FireTuning,
    /// The town's taps (`zombye_sim::power::WaterDef`): where the Make list says a recipe has water.
    pub water: zombye_sim::power::WaterDef,
    /// The vehicle kinds and the rules round driving (`vehicles.ron`): our own car is predicted
    /// with them, and every car drawn and kept out of by its kind's box.
    pub vehicles: zombye_sim::VehicleData,
    /// Which furniture is a container, for right-clicking one in the world (`world_actions.rs`).
    pub containers: zombye_sim::containers::ContainerData,
    /// The item tag that makes an item a blind to fit.
    pub blind_tag: String,
    /// The world's seed and what zombies wear (`population.ron`'s outfits): with them a zombie's
    /// outfit is worked out here as the server does (`zombye_sim::corpses::default_look`), for
    /// drawing the crowd variant that matches it. Snapshots send only the looks that differ.
    pub seed: u64,
    pub outfits: Vec<zombye_sim::corpses::Outfit>,
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
/// With `--gun` (#72): further off, so there is something to shoot at before they arrive.
const GUN_DEMO_DISTANCE: f32 = 16.0;
const DEMO_ZOMBIES: u32 = 6;

/// The hour a new single-player world starts at without `--time` (the sim's own default, 9:00,
/// stays for the server and tests).
pub const WAKE_HOUR: f32 = 14.0;

pub struct ServerLinkPlugin;

impl Plugin for ServerLinkPlugin {
    fn build(&self, app: &mut App) {
        let mut options = app.world().resource::<Options>().clone();
        let (base, mut config, own) = match local_content(&options) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        let base = options.connect.is_some().then_some(base);
        // What the server browser counts a server's mods against (#27, #24): the ones we have,
        // loaded or downloaded before; the rest are downloaded on joining.
        let cache = modpack::Cache::in_saves(&options.saves);
        app.insert_resource(crate::browser_ui::LoadedMods(own.iter().map(ModPack::hash).chain(cache.listed()).collect()));
        if let (Some(addr), Some(base)) = (options.connect.clone(), base) {
            // The server's mods, not ours (#24): what we haven't got of them is downloaded first.
            let mut joined = server_content(&addr, &options, &base, &own, &cache).and_then(|(content, mods)| join_remote(app, &addr, &options, &content, mods));
            // After the join screen (DCM-70), whose own connection has only just closed: the
            // server may not have let our key's survivor go yet. Ask again a moment later.
            for _ in 0..3 {
                match &joined {
                    Err(e) if options.joined && e.contains("same player key") => {
                        std::thread::sleep(Duration::from_millis(700));
                        joined = server_content(&addr, &options, &base, &own, &cache).and_then(|(content, mods)| join_remote(app, &addr, &options, &content, mods));
                    }
                    _ => break,
                }
            }
            match joined {
                Ok(()) => return,
                // After the join screen: back to it, with what went wrong.
                Err(e) if options.joined => {
                    eprintln!("can't join {addr}: {e}");
                    let args = crate::join_screen::failed_args(&std::env::args().skip(1).collect::<Vec<_>>(), &e);
                    crate::relaunch(crate::browser_ui::Relaunch { args, password: options.password.clone() });
                }
                // Asked for in the server browser (#27): the game doesn't stop. Back to our own
                // world (the autosave the join left, if we came from one), with the browser open
                // and the reason on it.
                Err(e) if options.from_browser.is_some() => {
                    eprintln!("can't join {addr}: {e}");
                    let autosave = save_file::path_for(&options.saves, crate::saving::AUTOSAVE).ok().filter(|p| p.is_file());
                    options.connect = None;
                    options.load = autosave.filter(|_| options.from_browser.as_deref() == Some("single")).map(|p| p.to_string_lossy().into_owned());
                    app.insert_resource(options.clone());
                    app.insert_resource(crate::browser_ui::BrowserStart { note: format!("Can't join {addr}: {e}"), addr: addr.clone() });
                }
                Err(e) => {
                    eprintln!("can't join {addr}: {e}");
                    std::process::exit(1);
                }
            }
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
        // A new game's choices (`new_game.rs`): the world's options onto the content, and the
        // survivor to make once it has joined. A loaded world keeps its own.
        let start_as = if options.load.is_none() {
            crate::new_game::from_options(&options, &mut config).unwrap_or_else(|e| {
                eprintln!("can't start that game: {e}");
                std::process::exit(2);
            })
        } else {
            None
        };
        app.insert_resource(crate::new_game::StartAs(start_as.clone()));
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
        // A packaged game (a playtest build) keeps the dev tools to itself unless asked for: no
        // god mode, spawning, skipping time or `--give` for a player who didn't ask.
        server.allow_dev = zombye_net::install::dev_tools();
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
        let demo_at = if (options.fight_demo || options.duel || options.holdup_demo || (options.talk_demo && !options.talk_wall)) && options.at.is_none() { open_ground_near(&server.world.map, server.world.player_spawn).map(|p| p.to_array()) } else { None };
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
        if let Some(start) = &start_as {
            server.world.begin_as(player_id, start);
            let o = &start.origin;
            println!("new game: {} ({} strengths, {} flaws), {}", o.background, o.strengths.len(), o.flaws.len(), server.world.config.sandbox.difficulty);
        }
        // `--gun ID` (#72): that gun in hand, loaded, with a spare magazine if it takes one and
        // three loads of loose rounds in the pockets.
        let gun = options.gun.as_deref().and_then(|id| {
            let found = server.world.config.weapons.iter().find(|w| w.id == id).and_then(|w| w.ranged.clone());
            if found.is_none() {
                eprintln!("--gun: no gun {id:?} in weapons.ron");
            }
            found.map(|r| (id.to_string(), r))
        });
        if let Some((id, r)) = &gun {
            let me = server.world.player(player_id).map(|p| p.state.pos).unwrap_or(server.world.player_spawn);
            let mut give = |item: &str, count: u16| {
                server.send(Intent::SpawnItem { item: item.into(), count, pos: me.to_array() });
                server.send(Intent::PickUp { player: player_id });
            };
            give(id, 1);
            if let Some(m) = &r.magazine {
                give(m, 1);
            }
            give(&r.ammo, r.capacity as u16 * 3);
        }
        // `--give`: things into our pockets, or at our feet when there's no room for them (a log).
        for want in options.give.iter().filter(|_| zombye_net::install::dev_tools()).flat_map(|g| g.split(',')).map(str::trim).filter(|w| !w.is_empty()) {
            let (id, n) = want.split_once(':').map_or((want, 1), |(id, n)| (id, n.parse().unwrap_or(1)));
            let Some(i) = server.world.item_index(id) else {
                eprintln!("--give: no item {id:?} in items.ron");
                continue;
            };
            let p = server.world.player(player_id).expect("just joined");
            let (c, pos) = (server.world.carry(p), p.state.pos);
            let fits = server.world.item_def(i).volume_l * n as f32 <= (c.room_l - c.used_l).max(0.0) + 1e-4;
            if fits && server.world.give_item(player_id, id, n) {
                println!("--give: {n} x {id}");
            } else if server.world.spawn_item(id, n, pos).is_some() {
                println!("--give: {n} x {id} at your feet (no room for it)");
            }
        }
        if options.fight_demo {
            // A bat in hand and a small group just ahead (the player faces +x), called over by a
            // noise. With a gun instead, they start further off.
            let me = server.world.player(player_id).map(|p| p.state.pos).unwrap_or(server.world.player_spawn);
            if gun.is_none() && !options.weapons_demo {
                server.send(Intent::SpawnItem { item: "baseball_bat".into(), count: 1, pos: me.to_array() });
                server.send(Intent::PickUp { player: player_id });
            }
            let distance = if gun.is_some() { GUN_DEMO_DISTANCE } else { DEMO_DISTANCE };
            server.send(Intent::SpawnZombies { count: DEMO_ZOMBIES, center: (me + Vec2::X * distance).to_array(), radius: 2.0 });
            server.send(Intent::MakeNoise { pos: me.to_array(), radius: distance + 5.0, lasting: false });
        }
        if gun.is_some() || options.fight_demo {
            // Take the weapon up now: an offscreen picture never ticks the world, and would show
            // him bare-handed.
            server.tick();
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
        if options.talk_demo {
            // Another survivor who talks and gestures by itself (`chat.rs`): a few metres ahead on
            // open ground, or outside the wall of the room we wake up in, to talk through it.
            let me = server.world.player(player_id).map(|p| p.state.pos).unwrap_or(server.world.player_spawn);
            let reach = server.world.config.speech.talk_m * server.world.config.zombie.wall_muffle;
            let behind = if options.talk_wall { behind_a_wall(&server.world.map, me, reach - 0.4) } else { None };
            let other = server.join(Some(behind.unwrap_or(me + Vec2::X * crate::chat::TALK_DISTANCE).to_array()));
            let at = server.world.player(other).map_or(me, |p| p.state.pos);
            match (options.talk_wall, behind) {
                (true, Some(_)) => println!("--talk-wall: player {other} outside the wall, {:.1} m from you", at.distance(me)),
                (true, None) => println!("--talk-wall: no place outside a wall within {reach:.1} m of you; player {other} is {:.0} m east instead", at.distance(me)),
                _ => println!("--talk-demo: player {other} {:.0} m east of you", at.distance(me)),
            }
            app.insert_resource(crate::chat::TalkDemo::new(other));
        }
        if options.holdup_demo {
            // Another survivor right in front of us with a few things on it, who puts its hands up
            // (`holdup.rs`); cable ties to tie them with and scissors to cut them off again for us.
            let me = server.world.player(player_id).map(|p| p.state.pos).unwrap_or(server.world.player_spawn);
            let other = server.join(Some((me + Vec2::X * crate::holdup::DEMO_DISTANCE).to_array()));
            let w = &mut server.world;
            w.give_item(player_id, "cable_tie", 3);
            w.give_item(player_id, "scissors", 1);
            w.give_item(other, "tinned_beans", 2);
            w.give_item(other, "painkillers", 1);
            w.hold_item(other, zombye_sim::Slot::Hand, "kitchen_knife");
            let at = w.player(other).map_or(me, |p| p.state.pos);
            println!("--holdup-demo: PvP on; player {other} {:.1} m east of you, hands up. Y ties, L searches, Y again frees", at.distance(me));
            app.insert_resource(crate::holdup::HoldupDemo::new(other, options.holdup_auto));
        }
        join_local(app, server, player_id);
    }
}

/// `--talk-wall`: the nearest open tile outdoors within `reach` metres of `from` with no line of
/// sight to it (a wall between, and no window in the way).
fn behind_a_wall(map: &TileMap, from: Vec2, reach: f32) -> Option<Vec2> {
    let r = reach.ceil() as i32;
    let centre = from.floor().as_ivec2();
    let mut best: Option<(f32, Vec2)> = None;
    for dy in -r..=r {
        for dx in -r..=r {
            let t = centre + IVec2::new(dx, dy);
            let p = t.as_vec2() + Vec2::splat(0.5);
            let d = p.distance(from);
            if d > reach || !map.in_bounds(t) || map.tile(t).room != 0 || map.tile_blocked(t) || zombye_sim::vision::line_of_sight(map, from, p) {
                continue;
            }
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// What the client keeps from the config of the world it joined.
fn join_info(config: &zombye_sim::WorldConfig, player_id: u32, player_spawn: Vec2) -> JoinInfo {
    JoinInfo {
        player_id,
        player_spawn,
        weapons: config.weapons.clone(),
        items: config.items.clone(),
        recipes: config.recipes.clone(),
        builds: config.builds.clone(),
        combat: config.combat.clone(),
        infection: config.infection.clone(),
        first_aid: config.health.first_aid.clone(),
        needs: config.needs,
        skills: config.skills.clone(),
        carry: config.carry.clone(),
        climb: config.climb.clone(),
        fire: config.fire.clone(),
        water: config.power.water.clone(),
        vehicles: config.vehicles.clone(),
        containers: config.containers.clone(),
        blind_tag: config.blinds.tag.clone(),
        seed: config.seed(),
        outfits: config.population.outfits.clone(),
    }
}

/// This game's own data, from the data folder (a fresh loot seed unless `--loot-seed`), as it is
/// before our own `--mod`s (what a server's mods go into instead) and with them, and our mods as
/// packs (#24, #25).
pub fn local_content(options: &Options) -> Result<(zombye_sim::WorldConfig, zombye_sim::WorldConfig, Vec<ModPack>), String> {
    let mut config = content::load_config(&options.data, options.seed).map_err(|e| format!("failed to load content: {e}"))?;
    // A new world finds different things each game, unless `--loot-seed` fixes them (a
    // loaded save keeps its own).
    config.loot_seed = Some(options.loot_seed.unwrap_or_else(content::fresh_loot_seed));
    // Mods (#25): their items, recipes and loot join the game's; their scripts run in the
    // world this process makes (on a server they run there, and only the data matters here).
    let base = config.clone();
    let mods = zombye_mods::load_into(&mut config, &options.mods).map_err(|e| format!("failed to load mods: {e}"))?;
    if !mods.is_empty() {
        println!("{}", zombye_mods::describe(&mods));
    }
    Ok((base, config, mods.iter().map(zombye_mods::Mod::pack).collect()))
}

/// The game data to join `addr` with (#24): the game's own (`base`) with **the server's mods**,
/// in the server's order. The server is asked what it runs; each mod's data comes from our own
/// `--mod` folders (`own`) when one holds exactly that data (by hash), else from the cache of
/// earlier downloads, else it is downloaded now, checked against the hash the server named, and
/// kept. Mods of ours the server doesn't run are left out while we play there. Only data arrives:
/// a mod's scripts run on the server, never here. Also a line for the rules panel.
fn server_content(addr: &str, options: &Options, base: &zombye_sim::WorldConfig, own: &[ModPack], cache: &modpack::Cache) -> Result<(zombye_sim::WorldConfig, String), String> {
    let info = zombye_net::remote::query(addr, Duration::from_secs(10))?;
    content_for(addr, &info, options, base, own, cache)
}

/// Our own game data (`base`, before our own mods) with `info`'s server's mods put in, from our
/// own `--mod` folders, the download cache, or downloaded now; and the mods' names for the rules
/// panel.
pub fn content_for(addr: &str, info: &zombye_net::wire::ServerInfo, options: &Options, base: &zombye_sim::WorldConfig, own: &[ModPack], cache: &modpack::Cache) -> Result<(zombye_sim::WorldConfig, String), String> {
    let timeout = Duration::from_secs(10);
    let got = modpack::gather(addr, &options.password, &info.mods, own, Some(cache), timeout)?;
    for line in got.lines() {
        println!("{line}");
    }
    let mut content = base.clone();
    let packs: Vec<ModPack> = got.packs.iter().map(|(p, _)| p.clone()).collect();
    zombye_mods::apply_packs(&mut content, &packs).map_err(|e| format!("the server's mods don't fit this game's data: {e}"))?;
    let names: Vec<String> = got
        .packs
        .iter()
        .map(|(p, from)| {
            let name = if p.name.is_empty() { &p.id } else { &p.name };
            let v = if p.version.is_empty() { String::new() } else { format!(" {}", p.version) };
            format!("{name}{v}{}", if *from == modpack::Source::Downloaded { " (downloaded)" } else { "" })
        })
        .collect();
    Ok((content, names.join(", ")))
}

/// `--connect`: join a multiplayer server (#21). The same things arrive another way: our player
/// and the rules in the server's welcome, the map built here from the server's seed with our own
/// game data (and checked against the server's), the doors and windows changed since and every
/// cracked one, and a first snapshot. Anything wrong (no server, refused, other game data) is the
/// error: the caller says it on stderr and the game doesn't start (or, joined from the server
/// browser, starts in its own world instead).
fn join_remote(app: &mut App, addr: &str, options: &Options, content: &zombye_sim::WorldConfig, mods: String) -> Result<(), String> {
    // A client that plays by itself says so (`--agent`, and `--autoplay`, which is one): the
    // server's rules may refuse agents, or mark them to the other players (#69).
    // Who we are on servers (DCM-66): a key made once and kept in the saves folder beside
    // settings.ron. Our survivors there are that key's, whatever name we join under.
    let identity = zombye_net::identity::Identity::load_or_create(&options.saves.join(zombye_net::identity::FILE))?;
    let mut remote = Remote::connect_with(addr, &options.name, &options.password, options.agent || options.autoplay, &identity, Duration::from_secs(10))?;
    // The camera is a server rule (docs/PLAN.md §5), and this client has only the isometric one.
    if remote.welcome.rules.camera != zombye_net::wire::Camera::Isometric {
        return Err(format!("{} is first person only, and this version of the game has no first-person camera yet", remote.welcome.rules.name));
    }
    if options.lag_ms > 0 {
        // `--lag MS`: a made-up slow line, for trying lag compensation on one machine (#23).
        remote.set_lag(Duration::from_millis(options.lag_ms as u64));
        println!("--lag: playing as if {} ms of ping away", options.lag_ms);
    }
    let joined = build_map(&remote.welcome, content)?;
    let first = remote.wait_snapshot(Duration::from_secs(10))?;
    let w = &remote.welcome;
    let player_id = w.player;
    println!(
        "joined {} at {addr} as {} (player {player_id}): pvp {}, up to {} players, logging out leaves your survivor in the world for {:.0} s ({:.0} s somewhere safe){}, tick {}",
        w.rules.name,
        w.name,
        if w.rules.pvp { "on" } else { "off (co-op)" },
        w.rules.max_players,
        w.rules.logout_secs,
        w.rules.logout_safe_secs,
        if w.rules.pvp && w.rules.spawn_protection_secs > 0.0 { format!(", new survivors are protected from players for {:.0} s", w.rules.spawn_protection_secs) } else { String::new() },
        first.tick
    );
    // Every rule, as the rules panel shows them (#69).
    println!("the rules of {}:", w.rules.name);
    for (label, text) in w.rules.lines() {
        println!("  {label}: {text}");
    }
    let damage = EdgeDamageMap(w.edge_damage.iter().map(|d| ((d.tile, d.edge), d.hp)).collect());
    let spawn = first.players.iter().find(|p| p.id == player_id).map(|p| Vec2::from(p.pos)).unwrap_or(joined.player_spawn);
    let join = join_info(&joined.config, player_id, spawn);
    let net = NetInfo { addr: addr.to_string(), server: w.rules.name.clone(), pvp: w.rules.pvp, allow_dev: w.rules.allow_dev, rules: w.rules.clone(), mods, name: w.name.clone(), ping_ms: None, down_kbps: None, closed: None };
    if options.god || options.ignored {
        remote.send(Intent::Dev { player: player_id, god: options.god.then_some(true), ignored: options.ignored.then_some(true) });
    }
    let counted = (std::time::Instant::now(), remote.bytes_in());
    let link = RemoteLink { remote, waiting: VecDeque::new(), counted };
    finish_join(app, Backend::Remote(Box::new(link)), Some(net), first, ClientMap(joined.map), damage, join);
    Ok(())
}

/// Single-player: what a client gets on joining (its map copy, the weapons, every damaged edge, a
/// first snapshot), from the world in this process.
fn join_local(app: &mut App, mut server: LocalServer, player_id: u32) {
    // `--god` / `--ignored`: as if F1 / F2 were pressed at once.
    let options = app.world().resource::<Options>();
    if options.god || options.ignored || options.weapons_demo {
        server.send(Intent::Dev { player: player_id, god: (options.god || options.weapons_demo).then_some(true), ignored: options.ignored.then_some(true) });
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
    app.insert_resource(ServerLink { server, net, prev: first.clone(), curr: first, sim_ms: 0.0, chasing: 0, tiers: [0; 3], private, heard: Vec::new(), speech: Vec::new(), felled: Vec::new(), everyone: Vec::new() })
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
        (Self { server: Backend::Local(server), net: None, prev: first.clone(), curr: first, sim_ms: 0.0, chasing: 0, tiers: [0; 3], private, heard: Vec::new(), speech: Vec::new(), felled: Vec::new(), everyone: Vec::new() }, id)
    }

    /// Tests: what a client joining as `player` keeps of the world's config.
    #[cfg(test)]
    pub fn join_info_for_test(&self, player: u32) -> JoinInfo {
        let Backend::Local(server) = &self.server else { panic!("a local world") };
        join_info(&server.world.config, player, server.world.player_spawn)
    }

    /// Tests: one tick of the world in this process, taken up as `tick_server` does (snapshots,
    /// private views).
    #[cfg(test)]
    pub fn step_for_test(&mut self) {
        let Backend::Local(server) = &mut self.server else { panic!("a local world") };
        let snap = server.tick();
        self.keep_private(&snap.private);
        self.prev = std::mem::replace(&mut self.curr, snap);
    }

    /// Tests: another player joins the world in this process at `at`.
    #[cfg(test)]
    pub fn join_for_test(&mut self, at: [f32; 2]) -> u32 {
        let Backend::Local(server) = &mut self.server else { panic!("a local world") };
        server.join(Some(at))
    }

    /// Tests: one tick of the world in this process; the snapshot `player`'s client gets.
    #[cfg(test)]
    pub fn tick_for_test(&mut self, player: u32) -> Snapshot {
        let Backend::Local(server) = &mut self.server else { panic!("a local world") };
        let full = server.tick();
        full.for_local(player, &server.world)
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
    if !link.speech.is_empty() {
        link.speech.clear();
    }
    if !link.felled.is_empty() {
        link.felled.clear();
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
    // As the resource, so it only counts as changed when an edge really is (the fog of war casts
    // its rays again when the map changes).
    map: &mut ResMut<ClientMap>,
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
    // Barricades first, so an edge turning boarded already has what's under its planks.
    for b in &snapshot.barricade_changes {
        let key = zombye_sim::map::EdgeRef::new(b.tile, b.edge).key();
        match b.barricade {
            Some(r) => map.0.barricades.insert(key, r),
            None => map.0.barricades.remove(&key),
        };
    }
    for c in &snapshot.edge_changes {
        transitions.0.push((*c, map.0.owned_edge(c.tile, c.edge)));
        map.0.set_owned_edge(c.tile, c.edge, c.kind);
        changes.0.push(*c);
    }
    for b in &snapshot.blind_changes {
        map.0.set_blind(zombye_sim::map::EdgeRef::new(b.tile, b.edge), b.blind);
    }
    // Trees felled: stumps on our map copy (walking past them predicts as the server moves).
    for t in &snapshot.tree_changes {
        if let Some(tile) = map.0.tile_mut(t.tile) {
            tile.feature = t.feature;
            link.felled.push(t.tile);
        }
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
    link.speech.extend(snapshot.speech.iter().cloned());
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
