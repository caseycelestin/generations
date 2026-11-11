//! The arithmetic every piece shares: frames to stereo, stereo into the
//! period, the period into range.
//!
//! Three free functions over slices, with no state. A producer piece
//! turns what it holds into stereo frames with [`stereo`] and sums them
//! into the period with [`add`]; the renderer ends the period with
//! [`hard_clip`]. Whether the frames came from a loaded sample or the
//! capture device makes no difference here.

use crate::audio::ChannelLayout;
use crate::config::Pcm;
use crate::render::Renderer;

/// One output frame: a sample per side.
pub type Frame = [Pcm; Renderer::CHANNELS];

/// Which sample of a source frame each output side hears, as an index
/// into the frame. A mono source is `[0, 0]`; a stereo one as stored is
/// `[0, 1]`; a source with more channels could pick any two.
pub type Sides = [usize; Renderer::CHANNELS];

/// The sides for frames of `layout`, heard as stored: mono on both
/// sides, stereo left and right.
pub fn sides_for(layout: ChannelLayout) -> Sides {
    match layout {
        ChannelLayout::Mono => [0, 0],
        ChannelLayout::Stereo => [0, 1],
    }
}

/// Stereo frames from `samples`, interleaved `width` wide: each side
/// takes the frame's sample at its index in `sides`. A trailing partial
/// frame is dropped.
pub fn stereo(samples: &[Pcm], width: usize, sides: Sides) -> impl Iterator<Item = Frame> {
    debug_assert!(
        sides.iter().all(|&s| s < width),
        "a side names a channel the frame has"
    );
    samples
        .chunks_exact(width)
        .map(move |frame| sides.map(|s| frame[s]))
}

/// Adds `frames` into `out`, an interleaved stereo slice, one frame per
/// output frame from the start of `out`. Stops at whichever runs out
/// first and leaves the rest of `out` untouched. Nothing is clipped here.
///
/// Adds rather than writes, so sources sum by each being added into the
/// same slice.
pub fn add(out: &mut [Pcm], frames: impl IntoIterator<Item = Frame>) {
    debug_assert_eq!(
        out.len() % Renderer::CHANNELS,
        0,
        "output is not whole frames"
    );
    let (out, _) = out.as_chunks_mut::<{ Renderer::CHANNELS }>();
    for (out, frame) in out.iter_mut().zip(frames) {
        for (o, s) in out.iter_mut().zip(frame) {
            *o += s;
        }
    }
}

/// Clamps every sample to `-1.0..=1.0`. The last thing before the
/// output: a sum past full scale would wrap at the DAC or be clamped by
/// the driver, depending on the backend; clipping here makes every
/// device and the offline bounce behave the same, and tells the user
/// they are over, as hardware does.
pub fn hard_clip(frames: &mut [Pcm]) {
    for s in frames.iter_mut() {
        *s = s.clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_is_heard_on_both_sides() {
        let frames: Vec<Frame> = stereo(&[0.5, -0.5], 1, sides_for(ChannelLayout::Mono)).collect();
        assert_eq!(frames, vec![[0.5, 0.5], [-0.5, -0.5]]);
    }

    #[test]
    fn stereo_is_heard_as_stored() {
        let frames: Vec<Frame> =
            stereo(&[0.5, -0.5, 0.0, 0.5], 2, sides_for(ChannelLayout::Stereo)).collect();
        assert_eq!(frames, vec![[0.5, -0.5], [0.0, 0.5]]);
    }

    #[test]
    fn any_two_channels_of_a_wide_frame_can_be_the_sides() {
        let frames: Vec<Frame> =
            stereo(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8], 4, [3, 1]).collect();
        assert_eq!(frames, vec![[0.4, 0.2], [0.8, 0.6]]);
    }

    #[test]
    fn a_partial_frame_is_dropped() {
        assert_eq!(stereo(&[0.1, 0.2, 0.3], 2, [0, 1]).count(), 1);
    }

    #[test]
    fn frames_are_summed_into_what_is_there() {
        let mut out = vec![1.0; 4];
        add(&mut out, [[0.5, -0.5], [0.2, 0.4]]);
        assert_eq!(out, vec![1.5, 0.5, 1.2, 1.4]);
    }

    #[test]
    fn a_short_source_leaves_the_rest_untouched() {
        let mut out = vec![0.0; 6];
        add(&mut out, [[1.0, 1.0]]);
        assert_eq!(out, vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_long_source_is_cut_at_the_period() {
        let mut out = vec![0.0; 2];
        add(&mut out, [[1.0, 1.0], [2.0, 2.0]]);
        assert_eq!(out, vec![1.0, 1.0]);
    }

    #[test]
    fn a_hard_clip_clamps_to_the_unit_range() {
        let mut frames = vec![1.5, -3.0, 0.5, 1.0];
        hard_clip(&mut frames);
        assert_eq!(frames, vec![1.0, -1.0, 0.5, 1.0]);
    }
}
