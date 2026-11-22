#!/bin/sh
# Silent end-to-end test of the release build on a private sound server. About three minutes.
# Needs: bwrap, pipewire, wireplumber, pactl, parec, cc, and the rain file in ~/Music.
set -eu
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/share/mise/shims:$PATH"
cargo build --release --locked --quiet
out=target/e2e
rm -rf "$out"
mkdir -p "$out"
cc -O2 -o "$out/reference" tests/e2e/reference.c
"$out/reference" "$HOME/Music/Rain on Window with Thunder (8h).opus" 136320000 576000 "$out/reference.raw"
status=0
scripts/sandbox.sh "$out" /usr/bin/python3 -I /lull/tests/e2e/e2e.py || status=$?
exit "$status"
