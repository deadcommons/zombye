---
title: Recipes
description: items/recipes.ron, what a survivor can make from what.
---

`items/recipes.ron` lists everything a survivor can craft. In game, recipes show in the **Make** list at the foot of
the inventory panel; over MCP, `recipes` lists them and `craft` makes one.

```rust
(id: "cut_jeans", name: "Cut jeans into rags", secs: 6.0,
    inputs: [(item: "jeans")], tools: [Tag("cuts")], outputs: [(item: "rag", count: 4)]),
```

## Fields

| Field | What it is |
|---|---|
| `id` | Lowercase letters, digits and `_`. Saves name queued crafts by it. |
| `name` | What players see. |
| `secs` | How long it takes, more than 0 and at most 600. The Handiwork skill makes it 10% faster a level, down to half. |
| `inputs` | `[(item: "id", count: n)]`, used up. `count` defaults to 1. |
| `tools` | `[Item("id")]` or `[Tag("tag")]`: needed, but kept. A tag matches any item with that tag in `items.ron`. |
| `outputs` | `[(item: "id", count: n)]`, made new. They go into pockets and bags while there's room, the rest at your feet. |

## Rules

- Inputs and tools must be carried or held, not worn. Take the jumper off before you rip it up.
- Only whole items count as inputs: a half-drunk bottle of water won't do.
- An output can't also be an input, and nothing can be listed twice.
- Every item and tool named must exist; every tag must be on at least one item. Errors name the recipe.
- Crafting doesn't stop you walking, but being hit or swinging cancels it. Nothing is used up until it's done.
- Each craft gives Handiwork experience.

## Ideas

- Tools by tag let several items do the job: `Tag("cuts")` is scissors or a kitchen knife.
- Washing rags shows an input that comes back changed: three rags and a bottle of water make three clean rags and an
  empty bottle.
- Improvised weapons: see the [nail bat tutorial](/docs/modding/first-mod/).
