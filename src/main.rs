//! The first print, saved: import a ramp, reverse it, save the project to a
//! directory, open it again and walk the lineage from the loaded copy.
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

    let mut engine = Engine::new();
    // A short mono ramp stands in for a real recording until step 4.
    let ramp = AudioBuffer::new(1, (0..8).collect::<Vec<_>>()).expect("valid buffer");
    let root = engine.import(ramp);
    let printed = engine.print(root, effects::reverse).expect("root exists");

    if let Err(e) = engine.save(&dir) {
        eprintln!("save failed: {e}");
        return ExitCode::FAILURE;
    }
    let engine = match Engine::open(&dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("open failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Walk the lineage from the print back to the root, on the loaded copy.
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
    ExitCode::SUCCESS
}
