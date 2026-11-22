//! Lull: a small player for focus sounds. This library holds everything; `main.rs` only wires it.
//! All unsafe code lives in `ffi`, so that is the one place to audit.

#![deny(unsafe_code)]

pub mod app;
pub mod audio;
pub mod config;
pub mod error;
pub mod ffi;
pub mod front;
pub mod host;
pub mod inbox;
pub mod keys;
pub mod link;
pub mod log;
pub mod media;
pub mod private_file;
pub mod remote;
pub mod runinfo;
pub mod sounds;
pub mod state;
pub mod terminal;
pub mod timecode;
pub mod track;
pub mod visual;
pub mod window;
