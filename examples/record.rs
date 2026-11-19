//! Everything a recording does, heard through a device: the main out is
//! recorded while plucks play, then while an input is monitored, and each
//! take is played back. Use headphones, or the mic records the speakers.
//!
//! Usage: `cargo run --example record`. Needs a duplex device with a
//! capture channel.

use std::error::Error;
use std::f32::consts::TAU;
use std::thread::sleep;
use std::time::Duration;

use generations::config::SAMPLE_RATE;
use generations::engine::{self, InputChannels, TryTakeError};
use generations::{AudioMemory, ChannelLayout};

mod device;
use device::Device;

/// A generous guess for a desktop; the library does not know the machine.
const AUDIO_RAM_BUDGET: usize = 1024 * 1024 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let memory = AudioMemory::with_budget(AUDIO_RAM_BUDGET);
    let device = Device::default_duplex()?;
    // 16 voices at once, room for 64 plays on their way, and one input.
    let (mut control, mut renderer) = engine::create(64, 16, device.capture_channels(), 1);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

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

    let mono = memory.alloc(ChannelLayout::Mono, SAMPLE_RATE as usize)?;
    if let Err(e) = control.record(mono) {
        println!("mono audio: {e}");
    }

    // Two seconds of the main out; the take ends when they are used.
    let take = memory.alloc(ChannelLayout::Stereo, 2 * SAMPLE_RATE as usize)?;
    let mut recording = control.record(take)?;
    println!("recording the plucks");

    // A refused recording hands its audio back.
    let spare = memory.alloc(ChannelLayout::Stereo, 5 * SAMPLE_RATE as usize)?;
    let refused = control.record(spare).expect_err("one recording at a time");
    println!("a second recording: {refused}");
    let spare = refused.into_audio();

    for pluck in &plucks {
        control.play(pluck)?;
        sleep(Duration::from_millis(400));
    }
    let recorded = loop {
        match recording.try_take() {
            Ok(recorded) => break recorded,
            Err(TryTakeError::Recording) => sleep(Duration::from_millis(50)),
            Err(error) => return Err(error.into()),
        }
    };
    println!("the take is full");
    if let Err(e) = recording.try_take() {
        println!("taking it again: {e}");
    }

    println!("playing back {} frames", recorded.frames());
    control.play(&recorded)?;
    let refused = control.record(recorded).expect_err("a voice holds it");
    println!("recording over it: {refused}");
    let recorded = refused.into_audio();
    sleep(Duration::from_millis(2200));

    // Monitored, an input is in the main out, so it is recorded too.
    let mut input = control.open_input(InputChannels::Mono(0))?;
    input.set_monitoring(true);
    let mut recording = control.record(spare)?;
    println!("recording channel 0 for three seconds: speak");
    sleep(Duration::from_secs(3));

    // Stopped early, a take holds what was heard until then.
    recording.stop();
    input.stop();
    let spoken = loop {
        match recording.try_take() {
            Ok(spoken) => break spoken,
            Err(TryTakeError::Recording) => sleep(Duration::from_millis(50)),
            Err(error) => return Err(error.into()),
        }
    };
    println!("stopped; playing back {} frames", spoken.frames());
    control.play(&spoken)?;
    sleep(Duration::from_millis(3200));

    // Recorded over from empty, now that the voice has ended. Dropping a
    // recording stops it; its place comes back at the renderer's next period.
    let recording = control.record(recorded)?;
    drop(recording);
    sleep(Duration::from_millis(100));
    let _again = control.record(spoken)?;
    println!("a place is free again");

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
