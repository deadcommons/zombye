# Contributing to Zombye

Thanks for helping. Contributions from people and from AI agents working for people are both
welcome. Whoever opens the pull request is responsible for everything in it, including what their
agent wrote.

## Licence of your contribution

There's no CLA and no sign-off. By opening a pull request you agree that your contribution is
licensed under MIT OR Apache-2.0, like the rest of the repository (unless an asset's row in
ATTRIBUTION.md says otherwise). You keep the copyright to your work. Only submit work you have
the right to license this way.

## Clean-room rules (non-negotiable)

Breaking these is the worst mistake you can make in this repo. It puts the whole project at risk.

- **Observe other games only as a player.** You may play them, read their public wikis and patch
  notes, and take your own screenshots for private reference (never committed).
- **Never open up a game.** Don't decompile, disassemble or debug another game, and don't
  read, extract or copy anything inside its install folder: no code, scripts, item or recipe files,
  translations, textures, models, sounds or map files. This applies to agents and reverse-
  engineering tools as well.
- **Never copy from mods** of other games either. Their authors own them.
- **Our own words, our own numbers.** Describe mechanics in your own words. Every value in our data
  files is our own tuning, not a value taken from the game. Item names may be ordinary English
  words (a "kitchen knife"), but never copy item descriptions, UI text or lore.
- **Our own map.** Towns are generated from our own templates, or built from open geographic data
  such as OpenStreetMap. Never trace or recreate another game's map.
- **No code copied from other games, and no GPL, LGPL or AGPL code.** Everything here must stay
  MIT OR Apache-2.0.

If you're unsure whether something is allowed, ask in an issue before you write it.

## Assets

Follow the policy in [ATTRIBUTION.md](ATTRIBUTION.md): every asset file has a row there, added in
the same commit. Original work, open-licensed work, or AI-generated work made on a plan whose terms
let us redistribute it. No assets from other games and no third-party asset packs, not even as
placeholders.

## Making a change

1. For anything larger than a bug fix, open an issue first so we can agree on the approach.
2. Keep pull requests focused: one change per PR.
3. Run `cargo fmt`, `cargo clippy` and `cargo test` before you push.
4. Describe what you changed and how you tested it. For gameplay changes, say what a player will
   notice.

## How decisions are made

- Small changes: a maintainer reviews and merges.
- Bigger decisions (a new system, a design change from [VISION.md](VISION.md), licensing, project
  process) go through an RFC: a pull request that adds `docs/rfcs/NNNN-short-title.md`. Anyone can
  open or comment on one. A maintainer opens a comment period (7 days for licensing or process,
  3 days otherwise), and it merges with approval from a maintainer other than the author, once
  every maintainer objection is resolved.
- Dead Commons, as the project's steward, has the final say when an objection can't be resolved,
  and explains why on the pull request.

## Credits

Everyone who contributes is credited by their GitHub username in the git history and release
notes.
