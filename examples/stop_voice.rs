//! A held tone, stopped after a second by dropping its voice.
//!
//! Usage: `cargo run --example stop_voice`. Needs a duplex device.

use std::error::Error;
use std::f32::consts::TAU;
use std::thread::sleep;
use std::time::Duration;

use generations::ChannelLayout;
use generations::config::SAMPLE_RATE;
use generations::engine;

mod device;
use device::Device;

fn main() -> Result<(), Box<dyn Error>> {
    let device = Device::default_duplex()?;
    // Room to load one audio; no inputs.
    let (mut control, mut renderer) = engine::create(device.capture_channels(), 0, 1, 8);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    // Four seconds of A220 at a third of full scale, so the stop is heard.
    let frames = 4 * SAMPLE_RATE as usize;
    let tone = (0..frames)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            0.3 * (TAU * 220.0 * t).sin()
        })
        .collect();
    let mut audio = control.load(tone, ChannelLayout::Mono)?;

    println!("a held tone, stopped after a second");
    let voice = audio.play();
    sleep(Duration::from_secs(1));
    drop(voice);
    sleep(Duration::from_millis(500));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
