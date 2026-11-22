//! The tray icon: click shows or hides, middle-click plays or pauses, scroll changes the volume.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::StandardItem;
use ksni::{MenuItem, Orientation, ToolTip};
use zbus::zvariant::Value;

use super::{Ask, Shared, Snapshot, clean_token, icon};
use crate::link::{Playback, Problem};
use crate::log;

/// One wheel notch.
const NOTCH: i32 = 120;
const VOLUME_STEP: i16 = 5;

/// The icon's state.
pub(crate) struct LullTray {
    shared: Arc<Shared>,
    shown: Snapshot,
    scrolled: i32,
}

/// Puts up the icon. `None` if that can't be done at all; if there is just no tray yet, it waits for one.
pub(crate) fn start(shared: &Arc<Shared>) -> Option<Handle<LullTray>> {
    // The callbacks turn this off again if it turns out there is no tray.
    shared.tray_online.store(true, Ordering::Relaxed);
    let tray = LullTray { shared: Arc::clone(shared), shown: shared.now(), scrolled: 0 };
    match tray.assume_sni_available(true).spawn() {
        Ok(handle) => {
            log::note("tray.started", &format!("online={}", shared.tray_online.load(Ordering::Relaxed)));
            Some(handle)
        }
        Err(error) => {
            shared.tray_online.store(false, Ordering::Relaxed);
            log::warn("tray.unavailable", &format!("closing the window will quit: {error}"));
            None
        }
    }
}

/// Follows a new snapshot. The tooltip has no clock, so this only talks when something changed.
pub(crate) fn show(handle: &Handle<LullTray>, now: &Snapshot) {
    // None means the tray service has ended, and `start` or the callbacks logged why.
    let _ = handle.update(|tray| tray.shown = now.clone());
}

/// The one-time notice after the first close.
pub(crate) fn say_still_here(connection: &zbus::blocking::Connection, playing: bool) {
    let summary = if playing { "Lull is still playing" } else { "Lull is in the tray" };
    let body = "Click the ring in the tray to bring it back. Right-click it to quit.";
    let hints: HashMap<&str, Value<'_>> = HashMap::from([("desktop-entry", Value::from("lull"))]);
    let actions: Vec<&str> = Vec::new();
    let args = ("Lull", 0u32, "", summary, body, actions, hints, 8_000i32);
    let (name, path) = ("org.freedesktop.Notifications", "/org/freedesktop/Notifications");
    if let Err(error) = connection.call_method(Some(name), path, Some(name), "Notify", &args) {
        log::warn("tray.notice_failed", &error.to_string());
    }
}

impl LullTray {
    fn playing(&self) -> bool {
        matches!(self.shown.playback, Playback::Playing | Playback::Waiting)
    }
}

impl ksni::Tray for LullTray {
    fn id(&self) -> String {
        "lull".into()
    }

    fn title(&self) -> String {
        "Lull".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        icon::pixmaps(self.playing())
    }

    fn tool_tip(&self) -> ToolTip {
        let state = match (self.shown.playback, self.shown.problem) {
            (Playback::Waiting, Problem::NoDevice) => "Waiting for a sound device",
            (Playback::Waiting, _) => "Waiting for the speakers",
            (Playback::Playing, _) => "Playing",
            (Playback::Paused, _) => "Paused",
            (Playback::Empty, _) => "No sound",
        };
        let name = if self.shown.title.is_empty() { "Lull" } else { &self.shown.title };
        ToolTip {
            // Plasma reads tooltips as markup, and file names are not markup.
            title: escaped(name),
            description: format!("{state}, volume {}%", self.shown.volume),
            ..ToolTip::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let token = ksni::current_xdg_activation_token().as_deref().and_then(clean_token);
        self.shared.ask(Ask::ShowOrHide { token });
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        self.shared.ask(Ask::Toggle);
    }

    fn scroll(&mut self, delta: i32, orientation: Orientation) {
        if !matches!(orientation, Orientation::Vertical) {
            return;
        }
        // Touchpads send many small deltas, wheels send whole notches.
        self.scrolled += delta.clamp(-4 * NOTCH, 4 * NOTCH);
        let notches = self.scrolled / NOTCH;
        if notches != 0 {
            self.scrolled -= notches * NOTCH;
            self.shared.ask(Ask::VolumeBy(notches as i16 * VOLUME_STEP));
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: if self.playing() { "Pause" } else { "Play" }.into(),
                enabled: self.shown.total > 0,
                activate: Box::new(|tray: &mut Self| tray.shared.ask(Ask::Toggle)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Open Lull".into(),
                activate: Box::new(|tray: &mut Self| {
                    let token = ksni::current_xdg_activation_token().as_deref().and_then(clean_token);
                    tray.shared.ask(Ask::Show { token });
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|tray: &mut Self| tray.shared.ask(Ask::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn watcher_online(&self) {
        self.shared.tray_online.store(true, Ordering::Relaxed);
        log::note("tray.online", "");
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        self.shared.tray_online.store(false, Ordering::Relaxed);
        log::warn("tray.offline", &format!("{reason:?}"));
        true // Keep waiting: Plasma restarting brings it back.
    }
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
