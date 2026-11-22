//! What the person controls Lull with: the terminal, or the window process and its restarts.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::host::{FromWindow, Window};
use crate::link::{Status, ToWindow};
use crate::log;
use crate::terminal::Terminal;

/// Restarts allowed inside `RESTART_WINDOW`.
const RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(60);
/// A window that hasn't said hello by now is stuck.
const STARTUP_LIMIT: Duration = Duration::from_secs(15);
/// Trays that take focus on mouse-down do it just before they report the click, so focus lost this
/// recently still means "the window was in front when the person clicked".
const FOCUS_GRACE: Duration = Duration::from_millis(400);

/// What became of a window that went away.
#[derive(Debug, PartialEq, Eq)]
pub enum After {
    /// Another is starting.
    Restarted,
    /// Too many crashes. The sound plays on.
    GaveUp,
    /// It never managed to open, so there is no way to control Lull.
    NeverOpened,
}

/// The window, across restarts.
pub struct Windowed {
    report: Arc<dyn Fn(FromWindow) + Send + Sync>,
    run: String,
    window: Option<Window>,
    /// The one being replaced, kept on screen until the new one is up, so raising never blinks.
    old: Option<Window>,
    crashes: Vec<Instant>,
    focused: bool,
    unfocused_at: Option<Instant>,
    next_id: u64,
    opened: bool,
    sent: Option<Status>,
    /// The running window is still to say hello, since this.
    starting: Option<Instant>,
    /// The latest title and list, kept until a full outbox has room. Losing one would lose it for good.
    title: Option<ToWindow>,
    list: Option<ToWindow>,
}

impl Windowed {
    /// Starts the first window.
    pub fn start(run: String, token: Option<&str>, report: Arc<dyn Fn(FromWindow) + Send + Sync>) -> Result<Self> {
        let mut this = Self {
            report,
            run,
            window: None,
            old: None,
            crashes: Vec::new(),
            focused: false,
            unfocused_at: None,
            next_id: 1,
            opened: false,
            sent: None,
            starting: None,
            title: None,
            list: None,
        };
        this.spawn(token)?;
        Ok(this)
    }

    fn spawn(&mut self, token: Option<&str>) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        self.sent = None;
        self.title = None;
        self.list = None;
        self.starting = Some(Instant::now());
        self.focused = false;
        self.unfocused_at = None;
        self.window = Some(Window::start(id, &self.run, token, Arc::clone(&self.report))?);
        Ok(())
    }

    /// True if `id` is the window that is running now.
    pub fn is_current(&self, id: u64) -> bool {
        self.window.as_ref().is_some_and(|window| window.id == id)
    }

    /// The running window's process id.
    pub fn pid(&self) -> Option<u32> {
        self.window.as_ref().map(Window::pid)
    }

    /// The window said hello: it is up and needs the full picture.
    pub fn opened(&mut self) {
        self.opened = true;
        self.starting = None;
        if let Some(old) = self.old.take() {
            old.retire();
        }
        self.sent = None;
    }

    /// Brings the window up, with focus if `token` allows it. Wayland can't raise an existing window
    /// for us, so one that is already open is replaced: the new one appears, then the old one goes.
    pub fn bring_up(&mut self, token: Option<&str>) -> Result<()> {
        if self.window.is_some() && self.starting.is_some() {
            return Ok(()); // One is a few milliseconds from appearing.
        }
        if let Some(older) = self.old.take() {
            older.retire();
        }
        let current = self.window.take();
        self.crashes.clear();
        match self.spawn(token) {
            Ok(()) => {
                self.old = current;
                Ok(())
            }
            Err(error) => {
                self.window = current;
                Err(error)
            }
        }
    }

    /// Takes the window away. All its memory comes back.
    pub fn put_away(&mut self) {
        for window in [self.window.take(), self.old.take()].into_iter().flatten() {
            window.retire();
        }
        self.starting = None;
        self.focused = false;
        self.unfocused_at = None;
    }

    /// Notes focus.
    pub fn set_focused(&mut self, focused: bool) {
        if self.focused && !focused {
            self.unfocused_at = Some(Instant::now());
        }
        self.focused = focused;
    }

    /// True if a window is up or on its way.
    pub fn is_up(&self) -> bool {
        self.window.is_some()
    }

    /// True if the window is up and has focus.
    pub fn is_focused(&self) -> bool {
        let just_lost = self.unfocused_at.is_some_and(|at| at.elapsed() < FOCUS_GRACE);
        self.window.is_some() && (self.focused || just_lost)
    }

    /// True if the running window has been starting for far too long.
    pub fn is_stuck_starting(&self) -> bool {
        self.window.is_some() && self.starting.is_some_and(|since| since.elapsed() > STARTUP_LIMIT)
    }

    /// Queues the title or the list, and keeps it until it has gone.
    pub fn send(&mut self, message: ToWindow) {
        match message {
            ToWindow::Title(_) => self.title = Some(message),
            _ => self.list = Some(message),
        }
        self.flush();
    }

    fn flush(&mut self) {
        let Some(window) = &self.window else { return };
        for waiting in [&mut self.title, &mut self.list] {
            if waiting.as_ref().is_some_and(|message| window.send(message)) {
                *waiting = None;
            }
        }
    }

    /// Sends levels. A late frame is worthless, so it is dropped quietly.
    pub fn send_levels(&self, batch: &[ToWindow]) {
        if let Some(window) = &self.window {
            window.send_batch(batch);
        }
    }

    /// Sends a snapshot, if it changed.
    pub fn show(&mut self, status: Status) {
        self.flush();
        if self.sent == Some(status) {
            return;
        }
        if let Some(window) = &self.window
            && window.send(&ToWindow::Status(status))
        {
            self.sent = Some(status);
        }
    }

    /// The window went away without being closed: start another, within reason.
    pub fn crashed(&mut self) -> After {
        self.focused = false;
        self.starting = None;
        if let Some(old) = self.old.take() {
            old.retire();
        }
        if let Some(mut window) = self.window.take() {
            let ending = window.ending();
            log::warn("window.crashed", &format!("window={} pid={} ending={ending}", window.id, window.pid()));
        }
        self.crashes.retain(|crash| crash.elapsed() < RESTART_WINDOW);
        self.crashes.push(Instant::now());
        if self.crashes.len() <= RESTARTS {
            match self.spawn(None) {
                Ok(()) => return After::Restarted,
                Err(error) => log::error("window.start_failed", &error.to_string()),
            }
        }
        if self.opened { After::GaveUp } else { After::NeverOpened }
    }
}

/// Either front end.
pub enum Front {
    /// `--no-window`.
    Terminal(Terminal),
    /// The normal way.
    Window(Box<Windowed>),
}

impl Front {
    /// The window, if that is the front end.
    pub fn window(&mut self) -> Option<&mut Windowed> {
        match self {
            Front::Window(window) => Some(window),
            Front::Terminal(_) => None,
        }
    }

    /// Called after a log line may have printed over the terminal's status.
    pub fn logged(&mut self) {
        if let Front::Terminal(terminal) = self {
            terminal.invalidate();
        }
    }
}
