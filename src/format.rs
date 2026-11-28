//! The one sample format everything runs at, so nothing between the file
//! and the device converts or checks. Audio is mono or stereo; a raw channel
//! count exists only where a file hands one over, and becomes a layout there
//! or is refused.

use thiserror::Error;

/// In Hz.
pub const SAMPLE_RATE: u32 = 48_000;

/// Always stereo.
pub const OUTPUT_CHANNELS: usize = 2;

pub(crate) type Frame = [f32; OUTPUT_CHANNELS];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ChannelLayoutError {
    #[error("channel count {0} is neither mono nor stereo")]
    BadChannelCount(u16),
}

/// Mono is heard on both sides. Interleaved stereo is left then right
/// everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLayout {
    Mono,
    Stereo,
}

impl ChannelLayout {
    pub fn count(self) -> usize {
        match self {
            ChannelLayout::Mono => 1,
            ChannelLayout::Stereo => 2,
        }
    }
}

impl TryFrom<u16> for ChannelLayout {
    type Error = ChannelLayoutError;

    fn try_from(channels: u16) -> Result<Self, ChannelLayoutError> {
        match channels {
            1 => Ok(ChannelLayout::Mono),
            2 => Ok(ChannelLayout::Stereo),
            _ => Err(ChannelLayoutError::BadChannelCount(channels)),
        }
    }
}

impl From<ChannelLayout> for u16 {
    fn from(layout: ChannelLayout) -> u16 {
        match layout {
            ChannelLayout::Mono => 1,
            ChannelLayout::Stereo => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChannelLayout::{Mono, Stereo};
    use super::*;

    #[test]
    fn a_channel_count_is_a_layout_or_refused() {
        assert_eq!(ChannelLayout::try_from(1), Ok(Mono));
        assert_eq!(ChannelLayout::try_from(2), Ok(Stereo));
        assert_eq!(
            ChannelLayout::try_from(0),
            Err(ChannelLayoutError::BadChannelCount(0))
        );
        assert_eq!(
            ChannelLayout::try_from(3),
            Err(ChannelLayoutError::BadChannelCount(3))
        );
        assert_eq!(u16::from(Stereo), 2);
    }
}
