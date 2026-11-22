//! The mixing engine: where we are in the sound, the loop, seek crossfades, fade and volume.
//! Pure: it pulls frames from two `Source`s and fills a buffer. It knows nothing about speakers,
//! threads or clocks, so every rule here is tested with fake sources.
//!
//! One mechanism does both joins. A transition plays the current source and the incoming one
//! together with equal-power gains, then swaps them:
//!   loop: the last 5 s over the first 5 s, then carry on from 0:05.
//!   seek: 80 ms from the old place over the new place.

use std::f32::consts::FRAC_PI_2;

use super::level::{self, Fade, Glide};
use crate::error::{Error, Result};
use crate::timecode::RATE;

/// Loop crossfade length (5 s). Shorter sounds use a third of their length.
pub const LOOP_FRAMES: u64 = 5 * RATE;
/// Seek crossfade length (80 ms).
pub const SEEK_FRAMES: u64 = 3_840;
/// Most frames mixed in one go. Scratch buffers are this size, so rendering never allocates.
const BLOCK: usize = 4_096;

/// Something that yields interleaved stereo frames at 48 kHz.
pub trait Source {
    /// Length in frames.
    fn total(&self) -> u64;
    /// Moves to exactly `frame`.
    fn seek(&mut self, frame: u64) -> Result<()>;
    /// Fills all of `out`, or fails.
    fn read(&mut self, out: &mut [f32]) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Join {
    Loop,
    Seek,
}

#[derive(Debug, Clone, Copy)]
struct Transition {
    join: Join,
    done: u64,
    len: u64,
}

/// Mixes two views of one sound into the stream that is heard.
pub struct Engine<S> {
    current: S,
    incoming: S,
    total: u64,
    loop_len: u64,
    position: u64,
    incoming_position: u64,
    /// Continuous frames since the last jump: how far a reconnect may safely rewind.
    since_jump: u64,
    transition: Option<Transition>,
    pending_seek: Option<u64>,
    needs_seek: bool,
    fade: Fade,
    volume: Glide,
    current_buf: Vec<f32>,
    incoming_buf: Vec<f32>,
}

impl<S: Source> Engine<S> {
    /// Starts paused at `start` (frames, clamped). `percent` is the 0-100 volume.
    /// Fails for sounds shorter than a second or sources that can't seek.
    pub fn new(current: S, incoming: S, start: u64, percent: u8) -> Result<Self> {
        let total = current.total();
        if total < RATE {
            return Err(Error::Track("it is shorter than one second".into()));
        }
        let mut engine = Self {
            loop_len: LOOP_FRAMES.min(total / 3),
            current,
            incoming,
            total,
            position: 0,
            incoming_position: 0,
            since_jump: 0,
            transition: None,
            pending_seek: None,
            needs_seek: false,
            fade: Fade::default(),
            volume: Glide::new(level::gain_for(percent)),
            current_buf: vec![0.0; BLOCK * 2],
            incoming_buf: vec![0.0; BLOCK * 2],
        };
        engine.jump(start)?;
        Ok(engine)
    }

    /// Where the listener is, in frames. A seek target shows immediately.
    pub fn position(&self) -> u64 {
        if let Some(target) = self.pending_seek {
            return target;
        }
        match self.transition {
            Some(t) if t.join == Join::Seek || t.done * 2 >= t.len => self.incoming_position,
            _ => self.position,
        }
    }

    /// Length in frames.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// True while playing, including while the fade in is running.
    pub fn is_playing(&self) -> bool {
        self.fade.up
    }

    /// True once paused and fully faded out.
    pub fn is_silent(&self) -> bool {
        self.fade.is_silent()
    }

    /// Fades in or out. Reversing mid-fade is fine.
    pub fn set_playing(&mut self, playing: bool) {
        self.fade.up = playing;
    }

    /// Glides to a new 0-100 volume.
    pub fn set_volume(&mut self, percent: u8) {
        self.volume.set(level::gain_for(percent));
        if self.fade.is_at_zero() {
            self.volume.settle();
        }
    }

    /// Moves by `delta` frames from where the listener is. Repeated presses add up.
    pub fn seek_by(&mut self, delta: i64) -> Result<()> {
        let target = self.position().saturating_add_signed(delta).min(self.last_start());
        if self.fade.is_at_zero() {
            return self.jump(target);
        }
        self.pending_seek = Some(target);
        // Don't make a seek wait out a 5 s loop join: finish it in the time a seek takes.
        if let Some(t) = &mut self.transition
            && t.len - t.done > SEEK_FRAMES
        {
            t.done = t.done * SEEK_FRAMES / t.len;
            t.len = SEEK_FRAMES;
        }
        Ok(())
    }

    /// Moves to exactly `frame`, clamped, the same way `seek_by` does.
    pub fn seek_to(&mut self, frame: u64) -> Result<()> {
        self.seek_by((frame.min(self.total) as i64).saturating_sub(self.position() as i64))
    }

    /// After the output vanished: steps back over the `unheard` frames and fades in again.
    /// Inside a loop join it steps both sides back and keeps the join's phase.
    pub fn recover(&mut self, unheard: u64) -> Result<()> {
        self.fade.cut();
        self.volume.settle();
        if let Some(target) = self.pending_seek {
            return self.jump(target);
        }
        let back = match self.transition {
            // What was cut off is the seek's crossfade, and the seek still stands.
            Some(t) if t.join == Join::Seek => return self.jump(self.incoming_position),
            Some(t) if unheard <= t.done => return self.rewind_join(t, unheard),
            _ => unheard.min(self.since_jump).min(self.position),
        };
        // Not `jump`: its clamp is for manual seeks, and would skip back from just before the join.
        let target = (self.position - back).min(self.total - self.loop_len);
        self.place(target)
    }

    fn rewind_join(&mut self, t: Transition, back: u64) -> Result<()> {
        self.position -= back;
        self.incoming_position -= back;
        self.since_jump = 0;
        self.transition = Some(Transition { done: t.done - back, ..t });
        self.current.seek(self.position)?;
        self.incoming.seek(self.incoming_position)
    }

    /// Stops at once after a read error. The next play seeks back to where it was.
    pub fn halt(&mut self) {
        self.position = self.position();
        self.fade.up = false;
        self.fade.cut();
        self.transition = None;
        self.pending_seek = None;
        self.needs_seek = true;
    }

    /// Fills `out` with interleaved stereo. Once silent it writes zeros and stops advancing.
    /// On error `out` is zeroed from that point, so it is always safe to send.
    pub fn render(&mut self, out: &mut [f32]) -> Result<()> {
        self.render_with_tap(out, |_, _| {})
    }

    /// Like `render`, and also hands `tap` the sound before fade and volume, for the ring.
    /// `tap` gets the block's frame offset inside `out`.
    pub fn render_with_tap(&mut self, out: &mut [f32], mut tap: impl FnMut(usize, &[f32])) -> Result<()> {
        out.fill(0.0);
        let frames = out.len() / 2;
        let mut done = 0;
        while done < frames {
            if self.fade.is_silent() {
                if let Some(target) = self.pending_seek {
                    self.jump(target)?;
                }
                break;
            }
            if self.needs_seek {
                self.jump(self.position)?;
            }
            if self.transition.is_none() {
                self.begin_join()?;
            }
            let count = self.block_len(frames - done);
            self.mix(&mut out[done * 2..(done + count) * 2])?;
            tap(done, &self.current_buf[..count * 2]);
            done += count;
        }
        Ok(())
    }

    /// The latest start that leaves room for a seek join before the loop join begins.
    fn last_start(&self) -> u64 {
        self.total - self.loop_len - SEEK_FRAMES
    }

    fn jump(&mut self, frame: u64) -> Result<()> {
        self.place(frame.min(self.last_start()))
    }

    fn place(&mut self, target: u64) -> Result<()> {
        self.current.seek(target)?;
        self.position = target;
        self.since_jump = 0;
        self.transition = None;
        self.pending_seek = None;
        self.needs_seek = false;
        Ok(())
    }

    fn begin_join(&mut self) -> Result<()> {
        let loop_start = self.total - self.loop_len;
        let (join, from, done, len) = if let Some(target) = self.pending_seek {
            (Join::Seek, target, 0, SEEK_FRAMES)
        } else if self.position >= loop_start {
            let into = (self.position - loop_start).min(self.loop_len - 1);
            (Join::Loop, into, into, self.loop_len)
        } else {
            return Ok(());
        };
        self.incoming.seek(from)?;
        self.incoming_position = from;
        self.pending_seek = None;
        self.transition = Some(Transition { join, done, len });
        Ok(())
    }

    /// Frames until the next boundary: a join starting or ending, or silence.
    fn block_len(&self, wanted: usize) -> usize {
        let mut count = wanted.min(BLOCK) as u64;
        count = count.min(match self.transition {
            Some(t) => t.len - t.done,
            None => self.total - self.loop_len - self.position,
        });
        if let Some(left) = self.fade.frames_to_silence() {
            count = count.min(u64::from(left));
        }
        count as usize
    }

    fn mix(&mut self, out: &mut [f32]) -> Result<()> {
        let samples = out.len();
        let frames = (samples / 2) as u64;
        self.current.read(&mut self.current_buf[..samples])?;
        if let Some(t) = self.transition {
            self.incoming.read(&mut self.incoming_buf[..samples])?;
            let (old, _) = self.current_buf[..samples].as_chunks_mut::<2>();
            let (new, _) = self.incoming_buf[..samples].as_chunks::<2>();
            for (index, (old, new)) in old.iter_mut().zip(new).enumerate() {
                let angle = (t.done + index as u64) as f32 / t.len as f32 * FRAC_PI_2;
                let (rise, fall) = angle.sin_cos();
                old[0] = old[0] * fall + new[0] * rise;
                old[1] = old[1] * fall + new[1] * rise;
            }
        }
        let input = &self.current_buf[..samples];
        match self.volume.settled() {
            // Almost all of the time neither gain is moving, so skip the per-frame ramps.
            Some(gain) if self.fade.is_full() => {
                for (out, &sample) in out.iter_mut().zip(input) {
                    *out = level::guard(sample * gain);
                }
            }
            _ => {
                for (frame, input) in out.as_chunks_mut::<2>().0.iter_mut().zip(input.as_chunks::<2>().0) {
                    let gain = self.fade.step() * self.volume.step();
                    *frame = [level::guard(input[0] * gain), level::guard(input[1] * gain)];
                }
            }
        }
        self.advance(frames);
        Ok(())
    }

    fn advance(&mut self, frames: u64) {
        self.position += frames;
        self.since_jump += frames;
        let Some(t) = &mut self.transition else { return };
        t.done += frames;
        self.incoming_position += frames;
        if t.done == t.len {
            self.since_jump = t.len;
            std::mem::swap(&mut self.current, &mut self.incoming);
            self.position = self.incoming_position;
            self.transition = None;
        }
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
