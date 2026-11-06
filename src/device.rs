//! The audio device: the hardware, and the thread it runs.
//!
//! This is the only code in the crate that knows `cpal` exists and the only
//! code that knows there is an audio thread. A [`Device`] is the system's
//! default output, checked once for the engine's one format; [`Device::start`]
//! takes a callback and returns a [`Stream`] that calls it once per period
//! from the audio thread, for as long as the stream is held. What the
//! callback does is the caller's business; the one that makes sound moves
//! an engine's [`Output`](crate::Output) in and fills from it.
//!
//! The engine never touches this module. Which machine the sampler is on,
//! and whether it has a device at all, is the caller's knowledge.
//!
//! There is one implementation. `cpal` is itself the abstraction over ALSA,
//! PipeWire, JACK and the rest; a trait of our own waits for a second
//! implementation.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, StreamConfig};
use thiserror::Error;

use crate::config::{OUTPUT_CHANNELS, SAMPLE_RATE};

/// Reasons a device cannot be opened or started.
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

/// An output device that can run at the engine's format, not yet running.
pub struct Device {
    device: cpal::Device,
    config: StreamConfig,
}

impl Device {
    /// The system's default output device, at [`SAMPLE_RATE`], stereo, `f32`.
    ///
    /// There is no resampling: a device that cannot run at the engine's
    /// rate is refused, as the roadmap says. The period size is the
    /// backend's default for now.
    pub fn default_output() -> Result<Self, DeviceError> {
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
        Ok(Self { device, config })
    }

    /// Starts the device. From now until the returned [`Stream`] is
    /// dropped, the audio thread calls `render` once per period with an
    /// interleaved stereo slice to fill.
    pub fn start(
        self,
        mut render: impl FnMut(&mut [f32]) + Send + 'static,
    ) -> Result<Stream, DeviceError> {
        let stream = self.device.build_output_stream(
            self.config,
            move |out: &mut [f32], _| render(out),
            // Runs on the audio thread when the stream itself fails (the
            // device went away, the backend hit an xrun it could not hide).
            // Nothing to do about it from here yet; say so and carry on.
            |e| eprintln!("audio stream error: {e}"),
            None,
        )?;
        stream.play()?;
        Ok(Stream { inner: stream })
    }
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// A running output stream. Playing while held; dropping it stops it.
pub struct Stream {
    /// Kept so it keeps running. `cpal` stops it on drop.
    #[allow(dead_code)] // held for its lifetime; read by nothing
    inner: cpal::Stream,
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream").finish_non_exhaustive()
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
        let stream = Device::default_output()
            .unwrap()
            .start(|out| out.fill(0.0))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(stream);
    }
}
