//! Lag-compensated melee (#23, `crate::lagcomp`): a blow lands where its attacker's client showed
//! the target, within the cap, against zombies and players; a trade of blows is settled by the
//! same rule for both sides; and none of it touches a world whose players don't report a view.

use super::pvp_tests::{duel, equip, pi_of};
use super::*;
use crate::PlayerInput;

/// 150 ms of ping as a client lives it: its snapshot cushion and both ends' ticks on top make
/// about 250 ms, 5 ticks.
const LAG_150: u64 = 5;

/// One tick with `id`'s client `lag` ticks behind (it says so before its input, as a client does).
fn step_lagged(w: &mut World, lagging: &[(u32, u64)]) {
    for &(id, lag) in lagging {
        w.player_saw(id, w.tick.saturating_sub(lag));
    }
    w.step();
}

/// The step (0 = the next) in which a `kind` asked for now by a fresh survivor holding `weapon` lands.
fn landing_step(weapon: &str, kind: ActionKind) -> usize {
    let (mut w, a, _) = duel(9, false);
    if weapon != "fists" {
        equip(&mut w, a, weapon);
    }
    match kind {
        ActionKind::Swing => w.player_attack(a),
        ActionKind::Shove => w.player_shove(a),
        ActionKind::Stomp => w.player_stomp(a),
    };
    for k in 0..100 {
        w.step();
        if !w.action_events.is_empty() {
            return k;
        }
    }
    panic!("never landed");
}

/// A swing of a bat at a zombie walking straight away (or straight in) at `per_tick` metres a
/// tick, `off` metres outside the bat's reach when the blow lands (negative: inside). The
/// attacker's client is `lag` ticks behind. Returns the zombies hit.
fn swing_at_walker(lag: u64, per_tick: f32, off: f32) -> u32 {
    let land = landing_step("baseball_bat", ActionKind::Swing);
    let (mut w, a, b) = duel(3, false);
    // The other survivor well out of it; ours unseen and unheard, so the zombie only does as told.
    let pb = pi_of(&w, b);
    w.players[pb].state.pos = Vec2::new(90.5, 90.5);
    w.set_dev(a, Some(true), Some(true));
    let bat = equip(&mut w, a, "baseball_bat");
    let reach = w.config.weapons[bat as usize].range + ZOMBIE_RADIUS;
    let origin = w.player(a).unwrap().state.pos;
    let z = w.add_zombie(origin + Vec2::new(reach, 0.0), 0.0, 0.0);
    w.zombies.wander_timer[z] = 1e9;
    // Where it stands during step `k` (the blow lands in step `land`).
    let at = |k: i64| origin + Vec2::new(reach + off - per_tick * (land as i64 - k) as f32, 0.0);
    // The world runs a while first, so there are ticks to look back on.
    for k in -60..0 {
        w.zombies.pos[z] = at(k);
        step_lagged(&mut w, &[(a, lag)]);
    }
    w.player_attack(a);
    for k in 0..=land as i64 {
        w.zombies.pos[z] = at(k);
        step_lagged(&mut w, &[(a, lag)]);
        // Until it is hit it only moves as told.
        let d = w.zombies.pos[z].distance(at(k));
        assert!(d < 1e-3 || !w.hit_events.is_empty(), "the zombie moved {d} m by itself in step {k}");
    }
    assert_eq!(w.action_events.len(), 1, "the blow landed in step {land}");
    w.action_events[0].hits
}

/// A zombie walking away at 3 m/s is 0.3 m out of reach on the server when the blow lands, but the
/// attacker's client, 150 ms of ping behind, showed it well in reach: it is hit. Without the lag it
/// is a miss, as before.
#[test]
fn a_blow_lands_where_a_lagging_attacker_saw_the_zombie() {
    assert_eq!(swing_at_walker(0, 0.15, 0.3), 0, "a local player's blow goes by where the zombie is");
    assert_eq!(swing_at_walker(LAG_150, 0.15, 0.3), 1, "at 150 ms the blow lands where the zombie was seen");
    // And the other way round: one walking in that the client still showed out of reach is a miss,
    // though the server already has it close enough. What you see is what you hit.
    assert_eq!(swing_at_walker(0, -0.15, -0.3), 1);
    assert_eq!(swing_at_walker(LAG_150, -0.15, -0.3), 0, "not hit where the attacker couldn't see it yet");
}

/// The rewind is capped: a client claiming to be two seconds behind is taken to be
/// `lag_comp_max_secs` behind, so a zombie that was in reach long ago is not hit.
#[test]
fn the_rewind_is_capped() {
    // 1.5 m out of reach now; 7 ticks ago (the cap) still 0.3 m out; 40 ticks ago it was in reach.
    assert_eq!(swing_at_walker(40, 0.15, 1.5), 0, "rewound past the cap");
    // Inside the cap the same claim does hit.
    assert_eq!(swing_at_walker(40, 0.15, 0.6), 1);
    let (w, ..) = duel(1, false);
    assert_eq!(w.lag_comp_ticks(), 7);
}

/// Nothing is kept, and nothing rewound, until a player's client reports what it shows: local
/// players (single-player, bots in the server's process) fight exactly as before. Once one does,
/// only the last few ticks are kept, and turning the cap to 0 turns it all off.
#[test]
fn local_players_are_untouched_and_the_history_is_short() {
    let (mut w, a, _) = duel(4, true);
    for _ in 0..30 {
        w.step();
    }
    assert!(w.lag_history().is_empty());
    assert_eq!(w.view_lag_secs(a), 0.0);
    for _ in 0..30 {
        step_lagged(&mut w, &[(a, 3)]);
    }
    assert_eq!(w.lag_history().len(), w.lag_comp_ticks() as usize + 1);
    assert!((w.view_lag_secs(a) - 4.0 * TICK_DT).abs() < 1e-4, "one more tick has gone by since it said: {}", w.view_lag_secs(a));
    // A client can't claim to see the future, or go back on what it has seen.
    w.player_saw(a, w.tick + 100);
    assert_eq!(w.player(a).unwrap().seen_tick, w.tick);
    w.player_saw(a, 3);
    assert_eq!(w.player(a).unwrap().seen_tick, w.tick);
    // Off: nothing kept.
    w.config.combat.lag_comp_max_secs = 0.0;
    w.step();
    assert!(w.lag_history().is_empty());
    assert_eq!(w.view_lag_secs(a), 0.0);
}

/// Player against player: the victim breaks into a run just before the blow lands. On the
/// attacker's client (150 ms) it still stood there, so the blow lands; the knockback then moves the
/// victim on from where it really is. For a local attacker it is a miss.
#[test]
fn a_blow_lands_where_a_lagging_attacker_saw_the_player() {
    let land = landing_step("baseball_bat", ActionKind::Swing);
    let run = |lag: u64| {
        let (mut w, a, b) = duel(5, true);
        equip(&mut w, a, "baseball_bat");
        // Just inside the bat's reach, facing away.
        let pb = pi_of(&w, b);
        w.players[pb].state.pos = Vec2::new(60.5 + 1.25 + PLAYER_RADIUS - 0.25, 60.5);
        w.players[pb].state.facing = 0.0;
        for _ in 0..20 {
            step_lagged(&mut w, &[(a, lag)]);
        }
        w.player_attack(a);
        let mut hits = Vec::new();
        let mut before = Vec2::ZERO;
        for k in 0..=land {
            if k + 3 == land {
                // Off it goes, three ticks before the blow.
                w.set_player_input(b, 1, PlayerInput { move_dir: Vec2::X, facing: 0.0, run: true, sneak: false, aim: false });
            }
            before = w.player(b).unwrap().state.pos;
            step_lagged(&mut w, &[(a, lag)]);
            hits.extend_from_slice(&w.player_hit_events);
        }
        (hits, before, w.player(b).unwrap().state.pos)
    };
    let (hits, ..) = run(0);
    assert!(hits.is_empty(), "a local attacker misses a player who got away");
    let (hits, before, after) = run(LAG_150);
    assert_eq!(hits.len(), 1, "at 150 ms the blow lands where the victim was seen");
    // The victim isn't pulled back to where it was seen: it goes on from where it is.
    assert!(after.x > before.x, "{before} -> {after}");
}

/// A trade of blows. B swings a bat; A's shove lands two ticks before B's blow would, and a shove
/// always staggers, which calls a swing off. For a local B that is that. For a B at 150 ms its
/// blow had already landed on its own screen when A's shove showed there, so it lands too; with
/// only one tick of lag it hadn't, and it is called off. The same rule whoever is A and B.
#[test]
fn a_trade_of_blows_goes_by_what_each_side_saw() {
    let swing = landing_step("baseball_bat", ActionKind::Swing);
    let shove = landing_step("fists", ActionKind::Shove);
    assert!(swing >= shove + 2);
    let trade = |lag_b: u64| {
        let (mut w, a, b) = duel(6, true);
        equip(&mut w, b, "baseball_bat");
        for _ in 0..20 {
            step_lagged(&mut w, &[(b, lag_b)]);
        }
        w.player_attack(b);
        let (mut by_a, mut by_b) = (0, 0);
        for k in 0..=swing + 2 {
            if k + 2 + shove == swing {
                w.player_shove(a);
            }
            step_lagged(&mut w, &[(b, lag_b)]);
            by_a += w.player_hit_events.iter().filter(|h| h.player == a).count();
            by_b += w.player_hit_events.iter().filter(|h| h.player == b).count();
        }
        // Either way B is reeling and has no swing left under way.
        assert!(w.player(b).unwrap().action.is_none());
        (by_a, by_b, w.state_hash())
    };
    assert_eq!(trade(0).0, 1);
    assert_eq!(trade(0).1, 0, "a local player's swing is called off by the shove");
    assert_eq!(trade(LAG_150).1, 1, "at 150 ms the swing had landed on its own screen: both blows count");
    assert_eq!(trade(LAG_150).0, 1);
    assert_eq!(trade(1).1, 0, "one tick behind, it hadn't");
    // Deterministic.
    assert_eq!(trade(LAG_150).2, trade(LAG_150).2);
}
