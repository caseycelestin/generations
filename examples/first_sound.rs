//! The first sound: write a decaying tone into audio, save it to a project,
//! and play it twice through the default output, the second over the
//! first's tail so the renderer sums them.
//!
//! Usage: `cargo run --example first_sound -- <project-dir>`. The directory
//! is created if it does not exist. Needs an output device.

use std::error::Error;
use std::f32::consts::TAU;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use generations::config::SAMPLE_RATE;
use generations::{AudioMemory, ChannelLayout, Project, engine};

mod device;
use device::Device;

/// A generous guess for a desktop; the library does not know the machine.
const AUDIO_RAM_BUDGET: usize = 1024 * 1024 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: first_sound <project-dir>")?;

    let memory = AudioMemory::with_budget(AUDIO_RAM_BUDGET);
    let mut project = Project::open(&dir)?;
    // Room for 64 commands in flight and 16 voices at once.
    let (control, mut renderer) = engine::create(64, 16);
    let mut stream = Device::default_output()?.start(move |out| renderer.render(out))?;

    // One second of A440, decaying from half scale to near silence.
    let frames = SAMPLE_RATE as usize;
    let mut pluck = memory.alloc(ChannelLayout::Mono, frames)?;
    pluck
        .extend((0..frames).map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            0.5 * (TAU * 440.0 * t).sin() * (-4.0 * t).exp()
        }))
        .ok_or("the engine holds the audio")?;
    let id = project.add_sample(&pluck)?;
    println!("saved as {id:?}");

    println!("pluck");
    let first = control.play(&pluck)?;
    sleep(Duration::from_millis(600));
    println!("first: {:?}", first.state());
    println!("pluck again, over the first one's tail");
    let second = control.play(&pluck)?;
    sleep(Duration::from_millis(1200));
    // Both have played to their last frame by now.
    println!("first: {:?}, second: {:?}", first.state(), second.state());
    control.collect();

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
