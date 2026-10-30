//! Opens the project at the given directory. There is no storage backend
//! yet, so this cannot run until one exists.
//!
//! Usage: `generations <project-dir>`.

use std::path::PathBuf;

use generations::Engine;

fn main() {
    let dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: generations <project-dir>");
    let _engine = Engine::open(&dir).expect("open project");
}
