//! An input heard through the main out, then muted while still open. Use
//! headphones, or a mic near speakers feeds back.
//!
//! Usage: `cargo run --example monitor`. Needs a duplex device with a
//! capture channel.

use std::error::Error;
use std::thread::sleep;
use std::time::Duration;

use generations::engine::{self, InputChannels};

mod device;
use device::Device;

fn main() -> Result<(), Box<dyn Error>> {
    let device = Device::default_duplex()?;
    // One input; nothing loaded.
    let (mut control, mut renderer) = engine::create::<8>(device.capture_channels(), 1, 0);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    let mut input = control.open_input(InputChannels::Mono(0))?;
    input.set_monitoring(true);
    println!("monitoring channel 0, in the middle");
    sleep(Duration::from_secs(3));

    input.set_monitoring(false);
    println!("muted, still open");
    sleep(Duration::from_secs(2));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
