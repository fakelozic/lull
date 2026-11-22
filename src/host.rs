//! Starts the window process and carries messages to and from it.
//! Writing happens on its own thread, so a stuck window can never hold up the sound process.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, SyncSender};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::link::{MAX_LINE, Request, ToWindow};
use crate::log;

/// Messages that may wait for a slow window. Two seconds of levels.
const QUEUE: usize = 64;
/// Lines of the window's stderr logged per window. The rest is read and dropped.
const STDERR_LINES: usize = 100;
/// The largest display scale passed on. Memory grows with its square: 6x would be 53 MB of pixels.
const MOST_SCALE: f32 = 3.0;

/// What the window's reader reports.
#[derive(Debug, Clone, Copy)]
pub enum FromWindow {
    /// A numbered request.
    Request {
        /// Which window process sent it.
        window: u64,
        /// The request's number.
        number: u64,
        /// What it asks.
        request: Request,
    },
    /// That window process is gone.
    Gone {
        /// Which one.
        window: u64,
    },
}

/// One running window process. Dropping it closes it.
pub struct Window {
    /// Which process this is, so late news from an old one can be ignored.
    pub id: u64,
    child: Child,
    socket: UnixStream,
    outbox: SyncSender<String>,
}

impl Window {
    /// Starts window number `id`. `report` runs on the reader thread.
    pub fn start(
        id: u64,
        run: &str,
        token: Option<&str>,
        report: Arc<dyn Fn(FromWindow) + Send + Sync>,
    ) -> Result<Self> {
        let failed = |e: std::io::Error| Error::Window(e.to_string());
        let (socket, theirs) = UnixStream::pair().map_err(failed)?;
        let mut command = Command::new(this_program()?);
        // A token works once, so never pass on the one Lull itself was started with.
        command.env_remove("XDG_ACTIVATION_TOKEN").env_remove("DESKTOP_STARTUP_ID");
        if let Some(token) = token {
            command.env("XDG_ACTIVATION_TOKEN", token);
        }
        let asked = log::now().to_string();
        if let Some(scale) = capped_scale() {
            command.env("SLINT_SCALE_FACTOR", scale.to_string());
        }
        let mut child = command
            .arg0("lull")
            .arg("--window")
            .arg(run)
            .arg(asked)
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            // Ctrl+C in a terminal goes to Lull, which fades out and then closes the window.
            .process_group(0)
            .spawn()
            .map_err(failed)?;
        // Toolkit errors and abort messages only ever appear on stderr.
        if let Some(stderr) = child.stderr.take() {
            std::thread::Builder::new()
                .name("window-errors".into())
                .spawn(move || record_stderr(stderr))
                .map_err(failed)?;
        }
        log::note("window.starting", &format!("window={id} pid={}", child.id()));
        let (outbox, waiting) = mpsc::sync_channel::<String>(QUEUE);
        let window = Self { id, child, socket, outbox };
        let mut writer = window.socket.try_clone().map_err(failed)?;
        writer.set_write_timeout(Some(Duration::from_secs(2))).map_err(failed)?;
        std::thread::Builder::new()
            .name("to-window".into())
            .spawn(move || {
                for line in waiting {
                    if writer.write_all(line.as_bytes()).is_err() {
                        // A broken or stuck window. The reader reports it once this closes.
                        let _ = writer.shutdown(Shutdown::Both);
                        return;
                    }
                }
            })
            .map_err(failed)?;
        let reader = window.socket.try_clone().map_err(failed)?;
        std::thread::Builder::new()
            .name("from-window".into())
            .spawn(move || read_requests(id, reader, &*report))
            .map_err(failed)?;
        Ok(window)
    }

    /// The window's process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// How a window that went away ended, waiting briefly for it to finish.
    pub fn ending(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status.to_string(),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Ok(None) => return "closed its connection but is still running".to_owned(),
                Err(error) => return error.to_string(),
            }
        }
    }

    /// Closes it without making the caller wait: it normally exits in 30 ms, but a frozen one takes a second.
    pub fn retire(self) {
        // If no thread can be started the closure is dropped here, which closes it all the same.
        let _ = std::thread::Builder::new().name("window-retire".into()).spawn(move || drop(self));
    }

    /// Queues several messages as one write, so the window wakes once. False if it is too far behind.
    pub fn send_batch(&self, messages: &[ToWindow]) -> bool {
        self.outbox.try_send(messages.iter().map(ToWindow::encode).collect()).is_ok()
    }

    /// Queues a message. False if the window is too far behind to take it.
    pub fn send(&self, message: &ToWindow) -> bool {
        self.outbox.try_send(message.encode()).is_ok()
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // Closing the socket is what tells the window to exit.
        let _ = self.socket.shutdown(Shutdown::Both);
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        log::warn("window.killed", &format!("window={} did not exit by itself", self.id));
        // It didn't go by itself. Both calls fail only if it has exited since.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn capped_scale() -> Option<f32> {
    let asked: f32 = std::env::var("SLINT_SCALE_FACTOR").ok()?.parse().ok()?;
    (!asked.is_finite() || asked > MOST_SCALE).then_some(MOST_SCALE)
}

fn this_program() -> Result<PathBuf> {
    let path = std::env::current_exe().map_err(|e| Error::Window(e.to_string()))?;
    let same_file = |other: &PathBuf| {
        let ours = std::fs::metadata("/proc/self/exe").ok()?;
        let theirs = std::fs::metadata(other).ok()?;
        Some((ours.dev(), ours.ino()) == (theirs.dev(), theirs.ino()))
    };
    // After a rebuild the path is a newer Lull that may speak another protocol. Our own image always fits.
    Ok(if same_file(&path) == Some(true) { path } else { PathBuf::from("/proc/self/exe") })
}

fn record_stderr(stderr: ChildStderr) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    let mut seen = 0;
    loop {
        line.clear();
        match (&mut reader).take(4096).read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) if seen < STDERR_LINES => {
                seen += 1;
                let text = String::from_utf8_lossy(&line);
                let text: String = text.trim_end().chars().filter(|c| !c.is_control()).collect();
                log::warn("window.stderr", &text);
            }
            Ok(_) => {} // Keep draining, or the window would block on a full pipe.
        }
    }
}

fn read_requests(window: u64, socket: UnixStream, report: &(dyn Fn(FromWindow) + Send + Sync)) {
    let mut reader = BufReader::new(socket);
    let mut line = String::new();
    loop {
        line.clear();
        match (&mut reader).take(MAX_LINE as u64).read_line(&mut line) {
            Ok(read) if read > 0 && line.ends_with('\n') => {}
            _ => break,
        }
        if let Some((number, request)) = Request::decode(line.trim_end_matches('\n')) {
            report(FromWindow::Request { window, number, request });
        }
    }
    report(FromWindow::Gone { window });
}
