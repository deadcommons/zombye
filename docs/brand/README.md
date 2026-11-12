# The Zombye brand

ZOMBYE™ is a trade mark of Grillo Ltd (trading as Dead Commons). The name, the Zombye logo, the
road-Z mark and the app icon are **not open source**: they are all rights reserved, under
[LICENSE-brand.txt](LICENSE-brand.txt), and are not covered by this repository's MIT / Apache-2.0
licences or by the art licence in `assets/LICENSE`.

That is why the logo files are not in this repository. Official builds carry them in a
`release-brand` folder beside the game (`$ZOMBYE_RELEASE_BRAND`; `crates/client/src/release_brand.rs`),
and show the logo on the main menu and the loading screen, the road Z in the top bar and the icon
on the window. A build made from this repository alone shows the name as plain text instead, and
works the same.

If you publish a fork or a modified build, give it your own name and logo. You may say, in plain
text, that it is based on Zombye by Dead Commons.
