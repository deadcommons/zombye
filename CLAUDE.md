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
- Only the **Game development** board (project 2) is for development. "Dead Commons roadmap"
  (project 1) is private admin work; don't touch it.
