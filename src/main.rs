//! The entry point. One binary, two jobs: the sound process, or (`--window`) its window.
//! A second launch hands over to the first and exits (`remote::control`).
//! Start order is for speed: config, lock, state, window, bus, then libraries and sound, which can
//! take 10 ms and run while the window starts. Stop is the reverse.

#![deny(unsafe_code)]

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

use lull::app::{self, App, Message};
use lull::config::{self, Options, Paths, Request};
use lull::error::{Error, Result};
use lull::front::{Front, Windowed};
use lull::link::Problem;
use lull::log::Process;
use lull::remote::{self, Ask, Handoff, Launch, Remote};
use lull::runinfo::{self, Running};
use lull::sounds::Opened;
use lull::state::{self, State, Store};
use lull::terminal::Terminal;
use lull::timecode::RATE;
use lull::{ffi, inbox, log, track, window};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

fn main() -> ExitCode {
    let result = config::parse(std::env::args_os().skip(1)).and_then(|request| match request {
        Request::Help => print_out(config::USAGE),
        Request::Version => print_out(concat!("lull ", env!("CARGO_PKG_VERSION"), "\n")),
        Request::Window { run, asked } => be_window(&run, asked),
        Request::Run(options) => run(options),
    });
    let code = match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::error("app.failed", &error.to_string());
            if matches!(error, Error::Usage(_)) { ExitCode::from(2) } else { ExitCode::FAILURE }
        }
    };
    log::flush();
    code
}

fn print_out(text: &str) -> Result<()> {
    use std::io::Write;
    // `lull --help | head` closing the pipe is not an error.
    let _ = std::io::stdout().write_all(text.as_bytes());
    Ok(())
}

fn be_window(run: &str, asked: Option<u64>) -> Result<()> {
    let paths = Paths::from_env()?;
    log::start(Process::Window, run, &paths.logs, &paths.home);
    let still = paths.reduced_motion();
    log::note("window.starting", &format!("reduced_motion={still}"));
    window::run(still, asked)
}

/// Longer than a running Lull needs to start or to shut down.
const HANDOVER: Duration = Duration::from_secs(3);

/// Takes the lock, or gives this launch to the Lull that has it and returns `None`.
/// It retries, because that one may still be starting or on its way out.
fn lock_or_hand_over(options: &Options, paths: &Paths) -> Result<Option<std::fs::File>> {
    let launch = Launch { token: paths.token.clone(), play: options.play, at: options.at, file: options.file.clone() };
    let deadline = Instant::now() + HANDOVER;
    loop {
        let busy = match state::lock(&paths.runtime) {
            Ok(lock) => return Ok(Some(lock)),
            Err(busy @ Error::Usage(_)) => busy,
            Err(error) => return Err(error),
        };
        match remote::hand_over(&launch) {
            Handoff::Done => {
                log::note("app.handed_over", "Lull was running, so it took this launch");
                return Ok(None);
            }
            Handoff::Refused => {
                let why = "Lull is running, and from outside it only opens sounds inside ~/Music. Quit it first";
                return Err(Error::Usage(why.into()));
            }
            Handoff::NotReady if Instant::now() >= deadline => return Err(busy),
            Handoff::NotReady => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn run(options: Options) -> Result<()> {
    let paths = Paths::from_env()?;
    let run_id = log::new_run();
    log::start(Process::Sound, &run_id, &paths.logs, &paths.home);
    log::note("app.start", &format!("{} options={options:?}", runinfo::describe()));
    let store = Store::open(&paths.state)?;
    let Some(_lock) = lock_or_hand_over(&options, &paths)? else { return Ok(()) };
    let stop = stop_on_signals()?;
    let (running, died) = Running::begin(&paths.state, &run_id);
    if let Some(died) = died {
        log::warn("app.previous_run_died", &format!("it was cut off without shutting down: {died}"));
    }
    let result = play(&options, paths, store, &run_id, &stop);
    // Returning, even with an error, is a known ending. Only a death leaves the marker behind.
    running.end();
    result
}

fn play(options: &Options, paths: Paths, store: Store, run_id: &str, stop: &AtomicUsize) -> Result<()> {
    let saved = store.load().unwrap_or_else(|error| {
        log::warn("state.reset", &error.to_string());
        State::default()
    });
    // A file named on the command line is checked before any window appears. Otherwise the window
    // starts first, because it is the slow part and a bad file only changes what it says.
    let named = match options.file {
        Some(_) => Some(open_first(options, &paths, &saved)?),
        None => None,
    };
    let (sender, inbox) = inbox::channel();
    let front = front_end(options, run_id, paths.token.as_deref(), &sender)?;
    let remote = if options.no_window { None } else { start_remote(&paths, &sender) };
    let (sound, problem) = match named {
        Some(opened) => opened,
        None => open_first(options, &paths, &saved)?,
    };
    let mut app = App::new(front, sender, store, saved, paths.music, paths.state);
    if let Some(remote) = remote {
        app.set_remote(remote);
    }
    app.set_problem(problem);
    if let Some(sound) = sound {
        app.load(sound)?;
    }
    app.show_now();
    app.run(&inbox, stop, options.play)
}

fn open_first(options: &Options, paths: &Paths, saved: &State) -> Result<(Option<Opened>, Problem)> {
    ffi::check_libraries()?;
    first_sound(options, paths, saved)
}

fn start_remote(paths: &Paths, sender: &inbox::Sender<Message>) -> Option<Remote> {
    let asks = sender.clone();
    let started = Remote::start(paths.music.clone(), move |ask| match ask {
        Ask::Quit => asks.send(Message::Remote(ask)),
        _ => asks.send_or_drop(Message::Remote(ask)),
    });
    started.inspect_err(|error| log::warn("remote.unavailable", &error.to_string())).ok()
}

/// What to open, and what to tell the person about it. A window can open with nothing, the terminal can't.
fn first_sound(options: &Options, paths: &Paths, saved: &State) -> Result<(Option<Opened>, Problem)> {
    let can_be_empty = !options.no_window && options.file.is_none();
    let path = match track::choose(options.file.as_deref(), saved.track.as_deref(), &paths.music) {
        Ok(path) => path,
        Err(error) if can_be_empty => {
            log::warn("app.no_sounds", &error.to_string());
            return Ok((None, Problem::NoSounds));
        }
        Err(error) => return Err(error),
    };
    let same = saved.track.as_ref() == Some(&path);
    let went_missing = options.file.is_none() && saved.track.is_some() && !same;
    let start = options.at.map_or(if same { saved.position } else { 0 }, |seconds| seconds * RATE);
    // A path given on the command line, then or last time, may be anywhere. One we found must really be in ~/Music.
    let chosen_by_person = options.file.is_some() || same;
    match Opened::new(path, start, saved.volume, (!chosen_by_person).then_some(paths.music.as_path())) {
        Ok(sound) => Ok((Some(sound), if went_missing { Problem::Missing } else { Problem::None })),
        Err(error) if can_be_empty => {
            log::error("audio.failed", &error.to_string());
            Ok((None, Problem::Unreadable))
        }
        Err(error) => Err(error),
    }
}

fn front_end(options: &Options, run_id: &str, token: Option<&str>, sender: &inbox::Sender<Message>) -> Result<Front> {
    if !options.no_window {
        let window = Windowed::start(run_id.to_owned(), token, app::window_reporter(sender.clone()))?;
        return Ok(Front::Window(Box::new(window)));
    }
    let keys = sender.clone();
    let terminal = Terminal::open(move |action| {
        keys.send_or_wait(Message::Key(action));
    })
    .map_err(|e| Error::Usage(format!("Can't start a thread: {e}")))?;
    Ok(Front::Terminal(terminal))
}

fn stop_on_signals() -> Result<Arc<AtomicUsize>> {
    // Holds the signal's number, so the log can say what stopped Lull.
    let stop = Arc::new(AtomicUsize::new(0));
    for signal in [SIGINT, SIGTERM, SIGHUP] {
        signal_hook::flag::register_usize(signal, Arc::clone(&stop), signal as usize)
            .map_err(|e| Error::Usage(format!("Can't watch for signals: {e}")))?;
    }
    Ok(stop)
}
