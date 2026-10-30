//! The first print, kept: open a project directory, import a ramp and
//! reverse it. Nothing is saved; every change is in the store before the
//! call returns. Then reopen the directory and walk the lineage from there.
//!
//! Usage: `generations <project-dir>`. The directory is created if needed.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use generations::{AudioBuffer, Engine, FileStore, effects};

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

fn open(dir: &Path) -> Result<Engine, Box<dyn std::error::Error>> {
    Ok(Engine::open(Box::new(FileStore::open(dir)?))?)
}

fn run(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = open(dir)?;
    // A short mono ramp stands in for a real recording until step 4.
    let ramp = AudioBuffer::new(1, (0..8).collect::<Vec<_>>())?;
    let root = engine.import(ramp)?;
    let printed = engine.print(root, effects::reverse)?;
    drop(engine);

    // Walk the lineage from the print back to the root, on a fresh open.
    let engine = open(dir)?;
    let mut cur = Some(printed);
    while let Some(id) = cur {
        let s = engine.project().get(id).expect("in lineage");
        println!(
            "id {} parent {:?} samples {:?}",
            id.raw(),
            s.parent().map(|p| p.raw()),
            engine.audio(id)?.samples()
        );
        cur = s.parent();
    }
    Ok(())
}
