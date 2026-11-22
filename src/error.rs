//! The one error type. Callers match on the variant, never on the text.

use std::fmt;

/// Everything that can go wrong, grouped by what the person can do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Bad arguments or environment. Lull does not start.
    Usage(String),
    /// A system library is missing or too old. Lull does not start.
    Library(String),
    /// The sound file is missing, unreadable or not usable.
    Track(String),
    /// No sound device is connected. Always worth retrying.
    NoDevice,
    /// The window can't open.
    Window(String),
    /// The sound server failed. Always worth retrying.
    Output(String),
    /// Reading or writing Lull's own state failed.
    State(String),
}

/// Shorthand used by every module.
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Usage(why) => write!(f, "{why}"),
            Error::Library(why) => write!(f, "A system audio library is unavailable: {why}"),
            Error::Track(why) => write!(f, "Can't play this sound: {why}"),
            Error::NoDevice => write!(f, "Can't reach the speakers: no sound device is connected"),
            Error::Window(why) => write!(f, "Can't show the window: {why}"),
            Error::Output(why) => write!(f, "Can't reach the speakers: {why}"),
            Error::State(why) => write!(f, "Can't save where you left off: {why}"),
        }
    }
}

impl std::error::Error for Error {}
