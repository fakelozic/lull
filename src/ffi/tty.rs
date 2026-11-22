//! Single-key input: switches the terminal off line mode, and always puts it back.

use std::io::IsTerminal;

/// Restores the terminal's original settings when dropped.
pub struct RawInput {
    original: libc::termios,
}

impl RawInput {
    /// Switches stdin to single keys. Returns `None` when stdin is not a terminal.
    pub fn enable() -> Option<Self> {
        if !std::io::stdin().is_terminal() {
            return None;
        }
        // SAFETY: termios is plain data and tcgetattr fills it before we read it.
        unsafe {
            let mut original: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut original) != 0 {
                return None;
            }
            let mut raw = original;
            // ISIG stays on so Ctrl+C works. IXON goes off so Ctrl+Q reaches us.
            raw.c_lflag &= !(libc::ICANON | libc::ECHO);
            raw.c_iflag &= !libc::IXON;
            // Ctrl+Z off: a stopped player would leave the stream open and run the buffer empty.
            raw.c_cc[libc::VSUSP] = 0;
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            (libc::tcsetattr(0, libc::TCSANOW, &raw) == 0).then_some(Self { original })
        }
    }
}

impl Drop for RawInput {
    fn drop(&mut self) {
        // SAFETY: `original` came from tcgetattr on the same descriptor.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.original) };
    }
}
