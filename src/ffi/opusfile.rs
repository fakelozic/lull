//! A safe wrapper around the system's official Opus file decoder (libopusfile).

use std::ffi::{c_int, c_void};
use std::path::Path;
use std::ptr::{self, NonNull};

use crate::audio::Source;
use crate::error::{Error, Result};
use crate::media::{Cursor, Media};

type File = *mut c_void;
type Ptr = *mut c_void;

/// `OpusFileCallbacks`. No close: the cursor belongs to `Opus`.
#[repr(C)]
struct Callbacks {
    read: unsafe extern "C" fn(Ptr, *mut u8, c_int) -> c_int,
    seek: unsafe extern "C" fn(Ptr, i64, c_int) -> c_int,
    tell: unsafe extern "C" fn(Ptr) -> i64,
    close: Option<unsafe extern "C" fn(Ptr) -> c_int>,
}

super::c_api!(Api from "libopusfile.so.0" {
    fn op_open_callbacks(Ptr, *const Callbacks, *const u8, usize, *mut c_int) -> File;
    fn op_free(File);
    fn op_seekable(File) -> c_int;
    fn op_link_count(File) -> c_int;
    fn op_channel_count(File, c_int) -> c_int;
    fn op_pcm_total(File, c_int) -> i64;
    fn op_pcm_seek(File, i64) -> c_int;
    fn op_read_float_stereo(File, *mut f32, c_int) -> c_int;
});

unsafe extern "C" fn read_bytes(stream: Ptr, out: *mut u8, count: c_int) -> c_int {
    let Ok(count) = usize::try_from(count) else { return -1 };
    if count == 0 || out.is_null() {
        return 0;
    }
    // SAFETY: `stream` is the Cursor owned by the Opus making this call, which holds &mut. The
    // library gives a buffer of `count` bytes. Zeroed first, as a slice must be initialised.
    let (cursor, out) = unsafe {
        ptr::write_bytes(out, 0, count);
        (&mut *stream.cast::<Cursor>(), std::slice::from_raw_parts_mut(out, count))
    };
    cursor.read(out).map_or(-1, |read| read as c_int)
}

unsafe extern "C" fn seek_bytes(stream: Ptr, offset: i64, whence: c_int) -> c_int {
    // SAFETY: as in `read_bytes`.
    let cursor = unsafe { &mut *stream.cast::<Cursor>() };
    if cursor.seek(offset, whence) { 0 } else { -1 }
}

unsafe extern "C" fn tell_bytes(stream: Ptr) -> i64 {
    // SAFETY: as in `read_bytes`.
    unsafe { &*stream.cast::<Cursor>() }.tell()
}

const CALLBACKS: Callbacks = Callbacks { read: read_bytes, seek: seek_bytes, tell: tell_bytes, close: None };

/// Opening may read this much. It parses every link's headers, so this caps their memory.
const OPEN_ALLOWANCE: u64 = 1024 * 1024;
/// One decode or seek may read this much. A normal seek needs under 1.5 MB.
const CALL_ALLOWANCE: u64 = 3 * 1024 * 1024;
/// A chained file is joined recordings. This is plenty of them.
const MOST_LINKS: c_int = 64;
const MOST_CHANNELS: c_int = 8;
/// 24 days. Beyond it positions are nonsense, and the maths on them should never overflow.
const MOST_FRAMES: i64 = 100_000_000_000;

/// A damaged spot. The decoder resumes after it, so skip it, up to a limit.
const OP_HOLE: c_int = -3;
const MAX_HOLES: u32 = 64;
/// Decoded and thrown away before every seek target (500 ms).
///
/// op_pcm_seek alone is not exact: about half of short forward seeks returned the wrong audio while
/// reporting the right position, and the rest were off by 3-5%. With this it is exact in every
/// trial and costs 1-2 ms (docs/decisions.md#seeking). Don't remove it.
const SEEK_PREROLL: u64 = 24_000;

/// One decoder on an opened sound: 48 kHz float stereo.
pub struct Opus {
    api: &'static Api,
    file: NonNull<c_void>,
    /// From Box::into_raw. The library keeps this pointer until op_free.
    cursor: NonNull<Cursor>,
    total: u64,
}

// SAFETY: an OggOpusFile and its Cursor have no thread affinity and are only used through &mut.
unsafe impl Send for Opus {}

impl Opus {
    /// Opens a regular, seekable Opus file. Reads only; never writes.
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_media(&Media::open(path, None)?)
    }

    /// Another decoder on the same opened sound, with its own place in it.
    pub fn from_media(media: &Media) -> Result<Self> {
        let api = loaded()?;
        let mut start = Box::new(Cursor::new(media.clone()));
        start.allow(OPEN_ALLOWANCE);
        let cursor = NonNull::from(Box::leak(start));
        let mut code = 0;
        // SAFETY: the cursor lives until Drop, after op_free. The library copies the callback table.
        let raw = unsafe { (api.op_open_callbacks)(cursor.as_ptr().cast(), &CALLBACKS, ptr::null(), 0, &mut code) };
        let Some(file) = NonNull::new(raw) else {
            // SAFETY: opening failed, so the library no longer holds the pointer Box::leak gave us.
            drop(unsafe { Box::from_raw(cursor.as_ptr()) });
            return Err(Error::Track(format!("opening failed: {}", reason(code))));
        };
        // Built before the checks below, so Drop frees everything if they fail.
        let mut opus = Self { api, file, cursor, total: 0 };
        // SAFETY: the file is open and owned by `opus`.
        let (seekable, total, links) = unsafe {
            let file = file.as_ptr();
            ((api.op_seekable)(file), (api.op_pcm_total)(file, -1), (api.op_link_count)(file))
        };
        if seekable == 0 || !(1..=MOST_FRAMES).contains(&total) {
            return Err(Error::Track("it has no usable length, so it can't seek or loop".into()));
        }
        if links > MOST_LINKS {
            return Err(Error::Track(format!(
                "it is {links} recordings joined together. Lull takes up to {MOST_LINKS}"
            )));
        }
        for link in 0..links {
            // SAFETY: the file is open and `link` is in range.
            if unsafe { (api.op_channel_count)(file.as_ptr(), link) } > MOST_CHANNELS {
                return Err(Error::Track(format!("it has more than {MOST_CHANNELS} channels")));
            }
        }
        opus.total = total as u64;
        Ok(opus)
    }

    fn allow_one_call(&mut self) {
        // SAFETY: the library touches the cursor only during our calls into it, and we hold &mut.
        unsafe { self.cursor.as_mut() }.allow(CALL_ALLOWANCE);
    }
}

impl Source for Opus {
    fn total(&self) -> u64 {
        self.total
    }

    fn seek(&mut self, frame: u64) -> Result<()> {
        let frame = frame.min(self.total);
        let from = frame.saturating_sub(SEEK_PREROLL);
        self.allow_one_call();
        // SAFETY: the file is open and we hold &mut.
        let code = unsafe { (self.api.op_pcm_seek)(self.file.as_ptr(), from as i64) };
        if code < 0 {
            return Err(Error::Track(format!("seeking failed: {}", reason(code))));
        }
        let mut discard = [0.0; 4096];
        let mut left = frame - from;
        while left > 0 {
            let count = left.min(discard.len() as u64 / 2) as usize;
            self.read(&mut discard[..count * 2])?;
            left -= count as u64;
        }
        Ok(())
    }

    fn read(&mut self, out: &mut [f32]) -> Result<()> {
        let mut done = 0;
        let mut holes = 0;
        self.allow_one_call();
        while done + 1 < out.len() {
            let rest = &mut out[done..];
            let room = rest.len().min(c_int::MAX as usize) as c_int;
            // SAFETY: `rest` holds at least `room` floats; the decoder writes no more than that.
            let got = unsafe { (self.api.op_read_float_stereo)(self.file.as_ptr(), rest.as_mut_ptr(), room) };
            match got {
                1.. => done += got as usize * 2,
                OP_HOLE if holes < MAX_HOLES => holes += 1,
                0 => return Err(Error::Track("the file ended earlier than it should".into())),
                _ => return Err(Error::Track(format!("reading failed: {}", reason(got)))),
            }
        }
        Ok(())
    }
}

impl Drop for Opus {
    fn drop(&mut self) {
        // SAFETY: we own both and this runs once. The library lets go of the cursor in op_free.
        unsafe {
            (self.api.op_free)(self.file.as_ptr());
            drop(Box::from_raw(self.cursor.as_ptr()));
        }
    }
}

fn reason(code: c_int) -> String {
    let plain = match code {
        -128 => "the file could not be read",
        -132 | -133 | -135 => "it is not an Opus file",
        -134 => "it uses an unsupported Opus version",
        -136 | -137 | -139 => "the file is damaged",
        -138 => "the file can't seek",
        _ => "the decoder failed",
    };
    format!("{plain} (opusfile {code})")
}
