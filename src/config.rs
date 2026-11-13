//! The one sample format everything runs at, so nothing between the file
//! and the device converts or checks.

/// Named only here, so an `i16` build for low-power hardware stays a
/// one-line change.
pub type Pcm = f32;

/// In Hz.
pub const SAMPLE_RATE: u32 = 48_000;

pub const MAX_CHANNELS: u16 = 2;
