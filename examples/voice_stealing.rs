//! With both of an audio's voices sounding, a third play cuts the oldest.
//!
//! Usage: `cargo run --example voice_stealing`. Needs a duplex device.

use std::error::Error;
use std::f32::consts::TAU;
use std::thread::sleep;
use std::time::Duration;

use generations::ChannelLayout;
use generations::SAMPLE_RATE;

mod device;
use device::Device;

fn main() -> Result<(), Box<dyn Error>> {
    let device = Device::default_duplex()?;
    // One audio with two voices of its own; no inputs.
    let (mut control, mut renderer) = generations::create(device.capture_channels(), 0, 1, 2, 0, 0);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    // A four-second sweep from A220 to A440 at a fifth of full scale, so
    // voices started apart are heard at different pitches.
    let frames = 4 * SAMPLE_RATE as usize;
    let seconds = frames as f32 / SAMPLE_RATE as f32;
    let sweep = (0..frames)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let phase = 220.0 * (t + t * t / (2.0 * seconds));
            0.2 * (TAU * phase).sin()
        })
        .collect();
    let mut audio = control.audio().load(sweep, ChannelLayout::Mono)?;

    println!("two voices of one sweep, a second apart");
    let _first = audio.play();
    sleep(Duration::from_secs(1));
    let _second = audio.play();
    sleep(Duration::from_secs(1));

    println!("a third cuts the oldest");
    let _third = audio.play();
    sleep(Duration::from_millis(4200));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
