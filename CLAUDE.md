@AGENTS.md

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
