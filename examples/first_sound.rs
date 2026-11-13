//! The first sound: open a project, alloc a sample, write a tone into it,
//! save it, and play it twice through the default audio device.
//!
//! The tone decays, so it sounds like a pluck. The second press lands while
//! the first is still ringing, which is two sounds summed by the renderer.
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
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use generations::config::SAMPLE_RATE;
use generations::{ChannelLayout, Project, engine};

mod device;
use device::Device;

/// RAM for loaded audio. The engine does not know the machine; this
/// example does not either, so it guesses generously for a desktop.
const SAMPLE_RAM_BUDGET: usize = 1024 * 1024 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: first_sound <project-dir>")?;

    let mut project = Project::open(&dir, SAMPLE_RAM_BUDGET)?;
    // Room for 64 commands in flight and 16 voices at once.
    let commands = NonZeroUsize::new(64).expect("not zero");
    let voices = NonZeroUsize::new(16).expect("not zero");
    let (control, mut renderer) = engine::create(commands, voices);
    let mut stream = Device::default_output()?.start(move |out| renderer.render(out))?;

    // One second of A440, decaying from half scale to near silence.
    let frames = SAMPLE_RATE as usize;
    let mut pluck = project.alloc(ChannelLayout::Mono, frames)?;
    let audio = pluck.write().ok_or("the engine holds the sample")?;
    audio.extend((0..frames).map(|i| {
        let t = i as f32 / SAMPLE_RATE as f32;
        0.5 * (TAU * 440.0 * t).sin() * (-4.0 * t).exp()
    }));
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
