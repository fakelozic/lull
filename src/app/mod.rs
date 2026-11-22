//! The sound process's brain: intent (playing or not), saving, and the main loop.
//! `window` answers the window, `remote` answers the tray, media keys and second launches.

mod audio;
mod health;
mod remote;
mod window;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use crate::audio::{Command, Event, Player};
use crate::error::{Error, Result};
use crate::ffi::Opus;
use crate::front::Front;
use crate::host::FromWindow;
use crate::inbox::{Receiver, Sender};
use crate::keys::Action;
use crate::link::{Playback, Problem, Status};
use crate::log;
use crate::remote::{Ask, Remote};
use crate::sounds::Opened;
use crate::state::{State, Store};
use crate::timecode::{self, RATE};

const SAVE_EVERY: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_millis(250);
/// Covers a connect in progress (3 s) plus the fade and its tail (1 s).
const SHUTDOWN_GRACE: Duration = Duration::from_secs(6);
/// The audio thread takes up to 40 ms to act on a seek. Show the target until then.
const SEEK_SETTLES: Duration = Duration::from_millis(200);
const SEEK_STEP: i64 = 30 * RATE as i64;
const VOLUME_STEP: u8 = 5;
/// A position change bigger than this (1.5 s) that playing doesn't explain is a jump, for MPRIS.
const JUMP: u64 = RATE * 3 / 2;
/// Show requests closer than this are one request: a double-click on the tray, or another program
/// calling in a loop, which would otherwise start a window process each time.
const SHOW_GAP: Duration = Duration::from_millis(300);
/// SIGHUP: the terminal closed.
const HANGUP: usize = 1;
/// Exists once the "still in the tray" notice has been shown.
const HINT_MARKER: &str = "tray-notice-shown";

/// Everything that can reach the main loop.
pub enum Message {
    /// A terminal key.
    Key(Action),
    /// News from the audio thread.
    Audio(Event),
    /// News from the window process.
    Window(FromWindow),
    /// The tray, media keys or a second launch.
    Remote(Ask),
}

struct Session {
    player: Player<Opus>,
    title: String,
    total: u64,
}

struct Coming {
    path: PathBuf,
    title: String,
    total: u64,
}

/// The running app.
pub struct App {
    front: Front,
    sender: Sender<Message>,
    store: Store,
    state: State,
    saved: Option<State>,
    music: PathBuf,
    /// The state folder.
    folder: PathBuf,
    remote: Option<Remote>,
    hint_due: bool,
    window_changed: Option<Instant>,
    /// What the bus was last told: track, position, when, and whether it was playing.
    told: Option<(u64, u64, Instant, bool)>,
    session: Option<Session>,
    /// The switch under way. There is never more than one, so each `Opened` can only mean this one.
    coming: Option<Coming>,
    /// Asked for during a switch: only the latest is kept.
    wanted: Option<PathBuf>,
    track: u64,
    /// The last two lists sent, by number: a press may be on the one just replaced.
    lists: VecDeque<(u64, Vec<PathBuf>)>,
    next_list: u64,
    /// The window can't come back. The sound plays on until it stops.
    windowless: bool,
    seek_changed: Option<Instant>,
    playing: bool,
    reconnecting: bool,
    watched: bool,
    closed: bool,
    problem: Problem,
    last_loss: Option<(Error, Duration)>,
    seeking: Option<(u64, Instant)>,
    handled: u64,
    started: Instant,
    next_health: Instant,
    volume_changed: Option<Instant>,
    underruns: u64,
    underrun_logged: Option<Instant>,
}

impl App {
    /// Builds the app with nothing open. `state` is what was saved.
    pub fn new(
        front: Front,
        sender: Sender<Message>,
        store: Store,
        state: State,
        music: PathBuf,
        folder: PathBuf,
    ) -> Self {
        Self {
            front,
            sender,
            store,
            state,
            saved: None,
            music,
            hint_due: !folder.join(HINT_MARKER).exists(),
            folder,
            remote: None,
            window_changed: None,
            told: None,
            session: None,
            coming: None,
            wanted: None,
            track: 0,
            lists: VecDeque::new(),
            next_list: 1,
            windowless: false,
            seek_changed: None,
            playing: false,
            reconnecting: false,
            watched: false,
            closed: false,
            problem: Problem::None,
            last_loss: None,
            seeking: None,
            handled: 0,
            started: Instant::now(),
            next_health: Instant::now() + health::FIRST_HEALTH,
            volume_changed: None,
            underruns: 0,
            underrun_logged: None,
        }
    }

    /// Shows a problem found while starting.
    pub fn set_problem(&mut self, problem: Problem) {
        self.problem = problem;
    }

    /// Takes the handle on the tray and media controls.
    pub fn set_remote(&mut self, remote: Remote) {
        self.remote = Some(remote);
    }

    /// Sends what is known so far, so the window's first frame is the whole picture.
    pub fn show_now(&mut self) {
        self.send_title();
        self.refresh();
    }

    /// Starts the audio thread on a sound, paused.
    pub fn load(&mut self, sound: Opened) -> Result<()> {
        let events = self.sender.clone();
        let player = Player::start(sound.engine, sound.title.clone(), move |event| match event {
            Event::Levels(_) | Event::Underrun(_) => events.send_or_drop(Message::Audio(event)),
            _ => events.send(Message::Audio(event)),
        })
        .map_err(|e| Error::Usage(format!("Can't start a thread: {e}")))?;
        self.track = 0;
        self.told = None;
        player.send(Command::Watch(self.watched));
        log::note("track.loaded", &format!("path={} volume={}", sound.path.display(), self.state.volume));
        self.state.track = Some(sound.path);
        self.state.position = player.position();
        if let Front::Terminal(terminal) = &self.front {
            terminal.intro(&sound.title);
        }
        log::event("app.ready", &format!("at={} of={}", clock(self.state.position), clock(sound.total)));
        self.session = Some(Session { player, title: sound.title, total: sound.total });
        Ok(())
    }

    /// Runs until quit, close or a signal, then fades out and saves.
    pub fn run(mut self, inbox: &Receiver<Message>, stop: &AtomicUsize, play: bool) -> Result<()> {
        if play {
            self.set_playing(true);
        }
        let mut next_save = Instant::now() + SAVE_EVERY;
        let mut next_tick = Instant::now();
        let mut result = Ok(());
        while !self.closed {
            match stop.load(Ordering::Relaxed) {
                0 => {}
                // Closing the terminal it was started from mustn't stop a player that lives in the tray.
                // Logging out sends SIGTERM.
                HANGUP if self.has_tray() => {
                    if stop.compare_exchange(HANGUP, 0, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                        log::note("app.terminal_closed", "carrying on in the tray");
                    }
                }
                _ => break,
            }
            match inbox.recv_timeout(TICK) {
                Ok(Message::Key(action)) => self.on_key(action),
                Ok(Message::Remote(ask)) => self.on_ask(ask),
                Ok(Message::Audio(Event::Levels(heard))) => {
                    self.forward_levels(&heard);
                    // Levels keep coming while playing, so the timeout may never fire:
                    // the tick has to run on the clock, or the time and the 30 s save both stop.
                    if Instant::now() < next_tick {
                        continue;
                    }
                }
                Ok(Message::Audio(event)) => self.on_audio(event),
                Ok(Message::Window(news)) => {
                    if let Err(error) = self.on_window(news) {
                        result = Err(error);
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }
            if self.session.as_ref().is_some_and(|session| !session.player.is_running()) {
                result = Err(Error::Output("audio stopped unexpectedly. Your place was saved".into()));
                break;
            }
            if self.windowless && !self.playing {
                self.closed = true; // Nothing is playing and there is no way to start it.
            }
            if self.front.window().is_some_and(|window| window.is_stuck_starting()) {
                log::warn("window.stuck", "it didn't come up in 15 s");
                if let Err(error) = self.window_gone() {
                    result = Err(error);
                    break;
                }
            }
            self.periodic_log();
            if Instant::now() >= next_save {
                self.save();
                next_save = Instant::now() + SAVE_EVERY;
            }
            self.refresh();
            next_tick = Instant::now() + TICK;
        }
        let why = match (stop.load(Ordering::Relaxed), &result) {
            (1, _) => "SIGHUP",
            (2, _) => "SIGINT",
            (15, _) => "SIGTERM",
            (_, Err(_)) => "error",
            _ => "quit, or closed with no tray",
        };
        if let Some(remote) = &self.remote {
            remote.stopping();
        }
        if let Some(window) = self.front.window() {
            window.put_away(); // Gone at once. The sound takes its second to fade.
        }
        let finished = self.finish(why);
        result.and(finished)
    }

    fn set_playing(&mut self, playing: bool) {
        let Some(session) = &self.session else { return };
        if !session.player.send(if playing { Command::Play } else { Command::Pause }) {
            log::warn("audio.busy", "a press was dropped: the audio thread is not answering");
            return;
        }
        if playing && matches!(self.problem, Problem::Unreadable | Problem::Missing) {
            self.problem = Problem::None;
        }
        if !playing {
            self.reconnecting = false;
            self.last_loss = None;
            if matches!(self.problem, Problem::NoDevice | Problem::NoServer) {
                self.problem = Problem::None;
            }
        }
        self.playing = playing;
        log::note(
            if playing { "user.play" } else { "user.pause" },
            &format!("at={}", clock(session.player.position())),
        );
    }

    fn set_volume(&mut self, volume: u8) {
        self.state.volume = volume;
        self.volume_changed = Some(Instant::now());
        if let Some(session) = &self.session {
            session.player.send(Command::SetVolume(volume));
        }
    }

    fn on_key(&mut self, action: Action) {
        if matches!(action, Action::Back | Action::Forward) {
            self.seek_changed = Some(Instant::now());
        }
        let step = |session: &Option<Session>, by| {
            if let Some(session) = session {
                session.player.send(Command::SeekBy(by));
            }
        };
        match action {
            Action::Toggle => self.set_playing(!self.playing),
            Action::Back => step(&self.session, -SEEK_STEP),
            Action::Forward => step(&self.session, SEEK_STEP),
            Action::Louder => self.set_volume(self.state.volume.saturating_add(VOLUME_STEP).min(100)),
            Action::Quieter => self.set_volume(self.state.volume.saturating_sub(VOLUME_STEP)),
            Action::Quit => self.closed = true,
        }
    }

    pub(super) fn open(&mut self, path: PathBuf) -> Result<()> {
        if self.coming.is_some() {
            self.wanted = Some(path);
            return Ok(());
        }
        log::note("user.open", &format!("path={}", path.display()));
        let sound = match Opened::new(path, 0, self.state.volume, Some(&self.music)) {
            Ok(sound) => sound,
            Err(error) => {
                log::error("audio.failed", &error.to_string());
                self.problem = Problem::Unreadable;
                return Ok(());
            }
        };
        self.problem = Problem::None;
        self.seeking = None;
        if let Some(session) = &self.session {
            let coming = Coming { path: sound.path, title: sound.title.clone(), total: sound.total };
            if session.player.send(Command::Open { engine: Box::new(sound.engine), title: sound.title }) {
                self.coming = Some(coming);
            }
        } else {
            self.load(sound)?;
            self.send_title();
        }
        Ok(())
    }

    fn seek_to(&mut self, frame: u64) {
        let Some(session) = &self.session else { return };
        // During a switch the position belongs to the sound fading in.
        let total = self.coming.as_ref().map_or(session.total, |coming| coming.total);
        let frame = frame.min(total);
        session.player.send(Command::SeekTo(frame));
        self.seek_changed = Some(Instant::now());
        self.seeking = Some((frame, Instant::now() + SEEK_SETTLES));
    }

    /// Catches up with a switch the audio thread has made and returns the position, read in the
    /// same instant, so one sound's position is never saved under the other's name.
    fn adopt_switch(&mut self) -> u64 {
        let Some(session) = &mut self.session else { return self.state.position };
        let (track, position) = session.player.current();
        if track != self.track {
            self.track = track;
            if let Some(coming) = self.coming.take() {
                log::note("track.switched", &format!("path={}", coming.path.display()));
                session.title = coming.title;
                session.total = coming.total;
                self.state.track = Some(coming.path);
            }
        }
        position
    }

    fn position(&self) -> u64 {
        self.session.as_ref().map_or(self.state.position, |session| session.player.position())
    }

    fn refresh(&mut self) {
        let now = Instant::now();
        self.seeking = self.seeking.filter(|&(_, until)| now < until);
        let position = self.seeking.map_or_else(|| self.position(), |(frame, _)| frame);
        let total = self.session.as_ref().map_or(0, |session| session.total);
        let playback = match (self.session.is_some(), self.playing, self.reconnecting) {
            (false, _, _) => Playback::Empty,
            (true, false, _) => Playback::Paused,
            (true, true, false) => Playback::Playing,
            (true, true, true) => Playback::Waiting,
        };
        self.tell_remote(playback, position, total);
        match &mut self.front {
            Front::Window(window) => window.show(Status {
                playback,
                problem: self.problem,
                seconds: if self.session.is_some() { position / RATE } else { 0 },
                total: total / RATE,
                volume: self.state.volume,
                handled: self.handled,
            }),
            Front::Terminal(terminal) => {
                let label = match playback {
                    Playback::Playing => "Playing",
                    Playback::Waiting => "Reconnecting",
                    Playback::Paused | Playback::Empty => "Paused",
                };
                terminal.status(label, position, total, self.state.volume);
            }
        }
    }

    fn save(&mut self) {
        // With nothing open the saved place still belongs to the sound that was there before.
        self.state.position = self.adopt_switch();
        if self.saved.as_ref() == Some(&self.state) {
            return;
        }
        match self.store.save(&self.state) {
            Ok(()) => self.saved = Some(self.state.clone()),
            Err(error) => log::error("state.save_failed", &error.to_string()),
        }
    }

    fn finish(&mut self, why: &str) -> Result<()> {
        log::event("shutdown.start", &format!("reason={why}"));
        let clean = self.session.as_mut().is_none_or(|session| session.player.shutdown(SHUTDOWN_GRACE));
        self.save();
        if !clean {
            log::error("shutdown.forced", "the audio thread did not stop cleanly");
            return Err(Error::Output("audio stopped unexpectedly. Your place was saved".into()));
        }
        log::event("shutdown.done", &format!("at={}", clock(self.state.position)));
        Ok(())
    }
}

/// Shares one reporter with every window process the host starts.
pub fn window_reporter(sender: Sender<Message>) -> Arc<dyn Fn(FromWindow) + Send + Sync> {
    Arc::new(move |news| match news {
        FromWindow::Request { .. } => sender.send_or_wait(Message::Window(news)),
        FromWindow::Gone { .. } => sender.send(Message::Window(news)),
    })
}

fn clock(frames: u64) -> String {
    timecode::format(frames / RATE)
}
