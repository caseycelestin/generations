//! Something heard: whatever produces frames for the period.
//!
//! The renderer wants two things from a source and nothing about how it
//! gets them: the next frames, and whether there will ever be more. What
//! it does with the frames, adding them into the period, is the caller's.

use crate::render::dsp::math::Frame;

/// Produces stereo frames, a period at a time, until it is finished.
pub trait Source {
    /// The next `frames` stereo frames, or fewer if it has fewer to give.
    /// Producing them moves the source past them.
    fn frames(&mut self, frames: usize) -> impl Iterator<Item = Frame>;

    /// Whether it will never produce a frame again, so whoever holds it
    /// may let it go.
    fn finished(&self) -> bool;
}
