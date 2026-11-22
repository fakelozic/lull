# API: what Lull puts on the session bus

Lull has no network ports. It is reachable only on the user's D-Bus session bus, which only that user's programs can use.
There are no passwords on that bus, so the rule is: **everything here is safe for any of the user's programs to call.**
Nothing can read a file, write one, or make Lull open anything outside ~/Music.

Bus name `org.mpris.MediaPlayer2.lull`, object `/org/mpris/MediaPlayer2`. Not offered with `--no-window`.

## Public on purpose
| Interface | Member | Does | Bound |
|---|---|---|---|
| `org.mpris.MediaPlayer2` | `Raise` | Shows the window | Once per 300 ms if one is up |
| | `Quit` | Fades out, saves, quits | - |
| | properties | Identity and capabilities | Read-only |
| `...MediaPlayer2.Player` | `Play`, `Pause`, `PlayPause` | As named | - |
| | `Stop` | Pauses and keeps the place | On purpose: losing the place in an 8 h sound only hurts |
| | `Next`, `Previous` | Nothing | `CanGoNext` and `CanGoPrevious` are false |
| | `Seek(x)` | Moves by microseconds | Clamped to the sound |
| | `SetPosition(o, x)` | Moves to microseconds | Ignored for another track or out of range |
| | `OpenUri(s)` | Opens and plays a `file://` sound | Must resolve inside ~/Music, else AccessDenied |
| | `Volume` (read, write) | 0.0 to 1.0, the slider's value | Clamped, NaN ignored |
| | `Rate` (read, write) | Always 1.0. Writing 0 pauses | - |
| | `Seeked`, `PropertiesChanged` | Tell listeners | - |
| `local.lull.Control1` | `Show(s token, b play, x at, s file)` | What a second `lull` calls | Token 256 printable bytes, file inside ~/Music, `at` -1 for none |

The tray (ksni) also serves `org.kde.StatusNotifierItem` and `com.canonical.dbusmenu`, under
`org.kde.StatusNotifierItem-PID-1`: click, middle-click, scroll, and a menu with Play/Pause, Open, Quit.

Example:

```sh
playerctl -p lull play-pause
busctl --user call org.mpris.MediaPlayer2.lull /org/mpris/MediaPlayer2 local.lull.Control1 Show sbxs "" false -- -1 ""
```

## Limits against a hostile local program
| Attack | Limit | Where |
|---|---|---|
| Huge message | Over 32 KB is skipped unread | `vendor/zbus` patch |
| Flood of calls | 16 handled at once, the rest wait in the socket | `vendor/zbus` patch |
| Flood of activation tokens | 4 kept, 256 bytes each | `vendor/ksni` patch |
| Flood into main | The mailbox drops above 128 | `src/inbox.rs` |
| Flood of shows | One window start per 300 ms | `src/app/remote.rs` |
| Flood into the log | 30 lines per event per minute | `src/log/limit.rs` |

`scripts/desktop-e2e.sh` attacks every one of these and fails at 50 MB.

## The worst a local program could do
Play, pause, seek, change the volume, show or hide the window, or quit Lull. Nothing else.
