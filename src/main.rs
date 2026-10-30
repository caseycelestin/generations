//! The first print, kept: create a project in a working directory, import a
//! ramp and reverse it. Nothing is saved explicitly; every change is written
//! through. Then reopen the directory and walk the lineage from that copy.
//!
//! Usage: `generations <project-dir>`. The directory is created if needed.

use std::path::PathBuf;
use std::process::ExitCode;

use generations::{AudioBuffer, Engine, effects};

fn main() -> ExitCode {
    let Some(dir) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: generations <project-dir>");
        return ExitCode::FAILURE;
    };
    match run(&dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = Engine::create(dir)?;
    // A short mono ramp stands in for a real recording until step 4.
    let ramp = AudioBuffer::new(1, (0..8).collect::<Vec<_>>())?;
    let root = engine.import(ramp)?;
    let printed = engine.print(root, effects::reverse)?;
    drop(engine);

    // Walk the lineage from the print back to the root, on a fresh open.
    let engine = Engine::open(dir)?;
    let project = engine.project();
    let mut cur = Some(printed);
    while let Some(id) = cur {
        let s = project.get(id).expect("in lineage");
        println!(
            "id {} parent {:?} samples {:?}",
            id.raw(),
            s.parent().map(|p| p.raw()),
            s.audio().samples()
        );
        cur = s.parent();
    }
    Ok(())
}
