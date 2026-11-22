//! MPRIS: what Plasma's media widget, lock screen and media keys use.

use std::collections::HashMap;
use std::sync::Arc;

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Str};

use super::{Ask, OBJECT, Shared, Snapshot, inside_music};
use crate::link::Playback;
use crate::timecode::RATE;

const MICROS: i128 = 1_000_000;

/// Serves `org.mpris.MediaPlayer2`.
pub(crate) struct Root(pub(crate) Arc<Shared>);

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        self.0.ask(Ask::Show { token: None });
    }

    fn quit(&self) {
        self.0.ask(Ask::Quit);
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn identity(&self) -> &str {
        "Lull"
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn desktop_entry(&self) -> &str {
        "lull"
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["file".into()]
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn supported_mime_types(&self) -> Vec<String> {
        vec!["audio/ogg".into(), "audio/opus".into()]
    }
}

/// Serves `org.mpris.MediaPlayer2.Player`.
pub(crate) struct Player(pub(crate) Arc<Shared>);

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn play(&self) {
        self.0.ask(Ask::Play);
    }

    fn pause(&self) {
        self.0.ask(Ask::Pause);
    }

    fn play_pause(&self) {
        self.0.ask(Ask::Toggle);
    }

    /// Pauses. The spec's Stop loses the place, which for an 8 hour sound would only ever hurt.
    fn stop(&self) {
        self.0.ask(Ask::Pause);
    }

    /// Nothing: there is no next sound (CanGoNext is false).
    fn next(&self) {}

    /// Nothing: there is no previous sound (CanGoPrevious is false).
    fn previous(&self) {}

    fn seek(&self, offset: i64) {
        self.0.ask(Ask::SeekBy(frames(offset)));
    }

    fn set_position(&self, track: ObjectPath<'_>, position: i64) {
        let now = self.0.now();
        let Ok(frame) = u64::try_from(frames(position)) else { return };
        // The spec says to ignore a stale track or a position outside it.
        if track.as_str() == track_path(&now) && frame <= now.total {
            self.0.ask(Ask::SeekTo { track: now.track, frame });
        }
    }

    fn open_uri(&self, uri: &str) -> zbus::fdo::Result<()> {
        let file = file_of(uri).and_then(|path| inside_music(&path, &self.0.music));
        let file = file.ok_or_else(|| zbus::fdo::Error::AccessDenied("only sounds inside ~/Music".into()))?;
        self.0.ask(Ask::Open(file));
        Ok(())
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        status(self.0.now().playback).to_owned()
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        metadata_of(&self.0.now())
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        f64::from(self.0.now().volume) / 100.0
    }

    #[zbus(property)]
    fn set_volume(&self, volume: f64) {
        if volume.is_finite() {
            self.0.ask(Ask::Volume((volume.clamp(0.0, 1.0) * 100.0).round() as u8));
        }
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        micros(self.0.now().position)
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    /// Only 1.0 exists, and the spec says 0 means pause.
    #[zbus(property)]
    fn set_rate(&self, rate: f64) {
        if rate == 0.0 {
            self.0.ask(Ask::Pause);
        }
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_go_next(&self) -> bool {
        false
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_go_previous(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.0.now().total > 0
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.0.now().total > 0
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.0.now().total > 0
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}

/// Emits what differs between `old` and `new`.
pub(crate) fn announce(connection: &zbus::blocking::Connection, old: &Snapshot, new: &Snapshot, jumped: bool) {
    let Ok(reference) = connection.object_server().interface::<_, Player>(OBJECT) else { return };
    let (player, emitter) = (reference.get(), reference.signal_emitter());
    let result = zbus::block_on(async {
        if old.playback != new.playback {
            player.playback_status_changed(emitter).await?;
        }
        if (old.track, old.total, &old.title, &old.detail) != (new.track, new.total, &new.title, &new.detail) {
            player.metadata_changed(emitter).await?;
        }
        if old.volume != new.volume {
            player.volume_changed(emitter).await?;
        }
        if (old.total > 0) != (new.total > 0) {
            player.can_play_changed(emitter).await?;
            player.can_pause_changed(emitter).await?;
            player.can_seek_changed(emitter).await?;
        }
        if jumped {
            Player::seeked(emitter, micros(new.position)).await?;
        }
        zbus::Result::Ok(())
    });
    if let Err(error) = result {
        crate::log::warn("remote.signal_failed", &error.to_string());
    }
}

fn status(playback: Playback) -> &'static str {
    match playback {
        // Waiting still means "press pause to stop it", so it reads as playing.
        Playback::Playing | Playback::Waiting => "Playing",
        Playback::Paused => "Paused",
        Playback::Empty => "Stopped",
    }
}

fn track_path(now: &Snapshot) -> String {
    if now.total == 0 {
        "/org/mpris/MediaPlayer2/TrackList/NoTrack".to_owned()
    } else {
        format!("/local/lull/track/{}", now.track)
    }
}

fn metadata_of(now: &Snapshot) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    if let Ok(path) = OwnedObjectPath::try_from(track_path(now)) {
        map.insert("mpris:trackid".to_owned(), OwnedValue::from(path.into_inner()));
    }
    if now.total > 0 {
        map.insert("mpris:length".to_owned(), OwnedValue::from(micros(now.total)));
        map.insert("xesam:title".to_owned(), OwnedValue::from(Str::from(now.title.clone())));
        if !now.detail.is_empty() {
            // Plasma prints the artist as the second line, where the window has "with thunder".
            let artists = zbus::zvariant::Value::from(vec![now.detail.clone()]);
            if let Ok(artists) = OwnedValue::try_from(artists) {
                map.insert("xesam:artist".to_owned(), artists);
            }
        }
    }
    map
}

fn micros(frames: u64) -> i64 {
    (i128::from(frames) * MICROS / i128::from(RATE)) as i64
}

fn frames(micros: i64) -> i64 {
    // Can't overflow: the result is smaller than the input.
    (i128::from(micros) * i128::from(RATE) / MICROS) as i64
}

/// The path in a `file://` URI.
fn file_of(uri: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    if uri.len() > 3 * 4096 {
        return None;
    }
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let mut bytes = Vec::with_capacity(rest.len());
    let mut input = rest.bytes();
    while let Some(byte) = input.next() {
        bytes.push(if byte == b'%' {
            let digit = |c: u8| char::from(c).to_digit(16);
            (digit(input.next()?)? * 16 + digit(input.next()?)?) as u8
        } else {
            byte
        });
    }
    (!bytes.contains(&0)).then(|| std::ffi::OsString::from_vec(bytes).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_convert_without_overflow() {
        assert_eq!(micros(48_000), 1_000_000);
        assert_eq!(frames(-30_000_000), -1_440_000);
        assert!(frames(i64::MAX) > 0 && frames(i64::MIN) < 0);
    }

    #[test]
    fn file_uris_decode_and_others_are_refused() {
        assert_eq!(file_of("file:///m/Rain%20%288h%29.opus"), Some("/m/Rain (8h).opus".into()));
        assert_eq!(file_of("file://localhost/m/a.opus"), Some("/m/a.opus".into()));
        for bad in ["http://x/a.opus", "file://host/a", "file:///a%2", "file:///a%zz", "file:///a%00b", "/a"] {
            assert_eq!(file_of(bad), None, "{bad}");
        }
    }
}
