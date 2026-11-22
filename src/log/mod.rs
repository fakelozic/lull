//! The one logger: JSON lines in `~/.local/state/lull/logs`, 7 days kept (docs/operations.md).
//!
//! Important events also go to stderr as `lull: <event> <detail>`, for the terminal and the journal.
//! It never panics and never stops the app: stderr or the disk can vanish and we must still save.
//! The audio thread never calls it; it reports events to main, which logs them.

mod files;
mod limit;
mod record;

use std::backtrace::Backtrace;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use files::Files;
use record::Writer;
pub use record::{KEEP_DAYS, Level};

struct Log {
    writer: Writer,
    files: Option<Files>,
    /// The window's stderr is captured by the sound process, so it only echoes if the file fails.
    echo: bool,
}

/// Lines that may wait for stderr. More are dropped: the file has them.
const ECHO_LINES: usize = 64;

enum Echo {
    Line(String),
    Flush(SyncSender<()>),
}

static ECHO: OnceLock<SyncSender<Echo>> = OnceLock::new();
static LIMIT: Mutex<limit::Limit> = Mutex::new(limit::Limit::new());
static LOG: OnceLock<Log> = OnceLock::new();
static PRUNED_ON: AtomicI64 = AtomicI64::new(-1);
static SAID_FULL: AtomicBool = AtomicBool::new(false);
static SAID_BROKEN: AtomicBool = AtomicBool::new(false);

/// Which process is logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Process {
    /// The main one.
    Sound,
    /// `lull --window`.
    Window,
}

/// Starts logging to files in `folder` and records any panic from now on. Call once, first thing.
/// If the folder can't be used, logging carries on to stderr alone.
pub fn start(process: Process, run: &str, folder: &Path, home: &Path) {
    let files = Files::open(folder);
    let problem = files.as_ref().err().map(ToString::to_string);
    let writer = Writer {
        process: if process == Process::Sound { "sound" } else { "window" },
        run: run.to_owned(),
        pid: std::process::id(),
        home: home.to_string_lossy().into_owned(),
    };
    let echo = process == Process::Sound || files.is_err();
    if LOG.set(Log { writer, files: files.ok(), echo }).is_err() {
        return;
    }
    start_echo();
    record_panics();
    if let Some(problem) = problem {
        warn("log.unavailable", &problem);
    }
}

/// A new run id: the start time in hex.
pub fn new_run() -> String {
    format!("{:x}", now())
}

/// A milestone, in the file and on stderr.
pub fn event(event: &str, detail: &str) {
    write(Level::Info, event, detail, true);
}

/// Detail for later (a press, a device name). File only, so the terminal stays readable.
pub fn note(event: &str, detail: &str) {
    write(Level::Info, event, detail, false);
}

/// Odd, but handled.
pub fn warn(event: &str, detail: &str) {
    write(Level::Warn, event, detail, true);
}

/// Something broke.
pub fn error(event: &str, detail: &str) {
    write(Level::Error, event, detail, true);
}

/// Milliseconds since 1970.
pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
}

fn write(level: Level, event: &str, detail: &str, show: bool) {
    let log = LOG.get();
    // The file first: stderr is the one that can be stuck.
    if let Some(log) = log
        && let Some(files) = &log.files
    {
        save(log, files, level, event, detail);
    }
    if show && log.is_none_or(|log| log.echo) {
        say(&format!("{event} {detail}"));
    }
}

fn save(log: &Log, files: &Files, level: Level, event: &str, detail: &str) {
    let millis = now();
    let day = (millis / 86_400_000) as i64;
    if PRUNED_ON.swap(day, Ordering::Relaxed) != day {
        SAID_FULL.store(false, Ordering::Relaxed);
        if let Err(problem) = files.prune(day) {
            say(&format!("log.prune_failed {problem}"));
        }
    }
    // try_lock: a panic inside here would log, and must not wait for itself.
    let dropped = match LIMIT.try_lock() {
        Ok(mut limit) if event != "process.panic" && !event.starts_with("shutdown.") => {
            match limit.check(event, millis) {
                Some(dropped) => dropped,
                None => return,
            }
        }
        _ => 0,
    };
    if dropped > 0 {
        let line = log.writer.line(millis, Level::Warn, "log.suppressed", &format!("event={event} lines={dropped}"));
        if let Err(problem) = files.append(day, &line, false) {
            say(&format!("log.write_failed {problem}"));
        }
    }
    let vital = level != Level::Info || event.starts_with("shutdown.") || event == "app.start";
    match files.append(day, &log.writer.line(millis, level, event, detail), vital) {
        Ok(true) => {}
        // Only the sound process says it: the window's stderr is logged, which would fill the reserve.
        Ok(false) if log.echo && !SAID_FULL.swap(true, Ordering::Relaxed) => say("log.full today's log is full"),
        Err(problem) if !SAID_BROKEN.swap(true, Ordering::Relaxed) => say(&format!("log.write_failed {problem}")),
        Ok(false) | Err(_) => {}
    }
}

/// stderr gets its own thread: a terminal paused with Ctrl+S, or a pipe nobody reads, blocks
/// whoever writes to it, and that must never be main or audio.
fn start_echo() {
    let (lines, waiting) = mpsc::sync_channel(ECHO_LINES);
    let thread = std::thread::Builder::new().name("echo".into()).stack_size(128 * 1024).spawn(move || {
        for echo in waiting {
            match echo {
                Echo::Line(line) => print_line(&line),
                // Only fails if flush() gave up waiting.
                Echo::Flush(done) => drop(done.try_send(())),
            }
        }
    });
    if thread.is_ok() {
        // Only set once: start() returns early on a second call.
        let _ = ECHO.set(lines);
    }
}

/// Gives stderr a moment to catch up. Call last thing before exiting.
pub fn flush() {
    let Some(echo) = ECHO.get() else { return };
    let (done, wait) = mpsc::sync_channel(1);
    if echo.try_send(Echo::Flush(done)).is_ok() {
        // A stuck stderr only costs this wait.
        let _ = wait.recv_timeout(Duration::from_millis(300));
    }
}

fn say(text: &str) {
    match ECHO.get() {
        // A full queue means stderr is stuck. The file has the line.
        Some(echo) => drop(echo.try_send(Echo::Line(text.to_owned()))),
        None => print_line(text),
    }
}

fn print_line(text: &str) {
    let mut err = std::io::stderr();
    let clear = if err.is_terminal() { "\r\x1b[K" } else { "" };
    // Nowhere left to report a failure to write the log itself.
    let _ = writeln!(err, "{clear}lull: {text}");
}

fn record_panics() {
    std::panic::set_hook(Box::new(|info| {
        let message = info.payload_as_str().unwrap_or("(no message)");
        let place = info.location().map_or_else(|| "an unknown place".to_owned(), ToString::to_string);
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("unnamed");
        let stack = Backtrace::force_capture();
        error("process.panic", &format!("thread={thread} at={place} message={message}\n{stack}"));
    }));
}
