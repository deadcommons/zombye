---
title: Weapons
description: items/weapons.ron, how every weapon swings.
---

`items/weapons.ron` says how each melee weapon behaves when swung. Every weapon also needs an item in
[`items.ron`](/docs/modding/items/) with `weapon: Some("its id")`: the weapon is how it swings, the item is the thing
you carry.

Zombies have about 100 hit points (85 to 115). Distances are in metres, times in seconds, chances and endurance from 0
to 1.

```rust
(
    id: "kitchen_knife",
    name: "Kitchen knife",
    category: ShortBlade,
    damage: (17.0, 25.0),
    range: 0.65,
    arc_degrees: 40.0,
    windup_seconds: 0.12,
    swing_seconds: 0.4,
    max_targets: 1,
    knockback: 0.05,
    stagger_chance: 0.15,
    knockdown_chance: 0.0,
    crit_chance: 0.22,
    crit_multiplier: 2.6,
    endurance_cost: 0.03,
    noise_radius: 2.5,
    spawn_weight: 3.0,
    found_in: ["kitchen", "dining"],
    length: 0.3,
    color: (0.75, 0.78, 0.82),
),
```

## Fields

| Field | What it is |
|---|---|
| `id`, `name` | As for items. |
| `category` | `Unarmed`, `ShortBlade`, `ShortBlunt`, `LongBlunt` or `Axe`. Decides the swing animation and the skill. |
| `skill` | `Some(Bludgeon)`, `Some(Edged)` or `Some(Brawl)`. Left out, the category's: blunt trains Bludgeon, blades and axes Edged, bare hands Brawl. Every weapon of a category must name the same one. |
| `damage` | `(min, max)` per hit, rolled evenly. |
| `range` | Reach from the player's centre to the edge of a zombie's body. |
| `arc_degrees` | How wide the swing is. |
| `windup_seconds` | From starting the swing to the moment it lands. |
| `swing_seconds` | From starting the swing until you can act again (at least the windup). |
| `max_targets` | How many zombies one swing can hit, nearest first. Each extra one takes less. |
| `knockback` | How far a hit pushes a zombie back. |
| `stagger_chance`, `knockdown_chance` | Rolled on each hit: knocked down, else staggered. Both are likelier from behind. |
| `crit_chance`, `crit_multiplier` | Critical hits. Likelier from behind. |
| `endurance_cost` | Endurance one swing uses. Walking gives back 0.05 a second, so cost ÷ swing time above that wears you down. |
| `noise_radius` | How far zombies hear the swing. |
| `spawn_weight` | How likely it is to be left lying in a house (0, the default: never). |
| `found_in` | Room types where it's three times as likely to be lying about. |
| `length`, `color` | Its grey-box look. |

Exactly one weapon must be `Unarmed`: bare hands, what a player with nothing in hand punches with.

## Feel

Our own weapons follow a pattern that's a good place to start:

- **Knives:** fast, short, one target, high crits. Go for the back.
- **Short blunt** (hammer, pan): staggers.
- **Long blunt** (bat, club, plank): a wide arc that hits several and knocks down.
- **Axes:** slow, heavy, tiring. Two hits, or one good one.

Swings get slower and weaker below half endurance, and skills change damage, cost and speed (see
[How the world works](/docs/play/world/#skills)).

## Testing weapons

The MCP `weapons` tool prints the table as the game read it. To feel a weapon without walking about, have an agent
spawn it and a few zombies, then `attack`: each reply says what was hit, for how much, and what happened to it. See
[Testing and debugging](/docs/agents/testing/).
