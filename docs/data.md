# What Lull stores

One file: `$XDG_STATE_HOME/lull/state`, normally `~/.local/state/lull/state`. Folder mode 700, file mode 600.

```
lull-state 1
position=136541234
volume=20
track=/home/you/Music/Rain.opus
```

- `position`: frames at 48 kHz (divide by 48000 for seconds).
- `volume`: 0-100.
- `track`: an absolute path as raw bytes. It is last because file names may contain newlines.
- Whether it was playing is never stored: Lull always opens paused.

Written every 30 s if changed, on pause and on quit, by writing `state.tmp`, flushing and renaming.
A damaged or unknown file is logged and replaced by defaults. Deleting the file is a safe reset.

The lock is `$XDG_RUNTIME_DIR/lull.lock`. The kernel releases it if Lull dies.
`tray-notice-shown` exists once the "still in the tray" notice was shown.

`running` exists only while Lull runs. Finding it at start means the last run died.

`logs/` holds 7 days of logs (docs/operations.md): file paths with the home folder hidden, and what was pressed.
Nothing is sent anywhere.
