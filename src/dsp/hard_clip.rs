//! Hard clip to the unit range.

/// Clamps every sample to `-1.0..=1.0`. The last thing before the
/// output: a sum past full scale would wrap at the DAC or be clamped by
/// the driver, depending on the backend; clipping here makes every
/// device and the offline bounce behave the same, and tells the user
/// they are over, as hardware does.
///
/// No state, so nothing but the frames to take.
pub fn hard_clip(frames: &mut [f32]) {
    for s in frames.iter_mut() {
        *s = s.clamp(-1.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hard_clip_clamps_to_the_unit_range() {
        let mut frames = vec![1.5, -3.0, 0.5, 1.0];
        hard_clip(&mut frames);
        assert_eq!(frames, vec![1.0, -1.0, 0.5, 1.0]);
    }
}
