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
//! An [`Input`] is the same for the default input: [`Input::start`] calls
//! its callback with each period of captured frames, and the callback that
//! records moves a `Take` in and writes to it.
//!
//! The callbacks always see `f32`. A device that wants or gives 16- or
//! 32-bit samples gets them converted in the callback, because `cpal` does
//! no conversion of its own and a raw ALSA `hw:` device (an I2S codec on
//! the target) often offers nothing else. The rate is never converted: a
//! device that cannot run at the engine's rate is refused, as the roadmap
//! says.
//!
//! There is one implementation. `cpal` is itself the abstraction over ALSA,
//! PipeWire, JACK and the rest; a trait of our own waits for a second
//! implementation.

// Shared by the examples, and each one uses part of it: first_sound has no
// input, first_take uses everything. The allow is module-wide, which also
// hides anything here that stops being used by every example; the price of
// keeping one copy. It goes when the device code moves to the binary crate.
#![allow(dead_code)]

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig, SupportedStreamConfig};
use rtrb::Producer;
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
    /// The system has no default input device.
    #[error("no input device")]
    NoInputDevice,
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

/// A device opened for one direction: picked once for the engine's rate
/// and a channel count, not yet running. What [`Device`] and [`Input`]
/// share; the direction is in which `cpal` calls they make.
struct Opened {
    device: cpal::Device,
    format: SampleFormat,
    config: StreamConfig,
}

impl Opened {
    /// Opens `device`, or fails with `missing` if there is none, at the
    /// engine's rate and `channels` channels, choosing from the
    /// configurations `supported` lists for it.
    fn open<I>(
        device: Option<cpal::Device>,
        missing: DeviceError,
        channels: u16,
        supported: impl FnOnce(&cpal::Device) -> Result<I, cpal::Error>,
    ) -> Result<Self, DeviceError>
    where
        I: Iterator<Item = cpal::SupportedStreamConfigRange>,
    {
        let device = device.ok_or(missing)?;
        let picked = pick(supported(&device)?, channels)?;
        Ok(Self {
            device,
            format: picked.sample_format(),
            config: picked.config(),
        })
    }

    /// Builds the stream with `build`, which gets the device, the config,
    /// the format to build for and the sending end of the error queue,
    /// then starts it. The queue and the [`Stream`] around it are the same
    /// in both directions.
    fn start(
        self,
        build: impl FnOnce(
            &cpal::Device,
            &StreamConfig,
            SampleFormat,
            Producer<cpal::Error>,
        ) -> Result<cpal::Stream, DeviceError>,
    ) -> Result<Stream, DeviceError> {
        let (errors_in, errors) = RingBuffer::new(ERROR_CAPACITY);
        let inner = build(&self.device, &self.config, self.format, errors_in)?;
        inner.play()?;
        Ok(Stream { inner, errors })
    }
}

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opened")
            .field("format", &self.format)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// An output device that can run at the engine's format, not yet running.
#[derive(Debug)]
pub struct Device(Opened);

impl Device {
    /// The system's default output device, at [`SAMPLE_RATE`], stereo.
    ///
    /// `f32` is preferred, since the renderer produces it; `i16` and `i32`
    /// are accepted and converted to. There is no resampling. The period
    /// size is the backend's default for now; a fixed size for the Pi
    /// would be set on the config here.
    pub fn default_output() -> Result<Self, DeviceError> {
        Ok(Self(Opened::open(
            cpal::default_host().default_output_device(),
            DeviceError::NoOutputDevice,
            OUTPUT_CHANNELS as u16,
            |d| d.supported_output_configs(),
        )?))
    }

    /// Starts the device. From now until the returned [`Stream`] is
    /// dropped, the audio thread calls `render` once per period with an
    /// interleaved stereo `f32` slice to fill.
    pub fn start(
        self,
        render: impl FnMut(&mut [f32]) + Send + 'static,
    ) -> Result<Stream, DeviceError> {
        self.0.start(|device, config, format, errors| match format {
            SampleFormat::F32 => build::<f32>(device, config, render, errors),
            SampleFormat::I16 => build::<i16>(device, config, render, errors),
            SampleFormat::I32 => build::<i32>(device, config, render, errors),
            // `pick` chose from these three.
            _ => unreachable!("device format was chosen from the convertible set"),
        })
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
    mut errors: Producer<cpal::Error>,
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

/// An input device that can run at the engine's rate with the channel
/// count asked for, not yet running.
#[derive(Debug)]
pub struct Input(Opened);

impl Input {
    /// The system's default input device, at [`SAMPLE_RATE`], with
    /// `channels` channels. Microphones are often mono and interfaces
    /// often stereo, so the caller says which it wants and is refused if
    /// the device cannot give it; see [`Input::default_input_any`] to take
    /// whichever it has.
    pub fn default_input(channels: u16) -> Result<Self, DeviceError> {
        Ok(Self(Opened::open(
            cpal::default_host().default_input_device(),
            DeviceError::NoInputDevice,
            channels,
            |d| d.supported_input_configs(),
        )?))
    }

    /// The default input with whichever of mono or stereo it offers, mono
    /// first. For a caller that will record what it gets.
    pub fn default_input_any() -> Result<Self, DeviceError> {
        Self::default_input(1).or_else(|_| Self::default_input(2))
    }

    /// Channels per frame the callback will see.
    pub fn channels(&self) -> u16 {
        self.0.config.channels
    }

    /// Starts the device. From now until the returned [`Stream`] is
    /// dropped, the audio thread calls `capture` once per period with the
    /// interleaved `f32` frames it heard.
    pub fn start(
        self,
        capture: impl FnMut(&[f32]) + Send + 'static,
    ) -> Result<Stream, DeviceError> {
        self.0.start(|device, config, format, errors| match format {
            SampleFormat::F32 => build_input::<f32>(device, config, capture, errors),
            SampleFormat::I16 => build_input::<i16>(device, config, capture, errors),
            SampleFormat::I32 => build_input::<i32>(device, config, capture, errors),
            _ => unreachable!("device format was chosen from the convertible set"),
        })
    }
}

/// Builds the input stream for one device sample type. The device's frames
/// are converted into a scratch buffer allocated here, on the control
/// thread, and the callback is given that; for `f32` the conversion is a
/// copy. A period larger than the scratch is delivered in pieces.
fn build_input<T: SizedSample>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut capture: impl FnMut(&[f32]) + Send + 'static,
    mut errors: Producer<cpal::Error>,
) -> Result<cpal::Stream, DeviceError>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let mut scratch = vec![0.0f32; SCRATCH_FRAMES * channels];
    Ok(device.build_input_stream(
        *config,
        move |input: &[T], _| {
            for chunk in input.chunks(scratch.len()) {
                let heard = &mut scratch[..chunk.len()];
                for (h, &sample) in heard.iter_mut().zip(chunk.iter()) {
                    *h = sample.to_sample::<f32>();
                }
                capture(heard);
            }
        },
        move |e| {
            let _ = errors.push(e);
        },
        None,
    )?)
}

/// A running stream, output or input. Running while held; dropping it
/// stops it, and drops whatever the callback owned.
pub struct Stream {
    /// Kept so it keeps running. `cpal` stops it on drop; nothing reads it.
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
