//! What the engine promises, proved with fake sources that need no files or sound hardware.

use super::*;
use crate::audio::FADE_FRAMES;

/// A fake sound whose sample at frame n is `f(n)`.
struct Fake {
    at: u64,
    total: u64,
    f: fn(u64) -> f32,
    fail_at: Option<u64>,
}

impl Source for Fake {
    fn total(&self) -> u64 {
        self.total
    }
    fn seek(&mut self, frame: u64) -> Result<()> {
        assert!(frame <= self.total, "seek past the end");
        self.at = frame;
        Ok(())
    }
    fn read(&mut self, out: &mut [f32]) -> Result<()> {
        for frame in out.as_chunks_mut::<2>().0 {
            if self.fail_at == Some(self.at) {
                return Err(Error::Track("fake read error".into()));
            }
            assert!(self.at < self.total, "read past the end");
            frame.fill((self.f)(self.at));
            self.at += 1;
        }
        Ok(())
    }
}

const TOTAL: u64 = 20 * RATE;
const LOOP_AT: u64 = TOTAL - LOOP_FRAMES;

fn ramp(n: u64) -> f32 {
    n as f32 / TOTAL as f32 * 0.6
}

/// A smooth 100 Hz wave: no two neighbours differ by more than 0.0066.
fn wave(n: u64) -> f32 {
    (n as f32 % 480.0 / 480.0 * std::f32::consts::TAU).sin() * 0.5
}

fn engine(f: fn(u64) -> f32, start: u64) -> Engine<Fake> {
    let source = || Fake { at: 0, total: TOTAL, f, fail_at: None };
    Engine::new(source(), source(), start, 100).unwrap()
}

fn render(engine: &mut Engine<Fake>, frames: u64) -> Vec<f32> {
    let mut out = vec![0.0; frames as usize * 2];
    engine.render(&mut out).unwrap();
    out
}

fn largest_step(samples: &[f32]) -> f32 {
    samples
        .as_chunks::<2>()
        .0
        .iter()
        .map(|f| f[0])
        .collect::<Vec<_>>()
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn starts_paused_and_silent_without_moving() {
    let mut e = engine(ramp, 1234);
    assert!(render(&mut e, 9000).iter().all(|&s| s == 0.0));
    assert_eq!(e.position(), 1234);
}

#[test]
fn plays_the_exact_samples_at_full_volume() {
    let mut e = engine(ramp, 100_000);
    e.set_playing(true);
    render(&mut e, FADE_FRAMES.into());
    let out = render(&mut e, 10_000);
    for (n, frame) in out.as_chunks::<2>().0.iter().enumerate() {
        assert_eq!(frame[0], ramp(114_400 + n as u64));
    }
}

#[test]
fn pause_fades_then_stops_on_an_exact_frame() {
    let mut e = engine(ramp, 100_000);
    e.set_playing(true);
    render(&mut e, 20_000);
    e.set_playing(false);
    let out = render(&mut e, 30_000);
    assert_eq!(e.position(), 120_000 + u64::from(FADE_FRAMES));
    assert!(e.is_silent());
    assert!(out[FADE_FRAMES as usize * 2..].iter().all(|&s| s == 0.0));
}

#[test]
fn seek_lands_on_the_exact_frame() {
    let mut e = engine(ramp, 0);
    e.set_playing(true);
    render(&mut e, 20_000);
    e.seek_by(200_000).unwrap();
    assert_eq!(e.position(), 220_000, "the target shows at once");
    render(&mut e, SEEK_FRAMES);
    let out = render(&mut e, 2);
    assert_eq!(out[0], ramp(220_000 + SEEK_FRAMES));
}

#[test]
fn repeated_seeks_add_up() {
    let mut e = engine(ramp, 0);
    e.set_playing(true);
    render(&mut e, 20_000);
    for _ in 0..3 {
        e.seek_by(50_000).unwrap();
    }
    assert_eq!(e.position(), 170_000);
    render(&mut e, 100);
    e.seek_by(50_000).unwrap();
    assert_eq!(e.position(), 220_100);
}

#[test]
fn seek_while_paused_moves_at_once() {
    let mut e = engine(ramp, 50_000);
    e.seek_by(1000).unwrap();
    assert_eq!(e.position(), 51_000);
    e.set_playing(true);
    render(&mut e, FADE_FRAMES.into());
    assert_eq!(render(&mut e, 1)[0], ramp(51_000 + u64::from(FADE_FRAMES)));
}

#[test]
fn seek_stops_at_both_ends() {
    let mut e = engine(ramp, 1000);
    e.seek_by(i64::MIN).unwrap();
    assert_eq!(e.position(), 0);
    e.seek_by(i64::MAX).unwrap();
    assert_eq!(e.position(), LOOP_AT - SEEK_FRAMES);
}

#[test]
fn loop_continues_from_five_seconds_in() {
    let mut e = engine(ramp, LOOP_AT - 20_000);
    e.set_playing(true);
    render(&mut e, 20_000);
    render(&mut e, LOOP_FRAMES);
    assert_eq!(e.position(), LOOP_FRAMES);
    assert_eq!(render(&mut e, 1)[0], ramp(LOOP_FRAMES));
}

#[test]
fn loop_repeats_for_many_rounds() {
    let mut e = engine(ramp, 0);
    e.set_playing(true);
    render(&mut e, LOOP_AT + LOOP_FRAMES);
    for _ in 0..5 {
        assert_eq!(e.position(), LOOP_FRAMES);
        render(&mut e, LOOP_AT);
    }
}

#[test]
fn loop_join_has_no_click() {
    let mut e = engine(wave, LOOP_AT - 20_000);
    e.set_playing(true);
    let out = render(&mut e, 20_000 + LOOP_FRAMES + 20_000);
    assert!(largest_step(&out) < 0.012, "step {}", largest_step(&out));
}

#[test]
fn loop_join_keeps_the_loudness_of_unrelated_sound() {
    // Fractional bits of a hash: noise, unrelated between the two ends.
    fn noise(n: u64) -> f32 {
        ((n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ n >> 3).wrapping_mul(0xBF58_476D_1CE4_E5B9) >> 40) as f32
            / 16_777_216.0
            * 0.4
            - 0.2
    }
    let mut e = engine(noise, LOOP_AT - 24_000);
    e.set_playing(true);
    render(&mut e, 24_000);
    let out = render(&mut e, LOOP_FRAMES);
    let expected = 0.2 / 3f32.sqrt();
    for second in out.as_chunks::<{ RATE as usize * 2 }>().0 {
        let rms = (second.iter().map(|s| s * s).sum::<f32>() / second.len() as f32).sqrt();
        assert!((20.0 * (rms / expected).log10()).abs() < 0.5, "rms {rms}");
    }
}

#[test]
fn seek_play_pause_and_volume_never_click() {
    let mut e = engine(wave, 1000);
    let mut out = Vec::new();
    e.set_playing(true);
    out.extend(render(&mut e, 20_000));
    e.seek_by(48_100).unwrap();
    out.extend(render(&mut e, 700));
    e.seek_by(-9_777).unwrap();
    out.extend(render(&mut e, 12_000));
    e.set_volume(10);
    out.extend(render(&mut e, 12_000));
    e.set_playing(false);
    out.extend(render(&mut e, 6000));
    e.set_playing(true);
    out.extend(render(&mut e, 12_000));
    e.set_playing(false);
    out.extend(render(&mut e, 16_000));
    assert!(largest_step(&out) < 0.016, "step {}", largest_step(&out));
}

#[test]
fn seek_during_a_loop_join_is_quick_and_clean() {
    let mut e = engine(wave, LOOP_AT - 20_000);
    e.set_playing(true);
    let mut out = render(&mut e, 120_000);
    // Backwards, so the target is clear of the loop join.
    e.seek_by(-300_000).unwrap();
    let target = e.position();
    out.extend(render(&mut e, SEEK_FRAMES * 2));
    // The join had under 80 ms left once hurried, then the seek took 80 ms.
    let landed = target + SEEK_FRAMES..=target + SEEK_FRAMES * 2;
    assert!(landed.contains(&e.position()), "waited out the loop join");
    assert!(largest_step(&out) < 0.016, "step {}", largest_step(&out));
}

#[test]
fn gain_never_clips_even_when_both_ends_are_at_full_scale() {
    let mut e = engine(|_| 1.0, LOOP_AT - 15_000);
    e.set_playing(true);
    assert!(render(&mut e, LOOP_FRAMES + 30_000).iter().all(|s| s.abs() <= 1.0));
}

#[test]
fn recovering_steps_back_over_unheard_audio_and_fades_in() {
    let mut e = engine(ramp, 100_000);
    e.set_playing(true);
    render(&mut e, 50_000);
    e.recover(14_400).unwrap();
    assert_eq!(e.position(), 135_600);
    assert!(e.is_playing());
    assert!(render(&mut e, 10)[0] < 0.000_01, "must fade in again");
}

#[test]
fn recovering_never_rewinds_past_a_jump() {
    let mut e = engine(ramp, 100_000);
    e.set_playing(true);
    render(&mut e, 3000);
    e.recover(14_400).unwrap();
    assert_eq!(e.position(), 100_000);
}

#[test]
fn a_read_error_stops_silently_and_play_retries() {
    let mut e = engine(ramp, 1000);
    e.current.fail_at = Some(5000);
    e.set_playing(true);
    assert!(e.render(&mut vec![0.5; 20_000]).is_err());
    e.halt();
    assert!(e.is_silent());
    e.current.fail_at = None;
    e.set_playing(true);
    render(&mut e, 100);
    assert_eq!(e.current.at, e.position(), "decoder and position agree again");
}

#[test]
fn short_sounds_loop_and_tiny_ones_are_refused() {
    let short = |total| {
        let s = || Fake { at: 0, total, f: |_| 0.1, fail_at: None };
        Engine::new(s(), s(), 0, 100)
    };
    assert!(short(RATE - 1).is_err());
    let mut e = short(RATE).unwrap();
    e.set_playing(true);
    e.seek_by(i64::MAX).unwrap();
    e.render(&mut vec![0.0; RATE as usize * 20]).unwrap();
}

#[test]
fn recovering_inside_a_loop_join_keeps_its_phase() {
    let mut e = engine(ramp, LOOP_AT - 20_000);
    e.set_playing(true);
    render(&mut e, 20_000 + 24_000);
    e.recover(14_400).unwrap();
    assert_eq!(e.position(), LOOP_AT + 9_600);
    let join = e.transition.unwrap();
    assert_eq!((join.done, e.current.at, e.incoming.at), (9_600, LOOP_AT + 9_600, 9_600));
    render(&mut e, LOOP_FRAMES - 9_600);
    assert_eq!(e.position(), LOOP_FRAMES);
}

#[test]
fn recovering_across_the_start_of_a_join_does_not_skip_back() {
    let mut e = engine(ramp, LOOP_AT - 20_000);
    e.set_playing(true);
    render(&mut e, 20_000 + 5_000);
    e.recover(6_000).unwrap();
    assert_eq!(e.position(), LOOP_AT - 1_000, "only the unheard frames");
    render(&mut e, 1_000 + LOOP_FRAMES);
    assert_eq!(e.position(), LOOP_FRAMES);
}
