//! Lull's only writes: where you left off, plus the single-instance lock.
//! The file is replaced atomically, so a crash leaves the old or the new one, never half of one.

use std::ffi::OsStr;
use std::fs::{self, File, TryLockError};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::private_file;

/// The volume on first launch (docs/decisions.md#volume).
pub const DEFAULT_VOLUME: u8 = 20;
const HEADER: &str = "lull-state 1";
/// A path is at most 4 KiB, so a real state file is well under this.
const MOST: u64 = 8 * 1024;

/// What is remembered between runs. Never whether it was playing: it always restores paused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// The last sound.
    pub track: Option<PathBuf>,
    /// The position in frames at 48 kHz.
    pub position: u64,
    /// The volume, 0-100.
    pub volume: u8,
}

impl Default for State {
    fn default() -> Self {
        Self { track: None, position: 0, volume: DEFAULT_VOLUME }
    }
}

impl State {
    /// Encodes the file. The track comes last, as raw bytes, because names may hold newlines.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = format!("{HEADER}\nposition={}\nvolume={}\ntrack=", self.position, self.volume).into_bytes();
        if let Some(track) = &self.track {
            bytes.extend_from_slice(track.as_os_str().as_bytes());
        }
        bytes
    }

    /// Decodes the file, or returns `None` if it is damaged or from another version.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut fields = bytes.splitn(4, |&byte| byte == b'\n');
        if fields.next()? != HEADER.as_bytes() {
            return None;
        }
        let position = std::str::from_utf8(fields.next()?.strip_prefix(b"position=")?).ok()?.parse().ok()?;
        let volume: u8 = std::str::from_utf8(fields.next()?.strip_prefix(b"volume=")?).ok()?.parse().ok()?;
        let track = fields.next()?.strip_prefix(b"track=")?;
        let track = (!track.is_empty()).then(|| PathBuf::from(OsStr::from_bytes(track)));
        // A saved path is always absolute. Anything else was edited or damaged.
        if volume > 100 || track.as_ref().is_some_and(|path| !path.is_absolute() || track_has_nul(path)) {
            return None;
        }
        Some(Self { track, position, volume })
    }
}

fn track_has_nul(path: &Path) -> bool {
    path.as_os_str().as_bytes().contains(&0)
}

/// Reads and writes the state in one folder.
pub struct Store {
    folder: PathBuf,
}

impl Store {
    /// Creates the folder (private to the user) if needed.
    pub fn open(folder: &Path) -> Result<Self> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder)
            .map_err(|e| Error::State(format!("{}: {e}", folder.display())))?;
        Ok(Self { folder: folder.to_owned() })
    }

    /// Loads the state. No file means defaults. A damaged file is an error the caller can log.
    pub fn load(&self) -> Result<State> {
        match private_file::read(&self.folder.join("state"), MOST) {
            Ok(bytes) => State::decode(&bytes).ok_or_else(|| Error::State("the saved file is damaged".into())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(Error::State(e.to_string())),
        }
    }

    /// Saves atomically: a new temporary file, synced, renamed over the old one, folder synced.
    pub fn save(&self, state: &State) -> Result<()> {
        private_file::replace(&self.folder.join("state"), &state.encode()).map_err(|e| Error::State(e.to_string()))
    }
}

/// Takes the single-instance lock, held until the returned file is dropped or the process dies.
pub fn lock(folder: &Path) -> Result<File> {
    let path = folder.join("lull.lock");
    let file = private_file::for_lock(&path).map_err(|e| Error::State(format!("{}: {e}", path.display())))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(Error::Usage("Lull is already running".into())),
        Err(TryLockError::Error(e)) => Err(Error::State(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_survives_odd_file_names() {
        let track = PathBuf::from(OsStr::from_bytes(b"/m/rain\nand \xff thunder.opus"));
        let state = State { track: Some(track), position: 1_383_000_000, volume: 35 };
        assert_eq!(State::decode(&state.encode()), Some(state));
    }

    #[test]
    fn damaged_state_is_refused() {
        for bad in [
            &b""[..],
            b"lull-state 1\nposition=5",
            b"lull-state 2\nposition=5\nvolume=20\ntrack=",
            b"lull-state 1\nposition=-5\nvolume=20\ntrack=",
            b"lull-state 1\nposition=5\nvolume=101\ntrack=",
            b"lull-state 1\nposition=5\nvolume=20\ntrack=relative.opus",
            b"lull-state 1\nposition=5\nvolume=20\ntrack=/a\0b",
        ] {
            assert_eq!(State::decode(bad), None);
        }
    }

    #[test]
    fn first_launch_is_20_percent() {
        assert_eq!(State::default().volume, 20);
    }
}
