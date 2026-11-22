//! Pure conversions between frames, seconds and the clock text people read.

/// Frames per second. Opus always decodes at 48 kHz and the DAC runs at 48 kHz, so nothing resamples.
pub const RATE: u64 = 48_000;

/// Formats whole seconds as `M:SS`, or `H:MM:SS` from one hour.
pub fn format(seconds: u64) -> String {
    let (hours, minutes, secs) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{secs:02}") } else { format!("{minutes}:{secs:02}") }
}

/// Parses `SS`, `M:SS` or `H:MM:SS` into seconds. Returns `None` for anything else.
pub fn parse(text: &str) -> Option<u64> {
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total: u64 = 0;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let value: u64 = part.parse().ok()?;
        if index > 0 && value >= 60 {
            return None;
        }
        total = total.checked_mul(60)?.checked_add(value)?;
    }
    // Must still fit once converted to frames.
    total.checked_mul(RATE).map(|_| total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_like_a_clock() {
        assert_eq!(format(0), "0:00");
        assert_eq!(format(2846), "47:26");
        assert_eq!(format(28_813), "8:00:13");
    }

    #[test]
    fn times_parse_back() {
        assert_eq!(parse("2712"), Some(2712));
        assert_eq!(parse("47:26"), Some(2846));
        assert_eq!(parse("8:00:13"), Some(28_813));
    }

    #[test]
    fn nonsense_times_are_refused() {
        for bad in ["", ":", "1:60", "-3", "+3", "1:2:3:4", "a", "99999999999999999999", "999999999999999"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }
}
