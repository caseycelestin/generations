//! Audio in this crate is mono or stereo, and [`ChannelLayout`] says
//! which, for a buffer and for one source of an input alike. A raw channel
//! count exists only where a file hands one over, and becomes a layout
//! there or is refused.

use thiserror::Error;

use crate::config::MAX_CHANNELS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ChannelLayoutError {
    /// A channel count was zero or above [`MAX_CHANNELS`]: no layout.
    #[error("channel count {0} is not in 1..={MAX_CHANNELS}")]
    BadChannelCount(u16),
}

/// How many channels a frame holds and what they mean: one heard on both
/// sides, or two heard left and right. Interleaved stereo is left then
/// right everywhere, so a pair is always the two channels in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLayout {
    Mono,
    Stereo,
}

impl ChannelLayout {
    /// How many channels a frame of this layout holds.
    pub fn count(self) -> usize {
        match self {
            ChannelLayout::Mono => 1,
            ChannelLayout::Stereo => 2,
        }
    }
}

/// The layout of a raw channel count, as a file or decoder gives one.
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

/// The raw channel count of a layout, as a file header wants one.
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
