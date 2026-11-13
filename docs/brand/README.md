# The Zombye brand

ZOMBYE™ is a trade mark of Grillo Ltd (trading as Dead Commons). The name, the Zombye logo, the
road-Z mark and the app icon are **not open source**: they are all rights reserved, under
[LICENSE-brand.txt](LICENSE-brand.txt), and are not covered by this repository's MIT / Apache-2.0
licences or by the art licence in `assets/LICENSE`.

## What's in this folder

- `zombye-logo.png`: the Zombye logo, shown at the top of the repository's README. It is here only
  so that page can show it. **It is not part of the open-source game, and no licence in this
  repository lets you use it:** all rights reserved, under [LICENSE-brand.txt](LICENSE-brand.txt).
  The game doesn't read it.

The rest of the logo files are not in this repository. Official builds carry them in a
`release-brand` folder beside the game (`$ZOMBYE_RELEASE_BRAND`; `crates/client/src/release_brand.rs`),
and show the logo on the main menu and the loading screen, the road Z in the top bar and the icon
on the window. A build made from this repository alone shows the name as plain text instead, and
works the same.

If you publish a fork or a modified build, give it your own name and logo, and take this logo out
of your README. You may say, in plain text, that it is based on Zombye by Dead Commons.
