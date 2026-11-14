#!/usr/bin/env bash
# Post the last day's problem reports, grouped by problem, to a Discord channel (docs/bug-reports.md).
#
#   tools/reports/digest.sh [HOURS]
#
# Run it once a day from cron on the machine that runs `zombye-reports serve`:
#
#   0 8 * * * /path/to/zombye/tools/reports/digest.sh >> ~/zombye-reports/digest.log 2>&1
#
# The webhook address is a secret and never kept in the repo: it is read from $ZOMBYE_REPORT_WEBHOOK,
# else the file ~/.config/zombye/report-webhook (one line, readable by you alone). Make the webhook in
# Discord: the channel's settings, Integrations, Webhooks. A private channel keeps players' notes
# private. With no reports in the time, nothing is posted.
#
# ZOMBYE_REPORTS is the zombye-reports binary (default: the release build beside this checkout, else
# the one on PATH); ZOMBYE_REPORTS_DIR is the store, as for the binary itself.
set -euo pipefail

hours="${1:-24}"
here="$(cd "$(dirname "$0")/../.." && pwd)"
bin="${ZOMBYE_REPORTS:-$here/target/release/zombye-reports}"
[ -x "$bin" ] || bin="$(command -v zombye-reports || true)"
[ -n "$bin" ] || { echo "digest: no zombye-reports binary (cargo build --release -p zombye-reports)" >&2; exit 1; }

hook="${ZOMBYE_REPORT_WEBHOOK:-}"
if [ -z "$hook" ] && [ -r "$HOME/.config/zombye/report-webhook" ]; then
  hook="$(head -n1 "$HOME/.config/zombye/report-webhook")"
fi

text="$("$bin" digest --hours "$hours" --max-chars 1900)"
if [ -z "$text" ]; then
  echo "digest: $(date -u +%FT%TZ) no reports in the last $hours h"
  exit 0
fi
if [ -z "$hook" ]; then
  echo "digest: no webhook (ZOMBYE_REPORT_WEBHOOK or ~/.config/zombye/report-webhook); the digest is:" >&2
  printf '%s\n' "$text"
  exit 1
fi

body="$(printf '%s' "$text" | python3 -c 'import json, sys; print(json.dumps({"content": sys.stdin.read(), "allowed_mentions": {"parse": []}}))')"
curl -fsS -X POST -H 'Content-Type: application/json' --data "$body" "$hook" > /dev/null
echo "digest: $(date -u +%FT%TZ) posted ($(printf '%s' "$text" | wc -l | tr -d ' ') lines)"
