//! `--autoplay`: the local player fights by itself, so combat can be watched and screenshotted
//! without a keyboard (CI, agents, verification). It only does what a player could: it sets the
//! movement input and facing the normal input path sends, and sends the same action intents as
//! the mouse and Space.
//!
//! Each tick it picks the nearest zombie still standing, turns to it and closes in (running when far
//! and rested). In reach it swings; crowded by two or more it alternates shoves and swings; a
//! downed zombie at its feet gets stomped. Dead, it gets up again after a few seconds.

use bevy::prelude::*;
use zombye_net::Intent;
use zombye_sim::ai::ZOMBIE_RADIUS;
use zombye_sim::{ActionKind, TICK_DT, ZombieAnim};

use crate::Options;
use crate::player::{LocalPlayer, PlayerInputSet};
use crate::server_link::{JoinInfo, ServerLink};

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
