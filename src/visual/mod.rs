//! The signature visual: colour and the Ripples ring. Pure maths, ported from the HTML prototype.

mod colour;
mod ripples;

pub use colour::oklch;
pub use ripples::{Levels, Ripples, SIDE, warm_up};

/// How many frequency bands the ring follows.
pub const BANDS: usize = 24;
