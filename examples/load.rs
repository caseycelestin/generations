//! A pluck loaded into the engine, then played from it.
//!
//! Usage: `cargo run --example load`. Needs a duplex device.

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
    // Room to load one audio, with eight voices of its own.
    let (mut control, mut renderer) = engine::create::<8>(1, 1, device.capture_channels(), 0, 1);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    // Half a second of A440, decaying from a third of full scale.
    let frames = SAMPLE_RATE as usize / 2;
    let pluck = (0..frames)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            0.3 * (TAU * 440.0 * t).sin() * (-6.0 * t).exp()
        })
        .collect();

    let mut audio = control.load(pluck, ChannelLayout::Mono)?;
    println!("a pluck, in the middle");
    audio.play();
    sleep(Duration::from_millis(700));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
