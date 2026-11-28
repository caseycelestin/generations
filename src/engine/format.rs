//! The one sample format everything runs at, so nothing between the file
//! and the device converts or checks.

/// In Hz.
pub const SAMPLE_RATE: u32 = 48_000;

/// Always stereo.
pub const OUTPUT_CHANNELS: usize = 2;

pub(crate) type Frame = [f32; OUTPUT_CHANNELS];
