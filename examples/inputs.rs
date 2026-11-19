//! Everything an input does, heard through a device. Use headphones, or a
//! mic near speakers feeds back.
//!
//! Usage: `cargo run --example inputs`. Needs a duplex device with at least
//! two capture channels.

use std::error::Error;
use std::thread::sleep;
use std::time::Duration;

use generations::engine::{self, InputChannels};

mod device;
use device::Device;

fn main() -> Result<(), Box<dyn Error>> {
    let device = Device::default_duplex()?;
    let channels = device.capture_channels();
    // No voices are played; two inputs open at once.
    let (mut control, mut renderer) = engine::create(1, 1, channels, 2);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    if let Err(e) = control.open_input(InputChannels::Mono(channels)) {
        println!("channel {channels}: {e}");
    }

    let mut left = control.open_input(InputChannels::Mono(0))?;
    left.set_monitoring(true);
    println!("monitoring channel 0, in the middle");
    sleep(Duration::from_secs(3));

    left.set_monitoring(false);
    println!("muted, still open");
    sleep(Duration::from_secs(2));

    // Inputs may read the same channel.
    let mut both = control.open_input(InputChannels::Stereo(0, 1))?;
    both.set_monitoring(true);
    println!("monitoring channels 0 and 1, left and right");
    sleep(Duration::from_secs(3));

    if let Err(e) = control.open_input(InputChannels::Mono(1)) {
        println!("a third input: {e}");
    }

    left.stop();
    // Dropping stops it too.
    drop(both);
    // Places come back at the renderer's next period.
    sleep(Duration::from_millis(100));
    let _again = control.open_input(InputChannels::Mono(1))?;
    println!("a place is free again");

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
