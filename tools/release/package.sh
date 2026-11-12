#!/usr/bin/env bash
# Package Zombye for playtesters: a folder holding the game, its assets and the bought art packed
# into zombye.pak (crates/client/src/pack.rs), ready for `butler push` to itch.io.
#
#   tools/release/package.sh linux      # on Linux (vm-t3)
#   tools/release/package.sh windows    # on Linux (vm-t3), cross-compiled: x86_64-pc-windows-gnu
#   tools/release/package.sh mac        # on the Mac: Zombye.app (Apple Silicon)
#
# Then:   tools/release/push.sh linux|windows|mac     (butler, to the itch.io page)
#
# Environment:
#   ZOMBYE_ART       the private art repo (default ~/games/zombye-art); its release-models,
#                    release-characters, release-anim and release-audio go into zombye.pak.
#                    Without it (or with ZOMBYE_NO_PACK=1) the build has this repo's own art only.
#   ZOMBYE_PACK_KEY  the key the pack is scrambled with (compiled into the game). Optional.
#   OUT              where packages go (default dist/).
#
# What's left out of assets/: Blender sources and specs (assets/source, assets/specs), and the
# placeholder crowd (assets/models/characters/zombie_*.zvat) when the release crowd is packed.
set -euo pipefail

platform="${1:?usage: package.sh linux|windows|mac}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
art="${ZOMBYE_ART:-$HOME/games/zombye-art}"
out="${OUT:-$root/dist}"
commit="$(git rev-parse --short HEAD)$(git diff --quiet HEAD -- crates assets || echo '+')"
export ZOMBYE_BUILD="playtest $(date -u +%Y-%m-%d) $commit"
echo "building $ZOMBYE_BUILD for $platform"

triple=""
features=""
exe=zombye
server=zombye-server
case "$platform" in
  linux) ;;
  mac) ;;
  windows)
    rustup target add x86_64-pc-windows-gnu >/dev/null
    triple=x86_64-pc-windows-gnu
    features="--features zombye-client/windows-gui"
    exe=zombye.exe
    server=zombye-server.exe
    ;;
  *) echo "unknown platform $platform (linux, windows or mac)" >&2; exit 2 ;;
esac

# The game for the platform, and (for the pack) the game for this machine.
# shellcheck disable=SC2086
cargo build --release -p zombye-client -p zombye-server ${triple:+--target $triple} $features
if [ -n "$triple" ]; then
  bin="target/$triple/release"
  cargo build --release -p zombye-client
else
  bin="target/release"
fi
host_client="target/release/zombye"

stage="$out/zombye-$platform"
rm -rf "$stage"
if [ "$platform" = mac ]; then
  app="$stage/Zombye.app"
  bindir="$app/Contents/MacOS"
  resdir="$app/Contents/Resources"
else
  bindir="$stage"
  resdir="$stage"
fi
mkdir -p "$bindir" "$resdir"

cp "$bin/$exe" "$bindir/"
cp "$bin/$server" "$bindir/"
if [ "$platform" = windows ]; then
  if command -v x86_64-w64-mingw32-strip >/dev/null; then
    x86_64-w64-mingw32-strip "$bindir/zombye.exe" "$bindir/zombye-server.exe"
  fi
elif command -v strip >/dev/null; then
  strip "$bindir/zombye" "$bindir/zombye-server" 2>/dev/null || true
fi

# The game's own assets (code-made art, data, shaders, fonts, sounds).
packed_crowd=0
[ -d "$art/release-characters/crowd" ] && [ "${ZOMBYE_NO_PACK:-0}" != 1 ] && packed_crowd=1
mkdir -p "$resdir/assets"
crowd_exclude=""
[ $packed_crowd = 1 ] && crowd_exclude="--exclude=./models/characters/zombie_*"
# shellcheck disable=SC2086
tar -C assets --exclude=./source --exclude=./specs --exclude=.DS_Store $crowd_exclude -cf - . | tar -C "$resdir/assets" -xf -
cp ATTRIBUTION.md assets/LICENSE "$resdir/"
cp tools/release/README-playtest.txt "$stage/README.txt"

# The bought art, packed.
if [ "${ZOMBYE_NO_PACK:-0}" != 1 ]; then
  dirs=""
  for d in release-models release-characters release-anim release-audio; do
    [ -d "$art/$d" ] && dirs="$dirs $art/$d"
  done
  if [ -n "$dirs" ]; then
    # shellcheck disable=SC2086
    "$host_client" --write-pack "$resdir/zombye.pak" $dirs
  else
    echo "warning: no release folders in $art: this build has the public art only" >&2
  fi
fi

if [ "$platform" = mac ]; then
  version="$(date -u +%Y.%m.%d)"
  cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Zombye</string>
  <key>CFBundleDisplayName</key><string>Zombye</string>
  <key>CFBundleIdentifier</key><string>com.deadcommons.zombye</string>
  <key>CFBundleExecutable</key><string>zombye</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
  # Signed ad hoc (no Apple Developer account yet): testers right-click, Open, the first time.
  codesign --force --deep -s - "$app"
fi

echo "$ZOMBYE_BUILD" > "$stage/build.txt"
du -sh "$stage"
echo "packaged: $stage"
