//! With every voice held, a new play cuts the oldest.
//!
//! Usage: `cargo run --example voice_stealing`. Needs a duplex device.

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
    let memory = AudioMemory::new();
    let device = Device::default_duplex()?;
    // Two voices at once, and room for one play on its way; no inputs.
    let (mut control, mut renderer) = engine::create(1, 2, device.capture_channels(), 0);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    // Four seconds of each, at a third of full scale.
    let frames = 4 * SAMPLE_RATE as usize;
    let mut tones = Vec::new();
    for pitch in [220.0, 330.0, 440.0] {
        let mut tone = memory.alloc(ChannelLayout::Mono, frames)?;
        tone.extend((0..frames).map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            0.3 * (TAU * pitch * t).sin()
        }))
        .ok_or("the engine holds the audio")?;
        tones.push(tone);
    }

    println!("two held tones");
    control.play(&tones[0])?;
    sleep(Duration::from_secs(1));
    control.play(&tones[1])?;
    sleep(Duration::from_secs(1));

    println!("a third cuts the oldest");
    control.play(&tones[2])?;
    sleep(Duration::from_millis(4200));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
