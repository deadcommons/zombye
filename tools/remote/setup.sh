#!/bin/bash
# Sets up a remote Zombye machine (Ubuntu 24.04, run as its login user). See docs/remote-dev.md.
#
#   setup.sh build [STOP_AT]   the build box: Rust, the game's build dependencies, OpenCode, Claude Code,
#                              the repo in ~/zombye, zgpu, and a note for agents working here
#   setup.sh gpu [STOP_AT]     the GPU box: the same minus the agents (it only runs the client;
#                              use an image with the NVIDIA driver, e.g. the Deep Learning Base AMI)
#
# STOP_AT (optional, e.g. "2026-10-31 20:00:00 UTC") schedules the machine to shut itself down then.
# Running it again is safe.
set -euo pipefail
role=${1:?usage: setup.sh build|gpu [STOP_AT]}
here=$(cd "$(dirname "$0")" && pwd)
export DEBIAN_FRONTEND=noninteractive

sudo apt-get update -q
sudo apt-get install -yq build-essential pkg-config clang mold cmake git tmux htop jq unzip rsync \
  libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev \
  mesa-vulkan-drivers vulkan-tools gh

# Rust, linking with mold (much faster links for Bevy on many cores)
if [ ! -x ~/.cargo/bin/cargo ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
source ~/.cargo/env
rustup component add clippy rustfmt
mkdir -p ~/.cargo
cat > ~/.cargo/config.toml <<'EOF'
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
EOF

[ -d ~/zombye ] || git clone https://github.com/deadcommons/zombye.git ~/zombye

if [ "$role" = build ]; then
  [ -x ~/.local/bin/claude ] || curl -fsSL https://claude.ai/install.sh | bash
  [ -x ~/.opencode/bin/opencode ] || curl -fsSL https://opencode.ai/install | bash
  mkdir -p ~/.local/bin ~/.config/opencode ~/.claude ~/wt
  install -m 755 "$here/zgpu" ~/.local/bin/zgpu
  install -m 755 "$here/zwork" ~/.local/bin/zwork
  install -m 755 "$here/zboard" ~/.local/bin/zboard
  # zboard, the agents' dashboard, as a user service (on 127.0.0.1:8765; reached over an SSH tunnel)
  mkdir -p ~/.config/systemd/user
  cat > ~/.config/systemd/user/zboard.service <<'EOF'
[Unit]
Description=zboard: the zwork agents' dashboard
[Service]
ExecStart=/bin/bash -c '. ~/.zombye-env; exec python3 ~/.local/bin/zboard 8765'
Restart=always
[Install]
WantedBy=default.target
EOF
  sudo loginctl enable-linger "$USER"
  systemctl --user daemon-reload
  systemctl --user enable --now zboard.service
  systemctl --user restart zboard.service
  # sccache: worktrees share compiled dependencies instead of each building Bevy from scratch
  command -v sccache >/dev/null || sudo apt-get install -yq sccache || cargo install sccache --locked
  # One environment for every shell, `ssh zombye-build 'cargo ...'` included: Ubuntu's .bashrc returns
  # early when not interactive, so this is read from its first line
  cat > ~/.zombye-env <<'EOF'
export PATH="$HOME/.local/bin:$HOME/.opencode/bin:$HOME/.cargo/bin:$PATH"
export ZOMBYE_CLIENT=$HOME/.local/bin/zgpu   # the MCP screenshot tool renders on the GPU box
export RUSTC_WRAPPER=sccache SCCACHE_CACHE_SIZE=200G
EOF
  grep -q zombye-env ~/.bashrc || sed -i '1i [ -f ~/.zombye-env ] \&\& . ~/.zombye-env' ~/.bashrc
  # Agents run with --auto (no one is there to approve each command), so the irreversible git and
  # GitHub moves are denied outright: merging and main stay with the owner
  [ -f ~/.config/opencode/opencode.json ] || cat > ~/.config/opencode/opencode.json <<'EOF'
{
  "$schema": "https://opencode.ai/config.json",
  "permission": {
    "bash": {
      "*": "allow",
      "git push --force*": "deny",
      "git push -f*": "deny",
      "git push * --force*": "deny",
      "git push * -f*": "deny",
      "git push * main*": "deny",
      "git push origin HEAD:main*": "deny",
      "gh pr merge*": "deny",
      "gh repo delete*": "deny",
      "gh repo edit*": "deny",
      "git worktree remove*": "deny",
      "rm -rf ~*": "deny",
      "rm -rf /*": "deny"
    }
  }
}
EOF
  # Global notes for agents here (OpenCode and Claude Code read these besides the repo's AGENTS.md)
  note='# This machine: the Zombye build box

No GPU. Build, test and run agents here, one git worktree per issue under ~/wt; pictures render on the
GPU box through `zgpu` (and the MCP `screenshot` tool, through $ZOMBYE_CLIENT). How it all fits:
docs/remote-dev.md in the repo.'
  echo "$note" > ~/.config/opencode/AGENTS.md
  echo "$note" > ~/.claude/CLAUDE.md
fi

if [ -n "${2:-}" ]; then
  sudo systemctl stop zombye-stop.timer 2>/dev/null || true
  sudo systemd-run --unit=zombye-stop --on-calendar="$2" /sbin/shutdown -h now
fi
echo "setup.sh $role: done"
