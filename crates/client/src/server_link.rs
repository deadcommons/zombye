//! Connection to the authoritative server. In single-player that's an in-process
//! [`LocalServer`] ticked at the sim rate in `FixedUpdate`; rendering interpolates between the last
//! two snapshots. A network transport will slot in behind the same resource later (M6).
//!
//! On "join" the client gets its player id, a copy of the map ([`ClientMap`]) and the weapon
//! definitions, and from then on only the map edits carried in snapshots, exactly as a remote
//! client will.

use bevy::prelude::*;
use std::collections::HashMap;

use zombye_net::{EdgeDamage, Intent, LocalServer, Snapshot, content};
use zombye_sim::map::{Edge, EdgeChange, OwnedEdge, Side, TileMap};
use zombye_sim::{CombatEvent, CombatTuning, TICK_RATE, WeaponDef, WorldConfig};

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
}

impl ServerLink {
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
        let templates = content::load_building_templates(&content::default_data_dir()).unwrap_or_else(|e| panic!("failed to load content: {e}"));
        let weapons = content::load_weapons(&content::default_data_dir()).unwrap_or_else(|e| panic!("failed to load content: {e}"));
        let population = content::load_population(&content::default_data_dir()).unwrap_or_else(|e| panic!("failed to load content: {e}"));
        let mut config = WorldConfig::new(options.seed, 0, templates).with_weapons(weapons).with_population(population);
        // Without --zombies, as many as the population file's densities give for this map.
        config.zombies = options.zombies;
        if let Some(chunks) = options.map_chunks {
            config = config.with_map_chunks(chunks);
        }
        if let Some(hour) = options.start_hour {
            config.start_hour = hour;
        }
        let mut server = LocalServer::new(config);
        // The fight demo wants room to swing: open ground near the house you wake up in.
        let demo_at = if options.fight_demo && options.at.is_none() { open_ground_near(&server.world.map, server.world.player_spawn).map(|p| p.to_array()) } else { None };
        let siege_at = if options.siege { siege_spot(&server.world.map, server.world.spawn_building) } else { None };
        let player_id = server.join(options.at.or(siege_at.map(|s| s.0.to_array())).or(demo_at));
        if options.fight_demo {
            // A bat in hand and a small group just ahead (the player faces +x), called over by a noise.
            let me = server.world.player(player_id).map(|p| p.state.pos).unwrap_or(server.world.player_spawn);
            server.send(Intent::SpawnItem { weapon: "baseball_bat".into(), pos: me.to_array() });
            server.send(Intent::PickUp { player: player_id });
            server.send(Intent::SpawnZombies { count: DEMO_ZOMBIES, center: (me + Vec2::X * DEMO_DISTANCE).to_array(), radius: 2.0 });
            server.send(Intent::MakeNoise { pos: me.to_array(), radius: 12.0, lasting: false });
        }
        if let Some((_, door, out)) = siege_at {
            siege(&mut server, player_id, door, out);
        }
        let first = Snapshot::capture(&mut server.world);
        let map = ClientMap(server.world.map.clone());
        let damage = EdgeDamageMap(EdgeDamage::all(&server.world).into_iter().map(|d| ((d.tile, d.edge), d.hp)).collect());
        let spawn = first.players.iter().find(|p| p.id == player_id).map(|p| Vec2::from(p.pos)).unwrap_or(server.world.player_spawn);
        let join = JoinInfo { player_id, player_spawn: spawn, weapons: server.world.config.weapons.clone(), combat: server.world.config.combat.clone() };
        app.insert_resource(ServerLink { server, prev: first.clone(), curr: first, sim_ms: 0.0, chasing: 0, tiers: [0; 3] })
            .insert_resource(map)
            .insert_resource(join)
            .insert_resource(damage)
            .init_resource::<EdgeChangesThisFrame>()
            .init_resource::<EdgeDamageThisFrame>()
            .init_resource::<CombatEventsThisFrame>()
            .insert_resource(Time::<Fixed>::from_hz(TICK_RATE as f64))
            .add_systems(First, clear_frame_changes)
            .add_systems(FixedUpdate, tick_server.in_set(ServerTick));
    }
}

fn clear_frame_changes(mut changes: ResMut<EdgeChangesThisFrame>, mut damage: ResMut<EdgeDamageThisFrame>, mut hits: ResMut<CombatEventsThisFrame>) {
    changes.0.clear();
    damage.0.clear();
    hits.0.clear();
}

fn tick_server(
    mut link: ResMut<ServerLink>,
    mut map: ResMut<ClientMap>,
    mut changes: ResMut<EdgeChangesThisFrame>,
    mut damage_map: ResMut<EdgeDamageMap>,
    mut damage: ResMut<EdgeDamageThisFrame>,
    mut hits: ResMut<CombatEventsThisFrame>,
) {
    let snapshot = link.server.tick();
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
    server.send(Intent::SpawnItem { weapon: "baseball_bat".into(), pos: me.to_array() });
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
