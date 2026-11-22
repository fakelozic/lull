//! Opens sounds and builds the Sounds list. Only ever reads.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::audio::{Engine, Source};
use crate::error::Result;
use crate::ffi::Opus;
use crate::link::Sound;
use crate::media::Media;
use crate::timecode::RATE;
use crate::track;

/// The most rows the list offers.
const MOST: usize = 200;
/// A cold or broken disk must not hold main up for longer. The list is cut short instead.
const LIST_TIME: Duration = Duration::from_secs(2);

/// A sound, opened and ready to play.
pub struct Opened {
    /// Ready, paused.
    pub engine: Engine<Opus>,
    /// The file.
    pub path: PathBuf,
    /// The display name.
    pub title: String,
    /// Length in frames.
    pub total: u64,
}

impl Opened {
    /// Opens `path` paused at `start` (frames) with the 0-100 volume. With `within`, it must really be
    /// inside that folder. Both decoders share the one opened file, so they can't end up on two recordings.
    pub fn new(path: PathBuf, start: u64, volume: u8, within: Option<&Path>) -> Result<Self> {
        let media = Media::open(&path, within)?;
        let engine = Engine::new(Opus::from_media(&media)?, Opus::from_media(&media)?, start, volume)?;
        Ok(Self { total: engine.total(), title: track::title(&path), engine, path })
    }
}

/// The rows for the window and, in the same order, the files they stand for.
/// Files that can't play, or really live outside `music`, are listed as "can't play".
pub fn catalogue(music: &Path, current: Option<&Path>) -> (Vec<Sound>, Vec<PathBuf>) {
    let Ok(files) = track::list(music, MOST) else { return Default::default() };
    let deadline = Instant::now() + LIST_TIME;
    files
        .into_iter()
        .take_while(|_| Instant::now() < deadline)
        .map(|path| {
            let opened = Media::open(&path, Some(music)).and_then(|media| Opus::from_media(&media));
            let seconds = opened.map_or(0, |opus| (opus.total() / RATE).max(1));
            let current = current == Some(path.as_path());
            (Sound { name: track::title(&path), seconds, current }, path)
        })
        .unzip()
}
