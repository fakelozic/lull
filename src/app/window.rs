//! The app's side of the window: answering it, showing and hiding it, and surviving its crashes.

use std::time::Instant;

use super::{App, HINT_MARKER};
use crate::audio::Command;
use crate::error::{Error, Result};
use crate::front::{After, Front};
use crate::host::FromWindow;
use crate::link::{Request, ToWindow};
use crate::timecode::RATE;
use crate::{log, sounds};

impl App {
    pub(super) fn on_window(&mut self, news: FromWindow) -> Result<()> {
        let Some(windowed) = self.front.window() else { return Ok(()) };
        match news {
            FromWindow::Request { window, number, request } if windowed.is_current(window) => {
                if request == Request::Hello {
                    windowed.opened();
                    log::event("window.opened", "");
                }
                self.handled = number;
                self.answer(request)
            }
            FromWindow::Gone { window } if windowed.is_current(window) => self.window_gone(),
            _ => Ok(()),
        }
    }

    pub(super) fn window_gone(&mut self) -> Result<()> {
        let Some(windowed) = self.front.window() else { return Ok(()) };
        let after = windowed.crashed();
        self.handled = 0;
        self.watch(false);
        if after == After::Restarted {
            log::warn("window.restarted", "it closed unexpectedly");
        } else if self.has_tray() {
            log::error("window.gave_up", "still in the tray. A click there tries again");
        } else if self.playing {
            // A UI failure must never stop the sound.
            self.windowless = true;
            log::error("window.gave_up", "still playing. `pkill -TERM -x lull` fades out and quits");
        } else {
            let why = if after == After::NeverOpened { "it would not open" } else { "it kept crashing" };
            return Err(Error::Window(format!("{why}. lull --no-window still works")));
        }
        Ok(())
    }

    pub(super) fn answer(&mut self, request: Request) -> Result<()> {
        match request {
            // The window follows with Watch, once it knows whether it animates at all.
            Request::Hello => self.send_title(),
            Request::SetPlaying(playing) => self.set_playing(playing),
            Request::Volume(volume) => self.set_volume(volume),
            Request::Watch(watched) => self.watch(watched),
            Request::Closed => self.put_away(),
            Request::Quit => self.closed = true,
            Request::Focused(focused) => {
                if let Some(window) = self.front.window() {
                    window.set_focused(focused);
                }
            }
            Request::Seek(seconds) => self.seek_to(seconds.saturating_mul(RATE)),
            Request::List => self.send_list(),
            Request::Open { list, row } => {
                let found = self.lists.iter().find(|(number, _)| *number == list).and_then(|(_, files)| files.get(row));
                if let Some(path) = found.cloned() {
                    return self.open(path);
                }
            }
        }
        Ok(())
    }

    pub(super) fn send_list(&mut self) {
        let (rows, files) = sounds::catalogue(&self.music, self.state.track.as_deref());
        let list = self.next_list;
        self.next_list += 1;
        self.lists.push_back((list, files));
        if self.lists.len() > 2 {
            self.lists.pop_front();
        }
        if let Some(window) = self.front.window() {
            window.send(ToWindow::Sounds { list, rows });
        }
    }

    pub(super) fn watch(&mut self, watched: bool) {
        self.watched = watched;
        if let Some(session) = &self.session {
            session.player.send(Command::Watch(watched));
        }
    }

    pub(super) fn send_title(&mut self) {
        if let (Some(session), Front::Window(window)) = (&self.session, &mut self.front) {
            window.send(ToWindow::Title(session.title.clone()));
        }
    }

    pub(super) fn forward_levels(&self, heard: &[(Instant, crate::visual::Levels)]) {
        let Front::Window(window) = &self.front else { return };
        let now = Instant::now();
        let batch: Vec<ToWindow> = heard
            .iter()
            .map(|&(at, levels)| {
                let delay_ms = at.saturating_duration_since(now).as_millis().min(1000) as u16;
                ToWindow::Levels { delay_ms, levels }
            })
            .collect();
        window.send_levels(&batch);
    }

    /// Brings the window up, with focus if the desktop gave permission.
    pub(super) fn bring_up(&mut self, token: Option<&str>) {
        let Some(window) = self.front.window() else { return };
        match window.bring_up(token) {
            Ok(()) => {
                self.windowless = false;
                self.handled = 0;
                self.send_title();
                self.refresh();
            }
            Err(error) => log::error("window.start_failed", &error.to_string()),
        }
    }

    /// The window was closed, or hidden from the tray. Without a tray there would be no way back, so quit.
    pub(super) fn put_away(&mut self) {
        if !self.has_tray() {
            self.closed = true;
            return;
        }
        if let Some(window) = self.front.window() {
            window.put_away();
        }
        self.handled = 0;
        self.watch(false);
        log::note("window.hidden", "to the tray");
        self.save();
        if self.hint_due {
            self.hint_due = false;
            if let Some(remote) = &self.remote {
                remote.say_still_here();
            }
            if let Err(error) = crate::private_file::replace(&self.folder.join(HINT_MARKER), b"") {
                log::warn("tray.notice_not_saved", &error.to_string());
            }
        }
    }

    pub(super) fn has_tray(&self) -> bool {
        self.remote.as_ref().is_some_and(|remote| remote.has_tray())
    }
}
