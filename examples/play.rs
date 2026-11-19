//! A pluck played to its end.
//!
//! Usage: `cargo run --example play`. Needs a duplex device.

use std::error::Error;
use std::f32::consts::TAU;
use std::thread::sleep;
use std::time::Duration;

use generations::config::SAMPLE_RATE;
use generations::engine;
use generations::{AudioMemory, ChannelLayout};

mod device;
use device::Device;

fn main() -> Result<(), Box<dyn Error>> {
    let device = Device::default_duplex()?;
    // One voice, no inputs.
    let (mut control, mut renderer) = engine::create(1, 1, device.capture_channels(), 0);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    // Half a second of A440, decaying from a third of full scale.
    let frames = SAMPLE_RATE as usize / 2;
    let mut pluck = AudioMemory::new().alloc(ChannelLayout::Mono, frames)?;
    pluck
        .extend((0..frames).map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            0.3 * (TAU * 440.0 * t).sin() * (-6.0 * t).exp()
        }))
        .ok_or("the engine holds the audio")?;

    // A dropped handle lets its voice play out.
    println!("a pluck, in the middle");
    control.play(&pluck)?;
    sleep(Duration::from_millis(700));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
