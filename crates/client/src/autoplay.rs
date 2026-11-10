//! `--autoplay`: the local player fights by itself, so combat can be watched and screenshotted
//! without a keyboard (CI, agents, verification). It only does what a player could: it sets the
//! movement input and facing the normal input path sends, and sends the same action intents as
//! the mouse and Space.
//!
//! Each tick it picks the nearest zombie still standing, turns to it and closes in (running when far
//! and rested). In reach it swings; crowded by two or more it alternates shoves and swings; a
//! downed zombie at its feet gets stomped. Dead, it gets up again after a few seconds. Holding a
//! gun (#72) it stands off instead and shoots the nearest it has a clear line to, reloading when
//! the gun is empty.

use bevy::prelude::*;
use zombye_net::Intent;
use zombye_sim::ai::ZOMBIE_RADIUS;
use zombye_sim::ranged::{MUZZLE_HEIGHT, flight};
use zombye_sim::{ActionKind, JobKind, Place, Slot, TICK_DT, ZombieAnim};

use crate::Options;
use crate::player::{LocalPlayer, PlayerInputSet};
use crate::server_link::{ClientMap, JoinInfo, ServerLink};

/// Movement chosen by autoplay for this tick; the player's input system uses it when `active`.
#[derive(Resource, Default)]
pub struct AutoInput {
    pub active: bool,
    pub move_dir: Vec2,
    pub run: bool,
}

/// Ignore zombies farther than this.
const SIGHT: f32 = 40.0;
/// Seconds lying dead before getting up again.
const RESPAWN_AFTER: f32 = 4.0;

pub struct AutoplayPlugin;

impl Plugin for AutoplayPlugin {
    fn build(&self, app: &mut App) {
        let active = app.world().resource::<Options>().autoplay;
        app.insert_resource(AutoInput { active, ..default() });
        if active {
            app.add_systems(FixedUpdate, autoplay.before(PlayerInputSet));
        }
    }
}

fn autoplay(
    join: Res<JoinInfo>,
    mut link: ResMut<ServerLink>,
    mut me: ResMut<LocalPlayer>,
    mut auto: ResMut<AutoInput>,
    time: Res<Time<Virtual>>,
    mut dead_for: Local<f32>,
    mut shoved_last: Local<bool>,
    mut reload_wait: Local<f32>,
    map: Res<ClientMap>,
) {
    let now = time.elapsed_secs();
    auto.move_dir = Vec2::ZERO;
    auto.run = false;
    if me.dead {
        *dead_for += TICK_DT;
        if *dead_for >= RESPAWN_AFTER {
            *dead_for = 0.0;
            link.send(Intent::Respawn { player: me.id });
        }
        return;
    }
    *dead_for = 0.0;
    let pos = me.curr.pos;
    let c = &join.combat;
    let weapon = &join.weapons[me.weapon.map_or_else(|| join.weapons.iter().position(|w| w.length <= 0.0).unwrap_or(0), |w| w as usize)];
    let reach = weapon.range + ZOMBIE_RADIUS;

    let mut nearest: Option<(f32, Vec2)> = None;
    let mut downed: Option<(f32, Vec2)> = None;
    let mut crowding = 0;
    for z in &link.curr.zombies {
        let to = Vec2::from(z.pos) - pos;
        let d = to.length();
        if d > SIGHT {
            continue;
        }
        match z.anim {
            ZombieAnim::Dead => {}
            ZombieAnim::Down | ZombieAnim::GetUp => {
                if downed.is_none_or(|(best, _)| d < best) {
                    downed = Some((d, to));
                }
            }
            _ => {
                if d <= c.shove_range + ZOMBIE_RADIUS + 0.15 {
                    crowding += 1;
                }
                if nearest.is_none_or(|(best, _)| d < best) {
                    nearest = Some((d, to));
                }
            }
        }
    }
    // With PvP on, other survivors are targets too: standing ones to swing at, downed ones to stamp on.
    // (A server we joined says so in its rules; our own world, in its combat settings.)
    if link.net.as_ref().map_or(join.combat.pvp, |n| n.pvp) {
        for p in link.curr.players.iter().filter(|p| p.id != me.id && !p.dead) {
            let to = Vec2::from(p.pos) - pos;
            let d = to.length();
            if d > SIGHT {
                continue;
            }
            if p.knock.is_some_and(|k| k.down) {
                if downed.is_none_or(|(best, _)| d < best) {
                    downed = Some((d, to));
                }
            } else {
                if d <= c.shove_range + ZOMBIE_RADIUS + 0.15 {
                    crowding += 1;
                }
                if nearest.is_none_or(|(best, _)| d < best) {
                    nearest = Some((d, to));
                }
            }
        }
    }
    // Only ask when the server will take it (an action under way or still recovering).
    // On a multiplayer server the snapshot is behind us: go by our own latest action too.
    let busy = me.acting(&link, now);
    // With a gun (#72): stand off and shoot the nearest with a clear line (a downed one when
    // nobody stands), closing in on what is out of range or behind something; empty, reload,
    // backing away from whatever is close.
    *reload_wait = (*reload_wait - TICK_DT).max(0.0);
    if let Some(r) = &weapon.ranged {
        let Some((d, to, down)) = nearest.map(|(d, to)| (d, to, false)).or(downed.map(|(d, to)| (d, to, true))) else { return };
        let dir = to / d.max(1e-3);
        me.aim = to.y.atan2(to.x);
        me.pitch = ((if down { 0.2 } else { 1.2 } - MUZZLE_HEIGHT) / d.max(0.5)).atan();
        let own = link.own(me.id);
        if own.and_then(|v| v.inventory.hand).is_none_or(|g| g.used == 0) {
            let loading = own.is_some_and(|v| v.jobs.iter().any(|j| matches!(j, JobKind::Reload { .. })));
            if !loading && *reload_wait <= 0.0 {
                link.send(Intent::Use { player: me.id, from: Place::Slot(Slot::Hand), index: 0 });
                *reload_wait = 1.0;
            }
            if d < 6.0 {
                auto.move_dir = -dir;
            }
            return;
        }
        let clear = flight(&map.0, pos, dir, d, me.pitch.tan()).end >= d - 0.35;
        if clear && d <= (r.range * 0.45).min(16.0) {
            if !busy {
                me.act(&mut link, &join, ActionKind::Swing, now);
            }
        } else {
            auto.move_dir = dir;
            auto.run = d > 22.0 && me.curr.endurance > 0.5 && !me.curr.winded;
        }
        return;
    }
    // A downed zombie at our feet, and nothing standing right on top of us: finish it.
    if let Some((d, to)) = downed
        && d <= c.stomp_range * 0.9
        && nearest.is_none_or(|(n, _)| n > reach * 0.8)
    {
        me.aim = to.y.atan2(to.x);
        if !busy {
            me.act(&mut link, &join, ActionKind::Stomp, now);
        }
        return;
    }
    let Some((d, to)) = nearest.or(downed) else { return };
    me.aim = to.y.atan2(to.x);
    if crowding >= 2 && !*shoved_last {
        // Crowded: shove them back, then swing into the gap (alternating).
        if !busy {
            me.act(&mut link, &join, ActionKind::Shove, now);
            *shoved_last = true;
        }
    } else if d <= reach * 0.95 {
        if !busy {
            me.act(&mut link, &join, ActionKind::Swing, now);
            *shoved_last = false;
        }
    } else {
        // Close in, but stop just inside reach.
        auto.move_dir = to / d;
        auto.run = d > 6.0 && me.curr.endurance > 0.5 && !me.curr.winded;
    }
}
