//! The audio device, over `cpal`. It lives beside the examples because
//! which machine the sampler runs on is the caller's knowledge; the library
//! builds with no audio dependency.
//!
//! [`Device`] captures and plays from one callback on one clock, as
//! `Renderer::render` takes them, in `f32` at the engine's rate. A device
//! that cannot is refused, not bridged or converted.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{DeviceId, DuplexStreamConfig};
use rtrb::{Consumer, RingBuffer};
use thiserror::Error;

use generations::engine::OUTPUT_CHANNELS;
use generations::engine::SAMPLE_RATE;

/// Stream errors kept for the control side before the oldest is dropped.
const ERROR_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeviceError {
    #[error("no duplex device")]
    NoDuplexDevice,
    #[error("audio backend: {0}")]
    Backend(String),
}

impl From<cpal::Error> for DeviceError {
    fn from(e: cpal::Error) -> Self {
        Self::Backend(e.to_string())
    }
}

#[derive(Debug)]
pub struct Device {
    device: cpal::Device,
    config: DuplexStreamConfig,
}

impl Device {
    /// ALSA's `default`, which is PipeWire on a desktop, at the device's
    /// own capture width. Asked for by id: cpal's default input and output
    /// devices have one direction each, and the first duplex device it
    /// lists can be `null`, which hears nothing.
    pub fn default_duplex() -> Result<Self, DeviceError> {
        let id: DeviceId = "alsa:default".parse()?;
        let device = cpal::default_host()
            .device_by_id(&id)
            .filter(|device| device.supports_duplex())
            .ok_or(DeviceError::NoDuplexDevice)?;
        let mut config = device.default_duplex_config()?;
        config.sample_rate = SAMPLE_RATE;
        config.output_channels = OUTPUT_CHANNELS as u16;
        Ok(Self { device, config })
    }

    /// The width of a capture frame, which `engine::create` is given.
    pub fn capture_channels(&self) -> usize {
        usize::from(self.config.input_channels)
    }

    /// `render` gets a period of interleaved capture and fills the same
    /// frames of interleaved stereo.
    pub fn start(
        self,
        mut render: impl FnMut(&[f32], &mut [f32]) + Send + 'static,
    ) -> Result<Stream, DeviceError> {
        let (mut errors_in, errors) = RingBuffer::new(ERROR_CAPACITY);
        let inner = self.device.build_duplex_stream(
            self.config,
            move |capture: &[f32], out: &mut [f32], _| render(capture, out),
            // Runs on the audio thread, so the error is queued for
            // `Stream::error` rather than handled here; a full queue drops it.
            move |e| {
                let _ = errors_in.push(e);
            },
            None,
        )?;
        inner.start()?;
        Ok(Stream { inner, errors })
    }
}

/// Dropping it stops the stream and drops whatever the callback owned.
pub struct Stream {
    /// Kept so it keeps running. `cpal` stops it on drop; nothing reads it.
    #[allow(dead_code)]
    inner: cpal::Stream,
    errors: Consumer<cpal::Error>,
}

impl Stream {
    /// Oldest first.
    pub fn error(&mut self) -> Option<cpal::Error> {
        self.errors.pop().ok()
    }
}
