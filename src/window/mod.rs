//! The window process (`lull --window`). It holds no audio: closing it or crashing it can't hurt the
//! sound, and all its memory comes back when it exits. It talks over the socket it was given as stdin.

mod controller;
mod painter;
mod ui;
mod view;

use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};
use slint::{CloseRequestResponse, ComponentHandle};

pub use controller::Controller;
pub use painter::Queue;
pub use ui::{AppWindow, SoundRow};

use crate::error::{Error, Result};
use crate::link::{MAX_LINE, Request, ToWindow};
use crate::log;

/// The newest of each kind, never a queue: a stuck window can't pile anything up.
#[derive(Default)]
struct Latest {
    status: Option<ToWindow>,
    title: Option<ToWindow>,
    sounds: Option<ToWindow>,
}

#[derive(Default)]
struct Mail {
    latest: Mutex<Latest>,
    /// A wake-up is already on its way to the UI thread.
    waking: AtomicBool,
}

type Inbox = Arc<Mail>;

/// How long the first frame may wait for the first snapshot, so it isn't blank and then filled in.
const FIRST_SNAPSHOT: Duration = Duration::from_millis(80);

/// Runs the window until it closes or the sound process goes away. `still` is reduced motion and
/// `asked` is when it was asked for, for the log.
pub fn run(still: bool, asked: Option<u64>) -> Result<()> {
    let socket = socket_on_stdin()?;
    // Alongside the toolkit's own start, which takes ten times as long.
    if let Err(error) = std::thread::Builder::new().name("ring-tables".into()).spawn(crate::visual::warm_up) {
        log::warn("window.warm_up_failed", &error.to_string());
    }
    let problem = |e: &dyn std::fmt::Display| Error::Window(e.to_string());
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()
        .map_err(|e| problem(&e))?;
    slint::set_xdg_app_id("lull").map_err(|e| problem(&e))?;
    let ui = AppWindow::new().map_err(|e| problem(&e))?;

    let queue = Queue::default();
    let inbox = Inbox::default();
    let writer = socket.try_clone().map_err(|e| problem(&e))?;
    writer.set_write_timeout(Some(Duration::from_secs(1))).map_err(|e| problem(&e))?;
    let controller = Controller::new(&ui, queue.clone(), still, move |number, request| {
        if let Err(error) = (&writer).write_all(request.encode(number).as_bytes()) {
            log::warn("window.send_failed", &error.to_string());
        }
    });

    let pending = Arc::clone(&inbox);
    let receiver = Rc::downgrade(&controller);
    ui.on_inbox_ready(move || {
        pending.waking.store(false, Ordering::Release);
        let latest = std::mem::take(&mut *pending.latest.lock().unwrap_or_else(PoisonError::into_inner));
        let Some(controller) = receiver.upgrade() else { return };
        // The title first: the status is shown with it.
        for message in [latest.title, latest.sounds, latest.status].into_iter().flatten() {
            controller.receive(message);
        }
    });
    let weak = ui.as_weak();
    let (theirs, reading) = (Arc::clone(&inbox), socket.try_clone().map_err(|e| problem(&e))?);
    std::thread::Builder::new()
        .name("link".into())
        .spawn(move || read_messages(reading, &weak, &queue, &theirs))
        .map_err(|e| problem(&e))?;
    show_first_snapshot(&inbox, &controller);
    wire_closing(&ui, &controller);
    ui.show().map_err(|e| problem(&e))?;
    watch_window(&ui, &controller, asked);
    controller.request(Request::Hello);
    controller.request(Request::Watch(!still));

    slint::run_event_loop().map_err(|e| problem(&e))
}

fn socket_on_stdin() -> Result<UnixStream> {
    let not_ours = || Error::Usage("--window is used by Lull itself. Run lull without it".into());
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(not_ours());
    }
    let socket = UnixStream::from(stdin.as_fd().try_clone_to_owned().map_err(|_| not_ours())?);
    socket.peer_addr().map_err(|_| not_ours())?;
    Ok(socket)
}

/// The first frame should be the whole picture, so wait the moment it takes to arrive.
fn show_first_snapshot(inbox: &Inbox, controller: &Controller) {
    let deadline = Instant::now() + FIRST_SNAPSHOT;
    while Instant::now() < deadline {
        let arrived = {
            let mut latest = inbox.latest.lock().unwrap_or_else(PoisonError::into_inner);
            latest.status.is_some().then(|| std::mem::take(&mut *latest))
        };
        if let Some(latest) = arrived {
            for message in [latest.title, latest.sounds, latest.status].into_iter().flatten() {
                controller.receive(message);
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn wire_closing(ui: &AppWindow, controller: &Rc<Controller>) {
    let closed = Rc::downgrade(controller);
    ui.window().on_close_requested(move || {
        if let Some(controller) = closed.upgrade() {
            controller.request(Request::Closed);
        }
        CloseRequestResponse::HideWindow
    });
    let (weak, quitting) = (ui.as_weak(), Rc::downgrade(controller));
    ui.on_quit(move || {
        if let Some(controller) = quitting.upgrade() {
            controller.request(Request::Quit);
        }
        if let Some(ui) = weak.upgrade()
            && let Err(error) = ui.hide()
        {
            log::warn("window.hide_failed", &error.to_string());
        }
    });
}

fn watch_window(ui: &AppWindow, controller: &Rc<Controller>, asked: Option<u64>) {
    let controller = Rc::downgrade(controller);
    let mut asked = asked;
    ui.window().on_winit_window_event(move |_, event| {
        if let Some(controller) = controller.upgrade() {
            match event {
                WindowEvent::RedrawRequested => {
                    controller.presented();
                    if let Some(asked) = asked.take() {
                        // Runs once this event, which does the drawing, has finished.
                        let drawn = slint::invoke_from_event_loop(move || {
                            log::note("window.first_frame", &format!("ms={}", log::now().saturating_sub(asked)));
                        });
                        if let Err(error) = drawn {
                            log::warn("window.first_frame", &error.to_string());
                        }
                    }
                }
                WindowEvent::Focused(focused) => controller.request(Request::Focused(*focused)),
                WindowEvent::Occluded(hidden) => {
                    log::note("window.visible", &format!("visible={}", !hidden));
                    controller.set_occluded(*hidden);
                }
                _ => {}
            }
        }
        EventResult::Propagate
    });
}

fn read_messages(socket: UnixStream, ui: &slint::Weak<AppWindow>, queue: &Queue, inbox: &Inbox) {
    let mut reader = BufReader::new(socket);
    let mut line = String::new();
    let mut reported = false;
    loop {
        line.clear();
        match (&mut reader).take(MAX_LINE as u64 + 1).read_line(&mut line) {
            Ok(read) if read > 0 && line.ends_with('\n') => {}
            _ => break,
        }
        match ToWindow::decode(line.trim_end_matches('\n')) {
            Some(ToWindow::Levels { delay_ms, levels }) => {
                queue.push(Instant::now() + Duration::from_millis(delay_ms.into()), levels);
            }
            Some(message) => {
                {
                    let mut latest = inbox.latest.lock().unwrap_or_else(PoisonError::into_inner);
                    let slot = match message {
                        ToWindow::Status(_) => &mut latest.status,
                        ToWindow::Title(_) => &mut latest.title,
                        _ => &mut latest.sounds,
                    };
                    *slot = Some(message);
                }
                if inbox.waking.swap(true, Ordering::AcqRel) {
                    continue;
                }
                if ui.upgrade_in_event_loop(|ui| ui.invoke_inbox_ready()).is_err() {
                    return; // The event loop already ended.
                }
            }
            None if !reported => {
                reported = true;
                log::warn("window.bad_message", "ignoring messages it can't read");
            }
            None => {}
        }
    }
    // The sound process is gone: a window with no sound behind it must not linger.
    if let Err(error) = slint::quit_event_loop() {
        log::warn("window.quit_failed", &error.to_string());
    }
}
