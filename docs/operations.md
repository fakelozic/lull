# Operations: logs and fixes

## The installed copy
| File | What |
|---|---|
| `~/.local/bin/lull` | The program you run (a copy, not a link into `target/`) |
| `~/.local/share/applications/lull.desktop` | The menu entry. Its name must match the app id `lull` |
| `~/.local/share/icons/hicolor/scalable/apps/lull.svg` | The icon |

`scripts/install.sh` writes them, `scripts/install.sh --uninstall` removes them. `built=` in `app.start` says which copy ran.

## Logs
- Folder: `~/.local/state/lull/logs/`, one file a day: `lull-YYYY-MM-DD.log` (UTC dates and times).
- The last 7 days are kept. Older files are deleted at start and when the day changes. Each day is capped at 2 MB.
- Estimate: 20-100 lines a day, about 5-25 KB. At the cap, 14 MB in total.
- Local only. The home folder is written as `~`. Files are mode 600.
- One JSON object per line: `time, level, event, process (sound or window), run, pid, detail`.
  `run` joins both processes of one launch.

```sh
cd ~/.local/state/lull/logs
tail -n 50 "$(ls lull-*.log | tail -1)"        # the latest lines
grep -hE '"level":"(warn|error)"' lull-*.log   # everything that went wrong this week
grep -h '"run":"RUN_ID"' lull-*.log            # one whole launch
```

## Events
| Event | Means | Look at |
|---|---|---|
| `app.start` | A launch: version, build time, system, options | Was it the build you think? |
| `app.ready`, `track.loaded`, `track.switched`, `app.no_sounds` | What opened | `src/main.rs` |
| `audio.busy` | A press was dropped: the audio thread was stuck | `audio.lost` just before it |
| `app.launched_again`, `app.handed_over` | A second launch went to the running Lull | - |
| `app.terminal_closed` | The launching terminal closed. It carried on | - |
| `app.health` | After 5 min, then hourly: memory, underruns, position | Memory that keeps rising is a leak |
| `app.previous_run_died` | The last run was killed, crashed hard or lost power | That run's last lines, then `coredumpctl list lull` |
| `app.failed` | Lull could not carry on | The detail |
| `process.panic` | A bug, with thread, file, line and stack | That file and line |
| `audio.connected`, `audio.restored`, `audio.device` | Stream opened or moved | - |
| `audio.lost` | No server or no device. It retries | `systemctl --user status pipewire pipewire-pulse` |
| `audio.underrun` | An audible gap. At most one line a minute | System load, `pw-top` ERR column |
| `audio.failed` | The file can't be read | `ffprobe` the file |
| `audio.paused` | The fade was heard and the stream closed | - |
| `user.play`, `user.pause`, `user.seek`, `user.volume`, `user.open` | What the person did before a problem | - |
| `window.starting`, `window.opened`, `window.visible`, `window.hidden` | The window's life | - |
| `window.first_frame` | How many ms the window took to appear. Over 150 is a regression | `scripts/startup.sh` |
| `window.warm_up_failed` | No thread for the ring tables | - |
| `remote.ready`, `remote.unavailable`, `remote.signal_failed` | The bus side | `busctl --user list` |
| `tray.started`, `tray.online`, `tray.offline`, `tray.unavailable`, `tray.clicked` | The tray icon | Is Plasma's tray running? |
| `tray.notice_failed`, `tray.notice_not_saved` | The one-time notice | - |
| `log.suppressed` | An event repeated over 30 times a minute, and this many lines were dropped | Who is calling Lull in a loop |
| `window.crashed`, `window.restarted`, `window.gave_up` | The window died: exit status or signal | `window.stderr` just before it |
| `window.stderr` | What the toolkit printed | - |
| `window.killed`, `window.stuck`, `window.start_failed` | A stuck or unstartable window | - |
| `window.send_failed`, `window.bad_message`, `window.hide_failed`, `window.quit_failed` | Window plumbing problems | `src/window/mod.rs` |
| `state.reset`, `state.save_failed` | The saved place was damaged or can't be written | Disk space, permissions |
| `shutdown.start`, `shutdown.done`, `shutdown.forced` | The reason it stopped, and whether cleanly | - |
| `log.unavailable`, `log.write_failed`, `log.prune_failed`, `log.full`, `log.marker_failed` | The log itself is in trouble (some go to stderr only) | Disk space, permissions |

## Common fixes
| Symptom | Fix |
|---|---|
| "Lull is already running" | It didn't answer on the bus for 3 s. `pkill -TERM -x lull` (fades out and saves) |
| A fix didn't change anything | The installed copy is older: `cmp ~/.local/bin/lull target/release/lull`, then `scripts/install.sh` |
| No tray icon | Look in the tray's hidden-icons arrow, then for `tray.` lines in the log |
| Won't open its window | `lull --no-window` still plays. Look for `window.stderr` |
| Wrong place or volume | Quit, then delete `~/.local/state/lull/state` |
| Silent but "Playing" | `audio.device` in the log shows where it plays. Check that device's volume |
