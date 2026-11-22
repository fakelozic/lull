//! The real decoder on real, truncated, damaged and missing files. Nothing is played.

use std::path::{Path, PathBuf};

use lull::audio::{Engine, LOOP_FRAMES, Source};
use lull::error::Error;
use lull::ffi::Opus;
use lull::timecode::RATE;

const TOTAL: u64 = 20 * RATE;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/noise.opus")
}

fn scratch(name: &str, bytes: &[u8]) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn read_at(source: &mut Opus, frame: u64, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames * 2];
    source.seek(frame).unwrap();
    source.read(&mut out).unwrap();
    out
}

#[test]
fn knows_the_exact_length() {
    assert_eq!(Opus::open(&fixture()).unwrap().total(), TOTAL);
}

#[test]
fn seek_returns_exactly_what_straight_playback_would() {
    let mut opus = Opus::open(&fixture()).unwrap();
    let whole = read_at(&mut opus, 0, TOTAL as usize);
    let mut targets = vec![0, 1, 647, 648, 959, 3839, 4800, 23_999, 24_000, 24_001, 48_000, 123_457, TOTAL - 900];
    let mut random = 7u64;
    for _ in 0..300 {
        random = random.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let target = random % (TOTAL - 5000);
        // Far away, then a short hop forward, which is where op_pcm_seek alone goes wrong.
        targets.extend([target, target + 1000, target + 3839]);
    }
    for target in targets {
        let got = read_at(&mut opus, target, 900);
        let expected = &whole[target as usize * 2..][..got.len()];
        let worst = got.iter().zip(expected).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-4, "frame {target} is off by {worst}");
    }
}

#[test]
fn plays_through_two_loop_joins_without_clipping() {
    let path = fixture();
    let mut engine = Engine::new(Opus::open(&path).unwrap(), Opus::open(&path).unwrap(), 0, 100).unwrap();
    engine.set_playing(true);
    let mut out = vec![0.0; 1920 * 2];
    let mut left = TOTAL + 2 * (TOTAL - LOOP_FRAMES);
    while left > 0 {
        let count = left.min(1920) as usize;
        engine.render(&mut out[..count * 2]).unwrap();
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        left -= count as u64;
    }
}

#[test]
fn a_truncated_file_errors_or_plays_only_what_is_left() {
    let bytes = std::fs::read(fixture()).unwrap();
    for (name, keep) in [("cut-half.opus", bytes.len() / 2), ("cut-tiny.opus", 300), ("cut-empty.opus", 0)] {
        let path = scratch(name, &bytes[..keep]);
        match Opus::open(&path) {
            Err(error) => assert!(matches!(error, Error::Track(_))),
            Ok(mut opus) => {
                let total = opus.total();
                assert!(total < TOTAL);
                read_at(&mut opus, 0, total as usize);
                assert!(opus.read(&mut [0.0; 2]).is_err(), "reading past the end must fail");
            }
        }
    }
}

#[test]
fn damage_in_the_middle_never_crashes_or_hangs() {
    let mut bytes = std::fs::read(fixture()).unwrap();
    let middle = bytes.len() / 2;
    for (index, byte) in bytes[middle..middle + 6000].iter_mut().enumerate() {
        *byte = (index * 31) as u8;
    }
    if let Ok(mut opus) = Opus::open(&scratch("damaged.opus", &bytes)) {
        let mut out = [0.0; 3840];
        for _ in 0..1000 {
            if opus.read(&mut out).is_err() {
                return;
            }
        }
        panic!("never reached the end");
    }
}

/// The fixture with a comment header of `megabytes`, as embedded cover art makes it.
fn with_art(megabytes: usize) -> PathBuf {
    let original = std::fs::read(fixture()).unwrap();
    let page_len = |at: usize| {
        let segments = usize::from(original[at + 26]);
        27 + segments + original[at + 27..at + 27 + segments].iter().map(|&n| usize::from(n)).sum::<usize>()
    };
    let head = page_len(0);
    let audio = head + page_len(head);
    let art = vec![b'A'; megabytes * 1024 * 1024];
    let mut packet = b"OpusTags".to_vec();
    packet.extend(4u32.to_le_bytes());
    packet.extend(b"lull");
    packet.extend(1u32.to_le_bytes());
    packet.extend((art.len() as u32).to_le_bytes());
    packet.extend(art);
    let mut lacing = vec![255u8; packet.len() / 255];
    lacing.push((packet.len() % 255) as u8);

    let mut out = original[..head].to_vec();
    let (mut at, mut sequence) = (0, 1u32);
    for (index, part) in lacing.chunks(255).enumerate() {
        out.extend(b"OggS\0");
        out.push(u8::from(index > 0));
        out.extend([0; 8]);
        out.extend(&original[14..18]);
        out.extend(sequence.to_le_bytes());
        out.extend([0; 4]); // Never read: Lull hides these pages from the decoder.
        out.push(part.len() as u8);
        out.extend(part);
        let size: usize = part.iter().map(|&n| usize::from(n)).sum();
        out.extend(&packet[at..at + size]);
        at += size;
        sequence += 1;
    }
    out.extend(&original[audio..]);
    scratch(&format!("art-{megabytes}.opus"), &out)
}

#[test]
fn cover_art_of_any_size_plays_exactly_like_the_plain_file() {
    let mut plain = Opus::open(&fixture()).unwrap();
    for megabytes in [1, 30] {
        let mut art = Opus::open(&with_art(megabytes)).unwrap();
        assert_eq!(art.total(), plain.total());
        for frame in [0, 1, 123_457, 700_001, TOTAL - 24_000] {
            assert_eq!(
                read_at(&mut art, frame, 20_000),
                read_at(&mut plain, frame, 20_000),
                "{megabytes} MB at {frame}"
            );
        }
    }
}

#[test]
fn a_swapped_path_cannot_change_an_open_sound() {
    let path = scratch("swap.opus", &std::fs::read(fixture()).unwrap());
    let media = lull::media::Media::open(&path, None).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"something else").unwrap();
    assert_eq!(Opus::from_media(&media).unwrap().total(), TOTAL);
}

#[test]
fn links_out_of_the_music_folder_are_refused() {
    let music = Path::new(env!("CARGO_TARGET_TMPDIR")).join("only-here");
    let _ = std::fs::remove_dir_all(&music);
    std::fs::create_dir_all(&music).unwrap();
    std::os::unix::fs::symlink(fixture(), music.join("escape.opus")).unwrap();
    assert!(lull::media::Media::open(&music.join("escape.opus"), Some(&music)).is_err());
}

#[test]
fn other_files_and_missing_files_are_refused() {
    let not_opus = scratch("not-opus.opus", &[7u8; 9000]);
    for path in [not_opus, PathBuf::from("/nonexistent.opus"), PathBuf::from("/tmp"), PathBuf::from("/dev/zero")] {
        assert!(matches!(Opus::open(&path), Err(Error::Track(_))), "{}", path.display());
    }
}

#[test]
#[ignore = "needs a long file: LULL_TEST_FILE=/path cargo test --release --test decoder -- --ignored --nocapture"]
fn seek_is_exact_across_a_long_real_file() {
    let path = PathBuf::from(std::env::var_os("LULL_TEST_FILE").expect("LULL_TEST_FILE"));
    let mut reference = Opus::open(&path).unwrap();
    let mut opus = Opus::open(&path).unwrap();
    let total = opus.total();
    let (mut random, mut worst, mut slowest) = (11u64, 0.0f32, std::time::Duration::ZERO);
    let mut warmup = vec![0.0; 3 * RATE as usize * 2];
    let trials: u64 = 1000;
    for trial in 0..trials {
        random = random.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let target = 3 * RATE + (total - 5 * RATE) / trials * trial + random % RATE;
        // Straight playback from 3 s earlier is the reference.
        reference.seek(target - 3 * RATE).unwrap();
        reference.read(&mut warmup).unwrap();
        let mut expected = [0.0; 4800];
        reference.read(&mut expected).unwrap();
        let before = std::time::Instant::now();
        opus.seek(target).unwrap();
        slowest = slowest.max(before.elapsed());
        let mut got = [0.0; 4800];
        opus.read(&mut got).unwrap();
        worst = got.iter().zip(&expected).map(|(a, b)| (a - b).abs()).fold(worst, f32::max);
    }
    println!("{trials} seeks over {} s: worst sample error {worst:e}, slowest seek {slowest:?}", total / RATE);
    assert!(worst < 1e-4);
}
