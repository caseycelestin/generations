//! The audio device: what drives the renderer live.
//!
//! A [`Device`] is an open output stream on the system's default audio
//! device, at the engine's one sample rate, stereo. The stream owns a
//! [`Renderer`] and calls its `render` from the audio callback once per
//! period, so the renderer is heard. There is one implementation, over
//! `cpal`, which is itself the abstraction over ALSA, PipeWire, JACK and
//! the rest; a trait of our own waits for a second implementation.
//!
//! The device is the only code in the crate that knows `cpal` exists, and
//! the only code that knows there is an audio thread. Everything the
//! callback runs is the renderer.
//!
//! This file holds the shape so far; starting the stream comes next.
//!
//! [`Renderer`]: crate::playback::Renderer

use cpal::Stream;

/// An open output stream, playing while it is held.
///
/// Dropping it stops the stream and, with it, the renderer inside.
pub struct Device {
    /// The stream, kept so it keeps running. `cpal` stops it on drop.
    #[allow(dead_code)] // held for its lifetime; read by nothing
    stream: Stream,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device").finish_non_exhaustive()
    }
}
