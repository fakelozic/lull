//! Arguments and environment, checked once at startup. Nothing else reads the environment.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::timecode;

/// Shown by `--help`.
pub const USAGE: &str = "\
Lull: a small player for focus sounds

  lull                 open the window with the last sound, or the first .opus in ~/Music, paused
  lull FILE            open this Opus file
  lull --at 47:26      open at this time
  lull --play          start playing straight away
  lull --no-window     control it from the terminal instead

  Closing the window keeps it in the tray. Running lull again brings it back.
  Tray: click shows or hides, middle-click plays or pauses, scroll changes the volume.

  Space        play or pause
  Left, Right  back or forward 30 seconds
  Down, Up     quieter or louder
  Ctrl+Q       fade out, remember the place and quit (also q in the terminal)
";

/// What the person asked for.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    /// Print the usage.
    Help,
    /// Print the version.
    Version,
    /// Run the player.
    Run(Options),
    /// Be the window process. Only Lull starts this.
    Window {
        /// The launch's id, for the log.
        run: String,
        /// When this window was asked for, in ms since 1970, to log how long opening took.
        asked: Option<u64>,
    },
}

/// Options for a run.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// A file given on the command line.
    pub file: Option<PathBuf>,
    /// A start time in seconds.
    pub at: Option<u64>,
    /// Start playing at once.
    pub play: bool,
    /// Use the terminal, not a window.
    pub no_window: bool,
}

/// Folders taken from the environment.
#[derive(Debug, PartialEq, Eq)]
pub struct Paths {
    /// The home folder.
    pub home: PathBuf,
    /// Where the log files go.
    pub logs: PathBuf,
    /// Where focus sounds live. Only ever read.
    pub music: PathBuf,
    /// Lull's own folder for its state file.
    pub state: PathBuf,
    /// Where the single-instance lock goes.
    pub runtime: PathBuf,
    /// Wayland's permission to take focus, from whatever launched us.
    pub token: Option<String>,
    /// KDE's settings, read for reduced motion.
    pub desktop: PathBuf,
}

/// Parses arguments, not including the program name.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Request> {
    let mut args = args.into_iter();
    let mut options = Options::default();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-h" | "--help") => return Ok(Request::Help),
            Some("-V" | "--version") => return Ok(Request::Version),
            Some("--play") => options.play = true,
            Some("--no-window") => options.no_window = true,
            Some("--window") => {
                let run = args.next().and_then(|value| value.into_string().ok()).unwrap_or_default();
                let run = run.chars().filter(char::is_ascii_alphanumeric).take(16).collect();
                let asked = args.next().and_then(|value| value.to_str().and_then(|text| text.parse().ok()));
                return Ok(Request::Window { run, asked });
            }
            Some("--at") => {
                let seconds = args.next().and_then(|value| value.to_str().and_then(timecode::parse));
                options.at = Some(seconds.ok_or_else(|| Error::Usage("--at needs a time like 47:26".into()))?);
            }
            Some(flag) if flag.starts_with('-') => {
                return Err(Error::Usage(format!("Unknown option {flag}. Try lull --help")));
            }
            _ if options.file.is_some() => return Err(Error::Usage("Give one file at a time".into())),
            _ => options.file = Some(PathBuf::from(arg)),
        }
    }
    Ok(Request::Run(options))
}

impl Paths {
    /// Reads the real environment.
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    /// Builds the paths from any lookup. Relative values are ignored, as the XDG spec says.
    pub fn from_lookup(get: impl Fn(&OsStr) -> Option<OsString>) -> Result<Self> {
        let folder = |name: &str| get(OsStr::new(name)).map(PathBuf::from).filter(|path| path.is_absolute());
        let home = folder("HOME").ok_or_else(|| Error::Usage("HOME is not set to a folder".into()))?;
        let state = folder("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")).join("lull");
        let runtime = folder("XDG_RUNTIME_DIR").unwrap_or_else(|| state.clone());
        let desktop = folder("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join("kdeglobals");
        let token = ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"]
            .into_iter()
            .find_map(|name| get(OsStr::new(name)).and_then(|value| value.into_string().ok()))
            .and_then(|value| crate::remote::clean_token(&value));
        Ok(Self { token, music: home.join("Music"), logs: state.join("logs"), home, state, runtime, desktop })
    }

    /// True if animations are turned off in KDE's settings. No file means animations are on.
    pub fn reduced_motion(&self) -> bool {
        // kdeglobals is normally about 10 KB.
        crate::private_file::read(&self.desktop, 512 * 1024)
            .is_ok_and(|text| animations_off(&String::from_utf8_lossy(&text)))
    }
}

/// KDE's "Animation speed: instant" is `AnimationDurationFactor=0` under `[KDE]`.
fn animations_off(settings: &str) -> bool {
    let mut in_kde = false;
    for line in settings.lines().map(str::trim) {
        if line.starts_with('[') {
            in_kde = line == "[KDE]";
        } else if in_kde && let Some(value) = line.strip_prefix("AnimationDurationFactor=") {
            return value.parse() == Ok(0.0f32);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_file_a_time_and_play() {
        let got = parse(["rain.opus", "--at", "47:26", "--play"].map(OsString::from)).unwrap();
        let want = Options { file: Some("rain.opus".into()), at: Some(2846), play: true, no_window: false };
        assert_eq!(got, Request::Run(want));
    }

    #[test]
    fn refuses_bad_arguments() {
        for bad in [vec!["--at"], vec!["--at", "x"], vec!["--nope"], vec!["a", "b"]] {
            assert!(parse(bad.into_iter().map(OsString::from)).is_err());
        }
    }

    #[test]
    fn honours_kde_animations_off() {
        assert!(animations_off("[KDE]\nAnimationDurationFactor=0\n"));
        assert!(!animations_off("[KDE]\nAnimationDurationFactor=0.5\n"));
        assert!(!animations_off("[Other]\nAnimationDurationFactor=0\n"));
    }

    #[test]
    fn ignores_relative_xdg_folders() {
        let paths = Paths::from_lookup(|name| match name.to_str() {
            Some("HOME") => Some("/home/a".into()),
            Some("XDG_STATE_HOME") => Some("relative".into()),
            _ => None,
        });
        assert_eq!(paths.unwrap().state, PathBuf::from("/home/a/.local/state/lull"));
    }
}
