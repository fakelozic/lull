#!/bin/sh
# Runs a command next to a private PipeWire that has only two virtual sinks, for silent end-to-end tests.
# Usage: scripts/sandbox.sh OUT_DIR COMMAND [ARGS...]
# Inside: no /dev/snd, no real runtime folder, no session bus, no network. It cannot reach the speakers.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
out=$(realpath "$1"); shift
music="$HOME/Music"
# The host handles core dumps even for sandboxed processes, so a crash in here used to pop up
# KDE's crash reporter on the real desktop. A core limit of exactly 1 makes the kernel skip the
# dump handler, and KDE_DEBUG switches KCrash off.
exec prlimit --core=1:1 -- bwrap --die-with-parent --unshare-all --new-session \
  --ro-bind /usr /usr --ro-bind /etc /etc \
  --symlink usr/bin /bin --symlink usr/sbin /sbin --symlink usr/lib /lib --symlink usr/lib64 /lib64 \
  --proc /proc --dev /dev --tmpfs /tmp --tmpfs /run --tmpfs /var \
  --dir /sandbox --perms 0700 --dir /sandbox/run --dir /sandbox/home --dir /sandbox/state \
  --ro-bind "$music" /sandbox/home/Music --ro-bind "$root" /lull --bind "$out" /out \
  --clearenv --setenv KDE_DEBUG 1 --setenv PATH /usr/bin --setenv LANG C.UTF-8 --setenv HOME /sandbox/home \
  --setenv XDG_RUNTIME_DIR /sandbox/run --setenv XDG_STATE_HOME /sandbox/state \
  --setenv DBUS_SESSION_BUS_ADDRESS unix:path=/nonexistent --chdir /sandbox \
  /bin/sh /lull/scripts/sandbox-init.sh "$@"
