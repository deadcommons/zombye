---
title: Items
description: items/items.ron, everything a survivor can carry.
---

`items/items.ron` is a list of every thing in the world a survivor can pick up. Weights and sizes are real-world
ones, so a new item's numbers can be guessed without a table.

```rust
(id: "bread", name: "Loaf of bread", category: Food, weight_kg: 0.5, volume_l: 2.0, stack: 2,
    food: Some((hunger: 0.30, uses: 3, spoils_hours: Some(48.0))), color: (0.78, 0.6, 0.38)),
```

## Fields

| Field | What it is |
|---|---|
| `id` | Lowercase letters, digits and `_`. Saves, loot tables and recipes name the item by it. |
| `name` | What players see. |
| `category` | `Food`, `Drink`, `Medical`, `Tool`, `Material`, `Clothing`, `Bag`, `Weapon`, `Light` or `Junk`. |
| `weight_kg` | Counts in full wherever it's carried: hands, pockets, bags or worn. |
| `volume_l` | Room it takes in pockets, bags and furniture. Held and worn items take none. |
| `stack` | How many share one slot (default 1). |
| `tags` | Words rules check for: `needs_tin_opener`, `opens_tins`, `cuts`, `rippable`, `fire`, `bandage`, `clean`, `disinfects`, `plaster`, `stitches`, `painkiller`, `antibiotic`, `refillable`, ... Recipes can ask for a tool by tag. |
| `two_handed` | Held in both hands (default `false`). |
| `weapon` | `Some("id")` of a weapon in [`weapons.ron`](/docs/modding/weapons/): the item swings like it. |
| `food`, `drink` | `Some((hunger: 0..1, thirst: 0..1, uses: n, spoils_hours: Some(hours)))`. How much hunger and thirst it takes away (negative makes it worse), how many goes it lasts, and when it starts to go off (`None` keeps forever). |
| `medical` | `Some((uses: n))`. What it treats comes from its tags. |
| `bag` | `Some((capacity_l: litres, carried: Back \| Shoulder \| Hand, slow: 0..1))`: room it adds, where it's carried, and speed lost while carried. |
| `clothing` | `Some((covers: [Head, Torso, Arms, Hands, Legs, Feet], takes: [Face, HeadTop, Neck], protection: 0..1, layer: Base \| Mid \| Outer, hood: false))`. |
| `light` | `Some((radius_m, hours))`: a light source, like the flashlight. |
| `model` | `Some("name")`: the wearable drawn on a survivor (`assets/models/wearables/<name>.glb`). Without one it's a grey box. |
| `color` | Grey-box colour, RGB from 0 to 1. |

## Food going off

Food with `spoils_hours` is fresh until then, stale up to twice that (it gives 70% and may make you sick), then mouldy
(40%, and it usually does). Food found in a cupboard was made some time before the world began, so not everything in a
fridge is fresh.

## Clothing

Two worn items can't cover the same body part on the same layer, or take the same place (the face, the top of the head,
the neck). `protection` is how much of a strike it stops on the parts it covers: layers multiply, and a scratch is
stopped more easily than a bite. A `hood` covers the head while nothing else is on top of it.

## Medical items by tag

| Tag | What it does |
|---|---|
| `plaster` | Dresses a bruise or scratch, clean. |
| `bandage` | Dresses any wound. With `clean` it goes on clean; without, dirty (a plain rag). |
| `disinfects` | Cleans an undressed wound, halving the chance it festers. |
| `stitches` | Stitches a gash. |
| `painkiller` | Eases pain for four hours. |
| `antibiotic` | A course of two cures festering. Never the zombie infection. |

## See also

- [Recipes](/docs/modding/recipes/) to make items from other items.
- [Loot and containers](/docs/modding/loot/) to put them in the world.
- The MCP `items` tool prints the whole table, with an optional category filter.
