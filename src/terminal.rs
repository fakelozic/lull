//! The terminal front end (`--no-window`): single keys and one status line. Also what the tests drive.

use std::io::{IsTerminal, Read, Write};

use crate::ffi::RawInput;
use crate::keys::{Action, Keys};
use crate::timecode;

/// Owns the terminal while Lull runs.
pub struct Terminal {
    interactive: bool,
    last_line: String,
    _raw: Option<RawInput>,
}

impl Terminal {
    /// Starts reading keys, passing each action to `on_key` from a reader thread.
    pub fn open(on_key: impl Fn(Action) + Send + 'static) -> std::io::Result<Self> {
        let raw = RawInput::enable();
        std::thread::Builder::new().name("keys".into()).spawn(move || {
            let mut keys = Keys::default();
            let mut buffer = [0u8; 32];
            let mut input = std::io::stdin();
            loop {
                match input.read(&mut buffer) {
                    Ok(0) => return,
                    Ok(count) => buffer[..count].iter().filter_map(|&b| keys.push(b)).for_each(&on_key),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return, // The terminal closed; SIGHUP handles the exit.
                }
            }
        })?;
        Ok(Self { interactive: std::io::stdout().is_terminal(), last_line: String::new(), _raw: raw })
    }

    /// Shows the sound's name and the keys.
    pub fn intro(&self, title: &str) {
        self.print(&format!("{title}\nSpace play/pause   Left/Right 30 s   Up/Down volume   q quit\n"));
    }

    /// Redraws the status line if it changed. `position` and `total` are in frames.
    pub fn status(&mut self, label: &str, position: u64, total: u64, volume: u8) {
        if !self.interactive {
            return;
        }
        let at = timecode::format(position / timecode::RATE);
        let of = timecode::format(total / timecode::RATE);
        let line = format!("{label:<14}{at} / {of}   volume {volume}%");
        if line != self.last_line {
            self.print(&format!("\r\x1b[K{line}"));
            self.last_line = line;
        }
    }

    /// Forces a redraw, for after a log line printed over the status.
    pub fn invalidate(&mut self) {
        self.last_line.clear();
    }

    fn print(&self, text: &str) {
        if !self.interactive {
            return;
        }
        let mut out = std::io::stdout();
        // A closed terminal is handled by SIGHUP, not here.
        let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.print("\n");
    }
}
