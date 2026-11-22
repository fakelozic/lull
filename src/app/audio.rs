//! The app's side of the audio thread: what each piece of its news changes.

use std::time::Duration;

use super::{App, clock};
use crate::audio::Event;
use crate::error::Error;
use crate::link::Problem;
use crate::log;

impl App {
    pub(super) fn on_audio(&mut self, event: Event) {
        match event {
            Event::Connected => {
                let name = if self.reconnecting { "audio.restored" } else { "audio.connected" };
                log::event(name, &format!("at={}", clock(self.position())));
                self.reconnecting = false;
                self.last_loss = None;
                if matches!(self.problem, Problem::NoDevice | Problem::NoServer) {
                    self.problem = Problem::None;
                }
            }
            Event::Lost { error, retry } => self.on_lost(error, retry),
            Event::Paused => {
                log::event("audio.paused", &format!("at={}", clock(self.position())));
                self.save();
            }
            Event::Failed(error) => {
                self.playing = false;
                self.reconnecting = false;
                self.problem = Problem::Unreadable;
                log::error("audio.failed", &error.to_string());
            }
            Event::Opened => {
                self.adopt_switch();
                self.send_title();
                self.save();
                if let Some(path) = self.wanted.take()
                    && self.state.track.as_ref() != Some(&path)
                    && let Err(error) = self.open(path)
                {
                    log::error("audio.failed", &error.to_string());
                }
            }
            Event::Underrun(count) => self.on_underrun(count),
            Event::Device(name) => log::note("audio.device", &format!("name={name}")),
            Event::Levels(_) => return,
        }
        self.front.logged();
    }

    pub(super) fn on_lost(&mut self, error: Error, retry: Duration) {
        if !self.playing {
            return; // A retry that was already failing when pause was pressed.
        }
        self.reconnecting = true;
        self.problem = if error == Error::NoDevice { Problem::NoDevice } else { Problem::NoServer };
        // Retries settle at one every 5 s and can run all night, so log changes only.
        let loss = (error, retry);
        if self.last_loss.as_ref() != Some(&loss) {
            log::warn("audio.lost", &format!("{}. Trying again in {} ms", loss.0, retry.as_millis()));
            self.last_loss = Some(loss);
        }
    }
}
