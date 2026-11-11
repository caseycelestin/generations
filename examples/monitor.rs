//! Hear the input: open the default capture device, feed it to the engine,
//! and listen through the default output for a few seconds.
//!
//! The engine does not know there is a microphone or a speaker. This file
//! opens both devices, adds an input to the engine and takes its output,
//! and hands each to its stream; `device/` beside this file is the only
//! code that knows `cpal`. The input is laid out like the device: the
//! layout says what each of the device's channels means, a stereo pair or
//! a mono source, and each source is a sound the engine can monitor on its
//! own. The frames go from the device to the engine as delivered.
//!
//! Usage: `cargo run --example monitor -- [mono|stereo ...]`. The words
//! are the layout, in device channel order; the default is one `mono`,
//! the device's first channel heard on both sides. The device must have
//! at least as many channels as the layout adds up to; any beyond are not
//! fed. Needs an output device and a capture device. Keep the speaker
//! away from the microphone.

use std::error::Error;
use std::thread::sleep;
use std::time::Duration;

use generations::{ChannelLayout, Renderer};

mod device;
use device::{Capture, Device};

/// How long to listen.
const LISTEN: Duration = Duration::from_secs(10);

fn main() -> Result<(), Box<dyn Error>> {
    let layout = std::env::args()
        .skip(1)
        .map(|word| match word.as_str() {
            "mono" => Ok(ChannelLayout::Mono),
            "stereo" => Ok(ChannelLayout::Stereo),
            other => Err(format!("{other}: expected mono or stereo")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let layout = if layout.is_empty() {
        vec![ChannelLayout::Mono]
    } else {
        layout
    };

    // No project: monitoring an input needs no samples and no disk.
    let (mut renderer, control) = Renderer::new();
    // The capture device is opened first so the engine can be told how
    // many frames it delivers at a time.
    let capture = Capture::default_input()?;
    let have = capture.channels();
    let (input, mut feed) = control.input(&layout, capture.period())?;
    // Declared after `engine`, so they are dropped first: both audio
    // threads stop before the engine goes.
    let mut playback = Device::default_output()?.start(move |out| renderer.render(out))?;
    // The first channels of the device, as many as the layout is wide.
    let take = (0..feed.channels()).collect();
    let mut capture = capture.start(take, move |frames| feed.feed(frames))?;

    println!(
        "input {layout:?} added over a {have}-channel device; silent for 2s, then heard for {LISTEN:?}"
    );
    sleep(Duration::from_secs(2));
    for source in input.sources() {
        source.monitor()?;
    }
    sleep(LISTEN);
    // Stop hearing it, then drop the input before the capture stream, and
    // its feed, go: the rings are then freed on this thread.
    for source in input.sources() {
        source.unmonitor()?;
    }
    drop(input);

    while let Some(e) = capture.error() {
        eprintln!("capture stream error: {e}");
    }
    while let Some(e) = playback.error() {
        eprintln!("playback stream error: {e}");
    }
    Ok(())
}
