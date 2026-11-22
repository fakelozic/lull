//! The words the window shows. Pure. Each detail line fits 312 px at 16 px.

use crate::link::{Playback, Problem, Status};
use crate::track;

/// What the two text lines say.
#[derive(Debug, PartialEq, Eq)]
pub struct Lines {
    /// The big line.
    pub heading: String,
    /// The line under it.
    pub detail: String,
    /// True when the detail is a problem, which is shown brighter.
    pub alert: bool,
}

/// Chooses the words for a status and the open sound's name.
pub fn lines(status: &Status, name: &str) -> Lines {
    let (title, description) = track::headline(name);
    let heading = match (status.playback, status.problem) {
        (Playback::Empty, Problem::None) => String::new(),
        (Playback::Empty, Problem::Unreadable) => "Can't open it".to_owned(),
        (Playback::Empty, _) => "No sounds yet".to_owned(),
        _ => title,
    };
    let problem = match status.problem {
        Problem::None => None,
        Problem::NoDevice => Some("No sound device. Waiting for one"),
        Problem::NoServer => Some("Can't reach the speakers. Retrying"),
        Problem::Unreadable => Some("Can't play this file. Pick another"),
        Problem::Missing => Some("Last sound is gone. Opened this one"),
        Problem::NoSounds => Some("Add .opus files to ~/Music"),
    };
    Lines { heading, detail: problem.map_or(description, str::to_owned), alert: problem.is_some() }
}

/// A sound's length in the list: "8 h", "1 h 25 min", "40 min", "20 s".
pub fn length(seconds: u64) -> String {
    let (hours, minutes) = (seconds / 3600, seconds % 3600 / 60);
    match (seconds, hours, minutes) {
        (0, _, _) => "can't play".to_owned(),
        (_, 0, 0) => format!("{seconds} s"),
        (_, 0, _) => format!("{minutes} min"),
        (_, _, 0) => format!("{hours} h"),
        _ => format!("{hours} h {minutes} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_the_sound_or_the_problem_on_the_same_line() {
        let mut status = Status { playback: Playback::Paused, ..Status::default() };
        let name = "Rain on Window with Thunder (8h)";
        assert_eq!(lines(&status, name).detail, "with thunder");
        status.problem = Problem::NoDevice;
        let shown = lines(&status, name);
        assert_eq!(shown.heading, "Rain on Window");
        assert!(shown.alert && shown.detail.starts_with("No sound device"));
    }

    #[test]
    fn lengths_read_naturally() {
        assert_eq!(length(28_813), "8 h");
        assert_eq!(length(5_100), "1 h 25 min");
        assert_eq!(length(20), "20 s");
        assert_eq!(length(0), "can't play");
    }
}
