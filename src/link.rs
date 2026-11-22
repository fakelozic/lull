//! What the sound process and the window say to each other, one line per message, over a private
//! socket pair. Pure. There is no path in it: the window picks a sound by its place in the list it
//! was sent, so it can't make the sound process open anything else.

use crate::timecode::RATE;
use crate::visual::{BANDS, Levels};

/// Longest line either side accepts.
pub const MAX_LINE: usize = 64 * 1024;

/// What the sound is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Playback {
    /// Nothing to play.
    #[default]
    Empty,
    /// Paused.
    Paused,
    /// Playing.
    Playing,
    /// Wants to play, but there is no device or server. Retrying.
    Waiting,
}

/// Why the second line shows a message instead of the sound's description.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Problem {
    /// Nothing is wrong.
    #[default]
    None,
    /// No device is connected.
    NoDevice,
    /// The sound server doesn't answer.
    NoServer,
    /// The sound can't be read.
    Unreadable,
    /// The last sound is gone, so another was opened.
    Missing,
    /// The music folder has no sounds.
    NoSounds,
}

/// One row of the Sounds list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sound {
    /// The display name. No control characters.
    pub name: String,
    /// Length in seconds. 0 means it can't be played.
    pub seconds: u64,
    /// The one that is open. By file, since two files can share a name.
    pub current: bool,
}

/// A snapshot for the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    /// What the sound is doing.
    pub playback: Playback,
    /// What is wrong.
    pub problem: Problem,
    /// Position in seconds.
    pub seconds: u64,
    /// Length in seconds.
    pub total: u64,
    /// 0-100.
    pub volume: u8,
    /// The last request number handled. The window ignores snapshots older than its last request,
    /// so a slider never jumps back under the pointer.
    pub handled: u64,
}

/// From the sound process to the window.
#[derive(Debug, Clone, PartialEq)]
pub enum ToWindow {
    /// A snapshot.
    Status(Status),
    /// The open sound's name.
    Title(String),
    /// The list behind the Sounds button.
    Sounds {
        /// Which list this is. `Request::Open` says which list its row is from.
        list: u64,
        /// The rows.
        rows: Vec<Sound>,
    },
    /// Levels, to show once `delay_ms` has passed and they are heard.
    Levels {
        /// Milliseconds from now.
        delay_ms: u16,
        /// The levels.
        levels: Levels,
    },
}

/// What the person asked for in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// The window is up.
    Hello,
    /// Start or stop, stated outright so a repeated message is harmless.
    SetPlaying(bool),
    /// Go to this second.
    Seek(u64),
    /// 0-100.
    Volume(u8),
    /// Send the sounds list.
    List,
    /// Open a row of the list that was on screen when it was pressed.
    Open {
        /// The list.
        list: u64,
        /// The row.
        row: usize,
    },
    /// Start or stop sending levels.
    Watch(bool),
    /// The person closed the window. Lull carries on in the tray.
    Closed,
    /// Ctrl+Q: fade out and quit.
    Quit,
    /// The window gained or lost focus, so a tray click knows whether to hide it or raise it.
    Focused(bool),
}

impl Playback {
    const ALL: [Self; 4] = [Self::Empty, Self::Paused, Self::Playing, Self::Waiting];
}

impl Problem {
    const ALL: [Self; 6] =
        [Self::None, Self::NoDevice, Self::NoServer, Self::Unreadable, Self::Missing, Self::NoSounds];
}

fn one_line(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

impl ToWindow {
    /// One line, newline included.
    pub fn encode(&self) -> String {
        let mut line = match self {
            Self::Status(s) => format!(
                "S\t{}\t{}\t{}\t{}\t{}\t{}",
                s.playback as u8, s.problem as u8, s.seconds, s.total, s.volume, s.handled
            ),
            Self::Title(title) => format!("T\t{}", one_line(title)),
            Self::Sounds { list, rows } => {
                let mut line = format!("M\t{list}");
                for sound in rows {
                    let row = format!("\t{}\t{}\t{}", sound.seconds, u8::from(sound.current), one_line(&sound.name));
                    // A huge folder must not make a line the window refuses.
                    if line.len() + row.len() >= MAX_LINE {
                        break;
                    }
                    line.push_str(&row);
                }
                line
            }
            Self::Levels { delay_ms, levels } => {
                let mut line = format!("L\t{delay_ms}\t");
                for value in levels.bands.iter().chain([&levels.bass]) {
                    line.push_str(&format!("{:03x}", (value.clamp(0.0, 1.3) / 1.3 * 4095.0) as u16));
                }
                line
            }
        };
        line.push('\n');
        line
    }

    /// Reads a line without its newline. `None` if malformed.
    pub fn decode(line: &str) -> Option<Self> {
        let mut parts = line.split('\t');
        Some(match parts.next()? {
            "S" => Self::Status(Status {
                playback: *Playback::ALL.get(parts.next()?.parse::<usize>().ok()?)?,
                problem: *Problem::ALL.get(parts.next()?.parse::<usize>().ok()?)?,
                seconds: parts.next()?.parse().ok()?,
                total: parts.next()?.parse().ok()?,
                volume: parts.next()?.parse().ok().filter(|&volume| volume <= 100)?,
                handled: parts.next()?.parse().ok()?,
            }),
            "T" => Self::Title(parts.next()?.to_owned()),
            "M" => {
                let list = parts.next()?.parse().ok()?;
                let mut rows = Vec::new();
                while let Some(seconds) = parts.next() {
                    let (seconds, current) = (seconds.parse().ok()?, parts.next()? == "1");
                    rows.push(Sound { seconds, current, name: parts.next()?.to_owned() });
                }
                Self::Sounds { list, rows }
            }
            "L" => {
                let delay_ms = parts.next()?.parse().ok()?;
                let hex = parts.next()?;
                if hex.len() != (BANDS + 1) * 3 {
                    return None;
                }
                let mut values = [0.0; BANDS + 1];
                for (index, value) in values.iter_mut().enumerate() {
                    let step = u16::from_str_radix(hex.get(index * 3..index * 3 + 3)?, 16).ok()?;
                    *value = f32::from(step) / 4095.0 * 1.3;
                }
                let mut levels = Levels { bass: values[BANDS], ..Levels::default() };
                levels.bands.copy_from_slice(&values[..BANDS]);
                Self::Levels { delay_ms, levels }
            }
            _ => return None,
        })
    }
}

impl Request {
    /// One line, newline included, carrying the request's number.
    pub fn encode(self, number: u64) -> String {
        let what = match self {
            Self::Hello => "hello".to_owned(),
            Self::SetPlaying(on) => format!("play {}", u8::from(on)),
            Self::Seek(seconds) => format!("seek {seconds}"),
            Self::Volume(percent) => format!("volume {percent}"),
            Self::List => "list".to_owned(),
            Self::Open { list, row } => format!("open {list}:{row}"),
            Self::Watch(on) => format!("watch {}", u8::from(on)),
            Self::Closed => "closed".to_owned(),
            Self::Quit => "quit".to_owned(),
            Self::Focused(on) => format!("focused {}", u8::from(on)),
        };
        format!("{number} {what}\n")
    }

    /// Reads a line without its newline. `None` if malformed or out of range.
    pub fn decode(line: &str) -> Option<(u64, Self)> {
        let mut parts = line.split(' ');
        let number = parts.next()?.parse().ok()?;
        let (name, value) = (parts.next()?, parts.next());
        if parts.next().is_some() {
            return None;
        }
        let flag = || match value? {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        };
        let request = match (name, value) {
            ("hello", None) => Self::Hello,
            ("list", None) => Self::List,
            ("closed", None) => Self::Closed,
            ("quit", None) => Self::Quit,
            ("focused", _) => Self::Focused(flag()?),
            ("play", _) => Self::SetPlaying(flag()?),
            ("watch", _) => Self::Watch(flag()?),
            ("seek", _) => Self::Seek(value?.parse().ok().filter(|seconds: &u64| seconds.checked_mul(RATE).is_some())?),
            ("volume", _) => Self::Volume(value?.parse().ok().filter(|&percent| percent <= 100)?),
            ("open", _) => {
                let (list, row) = value?.split_once(':')?;
                Self::Open { list: list.parse().ok()?, row: row.parse().ok()? }
            }
            _ => return None,
        };
        Some((number, request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_survives_the_trip() {
        let mut messages = vec![
            ToWindow::Title("Rain on Window ☂ (8h)".into()),
            ToWindow::Sounds { list: 1, rows: vec![] },
            ToWindow::Sounds {
                list: 2,
                rows: vec![
                    Sound { name: "Rain".into(), seconds: 28_813, current: true },
                    Sound { name: String::new(), seconds: 0, current: false },
                ],
            },
        ];
        for playback in Playback::ALL {
            for problem in Problem::ALL {
                messages.push(ToWindow::Status(Status {
                    playback,
                    problem,
                    seconds: 5,
                    total: 9,
                    volume: 45,
                    handled: 7,
                }));
            }
        }
        for message in messages {
            assert_eq!(ToWindow::decode(message.encode().trim_end_matches('\n')), Some(message));
        }
        let levels = Levels { bands: std::array::from_fn(|n| n as f32 / 20.0), bass: 0.7 };
        let Some(ToWindow::Levels { delay_ms: 300, levels: got }) =
            ToWindow::decode(ToWindow::Levels { delay_ms: 300, levels }.encode().trim_end())
        else {
            panic!("levels lost")
        };
        assert!(got.bands.iter().zip(levels.bands).all(|(a, b)| (a - b).abs() < 0.001));
    }

    #[test]
    fn every_request_survives_the_trip() {
        use Request::*;
        for request in [
            Hello,
            SetPlaying(true),
            SetPlaying(false),
            Seek(2846),
            Volume(45),
            List,
            Open { list: 4, row: 3 },
            Watch(true),
            Closed,
            Quit,
            Focused(true),
        ] {
            assert_eq!(Request::decode(request.encode(12).trim_end()), Some((12, request)));
        }
    }

    #[test]
    fn bad_requests_are_refused() {
        for bad in [
            "",
            "x hello",
            "1 volume 101",
            "1 seek -1",
            "1 seek 999999999999999",
            "1 open /etc/passwd",
            "1 open 3",
            "1 play 2",
            "1 hello 2",
        ] {
            assert_eq!(Request::decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn names_cannot_forge_a_second_message() {
        let line = ToWindow::Title("a\nS\t2\t0".into()).encode();
        assert_eq!(line.lines().count(), 1);
        assert_eq!(ToWindow::decode(line.trim_end()), Some(ToWindow::Title("aS20".into())));
    }
}
