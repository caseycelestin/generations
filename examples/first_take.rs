//! The first take: open a project, record a few seconds from the default
//! input, save the take loaded, and play it back through the default
//! output.
//!
//! The engine does not know there is a microphone or a speaker. This file
//! opens both devices, hands the engine's output to one and a take to the
//! other; `device/` beside this file is the only code that knows `cpal`.
//!
//! The take is sized to the recording time, so it fills on its own at the
//! end and hands itself back while the input stream is still running; the
//! stream is then dropped and the take saved. Stopping early would be the
//! same with the stream dropped sooner.
//!
//! Usage: `cargo run --example first_take -- <project-dir> [seconds]`. The
//! directory is created if it does not exist. Needs an input and an output
//! device.

use std::error::Error;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use generations::Engine;
use generations::config::SAMPLE_RATE;

mod device;
use device::{Device, Input};

/// RAM for loaded audio. The engine does not know the machine; this
/// example does not either, so it guesses generously for a desktop.
const SAMPLE_RAM_BUDGET: usize = 1024 * 1024 * 1024;

/// How long to record when the command line does not say.
const DEFAULT_SECONDS: u64 = 3;

/// A moment past the recording time, since the device takes a moment to
/// start delivering.
const START_SLACK: Duration = Duration::from_millis(300);

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let dir = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: first_take <project-dir> [seconds]")?;
    let seconds: u64 = match args.next() {
        Some(s) => s.to_string_lossy().parse()?,
        None => DEFAULT_SECONDS,
    };
    let frames = usize::try_from(u64::from(SAMPLE_RATE) * seconds)?;

    let mut engine = Engine::open(&dir, SAMPLE_RAM_BUDGET)?;
    let mut output = engine.output().ok_or("output already taken")?;
    // Declared after `engine`, so it is dropped first: the audio thread
    // stops before the engine lets go of its audio.
    let mut playback = Device::default_output()?.start(move |out| output.render(out))?;

    let input = Input::default_input(1)?;
    let mut recorder = engine.recorder(1, frames)?;
    let mut take = recorder.start_take()?;

    println!("recording {seconds} s, mono");
    let mut capture = input.start(move |heard| take.write(heard))?;
    sleep(Duration::from_secs(seconds) + START_SLACK);
    // The take filled and handed itself back a moment ago. Dropping the
    // stream drops the finished take; nothing more comes from it.
    while let Some(e) = capture.error() {
        eprintln!("input stream error: {e}");
    }
    drop(capture);

    let id = engine.save_take(&mut recorder, true)?;
    println!("saved as sample {}, loaded; playing it back", id.0);
    engine.play(id)?;
    sleep(Duration::from_secs(seconds) + Duration::from_millis(200));

    while let Some(e) = playback.error() {
        eprintln!("output stream error: {e}");
    }
    Ok(())
}
