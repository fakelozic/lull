# Architecture

As of step 3.

    lull (sound process)                                         lull --window (exits when closed)
    +--------------------------------------------------+         +-----------------------------+
    | main: App -- intent, saving, logging             | socket  | Controller -- Slint window  |
    |   |  ^                                           | <-----> | Painter -- Ripples, 30 fps  |
    |   |  +-- Event: state, levels, device            |  pair   +-----------------------------+
    |   +----- Command                                 |
    | audio thread: Engine + Spectrum --> Output       |
    +-----|-----------------------------------|--------+
          | f32 48 kHz                        v
     2 Opus decoders                    PipeWire --> default device
          ^
     Media: one open file from ~/Music, art hidden, reads capped

    tray, media keys, a second `lull` --D-Bus--> remote thread --Ask--> main
                                                              <--Snapshot--

    both processes --> ~/.local/state/lull/logs (7 days)

## Two processes
- The window holds no audio. A crash or freeze in it can't touch the sound: main restarts it (3 times a minute at most).
- Its memory all comes back when it exits. If the sound process dies, the window exits.
- They talk over an inherited socket pair (`src/link.rs`): no port, no path. The window picks sounds by row number.

## Threads
| Thread | Does | Never does |
|---|---|---|
| main | Intent (playing or not), saving, logging, answering the window | Touch audio |
| audio | Decode, mix, analyse, write. Reconnect with backoff | Log, write to disk, wait without a timeout while streaming |
| remote, zbus and ksni executors | The bus: tray, MPRIS, single instance | Make main wait, touch files except checking a path |
| to-window, from-window, window-errors | Move lines between the processes | Decide anything |
| keys (`--no-window`) | Turn bytes into actions | Anything else |

## Opening fast
1. Main takes the lock, or hands over to the running Lull and exits (12 ms).
2. The window process starts first. The bus, libraries and decoders start alongside it.
3. The window builds the ring's tables while the toolkit starts, and waits for the first snapshot before showing,
   so the first frame is the whole picture. About 50 ms, or 70 ms with cold file caches.

## Tray and window
- Closing puts the window process away and all its RAM comes back. The sound process stays, about 10 MB.
- Wayland can't raise an existing window from outside, so showing one that's behind replaces it: the new one appears
  with the click's activation token, then the old one goes.

## Life of a play
1. Paused: no stream, no decoding, no drawing. 0% CPU.
2. Play: connect (3 s limit), then fade in over 300 ms. The server keeps 300 ms queued.
3. The analyser runs on the mix before volume. Each result is dated to when it will be heard, and the window waits until then.
4. Pause or quit: fade out, send 500 ms of silence so the fade is heard, close the stream, save.

## Joins
The engine owns two decoders of the same file. A join mixes both with equal-power gains, then swaps them.
- Loop: the last 5 s over the first 5 s. Playback continues from 0:05.
- Seek: 80 ms from old to new. A seek during a loop join hurries the join instead of waiting 5 s.
- Another sound: the old one fades out, then the new one fades in.

## Failure
| What | What happens |
|---|---|
| Sound server restarts | Retry after 0.25, 0.5, 1, 2, 4, then every 5 s. Step back over unheard audio, fade in |
| Sound server freezes | Noticed within 4 s, then as above. Quit still works (6 s limit) |
| Device unplugged | PipeWire moves the stream to the next device |
| Every device unplugged | Keep the place, say so in the window, resume when one returns |
| Window crashes or freezes | Sound carries on. A crashed window restarts |
| Window can't be seen | After 1 s without frame callbacks it stops drawing and analysis stops |
| File unreadable | Stop, say why. Play tries again |
| Audio thread crash | Logged with file and line. Main saves and exits with an error |
| kill -9, power cut | At most 30 s of position is lost. The next start logs that the last run died |
