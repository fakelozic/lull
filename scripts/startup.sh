#!/bin/sh
# How fast it opens: cold, warm, second launch, tray click. Private servers: silent and invisible. About 2 min.
set -eu
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/share/mise/shims:$PATH"
cargo build --release --locked --quiet
out=target/startup
mkdir -p "$out"
# kded's watcher, so there is a tray to click.
exec scripts/wayland-sandbox.sh "$out" /bin/sh /lull/scripts/sandbox-init.sh /bin/sh -c '
  mkdir -p "$HOME/.config"
  for module in /usr/lib64/qt6/plugins/kf6/kded/*.so; do
    name=$(basename "$module" .so)
    [ "$name" = statusnotifierwatcher ] || printf "[Module-%s]\nautoload=false\n" "$name"
  done > "$HOME/.config/kded6rc"
  QT_QPA_PLATFORM=offscreen kded6 > /out/kded.log 2>&1 &
  sleep 3
  exec /usr/bin/python3 /lull/tests/e2e/startup.py'
