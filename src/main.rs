//! The first print: import a ramp, reverse it, and walk the lineage back.

use generations::{AudioBuffer, Engine, effects};

fn main() {
    let mut engine = Engine::new();

    // A short mono ramp stands in for a real recording until step 4.
    let ramp = AudioBuffer::new(1, (0..8).collect::<Vec<_>>()).expect("valid buffer");
    let root = engine.import(ramp);
    let printed = engine.print(root, effects::reverse).expect("root exists");

    // Walk the lineage from the print back to the root.
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
}
