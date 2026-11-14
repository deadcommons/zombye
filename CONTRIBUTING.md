# Contributing to Zombye

Thanks for helping. Contributions from people and from AI agents working for people are both
welcome. Whoever opens the pull request is responsible for everything in it, including what their
agent wrote. How we use AI, and what we ask of work made with it, is in [AI.md](AI.md).

## Licence of your contribution

There's no CLA. Every commit carries a sign-off under the
[Developer Certificate of Origin](https://developercertificate.org/) (DCO 1.1): a line at the end of
the commit message, added by `git commit -s`:

```
Signed-off-by: Your Name <you@example.com>
```

It says you wrote the change, or otherwise have the right to submit it under this project's
licences. A commit an agent wrote for you is signed off by you. A pull request whose commits aren't
signed off can't be merged; `git rebase --signoff main` adds the line to commits you already made.

By signing off you agree that:
- code, data, shaders and docs you contribute are licensed under MIT OR Apache-2.0, like the rest
  of the code;
- art and audio you contribute (models, textures, animations, images, sounds, music) are licensed
  under CC BY 4.0. That lets Dead Commons (Grillo Ltd) publish them in the game's art set under CC BY-NC-SA 4.0
  and use them in official releases, including paid ones.

You keep the copyright to your work. Only submit work you have the right to license this way, and
add its row to ATTRIBUTION.md.

## This repo and the official build

Everything here is open, and a build from this repo alone is the whole game with its own art. Dead
Commons also sells an official build, which adds art and audio bought under licences that don't
allow publishing the files. Those never come into this repo, and contributions never depend on them:
if a change looks right with this repo's art, it's right. The Zombye and Dead Commons names and logos
are trademarks, not open source ([docs/brand/LICENSE-brand.txt](docs/brand/LICENSE-brand.txt)): a
fork you publish takes its own name.

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
- **No code copied from other games, and no GPL, LGPL or AGPL code.** All code must stay MIT OR
  Apache-2.0 (art and audio: see assets/LICENSE).

If you're unsure whether something is allowed, ask in an issue before you write it.

## Assets

Follow the policy in [ATTRIBUTION.md](ATTRIBUTION.md): every asset file has a row there, added in
the same commit. Original work, open-licensed work, or AI-generated work made on a plan whose terms
let us redistribute it. No assets from other games and no third-party asset packs (bought ones included), not even as
placeholders.

## Find something to work on

- **Issues for newcomers:** [`good first issue`](https://github.com/deadcommons/zombye/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22)
  are small and well described; [`help wanted`](https://github.com/deadcommons/zombye/issues?q=is%3Aissue+is%3Aopen+label%3A%22help+wanted%22)
  are bigger pieces we'd like a hand with. Everything planned is on the
  [Game development board](https://github.com/orgs/deadcommons/projects/3).
- **Claim it before you start:** comment on the issue to say you're taking it, and a maintainer
  assigns it to you. An issue that is already assigned is taken; ask in it if it looks stalled. If
  you stop, say so in the issue so someone else can pick it up.
- **Your own idea:** open an issue with the feature form. Anything that changes how the game plays,
  a file format, saves, the network protocol, the modding API or a dependency needs a proposal
  first ([GOVERNANCE.md](GOVERNANCE.md)).
- **Questions and help:** [Discussions](https://github.com/deadcommons/zombye/discussions) (Q&A,
  ideas, show and tell), or #help and #dev on [Discord](https://discord.gg/y7NsmVG2br).

**Branches:** work merges into `playtest` first and reaches `main` after a playtest. Branch from
`playtest` and open your pull request against `playtest`, not `main`.

**Building:** a current stable Rust toolchain and a C++ compiler (Luau, the mod scripting language,
is built from source). On Linux the client also needs a few system packages; on Debian and Ubuntu:
`sudo apt install g++ pkg-config libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev`. The
server alone (`-p zombye-server`) needs only the compiler. This repo's own art is all you need: the bought
art in the official build is never required, and contributions never depend on it.

## Making a change

1. For anything larger than a bug fix, open an issue first so we can agree on the approach.
2. Keep pull requests focused: one change per PR.
3. Run `cargo clippy` and `cargo test --release --workspace` before you push. Match the style of
   the code round your change; don't run `cargo fmt` over the tree (the code isn't rustfmt-formatted,
   and it would rewrite every file).
4. Sign off every commit (`git commit -s`, see above), and add an `Assisted-by:` line where a tool
   did a real part of it ([AI.md](AI.md)).
5. Describe what you changed and how you tested it. For gameplay changes, say what a player will
   notice.
6. **Show it working.** A change a player can see or hear (gameplay, UI, visuals, animation, camera,
   lighting, sound) comes with proof in the pull request: screenshots, a before and after pair, or a
   short video or GIF of it in the game. A reviewer should be able to judge it without building your
   branch. Changes to code alone, docs, tests or tooling don't need any.
   - Offscreen renders are the easiest way: `zombye --render-to shot.png --load SAVE ...` draws one
     picture with no window (see AGENTS.md, Offscreen), and the same command before and after your
     change makes a fair pair. For movement or timing, record a short clip (`--autoplay`, `--walk`,
     `--turn` and the other recording flags play it by themselves).
   - Drag the pictures into the pull request's description or a comment. Don't commit them: review
     pictures never go in the tree (`saves/` is git-ignored for them).
   - Keep files small: a PNG or JPEG about 1280 px wide, a clip under 30 seconds.
   - Pictures from this repo's own art are what we expect. Maintainers with the official build may
     add pictures of it with the bought art; that's fine to show. The bought asset files themselves
     never come into this repo.

## How decisions are made

- Small changes: a maintainer reviews and merges.
- Bigger changes start as a proposal in the **Proposals** category of
  [Discussions](https://github.com/deadcommons/zombye/discussions), open for comment for at least a
  week and decided at the weekly call. [GOVERNANCE.md](GOVERNANCE.md) has the whole process and who
  decides.

## Credits

Everyone who contributes is credited by their GitHub username in the git history and release
notes.
