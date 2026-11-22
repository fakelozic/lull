#!/bin/sh
# Stress test: fails at 50 MB. Private servers only: silent and invisible. About 8 minutes plus the soak.
# Usage: scripts/stress.sh [SOAK_SECONDS] [test,names]
set -eu
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/share/mise/shims:$PATH"
cargo build --release --locked --quiet
out=target/stress
mkdir -p "$out"
exec scripts/wayland-sandbox.sh "$out" /bin/sh /lull/scripts/sandbox-init.sh /usr/bin/python3 -I /lull/tests/e2e/stress.py "$@"
