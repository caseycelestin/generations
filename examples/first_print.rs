//! The first print, on disk: open a project directory, import a ramp,
//! reverse it, then reopen the directory and walk the lineage from there.
//!
//! Nothing is saved anywhere in this file. Every change is on disk by the
//! time the call that made it returns.
//!
//! Usage: `cargo run --example first_print -- <project-dir>`. The directory
//! is created if it does not exist.

use std::error::Error;
use std::path::PathBuf;

use generations::{AudioBuffer, Engine, effects};

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: first_print <project-dir>")?;

    let mut engine = Engine::open(&dir)?;
    // A short mono ramp stands in for a real recording until step 4.
    let ramp = AudioBuffer::new(1, (0..8).collect::<Vec<_>>())?;
    let root = engine.import(&ramp)?;
    let printed = engine.print(root, effects::reverse)?;
    drop(engine);

    // Walk the lineage from the print back to the root, on a fresh open.
    let engine = Engine::open(&dir)?;
    let project = engine.project();
    let mut cur = Some(printed);
    while let Some(id) = cur {
        let sample = project.sample(id)?;
        println!(
            "id {} parent {:?} samples {:?}",
            id.0,
            sample.parent.map(|p| p.0),
            project.audio(id)?.samples()
        );
        cur = sample.parent;
    }
    Ok(())
}
