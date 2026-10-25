---
title: 'Tutorial: a nail bat'
description: Add a new weapon, the item that carries it, a recipe to make it and a place to find it.
---

In this tutorial you'll add a **nail bat**: a baseball bat with nails hammered through it. It hits harder than a bat
and crits more often. You'll make it craftable from a bat and a box of nails, with a claw hammer, and now and then
find one on a garage shelf.

It touches four files, which is most of what modding Zombye looks like. Allow ten minutes.

## 1. Copy the data

From the repository's root:

```bash
mkdir -p mods/nail-bat
```

```bash
cp -R assets/data mods/nail-bat/data
```

```bash
rm mods/nail-bat/data/sounds.ron
```

(Removing `sounds.ron` keeps the game's own sounds. See [How modding works](/docs/modding/).)

## 2. The weapon: how it swings

Open `mods/nail-bat/data/items/weapons.ron`. It's a list of weapons; add yours at the end, before the closing `]`. Start
from the baseball bat's numbers and make it a little heavier, slower and nastier:

```rust
    (
        id: "nail_bat",
        name: "Nail bat",
        category: LongBlunt,
        damage: (32.0, 44.0),
        range: 1.25,
        arc_degrees: 110.0,
        windup_seconds: 0.34,
        swing_seconds: 0.95,
        max_targets: 3,
        knockback: 0.55,
        stagger_chance: 0.6,
        knockdown_chance: 0.3,
        crit_chance: 0.18,
        crit_multiplier: 2.0,
        endurance_cost: 0.09,
        noise_radius: 6.0,
        length: 0.85,
        color: (0.55, 0.42, 0.3),
    ),
```

`category: LongBlunt` means it trains the Bludgeon skill like the bat. There's no `spawn_weight`, so it's never left
lying on a floor: you have to make it or find it in a container. The [weapons guide](/docs/modding/weapons/) explains
every field.

## 3. The item: what you carry

A weapon is how something swings; an item is the thing itself, with a weight and a size. Open `items/items.ron` and add
this next to the baseball bat:

```rust
    (id: "nail_bat", name: "Nail bat", category: Weapon, weight_kg: 1.3, volume_l: 2.5, two_handed: true,
        weapon: Some("nail_bat"), color: (0.55, 0.42, 0.3)),
```

`weapon: Some("nail_bat")` ties the item to the weapon you just wrote.

## 4. The recipe: how to make one

Open `items/recipes.ron` and add, before the closing `]`:

```rust
    (id: "make_nail_bat", name: "Hammer nails into a bat", secs: 20.0,
        inputs: [(item: "baseball_bat"), (item: "nails")],
        tools: [Item("claw_hammer")],
        outputs: [(item: "nail_bat")]),
```

Inputs are used up; tools are needed but kept. The [recipes guide](/docs/modding/recipes/) has the rest (tools by tag,
counts, how skill speeds it up).

## 5. Loot: where to find one

Open `loot.ron` and find the garage table, `(room: "garage", container: "any", ...)`. Add one line to its entries:

```rust
            (item: "nail_bat", weight: 0.3, rare: true),
```

`rare: true` means the world's rare-loot setting scales it. With a weight of 0.3 among that table's others, about one
garage shelf in 25 holds a nail bat.

## 6. Check it

```bash
./target/release/zombye-server --bench --ticks 1 --zombies 0 --data mods/nail-bat/data
```

If you made a mistake, it tells you where. Misspell `damage` as `damge` and you get:

```text
failed to load content: mods/nail-bat/data/items/weapons.ron: 242:5: Unexpected missing field named `damage` in `WeaponDef`
```

## 7. Play it

```bash
cargo run -p zombye-client --release -- --data mods/nail-bat/data
```

Find a bat, a box of nails and a claw hammer (try the garage and the laundry), open the inventory with **Tab**, and the
nail bat is in the **Make** list at the bottom.

## Or let an agent test it

Start the MCP server on your mod, with no zombies about, and ask your agent to try it:

```bash
claude mcp add nail-bat -- ./target/release/zombye-server --mcp --zombies 0 --data mods/nail-bat/data
```

> Spawn a baseball bat, a box of nails and a claw hammer at my feet and pick them up. Craft the nail bat, equip it,
> spawn one zombie next to me and hit it. Then check how often garage shelves hold a nail bat.

Here is what the server answered when we did exactly that:

```text
made nail bat from the baseball bat and the box of nails (401 ticks, 20.1 s)
changed: carried and held nails -1, baseball_bat -1, nail_bat +1; on the floor nothing

holding the nail bat (7 ticks, 0.3 s)

player 1 swung the nail bat facing e: landed after 7 ticks, 1 hit; ready again after 19 ticks;
100 HP, endurance 0.96; HITS: P1 hit #1 for 43, staggered (50 HP left)

a shelves (shelves, 30 L) in a garage fills from the fallback (garage, any): empty 15%, rolls 2-5
    nail_bat | weight 0.3 (rare) | 1% a pick | count 1-1
2000 sample fills: ...
    nail_bat | 3.8% | 1.0
```

43 damage and a stagger in one swing, and a nail bat in about 4% of garage shelves. If that's too generous, change
the weight and ask again: `loot` rolls a table thousands of times without anyone walking anywhere.

## Next

- Give it a model: see the [asset pipeline](https://github.com/deadcommons/zombye/blob/main/docs/asset-pipeline.md).
- Make a whole set of improvised weapons, or a recipe that turns planks and nails into something.
- Share it in the [Discord](https://discord.gg/y7NsmVG2br).
