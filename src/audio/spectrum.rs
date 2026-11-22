//! What the ring listens to: 24 log bands, 30 times a second, each measured against its usual level.
//! A port of the analysis in the prototype (its build script used numpy), held by parity tests.
//! Pure, and it never allocates after `new`.
//!
//! The one change from the prototype: "usual" there was the median of the whole 20 s clip. Live,
//! it is the median of the last 20 s, sampled twice a second.

use std::f64::consts::TAU;

use crate::timecode::RATE;
use crate::visual::{BANDS, Levels};

/// Samples in one transform (43 ms).
const WINDOW: usize = 2_048;
/// The input is real, so a complex transform of half the size does the same job for half the CPU.
const HALF: usize = WINDOW / 2;
/// Frames between two results (30 a second).
pub const HOP: usize = RATE as usize / 30;
/// "Usual" looks back 40 samples, 15 results apart: 20 s.
const HISTORY: usize = 40;
const HISTORY_EVERY: usize = 15;
const BASS_BANDS: usize = 7;

/// Turns a stream of stereo frames into `Levels`.
pub struct Spectrum {
    recent: Vec<f32>,
    at: usize,
    since: usize,
    hann: Vec<f32>,
    windowed: Vec<f32>,
    twiddles: Vec<(f32, f32)>,
    /// The transform's own factors, stage after stage, so each stage reads a plain run of memory.
    stage_cos: Vec<f32>,
    stage_sin: Vec<f32>,
    edges: [usize; BANDS + 1],
    real: Vec<f32>,
    imaginary: Vec<f32>,
    history: [[f32; HISTORY]; BANDS],
    kept: usize,
    results: usize,
    usual: [f32; BANDS],
    smooth: Levels,
}

impl Default for Spectrum {
    fn default() -> Self {
        Self::new()
    }
}

impl Spectrum {
    /// Starts with no idea of what is usual.
    pub fn new() -> Self {
        let last = (WINDOW - 1) as f64;
        Self {
            recent: vec![0.0; WINDOW],
            at: 0,
            since: 0,
            windowed: vec![0.0; WINDOW],
            hann: (0..WINDOW).map(|n| (0.5 - 0.5 * (TAU * n as f64 / last).cos()) as f32).collect(),
            twiddles: (0..WINDOW / 2)
                .map(|n| {
                    let angle = -TAU * n as f64 / WINDOW as f64;
                    (angle.cos() as f32, angle.sin() as f32)
                })
                .collect(),
            stage_cos: stage_factors(f64::cos),
            stage_sin: stage_factors(f64::sin),
            edges: std::array::from_fn(|n| {
                if n == BANDS { 700 } else { (2.0 * 350f64.powf(n as f64 / BANDS as f64)) as usize }
            }),
            real: vec![0.0; HALF],
            imaginary: vec![0.0; HALF],
            history: [[0.0; HISTORY]; BANDS],
            kept: 0,
            results: 0,
            usual: [0.0; BANDS],
            smooth: Levels::default(),
        }
    }

    /// Takes interleaved stereo. Calls `ready(frame, levels)` for each result, where `frame` is how
    /// far into this buffer the result ends, so the caller can time it to when it is heard.
    pub fn push(&mut self, stereo: &[f32], mut ready: impl FnMut(usize, Levels)) {
        for (index, frame) in stereo.as_chunks::<2>().0.iter().enumerate() {
            self.recent[self.at] = (frame[0] + frame[1]) / 2.0;
            self.at = (self.at + 1) % WINDOW;
            self.since += 1;
            if self.since == HOP {
                self.since = 0;
                let decibels = self.decibels();
                self.learn(&decibels);
                ready(index + 1, self.react(&decibels));
            }
        }
    }

    fn decibels(&mut self) -> [f32; BANDS] {
        // Even samples go in the real part, odd ones in the imaginary part.
        // The ring buffer's oldest sample is at `at`. Copy it out in order, then split even and odd.
        let (newest, oldest) = self.recent.split_at(self.at);
        let ordered = oldest.iter().chain(newest);
        self.windowed.iter_mut().zip(ordered).zip(&self.hann).for_each(|((out, sample), hann)| *out = sample * hann);
        let pairs = self.windowed.as_chunks::<2>().0.iter();
        for ((real, imaginary), pair) in self.real.iter_mut().zip(&mut self.imaginary).zip(pairs) {
            (*real, *imaginary) = (pair[0], pair[1]);
        }
        self.transform();
        std::array::from_fn(|band| {
            let from = self.edges[band];
            let to = self.edges[band + 1].max(from + 1);
            let sum: f32 = (from..to).map(|bin| self.magnitude(bin)).sum();
            20.0 * (sum / (to - from) as f32 + 1e-6).log10()
        })
    }

    /// Bin `bin` (1 to HALF - 1) of the full transform, untangled from the half-size one.
    fn magnitude(&self, bin: usize) -> f32 {
        let (a, b) = (self.real[bin], self.imaginary[bin]);
        let (c, d) = (self.real[HALF - bin], -self.imaginary[HALF - bin]);
        let (even_real, even_imaginary) = ((a + c) / 2.0, (b + d) / 2.0);
        let (odd_real, odd_imaginary) = ((b - d) / 2.0, (c - a) / 2.0);
        let (cos, sin) = self.twiddles[bin];
        let real = even_real + odd_real * cos - odd_imaginary * sin;
        let imaginary = even_imaginary + odd_real * sin + odd_imaginary * cos;
        // Not hypot: it guards against overflow we can't have, at several times the cost.
        (real * real + imaginary * imaginary).sqrt()
    }

    /// In-place radix-2 FFT.
    fn transform(&mut self) {
        let shift = usize::BITS - HALF.trailing_zeros();
        for index in 0..HALF {
            let other = index.reverse_bits() >> shift;
            if other > index {
                self.real.swap(index, other);
                self.imaginary.swap(index, other);
            }
        }
        // The first stage's only factor is 1, and it is half of all the blocks.
        for pairs in [&mut self.real, &mut self.imaginary] {
            for [a, b] in pairs.as_chunks_mut::<2>().0 {
                (*a, *b) = (*a + *b, *a - *b);
            }
        }
        // The second's are 1 and -i.
        let quads = self.real.as_chunks_mut::<4>().0.iter_mut();
        for (real, imaginary) in quads.zip(self.imaginary.as_chunks_mut::<4>().0) {
            let ([a, b, c, d], [e, f, g, h]) = (*real, *imaginary);
            *real = [a + c, b + h, a - c, b - h];
            *imaginary = [e + g, f - d, e - g, f + d];
        }
        let mut half = 4;
        while half < HALF {
            let factors = (&self.stage_cos[half - 1..2 * half - 1], &self.stage_sin[half - 1..2 * half - 1]);
            let reals = self.real.chunks_exact_mut(half * 2);
            for (real, imaginary) in reals.zip(self.imaginary.chunks_exact_mut(half * 2)) {
                let (top_real, bottom_real) = real.split_at_mut(half);
                let (top_imaginary, bottom_imaginary) = imaginary.split_at_mut(half);
                butterflies((top_real, top_imaginary), (bottom_real, bottom_imaginary), factors);
            }
            half *= 2;
        }
    }

    fn learn(&mut self, decibels: &[f32; BANDS]) {
        // The first second is remembered at every result, so "usual" is sensible almost at once.
        let due = self.results < HISTORY_EVERY || self.results.is_multiple_of(HISTORY_EVERY);
        self.results += 1;
        if !due {
            return;
        }
        let slot = self.kept % HISTORY;
        self.kept += 1;
        let count = self.kept.min(HISTORY);
        for (band, &value) in decibels.iter().enumerate() {
            self.history[band][slot] = value;
            let mut sorted = self.history[band];
            sorted[..count].sort_unstable_by(f32::total_cmp);
            self.usual[band] = (sorted[(count - 1) / 2] + sorted[count / 2]) / 2.0;
        }
    }

    fn react(&mut self, decibels: &[f32; BANDS]) -> Levels {
        let targets: [f32; BANDS] =
            std::array::from_fn(|band| ((decibels[band] - self.usual[band] + 3.0) / 15.0).clamp(0.0, 1.3));
        for (value, &target) in self.smooth.bands.iter_mut().zip(&targets) {
            *value += (target - *value) * if target > *value { 0.30 } else { 0.07 };
        }
        let low = targets[..BASS_BANDS].iter().sum::<f32>() / BASS_BANDS as f32;
        let bass = &mut self.smooth.bass;
        *bass += (low - *bass) * if low > *bass { 0.16 } else { 0.04 };
        self.smooth
    }
}

/// For each stage of half-size h = 1, 2, 4 ...: part(-pi k / h) for k < h. A stage starts at index h - 1.
fn stage_factors(part: fn(f64) -> f64) -> Vec<f32> {
    (0..HALF.trailing_zeros())
        .flat_map(|stage| {
            let half = 1usize << stage;
            (0..half).map(move |k| part(-std::f64::consts::PI * k as f64 / half as f64) as f32)
        })
        .collect()
}

/// Joins two half transforms. Equal-length slices and no indexing, so it compiles to SIMD.
fn butterflies(
    (top_real, top_imaginary): (&mut [f32], &mut [f32]),
    (bottom_real, bottom_imaginary): (&mut [f32], &mut [f32]),
    (cos, sin): (&[f32], &[f32]),
) {
    let count = cos.len();
    let (top_real, top_imaginary) = (&mut top_real[..count], &mut top_imaginary[..count]);
    let (bottom_real, bottom_imaginary) = (&mut bottom_real[..count], &mut bottom_imaginary[..count]);
    let sin = &sin[..count];
    for k in 0..count {
        let real = bottom_real[k] * cos[k] - bottom_imaginary[k] * sin[k];
        let imaginary = bottom_real[k] * sin[k] + bottom_imaginary[k] * cos[k];
        bottom_real[k] = top_real[k] - real;
        bottom_imaginary[k] = top_imaginary[k] - imaginary;
        top_real[k] += real;
        top_imaginary[k] += imaginary;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_edges_match_the_prototype() {
        let want =
            [2, 2, 3, 4, 5, 6, 8, 11, 14, 17, 22, 29, 37, 47, 60, 77, 99, 126, 161, 206, 263, 336, 429, 548, 700];
        assert_eq!(Spectrum::new().edges, want);
    }

    #[test]
    fn band_levels_match_numpy() {
        let mut spectrum = Spectrum::new();
        for n in 0..WINDOW as i64 {
            let hash = ((n * n * 7919 + n * 31) % 1009) as f64 / 1009.0;
            spectrum.recent[n as usize] = ((hash - 0.5) * 0.3 + (n as f64 * 0.05).sin() * 0.1) as f32;
        }
        let want = [
            7.7220, 7.7220, 9.2189, 14.7119, 9.7262, -1.0460, 1.9880, 10.0185, 26.5849, 19.9090, 5.1993, 6.4024,
            6.9833, 7.7465, 1.0526, 7.4022, 8.1692, 1.6172, 6.1983, 5.6223, 6.8496, 5.9118, 6.8966, 6.2035,
        ];
        for (got, want) in spectrum.decibels().into_iter().zip(want) {
            assert!((got - want).abs() < 0.01, "{got} vs {want}");
        }
    }

    #[test]
    fn rise_and_fall_match_the_prototype() {
        let mut spectrum = Spectrum::new();
        let want = [
            (0.240, 0.128),
            (0.408, 0.235_520),
            (0.5256, 0.325_837),
            (0.607_920, 0.401_703),
            (0.665_544, 0.465_430),
            (0.632_956, 0.454_813),
            (0.602_649, 0.444_621),
            (0.574_464, 0.434_836),
            (0.548_251, 0.425_442),
        ];
        for (index, (band, bass)) in want.into_iter().enumerate() {
            let got = spectrum.react(&[if index < 5 { 9.0 } else { 0.0 }; BANDS]);
            assert!((got.bands[0] - band).abs() < 1e-4 && (got.bass - bass).abs() < 1e-4, "step {index}");
        }
    }

    #[test]
    fn steady_rain_rests_low_and_thunder_swells_it() {
        let mut spectrum = Spectrum::new();
        let (mut random, mut quiet, mut loud) = (1u32, 0.0f32, 0.0f32);
        for frame in 0..RATE as usize * 24 {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let mut sample = (random >> 8) as f32 / 16_777_216.0 * 0.2 - 0.1;
            let thunder = frame >= RATE as usize * 22;
            if thunder {
                sample += (frame as f32 * TAU as f32 * 60.0 / RATE as f32).sin() * 0.4;
            }
            spectrum.push(&[sample, sample], |_, levels| {
                if thunder {
                    loud = loud.max(levels.bass);
                } else {
                    quiet = levels.bass;
                }
            });
        }
        assert!(quiet < 0.45 && loud > quiet + 0.15, "quiet {quiet}, thunder {loud}");
    }

    #[test]
    fn results_come_30_times_a_second() {
        let mut spectrum = Spectrum::new();
        let mut ends = Vec::new();
        spectrum.push(&vec![0.0; RATE as usize * 2], |frame, _| ends.push(frame));
        assert_eq!(ends.len(), 30);
        assert_eq!(ends[0], HOP);
    }
}
