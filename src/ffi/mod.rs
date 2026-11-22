//! The only unsafe code in Lull: bindings to two system libraries and two terminal calls.
//! Libraries load at run time, so building needs no -devel packages and no sudo.
//! Every signature and constant was checked against the Fedora 17.0 / 0.12 headers (docs/testing.md).

#![allow(unsafe_code)]

mod opusfile;
mod pulse;
mod tty;

pub use opusfile::Opus;
pub use pulse::Output;
pub use tty::RawInput;

use crate::error::{Error, Result};

/// Loads every library now, so a missing one stops startup instead of the first play.
pub fn check_libraries() -> Result<()> {
    opusfile::api().map(drop)?;
    pulse::api().map(drop)
}

fn open_library(name: &str) -> Result<libloading::Library> {
    // SAFETY: these are system C libraries whose initialisers have no preconditions.
    unsafe { libloading::Library::new(name) }.map_err(|e| Error::Library(format!("{name} ({e})")))
}

/// Declares a struct of function pointers plus the loader that fills it.
macro_rules! c_api {
    ($name:ident from $soname:literal { $(fn $func:ident($($arg:ty),*) $(-> $ret:ty)?;)* }) => {
        struct $name {
            $($func: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
            // The pointers above are valid only while this stays loaded.
            _library: libloading::Library,
        }

        impl $name {
            fn load() -> crate::error::Result<Self> {
                let library = super::open_library($soname)?;
                $(
                    // SAFETY: the declared type matches this symbol in the C header.
                    let $func = *unsafe {
                        library.get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>(stringify!($func))
                    }
                    .map_err(|e| crate::error::Error::Library(format!("{} ({e})", stringify!($func))))?;
                )*
                Ok(Self { $($func,)* _library: library })
            }
        }

        pub(super) fn api() -> crate::error::Result<&'static impl Sized> {
            loaded()
        }

        fn loaded() -> crate::error::Result<&'static $name> {
            static API: std::sync::OnceLock<crate::error::Result<$name>> = std::sync::OnceLock::new();
            API.get_or_init($name::load).as_ref().map_err(Clone::clone)
        }
    };
}
use c_api;
