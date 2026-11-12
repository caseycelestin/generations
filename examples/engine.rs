//! The new engine as a user sees it today: make one, keep the control,
//! give the renderer to the audio thread. Nothing can be played yet.
//!
//! Usage: `cargo run --example engine`.

use std::thread;

use generations::engine;

fn main() {
    // Both sides of one engine, with room for 64 commands in flight.
    let (control, renderer) = engine::create(64);

    // The renderer goes to the audio thread; a device callback would own it.
    let audio = thread::spawn(move || {
        let _renderer = renderer;
    });

    // The control stays here. Clones are the same engine, for handing to
    // other parts of the app.
    let _ui = control.clone();

    // Frees whatever the renderer has handed back. Sends do this on their
    // own; an app calls it when it goes quiet or before a big load.
    control.collect();

    audio.join().expect("audio thread");
}
