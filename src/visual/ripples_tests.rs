//! What the ring promises.

use super::*;

fn loud() -> Levels {
    Levels { bands: [1.2; BANDS], bass: 1.2 }
}

fn frame(ring: &mut Ripples) -> Vec<u8> {
    let mut rgb = vec![0; SIDE * SIDE * 3];
    ring.paint(&mut rgb);
    rgb
}

#[test]
fn paused_settles_to_a_still_circle_and_stops_drawing() {
    let mut ring = Ripples::new(false);
    ring.listen(loud());
    for _ in 0..100 {
        ring.advance(0.04, true);
    }
    let mut drawn = 0;
    while ring.advance(0.04, false) {
        drawn += 1;
        assert!(drawn < 100, "never settled");
    }
    assert!(ring.waves.is_empty());
    let first = frame(&mut ring);
    ring.listen(Levels::default());
    assert_eq!(first, frame(&mut ring), "sound must not move a paused ring");
}

#[test]
fn reduced_motion_never_animates() {
    let mut ring = Ripples::new(true);
    ring.listen(loud());
    let first = frame(&mut ring);
    assert!(!ring.advance(1.0, true));
    assert_eq!(first, frame(&mut ring));
}

#[test]
fn waves_never_come_more_than_twice_a_second() {
    let mut ring = Ripples::new(false);
    ring.listen(loud());
    let mut born = 0;
    for _ in 0..500 {
        ring.advance(0.02, true);
        born += usize::from(ring.waves.last().is_some_and(|wave| wave.age == 0.0));
        assert!(ring.waves.len() <= 8);
    }
    assert!(born <= 21, "{born} waves in 10 s");
}

#[test]
fn colours_take_15_seconds_to_go_round() {
    let mut ring = Ripples::new(false);
    for _ in 0..375 {
        ring.advance(0.04, true);
    }
    let turned = (ring.hue - FIRST_HUE).rem_euclid(TAU);
    assert!(turned.min(TAU - turned) < 0.01);
}

#[test]
fn the_glow_fades_to_the_window_colour_at_the_edges() {
    let mut ring = Ripples::new(false);
    ring.listen(loud());
    for _ in 0..200 {
        ring.advance(0.04, true);
        let rgb = frame(&mut ring);
        for corner in [0, SIDE - 1, SIDE * (SIDE - 1), SIDE * SIDE - 1] {
            for channel in 0..3 {
                assert!(u32::from(rgb[corner * 3 + channel]).abs_diff(GROUND[channel] as u32) <= 1);
            }
        }
    }
}

#[test]
fn error_function_matches_known_values() {
    for (x, want) in [(0.0, 0.0), (0.5, 0.5205), (1.0, 0.8427), (-1.0, -0.8427), (3.0, 1.0)] {
        assert!((erf(x) - want).abs() < 0.0008);
    }
}

#[test]
#[ignore = "timing: cargo test --release --lib ripples -- --ignored --nocapture"]
fn how_long_each_part_takes() {
    let mut ring = Ripples::new(false);
    ring.listen(Levels { bands: [0.5; BANDS], bass: 0.4 });
    for _ in 0..200 {
        ring.advance(0.04, true);
    }
    let mut rgb = vec![0; SIDE * SIDE * 3];
    let runs = 3000;
    let per = |start: std::time::Instant| start.elapsed().as_secs_f64() * 1e6 / f64::from(runs);
    let start = std::time::Instant::now();
    (0..runs).for_each(|_| ring.set_edge(66.0));
    println!("edge    {:7.1} us", per(start));
    let start = std::time::Instant::now();
    (0..runs).for_each(|_| ring.set_colours(0.4));
    println!("colours {:7.1} us", per(start));
    let start = std::time::Instant::now();
    (0..runs).for_each(|_| ring.set_inner(66.0, 13.0));
    println!("inner   {:7.1} us", per(start));
    let start = std::time::Instant::now();
    (0..runs).for_each(|_| ring.set_outer(13.0));
    println!("outer   {:7.1} us  ({} waves)", per(start), ring.waves.len());
    let start = std::time::Instant::now();
    (0..runs).for_each(|_| ring.blend(&mut rgb));
    println!("blend   {:7.1} us", per(start));
}

/// The HTML prototype's `paint()` run in Chromium on this exact state, flattened onto the ground.
const PROTOTYPE: &[u8] = include_bytes!("../../tests/fixtures/prototype-ring.rgb");

#[test]
fn looks_like_the_prototype() {
    let mut ring = Ripples::new(false);
    ring.listen(Levels {
        bands: std::array::from_fn(|band| 0.45 + 0.30 * (band as f32 * 0.45 + 1.0).sin()),
        bass: 0.42,
    });
    (ring.amount, ring.hue, ring.time) = (1.0, 3.4, 7.3);
    ring.waves = [(0.3, 0.5), (1.1, 0.9), (2.0, 0.4), (2.9, 0.7)].map(|(age, force)| Wave { age, force }).to_vec();
    let ours = frame(&mut ring);
    if let Some(folder) = std::env::var_os("LULL_SHOTS").map(std::path::PathBuf::from) {
        for (name, rgb) in [("ring-app", &ours[..]), ("ring-prototype", PROTOTYPE)] {
            let mut file = format!("P6\n{SIDE} {SIDE}\n255\n").into_bytes();
            file.extend(rgb);
            std::fs::write(folder.join(format!("{name}.ppm")), file).unwrap();
        }
    }
    let total: u64 = ours.iter().zip(PROTOTYPE).map(|(a, b)| u64::from(a.abs_diff(*b))).sum();
    let mean = total as f32 / ours.len() as f32;
    println!("mean difference from the prototype: {mean:.2} of 255");
    assert!(mean < 4.0, "drifted from the picked look: {mean}");
}

#[test]
fn every_pixel_lands_inside_the_edge_table() {
    let mut ring = Ripples::new(false);
    ring.amount = 1.0;
    for bass in [0.0, 1.3] {
        ring.levels = Levels { bands: [1.3 * bass / 1.3; BANDS], bass };
        for step in 0..40 {
            ring.time = step as f32 * 0.4;
            ring.set_edge(INNER_RADIUS + 12.0 * bass);
            for pixel in ring.pixels {
                let offset = i32::from(pixel.radius) - ring.edge[usize::from(pixel.angle)];
                assert!((0..OFFSETS as i32).contains(&offset), "offset {offset} at bass {bass}");
            }
        }
    }
}
