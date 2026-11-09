//! The first sound: open a project, import a WAV of a tone, load it, and
//! play it twice through the default audio device.
//!
//! The tone decays, so it sounds like a pluck. The second press lands while
//! the first is still ringing, which is two voices summed by the renderer.
//! The WAV is written at 44.1 kHz, so the import resamples it on the way
//! in.
//!
//! The engine does not know there is a speaker. This file opens the
//! device, takes the engine's output and hands it over; those are the only
//! lines here that need hardware, and `device/` beside this file is the
//! only code that knows `cpal`.
//!
//! Usage: `cargo run --example first_sound -- <project-dir>`. The directory
//! is created if it does not exist. Needs an output device.

use std::error::Error;
use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use generations::Engine;

// Shared by the examples; each one uses part of it.
#[allow(dead_code)]
mod device;
use device::Device;

/// RAM for loaded audio. The engine does not know the machine; this
/// example does not either, so it guesses generously for a desktop.
const SAMPLE_RAM_BUDGET: usize = 1024 * 1024 * 1024;

/// Rate of the WAV this example writes: not the project's, on purpose.
const WAV_RATE: u32 = 44_100;

/// One second of A440, decaying from half scale to near silence, as a mono
/// float WAV at `WAV_RATE`.
fn pluck_wav(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: WAV_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for i in 0..WAV_RATE {
        let t = i as f32 / WAV_RATE as f32;
        w.write_sample(0.5 * (TAU * 440.0 * t).sin() * (-4.0 * t).exp())?;
    }
    w.finalize()?;
    Ok(path.to_path_buf())
}

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: first_sound <project-dir>")?;

    let mut engine = Engine::open(&dir, SAMPLE_RAM_BUDGET)?;
    let mut output = engine.output().ok_or("output already taken")?;
    // Declared after `engine`, so it is dropped first: the audio thread
    // stops before the engine lets go of its audio.
    let mut stream = Device::default_output()?.start(move |out| output.render(out))?;

    let scratch = tempfile::tempdir()?;
    let pluck = engine.import(&pluck_wav(&scratch.path().join("pluck.wav"))?)?;
    engine.load(pluck)?;

    println!("pluck");
    engine.play(pluck)?;
    sleep(Duration::from_millis(600));
    println!("pluck again, over the first one's tail");
    engine.play(pluck)?;
    sleep(Duration::from_millis(1200));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
