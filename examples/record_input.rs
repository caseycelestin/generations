//! An input recorded without being heard, then played back.
//!
//! Usage: `cargo run --example record_input`. Needs a duplex device with a
//! capture channel.

use std::error::Error;
use std::thread::sleep;
use std::time::Duration;

use generations::config::SAMPLE_RATE;
use generations::engine::{self, InputChannels, Tap, TryTakeError};
use generations::{AudioMemory, ChannelLayout};

mod device;
use device::Device;

fn main() -> Result<(), Box<dyn Error>> {
    let device = Device::default_duplex()?;
    // One voice for the playback, one input.
    let (mut control, mut renderer) = engine::create(1, 1, device.capture_channels(), 1, 0);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    let input = control.open_input(InputChannels::Mono(0))?;
    // Room for five seconds; stopping the input ends the take sooner.
    let take = AudioMemory::new().alloc(ChannelLayout::Stereo, 5 * SAMPLE_RATE as usize)?;
    let mut recording = control.record(take, Tap::Input(&input))?;
    println!("recording channel 0 for three seconds, unheard: speak");
    sleep(Duration::from_secs(3));

    input.stop();
    let take = loop {
        match recording.try_take() {
            Ok(take) => break take,
            Err(TryTakeError::Recording) => sleep(Duration::from_millis(50)),
            Err(error) => return Err(error.into()),
        }
    };
    println!("playing back {} frames", take.frames());
    control.play(&take)?;
    sleep(Duration::from_millis(3200));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
