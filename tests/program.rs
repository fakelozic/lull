//! The real program as a child process: signals, saving, the lock and failure paths.
//! Every child gets a private runtime folder and a dead PULSE_SERVER, so none can reach the speakers.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use lull::state::State;
use lull::timecode::RATE;

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str, with_track: bool) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("program-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for folder in ["home/Music", "run", "state"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        std::fs::set_permissions(root.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();
        if with_track {
            let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/noise.opus");
            std::fs::copy(fixture, root.join("home/Music/noise.opus")).unwrap();
        }
        Self { root }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lull"));
        command
            .arg("--no-window")
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin")
            .env("HOME", self.root.join("home"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("PULSE_SERVER", "unix:/nonexistent/pulse")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }

    fn start(&self, args: &[&str]) -> Running {
        Running::new(self.command(args))
    }

    fn saved(&self) -> State {
        State::decode(&std::fs::read(self.root.join("state/lull/state")).unwrap()).unwrap()
    }
}

struct Running {
    child: Child,
    lines: Receiver<String>,
}

impl Running {
    fn new(mut command: Command) -> Self {
        let mut child = command.spawn().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Running { child, lines }
    }

    fn expect(&self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while let Ok(line) = self.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            if line.contains(text) {
                return;
            }
        }
        panic!("never logged {text}");
    }

    fn press(&mut self, keys: &[u8]) {
        self.child.stdin.as_mut().unwrap().write_all(keys).unwrap();
    }

    fn signal(&self, name: &str) {
        let sent = Command::new("/usr/bin/kill").args([&format!("-{name}"), &self.child.id().to_string()]).status();
        assert!(sent.unwrap().success());
    }

    fn exit_code(&mut self) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(7);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code().unwrap_or(-1);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("did not exit");
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn each_stop_signal_saves_and_exits_cleanly() {
    for signal in ["TERM", "INT", "HUP"] {
        let sandbox = Sandbox::new(signal, true);
        let mut app = sandbox.start(&["--at", "0:07"]);
        app.expect("app.ready");
        app.signal(signal);
        app.expect("shutdown.start");
        app.expect("shutdown.done");
        assert_eq!(app.exit_code(), 0);
        assert_eq!(sandbox.saved().position, 7 * RATE);
    }
}

#[test]
fn restores_the_place_and_volume() {
    let sandbox = Sandbox::new("restore", true);
    let mut app = sandbox.start(&["--at", "0:03"]);
    app.expect("app.ready");
    app.press(b"\x1b[A\x1b[A\x1b[Bq");
    assert_eq!(app.exit_code(), 0);
    let mut again = sandbox.start(&[]);
    again.expect("app.ready at=0:03");
    again.press(b"q");
    assert_eq!(again.exit_code(), 0);
    let saved = sandbox.saved();
    assert_eq!((saved.position, saved.volume), (3 * RATE, 25));
}

#[test]
fn kill_9_leaves_the_last_save_intact() {
    let sandbox = Sandbox::new("kill", true);
    let mut first = sandbox.start(&["--at", "0:05"]);
    first.expect("app.ready");
    first.press(b"q");
    assert_eq!(first.exit_code(), 0);
    let mut second = sandbox.start(&[]);
    second.expect("app.ready at=0:05");
    second.child.kill().unwrap();
    second.child.wait().unwrap();
    sandbox.start(&[]).expect("app.ready at=0:05");
}

#[test]
fn without_a_sound_server_it_retries_and_still_quits_fast() {
    let sandbox = Sandbox::new("no-server", true);
    let mut app = sandbox.start(&["--play", "--at", "0:04"]);
    app.expect("Trying again in 250 ms");
    app.expect("Trying again in 1000 ms");
    let before = Instant::now();
    app.press(b"q");
    assert_eq!(app.exit_code(), 0);
    assert!(before.elapsed() < Duration::from_millis(700));
    assert_eq!(sandbox.saved().position, 4 * RATE, "must not advance when nothing is heard");
}

#[test]
fn pause_stops_the_retries() {
    let sandbox = Sandbox::new("pause-retry", true);
    // A sound server that hangs up at once. Counting its callers is the only honest proof: main hides
    // "lost" lines while paused, so their absence would prove nothing.
    let socket = sandbox.root.join("run/hangup");
    let server = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = std::sync::Arc::clone(&calls);
    std::thread::spawn(move || {
        for caller in server.incoming() {
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            drop(caller);
        }
    });
    let mut command = sandbox.command(&["--play"]);
    command.env("PULSE_SERVER", format!("unix:{}", socket.display()));
    let mut app = Running::new(command);
    app.expect("Trying again in 500 ms");
    app.press(b" ");
    app.expect("audio.paused");
    std::thread::sleep(Duration::from_millis(300)); // An attempt already under way may still land.
    let paused = calls.load(std::sync::atomic::Ordering::Relaxed);
    assert!(paused >= 1, "never reached the fake server, so this would prove nothing");
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), paused, "kept connecting while paused");
}

#[test]
fn a_pipe_in_place_of_the_state_cannot_hang_startup() {
    let sandbox = Sandbox::new("pipe-state", true);
    std::fs::create_dir_all(sandbox.root.join("state/lull")).unwrap();
    for name in ["state", "running"] {
        let made = Command::new("/usr/bin/mkfifo").arg(sandbox.root.join("state/lull").join(name)).status();
        assert!(made.unwrap().success());
    }
    let mut app = sandbox.start(&[]);
    app.expect("app.ready");
    app.press(b"q");
    assert_eq!(app.exit_code(), 0);
}

impl Sandbox {
    fn log(&self) -> Vec<serde_json::Value> {
        let mut lines = Vec::new();
        for entry in std::fs::read_dir(self.root.join("state/lull/logs")).unwrap() {
            let path = entry.unwrap().path();
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            for line in std::fs::read_to_string(&path).unwrap().lines() {
                lines.push(serde_json::from_str(line).expect("every log line is JSON"));
            }
        }
        lines
    }
}

#[test]
fn the_log_says_what_ran_and_what_stopped_it() {
    let sandbox = Sandbox::new("log", true);
    let mut app = sandbox.start(&["--at", "0:06"]);
    app.expect("app.ready");
    app.signal("TERM");
    assert_eq!(app.exit_code(), 0);
    let log = sandbox.log();
    let detail = |event: &str| {
        let line = log.iter().find(|line| line["event"] == event).unwrap_or_else(|| panic!("no {event}"));
        line["detail"].as_str().unwrap().to_owned()
    };
    assert!(detail("app.start").contains("version="));
    assert!(detail("track.loaded").starts_with("path=~/Music/"), "the home folder must be hidden");
    assert!(detail("shutdown.start").contains("SIGTERM"));
    detail("shutdown.done");
}

#[test]
fn a_killed_run_is_reported_by_the_next_one() {
    let sandbox = Sandbox::new("died", true);
    let mut first = sandbox.start(&[]);
    first.expect("app.ready");
    first.child.kill().unwrap();
    first.child.wait().unwrap();
    let mut second = sandbox.start(&[]);
    second.expect("app.previous_run_died");
    second.press(b"q");
    assert_eq!(second.exit_code(), 0);
    let mut third = sandbox.start(&[]);
    third.expect("app.ready");
    third.press(b"q");
    assert_eq!(third.exit_code(), 0);
    let died = sandbox.log().iter().filter(|line| line["event"] == "app.previous_run_died").count();
    assert_eq!(died, 1, "a clean run must not look like a death");
}

#[test]
fn old_logs_are_deleted_at_start() {
    let sandbox = Sandbox::new("old-logs", true);
    let logs = sandbox.root.join("state/lull/logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join("lull-2020-01-01.log"), "{}\n").unwrap();
    std::fs::write(logs.join("notes.txt"), "keep").unwrap();
    sandbox.start(&[]).expect("app.ready");
    assert!(!logs.join("lull-2020-01-01.log").exists());
    assert!(logs.join("notes.txt").exists());
}

#[test]
fn an_unwritable_log_never_stops_the_app() {
    let sandbox = Sandbox::new("no-log", true);
    std::fs::create_dir_all(sandbox.root.join("state/lull")).unwrap();
    std::fs::write(sandbox.root.join("state/lull/logs"), "a file where the folder goes").unwrap();
    let mut app = sandbox.start(&[]);
    app.expect("app.ready");
    app.press(b"q");
    assert_eq!(app.exit_code(), 0);
}

#[test]
fn only_one_copy_runs() {
    let sandbox = Sandbox::new("lock", true);
    let first = sandbox.start(&[]);
    first.expect("app.ready");
    let mut second = sandbox.start(&[]);
    second.expect("already running");
    assert_eq!(second.exit_code(), 2);
}

#[test]
fn an_empty_music_folder_gets_a_plain_message() {
    let sandbox = Sandbox::new("empty", false);
    let mut app = sandbox.start(&[]);
    app.expect("there are no .opus files in");
    assert_eq!(app.exit_code(), 1);
}

#[test]
fn a_damaged_state_file_starts_fresh() {
    let sandbox = Sandbox::new("damaged", true);
    std::fs::create_dir_all(sandbox.root.join("state/lull")).unwrap();
    std::fs::write(sandbox.root.join("state/lull/state"), b"garbage").unwrap();
    let app = sandbox.start(&[]);
    app.expect("state.reset");
    app.expect("app.ready at=0:00");
}
