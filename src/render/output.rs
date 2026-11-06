//! The engine's audio output, as a thing the caller can drive.
//!
//! An [`Output`] is the render side of one engine, handed out once by
//! `Engine::output` so that whoever owns the audio thread can own it. It
//! has one job: fill a frame. What fills it is the engine's business,
//! which is why this is the only type the `render` module exports.

use crate::render::Renderer;

/// The render side of an engine: one method, which fills a frame.
///
/// Move it into a device callback and call [`fill`](Output::fill) once per
/// period for live sound, or call it in a loop for the same frames offline.
/// It never allocates, locks, touches disk, fails or reads a clock, so it
/// is safe to call with a hard deadline.
///
/// **Drop whatever drives this before dropping the engine.** A voice still
/// ringing holds a reference to its audio; if the engine goes first, that
/// reference is the last one and the buffer is freed on the audio thread
/// when the voice ends.
pub struct Output {
    renderer: Renderer,
}

impl Output {
    pub(crate) fn new(renderer: Renderer) -> Self {
        Self { renderer }
    }

    /// Fills `out`, an interleaved stereo `f32` slice, with the next period
    /// of sound, replacing whatever it held.
    ///
    /// Commands the engine has sent since the last call are applied first,
    /// so a play or stop lands at a period boundary.
    pub fn fill(&mut self, out: &mut [f32]) {
        self.renderer.render(out);
    }

    /// Whether voice `id` is playing. For tests.
    #[cfg(test)]
    pub(crate) fn is_playing(&self, id: crate::render::VoiceId) -> bool {
        self.renderer.is_playing(id)
    }

    /// How many voices are playing. For tests.
    #[cfg(test)]
    pub(crate) fn playing(&self) -> usize {
        self.renderer.playing()
    }
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Output").finish_non_exhaustive()
    }
}
