//! Resample: play a pluck, record the main out while it plays, save the
//! take as a new sample, and play that back.
//!
//! The recording taps the main out after the clip, so the take is exactly
//! what came out of the speaker: two plucks, the second over the first
//! one's tail. Saved, it is a sample like any import, with an id, and
//! loading it plays it.
//!
//! The engine does not know there is a speaker. This file opens the
//! device, takes the engine's output and hands it over; `device/` beside
//! this file is the only code that knows `cpal`.
//!
//! Usage: `cargo run --example resample -- <project-dir>`. The directory
//! is created if it does not exist. Needs an output device.

use std::error::Error;
use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use generations::config::{MAX_PERIOD_FRAMES, SAMPLE_RATE};
use generations::{Ledger, Project, Renderer, RendererSettings, Tap};

mod device;
use device::Device;

/// RAM for loaded audio and the recorder. The engine does not know the
/// machine; this example does not either, so it guesses generously for a
/// desktop.
const SAMPLE_RAM_BUDGET: usize = 1024 * 1024 * 1024;

/// The longest take: three seconds. The panel would convert from the
/// seconds or bars a user picks; the engine counts frames.
const MAX_TAKE_FRAMES: usize = 3 * SAMPLE_RATE as usize;

/// One second of A440, decaying from half scale to near silence, as a mono
/// float WAV at the project's rate.
fn pluck_wav(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for i in 0..SAMPLE_RATE {
        let t = i as f32 / SAMPLE_RATE as f32;
        w.write_sample(0.5 * (TAU * 440.0 * t).sin() * (-4.0 * t).exp())?;
    }
    w.finalize()?;
    Ok(path.to_path_buf())
}

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: resample <project-dir>")?;

    let ledger = Ledger::new(SAMPLE_RAM_BUDGET);
    let mut project = Project::open(&dir, ledger.clone())?;
    // The output device renders in pieces of at most this many frames.
    let (mut renderer, control) = Renderer::new(RendererSettings::new(MAX_PERIOD_FRAMES), ledger);
    let mut stream = Device::default_output()?.start(move |out| renderer.render(out))?;

    let scratch = tempfile::tempdir()?;
    let pluck = project.import(&pluck_wav(&scratch.path().join("pluck.wav"))?)?;
    let pluck = project.load(pluck)?;

    // Paid for here, once; every take records into the same buffer.
    let mut recorder = control.recorder(MAX_TAKE_FRAMES)?;

    println!("recording the main out");
    recorder.record(Tap::MainOut)?;
    println!("pluck");
    control.play(&pluck)?;
    sleep(Duration::from_millis(600));
    println!("pluck again, over the first one's tail");
    control.play(&pluck)?;
    sleep(Duration::from_millis(1200));
    println!("stop");
    recorder.stop()?;

    // The audio thread cannot say the take is back; ask until it is. A
    // panel would ask once per screen refresh.
    let take = loop {
        if let Some(take) = recorder.last_take() {
            break take;
        }
        sleep(Duration::from_millis(10));
    };
    let resample = project.save_take(take)?;
    let resample = project.load(resample)?;

    println!("the resample: what was heard, played back");
    control.play(&resample)?;
    sleep(Duration::from_millis(2000));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
