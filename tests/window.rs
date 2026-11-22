//! The real window, rendered into memory, clicked and typed into. No display and no sound process.
//! LULL_SHOTS=DIR also writes a screenshot of every state, for the design rounds.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use lull::link::{Playback, Problem, Request, Sound, Status, ToWindow};
use lull::visual::{BANDS, Levels};
use lull::window::{AppWindow, Controller, Queue};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, PlatformError, Rgb8Pixel, SharedString};

const WIDTH: usize = 360;
const HEIGHT: usize = 540;
const RAIN: &str = "Rain on Window with Thunder (8h)";

thread_local! {
    static MADE: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
}

struct Headless {
    start: Instant,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        MADE.with(|made| *made.borrow_mut() = Some(window.clone()));
        Ok(window)
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

struct Screen {
    window: Rc<MinimalSoftwareWindow>,
    ui: AppWindow,
    controller: Rc<Controller>,
    queue: Queue,
    requests: Rc<RefCell<Vec<(u64, Request)>>>,
}

impl Screen {
    fn new(still: bool) -> Self {
        let ui = AppWindow::new().unwrap();
        let queue = Queue::default();
        let requests = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::clone(&requests);
        let controller =
            Controller::new(&ui, queue.clone(), still, move |n, request| log.borrow_mut().push((n, request)));
        ui.show().unwrap();
        let window = MADE.with(|made| made.borrow_mut().take().unwrap());
        window.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
        window.dispatch_event(WindowEvent::WindowActiveChanged(true));
        let screen = Self { window, ui, controller, queue, requests };
        screen.wait(30);
        screen
    }

    fn status(&self, playback: Playback, problem: Problem) {
        let handled = self.requests.borrow().last().map_or(0, |&(n, _)| n);
        let has = playback != Playback::Empty;
        let (seconds, total) = if has { (2846, 28_813) } else { (0, 0) };
        self.controller.receive(ToWindow::Status(Status { playback, problem, seconds, total, volume: 45, handled }));
    }

    fn rain(playback: Playback) -> Self {
        let screen = Self::new(false);
        screen.controller.receive(ToWindow::Title(RAIN.into()));
        screen.status(playback, Problem::None);
        screen.wait(30);
        screen
    }

    fn wait(&self, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        loop {
            slint::platform::update_timers_and_animations();
            // A test screen always shows what it is given.
            self.controller.presented();
            if Instant::now() >= end {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn press(&self, x: f32, y: f32) {
        let position = LogicalPosition::new(x, y);
        self.window.dispatch_event(WindowEvent::PointerMoved { position });
        self.window.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
    }

    fn slide(&self, x: f32, y: f32) {
        self.window.dispatch_event(WindowEvent::PointerMoved { position: LogicalPosition::new(x, y) });
    }

    fn release(&self, x: f32, y: f32) {
        let position = LogicalPosition::new(x, y);
        self.window.dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
        self.wait(20);
    }

    fn click(&self, x: f32, y: f32) {
        self.press(x, y);
        self.release(x, y);
    }

    fn key(&self, key: impl Into<SharedString>) {
        let text = key.into();
        self.window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.window.dispatch_event(WindowEvent::KeyReleased { text });
        self.wait(20);
    }

    /// The requests since the last call, answered as the sound process would.
    fn taken(&self) -> Vec<Request> {
        self.requests.borrow_mut().drain(..).map(|(_, request)| request).collect()
    }

    fn pixels(&self) -> Vec<Rgb8Pixel> {
        self.wait(10);
        let mut pixels = vec![Rgb8Pixel::default(); WIDTH * HEIGHT];
        self.window.request_redraw();
        self.window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, WIDTH);
        });
        pixels
    }

    fn shot(&self, name: &str) {
        let Some(folder) = std::env::var_os("LULL_SHOTS").map(PathBuf::from) else { return };
        let mut file = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
        for pixel in self.pixels() {
            file.extend([pixel.r, pixel.g, pixel.b]);
        }
        std::fs::write(folder.join(format!("{name}.ppm")), file).unwrap();
    }
}

fn region(pixels: &[Rgb8Pixel], (x0, y0, x1, y1): (usize, usize, usize, usize)) -> Vec<Rgb8Pixel> {
    (y0..y1).flat_map(|y| pixels[y * WIDTH + x0..y * WIDTH + x1].iter().copied()).collect()
}

fn play_and_pause() {
    let screen = Screen::rain(Playback::Paused);
    screen.click(56.0, 480.0);
    assert_eq!(screen.taken(), [Request::SetPlaying(true)]);
    assert!(screen.ui.get_playing(), "shows at once");
    screen.click(56.0, 480.0);
    assert_eq!(screen.taken(), [Request::SetPlaying(false)]);
    screen.press(56.0, 480.0);
    screen.slide(220.0, 250.0);
    screen.release(220.0, 250.0);
    assert_eq!(screen.taken(), [], "sliding off cancels");
}

fn seek_commits_on_release() {
    let screen = Screen::rain(Playback::Paused);
    screen.press(60.0, 393.0);
    screen.slide(180.0, 393.0);
    screen.wait(20);
    assert_eq!(screen.taken(), [], "sought before release");
    screen.shot("seeking");
    screen.release(180.0, 393.0);
    let [Request::Seek(to)] = screen.taken()[..] else { panic!("expected one seek") };
    assert!(to.abs_diff(14_406) < 150, "landed at {to}");

    screen.press(60.0, 393.0);
    screen.slide(300.0, 393.0);
    screen.key(Key::Escape);
    screen.release(300.0, 393.0);
    assert_eq!(screen.taken(), [], "Escape cancels a drag");

    screen.press(180.0, 393.0);
    screen.slide(900.0, 393.0);
    screen.release(900.0, 393.0);
    assert_eq!(screen.taken(), [Request::Seek(28_813)], "clamps at the end");
}

fn volume_follows_the_drag() {
    let screen = Screen::rain(Playback::Paused);
    screen.press(150.0, 494.0);
    screen.slide(224.0, 494.0);
    screen.wait(20);
    let sent = screen.taken();
    assert!(sent.len() >= 2 && matches!(sent.last(), Some(Request::Volume(48..=52))), "{sent:?}");
    screen.release(224.0, 494.0);
}

fn keys() {
    let screen = Screen::rain(Playback::Paused);
    screen.key(" ");
    assert_eq!(screen.taken(), [Request::SetPlaying(true)]);
    screen.key(Key::RightArrow);
    screen.key(Key::LeftArrow);
    assert_eq!(screen.taken(), [Request::Seek(2876), Request::Seek(2846)]);
    screen.key(Key::UpArrow);
    screen.key(Key::DownArrow);
    assert_eq!(screen.taken(), [Request::Volume(50), Request::Volume(45)]);

    let quit = Rc::new(RefCell::new(0));
    let count = Rc::clone(&quit);
    screen.ui.on_quit(move || *count.borrow_mut() += 1);
    screen.window.dispatch_event(WindowEvent::KeyPressed { text: Key::Control.into() });
    screen.key("q");
    screen.window.dispatch_event(WindowEvent::KeyReleased { text: Key::Control.into() });
    assert_eq!(*quit.borrow(), 1, "Ctrl+Q");
}

fn every_control_is_reachable_by_tab() {
    let screen = Screen::rain(Playback::Paused);
    let mut looks = vec![screen.pixels()];
    let mut reached = Vec::new();
    for step in 1..=4 {
        screen.key(Key::Tab);
        let now = screen.pixels();
        assert!(looks.iter().all(|seen| *seen != now), "Tab stop {step} shows no focus");
        looks.push(now);
        screen.shot(&format!("focus-{step}"));
        screen.key(" ");
        reached.extend(screen.taken());
        if screen.ui.get_sheet_open() {
            screen.key(Key::Escape);
            // Closing the list returns focus to the window; walk back to this stop.
            (0..step).for_each(|_| screen.key(Key::Tab));
        }
    }
    assert!(reached.contains(&Request::List), "Sounds not reachable: {reached:?}");
    assert!(reached.iter().any(|r| matches!(r, Request::SetPlaying(_))), "Play not reachable: {reached:?}");
}

fn sounds_list() {
    let screen = Screen::rain(Playback::Paused);
    let before = screen.pixels();
    screen.press(300.0, 327.0);
    screen.wait(20);
    assert!(screen.ui.get_sheet_open(), "opens on press");
    assert_eq!(screen.taken(), [Request::List]);
    screen.release(300.0, 327.0);
    let sounds = vec![
        Sound { name: "Ocean".into(), seconds: 5100, current: false },
        Sound { name: RAIN.into(), seconds: 28_813, current: true },
    ];
    screen.controller.receive(ToWindow::Sounds { list: 7, rows: sounds });
    screen.status(Playback::Paused, Problem::None);
    screen.shot("sounds");
    let open = screen.pixels();
    let below = (0, 310, 250, HEIGHT);
    assert_eq!(region(&before, below), region(&open, below), "opening the list moved something");

    screen.click(56.0, 480.0);
    assert!(!screen.ui.get_sheet_open());
    assert_eq!(screen.taken(), [], "the click that closes the list must not press Play");

    screen.click(300.0, 327.0);
    screen.controller.receive(ToWindow::Status(Status::default())); // An old snapshot changes nothing.
    screen.click(120.0, 185.0);
    assert_eq!(screen.taken(), [Request::List, Request::Open { list: 7, row: 0 }]);
    assert!(!screen.ui.get_sheet_open());
}

fn old_snapshots_never_undo_a_press() {
    let screen = Screen::rain(Playback::Paused);
    screen.click(56.0, 480.0);
    let old = Status { playback: Playback::Paused, total: 28_813, handled: 0, ..Status::default() };
    screen.controller.receive(ToWindow::Status(old));
    assert!(screen.ui.get_playing());
    screen.status(Playback::Paused, Problem::None);
    assert!(!screen.ui.get_playing(), "the answer does count");
}

fn nothing_moves_on_state_change() {
    let screen = Screen::rain(Playback::Paused);
    let paused = screen.pixels();
    for (playback, problem) in [(Playback::Playing, Problem::None), (Playback::Waiting, Problem::NoDevice)] {
        screen.status(playback, problem);
        screen.wait(20);
        let now = screen.pixels();
        for area in
            [(24, 322, 240, 348), (24, 410, 80, 424), (270, 410, 336, 424), (112, 458, 180, 474), (300, 458, 336, 474)]
        {
            assert_eq!(region(&paused, area), region(&now, area), "{playback:?} moved {area:?}");
        }
    }
}

fn ring_animates_then_rests() {
    let screen = Screen::rain(Playback::Paused);
    let ring = (30, 48, 330, 305);
    let still = region(&screen.pixels(), ring);
    screen.wait(150);
    assert_eq!(still, region(&screen.pixels(), ring), "paused must not animate");
    screen.status(Playback::Playing, Problem::None);
    for step in 0..110 {
        let swing = |n: usize| 0.45 + 0.4 * ((step + n * 5) as f32 * 0.21).sin();
        let levels = Levels { bands: std::array::from_fn::<_, BANDS, _>(swing), bass: 0.5 };
        screen.queue.push(Instant::now(), levels);
        screen.wait(33);
    }
    let playing = region(&screen.pixels(), ring);
    assert_ne!(still, playing);
    screen.shot("playing");
    screen.status(Playback::Paused, Problem::None);
    screen.wait(3500);
    let rested = region(&screen.pixels(), ring);
    screen.wait(300);
    assert_eq!(rested, region(&screen.pixels(), ring), "kept drawing after it settled");
    screen.shot("paused");
}

fn reduced_motion() {
    let screen = Screen::new(true);
    screen.controller.receive(ToWindow::Title(RAIN.into()));
    screen.status(Playback::Playing, Problem::None);
    let ring = (30, 48, 330, 305);
    let first = region(&screen.pixels(), ring);
    screen.wait(400);
    assert_eq!(first, region(&screen.pixels(), ring));
    screen.shot("reduced-motion");
}

fn states_for_review() {
    let empty = Screen::new(false);
    empty.status(Playback::Empty, Problem::NoSounds);
    empty.shot("empty");
    empty.click(56.0, 480.0);
    assert_eq!(empty.taken(), [], "Play is off with no sound");
    empty.click(300.0, 327.0);
    empty.controller.receive(ToWindow::Sounds { list: 1, rows: vec![] });
    empty.status(Playback::Empty, Problem::NoSounds);
    empty.shot("empty-list");

    for (name, playback, problem) in [
        ("no-device", Playback::Waiting, Problem::NoDevice),
        ("no-server", Playback::Waiting, Problem::NoServer),
        ("unreadable", Playback::Paused, Problem::Unreadable),
        ("missing", Playback::Paused, Problem::Missing),
    ] {
        let screen = Screen::rain(playback);
        screen.status(playback, problem);
        screen.shot(name);
    }

    let long = Screen::rain(Playback::Paused);
    let name =
        "Distant Thunderstorm over a Tin Roof in the Mountains with Wind through Pine Trees and a Crackling Fire";
    long.controller.receive(ToWindow::Title(name.into()));
    long.status(Playback::Paused, Problem::None);
    long.shot("long-name");
    long.click(300.0, 327.0);
    let mut sounds: Vec<Sound> =
        (1..=6).map(|n| Sound { name: format!("Focus sound {n}"), seconds: n * 1700, current: false }).collect();
    sounds.insert(1, Sound { name: name.into(), seconds: 28_813, current: true });
    sounds.insert(2, Sound { name: "Broken download".into(), seconds: 0, current: false });
    long.controller.receive(ToWindow::Sounds { list: 2, rows: sounds });
    long.status(Playback::Paused, Problem::None);
    long.shot("long-list");
    long.click(120.0, 48.0 + 8.0 + 2.0 * 52.0 + 24.0);
    assert_eq!(long.taken(), [Request::List], "an unplayable row can't be opened");
}

/// One test, because Slint allows one platform per process.
#[test]
fn the_window_keeps_its_promises() {
    slint::platform::set_platform(Box::new(Headless { start: Instant::now() })).unwrap();
    let checks: [(&str, fn()); 11] = [
        ("play and pause", play_and_pause),
        ("seek commits on release", seek_commits_on_release),
        ("volume follows the drag", volume_follows_the_drag),
        ("keys", keys),
        ("every control is reachable by Tab", every_control_is_reachable_by_tab),
        ("sounds list", sounds_list),
        ("old snapshots never undo a press", old_snapshots_never_undo_a_press),
        ("nothing moves on a state change", nothing_moves_on_state_change),
        ("ring animates, then rests", ring_animates_then_rests),
        ("reduced motion", reduced_motion),
        ("other states", states_for_review),
    ];
    let mut failed = Vec::new();
    for (name, check) in checks {
        let ok = std::panic::catch_unwind(check).is_ok();
        println!("{} {name}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            failed.push(name);
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}
