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

/// Channels in the output the mixer renders: always stereo. A mono sample
/// is heard on both.
pub const OUTPUT_CHANNELS: usize = 2;

/// Upper bound on sample audio resident in RAM for playback, in bytes.
///
/// Playback reads only resident audio, so everything a pad can trigger
/// must fit under this. Loading past it is an error, not an eviction. Set
/// generously for now; the real number depends on the device's RAM.
pub const SAMPLE_RAM_BUDGET: usize = 1024 * 1024 * 1024;

/// Number of sounds that can play at once.
///
/// A fixed count bounds the work in one audio callback. When every voice
/// is busy, starting another steals one rather than failing. Set
/// generously for now; the cost per voice grows as voices gain pitch and
/// processing.
pub const VOICE_COUNT: usize = 64;
const _: () = assert!(VOICE_COUNT > 0, "the voice table cannot be empty");
