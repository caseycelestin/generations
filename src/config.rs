//! Build-time configuration for the engine.
//!
//! Everything in the engine runs at one sample format. Import converts to it,
//! output converts from it, and nothing in between has to check.

/// The engine's sample type: on disk, in RAM and on the render path, one
/// `f32`. Decided 2026-11-09 (roadmap, Audio). This is the only place
/// that names it, so nothing outside this file ever says `f32` as the
/// format: an `i16` build for low-powered hardware stays a one-line
/// change here.
pub type Pcm = f32;

/// Sample rate for every buffer in the engine, in Hz.
pub const SAMPLE_RATE: u32 = 48_000;

/// The largest channel count a buffer may have.
pub const MAX_CHANNELS: u16 = 2;

/// Channels in the output the mixer renders: always stereo. A mono sample
/// is heard on both.
pub const OUTPUT_CHANNELS: usize = 2;

/// The widest input the engine takes: the most channels one capture
/// device may deliver per frame.
pub const MAX_INPUT_CHANNELS: usize = 8;

/// Input sounds the engine holds at once, across every input: the slots
/// in the renderer's input mixer, one per mono or stereo source. Unlike
/// samples, an input sound is never stolen: `Engine::input` refuses an
/// input whose sources would not all fit until one is removed.
pub const MAX_INPUT_SOUNDS: usize = 8;

/// Samples that can play at once: the slots in the renderer's sample
/// mixer.
///
/// A fixed count bounds the work in one audio callback. When every slot
/// is taken, playing another sample steals the oldest rather than
/// failing. Set generously for now; the cost per slot grows as playback
/// gains pitch and processing.
pub const SAMPLE_PLAYBACK_COUNT: usize = 64;

/// Commands the engine can send the renderer before the renderer has had
/// a period to take them. A press that finds the ring full is dropped,
/// which is the right failure for a real-time system; with periods in the
/// hundreds per second and presses in the tens, it does not happen by hand.
pub const COMMAND_CAPACITY: usize = 64;
