---
name: triage-reports
description: Turn Zombye's problem reports (crashes, panics, F12, hitches, desyncs, slow ticks in the zombye-reports store) into GitHub issues on deadcommons/zombye. Use when asked to triage, check or summarise player reports or crashes.
---

# Triage problem reports

The store and what is in it: [docs/bug-reports.md](../../../docs/bug-reports.md). It lives on the
machine that runs `zombye-reports serve` (vm-t3, `~/zombye-reports`). Run every command there over ssh.
Each one prints JSON.

1. **What's waiting:** `zombye-reports problems --status new`, then `--status regressed`. Add
   `--since N` for the last N days (30 by default).
2. **Each problem:** `zombye-reports list --problem N --limit 3`, then `zombye-reports show ID` for
   the latest. Read its files in `files_dir`: `backtrace.txt` and the end of `zombye.log` for a panic
   or crash, and for an F12 report `screenshot.png`, `about.txt` and `world.zsave` (load it with
   `--load` or the MCP `load` to see the state). Find the code at the `location` and in the backtrace,
   at the commit in `build`.
3. **Already known?** Search deadcommons/zombye's issues for the message, the file or the symptom. If an issue
   matches, add a comment with the new count, installs and builds, and don't file another.
4. **File:** one issue per problem in deadcommons/zombye (it lands on the Game development board, project 3). Give it a plain title saying what goes wrong
   ("Game panics when a zombie climbs a fence on a chunk edge"), and in the description: how many
   reports, from how many installs, which builds and systems, first and last seen, the message and
   location, and the problem's number in the store. Put the backtrace and log lines in a collapsible
   "Agent notes" section. Label it `bug`. The repo is public: leave out anything that could identify a
   player (names, paths with a user name, addresses, chat lines).
5. **Record it:** `zombye-reports mark '#N' --status filed --issue zombye#M`. For noise, such as a
   one-off hitch on a slow machine or a report from a dev build, use
   `mark '#N' --status ignored --note why` instead.
6. **When a fix ships:** `mark '#N' --status fixed`. A new report for it turns the problem
   `regressed`. Check its `builds` first, because an old build still reporting the bug isn't a
   regression.

Never copy a player's save, screenshot or log into a public repo, issue or PR. Quote only the lines
that matter: the message, the location and the backtrace.
