#!/usr/bin/env bash
# Upload a package made by package.sh to the itch.io page with butler.
#
#   ITCH_TARGET=user/zombye tools/release/push.sh linux|windows|mac
#
# butler needs a login once (`butler login`) or BUTLER_API_KEY set (an itch.io API key: never put
# it in this repo). Channels are linux, windows and mac; itch.io's app updates testers' copies.
set -euo pipefail
platform="${1:?usage: push.sh linux|windows|mac}"
target="${ITCH_TARGET:?set ITCH_TARGET to the itch.io page, e.g. yourname/zombye}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="${OUT:-$root/dist}/zombye-$platform"
[ -d "$stage" ] || { echo "no package at $stage: run tools/release/package.sh $platform first" >&2; exit 1; }
version="$(cat "$stage/build.txt" | tr ' ' '-')"
butler push "$stage" "$target:$platform" --userversion "$version"
butler status "$target:$platform"
