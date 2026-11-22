//! Lull on the session bus: the tray, the media controls (MPRIS) and "show yourself" for a second launch.
//!
//! Main never waits for the bus. It leaves a snapshot and a nudge, and this module's thread does the
//! talking. What comes in becomes an `Ask` in main's bounded mailbox. The bus is open to the user's own
//! programs, so every argument is bounded here and paths must be inside ~/Music (docs/api.md).

mod control;
mod icon;
mod mpris;
mod tray;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

pub use control::{Handoff, hand_over};

use crate::link::{Playback, Problem};
use crate::log;

/// Lull's name on the bus. MPRIS fixes the prefix.
pub const NAME: &str = "org.mpris.MediaPlayer2.lull";
/// The one object path MPRIS allows.
pub const OBJECT: &str = "/org/mpris/MediaPlayer2";
/// Method calls that may wait inside the connection.
const QUEUED: usize = 32;
const TIMEOUT: Duration = Duration::from_secs(2);

/// Something asked of Lull from outside its window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// Play.
    Play,
    /// Pause.
    Pause,
    /// Play or pause.
    Toggle,
    /// Move by this many frames.
    SeekBy(i64),
    /// Go to this frame, if this sound is still the open one.
    SeekTo {
        /// The sound's number.
        track: u64,
        /// Where to.
        frame: u64,
    },
    /// 0-100.
    Volume(u8),
    /// This much louder or quieter.
    VolumeBy(i16),
    /// Bring the window up.
    Show {
        /// Wayland's permission to take focus.
        token: Option<String>,
    },
    /// Tray click: hide the window if it has focus, else bring it up.
    ShowOrHide {
        /// Wayland's permission to take focus.
        token: Option<String>,
    },
    /// A second launch: show, and do what its arguments said.
    Launch(Launch),
    /// Open this file, already checked to be in ~/Music, and play it.
    Open(PathBuf),
    /// Fade out and quit.
    Quit,
}

/// What a second `lull` passes to the first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launch {
    /// Wayland's permission to take focus.
    pub token: Option<String>,
    /// `--play`.
    pub play: bool,
    /// `--at`, in seconds.
    pub at: Option<u64>,
    /// A file, inside ~/Music.
    pub file: Option<PathBuf>,
}

/// What the bus shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Playing, paused or waiting.
    pub playback: Playback,
    /// What is wrong.
    pub problem: Problem,
    /// The big line.
    pub title: String,
    /// The line under it.
    pub detail: String,
    /// Goes up with each switch.
    pub track: u64,
    /// In frames.
    pub position: u64,
    /// In frames.
    pub total: u64,
    /// 0-100.
    pub volume: u8,
}

/// Takes a token from outside, if it looks like one.
pub fn clean_token(token: &str) -> Option<String> {
    let fine = (1..=256).contains(&token.len()) && token.bytes().all(|byte| byte.is_ascii_graphic());
    fine.then(|| token.to_owned())
}

/// The one way a path gets in from the bus: absolute, existing, and really inside `music`.
pub fn inside_music(path: &Path, music: &Path) -> Option<PathBuf> {
    if path.as_os_str().len() > 4096 || !path.is_absolute() {
        return None;
    }
    let real = path.canonicalize().ok()?;
    let root = music.canonicalize().ok()?;
    (real.starts_with(root) && real.is_file()).then_some(real)
}

pub(crate) struct Shared {
    snapshot: Mutex<Snapshot>,
    music: PathBuf,
    report: Box<dyn Fn(Ask) + Send + Sync>,
    tray_online: AtomicBool,
    stopping: AtomicBool,
    jumped: AtomicBool,
    hint: AtomicBool,
}

impl Shared {
    fn now(&self) -> Snapshot {
        self.snapshot.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn ask(&self, ask: Ask) {
        if !self.stopping.load(Ordering::Relaxed) {
            (self.report)(ask);
        }
    }
}

/// Main's handle on all of it.
pub struct Remote {
    shared: Arc<Shared>,
    nudge: SyncSender<()>,
}

impl Remote {
    /// Connects on its own thread, so launch never waits for the bus. `report` must not block.
    pub fn start(music: PathBuf, report: impl Fn(Ask) + Send + Sync + 'static) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            snapshot: Mutex::default(),
            music,
            report: Box::new(report),
            tray_online: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            jumped: AtomicBool::new(false),
            hint: AtomicBool::new(false),
        });
        let (nudge, nudged) = mpsc::sync_channel(1);
        let theirs = Arc::clone(&shared);
        std::thread::Builder::new().name("remote".into()).spawn(move || serve(&theirs, &nudged))?;
        Ok(Self { shared, nudge })
    }

    /// Leaves the newest state for the bus. `jumped` means the position moved other than by playing.
    pub fn update(&self, snapshot: Snapshot, jumped: bool) {
        let changed = {
            let mut held = self.shared.snapshot.lock().unwrap_or_else(PoisonError::into_inner);
            let changed = Snapshot { position: 0, ..held.clone() } != Snapshot { position: 0, ..snapshot.clone() };
            *held = snapshot;
            changed
        };
        if jumped {
            self.shared.jumped.store(true, Ordering::Relaxed);
        }
        // Position alone is only ever read, so it doesn't wake anyone.
        if changed || jumped {
            self.nudge();
        }
    }

    /// True if there is a tray icon to come back from.
    pub fn has_tray(&self) -> bool {
        self.shared.tray_online.load(Ordering::Relaxed)
    }

    /// Shows the "still in the tray" notification.
    pub fn say_still_here(&self) {
        self.shared.hint.store(true, Ordering::Relaxed);
        self.nudge();
    }

    /// From here on a second launch is told to wait and start afresh, not to show a window that is going.
    pub fn stopping(&self) {
        self.shared.stopping.store(true, Ordering::Relaxed);
    }

    fn nudge(&self) {
        // A full channel already holds a nudge, and that one will read this state.
        let _ = self.nudge.try_send(());
    }
}

fn serve(shared: &Arc<Shared>, nudged: &Receiver<()>) {
    let connection = match connect(shared) {
        Ok(connection) => connection,
        Err(error) => {
            log::warn("remote.unavailable", &format!("no media keys, tray or second launch: {error}"));
            return;
        }
    };
    log::note("remote.ready", &format!("name={NAME}"));
    let tray = tray::start(shared);
    let mut shown = Snapshot::default();
    loop {
        let now = shared.now();
        mpris::announce(&connection, &shown, &now, shared.jumped.swap(false, Ordering::Relaxed));
        if let Some(tray) = &tray {
            tray::show(tray, &now);
        }
        shown = now;
        if shared.hint.swap(false, Ordering::Relaxed) {
            tray::say_still_here(&connection, shown.playback == Playback::Playing);
        }
        if nudged.recv().is_err() {
            return;
        }
    }
}

fn connect(shared: &Arc<Shared>) -> zbus::Result<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::session()?
        .method_timeout(TIMEOUT)
        .max_queued(QUEUED)
        .serve_at(OBJECT, mpris::Root(Arc::clone(shared)))?
        .serve_at(OBJECT, mpris::Player(Arc::clone(shared)))?
        .serve_at(OBJECT, control::Control(Arc::clone(shared)))?
        .name(NAME)?
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_from_outside_are_bounded() {
        assert_eq!(clean_token("kwin-12_ab"), Some("kwin-12_ab".into()));
        for bad in ["", "has space", "nul\0", &"x".repeat(257)] {
            assert_eq!(clean_token(bad), None);
        }
    }

    #[test]
    fn paths_from_the_bus_must_be_inside_music() {
        let music = std::env::temp_dir().join(format!("lull-bus-music-{}", std::process::id()));
        std::fs::create_dir_all(&music).unwrap();
        std::fs::write(music.join("a.opus"), "").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", music.join("link.opus")).unwrap();
        assert!(inside_music(&music.join("a.opus"), &music).is_some());
        for bad in
            [music.join("link.opus"), music.join("../../etc/passwd"), "relative.opus".into(), "/etc/passwd".into()]
        {
            assert_eq!(inside_music(&bad, &music), None, "{}", bad.display());
        }
        std::fs::remove_dir_all(music).unwrap();
    }
}
