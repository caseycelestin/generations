//! Plucks recorded from the main out, then played back.
//!
//! Usage: `cargo run --example record_main_out`. Needs a duplex device.

use std::error::Error;
use std::f32::consts::TAU;
use std::thread::sleep;
use std::time::Duration;

use generations::config::SAMPLE_RATE;
use generations::engine::{self, Tap, TryTakeError};
use generations::{AudioMemory, ChannelLayout};

mod device;
use device::Device;

fn main() -> Result<(), Box<dyn Error>> {
    let memory = AudioMemory::new();
    let device = Device::default_duplex()?;
    // Room for the three plucks and the take played back; no inputs.
    let (mut control, mut renderer) = engine::create(device.capture_channels(), 0, 4, 8);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    // Half a second of each note, decaying from half scale.
    let frames = SAMPLE_RATE as usize / 2;
    let mut plucks = Vec::new();
    for pitch in [440.0, 554.4, 659.3] {
        let pluck = (0..frames)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                0.5 * (TAU * pitch * t).sin() * (-6.0 * t).exp()
            })
            .collect();
        plucks.push(control.load(pluck, ChannelLayout::Mono)?);
    }

    // Two seconds of the main out; the take ends when they are used.
    let take = memory.alloc(ChannelLayout::Stereo, 2 * SAMPLE_RATE as usize)?;
    let mut recording = control.record(take, Tap::MainOut)?;
    println!("recording the plucks");
    for pluck in &mut plucks {
        pluck.play().detach();
        sleep(Duration::from_millis(400));
    }

    let take = loop {
        match recording.try_take() {
            Ok(take) => break take,
            Err(TryTakeError::Recording) => sleep(Duration::from_millis(50)),
            Err(error) => return Err(error.into()),
        }
    };
    println!("playing back {} frames", take.frames());
    let mut take = control.load(take.samples().to_vec(), take.layout())?;
    take.play().detach();
    sleep(Duration::from_millis(2200));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
