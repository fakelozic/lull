//! OKLCH to sRGB, the same formula as the prototype. Equal lightness looks equally bright in every hue.

/// Converts OKLCH (hue in radians) to sRGB with channels 0-255, clamped to the gamut.
pub fn oklch(lightness: f32, chroma: f32, hue: f32) -> [f32; 3] {
    let (sin, cos) = hue.sin_cos();
    let (a, b) = (chroma * cos, chroma * sin);
    let l = (lightness + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (lightness - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (lightness - 0.089_484_18 * a - 1.291_486 * b).powi(3);
    let linear = [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s,
        -0.004_196_086 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ];
    linear.map(|x| {
        let x = x.clamp(0.0, 1.0);
        255.0 * if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_has_no_tint_and_the_ends_are_black_and_white() {
        assert_eq!(oklch(0.0, 0.0, 1.0), [0.0; 3]);
        for channel in oklch(1.0, 0.0, 2.0) {
            assert!((channel - 255.0).abs() < 0.1);
        }
        let [r, g, b] = oklch(0.5, 0.0, 4.0);
        assert!((r - g).abs() < 0.1 && (g - b).abs() < 0.1);
    }
}
