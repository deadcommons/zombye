//! Fair information (#66, docs/PLAN.md §4.8): a client learns about another player only through
//! its own survivor's senses, so a modified client is no wallhack.
//!
//! [`Snapshot::through_senses`] cuts a client's snapshot (already cut to its area of interest by
//! [`Snapshot::for_client`]) down to what its survivor could know, by the world's own record of
//! who sees and heard whom (`sim::awareness`):
//!
//! - **Other players** are listed, with what they wear, only while our survivor sees them (view
//!   cone, daylight, their stance and Stealth, a line of sight) or did a moment ago (the grace
//!   period). One it only hears is a sound at a position ([`Snapshot::heard`]), not a player.
//! - **Hits** (blood, flinches, the sounds of blows) that name a player we don't see are left out,
//!   whoever was hit: the swing itself is heard, if it is in earshot, like any other noise.
//! - **Items on the ground** are listed while our survivor has a line of sight to them (in any
//!   direction: things don't blink as you turn) or can reach them, so something dropped or taken
//!   behind a wall says nothing until you look. The client hides items in the fog of war anyway.
//!
//! Zombies are not cut (they aren't players, and the client's fog hides the ones out of sight),
//! and neither are doors and windows: the client's map copy has to be right wherever it can walk.

use glam::Vec2;
use zombye_sim::World;
use zombye_sim::vision::line_of_sight;

use crate::Snapshot;

impl Snapshot {
    /// This snapshot, cut for `player`'s client, with what its survivor couldn't see or hear
    /// taken out. With no such player in it, nothing of the world is left but the zombies.
    pub fn through_senses(mut self, player: u32, world: &World) -> Snapshot {
        let aware = world.awareness();
        // A world that hasn't ticked with its players yet (just loaded, or its first snapshot)
        // has no record: go by the rule itself.
        let fresh = aware.now() == 0;
        let sees = |id: u32| id == 0 || aware.sees(player, id) || (fresh && world.sees_now(player, id));
        self.players.retain(|p| sees(p.id));
        self.outfits.retain(|o| sees(o.id));
        self.hits.retain(|h| sees(h.player) && sees(h.by));
        let me: Option<Vec2> = world.player(player).map(|p| p.state.pos);
        let floor = self.floor.iter().find(|f| f.player == player).map(|f| f.items.clone()).unwrap_or_default();
        self.items.retain(|i| floor.contains(&i.id) || me.is_some_and(|m| line_of_sight(&world.map, m, i.pos)));
        self.heard.retain(|h| h.listener == player);
        // What was said (#67): only what our survivor heard, and from whom only if it sees them.
        self.speech.retain(|s| s.listener == player);
        for s in &mut self.speech {
            if !s.seen {
                s.speaker = 0;
            }
        }
        self
    }

    /// What the single-player client takes from the world in its own process: its own private
    /// parts ([`Snapshot::for_player`]) and, with player against player on (`--pvp`, `--duel`, a
    /// save of such a world), other players only through its survivor's senses, as on a server.
    pub fn for_local(self, player: u32, world: &World) -> Snapshot {
        let own = self.for_player(player);
        if world.config.combat.pvp { own.through_senses(player, world) } else { own }
    }
}
