//! The Ripples ring: Ripples, Spectrum, 15 s, Normal.
//! The numbers come from `paint()` and `tick()` in the HTML prototype.
//!
//! It is drawn on the CPU without a GPU driver, so it avoids per-pixel maths:
//! every shape is a circle, so each frame builds two radial tables (the wobbly inner circle by
//! distance from its edge, the waves by distance from the centre) and one colour table by angle.
//! A pixel is then two table reads and a blend.
//!
//! The waves' rate and the colour cycle passed a flash check. Repeat it before speeding either up.

use std::f32::consts::{FRAC_1_SQRT_2, TAU};
use std::sync::OnceLock;

use super::BANDS;
use super::colour::oklch;

/// Width and height of the square it draws, in pixels.
pub const SIDE: usize = 300;

const GROUND: [f32; 3] = [9.0, 13.0, 10.0];
/// Seconds for one full turn of the hue.
const CYCLE: f32 = 15.0;
const FIRST_HUE: f32 = 2.5;
const LIGHTNESS: f32 = 0.78;
/// What thunder adds to the lightness, at most.
const LIFT: f32 = 0.07;
const CHROMA: f32 = 0.115;
const ACCENT: (f32, f32) = (0.80, 0.11);
const INNER_RADIUS: f32 = 62.0;
const WAVE_START: f32 = 66.0;
/// Pixels per second.
const WAVE_SPEED: f32 = 21.0;
/// Seconds.
const WAVE_LIFE: f32 = 3.6;
/// Points around the inner circle where the wobble is worked out.
const POINTS: usize = 144;
/// Angles in the edge and colour tables. The table sizes are powers of two, so a masked index
/// needs no bounds check in the per-pixel loop, which is four fifths of the frame.
const ANGLES: usize = 512;
/// Table steps per pixel of radius.
const STEPS: f32 = 4.0;
const RADII: usize = 1_024;
/// Table index of "exactly on the inner circle's edge".
const EDGE: i32 = 1_024;
const OFFSETS: usize = 2_048;
const FULL: f32 = 32_768.0;
/// The per-pixel loop blends red, green and blue in one u64, 21 bits each, with 12-bit coverage.
/// A channel's sum is at most 255 x 4096 + 4096, which fits in 21 bits, so they never mix.
const LANE: u32 = 21;
const ONE: u32 = 4_096;
/// One past the furthest radius any pixel has: the square's corner, 212 px.
const FURTHEST: usize = 854;
/// Half-width of the smoothing around the inner circle, in points.
const SMOOTH: usize = 9;
/// The error function is tabled in steps of 1/256 out to 4, where it is 1.
const ERF_STEPS: f32 = 256.0;
const ERF_SIZE: usize = 1_024;
/// Breaks up banding in the dark glow without any noise in motion.
const DITHER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// One moment of sound: each band against its usual level, 0 to 1.3, already smoothed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Levels {
    /// Low to high.
    pub bands: [f32; BANDS],
    /// Bass and thunder.
    pub bass: f32,
}

#[derive(Debug, Clone, Copy)]
struct Wave {
    age: f32,
    force: f32,
}

#[derive(Debug, Clone, Copy)]
struct Pixel {
    radius: u16,
    angle: u16,
}

/// The ring's animation state and drawing tables.
pub struct Ripples {
    levels: Levels,
    waves: Vec<Wave>,
    next_wave: f32,
    time: f32,
    hue: f32,
    amount: f32,
    still: bool,
    pixels: &'static [Pixel],
    weights: [f32; 2 * SMOOTH + 1],
    erf: Box<[f32; ERF_SIZE]>,
    /// Per row, the span inside the circle the glow can reach. The corners are always ground.
    spans: Vec<(usize, usize)>,
    inner: Box<[u16; OFFSETS]>,
    outer: Box<[u16; RADII]>,
    edge: [i32; ANGLES],
    colours: [u64; ANGLES],
}

impl Ripples {
    /// `still` is reduced motion: a plain circle that never changes.
    pub fn new(still: bool) -> Self {
        let middle = SIDE as f32 / 2.0;
        let pixels = pixel_map();
        let spans = (0..SIDE)
            .map(|row| {
                let y = row as f32 + 0.5 - middle;
                let half = (middle * middle - y * y).sqrt();
                (((middle - half).floor() as usize).min(SIDE), ((middle + half).ceil() as usize).min(SIDE))
            })
            .collect();
        Self {
            levels: Levels::default(),
            waves: Vec::with_capacity(8),
            next_wave: 0.0,
            time: 0.0,
            hue: FIRST_HUE,
            amount: 0.0,
            still,
            pixels,
            spans,
            weights: {
                let raw: [f32; 2 * SMOOTH + 1] =
                    std::array::from_fn(|at| (-(at as f32 - SMOOTH as f32).powi(2) / 18.0).exp());
                let total: f32 = raw.iter().sum();
                raw.map(|weight| weight / total)
            },
            erf: Box::new(std::array::from_fn(|at| erf(at as f32 / ERF_STEPS))),
            inner: Box::new([0; OFFSETS]),
            outer: Box::new([0; RADII]),
            edge: [0; ANGLES],
            colours: [0; ANGLES],
        }
    }

    /// Takes the sound that is being heard now.
    pub fn listen(&mut self, levels: Levels) {
        self.levels = levels;
    }

    /// Moves on by `seconds`. Returns false when nothing changed, so there is nothing to draw.
    pub fn advance(&mut self, seconds: f32, playing: bool) -> bool {
        if self.still {
            return false;
        }
        let goal = f32::from(u8::from(playing));
        if !playing && self.amount == 0.0 {
            return false;
        }
        self.amount += (goal - self.amount) * (seconds * 3.0).min(1.0);
        if !playing && self.amount < 0.004 {
            // Settled: a still circle.
            self.amount = 0.0;
            self.waves.clear();
        }
        if playing {
            self.time += seconds;
            self.hue = (self.hue + seconds * TAU / CYCLE) % TAU;
            self.waves.iter_mut().for_each(|wave| wave.age += seconds);
            self.waves.retain(|wave| wave.age < WAVE_LIFE);
            self.next_wave -= seconds;
            if self.next_wave <= 0.0 {
                let bass = self.levels.bass;
                self.waves.push(Wave { age: 0.0, force: (bass * 1.3).min(1.0) });
                self.next_wave = 1.1 - 0.6 * bass.min(1.0);
            }
        }
        true
    }

    /// The accent for the play button and sliders: the ring's hue at a steady lightness.
    pub fn accent(&self) -> [u8; 3] {
        oklch(ACCENT.0, ACCENT.1, self.hue).map(|channel| channel.round() as u8)
    }

    /// Draws into `rgb`, which holds SIDE x SIDE x 3 bytes.
    pub fn paint(&mut self, rgb: &mut [u8]) {
        let bass = self.levels.bass * self.amount;
        let blur = 10.0 + 8.0 * bass.min(1.0);
        let base = INNER_RADIUS + 12.0 * bass;
        self.set_edge(base);
        self.set_colours(bass.min(1.0));
        self.set_inner(base, blur);
        self.set_outer(blur);
        self.blend(rgb);
    }

    fn set_edge(&mut self, base: f32) {
        let bands = &self.levels.bands;
        let mut raw = [0.0; POINTS];
        for (index, value) in raw.iter_mut().enumerate() {
            // Low bands at the top, high at the bottom, mirrored left and right.
            let at = (1.0 - (index as f32 / POINTS as f32 * TAU).cos()) / 2.0 * (BANDS - 1) as f32;
            let low = (at as usize).min(BANDS - 1);
            let high = (low + 1).min(BANDS - 1);
            *value = (bands[low] + (bands[high] - bands[low]) * (at - low as f32)) * self.amount;
        }
        let wrapped: [f32; POINTS + 2 * SMOOTH] = std::array::from_fn(|at| raw[(at + POINTS - SMOOTH) % POINTS]);
        let mut radii = [0.0; POINTS];
        for (index, radius) in radii.iter_mut().enumerate() {
            let near = &wrapped[index..index + 2 * SMOOTH + 1];
            let smooth: f32 = near.iter().zip(&self.weights).map(|(value, weight)| value * weight).sum();
            let sway = 1.5 * self.amount * (3.0 * index as f32 / POINTS as f32 * TAU + self.time * 0.5).sin();
            *radius = base + 25.0 * 0.3 * smooth + sway;
        }
        for (index, edge) in self.edge.iter_mut().enumerate() {
            let at = index as f32 * POINTS as f32 / ANGLES as f32;
            let low = at as usize;
            let radius = radii[low] + (radii[(low + 1) % POINTS] - radii[low]) * (at - low as f32);
            *edge = (radius * STEPS).round() as i32 - EDGE;
        }
    }

    fn set_colours(&mut self, bass: f32) {
        // Thirteen stops blended in sRGB, as the canvas conic gradient does.
        let stops: [[f32; 3]; 13] = std::array::from_fn(|stop| {
            let spread = 0.6 * (stop as f32 / 12.0 * TAU + self.time * 0.25).sin();
            oklch(LIGHTNESS + LIFT * bass, CHROMA, self.hue + spread)
        });
        for (index, colour) in self.colours.iter_mut().enumerate() {
            let at = index as f32 * 12.0 / ANGLES as f32;
            let low = at as usize;
            let next = (low + 1).min(12);
            *colour = packed(std::array::from_fn(|c| {
                (stops[low][c] + (stops[next][c] - stops[low][c]) * (at - low as f32)) as u8
            }));
        }
    }

    fn set_inner(&mut self, base: f32, blur: f32) {
        let circle = Circle { radius: base, width: 3.5, strength: 1.0, blur };
        self.inner.fill(FULL as u16);
        let reach = (circle.reach() * STEPS) as i32 + 1;
        let (from, to) = ((EDGE - reach).max(0) as usize, ((EDGE + reach) as usize).min(OFFSETS));
        for (index, keep) in self.inner[from..to].iter_mut().enumerate() {
            let from_edge = ((from + index) as i32 - EDGE) as f32 / STEPS;
            *keep = (circle.uncovered(from_edge, &self.erf) * FULL) as u16;
        }
    }

    fn set_outer(&mut self, blur: f32) {
        // Waves fade with `amount`, so pausing settles to a plain circle.
        let mut keep = [1.0f32; RADII];
        for wave in &self.waves {
            let radius = WAVE_START + wave.age * WAVE_SPEED;
            let circle = Circle {
                radius,
                width: 1.0 + 2.5 * wave.force,
                strength: (1.0 - wave.age / WAVE_LIFE).max(0.0) * (0.2 + 0.7 * wave.force) * self.amount,
                blur,
            };
            let reach = circle.reach();
            let from = ((radius - reach) * STEPS).max(0.0) as usize;
            let to = (((radius + reach) * STEPS) as usize + 1).min(FURTHEST);
            for (index, keep) in keep[..to].iter_mut().enumerate().skip(from) {
                *keep *= circle.uncovered(index as f32 / STEPS - radius, &self.erf);
            }
        }
        self.outer.iter_mut().zip(keep).for_each(|(out, keep)| *out = (keep * FULL) as u16);
    }

    fn blend(&self, rgb: &mut [u8]) {
        let ground_rgb = GROUND.map(|channel| channel as u8);
        let ground = packed(ground_rgb);
        let (lines, _) = rgb.as_chunks_mut::<{ SIDE * 3 }>();
        let rows = lines.iter_mut().zip(self.pixels.as_chunks::<SIDE>().0).zip(&self.spans);
        for (y, ((row, pixels), &(from, to))) in rows.enumerate() {
            let (row, _) = row.as_chunks_mut::<3>();
            row[..from].fill(ground_rgb);
            row[to..].fill(ground_rgb);
            let noise = DITHER[y % 4].map(|step| u64::from(step * ONE / 16) * packed([1, 1, 1]));
            for (x, (out, pixel)) in (from..).zip(row[from..to].iter_mut().zip(&pixels[from..to])) {
                let (radius, angle) = (usize::from(pixel.radius), usize::from(pixel.angle) % ANGLES);
                // The edge table is offset so that this lands inside `inner` for every radius and wobble.
                let inner = (radius as i32 - self.edge[angle]) as usize % OFFSETS;
                let keep = u32::from(self.inner[inner]) * u32::from(self.outer[radius % RADII]);
                let keep = (keep + (1 << 17)) >> 18; // 15 + 15 bits down to 12.
                let mixed = ground * u64::from(keep) + self.colours[angle] * u64::from(ONE - keep) + noise[x % 4];
                *out = [(mixed >> 12) as u8, (mixed >> (LANE + 12)) as u8, (mixed >> (2 * LANE + 12)) as u8];
            }
        }
    }
}

fn packed([red, green, blue]: [u8; 3]) -> u64 {
    u64::from(red) | u64::from(green) << LANE | u64::from(blue) << (2 * LANE)
}

/// Every pixel's radius and angle. It takes 2 ms, so the window works it out while the toolkit starts.
fn pixel_map() -> &'static [Pixel] {
    static MAP: OnceLock<Vec<Pixel>> = OnceLock::new();
    MAP.get_or_init(|| {
        let middle = SIDE as f32 / 2.0;
        (0..SIDE * SIDE)
            .map(|index| {
                let x = (index % SIDE) as f32 + 0.5 - middle;
                let y = (index / SIDE) as f32 + 0.5 - middle;
                let turn = x.atan2(-y).rem_euclid(TAU) / TAU; // Clockwise from the top.
                Pixel {
                    radius: ((x * x + y * y).sqrt() * STEPS).round() as u16,
                    angle: ((turn * ANGLES as f32).round() as usize % ANGLES) as u16,
                }
            })
            .collect()
    })
}

/// Builds the ring's fixed tables ahead of time. Safe to call from another thread.
pub fn warm_up() {
    pixel_map();
}

/// One circle's line and its blurred glow.
struct Circle {
    radius: f32,
    width: f32,
    strength: f32,
    blur: f32,
}

impl Circle {
    /// Beyond this many pixels from the line it adds nothing.
    fn reach(&self) -> f32 {
        self.width * 3.2 / 2.0 + 3.5 * self.blur
    }

    /// How much ground still shows `offset` pixels from the line: the blurred glow, then the line.
    fn uncovered(&self, offset: f32, table: &[f32; ERF_SIZE]) -> f32 {
        let half = self.width * 3.2 / 2.0;
        let mut glow = 0.0;
        if offset.abs() < self.reach() {
            let scale = FRAC_1_SQRT_2 / self.blur * ERF_STEPS;
            let look = |x: f32| {
                let value = table.get((x.abs() + 0.5) as usize).copied().unwrap_or(1.0);
                value.copysign(x)
            };
            let straight = 0.5 * (look((offset + half) * scale) - look((offset - half) * scale));
            // Blurring a circle piles more light on its inside than its outside.
            let bend = (self.radius / (self.radius + offset).max(self.blur)).sqrt().min(1.6);
            glow = (straight * bend).min(1.0) * self.strength * 0.75;
        }
        let line = (self.width / 2.0 + 0.5 - offset.abs()).clamp(0.0, self.width.min(1.0)) * self.strength;
        (1.0 - glow) * (1.0 - line)
    }
}

/// The Gauss error function, within 0.0005 (Abramowitz and Stegun 7.1.27).
fn erf(x: f32) -> f32 {
    let t = x.abs();
    let d = 1.0 + t * (0.278_393 + t * (0.230_389 + t * (0.000_972 + t * 0.078_108)));
    let y = 1.0 - 1.0 / (d * d * d * d);
    y.copysign(x)
}

#[cfg(test)]
#[path = "ripples_tests.rs"]
mod tests;
