#!/usr/bin/env bash
# Builds and tests the GNOME app on Linux (WSL Ubuntu or Fedora). Never writes into the repo target/.
set -euo pipefail

cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/dictto-target}"

packages=(libgtk-4-dev libadwaita-1-dev libpipewire-0.3-dev libclang-dev libdbus-1-dev pkg-config)
missing=()
for pkg in "${packages[@]}"; do
  dpkg -s "$pkg" >/dev/null 2>&1 || missing+=("$pkg")
done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "missing packages: ${missing[*]}" >&2
  echo "sudo apt install ${missing[*]}" >&2
  exit 1
fi

cargo build -p dictto-gnome
cargo test -p dictto-gnome
