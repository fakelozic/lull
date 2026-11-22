//! One opened sound file, as the decoder may see it. Safe code; `ffi::Opus` reads through it.
//!
//! Three jobs, all from the review of step 2:
//! - Open once. Every check and both decoders use that one open file, so swapping the path
//!   afterwards changes nothing, and a pipe or device can't hang us.
//! - Hide embedded art. libopusfile copies every tag into RAM, for each decoder: a 30 MB cover
//!   took Lull to 140 MB. Audio needs none of it (the gain is in the first header), so a big tag
//!   block is presented to the decoder as an empty one.
//! - Bound the rest. Each decoder call may read only so much (`Cursor`), which caps what any
//!   hostile file can make the decoder allocate.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;
use std::sync::Arc;

use crate::error::{Error, Result};

/// Tag blocks up to this size are left alone.
const PLAIN_TAGS: u64 = 64 * 1024;
/// Tag pages looked at before giving up. Covers 400 MB of art.
const MOST_TAG_PAGES: usize = 7_000;
/// Surround is fine. 255 channels would cost 25 MB of decoder state.
const MOST_CHANNELS: u8 = 8;
/// An Ogg page header with a full segment table.
const PAGE_HEADER: usize = 27 + 255;

#[derive(Debug)]
struct Shared {
    file: File,
    /// Replaces the file's first `skip` bytes. Empty when nothing is hidden.
    prefix: Vec<u8>,
    skip: u64,
    len: u64,
}

/// An opened sound. Cheap to clone; clones are the same open file.
#[derive(Debug, Clone)]
pub struct Media(Arc<Shared>);

struct Page {
    flags: u8,
    serial: [u8; 4],
    body: u64,
    size: u64,
    /// A packet ends on this page.
    ends_packet: bool,
    /// And it is the last thing on the page.
    ends_on_packet: bool,
}

impl Media {
    /// Opens `path` for reading. With `within`, the file that was really opened must be inside that
    /// folder, however the path got there.
    pub fn open(path: &Path, within: Option<&Path>) -> Result<Self> {
        let shown = path.display();
        let failed = |why: &dyn std::fmt::Display| Error::Track(format!("{shown}: {why}"));
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(path)
            .map_err(|e| failed(&e))?;
        let meta = file.metadata().map_err(|e| failed(&e))?;
        if !meta.is_file() {
            return Err(Error::Track(format!("{shown} is not a file")));
        }
        if let Some(root) = within {
            let root = root.canonicalize().map_err(|e| failed(&e))?;
            let real = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).map_err(|e| failed(&e))?;
            if !real.starts_with(root) {
                return Err(Error::Track(format!("{shown} points outside the music folder")));
            }
        }
        let (prefix, skip) = without_big_tags(&file, meta.len()).map_err(|e| failed(&e))?;
        let len = prefix.len() as u64 + meta.len().saturating_sub(skip);
        Ok(Self(Arc::new(Shared { file, prefix, skip, len })))
    }

    /// The length the decoder sees.
    pub fn len(&self) -> u64 {
        self.0.len
    }

    /// True if there is nothing to read.
    pub fn is_empty(&self) -> bool {
        self.0.len == 0
    }

    fn read_at(&self, out: &mut [u8], at: u64) -> io::Result<usize> {
        let shared = &*self.0;
        let fixed = shared.prefix.len() as u64;
        if at < fixed {
            let part = &shared.prefix[at as usize..];
            let count = part.len().min(out.len());
            out[..count].copy_from_slice(&part[..count]);
            return Ok(count);
        }
        shared.file.read_at(out, at - fixed + shared.skip)
    }
}

/// One decoder's own place in a `Media`, with an allowance.
#[derive(Debug)]
pub struct Cursor {
    media: Media,
    at: u64,
    allowance: u64,
}

impl Cursor {
    /// Starts at the beginning with nothing allowed.
    pub fn new(media: Media) -> Self {
        Self { media, at: 0, allowance: 0 }
    }

    /// Allows `bytes` of reading from now, replacing what was left.
    pub fn allow(&mut self, bytes: u64) {
        self.allowance = bytes;
    }

    /// Reads, or returns `None` on an I/O error or when the allowance is used up.
    pub fn read(&mut self, out: &mut [u8]) -> Option<usize> {
        if self.allowance == 0 {
            return None;
        }
        let room = out.len().min(usize::try_from(self.allowance).unwrap_or(usize::MAX));
        loop {
            match self.media.read_at(&mut out[..room], self.at) {
                Ok(count) => {
                    self.at += count as u64;
                    self.allowance -= count as u64;
                    return Some(count);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return None,
            }
        }
    }

    /// Moves like fseek. False if that would land before the start.
    pub fn seek(&mut self, offset: i64, whence: i32) -> bool {
        let from = match whence {
            libc::SEEK_SET => 0,
            libc::SEEK_CUR => self.at,
            libc::SEEK_END => self.media.len(),
            _ => return false,
        };
        match from.checked_add_signed(offset) {
            Some(at) if i64::try_from(at).is_ok() => {
                self.at = at;
                true
            }
            _ => false,
        }
    }

    /// The place now.
    pub fn tell(&self) -> i64 {
        self.at as i64
    }
}

fn page_at(file: &File, at: u64, len: u64) -> io::Result<Option<Page>> {
    let mut header = [0u8; PAGE_HEADER];
    if at.saturating_add(27) > len {
        return Ok(None);
    }
    file.read_exact_at(&mut header[..27], at)?;
    let segments = usize::from(header[26]);
    if &header[..5] != b"OggS\0" || at + 27 + segments as u64 > len {
        return Ok(None);
    }
    file.read_exact_at(&mut header[27..27 + segments], at + 27)?;
    let lacing = &header[27..27 + segments];
    let body = lacing.iter().map(|&size| u64::from(size)).sum();
    Ok(Some(Page {
        flags: header[5],
        serial: [header[14], header[15], header[16], header[17]],
        body,
        size: 27 + segments as u64 + body,
        ends_packet: lacing.iter().any(|&size| size < 255),
        ends_on_packet: lacing.last().is_some_and(|&size| size < 255),
    }))
}

/// Returns (replacement for the start, how much of the file it replaces).
/// Anything unexpected is left alone: the decoder judges it, inside the read allowance.
fn without_big_tags(file: &File, len: u64) -> io::Result<(Vec<u8>, u64)> {
    let untouched = (Vec::new(), 0);
    let Some(head) = page_at(file, 0, len)? else { return Ok(untouched) };
    if head.flags & 2 == 0 || head.body < 19 || head.size > len {
        return Ok(untouched);
    }
    let mut start = vec![0; head.size as usize];
    file.read_exact_at(&mut start, 0)?;
    let body = &start[(head.size - head.body) as usize..];
    if &body[..8] != b"OpusHead" {
        return Ok(untouched);
    }
    if body[9] > MOST_CHANNELS {
        return Err(io::Error::other(format!("it has {} channels. Lull plays up to {MOST_CHANNELS}", body[9])));
    }
    let mut at = head.size;
    for _ in 0..MOST_TAG_PAGES {
        let Some(page) = page_at(file, at, len)? else { return Ok(untouched) };
        if page.serial != head.serial || page.flags & 2 != 0 || at + page.size > len {
            return Ok(untouched);
        }
        at += page.size;
        if page.ends_packet {
            if !page.ends_on_packet || at - head.size <= PLAIN_TAGS {
                return Ok(untouched);
            }
            let sequence = u32::from_le_bytes([start[18], start[19], start[20], start[21]]).wrapping_add(1);
            start.extend_from_slice(&empty_tags(head.serial, sequence));
            return Ok((start, at));
        }
    }
    Err(io::Error::other("its embedded data is too large"))
}

/// A page holding a valid OpusTags packet with no tags.
fn empty_tags(serial: [u8; 4], sequence: u32) -> Vec<u8> {
    let mut packet = b"OpusTags".to_vec();
    packet.extend_from_slice(&4u32.to_le_bytes());
    packet.extend_from_slice(b"lull");
    packet.extend_from_slice(&0u32.to_le_bytes());

    let mut page = b"OggS\0\0".to_vec();
    page.extend_from_slice(&0i64.to_le_bytes());
    page.extend_from_slice(&serial);
    page.extend_from_slice(&sequence.to_le_bytes());
    page.extend_from_slice(&[0; 4]);
    page.push(1);
    page.push(packet.len() as u8);
    page.extend_from_slice(&packet);
    let crc = ogg_crc(&page);
    page[22..26].copy_from_slice(&crc.to_le_bytes());
    page
}

fn ogg_crc(bytes: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &byte in bytes {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { crc << 1 ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_checksums_match_a_real_encoder() {
        let file = include_bytes!("../tests/fixtures/noise.opus");
        let mut page = file[..27 + usize::from(file[26]) + usize::from(file[27])].to_vec();
        let stored = [page[22], page[23], page[24], page[25]];
        page[22..26].fill(0);
        assert_eq!(ogg_crc(&page).to_le_bytes(), stored);
    }
}
