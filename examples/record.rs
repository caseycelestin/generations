//! Record the main out, then play the recording back: three plucks are
//! played while a take records two seconds of what is heard, and once the
//! take is full its audio is played through the engine like any other.
//!
//! Usage: `cargo run --example record`. Needs an output device.

use std::error::Error;
use std::f32::consts::TAU;
use std::thread::sleep;
use std::time::Duration;

use generations::config::SAMPLE_RATE;
use generations::engine::{self, RecordingState};
use generations::{AudioMemory, ChannelLayout};

mod device;
use device::Device;

/// A generous guess for a desktop; the library does not know the machine.
const AUDIO_RAM_BUDGET: usize = 1024 * 1024 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let memory = AudioMemory::with_budget(AUDIO_RAM_BUDGET);
    // Room for 64 commands in flight and 16 voices at once.
    let (control, mut renderer) = engine::create(64, 16);
    let mut stream = Device::default_output()?.start(move |out| renderer.render(out))?;

    // Half a second of each note, decaying from half scale.
    let frames = SAMPLE_RATE as usize / 2;
    let mut plucks = Vec::new();
    for pitch in [440.0, 554.4, 659.3] {
        let mut pluck = memory.alloc(ChannelLayout::Mono, frames)?;
        pluck
            .extend((0..frames).map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                0.5 * (TAU * pitch * t).sin() * (-6.0 * t).exp()
            }))
            .ok_or("the engine holds the audio")?;
        plucks.push(pluck);
    }

    // Two seconds of the main out; the take ends Full when they are used.
    let take = memory.alloc(ChannelLayout::Stereo, 2 * SAMPLE_RATE as usize)?;
    let mut recording = control.record(take)?;

    println!("recording");
    for pluck in &plucks {
        control.play(pluck)?;
        sleep(Duration::from_millis(400));
    }
    while !matches!(recording.state(), RecordingState::Ended(_)) {
        sleep(Duration::from_millis(50));
    }
    println!("recording: {:?}", recording.state());

    let recorded = recording
        .take()
        .ok_or("an ended take hands its audio back")?;
    println!("playing back {} frames", recorded.frames());
    let mut playback = control.play(&recorded)?;
    sleep(Duration::from_millis(2200));
    println!("playback: {:?}", playback.state());

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
