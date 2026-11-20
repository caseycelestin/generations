//! A held tone, stopped after a second.
//!
//! Usage: `cargo run --example stop_voice`. Needs a duplex device.

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
    let (mut control, mut renderer) = engine::create::<8>(1, 1, device.capture_channels(), 0, 0);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    // Four seconds of A220 at a third of full scale, so the stop is heard.
    let frames = 4 * SAMPLE_RATE as usize;
    let mut tone = AudioMemory::new().alloc(ChannelLayout::Mono, frames)?;
    tone.extend((0..frames).map(|i| {
        let t = i as f32 / SAMPLE_RATE as f32;
        0.3 * (TAU * 220.0 * t).sin()
    }))
    .ok_or("the engine holds the audio")?;

    println!("a held tone, stopped after a second");
    let voice = control.play(&tone)?;
    sleep(Duration::from_secs(1));
    voice.stop();
    sleep(Duration::from_millis(500));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
