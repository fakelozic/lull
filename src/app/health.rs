//! What the app writes to the log by itself: settled presses, underruns and the hourly health line.

use std::time::{Duration, Instant};

use super::{App, clock};
use crate::{log, runinfo};

/// The first health line comes after 5 minutes, then hourly.
pub(super) const FIRST_HEALTH: Duration = Duration::from_secs(300);
const HEALTH_EVERY: Duration = Duration::from_secs(3600);
/// A volume or seek drag is logged once it settles, so holding a key can't fill the log.
const VOLUME_SETTLES: Duration = Duration::from_secs(1);
/// Underruns come in storms, so they get at most a line a minute.
const UNDERRUN_EVERY: Duration = Duration::from_secs(60);

impl App {
    pub(super) fn on_underrun(&mut self, count: u64) {
        self.underruns += 1;
        if self.underrun_logged.is_none_or(|last| last.elapsed() >= UNDERRUN_EVERY) {
            self.underrun_logged = Some(Instant::now());
            log::warn("audio.underrun", &format!("count={count} this_run={}", self.underruns));
        }
    }

    pub(super) fn periodic_log(&mut self) {
        if self.volume_changed.is_some_and(|changed| changed.elapsed() >= VOLUME_SETTLES) {
            self.volume_changed = None;
            log::note("user.volume", &format!("percent={}", self.state.volume));
        }
        if self.seek_changed.is_some_and(|changed| changed.elapsed() >= VOLUME_SETTLES) {
            self.seek_changed = None;
            log::note("user.seek", &format!("to={}", clock(self.position())));
        }
        if Instant::now() < self.next_health {
            return;
        }
        self.next_health = Instant::now() + HEALTH_EVERY;
        let window_pid = self.front.window().and_then(|window| window.pid());
        let detail = format!(
            "up={}s playing={} waiting={} at={} underruns={} pss={}MB window_pss={}MB",
            self.started.elapsed().as_secs(),
            self.playing,
            self.reconnecting,
            clock(self.position()),
            self.underruns,
            runinfo::memory_mb(std::process::id()),
            window_pid.map_or(0, runinfo::memory_mb),
        );
        log::note("app.health", &detail);
    }
}
