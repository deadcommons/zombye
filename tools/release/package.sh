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
#                    release-characters, release-anim and release-audio go into zombye.pak, and its
#                    release-brand/official_servers.ron becomes the build's official server list.
#                    Without it (or with ZOMBYE_NO_PACK=1) the build has this repo's own art only.
#                    Its release-brand pictures go beside the game (window icon, logo) and its
#                    zombye.ico / zombye.icns / icon.png become the app's icon (#76).
#   ZOMBYE_PACK_KEY  the key the pack is scrambled with (compiled into the game). Needed whenever the
#                    bought art is packed: the built-in key is in the public source, so a pack made with
#                    it can be unpacked by anyone. Keep it out of this repo and its history.
#   ZOMBYE_REPORT_URL  where the game and its server send problem reports (`http://HOST:7780`, the
#                    report service, docs/bug-reports.md; compiled in). Without it nothing is sent.
#                    Never put the address in this repo.
#   ZOMBYE_REPORT_KEY  the key the report service asks for (compiled in). Optional.
#   OUT              where packages go (default dist/).
#   ZOMBYE_SIGN_ID   (mac) a "Developer ID Application: ..." identity in the keychain; without it the
#                    app is signed ad hoc.
#   ZOMBYE_NOTARY_PROFILE  (mac) a notarytool keychain profile (xcrun notarytool store-credentials);
#                    with it the signed app is notarised and stapled.
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
if [ -n "${ZOMBYE_REPORT_URL:-}" ]; then
  echo "problem reports go to the report service (ZOMBYE_REPORT_URL)"
  export ZOMBYE_REPORT_URL ZOMBYE_REPORT_KEY="${ZOMBYE_REPORT_KEY:-}"
else
  echo "no ZOMBYE_REPORT_URL: this build sends no problem reports"
fi

# The bought art may only ship scrambled with a key of our own (see ZOMBYE_PACK_KEY above).
if [ "${ZOMBYE_NO_PACK:-0}" != 1 ] && [ -z "${ZOMBYE_PACK_KEY:-}" ]; then
  for d in release-models release-characters release-anim release-audio; do
    if [ -d "$art/$d" ]; then
      echo "error: ZOMBYE_PACK_KEY isn't set, and the bought art in $art would be packed with the public key." >&2
      echo "       Set ZOMBYE_PACK_KEY, or ZOMBYE_NO_PACK=1 for a build with this repo's own art only." >&2
      exit 1
    fi
  done
fi

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
    # The .exe's icon, embedded by crates/client/build.rs (the icon file never comes into this repo).
    if [ -f "$art/release-brand/zombye.ico" ]; then
      export ZOMBYE_WINDOWS_ICON="$art/release-brand/zombye.ico"
      echo "exe icon: $ZOMBYE_WINDOWS_ICON"
    else
      echo "exe icon: none ($art/release-brand/zombye.ico isn't there)"
    fi
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
# The official servers (the server list's first section): the art repo's list in place of this
# repo's empty one. No server address is kept in this repo.
if [ -f "$art/release-brand/official_servers.ron" ]; then
  cp "$art/release-brand/official_servers.ron" "$resdir/assets/data/official_servers.ron"
  echo "official servers: $art/release-brand/official_servers.ron"
else
  echo "official servers: none ($art/release-brand/official_servers.ron isn't there): the build lists no official server"
fi
# The brand's pictures (window icon, logo, wordmark, the Z), read as release-brand beside the game
# (crates/client/src/release_brand.rs), and the app's icon for each platform.
brand="$art/release-brand"
if ls "$brand"/*.png >/dev/null 2>&1; then
  mkdir -p "$resdir/release-brand"
  cp "$brand"/*.png "$resdir/release-brand/"
  echo "brand: $brand"
  if [ "$platform" = mac ] && [ -f "$brand/zombye.icns" ]; then
    cp "$brand/zombye.icns" "$resdir/zombye.icns"
  fi
  if [ "$platform" = linux ] && [ -f "$brand/icon.png" ]; then
    cp "$brand/icon.png" "$stage/zombye.png"
    cat > "$stage/zombye.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Zombye
Comment=Zombie survival
Exec=zombye
Icon=zombye
Terminal=false
Categories=Game;
DESKTOP
  fi
else
  echo "brand: none ($brand has no pictures): no logo or icon in this build"
fi
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
  icon_key=""
  [ -f "$resdir/zombye.icns" ] && icon_key="<key>CFBundleIconFile</key><string>zombye</string>"
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
  $icon_key
</dict>
</plist>
PLIST
  if [ -n "${ZOMBYE_SIGN_ID:-}" ]; then
    # Developer ID, hardened runtime, then notarised and stapled so Gatekeeper opens it at once.
    codesign --force --options runtime --timestamp -s "$ZOMBYE_SIGN_ID" "$bindir/zombye-server"
    codesign --force --options runtime --timestamp -s "$ZOMBYE_SIGN_ID" "$app"
    codesign --verify --strict --verbose=2 "$app"
    if [ -n "${ZOMBYE_NOTARY_PROFILE:-}" ]; then
      zip="$out/Zombye-notarize.zip"
      rm -f "$zip"
      ditto -c -k --keepParent "$app" "$zip"
      xcrun notarytool submit "$zip" --keychain-profile "$ZOMBYE_NOTARY_PROFILE" --wait
      xcrun stapler staple "$app"
      rm -f "$zip"
      spctl --assess --type execute --verbose=2 "$app"
    else
      echo "warning: ZOMBYE_NOTARY_PROFILE not set: signed but not notarised" >&2
    fi
  else
    # Signed ad hoc: testers allow it once in System Settings > Privacy & Security.
    codesign --force --deep -s - "$app"
  fi
fi

echo "$ZOMBYE_BUILD" > "$stage/build.txt"
du -sh "$stage"
echo "packaged: $stage"
