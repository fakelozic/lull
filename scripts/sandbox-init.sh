#!/bin/sh
# Runs inside sandbox.sh only: starts the private sound server, then the test command.
set -eu
grep -q "^Max core file size *1 *1 " /proc/self/limits || { echo "refusing: crash dumps would reach the desktop" >&2; exit 1; }
[ ! -e /dev/snd ] || { echo "refusing: real sound devices are visible" >&2; exit 1; }
[ "$XDG_RUNTIME_DIR" = /sandbox/run ] || { echo "refusing: not in the sandbox" >&2; exit 1; }
conf=$HOME/.config/pipewire/pipewire.conf.d
mkdir -p "$conf"
cat > "$conf/sinks.conf" <<'CONF'
context.objects = [
  { factory = adapter args = { factory.name = support.null-audio-sink node.name = test_a media.class = Audio/Sink audio.position = [ FL FR ] priority.session = 2000 object.linger = true monitor.channel-volumes = false } }
  { factory = adapter args = { factory.name = support.null-audio-sink node.name = test_b media.class = Audio/Sink audio.position = [ FL FR ] priority.session = 1000 object.linger = true monitor.channel-volumes = false } }
]
CONF
pipewire > /out/pipewire.log 2>&1 &
sleep 1
wireplumber -p policy > /out/wireplumber.log 2>&1 &
sleep 1
pipewire-pulse > /out/pipewire-pulse.log 2>&1 &
sleep 2
exec "$@"
