//! The new engine as a user sees it today: load a sample from a project,
//! make an engine, keep the control, play its audio as voices, and give the
//! renderer to the audio thread to render periods. One voice plays out, one
//! is stopped, and each reports how it ended. The main out is recorded
//! while they play. Nothing is heard: the periods go to a buffer, not a
//! device.
//!
//! A demonstration of the shape, not a program to run: it expects a
//! project at `project/` that already holds sample 0.

use std::error::Error;
use std::num::NonZeroUsize;
use std::path::Path;
use std::thread;

use generations::engine::{self, RecordingState, VoiceState};
use generations::{AudioMemory, ChannelLayout, Project, SampleId};

/// RAM for audio: a generous guess for a desktop.
const AUDIO_RAM_BUDGET: usize = 1024 * 1024 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let memory = AudioMemory::with_budget(AUDIO_RAM_BUDGET);
    let mut project = Project::open(Path::new("project"))?;
    let audio = project.load(SampleId(0), &memory)?;

    // Room for 64 commands in flight and 16 voices at once.
    let commands = NonZeroUsize::new(64).expect("not zero");
    let voices = NonZeroUsize::new(16).expect("not zero");
    let (control, renderer) = engine::create(commands, voices);

    // Clones are the same engine.
    let _ui = control.clone();

    let played = control.play(&audio)?;
    let stopped = control.play(&audio)?;
    stopped.stop()?;

    // Room for a second of the main out, paid for now.
    let take = memory.alloc(ChannelLayout::Stereo, 48_000)?;
    let recording = control.record(take)?;

    // A device callback would own the renderer.
    let audio_thread = thread::spawn(move || {
        let mut renderer = renderer;
        // Periods of 256 stereo frames, about a second at 48 kHz: enough
        // for short audio to play out.
        let mut period = vec![0.0; 256 * 2];
        for _ in 0..188 {
            renderer.render(&mut period);
        }
    });
    audio_thread.join().expect("audio thread");

    recording.stop()?;

    // The audio thread reports when a recording has ended.
    match recording.state() {
        RecordingState::Recording => println!("still recording"),
        RecordingState::Ended(why) => println!("recording ended: {why:?}"),
    }

    // Once it has ended, the audio comes back, here to be kept.
    if let Ok(recorded) = recording.take() {
        project.add_sample(&recorded)?;
    }

    // The audio thread reports how a voice ended, once it has: here
    // Finished, then Stopped.
    for voice in [&played, &stopped] {
        match voice.state() {
            VoiceState::Playing => println!("still playing"),
            VoiceState::Ended(why) => println!("ended: {why:?}"),
        }
    }

    // Every voice playing this audio, without their handles.
    control.stop(&audio)?;

    // Every voice at once, without their handles.
    control.stop_all()?;

    // Sends collect on their own; call it when the app goes quiet.
    control.collect();
    Ok(())
}
