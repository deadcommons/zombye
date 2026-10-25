---
title: Buildings
description: buildings/*.ron, house, shop and office layouts drawn in text.
---

Every building in the town comes from a template in `buildings/`, one file each. Add a file and the town generator
starts using it on lots it fits. Templates are drawn as text, one character per one-metre tile, and the generator
turns and mirrors them to face their street.

```rust
// Small one-storey bungalow. One character per tile; "." is outside.
// The bottom row is the front of the house and faces the street.
// Openings are (x, y, side): side N/E/S/W of tile (x, y).
BuildingTemplate(
    id: "house_bungalow",
    name: "Small bungalow",
    kind: House,
    roof: GableX,
    rooms: {
        'B': (name: "Bedroom", room_type: "bedroom", floor: Carpet),
        'b': (name: "Bathroom", room_type: "bathroom", floor: TileFloor),
        'H': (name: "Hall", room_type: "hall", floor: Wood),
        'K': (name: "Kitchen", room_type: "kitchen", floor: TileFloor),
        'L': (name: "Living room", room_type: "living", floor: Wood),
    },
    levels: [
        (
            layout: [
                "BBBBbbbKKK",
                "BBBBbbbKKK",
                "BBBBbbbKKK",
                "BBBBHHHKKK",
                "LLLLHHHKKK",
                "LLLLLLLKKK",
                "LLLLLLLKKK",
                "LLLLLLLKKK",
            ],
            doors: [(2, 7, S), (8, 0, N), (3, 3, E), (5, 2, S)],
            arches: [(5, 4, S), (6, 6, E)],
            windows: [(0, 1, W), (2, 0, N), (5, 0, N), (9, 2, E), (9, 5, E), (8, 7, S), (0, 6, W), (4, 7, S)],
            furniture: [
                (kind: "bed", at: (1, 0), size: (2, 2)),
                (kind: "wardrobe", at: (3, 0)),
                (kind: "fridge", at: (7, 0)),
                (kind: "counter", at: (9, 0), size: (1, 2), facing: W),
                (kind: "sofa", at: (0, 5), size: (1, 2), facing: E),
                // ...
            ],
        ),
    ],
)
```

## How it's read

- **Coordinates:** x goes right, y goes down, `(0, 0)` is the top-left tile. **The bottom row is the front** and
  faces the street.
- **Rooms** are characters in the layout. `.` is outside. **Walls appear by themselves** wherever two different rooms
  meet, or a room meets the outside.
- **Openings** sit on a tile's edge: `(x, y, side)` with side `N`, `E`, `S` or `W`. `doors` open and shut, `windows`
  can be opened, smashed and climbed through, `arches` are open doorways.
- **Furniture** has a `kind`, the tile `at` its top-left, a `size` in tiles (default `(1, 1)`) and the way it's
  `facing` (default `S`).
- **Levels:** the first is the ground floor; add more for upstairs. Every level has the same footprint size, and
  `stairs` furniture joins them.

## Fields

| Field | What it is |
|---|---|
| `id`, `name` | The template's id and name. |
| `kind` | `House`, `Office`, `Shop` or `Civic`. Decides which lots it goes on: houses on house lots (at most about 13 × 15 m), shops and offices on the main street, civic buildings (town hall, church, school, up to about 40 × 40 m) on the square's landmark lots. |
| `roof` | `GableX` (ridge along x), `GableY` (ridge along y) or `Flat`. |
| `rooms` | Character → `(name, room_type, floor)`. `room_type` drives loot and population (`kitchen`, `bedroom`, ...). `floor` is `Wood`, `Carpet`, `TileFloor` or `Concrete`. |
| `levels` | Each with `layout`, `doors`, `windows`, `arches` and `furniture`. |

## Furniture kinds

`bed`, `sofa`, `table`, `coffee_table`, `desk`, `wardrobe`, `counter`, `stove`, `fridge`, `sink`, `toilet`,
`bathtub`, `tv`, `washer`, `dryer`, `shelf`, `workbench`, `stairs`. Which of them can be searched, and what they hold,
depends on the room: see [Loot and containers](/docs/modding/loot/). Kinds without a 3D model are drawn as grey boxes.

## Checked on load

The game refuses a template with a mistake and says what's wrong: furniture outside the building, overlapping, or
across a wall between rooms; two openings on one edge; a room you can't reach from outside; an upstairs you can't
reach from the stairs; a doorway blocked by furniture; furniture you can't get to.

## See it

Start the game on your data and walk in, or have an agent call `buildings` (every building, its template and where it
is) and `screenshot` at its position. `--seed` changes the town, so try a few to see your template on different lots.

:::caution[Saves]
A save regenerates its town from the seed and the templates. A template change that changes the map makes older
saves refuse to load (see [Saves](/docs/reference/saves/)).
:::
