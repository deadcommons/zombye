#!/bin/bash
# The GPU box's screen: an X desktop on the NVIDIA GPU, shared with Amazon DCV, so the game can run in a
# real window and be played from a browser (zboard's Play). Run as the login user on the GPU box, after
# setup.sh gpu. See docs/remote-dev.md. Running it again is safe.
#
# DCV is free on EC2 when the instance can read its licence bucket (an instance role allowing
# s3:GetObject on arn:aws:s3:::dcv-license.<region>/*).
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

# A desktop on X (not Wayland: DCV's console session shares the X server on :0), with the NVIDIA driver
sudo apt-get install -yq ubuntu-desktop-minimal gdm3 xserver-xorg-core x11-xserver-utils xdotool mesa-utils
sudo sed -i 's/^#\?WaylandEnable=.*/WaylandEnable=false/' /etc/gdm3/custom.conf
grep -q '^WaylandEnable=false' /etc/gdm3/custom.conf || sudo sed -i '/^\[daemon\]/a WaylandEnable=false' /etc/gdm3/custom.conf
# No monitor is attached: give the GPU a virtual 1920x1080 screen
sudo nvidia-xconfig --preserve-busid --enable-all-gpus --connected-monitor=DFP-0 --virtual=1920x1080 >/dev/null
sudo systemctl set-default graphical.target
# Log the user in on :0 automatically, so the console session has a desktop to share
sudo sed -i '/^AutomaticLogin/d' /etc/gdm3/custom.conf
sudo sed -i '/^\[daemon\]/a AutomaticLoginEnable=true\nAutomaticLogin='"$USER" /etc/gdm3/custom.conf

# Amazon DCV (from AWS's download host)
if ! command -v dcv >/dev/null; then
  tmp=$(mktemp -d)
  curl -fsSL https://d1uj6qtbmh3dt5.cloudfront.net/NICE-GPG-KEY -o "$tmp/key" && gpg --import "$tmp/key" 2>/dev/null
  curl -fsSL https://d1uj6qtbmh3dt5.cloudfront.net/nice-dcv-ubuntu2404-x86_64.tgz -o "$tmp/dcv.tgz"
  tar -xzf "$tmp/dcv.tgz" -C "$tmp"
  sudo apt-get install -yq "$tmp"/nice-dcv-*/nice-dcv-server_*.deb "$tmp"/nice-dcv-*/nice-dcv-web-viewer_*.deb
  sudo usermod -aG video dcv
  rm -rf "$tmp"
fi

# Reached only through the SSH tunnel: listen on localhost, no login of its own, one console session for
# the user, and allow zboard's page to frame it
sudo tee /etc/dcv/dcv.conf >/dev/null <<EOF
[license]
[log]
level="info"
[session-management]
create-session = true
[session-management/automatic-console-session]
owner="$USER"
[display]
target-fps = 60
[connectivity]
web-listen-endpoints=['127.0.0.1:8443']
quic-listen-endpoints=['127.0.0.1:8443']
enable-quic-frontend=false
web-x-frame-options="ALLOWALL"
[security]
authentication="none"
EOF
sudo systemctl enable dcvserver

# DCV over plain HTTP on :8080, so a browser takes it without a certificate (localhost is still a secure
# context). Reached only through the build box (the firewall lets nothing else in), over the owner's
# tunnel to it. DCV checks the websocket's origin, so the proxy presents DCV's own.
sudo apt-get install -yq nginx wmctrl
sudo tee /etc/nginx/sites-available/dcv >/dev/null <<'EOF'
map $http_upgrade $connection_upgrade { default upgrade; "" close; }
server {
  listen 8080;
  location / {
    proxy_pass https://127.0.0.1:8443;
    proxy_ssl_verify off;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection $connection_upgrade;
    proxy_set_header Host localhost:8443;
    proxy_set_header Origin https://localhost:8443;
    proxy_read_timeout 1d;
    proxy_buffering off;
  }
}
EOF
sudo rm -f /etc/nginx/sites-enabled/default
sudo ln -sf /etc/nginx/sites-available/dcv /etc/nginx/sites-enabled/dcv
sudo systemctl reload nginx || sudo systemctl restart nginx

# A quiet desktop: no update offers, crash reports, printers, notifications, lock or wallpaper
sudo apt-get purge -yq update-notifier update-manager apport apport-gtk gnome-initial-setup || true
sudo systemctl disable --now cups cups-browsed 2>/dev/null || true
gs() { DISPLAY=:0 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus gsettings set "$@" 2>/dev/null || true; }
gs org.gnome.desktop.notifications show-banners false
gs org.gnome.desktop.screensaver lock-enabled false
gs org.gnome.desktop.screensaver idle-activation-enabled false
gs org.gnome.desktop.lockdown disable-lock-screen true
gs org.gnome.settings-daemon.plugins.power idle-dim false
sudo loginctl unlock-sessions
gs org.gnome.desktop.session idle-delay 0
gs org.gnome.shell.extensions.dash-to-dock autohide true
gs org.gnome.shell.extensions.dash-to-dock dock-fixed false
gs org.gnome.desktop.background picture-uri ''
gs org.gnome.desktop.background primary-color '#121214'
sudo systemctl restart gdm3 || true
sleep 5
sudo systemctl restart dcvserver
echo "dcv.sh: done"
