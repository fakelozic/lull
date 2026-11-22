//! Joins the Slint window to the sound process: requests go out, snapshots come in.
//! A press shows at once. Snapshots older than the last press are ignored until it is answered.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use super::painter::{Painter, Queue};
use super::ui::{AppWindow, Format, SoundRow};
use super::view;
use crate::link::{Playback, Request, Sound, Status, ToWindow};
use crate::{log, timecode, track};

struct Model {
    status: Status,
    name: String,
    sounds: Vec<Sound>,
    list: u64,
    sent: u64,
}

/// The window's logic, without a socket or a process, so tests can drive it.
pub struct Controller {
    ui: slint::Weak<AppWindow>,
    model: RefCell<Model>,
    painter: Painter,
    still: bool,
    send: Box<dyn Fn(u64, Request)>,
}

impl Controller {
    /// Wires every callback. `send` delivers numbered requests to the sound process.
    pub fn new(ui: &AppWindow, queue: Queue, still: bool, send: impl Fn(u64, Request) + 'static) -> Rc<Self> {
        let model = Model { status: Status::default(), name: String::new(), sounds: Vec::new(), list: 0, sent: 0 };
        let controller = Rc::new(Self {
            ui: ui.as_weak(),
            model: RefCell::new(model),
            painter: Painter::new(ui, queue, still),
            still,
            send: Box::new(send),
        });
        ui.global::<Format>().on_clock(|seconds| timecode::format(seconds.max(0) as u64).into());
        controller.wire(ui);
        let this = Rc::downgrade(&controller);
        controller.painter.on_hidden(move || {
            if let Some(this) = this.upgrade() {
                log::note("window.visible", "visible=false reason=no frames shown");
                this.request(Request::Watch(false));
            }
        });
        controller
    }

    fn wire(self: &Rc<Self>, ui: &AppWindow) {
        let this = Rc::downgrade(self);
        ui.on_set_playing(move |on| {
            let Some(this) = this.upgrade() else { return };
            this.act(Request::SetPlaying(on), |s| s.playback = if on { Playback::Playing } else { Playback::Paused });
        });
        let this = Rc::downgrade(self);
        ui.on_seek(move |to| {
            let Some(this) = this.upgrade() else { return };
            let to = to.max(0) as u64;
            this.act(Request::Seek(to), |status| status.seconds = to);
        });
        let this = Rc::downgrade(self);
        ui.on_set_volume(move |to| {
            let Some(this) = this.upgrade() else { return };
            let to = to.clamp(0, 100) as u8;
            if this.model.borrow().status.volume != to {
                this.act(Request::Volume(to), |status| status.volume = to);
            }
        });
        let this = Rc::downgrade(self);
        ui.on_list_sounds(move || {
            let Some(this) = this.upgrade() else { return };
            this.request(Request::List);
        });
        let this = Rc::downgrade(self);
        ui.on_open_sound(move |row| {
            let Some(this) = this.upgrade() else { return };
            if let Ok(row) = usize::try_from(row) {
                let list = this.model.borrow().list;
                this.request(Request::Open { list, row });
            }
        });
    }

    fn act(&self, request: Request, change: impl FnOnce(&mut Status)) {
        change(&mut self.model.borrow_mut().status);
        self.show();
        self.request(request);
    }

    /// Sends a request with the next number.
    pub fn request(&self, request: Request) {
        let number = {
            let mut model = self.model.borrow_mut();
            model.sent += 1;
            model.sent
        };
        (self.send)(number, request);
    }

    /// A frame reached the screen. If the window had gone unseen, it is back.
    pub fn presented(&self) {
        if self.painter.presented() {
            log::note("window.visible", "visible=true");
            self.set_visible(true);
        }
    }

    /// The system said outright that the window is covered or uncovered. That outranks guessing from frames.
    pub fn set_occluded(&self, occluded: bool) {
        self.painter.set_occluded(occluded);
        self.set_visible(!occluded);
    }

    fn set_visible(&self, visible: bool) {
        self.painter.set_visible(visible);
        // With reduced motion nothing follows the levels, so don't have them worked out.
        self.request(Request::Watch(visible && !self.still));
    }

    /// Takes a message from the sound process.
    pub fn receive(&self, message: ToWindow) {
        {
            let mut model = self.model.borrow_mut();
            match message {
                ToWindow::Status(status) if status.handled >= model.sent => model.status = status,
                ToWindow::Title(name) => model.name = name,
                ToWindow::Sounds { list, rows } => (model.list, model.sounds) = (list, rows),
                // An old snapshot, or levels, which the reader queues for the painter.
                ToWindow::Status(_) | ToWindow::Levels { .. } => return,
            }
        }
        self.show();
        self.show_sounds();
    }

    fn show(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let model = self.model.borrow();
        let status = &model.status;
        let lines = view::lines(status, &model.name);
        ui.set_has_sound(status.playback != Playback::Empty);
        ui.set_playing(matches!(status.playback, Playback::Playing | Playback::Waiting));
        ui.set_heading(lines.heading.into());
        ui.set_detail(lines.detail.into());
        ui.set_alert(lines.alert);
        ui.set_seconds(i32::try_from(status.seconds).unwrap_or(i32::MAX));
        ui.set_total(i32::try_from(status.total).unwrap_or(i32::MAX));
        ui.set_volume(status.volume.into());
        // While waiting nothing is heard, so the ring rests.
        self.painter.set_playing(status.playback == Playback::Playing);
    }

    fn show_sounds(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        // The list only needs rebuilding while it can be seen.
        if !ui.get_sheet_open() {
            return;
        }
        let model = self.model.borrow();
        let rows: Vec<SoundRow> = model
            .sounds
            .iter()
            .map(|sound| {
                // Two lines, like the player: the part that tells similar sounds apart stays visible.
                let (title, detail) = track::headline(&sound.name);
                SoundRow {
                    name: title.into(),
                    detail: detail.into(),
                    length: view::length(sound.seconds).into(),
                    current: sound.current,
                    playable: sound.seconds > 0,
                }
            })
            .collect();
        let shown: ModelRc<SoundRow> = ui.get_sounds();
        // Replacing an identical list would drop the hover under the pointer every second.
        if !slint::Model::iter(&shown).eq(rows.iter().cloned()) {
            ui.set_sounds(ModelRc::new(VecModel::from(rows)));
        }
    }
}
