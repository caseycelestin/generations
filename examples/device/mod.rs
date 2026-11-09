//! The audio device: the hardware, and the thread it runs.
//!
//! This lives beside the examples, not in the library. The engine never
//! touches a device: which machine the sampler is on, and whether it has an
//! audio device at all, is the caller's knowledge, so the caller owns the
//! only code that knows `cpal` exists and that there is an audio thread.
//! The library builds with no audio dependency.
//!
//! A [`Device`] is the system's default output, checked once for the
//! engine's rate and channel count; [`Device::start`] takes a callback and
//! returns a [`Stream`] that calls it once per period from the audio
//! thread, for as long as the stream is held. The callback that makes
//! sound moves an engine's output, its `Renderer`, in and renders from it.
//!
//! The callback always sees `f32`. A device that wants 16- or 32-bit
//! samples gets them converted in the callback, because `cpal` does
//! no conversion of its own and a raw ALSA `hw:` device (an I2S codec on
//! the target) often offers nothing else. The rate is never converted: a
//! device that cannot run at the engine's rate is refused, as the roadmap
//! says.
//!
//! There is one implementation. `cpal` is itself the abstraction over ALSA,
//! PipeWire, JACK and the rest; a trait of our own waits for a second
//! implementation.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig, SupportedStreamConfig};
use rtrb::{Consumer, RingBuffer};
use thiserror::Error;

use generations::config::{OUTPUT_CHANNELS, SAMPLE_RATE};

/// Frames of `f32` scratch the callback renders into before converting
/// to the device's format. A period larger than this is rendered in
/// pieces; nothing allocates on the audio thread either way.
const SCRATCH_FRAMES: usize = 4096;

/// Stream errors kept for the control side before the oldest is dropped.
const ERROR_CAPACITY: usize = 8;

/// Reasons a device cannot be opened or started.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeviceError {
    /// The system has no default output device.
    #[error("no output device")]
    NoOutputDevice,
    /// The device cannot run at the engine's rate and the channel count
    /// asked for in any sample format this code converts.
    #[error("device does not support {rate} Hz, {channels} channels, in f32, i16 or i32")]
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
    format: SampleFormat,
    config: StreamConfig,
}

impl Device {
    /// The system's default output device, at [`SAMPLE_RATE`], stereo.
    ///
    /// `f32` is preferred, since the renderer produces it; `i16` and `i32`
    /// are accepted and converted to. There is no resampling. The period
    /// size is the backend's default for now; a fixed size for the Pi
    /// would be set on the config here.
    pub fn default_output() -> Result<Self, DeviceError> {
        let channels = OUTPUT_CHANNELS as u16;
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(DeviceError::NoOutputDevice)?;
        let supported = pick(device.supported_output_configs()?, channels)?;
        Ok(Self {
            device,
            format: supported.sample_format(),
            config: supported.config(),
        })
    }

    /// Starts the device. From now until the returned [`Stream`] is
    /// dropped, the audio thread calls `render` once per period with an
    /// interleaved stereo `f32` slice to fill.
    pub fn start(
        self,
        render: impl FnMut(&mut [f32]) + Send + 'static,
    ) -> Result<Stream, DeviceError> {
        let (errors_in, errors) = RingBuffer::new(ERROR_CAPACITY);
        let inner = match self.format {
            SampleFormat::F32 => build::<f32>(&self.device, &self.config, render, errors_in),
            SampleFormat::I16 => build::<i16>(&self.device, &self.config, render, errors_in),
            SampleFormat::I32 => build::<i32>(&self.device, &self.config, render, errors_in),
            // `default_output` picked from these three.
            _ => unreachable!("device format was chosen from the convertible set"),
        }?;
        inner.play()?;
        Ok(Stream { inner, errors })
    }
}

/// The configuration to run at: the engine's rate, `channels` channels,
/// and the most convenient of the sample formats this code converts.
fn pick(
    configs: impl Iterator<Item = cpal::SupportedStreamConfigRange>,
    channels: u16,
) -> Result<SupportedStreamConfig, DeviceError> {
    configs
        .filter(|c| c.channels() == channels)
        .filter_map(|c| c.try_with_sample_rate(SAMPLE_RATE))
        .filter_map(|c| preference(c.sample_format()).map(|rank| (rank, c)))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, c)| c)
        .ok_or(DeviceError::UnsupportedFormat {
            rate: SAMPLE_RATE,
            channels,
        })
}

/// Lower is better: no conversion first, then the cheaper conversion.
fn preference(format: SampleFormat) -> Option<u8> {
    match format {
        SampleFormat::F32 => Some(0),
        SampleFormat::I16 => Some(1),
        SampleFormat::I32 => Some(2),
        _ => None,
    }
}

/// Builds the stream for one device sample type. The callback renders
/// `f32` into a scratch buffer allocated here, on the control thread, and
/// converts into the device's buffer; for `f32` that conversion is a copy.
fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut render: impl FnMut(&mut [f32]) + Send + 'static,
    mut errors: rtrb::Producer<cpal::Error>,
) -> Result<cpal::Stream, DeviceError> {
    let mut scratch = vec![0.0f32; SCRATCH_FRAMES * usize::from(config.channels)];
    Ok(device.build_output_stream(
        *config,
        move |out: &mut [T], _| {
            for chunk in out.chunks_mut(scratch.len()) {
                let rendered = &mut scratch[..chunk.len()];
                render(rendered);
                for (o, &sample) in chunk.iter_mut().zip(rendered.iter()) {
                    *o = T::from_sample(sample);
                }
            }
        },
        // Runs on the audio thread when the stream itself fails (the
        // device went away, the backend hit an xrun it could not hide).
        // Nothing here may block, so the error is queued for the control
        // side to read with `Stream::error`; a full queue drops it.
        move |e| {
            let _ = errors.push(e);
        },
        None,
    )?)
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("format", &self.format)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// A running stream. Running while held; dropping it
/// stops it, and drops whatever the callback owned.
pub struct Stream {
    /// Kept so it keeps running. `cpal` stops it on drop; nothing reads it.
    #[allow(dead_code)]
    inner: cpal::Stream,
    /// Errors the audio thread reported, oldest first.
    errors: Consumer<cpal::Error>,
}

impl Stream {
    /// The oldest stream error not yet taken, if any. The audio thread
    /// queues them without blocking; the control side reads them here
    /// whenever it likes.
    pub fn error(&mut self) -> Option<cpal::Error> {
        self.errors.pop().ok()
    }
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream").finish_non_exhaustive()
    }
}
