# Testing

| Command | Covers | Can it make sound? |
|---|---|---|
| `scripts/check.sh` | Format, lint, unit, decoder, window and program tests, release build | No: no test connects to a sound server |
| `scripts/window-e2e.sh` | Both processes: CPU, RAM, hiding, crash, close, real screenshots, flashing, sound/ring timing | No: private displays and sound server |
| `scripts/desktop-e2e.sh` | KDE's real tray service and widget, real clicks, playerctl, single instance, D-Bus attacks | No: private servers |
| `scripts/startup.sh` | Launch to first frame: warm, cold, second launch, tray click | No: private servers |
| `scripts/stress.sh [SOAK_SECONDS]` | 50 MB hard limit: hostile files, huge folders, real click and key storms, crash loops, scales | No: private display and sound server |
| `LULL_SHOTS=DIR cargo test --test window` | Writes a screenshot of every window state | No |
| `scripts/e2e.sh` | Real binary and real PipeWire: bit-exact output, fades, loop, devices, restarts, CPU, RAM | No: bwrap, no /dev/snd, virtual sinks only |
| `LULL_TEST_FILE=/path cargo test --release --test decoder -- --ignored --nocapture` | 1,000 exact seeks across a long real file | No: decode only |

`tests/fixtures/noise.opus` is 20 s of stereo pink noise at 127 kbps, made by ffmpeg. It has no third-party audio.

## Sandbox
Every sandbox runs under `prlimit --core=1:1`, or a crash inside opens KDE's crash reporter on the real desktop.

The window test uses three displays: headless Mutter for CPU and RAM (it draws), virtual KWin for the close request
(it doesn't composite: never take CPU from it), Xvfb for pictures.

`scripts/sandbox.sh OUT CMD` gives the command its own PipeWire, WirePlumber and PulseAudio server with two virtual
sinks (`test_a`, `test_b`). It has no network, session bus or real runtime folder, and refuses to run if it can see /dev/snd.

## Checking the C bindings
The signatures, constants and struct sizes in `src/ffi` were checked against the exact -devel packages, extracted
without installing:

    dnf download --repo=fedora --repo=updates --arch=x86_64 opusfile-devel pulseaudio-libs-devel
    rpm2cpio FILE.rpm | cpio -idm

Versions: opusfile 0.12-20.fc45, pulseaudio-libs 17.0-11.fc45. Re-check if you add a function.

## Installing
Checked by hand in the Wayland sandbox: install, menu listing, icon in a Papirus-style theme, launch from the entry,
KWin's window icon, second launch, update while running, uninstall. There is no script for it: repeat after changing
`scripts/install.sh` or `packaging/`.

## Not covered
- A click in the real Plasma menu, and the icon in the real taskbar.
- How it sounds: a person has to listen.
- Audible output on a physical sound device, sleep and resume, Bluetooth switching.
- The 8 hour soak (step 4).
- The tray on the real Plasma panel, hardware media keys, the notification's look, focus on the real KWin.
- Launch speed right after a reboot: the test drops only Lull's own files from the cache.
- Minimise and screen lock on the real KWin. The test stops the compositor's frames to imitate them.
- Real mouse and keys on Wayland. The tests drive the window in memory and on a private X screen.
