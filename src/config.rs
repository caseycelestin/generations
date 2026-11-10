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

/// Channels the capture device delivers, and the width of every frame on
/// the input ring. The device is refused if it cannot run at this count,
/// as the output is; which of them an input is, is chosen at `Engine::input`.
pub const INPUT_CHANNELS: usize = 2;

/// Samples an input ring holds between the capture and output callbacks:
/// a few periods, so the two clocks have room to disagree before a frame
/// is dropped.
pub const INPUT_CAPACITY: usize = INPUT_CHANNELS * 4096;

/// Samples that can play at once: the slots in the renderer's sample
/// mixer.
///
/// A fixed count bounds the work in one audio callback. When every slot
/// is taken, playing another sample steals the oldest rather than
/// failing. Set generously for now; the cost per slot grows as playback
/// gains pitch and processing.
pub const SAMPLE_PLAYBACK_COUNT: usize = 64;

/// Inputs that can be heard at once: the slots in the renderer's input
/// mixer. Each is its own ring; starting another when every slot is
/// taken steals the oldest, as with samples.
pub const INPUT_COUNT: usize = 2;

/// Commands the engine can send the renderer before the renderer has had
/// a period to take them. A press that finds the ring full is dropped,
/// which is the right failure for a real-time system; with periods in the
/// hundreds per second and presses in the tens, it does not happen by hand.
pub const COMMAND_CAPACITY: usize = 64;
