# Problem reports

The game and its server report their own problems to a small service of ours, so we don't depend on
players sending logs. An agent reads the reports there and turns them into issues.

## What is sent

| Kind | From | When | Grouped by |
|---|---|---|---|
| `panic` | game, server | a Rust panic: the message, where, the thread, a backtrace, the log's tail | the file and the message, numbers taken out |
| `crash` | game | the game ended badly without a panic (a signal, an exit code but 0 or 1) | how it ended |
| `f12` | game | the player pressed F12: screenshot, log, `about.txt`, the save (single-player, up to 12 MB), and the player's note | each one is its own problem |
| `hitch` | game | a frame over 1 s, the window focused, after the first 30 s; at most one every 15 minutes | the system |
| `desync` | game | our survivor's prediction corrected by over a metre 3 times in a minute; at most one every 15 minutes | single-player or multiplayer |
| `slow_tick` | server | a tick over 150 ms (three ticks' worth); at most one every 10 minutes | one problem |

Every report also has the build, the system, an anonymous id for the install (a hash of its identity key,
so reports from one install can be counted but not matched to a server's records), and a few details
(single-player or multiplayer, the tick, zombies in view, where the survivor stood, HP). The log's tail
says which GPU it ran on (Bevy logs the adapter at startup).

**What isn't sent:** no name or IP address. The service never stores an IP. It holds one in memory only,
for the rate limit (40 reports an hour from one address). The F12 `about.txt` names the server the
player was on. A log can hold the name the player joined a server under.

**Consent:** "Send problem reports" on the settings' Game tab (`send_reports` in `settings.ron`). It is on
unless turned off, and the first start of a packaged game says so on the yellow line. With it off nothing
is sent and F12 just writes its folder, as before. A server sends only when it has a report address, and
`--no-reports` turns that off. Builds from a checkout have no address, so development sends nothing
unless `ZOMBYE_REPORT_URL` is set.

## How it works

- `zombye_net::report` (`crates/net/src/report.rs`) has the report format, `queue` (to a `pending/`
  folder) and `flush` (one plain HTTP POST a report). A report made as the game falls over, or with the
  network down, goes the next time anything flushes. A report waiting more than 14 days is dropped.
- **F12 asks what happened:** once the screenshot is taken a box asks "What happened?" (up to 400
  characters; Enter sends, Esc sends without a note, every other key is held back meanwhile). The note
  goes into `about.txt` and the report's details as `what`. `--report-box TEXT` shows the box, for
  pictures. With reports off the folder is opened instead, the note already in `about.txt`.
- **The game** (`crates/client/src/report.rs`): the packaged game runs as a supervised child. Its panic
  hook leaves `logs/panic.zrpt`, and the parent sends that with the log as soon as the child has gone
  down. F12 uploads its folder once the screenshot is written, and the yellow line gives the report's
  number. Reports wait in `reports/pending/` in the player's folder.
- **The server** (`--host`, `crates/server/src/multiplayer.rs`): a panic hook sends the server's own
  panics, and the status report sends slow ticks. Reports wait in `reports/` beside the save.
- **Where to:** `ZOMBYE_REPORT_URL` (`http://HOST:PORT`) when the program runs, else the one compiled in
  by `tools/release/package.sh` from the same variable. `ZOMBYE_REPORT_KEY` the same way, if the service
  asks for one. The address is not kept in this repo.

## The service: `zombye-reports`

`crates/reports`: one binary with no dependencies outside the workspace except SQLite, which is built in.

```bash
cargo build --release -p zombye-reports
# The service: reports.db and files/<id>/ under --dir (default ~/zombye-reports)
ZOMBYE_REPORT_KEY=some-key target/release/zombye-reports serve --addr 0.0.0.0:7780 --max-gb 20
```

On vm-t3, run it in its own tmux session (as the game server is) or as a systemd user service. Players can
only reach it once its port is open (the Azure network security group and `ufw allow 7780/tcp`). The game
server on the same machine reports to it with `ZOMBYE_REPORT_URL=http://127.0.0.1:7780`. There's no TLS:
reports go in the clear. Put a reverse proxy with a domain in front of it before reports hold anything
sensitive. `GET /health` says `ok`.

### Questions an agent asks

Every command prints JSON. Run them over ssh on the machine with the store.

```bash
zombye-reports problems [--since 7] [--status new|filed|fixed|ignored|regressed] [--kind panic]
zombye-reports list --problem 12           # the reports of problem #12 (or --signature SIG, --kind K)
zombye-reports show 345                    # one report: everything, and files_dir with its files
zombye-reports mark '#12' --status filed --issue zombye#120 --note "first seen in playtest 11-13"
zombye-reports sql "SELECT build, COUNT(*) FROM reports WHERE kind = 'hitch' GROUP BY build"   # read-only
zombye-reports prune --days 90             # drop old reports and their files; problems stay
zombye-reports digest --hours 24           # plain text for a chat channel (not JSON); prints nothing if no reports
```

### The daily digest

Reports never become issues by themselves: one problem can bring hundreds. Instead,
`tools/reports/digest.sh` posts `digest` to a Discord channel once a day: how many reports from how
many installs, then a line a problem (regressed and new first, how often in the day and in all, the
builds, the latest report's number) with each player's F12 note quoted under it, cut to fit one message.
Run it from cron on the machine with the store:

```bash
0 8 * * * /path/to/zombye/tools/reports/digest.sh >> ~/zombye-reports/digest.log 2>&1
```

The webhook address is read from `ZOMBYE_REPORT_WEBHOOK` or `~/.config/zombye/report-webhook`, never
from the repo. Use a private channel: the notes are players' own words. A problem worth fixing is then
filed by hand or with the triage skill, and `mark`ed with its issue, so it shows as filed in the next digest.

`problems` lists regressed and new problems first, then the most reported. Each one has its count, how
many installs it reached, the builds and systems, and its latest report. A report for a problem marked
`fixed` turns it `regressed`.

The tables are `reports` (one row a report; `details` and `files` are JSON) and `problems` (one row a
signature: `status`, `issue`, `note`, `count`, `first_seen`, `last_seen`). See `SCHEMA` in
`crates/reports/src/main.rs`.

## Triage into GitHub issues

Follow the [triage-reports skill](../.claude/skills/triage-reports/SKILL.md). Briefly: take the new and
regressed problems, read a report or two of each (`show`, then its files), file one issue a problem in
deadcommons/zombye or add to the one it matches, and `mark` the problem with the issue. A player's F12 report
is read like a bug report: the screenshot, `about.txt` and the save.
