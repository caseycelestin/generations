//! Hear the input: open the default capture device, feed it to the engine,
//! and listen through the default output for a few seconds.
//!
//! The engine does not know there is a microphone or a speaker. This file
//! opens both devices, adds an input to the engine and takes its output,
//! and hands each to its stream; `device/` beside this file is the only
//! code that knows `cpal`. Which capture channels the input is, and
//! whether it is heard, is the engine's business: named here once, on
//! `Engine::input`, and switched with `monitor`, and both arrive on the
//! render side as commands.
//!
//! Usage: `cargo run --example monitor -- [channel ...]`. One channel
//! index is heard on both sides, two are heard left and right; the
//! default is channel 0. Needs an input and an output device. Keep the
//! speaker away from the microphone.

use std::error::Error;
use std::thread::sleep;
use std::time::Duration;

use generations::Engine;

mod device;
use device::{Capture, Device};

/// RAM for loaded audio; nothing is loaded here, so any figure does.
const SAMPLE_RAM_BUDGET: usize = 64 * 1024 * 1024;

/// How long to listen.
const LISTEN: Duration = Duration::from_secs(10);

fn main() -> Result<(), Box<dyn Error>> {
    let channels = std::env::args()
        .skip(1)
        .map(|a| a.parse::<usize>())
        .collect::<Result<Vec<_>, _>>()?;
    let channels = if channels.is_empty() {
        vec![0]
    } else {
        channels
    };

    let project = tempfile::tempdir()?;
    let mut engine = Engine::open(project.path(), SAMPLE_RAM_BUDGET)?;
    let mut output = engine.output().ok_or("output already taken")?;
    let mut input = engine.input(&channels)?;
    let sound = input.sound();
    // Declared after `engine`, so they are dropped first: both audio
    // threads stop before the engine goes.
    let mut playback = Device::default_output()?.start(move |out| output.render(out))?;
    let mut capture = Capture::default_input()?.start(move |frames| input.feed(frames))?;

    println!("input channel(s) {channels:?} added; silent for 2s, then heard for {LISTEN:?}");
    sleep(Duration::from_secs(2));
    engine.monitor(sound)?;
    sleep(LISTEN);
    // Stop hearing it, then remove it before the capture stream, and its
    // `Input`, go.
    engine.unmonitor(sound)?;
    engine.remove_input(sound)?;

    while let Some(e) = capture.error() {
        eprintln!("capture stream error: {e}");
    }
    while let Some(e) = playback.error() {
        eprintln!("playback stream error: {e}");
    }
    Ok(())
}
