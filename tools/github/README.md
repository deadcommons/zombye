# Project boards from the command line

`board.sh` puts issues on the deadcommons project boards and sets their **Status**, so agents can keep
the boards current as they work. Issues of deadcommons/zombye go on the
[Game development](https://github.com/orgs/deadcommons/projects/3) board (project 3), issues of
deadcommons/zombye-art on project 1; `--project N` picks another.

```bash
tools/github/board.sh status zombye 14 "In Progress"   # adds it to the board first if it isn't on it
tools/github/board.sh status zombye 14 Done
tools/github/board.sh add    zombye 31
tools/github/board.sh show   zombye 14                 # the boards it is on and its Status there
```

**Where it runs:** on the build machine, where its token lives. Projects need GitHub's GraphQL API with a
token allowed to write organisation projects, which the cloud sessions' GitHub connection doesn't have.
A cloud session reaches the build machine through a Remote Control session on a Mac, then `ssh` (see
docs/remote-dev.md).

**The token:** a fine-grained token of the bot account (never a person's), resource owner `deadcommons`,
repository access to the repos whose issues go on the boards with **Issues: Read-only**, and organisation
permission **Projects: Read and write**. It is read from `$GH_PROJECT_TOKEN`, else
`~/.config/zombye/gh-project-token` (`chmod 600`; `ZOMBYE_BOARD_TOKEN_FILE` names another file). The
owner puts it there; the script never prints it, and agents never read or echo the file. Without it the
script stops with a message saying what is missing.

Needs `gh` (for its GraphQL client); nothing else.

**Without the script:** each board's own Workflows (board menu, Workflows) can move cards by themselves:
"Item added to project" to Todo, "Pull request linked to issue" to In Progress, "Item closed" and
"Pull request merged" to Done.
