//! Connection to the authoritative server. In single-player that's an in-process
//! [`LocalServer`] ticked at the sim rate in `FixedUpdate`; rendering interpolates between the last
//! two snapshots. A network transport will slot in behind the same resource later (M6).
//!
//! On "join" the client gets its player id, a copy of the map ([`ClientMap`]) and the weapon
//! definitions, and from then on only the map edits carried in snapshots, exactly as a remote
//! client will.

use bevy::prelude::*;
use std::collections::HashMap;

use zombye_net::{EdgeDamage, Intent, LocalServer, PrivateView, Snapshot, content, save_file};
use zombye_sim::map::{Edge, EdgeChange, OwnedEdge, Side, TileMap};
use zombye_sim::{CombatEvent, CombatTuning, ItemDef, ItemNote, TICK_RATE, WeaponDef};

use crate::Options;

#[derive(Resource)]
pub struct ServerLink {
    server: LocalServer,
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
        self.server.send(intent);
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
    /// Combat reach and arcs, for choosing between shove and stomp.
    pub combat: CombatTuning,
}

/// The `FixedUpdate` system that ticks the server. Input is sent before it and reconciled after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerTick;

/// Map edits received this frame, for renderers (doors).
#[derive(Resource, Default)]
pub struct EdgeChangesThisFrame(pub Vec<EdgeChange>);

/// Combat events (hits, kills, shoves, stomps, wounds) received this frame, for hit feedback.
#[derive(Resource, Default)]
pub struct CombatEventsThisFrame(pub Vec<CombatEvent>);

/// What came of our item requests this frame (done, or why not), for the HUD.
#[derive(Resource, Default)]
pub struct ItemNotesThisFrame(pub Vec<ItemNote>);

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

pub struct ServerLinkPlugin;

impl Plugin for ServerLinkPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().resource::<Options>().clone();
        let mut config = content::load_config(&options.data, options.seed).unwrap_or_else(|e| panic!("failed to load content: {e}"));
        // Without --zombies, as many as the population file's densities give for this map.
        config.zombies = options.zombies;
        if let Some(chunks) = options.map_chunks {
            config = config.with_map_chunks(chunks);
        }
        if let Some(hour) = options.start_hour {
            config.start_hour = hour;
        }
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
            return finish_join(app, server, player_id);
        }
        // The fight demo wants room to swing: open ground near the house you wake up in.
        let demo_at = if options.fight_demo && options.at.is_none() { open_ground_near(&server.world.map, server.world.player_spawn).map(|p| p.to_array()) } else { None };
        let siege_at = if options.siege { siege_spot(&server.world.map, server.world.spawn_building) } else { None };
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
        finish_join(app, server, player_id);
    }
}

/// What a client gets on joining (its map copy, the weapons, every damaged edge, a first
/// snapshot), and the systems that tick the server.
fn finish_join(app: &mut App, mut server: LocalServer, player_id: u32) {
    // `--god` / `--ignored`: as if F1 / F2 were pressed at once.
    let options = app.world().resource::<Options>();
    if options.god || options.ignored {
        server.send(Intent::Dev { player: player_id, god: options.god.then_some(true), ignored: options.ignored.then_some(true) });
    }
    let first = Snapshot::capture(&mut server.world);
    let map = ClientMap(server.world.map.clone());
    let damage = EdgeDamageMap(EdgeDamage::all(&server.world).into_iter().map(|d| ((d.tile, d.edge), d.hp)).collect());
    let spawn = first.players.iter().find(|p| p.id == player_id).map(|p| Vec2::from(p.pos)).unwrap_or(server.world.player_spawn);
    let join = JoinInfo {
        player_id,
        player_spawn: spawn,
        weapons: server.world.config.weapons.clone(),
        items: server.world.config.items.clone(),
        combat: server.world.config.combat.clone(),
    };
    let private = first.private.clone();
    app.insert_resource(ServerLink { server, prev: first.clone(), curr: first, sim_ms: 0.0, chasing: 0, tiers: [0; 3], private })
        .insert_resource(map)
        .insert_resource(join)
        .insert_resource(damage)
        .init_resource::<EdgeChangesThisFrame>()
        .init_resource::<EdgeDamageThisFrame>()
        .init_resource::<CombatEventsThisFrame>()
        .init_resource::<ItemNotesThisFrame>()
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
        (Self { server, prev: first.clone(), curr: first, sim_ms: 0.0, chasing: 0, tiers: [0; 3], private }, id)
    }

    /// The world being served (to save it).
    pub fn world(&self) -> &zombye_sim::World {
        &self.server.world
    }

    /// Serve `world` (a loaded save) from now on: drops queued intents and starts the snapshots
    /// afresh from it. Returns the first snapshot.
    pub fn replace_world(&mut self, world: zombye_sim::World) -> &Snapshot {
        self.server.replace_world(world);
        let first = Snapshot::capture(&mut self.server.world);
        let s = self.server.world.stats;
        (self.sim_ms, self.chasing, self.tiers) = (0.0, 0, [s.active, s.near, s.virtual_]);
        self.private = first.private.clone();
        self.prev = first.clone();
        self.curr = first;
        &self.curr
    }
}

fn clear_frame_changes(mut changes: ResMut<EdgeChangesThisFrame>, mut damage: ResMut<EdgeDamageThisFrame>, mut hits: ResMut<CombatEventsThisFrame>, mut notes: ResMut<ItemNotesThisFrame>) {
    changes.0.clear();
    damage.0.clear();
    hits.0.clear();
    notes.0.clear();
}

#[allow(clippy::too_many_arguments)]
fn tick_server(
    mut link: ResMut<ServerLink>,
    mut map: ResMut<ClientMap>,
    mut changes: ResMut<EdgeChangesThisFrame>,
    mut damage_map: ResMut<EdgeDamageMap>,
    mut damage: ResMut<EdgeDamageThisFrame>,
    mut hits: ResMut<CombatEventsThisFrame>,
    mut notes: ResMut<ItemNotesThisFrame>,
    join: Res<JoinInfo>,
) {
    // A remote server sends us only our own private view and notes; do as it will.
    let snapshot = link.server.tick().for_player(join.player_id);
    link.keep_private(&snapshot.private);
    notes.0.extend(snapshot.item_notes.iter().cloned());
    link.sim_ms = link.server.world.stats.tick_ms;
    link.chasing = link.server.world.stats.chasing;
    let s = link.server.world.stats;
    link.tiers = [s.active, s.near, s.virtual_];
    for c in &snapshot.edge_changes {
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
    link.prev = std::mem::replace(&mut link.curr, snapshot);
}

/// `--siege`: where you start (outside the house you woke up in, a few metres out from one of its
/// shut doors), the door's tile inside, and the way out through it. Doors in the south and east
/// walls come first: those face the default camera.
fn siege_spot(map: &TileMap, house: usize) -> Option<(Vec2, IVec2, IVec2)> {
    let b = &map.buildings[house];
    let ours = |t: IVec2| map.tile(t).room != 0 && map.rooms[map.tile(t).room as usize].building as usize == house;
    for side in [Side::S, Side::E, Side::N, Side::W] {
        let out = side.offset();
        for y in b.min.y..b.min.y + b.size.y {
            for x in b.min.x..b.min.x + b.size.x {
                let t = IVec2::new(x, y);
                let clear = (1..=SIEGE_DISTANCE + 1).all(|k| {
                    let c = t + out * k;
                    map.tile(c).room == 0 && !map.tile_blocked(c) && (k == 1 || Side::ALL.iter().all(|&s| map.edge(c, s) == Edge::None))
                });
                if ours(t) && map.edge(t, side) == Edge::DoorClosed && clear {
                    return Some(((t + out * SIEGE_DISTANCE).as_vec2() + Vec2::splat(0.5), t, out));
                }
            }
        }
    }
    None
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

/// `--siege`: how far out from the door you start (tiles), and how many zombies batter it.
const SIEGE_DISTANCE: i32 = 10;
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
