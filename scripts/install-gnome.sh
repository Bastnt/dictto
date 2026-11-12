#!/usr/bin/env bash
# Builds and installs Dictto for GNOME into the user's home. Run from the repo root on Fedora.
# Idempotent: safe to run again after every pull.
set -euo pipefail

cd "$(dirname "$0")/.."

uuid="dictto@dictto.app"
bin_dir="$HOME/.local/bin"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
ext_dir="$data_dir/gnome-shell/extensions/$uuid"
pkg="apps/gnome/packaging"

export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release -p dictto-gnome -p dictto-gnome-settings

# A running daemon keeps the old inode and would resurrect it as "(deleted)"; stop it first.
was_running=0
if gdbus call --session --dest com.dictto.App --object-path /com/dictto/App \
  --method com.dictto.App1.Quit >/dev/null 2>&1; then
  was_running=1
  sleep 1
fi

install -Dm755 target/release/dictto-gnome "$bin_dir/dictto-gnome"
install -Dm755 target/release/dictto-gnome-settings "$bin_dir/dictto-gnome-settings"

# check.sh, test.js and README.md are development files.
(
  cd apps/gnome/extension
  find . -type f ! -name check.sh ! -name test.js ! -name README.md -print0 |
    while IFS= read -r -d '' file; do
      install -Dm644 "$file" "$ext_dir/$file"
    done
)

mkdir -p "$data_dir/applications"
sed "s|@BIN@|$bin_dir|" "$pkg/com.dictto.App.desktop" >"$data_dir/applications/com.dictto.App.desktop"
install -Dm644 "$pkg/com.dictto.App.png" "$data_dir/icons/hicolor/512x512/apps/com.dictto.App.png"
update-desktop-database "$data_dir/applications" 2>/dev/null || true
gtk-update-icon-cache -q -t "$data_dir/icons/hicolor" 2>/dev/null || true

# Synthetic keystrokes go through /dev/uinput, which needs group access.
rule=/etc/udev/rules.d/70-dictto-uinput.rules
modules=/etc/modules-load.d/dictto-uinput.conf
need_sudo=0
cmp -s "$pkg/70-dictto-uinput.rules" "$rule" 2>/dev/null || need_sudo=1
cmp -s "$pkg/dictto-uinput.conf" "$modules" 2>/dev/null || need_sudo=1
id -nG "$USER" | tr ' ' '\n' | grep -qx input || need_sudo=1

if [ "$need_sudo" -eq 1 ]; then
  echo "Configuring /dev/uinput access (sudo)..."
  sudo install -Dm644 "$pkg/70-dictto-uinput.rules" "$rule"
  sudo install -Dm644 "$pkg/dictto-uinput.conf" "$modules"
  sudo modprobe uinput
  sudo usermod -aG input "$USER"
  sudo udevadm control --reload
  sudo udevadm trigger
fi

if [ "$was_running" -eq 1 ]; then
  setsid -f "$bin_dir/dictto-gnome" >/dev/null 2>&1 </dev/null
  echo "Restarted dictto-gnome."
fi

cat <<MSG

Installed. Log out and log back in: the input group and the new shell extension
are only picked up by a fresh session on Wayland. Then run:

  gnome-extensions enable $uuid

and start Dictto from the app grid (or run $bin_dir/dictto-gnome).
MSG
