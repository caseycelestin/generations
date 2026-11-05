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
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, Stream, StreamConfig};
use thiserror::Error;

use crate::config::{OUTPUT_CHANNELS, SAMPLE_RATE};
use crate::playback::Renderer;

/// Reasons a device cannot be started.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeviceError {
    /// The system has no default output device.
    #[error("no output device")]
    NoOutputDevice,
    /// The output device cannot run at the engine's one format.
    #[error("output device does not support {rate} Hz, {channels} channels, f32")]
    UnsupportedFormat { rate: u32, channels: u16 },
    /// The audio backend refused; the message is its own.
    #[error("audio backend: {0}")]
    Backend(String),
}

impl From<cpal::Error> for DeviceError {
    fn from(e: cpal::Error) -> Self {
        Self::Backend(e.to_string())
    }
}

/// An open output stream, playing while it is held.
///
/// Dropping it stops the stream and, with it, the renderer inside.
pub struct Device {
    /// The stream, kept so it keeps running. `cpal` stops it on drop.
    #[allow(dead_code)] // held for its lifetime; read by nothing
    stream: Stream,
}

impl Device {
    /// Opens the default output device at [`SAMPLE_RATE`], stereo, `f32`,
    /// moves `renderer` into its callback, and starts the stream.
    ///
    /// The period size is the backend's default for now. There is no
    /// resampling: a device that cannot run at the engine's rate is
    /// refused, as the roadmap says.
    pub fn start(mut renderer: Renderer) -> Result<Self, DeviceError> {
        let channels = OUTPUT_CHANNELS as u16;
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(DeviceError::NoOutputDevice)?;
        let supported = device
            .supported_output_configs()?
            .filter(|c| c.channels() == channels && c.sample_format() == SampleFormat::F32)
            .find_map(|c| c.try_with_sample_rate(SAMPLE_RATE))
            .ok_or(DeviceError::UnsupportedFormat {
                rate: SAMPLE_RATE,
                channels,
            })?;
        let config = StreamConfig {
            buffer_size: BufferSize::Default,
            ..supported.config()
        };
        let stream = device.build_output_stream(
            config,
            move |out: &mut [f32], _| renderer.render(out),
            // Runs on the audio thread when the stream itself fails (the
            // device went away, the backend hit an xrun it could not hide).
            // Nothing to do about it from here yet; say so and carry on.
            |e| eprintln!("audio stream error: {e}"),
            None,
        )?;
        stream.play()?;
        Ok(Self { stream })
    }
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a real output device, so CI skips it. Run by hand with
    /// `cargo test -- --ignored` on a machine with audio.
    #[test]
    #[ignore = "needs an audio device"]
    fn starts_and_stops_on_the_default_device() {
        let (_producer, consumer) = rtrb::RingBuffer::new(8);
        let device = Device::start(Renderer::new(consumer)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(device);
    }
}
