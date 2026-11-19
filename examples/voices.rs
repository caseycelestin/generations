//! Everything a voice does, heard through a device.
//!
//! Usage: `cargo run --example voices`. Needs a duplex device.

use std::error::Error;
use std::f32::consts::TAU;
use std::thread::sleep;
use std::time::Duration;

use generations::config::SAMPLE_RATE;
use generations::engine;
use generations::{Audio, AudioMemory, ChannelLayout};

mod device;
use device::Device;

/// A generous guess for a desktop; the library does not know the machine.
const AUDIO_RAM_BUDGET: usize = 1024 * 1024 * 1024;

/// Mono, from a third of full scale; `decay` is per second, zero to hold.
fn tone(
    memory: &AudioMemory,
    pitch: f32,
    seconds: f32,
    decay: f32,
) -> Result<Audio, Box<dyn Error>> {
    let frames = (seconds * SAMPLE_RATE as f32) as usize;
    let mut audio = memory.alloc(ChannelLayout::Mono, frames)?;
    audio
        .extend((0..frames).map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            0.3 * (TAU * pitch * t).sin() * (-decay * t).exp()
        }))
        .ok_or("the engine holds the audio")?;
    Ok(audio)
}

fn main() -> Result<(), Box<dyn Error>> {
    let memory = AudioMemory::with_budget(AUDIO_RAM_BUDGET);
    let device = Device::default_duplex()?;
    // Two voices at once, and room for one play on its way; no inputs.
    let (mut control, mut renderer) = engine::create(1, 2, device.capture_channels(), 0);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    let pluck = tone(&memory, 440.0, 0.5, 6.0)?;
    let higher = tone(&memory, 554.4, 0.5, 6.0)?;
    let low = tone(&memory, 220.0, 4.0, 0.0)?;
    let mid = tone(&memory, 330.0, 4.0, 0.0)?;
    let high = tone(&memory, 440.0, 4.0, 0.0)?;

    // One second of A440 on the left, silence on the right.
    let frames = SAMPLE_RATE as usize;
    let mut left = memory.alloc(ChannelLayout::Stereo, frames)?;
    left.extend((0..frames).flat_map(|i| {
        let t = i as f32 / SAMPLE_RATE as f32;
        [0.3 * (TAU * 440.0 * t).sin(), 0.0]
    }))
    .ok_or("the engine holds the audio")?;

    // A dropped handle lets its voice play out.
    println!("mono, in the middle");
    control.play(&pluck)?;
    sleep(Duration::from_millis(700));

    println!("stereo, as stored: left only");
    control.play(&left)?;
    sleep(Duration::from_millis(1200));

    println!("two plucks, summed");
    control.play(&pluck)?;
    sleep(Duration::from_millis(150));
    control.play(&higher)?;
    sleep(Duration::from_millis(800));

    println!("a held tone, stopped after a second");
    let voice = control.play(&low)?;
    sleep(Duration::from_secs(1));
    voice.stop();
    sleep(Duration::from_millis(500));

    println!("two held tones");
    control.play(&low)?;
    sleep(Duration::from_secs(1));
    control.play(&mid)?;
    sleep(Duration::from_secs(1));

    // With every voice held, a play takes the oldest's place at the
    // renderer's next period, and holds the one place on its way until then.
    println!("a third cuts the oldest");
    control.play(&high)?;
    if let Err(e) = control.play(&pluck) {
        println!("another at once: {e}");
    }
    sleep(Duration::from_millis(4200));

    // Sends collect on their own; call it when the app goes quiet.
    control.collect();

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
