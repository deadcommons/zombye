# Maintainers: our own machines, boards and repos

This page is for Dead Commons' maintainers and the agents working for them on our own machines.
If you're contributing from your own fork, none of it applies to you: follow
[CONTRIBUTING.md](../CONTRIBUTING.md) and the "Start here" section of [AGENTS.md](../AGENTS.md).

## The other Dead Commons repos

This repo is going public (AGENTS.md, Open source and the official build): write as if it already is. The rest of the [deadcommons](https://github.com/deadcommons) org is private: name
those repos here, but never copy their files, contents or plans into this repo, its issues or its PRs.

| Repo | What's in it | On the Mac |
|---|---|---|
| [zombye-art](https://github.com/deadcommons/zombye-art) | Art in progress: Blender scenes, generator output, textures, renders, review rounds. An asset comes here only when it's promoted ([docs/asset-pipeline.md](asset-pipeline.md)). | `~/games/zombye-art` (Git LFS; `$ZOMBYE_ART`) |
| [studio](https://github.com/deadcommons/studio) | The business side: brand sources, names, legal, research, marketing, finance, and setting up a Mac for the work (`dev-setup/`). | `~/games/studio` |
| [hosting](https://github.com/deadcommons/hosting) | The commercial hosting service built on this repo's open-source server. | not cloned |

Work goes in the repo it belongs to: art in zombye-art, brand and business in studio, the hosted service in
hosting, the game here. Read another repo when the work needs it (the asset tools find zombye-art through
`$ZOMBYE_ART`); change it only when the task is about it.

## Working from the GitHub projects

Work is tracked in GitHub issues on two boards, nowhere else (not Linear, not lists in docs or code):

| Board | For | Issues live in |
|---|---|---|
| [Game development](https://github.com/orgs/deadcommons/projects/3) (project 3, public) | the game: code, data, docs, art promoted here | deadcommons/zombye (added to the board by itself) |
| [Dead Commons roadmap](https://github.com/orgs/deadcommons/projects/1) (project 1, private) | the business: Steam, itch, trademarks, marketing, hosting, accounts | deadcommons/zombye-art, label `business` |

- **Before starting:** find the issue for the work, or open one in the right repo (and on the right
  board), and set its board Status to **In Progress**.
- **While working:** reference the issue in commit messages (`Refs #N`, or `Closes #N` on the commit
  that finishes it). Follow-up work you find becomes its own issue, not a TODO in the code.
- **When done:** close the issue with a comment saying what was done and naming the commit or PR (a
  PR that says `Closes #N` does it when merged), and set Status to **Done**. Work stopped part way gets
  a comment saying where it got to.
- **New ideas** from the project chat are triaged into issues the same day: the game's in
  deadcommons/zombye, the business's in zombye-art with the `business` label.
- **Setting Status:** on the build machine, `tools/github/board.sh status REPO N STATUS`
  (../tools/github/README.md); from a cloud session, reach the build machine first.
- **On the remote machines** (remote-dev.md) the agent commits with `Refs #N` and opens a draft
  PR saying `Closes #N`; the owner approves the review pictures, and merging the PR closes the issue.
- deadcommons/zombye is public: business plans, keys, addresses and private repo contents never go in
  its issues; they go in the private board's issues.

## Working on the remote machines from the Mac

When the remote machines are up (`ssh zombye-build true` works), the Mac drives and they do the work:
**[remote-dev.md](remote-dev.md)**. Start issues as agents in their own worktrees
(`ssh zombye-build zwork start N [MODEL]`, or zboard), as many at once as don't touch the same code; the owner follows them on zboard (`ssh -fN zombye-board`, http://localhost:8765); follow them yourself with
`zwork ls` and `zwork log N`; bring each one's `review/*.png` back to `saves/` here and show them to the
owner; send feedback with `zwork say N "..."`. Builds, tests, renders and remote play (zboard's Play) run
there, not here. Asset work (Blender, Meshy, ElevenLabs) stays here.
