//! The audio device, over `cpal`. It lives beside the examples because
//! which machine the sampler runs on is the caller's knowledge; the library
//! builds with no audio dependency.
//!
//! [`Device`] is the default output and [`Capture`] the default input, each
//! checked once for the engine's rate. `start` takes a callback the audio
//! thread calls once per period for as long as the returned [`Stream`] is
//! held. Its scratch is allocated when the stream is built, never on the
//! audio thread.
//!
//! Callbacks always see `f32`. A device that wants 16- or 32-bit samples
//! gets them converted in the callback, because `cpal` converts nothing and
//! a raw ALSA `hw:` device (an I2S codec on the target) often offers
//! nothing else. The rate is never converted: a device that cannot run at
//! it is refused.
//!
//! One implementation, no trait: `cpal` is already the abstraction over
//! ALSA, PipeWire and JACK.
//!
//! Each example uses the half it needs, so the other half is dead code
//! there.
#![allow(dead_code)]

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, FromSample, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize,
    SupportedStreamConfig,
};
use rtrb::{Consumer, RingBuffer};
use thiserror::Error;

use generations::config::SAMPLE_RATE;
use generations::engine::Renderer;

/// Asked for so the period is known before the stream runs. Clamped to
/// what the device supports.
const CAPTURE_PERIOD_FRAMES: u32 = 512;

/// Frames of `f32` scratch the callback renders into before converting
/// to the device's format. A period larger than this is rendered in
/// pieces; nothing allocates on the audio thread either way.
const SCRATCH_FRAMES: usize = 4096;

/// Stream errors kept for the control side before the oldest is dropped.
const ERROR_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DeviceError {
    #[error("no output device")]
    NoOutputDevice,
    #[error("no input device")]
    NoInputDevice,
    #[error("device does not support {rate} Hz, {channels} channels, in f32, i16 or i32")]
    UnsupportedFormat { rate: u32, channels: u16 },
    #[error("the capture device has {have} channels; there is no channel {channel}")]
    NoSuchChannel { channel: usize, have: usize },
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
    /// Stereo. The period is the backend's default; a fixed size for the
    /// Pi would be set on the config here.
    pub fn default_output() -> Result<Self, DeviceError> {
        let channels = Renderer::CHANNELS as u16;
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

    /// `render` fills an interleaved stereo period.
    pub fn start(
        self,
        render: impl FnMut(&mut [f32]) + Send + 'static,
    ) -> Result<Stream, DeviceError> {
        let (errors_in, errors) = RingBuffer::new(ERROR_CAPACITY);
        let inner = match self.format {
            SampleFormat::F32 => build::<f32>(&self.device, &self.config, render, errors_in),
            SampleFormat::I16 => build::<i16>(&self.device, &self.config, render, errors_in),
            SampleFormat::I32 => build::<i32>(&self.device, &self.config, render, errors_in),
            _ => unreachable!("device format was chosen from the convertible set"),
        }?;
        inner.play()?;
        Ok(Stream { inner, errors })
    }
}

/// The engine's rate, in the most convenient format this code converts.
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

/// An input device that can run at the engine's input format, not yet
/// running.
pub struct Capture {
    device: cpal::Device,
    format: SampleFormat,
    config: StreamConfig,
}

impl Capture {
    /// At the device's own default channel count and a fixed period near
    /// [`CAPTURE_PERIOD_FRAMES`].
    pub fn default_input() -> Result<Self, DeviceError> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or(DeviceError::NoInputDevice)?;
        let channels = device.default_input_config()?.channels();
        let supported = pick(device.supported_input_configs()?, channels)?;
        let period = match *supported.buffer_size() {
            SupportedBufferSize::Range { min, max } => CAPTURE_PERIOD_FRAMES.clamp(min, max),
            SupportedBufferSize::Unknown => CAPTURE_PERIOD_FRAMES,
        };
        let mut config = supported.config();
        config.buffer_size = BufferSize::Fixed(period);
        Ok(Self {
            device,
            format: supported.sample_format(),
            config,
        })
    }

    /// The width of a device frame, which `start`'s `take` indexes into.
    pub fn channels(&self) -> usize {
        usize::from(self.config.channels)
    }

    /// In frames, per callback.
    pub fn period(&self) -> usize {
        match self.config.buffer_size {
            BufferSize::Fixed(frames) => frames as usize,
            BufferSize::Default => CAPTURE_PERIOD_FRAMES as usize,
        }
    }

    /// `feed` gets interleaved frames of just the device channels in
    /// `take`, in that order.
    pub fn start(
        self,
        take: Vec<usize>,
        feed: impl FnMut(&[f32]) + Send + 'static,
    ) -> Result<Stream, DeviceError> {
        let have = self.channels();
        if let Some(&channel) = take.iter().find(|&&c| c >= have) {
            return Err(DeviceError::NoSuchChannel { channel, have });
        }
        let (errors_in, errors) = RingBuffer::new(ERROR_CAPACITY);
        let inner = match self.format {
            SampleFormat::F32 => {
                build_input::<f32>(&self.device, &self.config, take, feed, errors_in)
            }
            SampleFormat::I16 => {
                build_input::<i16>(&self.device, &self.config, take, feed, errors_in)
            }
            SampleFormat::I32 => {
                build_input::<i32>(&self.device, &self.config, take, feed, errors_in)
            }
            _ => unreachable!("device format was chosen from the convertible set"),
        }?;
        inner.play()?;
        Ok(Stream { inner, errors })
    }
}

fn build_input<T: SizedSample>(
    device: &cpal::Device,
    config: &StreamConfig,
    take: Vec<usize>,
    mut feed: impl FnMut(&[f32]) + Send + 'static,
    mut errors: rtrb::Producer<cpal::Error>,
) -> Result<cpal::Stream, DeviceError>
where
    f32: FromSample<T>,
{
    let width = usize::from(config.channels);
    let mut scratch = vec![0.0f32; SCRATCH_FRAMES * take.len()];
    Ok(device.build_input_stream(
        *config,
        move |input: &[T], _| {
            for frames in input.chunks(SCRATCH_FRAMES * width) {
                let frames = frames.chunks_exact(width);
                let fed = frames.len() * take.len();
                let out = scratch[..fed].chunks_exact_mut(take.len());
                for (out, frame) in out.zip(frames) {
                    for (o, &channel) in out.iter_mut().zip(&take) {
                        *o = frame[channel].to_sample::<f32>();
                    }
                }
                feed(&scratch[..fed]);
            }
        },
        move |e| {
            let _ = errors.push(e);
        },
        None,
    )?)
}

impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("format", &self.format)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("format", &self.format)
            .field("config", &self.config)
            .finish_non_exhaustive()
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

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream").finish_non_exhaustive()
    }
}
