//! Loudness rules: the volume curve, click-free ramps and the overload guard. Pure maths, no I/O.

/// Play, pause and quit fade length in frames (300 ms).
pub const FADE_FRAMES: u32 = 14_400;
/// A volume change glides over 100 ms so it never crackles.
const GLIDE_FRAMES: u32 = 4_800;
/// Samples below this pass through bit for bit.
const KNEE: f32 = 0.95;

/// Turns a 0-100 slider value into a gain.
///
/// Squared, not cubed: rain averages around -23 dBFS, so cubed would make the default 20%
/// -42 dB and near silent at a moderate system volume. Squared gives -28 dB (docs/decisions.md#volume).
pub fn gain_for(percent: u8) -> f32 {
    let x = f32::from(percent.min(100)) / 100.0;
    x * x
}

/// Squeezes the rare sample above 0.95 smoothly under full scale. Quieter samples are untouched.
pub fn guard(sample: f32) -> f32 {
    let size = sample.abs();
    if size <= KNEE {
        return sample;
    }
    let room = 1.0 - KNEE;
    sample.signum() * (KNEE + room * ((size - KNEE) / room).tanh())
}

/// The play and pause fade. It can reverse half way without a jump.
#[derive(Debug, Default)]
pub struct Fade {
    level: u32,
    /// Where the fade is heading: true for audible.
    pub up: bool,
}

impl Fade {
    /// True when fully faded out and staying there.
    pub fn is_silent(&self) -> bool {
        self.level == 0 && !self.up
    }

    /// True when nothing is audible at this instant, whatever the direction.
    pub fn is_at_zero(&self) -> bool {
        self.level == 0
    }

    /// Frames left until silence, or `None` if not fading out.
    pub fn frames_to_silence(&self) -> Option<u32> {
        (!self.up).then_some(self.level)
    }

    /// True when fully faded in and staying there.
    pub fn is_full(&self) -> bool {
        self.up && self.level == FADE_FRAMES
    }

    /// Cuts to zero at once. Only for when nothing is being heard (lost output, read error).
    pub fn cut(&mut self) {
        self.level = 0;
    }

    /// Advances one frame and returns its gain (an S-curve, so both ends are smooth).
    pub fn step(&mut self) -> f32 {
        self.level = if self.up { (self.level + 1).min(FADE_FRAMES) } else { self.level.saturating_sub(1) };
        if self.level == FADE_FRAMES {
            return 1.0; // Exact, so audio at full level is not altered at all.
        }
        let t = self.level as f32 / FADE_FRAMES as f32;
        t * t * (3.0 - 2.0 * t)
    }
}

/// A gain that glides to each new target.
#[derive(Debug)]
pub struct Glide {
    from: f32,
    to: f32,
    done: u32,
}

impl Glide {
    /// Starts settled at `gain`.
    pub fn new(gain: f32) -> Self {
        Self { from: gain, to: gain, done: GLIDE_FRAMES }
    }

    /// The gain at this instant.
    pub fn now(&self) -> f32 {
        if self.done >= GLIDE_FRAMES {
            return self.to;
        }
        self.from + (self.to - self.from) * (self.done as f32 / GLIDE_FRAMES as f32)
    }

    /// Heads for `gain` from wherever it is now.
    pub fn set(&mut self, gain: f32) {
        self.from = self.now();
        self.to = gain;
        self.done = 0;
    }

    /// Jumps to the target. Only while silent.
    pub fn settle(&mut self) {
        self.done = GLIDE_FRAMES;
    }

    /// The gain, if it has stopped moving.
    pub fn settled(&self) -> Option<f32> {
        (self.done >= GLIDE_FRAMES).then_some(self.to)
    }

    /// Advances one frame and returns its gain.
    pub fn step(&mut self) -> f32 {
        self.done = (self.done + 1).min(GLIDE_FRAMES);
        self.now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_is_perceptual_and_capped_at_100() {
        assert_eq!(gain_for(0), 0.0);
        assert_eq!(gain_for(100), 1.0);
        assert_eq!(gain_for(255), 1.0);
        assert!((20.0 * gain_for(20).log10() + 27.96).abs() < 0.01);
    }

    #[test]
    fn guard_leaves_normal_audio_untouched_and_never_clips() {
        for step in -950..=950 {
            let x = step as f32 / 1000.0;
            assert_eq!(guard(x).to_bits(), x.to_bits());
        }
        let mut last = KNEE;
        for step in 951..4000 {
            let y = guard(step as f32 / 1000.0);
            assert!(y >= last && y <= 1.0);
            last = y;
        }
        assert!(guard(-9.0) >= -1.0);
    }

    #[test]
    fn fade_takes_300_ms_and_can_reverse_without_a_jump() {
        let mut fade = Fade { up: true, ..Fade::default() };
        let mut last = 0.0;
        for frame in 0..FADE_FRAMES * 2 {
            fade.up = !(5000..9000).contains(&frame);
            let now = fade.step();
            assert!((now - last).abs() < 0.00012, "jump at {frame}");
            last = now;
        }
        assert_eq!(last, 1.0);
        fade.up = false;
        for _ in 0..FADE_FRAMES {
            fade.step();
        }
        assert!(fade.is_silent());
    }

    #[test]
    fn volume_changes_glide_even_when_interrupted() {
        let mut glide = Glide::new(0.0);
        let mut last = 0.0;
        for frame in 0..20_000 {
            if frame % 1000 == 0 {
                glide.set(if frame % 2000 == 0 { 1.0 } else { 0.1 });
            }
            let now = glide.step();
            assert!((now - last).abs() < 0.00022);
            last = now;
        }
    }
}
