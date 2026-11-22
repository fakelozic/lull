#!/bin/sh
# Silent, invisible end-to-end test of the window: private KWin and private PipeWire. About four minutes.
set -eu
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/share/mise/shims:$PATH"
cargo build --release --locked --quiet
out=target/window-e2e
mkdir -p "$out"
exec scripts/wayland-sandbox.sh "$out" /bin/sh /lull/scripts/sandbox-init.sh /usr/bin/python3 /lull/tests/e2e/window_e2e.py
