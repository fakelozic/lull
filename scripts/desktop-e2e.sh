#!/bin/sh
# Tray, media keys and single instance against real KDE parts, on private servers. Silent, invisible, about 2 min.
set -eu
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/share/mise/shims:$PATH"
cargo build --release --locked --quiet
out=target/desktop-e2e
mkdir -p "$out"
exec scripts/wayland-sandbox.sh "$out" /bin/sh /lull/scripts/sandbox-init.sh /usr/bin/python3 /lull/tests/e2e/desktop_e2e.py
