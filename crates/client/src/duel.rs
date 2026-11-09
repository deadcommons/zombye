//! `--duel`: player against player to watch, with no second keyboard (verification, pictures,
//! clips). PvP is on, and another survivor joins a few metres off with a crowbar and comes
//! at the local player by itself. It is a player like any other: this only sends the intents a
//! client would (its movement input, `Attack`, `Shove`, `Stomp`), so everything it does and
//! everything done to it goes through the server's rules. With `--autoplay` the local player
//! fights back (`autoplay.rs` takes other players as targets when PvP is on).

use bevy::prelude::*;
use zombye_net::{Intent, PlayerView};
use zombye_sim::players::PLAYER_RADIUS;
use zombye_sim::{CombatTuning, WeaponDef};

use crate::player::PlayerInputSet;
use crate::server_link::{JoinInfo, ServerLink};

/// How far east of the local player the other survivor starts.
pub const START_DISTANCE: f32 = 5.0;

/// The other survivor of a `--duel`, and the sequence number of its last input.
#[derive(Resource)]
pub struct Duel {
    pub foe: u32,
    pub seq: u32,
}

pub struct DuelPlugin;

impl Plugin for DuelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, drive.before(PlayerInputSet).run_if(resource_exists::<Duel>).run_if(crate::live));
    }
}

/// What the other survivor does this tick: where it walks, which way it faces, and what it asks for.
#[derive(Debug)]
pub struct Move {
    pub move_dir: Vec2,
    pub facing: f32,
    pub act: Option<Intent>,
}

/// One tick of the other survivor's fight against `them`: close in to its weapon's reach and swing,
/// shove now and then (every third blow, when close enough), stamp on them when they're down.
/// `tick` only spaces the shoves out.
pub fn decide(me: &PlayerView, them: &PlayerView, weapon: &WeaponDef, c: &CombatTuning, tick: u64) -> Move {
    let to = Vec2::from(them.pos) - Vec2::from(me.pos);
    let d = to.length();
    let facing = if d > 1e-3 { to.y.atan2(to.x) } else { me.facing };
    let toward = if d > 1e-3 { to / d } else { Vec2::ZERO };
    if them.dead {
        return Move { move_dir: Vec2::ZERO, facing, act: None };
    }
    let can_act = me.action.is_none() && me.ready_in <= 0.0 && me.knock.is_none();
    let down = them.knock.is_some_and(|k| k.down);
    let (reach, act) = if down {
        (c.stomp_range * 0.8, Intent::Stomp { player: me.id })
    } else if (tick / 20).is_multiple_of(3) {
        (c.shove_range + PLAYER_RADIUS - 0.1, Intent::Shove { player: me.id })
    } else {
        ((weapon.range + PLAYER_RADIUS) * 0.9, Intent::Attack { player: me.id })
    };
    if d > reach {
        Move { move_dir: toward, facing, act: None }
    } else {
        Move { move_dir: Vec2::ZERO, facing, act: can_act.then_some(act) }
    }
}

fn drive(join: Res<JoinInfo>, mut link: ResMut<ServerLink>, mut duel: ResMut<Duel>) {
    let (Some(me), Some(them)) = (link.curr.players.iter().find(|p| p.id == duel.foe).copied(), link.curr.players.iter().find(|p| p.id == join.player_id).copied()) else { return };
    if me.dead {
        return;
    }
    let unarmed = join.weapons.iter().position(|w| w.length <= 0.0).unwrap_or(0);
    let weapon = &join.weapons[me.weapon.map_or(unarmed, |w| w as usize)];
    let m = decide(&me, &them, weapon, &join.combat, link.curr.tick);
    duel.seq += 1;
    // Aiming all the while: only an aiming player fights.
    link.send(Intent::PlayerInput { player: me.id, seq: duel.seq, move_dir: m.move_dir.to_array(), facing: m.facing, run: false, sneak: false, aim: true });
    if let Some(act) = m.act {
        link.send(act);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zombye_sim::{BuildingTemplate, WorldConfig};

    /// The other survivor of a duel, driven only by `decide` and intents, walks up to a player who
    /// stands still and beats it to death; with PvP off the same intents never hurt it.
    #[test]
    fn the_duel_bot_fights_through_intents() {
        for pvp in [true, false] {
            let templates = vec![BuildingTemplate::from_ron(include_str!("../../../assets/data/buildings/bungalow.ron")).unwrap()];
            let weapons = zombye_sim::items::parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
            let items = zombye_sim::items::parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap();
            let mut config = WorldConfig::new(4, 0, templates).with_weapons(weapons).with_items(items).with_pvp(pvp);
            config.needs = zombye_sim::NeedsTuning::off();
            let mut server = zombye_net::LocalServer::new(config);
            let id = server.join(None);
            let at = server.world.player(id).unwrap().state.pos;
            let foe = server.join(Some((at + Vec2::new(1.5, 0.0)).to_array()));
            assert!(server.world.hold_item(foe, zombye_sim::Slot::Hand, "kitchen_knife"));
            let mut snap = server.tick();
            let (mut seq, mut shoved, mut stomped) = (0, false, false);
            for _ in 0..20 * 120 {
                let view = |p: u32| *snap.players.iter().find(|v| v.id == p).unwrap();
                let (me, them) = (view(foe), view(id));
                let weapon = &server.world.config.weapons[me.weapon.unwrap() as usize];
                let m = decide(&me, &them, weapon, &server.world.config.combat, snap.tick);
                seq += 1;
                server.send(Intent::PlayerInput { player: foe, seq, move_dir: m.move_dir.to_array(), facing: m.facing, run: false, sneak: false, aim: true });
                shoved |= matches!(m.act, Some(Intent::Shove { .. }));
                stomped |= matches!(m.act, Some(Intent::Stomp { .. }));
                if let Some(act) = m.act {
                    server.send(act);
                }
                snap = server.tick();
                if view_dead(&snap, id) {
                    break;
                }
            }
            let v = server.world.player(id).unwrap();
            if pvp {
                assert!(v.dead, "two minutes of knife and boots left it at {} HP", v.hp);
                let text = v.run.cause_of_death_with(&server.world.config.weapons).unwrap();
                assert!(text.contains("survivor") || text.starts_with("Bled out"), "{text}");
                assert!(shoved, "it never shoved");
                assert_eq!(server.world.player(foe).unwrap().run.player_kills, u32::from(text.contains("survivor")));
            } else {
                assert!(!v.dead && v.health.wounds.is_empty() && v.knock.is_none());
                assert!(shoved && !stomped);
            }
        }
    }

    fn view_dead(snap: &zombye_net::Snapshot, id: u32) -> bool {
        snap.players.iter().any(|p| p.id == id && p.dead)
    }
}
