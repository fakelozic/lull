//! Turns terminal bytes into actions. Pure, and copes with escape codes split across reads.

/// What a key press asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Play or pause.
    Toggle,
    /// 30 seconds back.
    Back,
    /// 30 seconds forward.
    Forward,
    /// One step louder.
    Louder,
    /// One step quieter.
    Quieter,
    /// Fade out and quit.
    Quit,
}

#[derive(Debug, Default, Clone, Copy)]
enum Seen {
    #[default]
    Nothing,
    Escape,
    Sequence,
}

/// Remembers a half-read escape code between bytes.
#[derive(Debug, Default)]
pub struct Keys {
    seen: Seen,
}

impl Keys {
    /// Feeds one byte and returns an action when a key is complete.
    pub fn push(&mut self, byte: u8) -> Option<Action> {
        match (std::mem::take(&mut self.seen), byte) {
            (_, 0x1b) => self.seen = Seen::Escape,
            (Seen::Escape, b'[' | b'O') => self.seen = Seen::Sequence,
            (Seen::Sequence, b'0'..=b'?') => self.seen = Seen::Sequence,
            (Seen::Sequence, b'A') => return Some(Action::Louder),
            (Seen::Sequence, b'B') => return Some(Action::Quieter),
            (Seen::Sequence, b'C') => return Some(Action::Forward),
            (Seen::Sequence, b'D') => return Some(Action::Back),
            (Seen::Sequence, _) => {}
            (_, b' ') => return Some(Action::Toggle),
            (_, b'q' | b'Q' | 0x11) => return Some(Action::Quit),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn understands_every_key_and_ignores_the_rest() {
        let mut keys = Keys::default();
        let got: Vec<_> = b"x \x1b[A\x1b[B\x1bOC\x1b[1;5D\x1b[15~q\x11".iter().filter_map(|&b| keys.push(b)).collect();
        use Action::*;
        assert_eq!(got, [Toggle, Louder, Quieter, Forward, Back, Quit, Quit]);
    }
}
