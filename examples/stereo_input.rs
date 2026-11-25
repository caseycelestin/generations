//! Two capture channels heard as one input, left and right. Use headphones,
//! or a mic near speakers feeds back.
//!
//! Usage: `cargo run --example stereo_input`. Needs a duplex device with at
//! least two capture channels.

use std::error::Error;
use std::thread::sleep;
use std::time::Duration;

use generations::engine::{self, InputChannels};

mod device;
use device::Device;

fn main() -> Result<(), Box<dyn Error>> {
    let device = Device::default_duplex()?;
    // One input; nothing loaded.
    let (mut control, mut renderer) = engine::create(device.capture_channels(), 1, 0, 8, 0, 0);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    let mut input = control.inputs().open(InputChannels { left: 0, right: 1 })?;
    input.set_monitoring(true);
    println!("monitoring channels 0 and 1, left and right");
    sleep(Duration::from_secs(3));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
