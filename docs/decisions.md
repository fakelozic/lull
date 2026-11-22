# Decisions

Newest first.

## Two vendored crates with small patches
From attacking the new D-Bus surface.
- **Found:** one 16 MB message took Lull to 38.6 MB for good (128 MB is allowed), 800 medium ones to 110 MB, and
  30,000 small ones to 144 MB.
- **Decision:** `vendor/zbus` (message limit, bounded dispatch) and `vendor/ksni` (bounded token queue).
- **Rejected:** limits in our own code. The tray library and Slint open their own connections.
- **Costs:** updating those two crates means re-applying a few lines (each LULL-PATCH.md).

## Opens fast
Requirement: it opens fast.
- **Measured:** about 50 ms to a complete first frame, warm. 67-81 ms with file caches dropped, 42 ms from a tray
  click, 12 ms for a second launch to exit. Step 2 was also about 50 ms: the tray and bus added nothing.
- **Guard:** `scripts/startup.sh` fails over 100 ms warm or 150 ms cold.
- **Rejected:** a cut-down font set (7-11 ms of work saved, no reliable gain in the first frame, risks missing
  scripts) and a hidden warm window (14 MB all the time).

## The tray
From the plan.
- **Decision:** closing hides, whether playing or paused, and says so once. Without a tray, closing quits.
- **Decision:** a click hides a window that has focus (or lost it in the last 400 ms, as some trays take focus on
  mouse-down) and raises one that doesn't. Clicks within 300 ms count as one.
- **Decision:** SIGHUP doesn't quit while there is a tray. SIGTERM (logout) and Ctrl+C do.

## Memory is bounded at every input
From the stress test and a code review. Requirement: always under 50 MB.
- **Found:** 30 MB of cover art took Lull to 140 MB, 50,000 joined recordings to 67 MB.
- **Decision:** one open file per sound (`src/media.rs`). A tag block over 64 KB is shown to the decoder as an empty
  one. Opening may read 1 MB and each decode 3 MB. At most 64 joined recordings and 8 channels.
- **Also bounded:** every mailbox, the folder listing (200 kept), our own files (read limits), the display scale
  override (3x).
- **Costs:** a file with over 1 MB of junk after its last page, or over 64 joined recordings, is refused.

## Sounds must really be inside ~/Music
From the review.
- **Decision:** a sound Lull finds or lists is checked after opening, by where the open file really is.
- **Costs:** a link in ~/Music to a file on another drive shows "can't play". A path on the command line may be anywhere.

## A window failure never stops sound that is playing
From the review.
- **Decision:** if the window can't come back while playing, Lull plays on and says how to stop it in the log.
  If nothing is playing it exits, because nobody could start it.
- **Not done:** a heartbeat for a window that freezes later. KWin already offers to close those, and a closed or
  killed one restarts. A window stuck while starting is replaced after 15 s.

## Logs
Requirement: 7 days of logs, so a failure can be fixed.
- **Decision:** daily JSON-line files under the state folder, 7 days, 2 MB a day, home folder hidden, local only.
- **Why:** a crash must be explainable afterwards. The journal only has them when started as a service.

## The window is a second process on a socket pair
- **Decision:** main starts `lull --window` with one end of a socket pair as stdin.
- **Why:** a UI crash can't stop the sound and the RAM returns on close. No port, path or bus to secure.
- **Rejected:** D-Bus for this (30 messages a second through the bus). It still comes in step 3 for media keys.

## 30 fps, paced to fixed deadlines
Requirement: at least 30 fps.
- **Decision:** the ring draws at 30 fps, like the prototype. At 60 Hz that is two refreshes a frame.
- **Why:** smoothness matters more than the CPU it costs. It replaces a 20 fps cut made to save CPU.
- **Pacing:** each frame is scheduled against a fixed deadline, because Slint's repeating timer drifts.

## Built for x86-64-v3, hot crates at opt-level 3
- **Why:** the pixel, FFT and blit loops. Everything else stays size-optimised.
- **Limit:** remove `.cargo/config.toml` before shipping binaries for older machines.

## CPU target missed
Measured on headless Mutter and on a real screen.
- 7.6-7.9% of a core with the window playing at 30 fps, against a planned 5% (sound 2.3, window 5.3).
  1.7% when the window can't be seen, 0% paused.
- 20 fps got it to 6.2%. 30 fps is the requirement, so smoothness wins over this target.
- The check stays failing in `scripts/window-e2e.sh`. The target was not quietly raised.

## Seeking
From measurements.
- **Decision:** every seek lands 500 ms early, decodes and discards up to the target.
- **Why:** `op_pcm_seek` (opusfile 0.12) reports the right position but returned wrong audio on 647 and 757 of 1,500
  short forward seeks (test file, rain file). Full seeks differed from straight playback by 3-5% until the decoder
  settled. With 500 ms, 1,000 seeks across the 8 h file differ by at most 1e-7 and take at most 1.9 ms.
- **Rejected:** 80 ms (still differs), 1 s (no gain at twice the cost).
- **Limit:** a synthetic pure-tone file at 32 kbps still differed by up to 0.4% of full scale. Rain-like audio is exact.

## Volume
- **Decision:** gain = (slider)². The default stays 20% as planned, which is -28 dB.
- **Why:** rain recordings average around -23 dBFS and system volume is often well below full. The usual cubed curve would put
  20% at -42 dB, close to silent.
- **Rejected:** cubed (too quiet at the default), raising the default (the plan says 20%).

## No device
Found by the end-to-end test.
- **Decision:** PipeWire's `auto_null` dummy sink counts as "no device": hold the place and retry.
- **Why:** with every device unplugged, playback carried on into nothing and would lose the place overnight.

## Plain libpulse loop instead of pa_simple
- **Decision:** libpulse's main loop on the audio thread, every wait with a timeout.
- **Why:** pa_simple can block forever if the server freezes, hanging pause and quit, and can't report underruns or
  which device the stream is on.

## No stream while paused
- **Decision:** close the stream after the fade out and reopen on play.
- **Why:** 0% CPU while paused, the DAC can suspend, and paused time can't cause underruns.

## Installing
- A copy in `~/.local/bin`, not a link into `target/`: a build in progress or a `cargo clean` can't break the installed player.
- The menu entry uses the full path, because `~/.local/bin` isn't always on Plasma's PATH (terminals often add it).
- `StartupNotify=true`, or the launcher passes no activation token and Wayland won't focus the window.
- Replacing the program while it runs is safe: the host starts windows from its own image (`host.rs`, `this_program`).
- No autostart: no daemon at login.
