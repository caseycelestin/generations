//! The engine as a user sees it: load a sample from a project,
//! make an engine, keep the control, play its audio as voices, and give the
//! renderer to the audio thread to render periods. One voice plays out, one
//! is stopped, and each reports once it has ended. The main out is recorded
//! while they play. Nothing is heard: the periods go to a buffer, not a
//! device.
//!
//! A demonstration of the shape, not a program to run: it expects a
//! project at `project/` that already holds sample 0.

use std::error::Error;
use std::path::Path;
use std::thread;

use generations::engine;
use generations::{AudioMemory, ChannelLayout, Project, SampleId};

/// A generous guess for a desktop.
const AUDIO_RAM_BUDGET: usize = 1024 * 1024 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let memory = AudioMemory::with_budget(AUDIO_RAM_BUDGET);
    let mut project = Project::open(Path::new("project"))?;
    let audio = project.load(SampleId(0), &memory)?;

    // 16 voices at once, and room for 64 plays on their way.
    let (mut control, renderer) = engine::create(64, 16, 0, 0);

    let mut played = control.play(&audio)?;
    let mut stopped = control.play(&audio)?;
    stopped.stop();

    // Room for a second of the main out, paid for now.
    let take = memory.alloc(ChannelLayout::Stereo, 48_000)?;
    let mut recording = control.record(take)?;

    // A device callback would own the renderer.
    let audio_thread = thread::spawn(move || {
        let mut renderer = renderer;
        // Periods of 256 stereo frames, about a second at 48 kHz: enough
        // for short audio to play out.
        let mut period = vec![0.0; 256 * 2];
        for _ in 0..188 {
            renderer.render(&[], &mut period);
        }
    });
    audio_thread.join().expect("audio thread");

    recording.stop();

    // How it ended, with the audio holding what was recorded, here to be
    // kept; once.
    match recording.try_wait() {
        Ok((why, recorded)) => {
            println!("recording ended: {why:?}");
            project.add_sample(&recorded)?;
        }
        Err(error) => println!("no result: {error}"),
    }

    // Whether each voice has ended; taking that also frees its slot.
    for voice in [&mut played, &mut stopped] {
        match voice.try_wait() {
            Ok(()) => println!("ended"),
            Err(error) => println!("not ended: {error}"),
        }
    }

    // Sends collect on their own; call it when the app goes quiet.
    control.collect();
    Ok(())
}
