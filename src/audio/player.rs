//! The audio thread. It decodes and writes, nothing else: no disk writes, no logging, no UI.
//! While paused it holds no stream and sleeps until a command arrives.
//! If the sound server goes away it retries with backoff and resumes at the same place.

use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::engine::{Engine, Source};
use super::spectrum::Spectrum;
use crate::error::{Error, Result};
use crate::ffi::Output;
use crate::timecode::RATE;
use crate::visual::Levels;

/// Audio kept queued at the sound server (300 ms).
pub const BUFFER_FRAMES: u32 = 14_400;
/// Silence sent before closing, so the buffered fade is heard in full (500 ms).
const TAIL_FRAMES: u64 = 24_000;
/// Most frames written at once (40 ms).
const CHUNK: usize = 1_920;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Longest sleep while streaming, which bounds how late a command is noticed.
const WAIT: Duration = Duration::from_millis(40);
/// A stream that takes no audio for this long is replaced.
const STALL: Duration = Duration::from_secs(4);
/// Commands that may wait. Key repeat is 30 a second, so this is far more than anyone can type.
const MAILBOX: usize = 1_024;
/// Commands handled between two fills, so a seek storm can't starve the speakers.
const COMMANDS_PER_FILL: usize = 16;
/// The position shares an atomic with the number of the sound it belongs to, so they can't be read
/// mismatched. 48 bits of frames is 185 years.
const TRACK_SHIFT: u32 = 48;
/// Audio accepted before the retry delay resets.
const HEALTHY_FRAMES: u64 = 48_000;

/// What the rest of the app may ask of the audio thread.
pub enum Command<S> {
    /// Fade in and play.
    Play,
    /// Fade out, then release the device.
    Pause,
    /// Move by this many frames.
    SeekBy(i64),
    /// Move to this frame.
    SeekTo(u64),
    /// Set the 0-100 volume.
    SetVolume(u8),
    /// Report levels, or stop: only the window wants them.
    Watch(bool),
    /// Fade out, swap to this sound and carry on as before.
    Open {
        /// The new sound, ready to play.
        engine: Box<Engine<S>>,
        /// Its name, for the system mixer.
        title: String,
    },
    /// Fade out, close and end the thread.
    Shutdown,
}

/// What the audio thread reports.
#[derive(Debug, Clone)]
pub enum Event {
    /// A stream opened.
    Connected,
    /// The stream failed. A retry follows after `retry`.
    Lost {
        /// Why it failed.
        error: Error,
        /// The wait before the next attempt.
        retry: Duration,
    },
    /// The stream is on this device (at connect, and whenever the system moves it).
    Device(String),
    /// The fade out was heard and the stream is closed. Position is now final.
    Paused,
    /// The sound can't be read. Playback stopped.
    Failed(Error),
    /// The sound asked for with `Open` is now the one playing.
    Opened,
    /// What the ring shows, oldest first, each with when it leaves the speakers.
    /// One event per buffer fill, so main wakes a few times a second, not thirty.
    Levels(Vec<(Instant, Levels)>),
    /// The buffer ran empty, with the running count for this stream.
    Underrun(u64),
}

/// The handle main holds: send commands, read the position, shut down.
pub struct Player<S> {
    commands: SyncSender<Command<S>>,
    position: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl<S: Source + Send + 'static> Player<S> {
    /// Starts the thread. `report` runs on it, so it must never block.
    pub fn start(engine: Engine<S>, title: String, report: impl Fn(Event) + Send + 'static) -> std::io::Result<Self> {
        let (commands, inbox) = mpsc::sync_channel(MAILBOX);
        let position = Arc::new(AtomicU64::new(engine.position()));
        let shared = Arc::clone(&position);
        let thread = std::thread::Builder::new().name("audio".into()).spawn(move || {
            let worker = Worker {
                engine,
                title,
                inbox,
                report,
                position: shared,
                stream: None,
                retry_at: Instant::now(),
                failures: 0,
                quitting: false,
                samples: vec![0.0; CHUNK * 2],
                spectrum: Spectrum::new(),
                watched: false,
                next: None,
                track: 0,
            };
            worker.run();
        })?;
        Ok(Self { commands, position, thread: Some(thread) })
    }

    /// Sends a command without ever waiting. False if it could not be taken: the mailbox is full
    /// (the sound server is stuck) or the thread has died, which `is_running` reports.
    pub fn send(&self, command: Command<S>) -> bool {
        self.commands.try_send(command).is_ok()
    }

    /// The position in frames, updated as audio is written.
    pub fn position(&self) -> u64 {
        self.current().1
    }

    /// Which sound is playing (0 for the first, one more for each `Opened`) and its position.
    /// Read together, so a position is never saved under another sound's name.
    pub fn current(&self) -> (u64, u64) {
        let value = self.position.load(Ordering::Acquire);
        (value >> TRACK_SHIFT, value & ((1 << TRACK_SHIFT) - 1))
    }

    /// False if the audio thread ended.
    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|thread| !thread.is_finished())
    }

    /// Fades out and waits up to `grace`. Returns false if it had to give up or the thread crashed.
    pub fn shutdown(&mut self, grace: Duration) -> bool {
        let Some(thread) = self.thread.take() else { return true };
        let deadline = Instant::now() + grace;
        let mut told = false;
        while !thread.is_finished() {
            told = told || self.send(Command::Shutdown);
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        thread.join().is_ok()
    }
}

struct Stream {
    output: Output,
    written: u64,
    silence: u64,
    underflows: u64,
    /// What the server really queues.
    queue: u32,
    last_write: Instant,
    device: CString,
}

struct Next<S> {
    engine: Box<Engine<S>>,
    title: String,
    play: bool,
}

struct Worker<S, F> {
    engine: Engine<S>,
    title: String,
    inbox: Receiver<Command<S>>,
    report: F,
    position: Arc<AtomicU64>,
    stream: Option<Stream>,
    retry_at: Instant,
    failures: u32,
    quitting: bool,
    samples: Vec<f32>,
    spectrum: Spectrum,
    watched: bool,
    /// Waiting for the fade out before it takes over.
    next: Option<Next<S>>,
    /// Goes up at each switch.
    track: u64,
}

impl<S: Source, F: Fn(Event)> Worker<S, F> {
    fn run(mut self) {
        loop {
            if self.stream.is_some() {
                self.take_commands();
                self.stream_once();
            } else if self.quitting {
                break;
            } else {
                self.without_stream();
            }
            self.publish();
        }
    }

    fn publish(&self) {
        self.position.store(self.track << TRACK_SHIFT | self.engine.position(), Ordering::Release);
    }

    fn take_commands(&mut self) {
        for _ in 0..COMMANDS_PER_FILL {
            match self.inbox.try_recv() {
                Ok(command) => self.apply(command),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => return self.apply(Command::Shutdown),
            }
        }
    }

    fn apply(&mut self, command: Command<S>) {
        match command {
            Command::Play if !self.quitting => match &mut self.next {
                Some(next) => next.play = true,
                None => self.engine.set_playing(true),
            },
            Command::Play => {}
            Command::Pause => {
                self.engine.set_playing(false);
                if let Some(next) = &mut self.next {
                    next.play = false;
                }
            }
            Command::SetVolume(percent) => {
                self.engine.set_volume(percent);
                if let Some(next) = &mut self.next {
                    next.engine.set_volume(percent);
                }
            }
            Command::SeekBy(frames) => self.seek(|engine| engine.seek_by(frames)),
            Command::SeekTo(frame) => self.seek(|engine| engine.seek_to(frame)),
            Command::Watch(watched) => self.watched = watched,
            Command::Open { engine, title } => {
                let play = self.next.as_ref().map_or(self.engine.is_playing(), |next| next.play);
                self.engine.set_playing(false);
                self.next = Some(Next { engine, title, play });
                self.take_over_if_silent();
            }
            Command::Shutdown => {
                self.quitting = true;
                self.engine.set_playing(false);
            }
        }
    }

    fn seek(&mut self, go: impl Fn(&mut Engine<S>) -> Result<()>) {
        match &mut self.next {
            Some(next) => {
                // The sound fading out is not to blame. The new one tries again when it takes over.
                if go(&mut next.engine).is_err() {
                    next.engine.halt();
                }
            }
            None => {
                if let Err(error) = go(&mut self.engine) {
                    self.fail(error);
                }
            }
        }
    }

    fn take_over_if_silent(&mut self) {
        if self.quitting || !self.engine.is_silent() {
            return;
        }
        let Some(next) = self.next.take() else { return };
        self.engine = *next.engine;
        self.title = next.title;
        self.engine.set_playing(next.play);
        self.track += 1;
        self.publish();
        (self.report)(Event::Opened);
    }

    fn without_stream(&mut self) {
        let received = if self.engine.is_playing() {
            let wait = self.retry_at.saturating_duration_since(Instant::now());
            if wait.is_zero() {
                // Act on a queued pause or quit before spending time connecting.
                self.take_commands();
                if self.engine.is_playing() {
                    self.connect();
                }
                return;
            }
            self.inbox.recv_timeout(wait)
        } else {
            self.inbox.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match received {
            Ok(command) => {
                // A fresh press of play shouldn't sit out an old retry delay.
                if matches!(command, Command::Play) && !self.engine.is_playing() {
                    self.retry_at = Instant::now();
                    self.failures = 0;
                }
                let pausing = matches!(command, Command::Pause);
                self.apply(command);
                // Without a stream there is no fade to wait for.
                if pausing && !self.engine.is_playing() {
                    self.report_paused();
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => self.apply(Command::Shutdown),
        }
    }

    fn connect(&mut self) {
        match Output::connect(&self.title, BUFFER_FRAMES, CONNECT_TIMEOUT) {
            Ok(output) => {
                self.stream = Some(Stream {
                    queue: output.queue_frames().unwrap_or(BUFFER_FRAMES),
                    output,
                    written: 0,
                    silence: 0,
                    underflows: 0,
                    last_write: Instant::now(),
                    device: CString::default(),
                });
                (self.report)(Event::Connected);
            }
            Err(error) => self.lost(error, 0),
        }
    }

    fn stream_once(&mut self) {
        if let Err(error) = self.pump() {
            let unheard = self.stream.take().map_or(0, |stream| stream.written.min(stream.queue.into()));
            self.lost(error, unheard);
        }
    }

    /// Fills what the server asks for, then sleeps until it wants more.
    fn pump(&mut self) -> Result<()> {
        self.take_over_if_silent();
        let Some(stream) = &mut self.stream else { return Ok(()) };
        let queue = u64::from(stream.queue);
        let mut wanted = stream.output.writable()?;
        // Once this fill is written the queue is full again, which dates every frame in it.
        let (asked, queued_at, total_asked) = (wanted, Instant::now(), wanted as u64);
        let mut heard = Vec::new();
        while wanted > 0 {
            let count = wanted.min(CHUNK);
            let samples = &mut self.samples[..count * 2];
            let was_silent = self.engine.is_silent();
            let (watched, spectrum, heard) = (self.watched, &mut self.spectrum, &mut heard);
            let written = (asked - wanted) as u64;
            let rendered = self.engine.render_with_tap(samples, |offset, sound| {
                if !watched {
                    return;
                }
                spectrum.push(sound, |frame, levels| {
                    let after = total_asked - (written + (offset + frame) as u64);
                    let wait = queue.saturating_sub(after);
                    let heard_at = queued_at + Duration::from_micros(wait * 1_000_000 / RATE);
                    heard.push((heard_at, levels));
                });
            });
            if let Err(error) = rendered {
                self.engine.halt();
                (self.report)(Event::Failed(error));
            }
            stream.output.write(samples)?;
            stream.written += count as u64;
            stream.silence = if was_silent { stream.silence + count as u64 } else { 0 };
            stream.last_write = Instant::now();
            wanted -= count;
        }
        if !heard.is_empty() {
            (self.report)(Event::Levels(heard));
        }
        if stream.written >= HEALTHY_FRAMES {
            self.failures = 0;
        }
        let finished = self.next.is_none() || self.quitting;
        // A larger queue than asked for needs a longer tail, or the end of the fade is never heard.
        if finished && self.engine.is_silent() && stream.silence >= TAIL_FRAMES.max(queue + TAIL_FRAMES / 2) {
            self.stream = None;
            self.report_paused();
            return Ok(());
        }
        if stream.last_write.elapsed() > STALL {
            return Err(Error::Output("the sound server stopped taking audio".into()));
        }
        stream.output.wait(WAIT)?;
        if let Some(device) = stream.output.device()
            && device != stream.device.as_c_str()
        {
            stream.device = device.to_owned();
            stream.queue = stream.output.queue_frames().unwrap_or(stream.queue);
            (self.report)(Event::Device(device.to_string_lossy().into_owned()));
        }
        let underflows = stream.output.underflows();
        if underflows > stream.underflows {
            stream.underflows = underflows;
            (self.report)(Event::Underrun(underflows));
        }
        Ok(())
    }

    fn lost(&mut self, error: Error, unheard: u64) {
        if let Err(track_error) = self.engine.recover(unheard) {
            self.fail(track_error);
        }
        // The old sound is cut to zero now, so a waiting one can take over.
        self.take_over_if_silent();
        if !self.engine.is_playing() {
            self.report_paused();
            return;
        }
        // 0.25, 0.5, 1, 2, 4, then every 5 s for as long as it takes.
        let retry = Duration::from_millis((250 << self.failures.min(5)).min(5_000));
        self.failures = self.failures.saturating_add(1);
        self.retry_at = Instant::now() + retry;
        (self.report)(Event::Lost { error, retry });
    }

    fn report_paused(&self) {
        // Publish first: main saves the position when it receives this.
        self.publish();
        (self.report)(Event::Paused);
    }

    fn fail(&mut self, error: Error) {
        self.engine.halt();
        (self.report)(Event::Failed(error));
    }
}
