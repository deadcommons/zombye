# Vision

Zombye is slow-burn zombie survival where every death is your own story, built in real 3D on
an open engine that anyone can extend. This page says what we keep from the slow-burn survival
genre, what we change and what we never do, so that contributors can tell whether a change fits.

## What we keep

- **The fantasy:** an ordinary person in an ordinary early-1990s American small town, the day
  after. No superpowers, no quest markers. Death is permanent, and it's your story.
- **Zombies that are dangerous in numbers:** they see, hear, lose track of you, follow noise,
  thump on doors until they break, and pile up into sieges.
- **Sneak or fight:** stealth, shoving, stomping and melee you have to commit to.
- **The body matters:** wounds per body part, bleeding, bandages, infection, hunger, thirst,
  fatigue, endurance.
- **Looting that makes sense:** what you find depends on the room you're in.
- **Crafting from junk:** ripping clothes into rags, patching, improvised tools and barricades.
- **A readable isometric view** with walls that fade near you and fog where you can't see.
- **Multiplayer and sandbox settings** as first-class features, not afterthoughts.

## What we change

- **Real 3D**, not 2D sprites: a camera that can snap between 4 angles, real height and lighting.
- **Performance:** thousands of simulated zombies and hundreds on screen at high frame rates,
  on ordinary hardware.
- **Open by default:** source code, data, tools and our assets are public. Anyone can build,
  fork, mod and host it.
- **Data-driven everything:** every gameplay number lives in a data file with a schema, and the
  base game is just the `core` data pack.
- **Agents as first-class players and builders:** the same intent/observation API that clients use
  is exposed over MCP, so agents can play, test, run servers and make mods.
- **Server-authoritative multiplayer** from day one.

## What we never do

- Copy anything from another game: code, data, text, art, sounds or map. We match the *feel*,
  in our own work. See the clean-room rules in [CONTRIBUTING.md](CONTRIBUTING.md).
- Lock self-hosting behind an account or a fee. Official hosting earns its place by being the most
  convenient option, not the only one.
- Trash the original or its developers. We're here because we love their game.

## How to tell if a change fits

Ask: *would someone who loves slow, unforgiving survival games feel at home, and is it better?*
If it makes the game less recognisable without a clear gain, it probably belongs in a mod. Bigger design changes go through
the decision process in [CONTRIBUTING.md](CONTRIBUTING.md#how-decisions-are-made).
