#!/bin/sh
# Runs a command on a private, invisible KWin (Wayland, 1920x1080), for window tests.
# Usage: scripts/wayland-sandbox.sh OUT_DIR COMMAND [ARGS...]
# Inside: no GPU, no sound devices, no real runtime folder, no real session bus, no network.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
out=$(realpath "$1"); shift
# The host handles core dumps even for sandboxed processes, so a crash in here used to pop up
# KDE's crash reporter on the real desktop. A core limit of exactly 1 makes the kernel skip the
# dump handler, and KDE_DEBUG switches KCrash off.
exec prlimit --core=1:1 -- bwrap --die-with-parent --unshare-all --new-session \
  --ro-bind /usr /usr --ro-bind /etc /etc \
  --symlink usr/bin /bin --symlink usr/sbin /sbin --symlink usr/lib /lib --symlink usr/lib64 /lib64 \
  --proc /proc --dev /dev --tmpfs /tmp --tmpfs /run --tmpfs /var \
  --dir /sandbox --perms 0700 --dir /sandbox/run --dir /sandbox/home --dir /sandbox/state \
  --ro-bind "$HOME/Music" /sandbox/home/Music \
  --ro-bind "$HOME/.local/share/fonts" /sandbox/home/.local/share/fonts \
  --ro-bind "$root" /lull --bind "$out" /out \
  --clearenv --setenv KDE_DEBUG 1 --setenv PATH /usr/bin --setenv LANG C.UTF-8 --setenv HOME /sandbox/home \
  --setenv XDG_RUNTIME_DIR /sandbox/run --setenv XDG_STATE_HOME /sandbox/state \
  --setenv XDG_CURRENT_DESKTOP KDE --chdir /sandbox \
  /bin/sh /lull/scripts/wayland-sandbox-init.sh "$@"
