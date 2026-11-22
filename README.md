# Lull

A small, calm player for focus sounds on Linux: one long Opus file, looped without a seam, resumed where you left off.
**Working: the audio core, the window, and the tray with media keys.** Not done yet: sleep and resume, device-change hardening and a long soak test.
Costs about 20 MB with the window and 10 MB in the tray. Opens in about 50 ms.

## Needs
Fedora with PipeWire, plus the system's `opusfile` and `pulseaudio-libs`. Rust 1.92+ to build, on an x86-64-v3 CPU (2013 or newer).
No -devel packages and no sudo.

## Install
Build it first (below), then install. Open **Lull** from the app menu, or type `lull` in a terminal.

```sh
scripts/install.sh              # after a checked build: program, menu entry and icon, for this user. No sudo, no autostart
scripts/install.sh --uninstall  # removes those three. Keeps music, saved place and logs
```

A rebuild does not update the installed copy: run the installer again, then quit and reopen Lull.

## Build and run

```sh
cargo build --release
target/release/lull                 # opens the window: the last sound, or the first .opus in ~/Music, paused
target/release/lull --at 47:26      # opens at that time
target/release/lull --play          # starts playing straight away
target/release/lull path/to/a.opus  # opens that file
target/release/lull --no-window     # terminal controls instead of the window
```

| Key | Does |
|---|---|
| Space | Play or pause, with a 300 ms fade |
| Left, Right | 30 seconds back or forward |
| Down, Up | Quieter or louder |
| Tab | Move between the controls |
| Escape | Close the Sounds list, or cancel a drag |
| Ctrl+Q (also q or Ctrl+C in a terminal) | Fade out, remember the place, quit |

Closing the window keeps it in the tray, playing or paused. So does closing the terminal it was started from.
Running `lull` again brings it back (`--play`, `--at` and a file in ~/Music pass through). It always starts paused.

| Tray | Does |
|---|---|
| Click | Hide the window, or bring it to the front |
| Middle-click | Play or pause |
| Scroll | Volume |
| Right-click | Play/Pause, Open, Quit |

Media keys, Plasma's media widget and `playerctl -p lull` work too.
Where there is no tray, closing the window quits.

## If something breaks
Lull keeps the last 7 days of logs in `~/.local/state/lull/logs/`, one file a day. They stay on your computer.
[docs/operations.md](docs/operations.md) says how to read them.

## Check

```sh
scripts/check.sh   # format, lint, tests, build. Silent, about 10 s
scripts/e2e.sh     # the real binary on a private sound server. Silent, about 3 min
scripts/desktop-e2e.sh # tray, media keys, single instance and D-Bus attacks on real KDE parts. About 4 min
scripts/startup.sh     # how fast it opens. About 2 min
scripts/stress.sh 600  # hard 50 MB limit under attack. Silent and invisible, about 20 min
scripts/window-e2e.sh  # both processes on private displays. Silent and invisible, about 4 min
```

## Docs
- [Architecture](docs/architecture.md)
- [Codebase map](docs/codebase-map.md)
- [Decisions](docs/decisions.md)
- [What it stores](docs/data.md)
- [The D-Bus API and its limits](docs/api.md)
- [Operations: logs and fixes](docs/operations.md)
- [Testing](docs/testing.md)

## License
Lull is free software: [GPL-3.0-only](LICENSE). Copyright (C) 2026 fakelozic. It comes with no warranty.

Included work by others keeps its own license:

| Part | License |
|---|---|
| `vendor/zbus` (patched) | [MIT](vendor/zbus/LICENSE) |
| `vendor/ksni` (patched) | [Unlicense](vendor/ksni/UNLICENSE) |
| `ui/fonts` | [SIL Open Font License](ui/fonts/LICENSES.txt) |

The window is built with [Slint](https://slint.dev), used under GPL-3.0-only.
