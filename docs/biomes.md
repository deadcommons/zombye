# Biomes

What grows on the wild ground outside town, and how, is data: `assets/data/biomes.ron`. A biome is a
named set of numbers for trees, bushes, the ground's colours, what lies on the ground, and grass. The
woods round the town are a patchwork of them. Add a biome by adding an entry to the file; no code.

The town's own ground (lawns, yards, the square, street trees) is not a biome. The corners of town
its blocks leave wild are (those of biomes with `in_town: true`).

## What is simulation and what is only drawn

- **Simulation:** where trees, bushes and bare patches stand. They are tiles of the map: trees block
  walking and sight, can be chopped down, and zombies path round them. Changing them makes a
  different map, so a save made with other biomes loads only with `--load-anyway`, and a multiplayer
  client needs the server's biomes file (as with every other data file).
- **Only drawn:** which kind of tree a tree is (a birch and a broadleaf are the same to the
  simulation, as are a pine and a spruce), how tall it is, bush sizes, every colour, ferns, rocks,
  stumps, branches, pebbles, grass and flowers.

Everything is placed by hashes of the world's seed and the tile, never the town generator's random
numbers. So the town, where you wake up and the rest of the map stay the same while you tune a
biome, and changing one biome's numbers moves nothing in another.

## Trying settings quickly

| How | What it does |
|---|---|
| `--biome ID` (client and server; or `ZOMBYE_BIOME=ID`) | Grows that one biome over all the wild ground. |
| `everywhere: Some("ID")` in the file | The same, from the file. |
| `--biomes FILE` (or `ZOMBYE_BIOMES=FILE`) | Reads that file in place of the data folder's `biomes.ron`: keep a copy to play with. |
| `--watch-biomes` (client) | Watches the biomes file. Save it, and the game starts again in place with the new biomes: the same seed, standing where you stood, at the same hour, zoom and camera angle. A mistake in the file is shown on the yellow line and nothing restarts. Only in your own new world (not with `--connect` or `--load`). Each restart is a new world, and quitting autosaves as usual. |
| MCP `biomes {x, y}` | A map of which biome is where, how much ground each has, and each one's settings. |

A good loop for one biome:

```bash
cargo run -p zombye-client --profile play -- --biome meadow --watch-biomes --zombies 0 --at 60,60 --zoom 60
```

then edit `assets/data/biomes.ron` and save. For a picture with no window:

```bash
cargo run -p zombye-client --release -- --render-to shot.png --biome pine_wood --zombies 0 --at 60,60 --center 60,60 --zoom 70 --time 15
```

## Where a biome is

The map is cut into squares of `cell_m` metres. In each, every biome gets a score:

    weight x fit x (0.2 + noise)

- `fit` is 1 where the place is inside the biome's `town_distance`, `height` and `grade`, 0 outside,
  fading over `blend_distance_m`, `blend_height_m` and `blend_grade` at the ends.
- `noise` is the biome's own, rising and falling about every `patch_m` metres, so each biome has
  places where it wins.

The highest score takes the square. Borders are then made ragged by up to `ragged_m` metres, so two
biomes' trees and ground mix where they meet. Where no biome fits, the first in the file grows.

So: a higher `weight` means more of the map; a larger `patch_m` means fewer, bigger patches; a narrow
`grade` or `town_distance` pins a biome to hillsides or to the edge of town.

## The file

Top level:

| Field | Default | Meaning |
|---|---|---|
| `everywhere` | `None` | One biome's id to grow everywhere. |
| `cell_m` | 4 | Side of the squares a biome is chosen for, metres. |
| `ragged_m` | 3.0 | How far borders wander, metres. 0 is a straight step. |
| `blend_distance_m` | 18.0 | Fade at the ends of `town_distance`, metres. |
| `blend_height_m` | 3.0 | Fade at the ends of `height`, metres. |
| `blend_grade` | 0.04 | Fade at the ends of `grade`. |
| `biomes` | | The list. Ids are lower case, digits and `_`, each once. |

A biome has an `id`, a `name` for people, and six blocks. Everything but the id (and each tree kind's
height) may be left out.

### `area`: where it may be

| Field | Default | Meaning |
|---|---|---|
| `weight` | 1.0 | Share of the ground where it fits, against the others. 0: only with `everywhere`. |
| `patch_m` | 90.0 | About how big its patches are, metres. |
| `town_distance` | anywhere | Metres from the edge of town (from, to). Wild corners in town count as 0. |
| `height` | anywhere | Metres above the town (from, to). The hills reach about 18. |
| `grade` | anywhere | Steepness of the hillside, rise over run (from, to). The steepest faces are about 0.3. |
| `in_town` | true | Whether it may fill the wild corners inside the town's bounds. |

### `trees`

Trees stand in clumps with thinner ground between.

| Field | Default | Meaning |
|---|---|---|
| `thick` | 0 | Trees per 100 m² inside a clump. 25 is the most a spacing of 1 has room for. |
| `thin` | 0 | Trees per 100 m² between clumps. |
| `clump_m` | 24.0 | About how big clumps and gaps are, metres. |
| `clumped` | 0.4 | How much of the ground is clump, 0 to 1. |
| `soft` | 0.3 | How a clump thins at its edge: 0 a hard edge, 1 a long fade. |
| `spacing` | 1 | Tiles kept free of other trunks round each tree, 1 to 6. |
| `kinds` | none | `(kind, weight, height: (from, to))` each. Kinds: `Broadleaf`, `Birch`, `Pine`, `Spruce`. No kinds: no trees. |
| `slope_from`, `slope_to`, `slope_most` | 0.11, 0.19, 0.85 | Hillsides steeper than `slope_from` begin to lose trees; from `slope_to` on they have lost `slope_most` of them. |

Density is what the dice ask for; `spacing` then turns away trees that would stand too close, so very
high `thick` with a wide `spacing` gives an even stand, not a denser one.

### `bushes`

| Field | Default | Meaning |
|---|---|---|
| `per_100m2` | 0 | Bushes per 100 m² of open ground. |
| `in_clumps` | 1.0 | Times as many inside a clump of trees as between clumps. 0: none in the clumps; 8: knots of scrub. |
| `size` | (0.85, 1.25) | How big (from, to); 1 is about a metre across. |

### `ground`

Colours are `"#rrggbb"`.

| Field | Default | Meaning |
|---|---|---|
| `colours` | the palette's long grass | Open ground, in patches of each colour (1 to 4; the first covers most). |
| `patch_m` | 5.0 | About how big the colour patches are, metres. |
| `under_trees` | the palette's forest floor | The ground where trees stand close, in patches. |
| `needles` | the palette's | Fallen needles under pines and spruces. |
| `needle_cover` | 0.25 | How much of the ground under conifers they cover, 0 to 1. |
| `bare_per_100m2` | 1.5 | Tiles of bare earth per 100 m² of open ground. |
| `slope` | the palette's `dirt` | The earth that shows through on a slope (the hills drift to it from a grade of about 0.06). |
| `rock` | the palette's `cliff` | The stone of the steepest slopes (from about 0.11). |

The hills' own shading (each facet lit its own way, tree-foot shade) is laid over these colours.

### `litter`

Counts are per 100 m².

| Field | Default | Meaning |
|---|---|---|
| `ferns`, `ferns_open` | 0, 0 | Ferns under trees, and in the open. |
| `rocks` | 0 | Rocks, anywhere. |
| `rock_size` | (0.35, 0.75) | Metres across (from, to), at most 1.2. |
| `rock_groups` | 0.3 | Share of rocks lying in a group of two to four, 0 to 1. |
| `stumps` | 0 | Old stumps under trees. |
| `branches` | 0 | Fallen branches under and near trees. |
| `branch_size` | (0.7, 1.6) | Metres long (from, to), at most 2.5. |
| `pebbles` | 0 | Scatterings of pebbles. |

Litter is drawn only: you walk over it. Logs you can pick up are the simulation's deadwood, apart
from biomes.

### `grass`

| Field | Default | Meaning |
|---|---|---|
| `open`, `under_trees` | `Long`, `Thin` | The grass in the open and where trees stand close: `None`, `Thin`, `Short`, `Long`. |
| `cover`, `cover_under_trees` | 1.0, 0.6 | How much of that ground has grass, 0 to 1. |
| `dry` | (0.15, 0.6) | 0 fresh green to 1 straw (from, to); it varies between the two in drifts. |
| `flowers` | 0.3 | Wildflowers: 0 none, 1 as on a town's rough ground, more for a flower meadow. |

The defaults in these tables are the ones in `crates/sim/src/biomes.rs`; if they differ, the code is
right.

## The shipped biomes

| Id | Where | Look |
|---|---|---|
| `wood_edge` | Within 40 m of town | Open, light woodland, long grass, flowers, scrub. |
| `mixed_woods` | From 25 m out | Broadleaves with some conifers, thickets and glades. |
| `pine_wood` | From 70 m out | Tall dark conifers on a floor of needles. |
| `birch_grove` | Small patches | Bright close stands of birch, ferns. |
| `meadow` | Gentle ground | Long grass and wildflowers, a lone tree, knots of bushes. |
| `deep_woods` | From 90 m out | Old, close woods, dark floor, ferns and deadwood. |
| `rocky_slope` | Grades over 0.16 | Thin dry grass, bare earth, rocks, stunted conifers. |

## Mistakes

The file is checked when the game or server starts: unknown fields, ranges the wrong way round,
numbers out of range, an id twice, `everywhere` naming a biome that isn't there. The message names
the biome and the field, and the game doesn't start (with `--watch-biomes`, it keeps running on the
old biomes).
