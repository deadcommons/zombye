---
title: Zombie population
description: population.ron, where zombies live, whether they come back, and how noise draws them in.
---

`population.ron` decides how many zombies there are and where, whether emptied places fill up again, and how loud
events pull the far horde toward them. Every field is optional; left out, it takes the default shown in the shipped
file.

## Density

```rust
density: (
    town_core: 40000.0,
    street: 32000.0,
    yard: 19000.0,
    indoors: 10000.0,
    forest_road: 7000.0,
    forest_edge: 2600.0,
    deep_forest: 650.0,
),
rooms: { "kitchen": 1.3, "living": 1.3, "bedroom": 0.8, "bathroom": 0.6 },
scale: 1.0,
```

Zombies per square kilometre of open ground (not walls, trees or furniture) in each zone:

| Zone | Where |
|---|---|
| `town_core` | The civic and commercial blocks along the main street, and the street itself. |
| `street` | Roads, parking lots, sidewalks. |
| `yard` | Gardens and lots. |
| `indoors` | Any room, times the `rooms` multiplier for its type. |
| `forest_road` | Roads out of town. |
| `forest_edge` | Forest within `forest_edge_width` metres of town. |
| `deep_forest` | The rest. |

`scale` multiplies everything (a sandbox "population" setting). `core_fraction` shrinks the town core along the street.
On the default map this gives about 2,950 zombies. `--zombies N` on the command line sets the total instead and keeps
the proportions.

**Speed:** `fast_share` of them (one in ten) move at `fast_speed`, the rest at `slow_speed`, in metres a second.

## Respawn

```rust
respawn: (
    enabled: false,
    interval_secs: 120.0,
    min_distance: 250.0,
    seen_distance: 100.0,
    unseen_secs: 1800.0,
    top_up_below: 0.75,
    max_per_region: 6,
    max_per_check: 60,
),
```

Off by default. When on, every `interval_secs` the map's 64 m regions are checked: one at least `min_distance` from
every player, that nobody has been within `seen_distance` of for `unseen_secs`, holding less than `top_up_below` of its
share, is topped back up, a few at a time. Never in or next to the house you wake up in.

## Migration toward noise

```rust
migration: (
    enabled: true,
    min_radius: 25.0,
    reach_mult: 4.0,
    max_reach: 700.0,
    indoor_mult: 0.6,
    life_secs: 600.0,
    lasting_secs: 1800.0,
    spread_speed: 3.0,
    join_chance: 0.5,
    gather_radius: 12.0,
    merge_radius: 24.0,
    max_attractors: 16,
),
```

A sound at least `min_radius` metres loud (breaking a door is 26, smashing glass 22) leaves an **attractor**. Its pull
spreads outward at `spread_speed` metres a second to `reach_mult` times the sound's radius (at most `max_reach`, and
`indoor_mult` of that from indoors), and fades over `life_secs`. Far zombies wandering in a region that feels the pull
set off toward it a few at a time, so a big noise brings the horde in over minutes, not at once. Loud sounds close
together (`merge_radius`) strengthen one attractor instead of making another.

## Watching it

The MCP `regions` tool draws heat maps of living zombies and targets per region, the pull of every attractor, and the
regions furthest from their targets. `make_noise {x, y, radius: 120}` and a few minutes of `step_ticks` show a horde
arriving. `stats` gives the totals.
