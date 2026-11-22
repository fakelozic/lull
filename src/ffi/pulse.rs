//! A safe wrapper around one PulseAudio playback stream (PipeWire serves this API on Fedora).
//!
//! It uses the plain main loop on the caller's thread, not pa_simple: every wait has a timeout,
//! so a stuck sound server can't hang play, pause or quit, and underruns can be counted.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::timecode::RATE;

type Ptr = *mut c_void;
type Notify = Option<unsafe extern "C" fn(Ptr, Ptr)>;
type Free = Option<unsafe extern "C" fn(Ptr)>;

#[repr(C)]
struct SampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

const _: () = assert!(size_of::<SampleSpec>() == 12 && size_of::<BufferAttr>() == 20);

super::c_api!(Api from "libpulse.so.0" {
    fn pa_mainloop_new() -> Ptr;
    fn pa_mainloop_free(Ptr);
    fn pa_mainloop_get_api(Ptr) -> Ptr;
    fn pa_mainloop_prepare(Ptr, c_int) -> c_int;
    fn pa_mainloop_poll(Ptr) -> c_int;
    fn pa_mainloop_dispatch(Ptr) -> c_int;
    fn pa_context_new(Ptr, *const c_char) -> Ptr;
    fn pa_context_connect(Ptr, *const c_char, c_uint, *const c_void) -> c_int;
    fn pa_context_disconnect(Ptr);
    fn pa_context_unref(Ptr);
    fn pa_context_get_state(Ptr) -> c_int;
    fn pa_context_errno(Ptr) -> c_int;
    fn pa_strerror(c_int) -> *const c_char;
    fn pa_stream_new(Ptr, *const c_char, *const SampleSpec, *const c_void) -> Ptr;
    fn pa_stream_connect_playback(Ptr, *const c_char, *const BufferAttr, c_uint, *const c_void, Ptr) -> c_int;
    fn pa_stream_disconnect(Ptr) -> c_int;
    fn pa_stream_unref(Ptr);
    fn pa_stream_get_state(Ptr) -> c_int;
    fn pa_stream_get_buffer_attr(Ptr) -> *const BufferAttr;
    fn pa_stream_get_device_name(Ptr) -> *const c_char;
    fn pa_stream_writable_size(Ptr) -> usize;
    fn pa_stream_write(Ptr, *const c_void, usize, Free, i64, c_int) -> c_int;
    fn pa_stream_set_underflow_callback(Ptr, Notify, Ptr);
});

const FLOAT32LE: c_int = 5;
const CONTEXT_READY: c_int = 4;
const CONTEXT_FAILED: c_int = 5;
const STREAM_READY: c_int = 2;
const STREAM_FAILED: c_int = 3;
/// Never start a second sound server if PipeWire is down.
const NO_AUTOSPAWN: c_uint = 1;
/// Makes the buffer target cover the whole delay, device included.
const ADJUST_LATENCY: c_uint = 0x2000;
const SEEK_RELATIVE: c_int = 0;
const FRAME_BYTES: usize = 8;
/// With no real device, PipeWire moves streams to this sink, which discards everything.
/// Playing into it would lose the listener's place.
const DUMMY_SINK: &CStr = c"auto_null";

unsafe extern "C" fn count_underflow(_stream: Ptr, data: Ptr) {
    // SAFETY: `data` is the AtomicU64 boxed in Output, which outlives the stream.
    let counter = unsafe { &*data.cast::<AtomicU64>() };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// One connected playback stream: f32 stereo at 48 kHz, following the default device.
/// Not Send: create, use and drop it on the audio thread.
pub struct Output {
    api: &'static Api,
    main_loop: Ptr,
    context: Ptr,
    stream: Ptr,
    underflows: Box<AtomicU64>,
}

impl Output {
    /// Connects and waits until ready, for at most `timeout`.
    /// `buffer_frames` is how much audio the server keeps queued. `title` shows in the system mixer.
    pub fn connect(title: &str, buffer_frames: u32, timeout: Duration) -> Result<Self> {
        let api = loaded()?;
        let deadline = Instant::now() + timeout;
        let null = ptr::null_mut();
        let mut out =
            Self { api, main_loop: null, context: null, stream: null, underflows: Box::new(AtomicU64::new(0)) };
        out.open_context(deadline)?;
        out.open_stream(title, buffer_frames, deadline)?;
        Ok(out)
    }

    fn open_context(&mut self, deadline: Instant) -> Result<()> {
        // SAFETY: each pointer is null-checked before use. Drop frees whatever was created.
        let started = unsafe {
            self.main_loop = (self.api.pa_mainloop_new)();
            if self.main_loop.is_null() {
                return Err(Error::Output("out of memory".into()));
            }
            self.context = (self.api.pa_context_new)((self.api.pa_mainloop_get_api)(self.main_loop), c"Lull".as_ptr());
            if self.context.is_null() {
                return Err(Error::Output("out of memory".into()));
            }
            (self.api.pa_context_connect)(self.context, ptr::null(), NO_AUTOSPAWN, ptr::null())
        };
        if started < 0 {
            return Err(self.failure());
        }
        loop {
            // SAFETY: the context is live until Drop.
            match unsafe { (self.api.pa_context_get_state)(self.context) } {
                CONTEXT_READY => return Ok(()),
                CONTEXT_FAILED.. => return Err(self.failure()),
                _ => self.wait_before(deadline)?,
            }
        }
    }

    fn open_stream(&mut self, title: &str, buffer_frames: u32, deadline: Instant) -> Result<()> {
        let name = CString::new(title.replace('\0', " ")).unwrap_or_else(|_| c"Lull".to_owned());
        let spec = SampleSpec { format: FLOAT32LE, rate: RATE as u32, channels: 2 };
        let auto = u32::MAX;
        let tlength = buffer_frames.saturating_mul(FRAME_BYTES as u32);
        let attr = BufferAttr { maxlength: auto, tlength, prebuf: auto, minreq: auto, fragsize: auto };
        let data = ptr::from_ref::<AtomicU64>(&self.underflows).cast_mut().cast::<c_void>();
        // SAFETY: the structs match the C layout and live through the calls. A null device means
        // "follow the default". The boxed counter outlives the stream (see Drop).
        let started = unsafe {
            self.stream = (self.api.pa_stream_new)(self.context, name.as_ptr(), &spec, ptr::null());
            if self.stream.is_null() {
                return Err(self.failure());
            }
            (self.api.pa_stream_set_underflow_callback)(self.stream, Some(count_underflow), data);
            (self.api.pa_stream_connect_playback)(
                self.stream,
                ptr::null(),
                &attr,
                ADJUST_LATENCY,
                ptr::null(),
                ptr::null_mut(),
            )
        };
        if started < 0 {
            return Err(self.failure());
        }
        while !self.is_ready()? {
            self.wait_before(deadline)?;
        }
        self.check_device()
    }

    fn is_ready(&self) -> Result<bool> {
        // SAFETY: both objects are live until Drop.
        let (context, stream) =
            unsafe { ((self.api.pa_context_get_state)(self.context), (self.api.pa_stream_get_state)(self.stream)) };
        if context >= CONTEXT_FAILED || stream >= STREAM_FAILED {
            return Err(self.failure());
        }
        Ok(stream == STREAM_READY)
    }

    /// Fails while the stream points at the dummy sink. The library updates the name when a stream moves.
    fn check_device(&self) -> Result<()> {
        // SAFETY: the stream is ready; the name is null or a string owned by it, read before any other call.
        let dummy = unsafe {
            let name = (self.api.pa_stream_get_device_name)(self.stream);
            !name.is_null() && CStr::from_ptr(name) == DUMMY_SINK
        };
        if dummy { Err(Error::NoDevice) } else { Ok(()) }
    }

    fn wait_before(&mut self, deadline: Instant) -> Result<()> {
        if Instant::now() >= deadline {
            return Err(Error::Output("the sound server did not answer in time".into()));
        }
        self.wait(Duration::from_millis(50))
    }

    /// Handles server events, sleeping for at most `timeout` if there are none.
    pub fn wait(&mut self, timeout: Duration) -> Result<()> {
        let micros = timeout.as_micros().min(c_int::MAX as u128) as c_int;
        // SAFETY: the loop is live and used from this thread only. Order: prepare, poll, dispatch.
        let code = unsafe {
            let mut code = (self.api.pa_mainloop_prepare)(self.main_loop, micros);
            if code >= 0 {
                code = (self.api.pa_mainloop_poll)(self.main_loop);
            }
            if code >= 0 {
                code = (self.api.pa_mainloop_dispatch)(self.main_loop);
            }
            code
        };
        if code < 0 { Err(self.failure()) } else { Ok(()) }
    }

    /// Frames the server wants now. Fails if the stream is gone.
    pub fn writable(&self) -> Result<usize> {
        self.is_ready()?;
        self.check_device()?;
        // SAFETY: the stream is live and ready.
        let bytes = unsafe { (self.api.pa_stream_writable_size)(self.stream) };
        if bytes == usize::MAX { Err(self.failure()) } else { Ok(bytes / FRAME_BYTES) }
    }

    /// Queues interleaved stereo samples. The library copies them, so the buffer is free afterwards.
    pub fn write(&mut self, samples: &[f32]) -> Result<()> {
        // SAFETY: pointer and byte count describe `samples`. A null free callback means "copy now".
        let code = unsafe {
            (self.api.pa_stream_write)(
                self.stream,
                samples.as_ptr().cast(),
                size_of_val(samples),
                None,
                0,
                SEEK_RELATIVE,
            )
        };
        if code < 0 { Err(self.failure()) } else { Ok(()) }
    }

    /// How much audio the server really keeps queued. It may differ from what was asked for.
    pub fn queue_frames(&self) -> Option<u32> {
        // SAFETY: the stream is ready. The struct is owned by it and only read here.
        unsafe {
            let attr = (self.api.pa_stream_get_buffer_attr)(self.stream);
            (!attr.is_null()).then(|| (*attr).tlength / FRAME_BYTES as u32).filter(|&frames| frames > 0)
        }
    }

    /// The device the stream is on now. It changes when the system moves the stream.
    pub fn device(&self) -> Option<&CStr> {
        // SAFETY: the stream is live; the string is owned by it and `wait`, the only thing that can
        // replace it, needs &mut self, so it can't change while this borrow lasts.
        unsafe {
            let name = (self.api.pa_stream_get_device_name)(self.stream);
            (!name.is_null()).then(|| CStr::from_ptr(name))
        }
    }

    /// How many times the server ran out of audio on this stream.
    pub fn underflows(&self) -> u64 {
        self.underflows.load(Ordering::Relaxed)
    }

    fn failure(&self) -> Error {
        if self.context.is_null() {
            return Error::Output("no connection".into());
        }
        // SAFETY: pa_strerror returns a static string, or null for an unknown code.
        let text = unsafe {
            let text = (self.api.pa_strerror)((self.api.pa_context_errno)(self.context));
            (!text.is_null()).then(|| CStr::from_ptr(text).to_string_lossy().into_owned())
        };
        Error::Output(text.unwrap_or_else(|| "the sound server failed".into()))
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        // SAFETY: released in reverse order. `underflows` is freed after this body, so after the stream.
        unsafe {
            if !self.stream.is_null() {
                (self.api.pa_stream_disconnect)(self.stream);
                (self.api.pa_stream_unref)(self.stream);
            }
            if !self.context.is_null() {
                (self.api.pa_context_disconnect)(self.context);
                (self.api.pa_context_unref)(self.context);
            }
            if !self.main_loop.is_null() {
                (self.api.pa_mainloop_free)(self.main_loop);
            }
        }
    }
}
