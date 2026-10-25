---
title: Loot and containers
description: containers.ron and loot.ron, what furniture holds and what's inside.
---

Two files decide what you find. `containers.ron` says which furniture can be searched and how big it is;
`loot.ron` says what each one holds, depending on the room it's in.

## Containers

```rust
containers: [
    (id: "fridge", name: "Fridge", room_l: 60.0, search_secs: 1.0, noise_m: 3.0),
    (id: "wardrobe", name: "Wardrobe", room_l: 80.0, search_secs: 2.0, noise_m: 4.0),
    // ...
],
furniture: [
    (furniture: "fridge", room: "any", container: "fridge"),
    (furniture: "sink", room: "bathroom", container: "bathroom_cabinet"),
    (furniture: "sink", room: "any", container: "under_sink"),
    (furniture: "wardrobe", room: "hall", container: "coat_cupboard"),
    // ...
],
```

- A **container** kind has its room in litres, how long the first search takes (half as long again when sneaking,
  which is quieter), and how far the noise of opening it carries.
- **`furniture`** maps a kind of furniture in a kind of room to a container. `room: "any"` is the fallback: a sink in a
  bathroom is a bathroom cabinet, a sink anywhere else an under-sink cupboard. Furniture with no entry (beds, sofas,
  tables) isn't a container.

## Loot tables

```rust
(room: "kitchen", container: "fridge", empty: 0.15, rolls: (1, 4), entries: [
    (item: "soda_can", weight: 3, count: (1, 4)),
    (item: "milk", weight: 3),
    (item: "apple", weight: 3, count: (1, 4)),
    (item: "bread", weight: 1),
]),
```

| Field | What it is |
|---|---|
| `room`, `container` | Which containers use it: a room type and a container kind, either may be `"any"`. |
| `empty` | Chance it holds nothing at all. |
| `rolls` | `(min, max)` picks. |
| `entries` | `item`, `weight` among the table's entries (default 1), `count` per pick (default `(1, 1)`), and `rare: true` for the few the rare-loot setting should touch. |
| `groups` | Extra lists rolled after the picks, each with a `name`, a `chance` and its own `rolls` and `entries`: the kitchen's cutlery drawer, the laundry's toolbox. |

**Which table a container uses:** the first of (room, container), (any, container), (room, any), (any, any). So a
fridge is a fridge wherever it stands, unless a room says otherwise; furniture with no table of its own (shelves, a
workbench) holds what its room holds. With no table at all, it's empty.

**Filling:** a container is filled the first time anyone opens it, from the world's seed and the container's id. It
holds the same whenever and in whatever order houses are searched. Picks of the same item add up; a pick that doesn't
fit is cut to what fits.

### Sandbox settings

At the top of `loot.ron`:

- `scale` multiplies every table's rolls: 0.25 is scarce, 2 is plenty.
- `rare_scale` multiplies the weight of `rare` entries: 0 means never.

### Room types

The shipped buildings use: `kitchen`, `bathroom`, `bedroom`, `living`, `dining`, `hall`, `laundry`, `porch`,
`garage`, `office`, and the shops' `shop`, `pharmacy` and `hardware`. A `shed` table is ready for a shed. A new room
type in a [building template](/docs/modding/buildings/) just needs tables here.

## Tuning without walking

The MCP `loot` tool is made for this. Give it a room and a container (or a furniture kind) and it says which table
applies, the chance of each item per pick, and then rolls it a few thousand times:

```text
loot {room: "garage", container: "shelf", n: 2000}

a shelves (shelves, 30 L) in a garage fills from the fallback (garage, any): empty 15%, rolls 2-5
  ...
2000 sample fills: 15% empty, on average 2.9 stacks, 3.4 L, 1.7 kg; item | in % of fills | average count when found:
    empty_bottle | 29.4% | 2.3
    rag | 28.8% | 2.3
    nails | 18.8% | 1.1
    claw_hammer | 14.1% | 1.1
    flashlight | 9.2% | 1.0
    crowbar | 8.4% | 1.1
    fire_axe | 3.8% | 1.0
```

(Some lines left out. This run was on the [nail bat](/docs/modding/first-mod/) mod's data.)

`containers_near` shows the real containers round a player and which table each will fill from.
