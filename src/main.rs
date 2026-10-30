use generations::{AudioBuffer, Project};

fn main() {
    let mut project = Project::new();
    let silence = AudioBuffer::new(2, vec![0; 96_000]).expect("valid buffer");
    let root = project.add_root(silence.clone());
    let gen1 = project.print(root, silence).expect("parent exists");

    for id in project.lineage(gen1).expect("sample exists") {
        let s = project.get(id).expect("in lineage");
        println!(
            "gen {} id {} {:.2}s",
            project.generation(id).unwrap(),
            id.raw(),
            s.audio().duration_secs()
        );
    }
}
