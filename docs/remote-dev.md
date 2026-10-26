# Remote development: many issues at once on remote machines

The owner drives development from Claude on their Mac; the work runs on two remote Linux machines; the
owner sees screenshots and approves them before anything closes. Each issue gets its own agent in its own
git worktree, so many issues move at the same time. Any issue's build can be played remotely, in the
browser, streamed from a GPU; playing a local build on the Mac still works too.

## The machines

| Machine | What it is | What happens there |
|---|---|---|
| **The Mac** | the owner's laptop, with Claude (desktop app) | deciding what to work on, reviewing pictures and PRs, playing builds (streamed, or built locally), merging, and all asset work: Blender, Meshy and ElevenLabs (their MCP servers and keys live only here) |
| **`zombye-build`** | many cores, lots of memory and disk, **no GPU** | the code: one worktree and one agent per issue, builds, tests, benches, the headless and MCP servers |
| **`zombye-gpu`** | a small machine with an NVIDIA GPU, beside the build box | only the client: offscreen pictures for the build box (`zgpu`), and a desktop for remote play |
| **`zombye-play`** (optional) | another small GPU machine, in the region nearest the owner | remote play only, for less lag: with it, Play runs there (`ZOMBYE_PLAY_HOST`) |

The names are SSH aliases: in the Mac's `~/.ssh/config` for both, and in the build box's for the GPU box
(over the private network). Addresses, accounts and keys stay out of this repo. The remote machines hold
two credentials, both signed in by the owner: `gh` (to push branches, open PRs and keep the board) and
OpenCode's GitHub Copilot login (the model the agents run on). They hold no other API keys.

## The dashboard: zboard

The owner follows the agents on one web page, **zboard**, served by the build box on its own
`127.0.0.1:8765` (a user service) and reached from the Mac over an SSH tunnel: `ssh -fN zombye-board` (an
alias forwarding `8765` to zboard and `8080` to the play stream), then **http://localhost:8765**, in a
browser or the Claude app's browser pane.

- **The screen** fills the middle: the selected issue's review pictures, newest first, as they're rendered
  (a strip of all of them underneath), or, on **Play**, the game itself.
- **The issues** down the side, like a task list: each one's state (working, ready for review, finished,
  failed), time, pictures, and only its short to-do list (agents keep one with their todo tool, as the
  brief asks), or else its latest line. The full log is under **Details**.
- **Actions:** **Start** any open issue (grouped by milestone) with a model; **Send** feedback to the
  selected agent (`zwork say`); **Stop** it; **Approve**: mark the PR ready, squash-merge it (which closes
  the issue), set the board to Done, remove the worktree and pull `~/zombye`; **Play this build** or **Play
  main**; **Close game**.

It's plain Python (`tools/remote/zboard`) over what `zwork` leaves in `~/wt`; everything it does is a
`zwork`, `zgpu` or `gh` command that works without it too.

## Remote play

**Play** builds that issue's client on the build box (`zwork play N`, or `main`: `cargo build --profile
play`, quick with the shared cache), copies it to the play machine (`zgpu --play`) and starts it there in
a real window, fullscreen on a desktop drawn by the GPU, replacing any game already running. **Amazon DCV**
shares that desktop: the A10G encodes it as H.264 video (NVENC), and its web client plays it in zboard's
screen with keyboard and mouse going back. Click into it to play.

- **Where it runs:** the GPU box (`zombye-gpu`) unless `ZOMBYE_PLAY_HOST` names another machine. Most of
  the lag is distance: from the owner's Mac, us-east-1 is about 88 ms away and London about 11 ms, so a
  small GPU box in the nearest region (`zombye-play`) feels far better than one beside the build box.
- **How it's reached:** DCV listens on the machine's localhost only, without its own login; nginx serves
  it as plain HTTP on `:8080` (a browser takes localhost without a certificate, and nginx presents DCV's own
  origin, which DCV checks), and only the build box and the owner's tunnel can reach that port.
- **Set up** with `tools/remote/dcv.sh` on the GPU machine: an X desktop on the NVIDIA driver with a
  virtual 1920x1080 screen, logged in automatically, quietened (no update offers, crash reports,
  notifications, lock screen), DCV with one console session, and the nginx proxy. DCV is free on EC2 when
  the instance can read AWS's licence bucket (an instance role allowing `s3:GetObject` on
  `arn:aws:s3:::dcv-license.<region>/*`).

## How an issue goes

1. **Start.** From zboard, or from the Mac: `ssh zombye-build zwork start 97`. That makes the worktree `~/wt/97` on a
   new branch `issue-97` from `origin/main` and starts an OpenCode agent on it in tmux, with a brief
   (`zwork`'s `brief`): read the issue, set it In Progress, do only what it asks, test, render review
   pictures, commit with `Refs #97`, push, open a **draft PR** that says `Closes #97`, report.
2. **Watch.** `zwork ls` lists every issue being worked (running, finished or failed, commits, pictures,
   model); `zwork log 97` shows what its agent has said and done.
3. **Review.** The agent leaves its pictures in `~/wt/97/review/` (not committed); zboard's screen shows
   them as they appear. Without it: `scp 'zombye-build:wt/97/review/*.png' saves/review-97/` (git-ignored).
4. **Feedback.** `zwork say 97 "the panel overlaps the hotbar at 1280x720; move it up"` carries on the
   same agent session with that message: it changes, re-renders, pushes again.
5. **Playtest** (when pictures aren't enough): zboard's **Play this build**, streamed (Remote play). Or
   locally, on the Mac: `git fetch && git switch issue-97` and `cargo run -p zombye-client --profile play`
   (the Linux build doesn't run on the Mac).
6. **Land.** The owner approves (zboard's Approve, or by hand: merge the PR, which closes the issue, set
   its board Status to Done, `zwork rm 97`), and the Mac's checkout `git pull`s.

Agents never merge, push to main or force-push: the brief says so, and OpenCode on the box denies those
commands outright (`~/.config/opencode/opencode.json`), since it runs with `--auto` and nobody is there to
approve each command.

## Running many at once

- **What limits it is conflicts, not machines.** The build box has cores and memory for ten or more agents
  building at once; what goes wrong is two agents changing the same code. Start together issues that
  touch different areas (one in the sim, one in the HUD, one in data files); two that both rework the
  inventory panel go one after the other. A PR that conflicts after another lands: `zwork say N "rebase
  on origin/main and fix the conflicts"`.
- **Builds share a cache.** Every worktree has its own `target/`, but `sccache` (`RUSTC_WRAPPER`) shares
  compiled dependencies between them, so only the first build of Bevy is slow. Agents use `--profile play`
  for day-to-day builds; a full `--release` build takes about 4 minutes, mostly one core on the last crates
  (one codegen unit, thin LTO), and much less with the cache warm.
- **Models.** `zwork start N MODEL` picks one (`opencode models github-copilot` lists them); the default is
  `$ZWORK_MODEL`, else `github-copilot/claude-sonnet-5.5`. Each model uses up the enterprise's Copilot
  premium requests at its own rate, and every turn of an agent's work is a request: a strong model for
  hard issues, a fast cheap one for small, clear ones, and the Copilot usage page checked after the first
  batch.
- **Pictures.** `zgpu` renders in about 2.5 s; agents render as many as they need.

## zgpu: the client on the GPU box

`zgpu ARGS` runs `zombye ARGS` on the GPU box as if it ran here: `zgpu --render-to review/hud.png --hud
--load saves/x.zsave`. The client finds its assets by the absolute path of the checkout it was built in,
so `zgpu` mirrors everything to the same absolute paths there: the newest `target/release/zombye` or
`target/play/zombye` of the checkout you're in (or `$ZOMBYE_GPU_BIN`), that checkout's `assets/`, and every
file or folder the arguments name (saves, `--data`). The `--render-to` output comes back, and the exit
code, stdout and stderr are the client's. `ZOMBYE_CLIENT` points at it, so the MCP `screenshot` tool works
on the build box as documented in AGENTS.md. Pictures match the Mac's. `zgpu --play ARGS` starts the game
windowed on the GPU machine's desktop instead (see Remote play) and `zgpu --stop` closes it; with
`ZOMBYE_GPU_HOST` it targets another machine.

## Driving it from the Mac (Claude)

Everything is a command over SSH; the box's environment (`~/.zombye-env`, read first by `~/.bashrc`)
applies to non-interactive commands too.

```bash
ssh zombye-build zwork ls
ssh zombye-build 'zwork start 97 github-copilot/claude-opus-5.5'
ssh zombye-build 'zwork log 97 60'
scp 'zombye-build:wt/97/review/*.png' saves/review-97/
ssh zombye-build 'cd ~/wt/97 && cargo test -p zombye-sim --release'
```

Claude can also do an issue itself in a worktree over SSH instead of handing it to an agent: the same
worktree layout, `tmux` for anything long (a dropped SSH connection doesn't stop it).

## Setting the machines up

Any two Ubuntu 24.04 machines will do; what's been used: a build box with 192 cores, 384 GB and a 2 TB
disk (c7a.48xlarge), and a GPU box with one NVIDIA A10G (g5.2xlarge, from an image with the NVIDIA driver,
such as the Deep Learning Base AMI), plus optionally a play box like it near the owner. SSH only from the
owner's address and between the machines; nothing else is open.

1. On each: copy `tools/remote/` over and run `setup.sh build` or `setup.sh gpu`, optionally with a time
   to shut itself down (`setup.sh build "2026-10-31 20:00:00 UTC"`). It installs the build dependencies,
   Rust with mold, the repo in `~/zombye`, and on the build box OpenCode, Claude Code, `sccache`, `zgpu`,
   `zwork`, `zboard` (as a user service), the OpenCode guardrails and a note for agents.
2. Give the build box a key into the GPU box (and the play box) and aliases for them in its
   `~/.ssh/config`. On each GPU machine meant for play, run `dcv.sh`.
3. The owner signs in on the build box: `gh auth login` (plus `gh auth refresh -s project` for the
   board) and `opencode auth login` (GitHub Copilot).

A stopped machine still pays for its disk: delete them when the work is done.
