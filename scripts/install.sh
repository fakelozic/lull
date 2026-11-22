#!/bin/sh
# Installs the built Lull for this user only: the program, its app-menu entry and its icon. No sudo, no autostart.
# Usage: scripts/install.sh [--uninstall]
# Build and check first (scripts/check.sh). A running Lull keeps its old version until it is quit.
set -eu
cd "$(dirname "$0")/.."

data=${XDG_DATA_HOME:-$HOME/.local/share}
bin=$HOME/.local/bin/lull
entry=$data/applications/lull.desktop
icons=$data/icons/hicolor
icon=$icons/scalable/apps/lull.svg

refresh() {
  # An existing GTK cache hides new icons until it is rebuilt.
  if [ -f "$icons/icon-theme.cache" ] && command -v gtk-update-icon-cache > /dev/null; then
    gtk-update-icon-cache --force --ignore-theme-index --quiet "$icons"
  fi
  if command -v kbuildsycoca6 > /dev/null; then
    kbuildsycoca6 > /dev/null 2>&1 || echo "Plasma's menu didn't refresh. It will at the next login." >&2
  fi
}

case ${1-} in
--uninstall)
  rm -f "$bin" "$entry" "$icon"
  refresh
  echo "Removed Lull. Your music, saved place and logs are untouched."
  exit 0 ;;
"") ;;
*) echo "Usage: $0 [--uninstall]" >&2; exit 2 ;;
esac

built=target/release/lull
[ -x "$built" ] || { echo "Not built yet: run scripts/check.sh" >&2; exit 1; }
if [ -n "$(find src ui vendor .cargo build.rs Cargo.toml Cargo.lock -type f -newer "$built" -print -quit)" ]; then
  echo "The build is older than the source: run scripts/check.sh" >&2
  exit 1
fi
"$built" --version > /dev/null

# The entry needs the full path, because ~/.local/bin isn't on Plasma's PATH.
case $bin in
*[!A-Za-z0-9/._\ -]*) echo "Can't write a menu entry for this path: $bin" >&2; exit 1 ;;
esac

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
sed "s|@BIN@|$bin|g" packaging/lull.desktop > "$stage/lull.desktop"
if command -v desktop-file-validate > /dev/null; then
  desktop-file-validate "$stage/lull.desktop"
fi

# Each file appears whole or not at all, and a running Lull keeps the file it started from.
put() {
  mkdir -p "$(dirname "$3")"
  install -m "$1" "$2" "$3.new"
  mv -f "$3.new" "$3"
}
put 755 "$built" "$bin"
put 644 packaging/lull.svg "$icon"
put 644 "$stage/lull.desktop" "$entry"
refresh

echo "Installed Lull:"
printf '  %s\n' "$bin" "$entry" "$icon"
