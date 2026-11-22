# Codebase map

| Path | What it does |
|---|---|
| `src/main.rs` | Entry point: wiring for both processes |
| `src/app/mod.rs` | The sound process's brain: intent, saving, the main loop |
| `src/app/window.rs` | The app's side of the window |
| `src/app/audio.rs` | What each piece of news from the audio thread changes |
| `src/app/health.rs` | What the app logs by itself: health, underruns, settled presses |
| `src/app/remote.rs` | The app's side of the tray, media keys and second launches |
| `src/remote/mod.rs` | The bus thread, and the checks on what comes in |
| `src/remote/mpris.rs` | Media controls |
| `src/remote/tray.rs`, `src/remote/icon.rs` | The tray icon and its drawing |
| `src/remote/control.rs` | Single instance: handing a launch over |
| `src/log/limit.rs` | Stops one event flooding the log |
| `vendor/zbus`, `vendor/ksni` | Patched copies. Read each LULL-PATCH.md |
| `src/front.rs` | Terminal or window, and the window's restarts |
| `src/host.rs` | Starts the window process and carries its messages |
| `src/link.rs` | Pure: the messages between the processes |
| `src/media.rs` | One opened file as the decoder sees it: containment, art hidden, read allowance |
| `src/inbox.rs` | Main's bounded mailbox |
| `src/private_file.rs` | Safe reads and atomic writes of our own small files |
| `src/sounds.rs` | Opens sounds, builds the Sounds list |
| `src/window/mod.rs` | The window process |
| `src/window/controller.rs` | Window logic, testable without a display |
| `src/window/painter.rs` | The animation clock, and noticing it can't be seen |
| `src/window/view.rs` | Pure: the words the window shows |
| `src/visual/ripples.rs` | Pure: the ring, drawn on the CPU |
| `src/visual/colour.rs` | Pure: OKLCH |
| `src/audio/spectrum.rs` | Pure: what the ring listens to |
| `ui/theme.slint` | Every colour, size and position |
| `ui/app.slint`, `ui/controls.slint`, `ui/sounds.slint` | The window, its controls, the Sounds list |
| `src/log/` | The logger: rotation, privacy, panics |
| `src/runinfo.rs` | Facts for the log, and the died-last-time marker |
| `src/config.rs` | Arguments and environment. The only place that reads the environment |
| `src/error.rs` | The one error type |
| `src/audio/engine.rs` | Pure mixing: position, loop, seek joins |
| `src/audio/engine_tests.rs` | The engine's promises, with fake sources |
| `src/audio/level.rs` | Pure: volume curve, fades, glide, overload guard |
| `src/audio/player.rs` | The audio thread: stream life, reconnect |
| `src/ffi/mod.rs` | The only unsafe folder: library loading |
| `src/ffi/opusfile.rs` | Safe Opus decoder, with exact seeking |
| `src/ffi/pulse.rs` | Safe playback stream with timeouts |
| `src/ffi/tty.rs` | Single-key terminal mode |
| `src/keys.rs` | Pure: bytes to actions |
| `src/state.rs` | State file and single-instance lock |
| `src/terminal.rs` | The `--no-window` front end |
| `src/timecode.rs` | Pure: time formatting and parsing |
| `src/track.rs` | Finds and names sounds |
| `tests/decoder.rs` | Real decoder: exact seeks, bad files |
| `tests/program.rs` | Real binary: signals, saving, lock, logs, no server |
| `tests/window.rs` | The real window in memory: every interaction, screenshots |
| `tests/panic_log.rs` | A panic reaches the log |
| `tests/e2e/` | Sample-level checks on a private sound server |
| `scripts/` | The check and test scripts, their sandboxes, and `install.sh` |
| `packaging/` | The app-menu entry and the app icon |

## To change...
| To change | Look at |
|---|---|
| Default volume | `state.rs` `DEFAULT_VOLUME`, and docs/decisions.md#volume |
| Volume curve | `audio/level.rs` `gain_for` |
| Fade length | `audio/level.rs` `FADE_FRAMES` |
| Loop or seek join | `audio/engine.rs` `LOOP_FRAMES`, `SEEK_FRAMES` |
| Buffer size, retry timing | `audio/player.rs` |
| Terminal keys | `keys.rs`, then `App::on_key` in `app/mod.rs`, then README |
| Window keys | `ui/app.slint`, then `tests/window.rs`, then README |
| Colours, fonts, positions | `ui/theme.slint` only |
| The ring | `visual/ripples.rs`. Its parity test must still pass |
| What the tray or media keys do | `remote/`, then `app/remote.rs`, then docs/api.md and the desktop test |
| Days of logs | `log/record.rs` `KEEP_DAYS` |
| A new message between processes | `link.rs`, `app/window.rs`, `window/controller.rs` |
