# UI design guide

How every screen, panel and piece of the HUD in Zombye is laid out and drawn, so the inventory, the
Make and Build screens, health, skills, the map, the menus and character creation read as one game.
[art-direction.md](art-direction.md) is the look of the world; this is the look of what sits over
it. The code side is `crates/client/src/theme.rs`: every colour, size and component here has a name
there, and nothing in a panel sets a colour or a size of its own.

This guide replaces the "rough painted skin" (dry-brushed sheets and buttons). Panels move to it
one at a time, the inventory first; until a panel has moved it keeps the old skin.

## The idea in one sentence

**Quiet, flat, dark chrome that gets out of the way of the world, with the things the survivor
actually holds (a map, a note, a page) drawn as paper inside it.**

Five rules follow:

1. **The world stays on screen.** A full screen is a dark veil over the game, not a wall in front of
   it: you still see the street and whatever is walking up it. Nothing in the UI is brighter or more
   colourful than the world behind it, except the one thing that matters now.
2. **Flat.** Solid fills, one-pixel hairlines, small radii. No textures, no bevels, no glows, no
   gradients (the veil is the one exception), no shadows except under something that floats (a hover
   card, a menu).
3. **One accent.** Zombie green means *this one*: the open tab, what is picked, the main button.
   Every other colour is a warning of some kind, and is always backed by a word or a shape.
4. **Two kinds of surface.** *Chrome* is the dark frame and everything in it. *Paper* is for what
   exists in the survivor's hands: the map today, notes, recipe cards and book pages later. Paper
   always sits inside chrome, never the other way round.
5. **Plain words, one key each.** Every label is an ordinary word, every action shows its key in a
   keycap, and nothing needs a manual. Our copy never names another game.

## Colour

### Chrome

| Token | Value | Use |
|---|---|---|
| `INK` | `#141414` (brand near-black) | Panel fill, at 86% over the game. Text on a green button. |
| `BONE` | `#F2E6C9` (brand bone white) | Text, values, icons' lightest tone. |
| `DIM` | `#C6BDA6` | Secondary text: labels, units, hints. |
| `FAINT` | `#7D776A` | Empty slots' labels, disabled things, resting rail tabs. Never for anything that must be read to play. |
| `LINE` | bone at 11% | Hairlines: column dividers, pane rules, borders of tiles and buttons. |
| `CELL` | bone at 4.5% | Fill of a tile, slot, chip or secondary button. |
| `CELL_HOVER` | bone at 9%, border bone at 50% | The thing under the pointer. |
| `GREEN` | `#8DB06B` (brand zombie green) | The accent: open tab, picked tile, primary button, a gauge with room left, good condition. |
| `AMBER` | `#E0B04A` | A warning: worn, stale, nearly full, burdened. |
| `RED` | `#D7262B` (brand red) | Danger only: broken, mouldy, bleeding, overloaded, health nearly gone. |
| `BLUE` | `#7FA3C7` | Sleep and rest, nothing else. |

No other hues. An item's own colour (from `palette.ron`, through its entry in `items.ron`) appears
only inside its icon.

Colour is never the only signal. A worn weapon has an amber bar *and* the word "worn"; a picked
tile has a green outline *and* a tick; a full bag's gauge turns amber *and* reads `11.8 / 12 L`.

### Paper

| Token | Value | Use |
|---|---|---|
| `PAPER` | `#E9DCBB` | The sheet. |
| `PAPER_LIGHT` | `#F8F0DA` | Roads on the map, a highlighted line. |
| `PAPER_INK` | `#2B2620` | Everything printed or drawn: lines, labels, building walls. |
| `PENCIL` | `#D7262B` | The survivor's own marks: where you are, where you walked, a cross, a note. |
| fills | dusty tan `#C9A27A`, dusty red `#C98372`, blue-grey `#8F9BAA`, sage `#B9C08F` | Flat tints by kind of thing. Muted: paper is never brighter than the chrome's bone text. |
| greens | grass `#D2D6A2`, broadleaf trees `#8C9A6A`, pines `#77875E` | The map's ground and trees. |

Paper may carry a little grain and a fold line or two, being a thing and not chrome. Text on paper is
ink, in the same typefaces. Chrome controls (buttons, chips, the rail) are never drawn on paper: a
paper screen keeps the frame's top bar, rail and footer, and its own controls live there.

## Type

Two faces (SIL OFL 1.1, `assets/fonts/barlow/`), figures always tabular.

| Role | Face | Size (UI px at 100%) | Notes |
|---|---|---|---|
| Screen title | Barlow Condensed SemiBold, capitals, tracking 0.1 em | 30 | One per screen, top left: INVENTORY. |
| Column title | Condensed, capitals, 0.06 em | 22 | GEAR, YOU, NEARBY. |
| Pane header, button, chip, tab | Condensed, capitals, 0.06 em | 17 (chips and tabs 15) | HIKING PACK, TAKE ALL. |
| Slot and field labels | Condensed, capitals | 14 | HEAD, MAIN HAND. |
| Body | Barlow Regular | 15 | Names, sentences, rows. |
| Value | Barlow Medium | 15 (the load figure 22) | Numbers that matter: 21.5 of `21.5 / 40 L`. |
| Small | Barlow Regular | 14 | Hints, units, tags. Nothing is smaller. |

Capitals only in Condensed. Body text is sentence case. Names never wrap: one too long for its
place is cut with an ellipsis and shown whole on the hover card.

Text keeps a contrast of at least 7:1 on chrome over the brightest ground the game draws (the veil
and the panel fill are set so `BONE` and `DIM` hold it; `FAINT` is exempt, and so is used only for
what needn't be read).

## Shape and space

- A 4-pixel grid. Gaps between tiles, chips and buttons: 8. Padding inside a column: 24 (16 below
  1600 wide). Between a pane's header and its tiles: 10; between panes: 18.
- Radius 6 on tiles, slots, chips, buttons, fields; 8 on floating cards and menus; 3 on bars.
- Hairlines are 1 pixel of `LINE`. An empty place to put something (a slot, a spare tile) is the
  same box with a dashed hairline and no fill.
- Sizes follow the UI scale (75% to 150%); never set a size in window pixels.

## The frame

Every full screen is the same frame, and the screens are its tabs:

```
+--------------------------------------------------------------------+
| (mark)  SCREEN TITLE   context line                    Close [key] |  top bar, 76
+----+---------------------------------------------------------------+
|rail|  column        |  column                  |  column           |
| 92 |                |                          |                   |  body
|    |                |                          |                   |
+----+---------------------------------------------------------------+
| [key] hint   [key] hint   [key] hint                               |  footer, 46
+--------------------------------------------------------------------+
```

- **Top bar:** the mark, the screen's title, one line of context (day, hour, the room you are in),
  and Close with its key at the right.
- **Rail:** one tab per screen, an icon over its name with its key in the corner: Inventory, Make,
  Build, Survivor, Map. The open one is bone white with a green icon and a 4-pixel green bar
  on its outer edge. A screen's own key opens it and closes it; the rail and the other screens'
  keys switch between them without closing.
- **Sub-tabs:** a screen with several pages (Survivor: Info, Health, Skills, Protection) lists them
  in the top bar after the context line, as chips; the open one is green. `H` and `K` open Survivor
  straight on Health and Skills.
- **Body:** two or three columns divided by hairlines. A column has a title and scrolls on its own,
  up and down only. **Nothing scrolls sideways, ever:** content wraps to the width it has.
- **Footer:** the keys that work on this screen, as keycaps with a word each. If an action has no
  place in the footer it doesn't have a hidden key.
- Below 1600 UI pixels wide the rail narrows to icons (64), column padding drops to 16 and the side
  columns narrow; the layout holds down to 1280 x 720.
- The game goes on behind a screen in play (a multiplayer server never pauses), which is why the
  veil is see-through. Menus that do pause (main menu, pause, settings, character creation) use the
  same frame over an opaque `INK` ground or a held picture of the world.

## Components

Each is one function in `theme.rs`; a panel never builds its own.

- **Button.** 38 high. *Primary* (one per screen at most): green fill, ink text. *Secondary*:
  `CELL` fill, hairline, bone text. *Disabled*: no fill, `FAINT` text. Its key sits in it as a keycap.
- **Keycap.** A key to press: Barlow Medium 14 in a rounded hairline box. Used the same way in
  buttons, the footer, hover cards and the HUD's hints.
- **Chip.** A filter that is on or off: 32 high; off is `CELL`, on is green with ink text. The first
  chip of a row is All.
- **Field.** Search: a dark inset with a hairline, a prompt in `FAINT` saying what it searches, its
  key at the right end.
- **Pane.** A container's contents: a header (small icon, name, where it is: "on your back", then
  the gauge `21.5 / 40 L` at the right) over a rule that is itself the gauge, filled green to the
  share used, amber from 85%. Under it the tiles. A click on the header folds the pane to its header.
- **Tile.** One kind of thing, 104 square: its icon, its name under it, a count at the top right
  when more than one, a short tag at the top left when there is something to say ("stale", "half",
  "6 left"), and along the bottom edge a 4-pixel bar when it has a condition, freshness or contents
  (green, amber from 60%, red from 30% of it left). Stacks of the same thing in a pane show as one
  tile. *Hover*: brighter, with the hover card. *Picked*: green outline and a tick.
- **Row.** The compact form of a tile, 32 high, for big loads: small icon, name, tag, count, weight
  at the right. The view switch at the top of the screen changes every pane between tiles and rows.
- **Slot.** A place on the body or in a hand. Empty: dashed box with its label. Filled: the item's
  icon and name, its condition bar, the label kept at the top left. A bag's slot has a green
  hairline: it is also a pane.
- **Bar.** One flat fill on a dark track, radius 3: load, a skill's progress, the action under way.
- **Hover card.** Floats by the pointer: name in Condensed, a line of type, weight and size, then
  rows of label, bar or value, then what can be done with it and the keys.
- **Menu.** Right-click: a flat list on a card, the likeliest first, numbered 1 to 9.
- **Status chip.** The HUD's status icons: a white glyph on a dark rounded square, pale yellow, amber
  and then red as it worsens, blue for sleep.
- **Note.** One line of plain words saying what just happened or why not ("no room for the tin in
  the tote bag"), bottom centre, gone after a few seconds.

## Icons

- **Item icons** are flat drawings on a 48-unit grid in three tones of the item's own colour (the
  colour, a darker and a lighter), with no outline, no gradient and no lettering. Long things lie at
  45 degrees, handle down. The silhouette says which item it is; things alike are told apart by shape
  (the five bags, the three tins), not by colour alone. Sources are SVG, one per item id, packed into
  one atlas; an item with no icon of its own (a mod's) shows its category's glyph in its colour.
- **Interface glyphs** (rail tabs, status chips, the chevron, the tick) are one tone, `BONE` or the
  state's colour, on the same grid.
- All drawn by us. Nothing traced, nothing from a pack.

## Behaviour

- **A click does the likely thing, a right-click lists everything.** In the inventory a click moves
  a tile to the other side (yours to the open container or the floor, theirs to you); onto a named
  pane or slot by dragging it there.
- **Everything has a key, and shows it.** Screens: their rail key. Actions: a keycap on the button
  and in the footer.
- **One request, one answer.** A press sends one request; the thing shows as on its way until the
  server answers, and a refusal is a note in plain words.
- **Lots of things stay manageable** by wrapping, folding panes, the compact rows, search and the
  chips, which narrow every pane at once. Never by a sideways scroll.
- **State is remembered:** which panes are folded, tiles or rows, the sort, the open tab.
- **Motion** is short and only where it explains something: 120 ms on hover and press, a bar that
  fills as an action runs. Nothing slides in, nothing bounces.

## Screens

| Screen | Body | Notes |
|---|---|---|
| Inventory | Gear (hands, body slots, bag slots, hotbar, load) / You (a pane for the pockets and for each bag) / Nearby (the open container, the floor) | Search, sort, chips and the view switch over the middle column. |
| Make | Recipes by category / the chosen recipe: what goes in, tools, what comes out, the time, Make | Fire and cooking add their recipes here. |
| Build | What can be built, by category / the chosen piece: materials, tools, where it goes | Barricades first; walls, furniture and bigger works later, in the same list. |
| Survivor: Info | Who this is: name, Background, days survived, kills / Strengths and Flaws as two lists / appearance | A Strength is green with a plus, a Flaw amber with a minus, each with one line saying what it does. What character creation chose is shown here. |
| Survivor: Health | The body, part by part / wounds as rows with their state and treatments | |
| Survivor: Skills | The skills / the chosen skill: level, progress, what it gives | |
| Survivor: Protection | The body, part by part / what covers each part and how much it stops, by kind of wound | The Gear column's one-line summary opens it. |
| Map | Paper, filling the body | The survivor's paper map: printed streets, the rest inked in as it is seen, red pencil for your own marks. Pan and zoom keys in the footer. |
| Menus, settings, character creation | The frame over an opaque ground, one column of choices and one of detail | |

### What is built

The guide is ahead of the game. Built on the frame today: **Inventory** (with dragging between bags,
gear slots, the hotbar and what is nearby), **Make** (a plain list of recipes, no categories or detail
column yet), **Health** (condition and status on the left, wounds as cards with their treatments on
the right) and **Skills** (a card a skill), with the veil at 97.5% so text keeps its contrast over
sunlit ground. Health and Skills are tabs of their own for now; the Survivor screen will gather them. Build, Survivor, the Map and the menus are other pieces of
work that move onto these tokens as they land; until then the rail lists only what opens.

## The HUD

The same tokens at a smaller size: status chips, the hotbar's slots, the weapon line and the current
action's bar are tiles, slots and bars from this guide on `INK` at 86%, with no frame round them.
Only what is needed now is on screen.

## Checklist for a new screen

1. It is a tab of the frame, with a key shown on the rail and in the footer.
2. Every colour, size and component comes from `theme.rs`.
3. No text under 14, none of it only in `FAINT`.
4. Nothing scrolls sideways at 1280 x 720 or at 150% scale on 1920 x 1080.
5. Every state shown by colour is also shown by a word or a shape.
6. One primary button at most.
7. The words are plain, in sentence case outside Condensed labels, and name no other game.
8. Reviewed in offscreen pictures (`--render-to ... --hud`) at 1280 x 720 and 1920 x 1080.
