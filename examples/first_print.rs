//! The first print, on disk: open a project directory, import a WAV of a
//! ramp, reverse it, then reopen the directory and walk the lineage from
//! there.
//!
//! Nothing is saved anywhere in this file. Every change is on disk by the
//! time the call that made it returns.
//!
//! Usage: `cargo run --example first_print -- <project-dir>`. The directory
//! is created if it does not exist.

use std::error::Error;
use std::path::{Path, PathBuf};

use generations::{Engine, effects};

/// RAM for loaded audio. The engine does not know the machine; this
/// example does not either, so it guesses generously for a desktop.
const SAMPLE_RAM_BUDGET: usize = 1024 * 1024 * 1024;

/// Eight frames stepping up by a thousand, as a mono 16-bit WAV at 48 kHz.
fn ramp_wav(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for i in 0..8i16 {
        w.write_sample(i * 1000)?;
    }
    w.finalize()?;
    Ok(path.to_path_buf())
}

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: first_print <project-dir>")?;

    let mut engine = Engine::open(&dir, SAMPLE_RAM_BUDGET)?;
    // A short mono ramp, written as a 16-bit WAV at the project rate so
    // the import has a real file to read.
    let scratch = tempfile::tempdir()?;
    let ramp = ramp_wav(&scratch.path().join("ramp.wav"))?;
    let root = engine.import(&ramp)?;
    let printed = engine.print(root, effects::reverse)?;
    drop(engine);

    // Walk the lineage from the print back to the root, on a fresh open.
    let engine = Engine::open(&dir, SAMPLE_RAM_BUDGET)?;
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
