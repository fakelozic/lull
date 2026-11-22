//! The app's side of the bus: answering it, and telling it what is true now.

use std::time::Instant;

use super::{App, JUMP, SHOW_GAP};
use crate::link::{Playback, Problem};
use crate::remote::{Ask, Launch, Snapshot};
use crate::timecode::RATE;
use crate::{log, track};

impl App {
    pub(super) fn on_ask(&mut self, ask: Ask) {
        match ask {
            Ask::Play => self.set_playing(true),
            Ask::Pause => self.set_playing(false),
            Ask::Toggle => self.set_playing(!self.playing),
            Ask::SeekBy(frames) => self.seek_to(self.heading_for().saturating_add_signed(frames)),
            Ask::SeekTo { track, frame } => {
                if track == self.track {
                    self.seek_to(frame);
                }
            }
            Ask::Volume(volume) => self.set_volume(volume.min(100)),
            Ask::VolumeBy(step) => {
                let volume = i16::from(self.state.volume).saturating_add(step).clamp(0, 100);
                self.set_volume(volume as u8);
            }
            Ask::Show { token } => self.show_from_outside(token.as_deref()),
            Ask::ShowOrHide { token } => {
                if self.window_just_changed() {
                    return;
                }
                self.window_changed = Some(Instant::now());
                let focused = self.front.window().is_some_and(|window| window.is_focused());
                log::note("tray.clicked", &format!("focused={focused} token={}", token.is_some()));
                if focused { self.put_away() } else { self.bring_up(token.as_deref()) }
            }
            Ask::Launch(launch) => self.on_launch(launch),
            Ask::Open(path) => {
                if self.open_from_outside(path) {
                    self.set_playing(true);
                }
            }
            Ask::Quit => self.closed = true,
        }
    }

    fn on_launch(&mut self, launch: Launch) {
        log::note("app.launched_again", &format!("play={} at={:?} file={:?}", launch.play, launch.at, launch.file));
        // The window first: it is what the person is waiting to see.
        self.show_from_outside(launch.token.as_deref());
        if let Some(path) = launch.file
            && !self.open_from_outside(path)
        {
            return;
        }
        if let Some(seconds) = launch.at {
            self.seek_to(seconds.saturating_mul(RATE));
        }
        if launch.play {
            self.set_playing(true);
        }
    }

    fn window_just_changed(&self) -> bool {
        self.window_changed.is_some_and(|at| at.elapsed() < SHOW_GAP)
    }

    fn show_from_outside(&mut self, token: Option<&str>) {
        // A hidden window always opens at once. Only replacing one that is already up is spaced out.
        let up = self.front.window().is_some_and(|window| window.is_up());
        if up && self.window_just_changed() {
            return;
        }
        self.window_changed = Some(Instant::now());
        self.bring_up(token);
    }

    /// True if that sound is now open or on its way.
    fn open_from_outside(&mut self, path: std::path::PathBuf) -> bool {
        if self.session.is_some() && self.coming.is_none() && self.state.track.as_ref() == Some(&path) {
            return true;
        }
        match self.open(path) {
            Ok(()) => self.problem != Problem::Unreadable,
            Err(error) => {
                log::error("audio.failed", &error.to_string());
                false
            }
        }
    }

    /// Where a seek that is still under way is going, else the position.
    fn heading_for(&self) -> u64 {
        match self.seeking {
            Some((frame, until)) if Instant::now() < until => frame,
            _ => self.position(),
        }
    }

    pub(super) fn tell_remote(&mut self, playback: Playback, position: u64, total: u64) {
        let Some(remote) = &self.remote else { return };
        let (title, detail) = self.session.as_ref().map(|session| track::headline(&session.title)).unwrap_or_default();
        let now = Instant::now();
        let jumped = self.told.is_some_and(|(track, before, at, was_playing)| {
            let played = if was_playing { (now - at).as_secs_f64() * RATE as f64 } else { 0.0 };
            track == self.track && (position as f64 - before as f64 - played).abs() > JUMP as f64
        });
        self.told = Some((self.track, position, now, playback == Playback::Playing));
        let problem = self.problem;
        let (track, volume) = (self.track, self.state.volume);
        remote.update(Snapshot { playback, problem, title, detail, track, position, total, volume }, jumped);
    }
}
