//! Finds focus sounds. Only ever reads.

use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The first `most` Opus files directly inside `folder`, by name.
/// It keeps only that many while it reads, so a folder of 150,000 files costs no more memory.
pub fn list(folder: &Path, most: usize) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(folder).map_err(|e| Error::Track(format!("{}: {e}", folder.display())))?;
    let mut first = BinaryHeap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("opus")) {
            continue;
        }
        let full = first.len() >= most;
        if full && first.peek().is_none_or(|last| path >= *last) {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        if full {
            first.pop();
        }
        first.push(path);
    }
    Ok(first.into_sorted_vec())
}

/// The name people see: the file name without its extension or control characters.
pub fn title(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or(path.as_os_str()).to_string_lossy();
    // A file name must not be able to send escape codes to the terminal.
    stem.chars().filter(|c| !c.is_control()).collect()
}

/// Splits a name into the big line and the quieter one under it:
/// "Rain on Window with Thunder (8h)" becomes ("Rain on Window", "with thunder").
pub fn headline(name: &str) -> (String, String) {
    let mut name = name.trim();
    // A note like "(8h)" at the end repeats what the length already says.
    if name.ends_with(')')
        && let Some(open) = name.rfind(" (")
    {
        name = name[..open].trim_end();
    }
    for (separator, lead) in [(" with ", "with "), (" - ", ""), (" – ", ""), (", ", "")] {
        if let Some((first, rest)) = name.split_once(separator)
            && !first.is_empty()
        {
            return (first.to_owned(), format!("{lead}{}", rest.to_lowercase()));
        }
    }
    (name.to_owned(), String::new())
}

/// Picks what to open: the file asked for, else the saved one if it still exists, else the first found.
pub fn choose(asked: Option<&Path>, saved: Option<&Path>, music: &Path) -> Result<PathBuf> {
    if let Some(path) = asked {
        return path.canonicalize().map_err(|e| Error::Track(format!("{}: {e}", path.display())));
    }
    if let Some(path) = saved.filter(|path| path.is_file()) {
        return Ok(path.to_owned());
    }
    list(music, 1)?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Track(format!("there are no .opus files in {}", music.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_keeps_only_the_first_few_by_name() {
        let folder = std::env::temp_dir().join(format!("lull-list-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        for name in ["d.opus", "a.OPUS", "c.opus", "b.opus", "notes.txt"] {
            std::fs::write(folder.join(name), "").unwrap();
        }
        assert_eq!(list(&folder, 2).unwrap(), [folder.join("a.OPUS"), folder.join("b.opus")]);
        assert_eq!(list(&folder, 9).unwrap().len(), 4);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn titles_cannot_inject_terminal_codes() {
        assert_eq!(title(Path::new("/m/Rain\x1b[2J.opus")), "Rain[2J");
    }

    #[test]
    fn names_split_into_two_lines() {
        let split = |name| headline(name);
        let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        assert_eq!(split("Rain on Window with Thunder (8h)"), pair("Rain on Window", "with thunder"));
        assert_eq!(split("Ocean - Night"), pair("Ocean", "night"));
        assert_eq!(split("Brown noise"), pair("Brown noise", ""));
        assert_eq!(split("(8h)"), pair("(8h)", ""));
    }
}
