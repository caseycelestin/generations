//! A level that moves without clicking.
//!
//! A [`Gain`] is a linear factor and the value it is moving to. Setting it
//! only names the target; the next period ramps from where it was to
//! there, frame by frame, so a jump in level is spread over a period
//! instead of landing on one sample, which the ear hears as a click.

use crate::render::dsp::math::Frame;

/// A linear level, 0 silent and 1 as given, ramped to its target over the
/// period after it is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gain {
    /// The level the last frame was given.
    now: f32,
    /// The level to reach by the end of the next period.
    target: f32,
}

impl Gain {
    /// At `level` already, with nothing to ramp.
    pub const fn at(level: f32) -> Self {
        Self {
            now: level,
            target: level,
        }
    }

    /// Moves to `level` over the next period.
    pub fn set(&mut self, level: f32) {
        self.target = level;
    }

    /// `frames`, a period of `period` frames, each scaled by the level,
    /// ramping from where it was to the target and reaching it on the last
    /// frame. A period that ends early stops short of the target and the
    /// next starts from the target anyway: a step too small to hear.
    pub fn apply(
        &mut self,
        period: usize,
        frames: impl Iterator<Item = Frame>,
    ) -> impl Iterator<Item = Frame> {
        let (from, to) = (self.now, self.target);
        self.now = to;
        let step = if period == 0 {
            0.0
        } else {
            (to - from) / period as f32
        };
        frames.enumerate().map(move |(n, frame)| {
            let level = from + step * (n + 1) as f32;
            frame.map(|s| s * level)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ones(n: usize) -> impl Iterator<Item = Frame> {
        std::iter::repeat_n([1.0, 1.0], n)
    }

    #[test]
    fn a_steady_level_scales_every_frame() {
        let mut g = Gain::at(0.5);
        let out: Vec<Frame> = g.apply(2, ones(2)).collect();
        assert_eq!(out, vec![[0.5, 0.5], [0.5, 0.5]]);
    }

    #[test]
    fn a_new_level_is_ramped_to_over_one_period_then_held() {
        let mut g = Gain::at(0.0);
        g.set(1.0);
        let out: Vec<Frame> = g.apply(4, ones(4)).collect();
        assert_eq!(out, vec![[0.25; 2], [0.5; 2], [0.75; 2], [1.0; 2]]);
        let out: Vec<Frame> = g.apply(2, ones(2)).collect();
        assert_eq!(out, vec![[1.0; 2], [1.0; 2]]);
    }

    #[test]
    fn silence_is_still_every_frame_given() {
        let mut g = Gain::at(0.0);
        assert_eq!(g.apply(3, ones(3)).count(), 3);
    }
}
