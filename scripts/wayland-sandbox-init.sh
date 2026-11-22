#!/bin/sh
# Runs inside wayland-sandbox.sh only: starts a private bus and compositor, then the test command.
set -eu
grep -q "^Max core file size *1 *1 " /proc/self/limits || { echo "refusing: crash dumps would reach the desktop" >&2; exit 1; }
[ ! -e /dev/dri ] && [ ! -e /dev/snd ] || { echo "refusing: real devices are visible" >&2; exit 1; }
[ "$XDG_RUNTIME_DIR" = /sandbox/run ] || { echo "refusing: not in the sandbox" >&2; exit 1; }
# No service folders, so this bus can't start portals or anything else.
cat > /tmp/bus.conf <<'CONF'
<busconfig>
  <type>session</type>
  <listen>unix:path=/sandbox/run/bus</listen>
  <policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/><allow user="*"/></policy>
</busconfig>
CONF
dbus-daemon --config-file=/tmp/bus.conf --fork > /out/dbus.log 2>&1
export DBUS_SESSION_BUS_ADDRESS=unix:path=/sandbox/run/bus
# No screenshots from this KWin: ScreenShot2 segfaults it when composited without a GPU.
# The window tests take theirs from Xvfb instead (tests/e2e/window_e2e.py).
KWIN_COMPOSE=Q QT_QUICK_BACKEND=software \
  kwin_wayland --virtual --width 1920 --height 1080 --socket lull-test --no-lockscreen --no-global-shortcuts > /out/kwin.log 2>&1 &
tries=0
until [ -S "$XDG_RUNTIME_DIR/lull-test" ]; do
  tries=$((tries + 1))
  [ "$tries" -lt 100 ] || { echo "the private compositor did not start" >&2; cat /out/kwin.log >&2; exit 1; }
  sleep 0.2
done
export WAYLAND_DISPLAY=lull-test
exec "$@"
