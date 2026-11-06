//! The first sound: open a project, import a tone, print its reverse, load
//! both, and play them through the default audio device.
//!
//! The tone decays, so the root sounds like a pluck and its reverse like a
//! swell. The second starts while the first is still ringing, which is two
//! voices summed by the renderer.
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
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use generations::config::{SAMPLE_RAM_BUDGET, SAMPLE_RATE};
use generations::{AudioBuffer, Engine, effects};

mod device;
use device::Device;

/// One second of A440, decaying from half scale to near silence.
fn pluck() -> Result<AudioBuffer, Box<dyn Error>> {
    let samples: Vec<f32> = (0..SAMPLE_RATE)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            0.5 * (TAU * 440.0 * t).sin() * (-4.0 * t).exp()
        })
        .collect();
    Ok(AudioBuffer::from_f32(1, &samples)?)
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
    let _stream = Device::default_output()?.start(move |out| output.render(out))?;

    let root = engine.import(&pluck()?)?;
    let swell = engine.print(root, effects::reverse)?;
    engine.load(root)?;
    engine.load(swell)?;

    println!("pluck");
    engine.play(root)?;
    sleep(Duration::from_millis(600));
    println!("swell, over the pluck's tail");
    engine.play(swell)?;
    sleep(Duration::from_millis(1200));
    Ok(())
}
