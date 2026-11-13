@AGENTS.md

## The other Dead Commons repos

This repo is going public (AGENTS.md, Open source and the official build): write as if it already is. The rest of the [deadcommons](https://github.com/deadcommons) org is private: name
those repos here, but never copy their files, contents or plans into this repo, its issues or its PRs.

| Repo | What's in it | On the Mac |
|---|---|---|
| [zombye-art](https://github.com/deadcommons/zombye-art) | Art in progress: Blender scenes, generator output, textures, renders, review rounds. An asset comes here only when it's promoted ([docs/asset-pipeline.md](docs/asset-pipeline.md)). | `~/games/zombye-art` (Git LFS; `$ZOMBYE_ART`) |
| [studio](https://github.com/deadcommons/studio) | The business side: brand sources, names, legal, research, marketing, finance, and setting up a Mac for the work (`dev-setup/`). | `~/games/studio` |
| [hosting](https://github.com/deadcommons/hosting) | The commercial hosting service built on this repo's open-source server. | not cloned |

Work goes in the repo it belongs to: art in zombye-art, brand and business in studio, the hosted service in
hosting, the game here. Read another repo when the work needs it (the asset tools find zombye-art through
`$ZOMBYE_ART`); change it only when the task is about it.

## Working from the GitHub project

All development work goes through the
[Game development project](https://github.com/orgs/deadcommons/projects/2) on GitHub
(`gh project … --owner deadcommons`, project number 2). The repo doesn't keep task lists.

- **Before starting:** find the issue for the work, or create one in the right milestone and add it
  to the project. Set its board Status to **In Progress**.
- **While working:** reference the issue in commit messages (`Refs #N`, or `Closes #N` on the
  commit that finishes it). New follow-up work you discover becomes its own issue on the board,
  not a TODO in the code or a list in a doc.
- **When done:** close the issue with a comment naming the commit, and set Status to **Done**.
- **On the remote machines** (docs/remote-dev.md) an issue ends differently: the agent commits with
  `Refs #N` and opens a draft PR saying `Closes #N`; the owner approves the review pictures, and merging
  the PR closes the issue (then set Status to **Done**).
- Only the **Game development** board (project 2) is for development. "Dead Commons roadmap"
  (project 1) is private admin work; don't touch it.

## Working on the remote machines from the Mac

When the remote machines are up (`ssh zombye-build true` works), the Mac drives and they do the work:
**[docs/remote-dev.md](docs/remote-dev.md)**. Start issues as agents in their own worktrees
(`ssh zombye-build zwork start N [MODEL]`, or zboard), as many at once as don't touch the same code; the owner follows them on zboard (`ssh -fN zombye-board`, http://localhost:8765); follow them yourself with
`zwork ls` and `zwork log N`; bring each one's `review/*.png` back to `saves/` here and show them to the
owner; send feedback with `zwork say N "..."`. Builds, tests, renders and remote play (zboard's Play) run
there, not here. Asset work (Blender, Meshy, ElevenLabs) stays here.
