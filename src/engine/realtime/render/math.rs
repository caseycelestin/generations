//! Arithmetic on periods: free functions over slices, no state.

use super::Frame;

/// Adds rather than writes, so sources sum. Stops at whichever runs out
/// first.
pub(crate) fn add(out: &mut [Frame], frames: &[Frame]) {
    for (out, frame) in out.iter_mut().zip(frames) {
        for (o, s) in out.iter_mut().zip(frame) {
            *o += s;
        }
    }
}

/// Clamps every sample to `-1.0..=1.0`, last thing before the output, so
/// every device and an offline bounce clip the same way.
pub(crate) fn hard_clip(frames: &mut [Frame]) {
    for s in frames.as_flattened_mut() {
        *s = s.clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_summed_into_what_is_there() {
        let mut out = vec![[1.0, 1.0]; 2];
        add(&mut out, &[[0.5, -0.5], [0.2, 0.4]]);
        assert_eq!(out, vec![[1.5, 0.5], [1.2, 1.4]]);
    }

    #[test]
    fn a_short_source_leaves_the_rest_untouched() {
        let mut out = vec![[0.0, 0.0]; 3];
        add(&mut out, &[[1.0, 1.0]]);
        assert_eq!(out, vec![[1.0, 1.0], [0.0, 0.0], [0.0, 0.0]]);
    }

    #[test]
    fn a_long_source_is_cut_at_the_period() {
        let mut out = vec![[0.0, 0.0]; 1];
        add(&mut out, &[[1.0, 1.0], [2.0, 2.0]]);
        assert_eq!(out, vec![[1.0, 1.0]]);
    }

    #[test]
    fn a_hard_clip_clamps_to_the_unit_range() {
        let mut frames = vec![[1.5, -3.0], [0.5, 1.0]];
        hard_clip(&mut frames);
        assert_eq!(frames, vec![[1.0, -1.0], [0.5, 1.0]]);
    }
}
