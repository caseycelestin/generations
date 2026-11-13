//! The new engine as a user sees it today: load a sample from a project,
//! make an engine, keep the control, give the renderer to the audio thread
//! to render periods, and play the sample as a voice. Nothing is heard yet:
//! the renderer does not read commands.
//!
//! A demonstration of the shape, not a program to run: it expects a
//! project at `project/` that already holds sample 0.

use std::error::Error;
use std::num::NonZeroUsize;
use std::path::Path;
use std::thread;

use generations::engine::{self, VoiceState};
use generations::{Ledger, Project, SampleId};

/// RAM for loaded audio: a generous guess for a desktop.
const SAMPLE_RAM_BUDGET: usize = 1024 * 1024 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let ledger = Ledger::new(SAMPLE_RAM_BUDGET);
    let project = Project::open(Path::new("project"), ledger)?;
    let sample = project.load(SampleId(0))?;

    // Room for 64 commands in flight and 16 voices at once.
    let commands = NonZeroUsize::new(64).expect("not zero");
    let voices = NonZeroUsize::new(16).expect("not zero");
    let (control, renderer) = engine::create(commands, voices);

    // A device callback would own the renderer.
    let audio = thread::spawn(move || {
        let mut renderer = renderer;
        // One period of 256 stereo frames.
        let mut period = vec![1.0; 256 * 2];
        renderer.render(&mut period);
    });

    // Clones are the same engine.
    let _ui = control.clone();

    let voice = control.play(&sample)?;
    voice.stop()?;

    // The audio thread reports how a voice ended, once it has.
    match voice.state() {
        VoiceState::Playing => println!("still playing"),
        VoiceState::Ended(why) => println!("ended: {why:?}"),
    }

    // Sends collect on their own; call it when the app goes quiet.
    control.collect();

    audio.join().expect("audio thread");
    Ok(())
}
