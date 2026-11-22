//! One copy of Lull: a second launch hands its arguments to the first and exits in milliseconds.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use zbus::fdo;

use super::{Ask, Launch, NAME, OBJECT, Shared, clean_token, inside_music};

const INTERFACE: &str = "local.lull.Control1";

/// Serves `local.lull.Control1`.
pub(crate) struct Control(pub(crate) Arc<Shared>);

#[zbus::interface(name = "local.lull.Control1")]
impl Control {
    /// `token` and `file` may be empty. `at` is seconds, or -1.
    fn show(&self, token: &str, play: bool, at: i64, file: &str) -> fdo::Result<()> {
        if self.0.stopping.load(Ordering::Relaxed) {
            return Err(fdo::Error::Failed("shutting down".into()));
        }
        let file = match file {
            "" => None,
            path => Some(
                inside_music(Path::new(path), &self.0.music)
                    .ok_or_else(|| fdo::Error::AccessDenied("only sounds inside ~/Music".into()))?,
            ),
        };
        self.0.ask(Ask::Launch(Launch { token: clean_token(token), play, at: u64::try_from(at).ok(), file }));
        Ok(())
    }
}

/// What became of handing over.
#[derive(Debug, PartialEq, Eq)]
pub enum Handoff {
    /// The running Lull took it.
    Done,
    /// It refused the file.
    Refused,
    /// Nobody answered: still starting, shutting down, or no bus.
    NotReady,
}

/// Asks the running Lull to take this launch over.
pub fn hand_over(launch: &Launch) -> Handoff {
    let file = launch.file.as_deref().map(absolute);
    let Some(file) = file.as_deref().map_or(Some(""), Path::to_str) else { return Handoff::Refused };
    let connect = || zbus::blocking::connection::Builder::session()?.method_timeout(super::TIMEOUT).build();
    let Ok(connection) = connect() else { return Handoff::NotReady };
    let at = launch.at.and_then(|seconds| i64::try_from(seconds).ok()).unwrap_or(-1);
    let body = (launch.token.as_deref().unwrap_or(""), launch.play, at, file);
    match connection.call_method(Some(NAME), OBJECT, Some(INTERFACE), "Show", &body) {
        Ok(_) => Handoff::Done,
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str().ends_with(".AccessDenied") => Handoff::Refused,
        Err(_) => Handoff::NotReady,
    }
}

fn absolute(path: &Path) -> PathBuf {
    path.canonicalize().or_else(|_| std::path::absolute(path)).unwrap_or_else(|_| path.to_owned())
}
