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

/// The widest input the engine takes: the most channels one capture
/// device may deliver per frame.
pub const MAX_INPUT_CHANNELS: usize = 8;

/// The longest period, in frames, the engine takes from a device: the
/// most frames one callback may deliver. A bound, not a device fact; the
/// caller gets the period from the device and the engine refuses one
/// past this. It is what makes the input rings, which are sized from the
/// period, fixed overhead with a worst case known at compile time rather
/// than audio-sized memory for the ledger: see `input::MAX_RING_BYTES`.
/// 4096 frames is 85 ms at 48 kHz, already more latency than a sampler
/// wants.
pub const MAX_PERIOD_FRAMES: usize = 4096;
