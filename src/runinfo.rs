//! Facts for the log: what is running, on what, how much memory, and whether the last run died.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Version, build and system, for the first line of each run.
pub fn describe() -> String {
    let built = std::env::current_exe()
        .and_then(fs::metadata)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |time| time.as_secs());
    let os = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let os = os.lines().find_map(|line| line.strip_prefix("PRETTY_NAME=")).unwrap_or("?").trim_matches('"');
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    format!("version={} built={built} os={os} kernel={}", env!("CARGO_PKG_VERSION"), kernel.trim())
}

/// A process's fair share of memory (PSS) in MB, or 0 if it can't be read.
pub fn memory_mb(pid: u32) -> u64 {
    fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
        .ok()
        .and_then(|text| {
            let line = text.lines().find_map(|line| line.strip_prefix("Pss:"))?;
            line.split_whitespace().next()?.parse::<u64>().ok()
        })
        .map_or(0, |kb| kb / 1024)
}

/// A file that exists only while Lull runs. Found at start, it means the last run was cut off
/// (crash, kill -9, out of memory, power cut): deaths that can't write their own log line.
pub struct Running {
    path: PathBuf,
}

impl Running {
    /// Marks this run as started and returns what the last one left, if it died.
    /// Call only while holding the single-instance lock.
    pub fn begin(folder: &Path, run: &str) -> (Self, Option<String>) {
        let path = folder.join("running");
        let left = match crate::private_file::read(&path, 512) {
            Ok(bytes) => Some(
                String::from_utf8_lossy(&bytes)
                    .chars()
                    .filter(|c| c.is_ascii_graphic() || *c == ' ')
                    .take(120)
                    .collect(),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(format!("(its marker is unreadable: {error})")),
        };
        let mark = format!("run={run} pid={}", std::process::id());
        if let Err(error) = crate::private_file::replace(&path, mark.as_bytes()) {
            crate::log::warn("log.marker_failed", &error.to_string());
        }
        (Self { path }, left)
    }

    /// Marks this run as ended in a known way.
    pub fn end(self) {
        if let Err(error) = fs::remove_file(&self.path) {
            crate::log::warn("log.marker_failed", &error.to_string());
        }
    }
}
