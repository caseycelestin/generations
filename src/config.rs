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

/// The engine's sample type: on disk, in RAM and on the render path, one
/// `f32`. Decided 2026-11-09 (roadmap, Audio); `audio_v2` is its first
/// user. Takes over the name `Pcm` when the last caller of the `i16`
/// format ports, so nothing outside this file ever names `f32` as the
/// format: an `i16` build for low-powered hardware stays a one-line
/// change here.
pub type PcmV2 = f32;

/// Sample rate for every buffer in the engine, in Hz.
pub const SAMPLE_RATE: u32 = 48_000;

/// The largest channel count a buffer may have.
pub const MAX_CHANNELS: u16 = 2;

/// Channels in the output the mixer renders: always stereo. A mono sample
/// is heard on both.
pub const OUTPUT_CHANNELS: usize = 2;

/// Number of sounds that can play at once.
///
/// A fixed count bounds the work in one audio callback. When every voice
/// is busy, starting another steals one rather than failing. Set
/// generously for now; the cost per voice grows as voices gain pitch and
/// processing.
pub const VOICE_COUNT: usize = 64;
const _: () = assert!(VOICE_COUNT > 0, "the voice table cannot be empty");

/// Commands the engine can send the renderer before the renderer has had
/// a period to take them. A press that finds the ring full is dropped,
/// which is the right failure for a real-time system; with periods in the
/// hundreds per second and presses in the tens, it does not happen by hand.
pub const COMMAND_CAPACITY: usize = 64;
