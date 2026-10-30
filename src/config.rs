//! Build-time configuration for the engine.
//!
//! Everything in the engine runs at one sample format. Import converts to it,
//! output converts from it, and nothing in between has to check.

/// The in-memory and on-disk PCM sample type.
///
/// This is the only place that names the storage width. Code outside
/// [`crate::audio::AudioBuffer`] must not refer to `i16` directly; it works
/// in `f32` and converts at the buffer boundary.
pub type Pcm = i16;

/// Sample rate for every buffer in the engine, in Hz.
pub const SAMPLE_RATE: u32 = 48_000;

/// The largest channel count a buffer may have.
pub const MAX_CHANNELS: u16 = 2;
