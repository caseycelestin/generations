use generations::{AudioBuffer, Project};

fn main() {
    let mut project = Project::new();
    let silence = AudioBuffer::new(2, vec![0; 96_000]).expect("valid buffer");
    let root = project.add(None, silence.clone()).expect("no parent to check");
    let gen1 = project.add(Some(root), silence).expect("parent exists");

    // Walk the lineage back to the root.
    let mut cur = Some(gen1);
    while let Some(id) = cur {
        let s = project.get(id).expect("in lineage");
        println!("id {} parent {:?} frames {}", id.raw(), s.parent().map(|p| p.raw()), s.audio().frames());
        cur = s.parent();
    }
}
