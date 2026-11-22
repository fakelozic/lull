//! Runs the ring: 30 frames a second while it moves and can be seen, nothing otherwise.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use slint::{Color, ComponentHandle, Image, Rgb8Pixel, SharedPixelBuffer, Timer, TimerMode, Weak};

use super::ui::AppWindow;
use crate::visual::{Levels, Ripples, SIDE};

/// 30 fps, the required minimum. At 60 Hz each frame shows for exactly two refreshes.
const FRAME: Duration = Duration::from_nanos(33_333_333);
/// The accent follows the ring five times a second. Every frame would repaint all the controls.
const ACCENT_EVERY: u32 = 6;
/// A longer gap (a stall, a sleep) doesn't make the animation leap.
const LONGEST_STEP: f32 = 0.1;
const MOST_QUEUED: usize = 64;
/// Frames offered without one reaching the screen before the window counts as hidden (1 s).
///
/// winit 0.30 drops Wayland's "suspended" state, so minimising sends no event. But the compositor
/// stops frame callbacks for a window nobody can see, so redraws stop arriving, and that does show.
const UNSEEN_LIMIT: u32 = 30;

/// Levels waiting for the moment they are heard.
#[derive(Clone, Default)]
pub struct Queue(Arc<Mutex<VecDeque<(Instant, Levels)>>>);

impl Queue {
    /// Queues levels from the reader thread.
    pub fn push(&self, due: Instant, levels: Levels) {
        let mut queue = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.len() == MOST_QUEUED {
            queue.pop_front();
        }
        queue.push_back((due, levels));
    }

    fn take_due(&self, now: Instant) -> Option<Levels> {
        let mut queue = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let mut latest = None;
        while queue.front().is_some_and(|&(due, _)| due <= now) {
            latest = queue.pop_front().map(|(_, levels)| levels);
        }
        latest
    }
}

struct Animation {
    ui: Weak<AppWindow>,
    queue: Queue,
    ring: Ripples,
    /// Two, swapped, so drawing never copies a buffer the screen still holds.
    frames: [SharedPixelBuffer<Rgb8Pixel>; 2],
    front: usize,
    last: Instant,
    due: Instant,
    count: u32,
    playing: bool,
    visible: bool,
    /// Said by the system, as opposed to guessed from frames.
    occluded: bool,
    unseen: u32,
    on_hidden: Box<dyn Fn()>,
}

impl Animation {
    /// Draws one frame. Returns false when there is nothing left to animate.
    fn step(&mut self) -> bool {
        let now = Instant::now();
        if let Some(levels) = self.queue.take_due(now) {
            self.ring.listen(levels);
        }
        let seconds = now.duration_since(self.last).as_secs_f32().min(LONGEST_STEP);
        self.last = now;
        if !self.visible || !self.ring.advance(seconds, self.playing) {
            return false;
        }
        self.unseen += 1;
        if self.unseen > UNSEEN_LIMIT {
            self.visible = false;
            (self.on_hidden)();
            return false;
        }
        self.count = self.count.wrapping_add(1);
        self.show(self.count.is_multiple_of(ACCENT_EVERY));
        true
    }

    /// How long until the next frame is due. Slint re-arms a repeating timer from "now", so each
    /// late wake-up would push back every later frame and 30 fps would drift to 29.
    fn until_next(&mut self) -> Duration {
        let now = Instant::now();
        self.due += FRAME;
        if self.due <= now {
            self.due = now + FRAME;
        }
        self.due - now
    }

    fn show(&mut self, with_accent: bool) {
        let Some(ui) = self.ui.upgrade() else { return };
        self.front ^= 1;
        self.ring.paint(self.frames[self.front].make_mut_bytes());
        ui.set_ring(Image::from_rgb8(self.frames[self.front].clone()));
        if with_accent {
            let [red, green, blue] = self.ring.accent();
            ui.set_tone(Color::from_rgb_u8(red, green, blue));
        }
    }
}

/// The handle the window holds.
pub struct Painter {
    animation: Rc<RefCell<Animation>>,
    timer: Rc<Timer>,
}

impl Painter {
    /// Draws the first, still frame. `still` is reduced motion.
    pub fn new(ui: &AppWindow, queue: Queue, still: bool) -> Self {
        let side = SIDE as u32;
        let mut animation = Animation {
            ui: ui.as_weak(),
            queue,
            ring: Ripples::new(still),
            frames: [SharedPixelBuffer::new(side, side), SharedPixelBuffer::new(side, side)],
            front: 0,
            last: Instant::now(),
            due: Instant::now(),
            count: 0,
            playing: false,
            visible: true,
            occluded: false,
            unseen: 0,
            on_hidden: Box::new(|| {}),
        };
        animation.show(true);
        Self { animation: Rc::new(RefCell::new(animation)), timer: Rc::new(Timer::default()) }
    }

    /// Calls `hidden` when frames stop reaching the screen. It must not call back into the painter.
    pub fn on_hidden(&self, hidden: impl Fn() + 'static) {
        self.animation.borrow_mut().on_hidden = Box::new(hidden);
    }

    /// A frame reached the screen. Returns true if the window had counted as hidden until now.
    pub fn presented(&self) -> bool {
        let mut animation = self.animation.borrow_mut();
        animation.unseen = 0;
        // A redraw can still arrive for a covered window (X11), so it doesn't overrule the system.
        !animation.visible && !animation.occluded
    }

    /// Follows play and pause.
    pub fn set_playing(&self, playing: bool) {
        let changed = std::mem::replace(&mut self.animation.borrow_mut().playing, playing) != playing;
        if changed {
            self.wake();
        }
    }

    /// Follows what the system says about the window being covered.
    pub fn set_occluded(&self, occluded: bool) {
        self.animation.borrow_mut().occluded = occluded;
    }

    /// Follows the window being hidden or shown.
    pub fn set_visible(&self, visible: bool) {
        {
            let mut animation = self.animation.borrow_mut();
            animation.visible = visible;
            animation.unseen = 0;
        }
        if visible {
            self.wake();
        }
    }

    fn wake(&self) {
        if self.timer.running() {
            return;
        }
        {
            let mut animation = self.animation.borrow_mut();
            animation.last = Instant::now();
            animation.due = animation.last + FRAME;
            animation.unseen = 0;
        }
        let animation = Rc::clone(&self.animation);
        // Weak, or the timer's own callback would keep it alive for ever.
        let timer = Rc::downgrade(&self.timer);
        self.timer.start(TimerMode::Repeated, FRAME, move || {
            let moving = animation.borrow_mut().step();
            let Some(timer) = timer.upgrade() else { return };
            if moving {
                timer.set_interval(animation.borrow_mut().until_next());
            } else {
                timer.stop();
            }
        });
    }
}
