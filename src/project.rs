//! The pool of samples that makes up a project, and the lineage over it.

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;

use crate::audio::AudioBuffer;
use crate::sample::{Sample, SampleId};

/// Reasons a project operation can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ProjectError {
    /// No sample with this id exists in the project.
    #[error("no sample with id {}", .0.raw())]
    UnknownSample(SampleId),
    /// The sample has children and cannot be removed on its own.
    #[error("sample {} has children and cannot be removed", .0.raw())]
    HasChildren(SampleId),
}

/// A self-contained collection of samples.
///
/// Samples live in one flat pool keyed by id. Lineage is a forest over the
/// pool via each sample's parent pointer, so a chop of one sample into many
/// is just many samples sharing a parent.
#[derive(Debug, Default)]
pub struct Project {
    samples: HashMap<SampleId, Sample>,
    next_id: u32,
}

impl Project {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a root: audio that starts a new lineage, whether it came from a
    /// file or an input. The project does not record which.
    pub fn add_root(&mut self, audio: AudioBuffer) -> SampleId {
        self.add(None, audio)
    }

    /// Adds a new generation printed from `parent`.
    ///
    /// The caller has already produced `audio`; the project only records
    /// that it descends from `parent`.
    pub fn print(&mut self, parent: SampleId, audio: AudioBuffer) -> Result<SampleId, ProjectError> {
        self.require(parent)?;
        Ok(self.add(Some(parent), audio))
    }

    /// Removes a sample that has no children.
    ///
    /// A sample with children is refused: later generations depend on it
    /// and the lineage must stay readable. Removing a whole lineage is a
    /// separate, explicit operation that does not exist yet.
    pub fn remove(&mut self, id: SampleId) -> Result<Sample, ProjectError> {
        self.require(id)?;
        if self.children(id).next().is_some() {
            return Err(ProjectError::HasChildren(id));
        }
        Ok(self.samples.remove(&id).expect("checked above"))
    }

    pub fn get(&self, id: SampleId) -> Option<&Sample> {
        self.samples.get(&id)
    }

    pub fn contains(&self, id: SampleId) -> bool {
        self.samples.contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Every sample, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = &Sample> {
        self.samples.values()
    }

    /// The sample one generation behind `id`.
    pub fn parent(&self, id: SampleId) -> Result<Option<SampleId>, ProjectError> {
        Ok(self.require(id)?.parent())
    }

    /// Samples printed directly from `id`, in id order.
    pub fn children(&self, id: SampleId) -> impl Iterator<Item = SampleId> + '_ {
        let mut ids: Vec<SampleId> = self
            .samples
            .values()
            .filter(move |s| s.parent() == Some(id))
            .map(Sample::id)
            .collect();
        ids.sort();
        ids.into_iter()
    }

    /// The chain from `id` back to its root, starting with `id` itself.
    pub fn lineage(&self, id: SampleId) -> Result<Vec<SampleId>, ProjectError> {
        let mut chain = vec![id];
        let mut cur = self.require(id)?;
        while let Some(p) = cur.parent() {
            chain.push(p);
            cur = self.require(p)?;
        }
        Ok(chain)
    }

    /// How many generations `id` is from its root. A root is generation 0.
    pub fn generation(&self, id: SampleId) -> Result<usize, ProjectError> {
        Ok(self.lineage(id)?.len() - 1)
    }

    /// Samples with no parent.
    pub fn roots(&self) -> impl Iterator<Item = SampleId> + '_ {
        let mut ids: Vec<SampleId> =
            self.samples.values().filter(|s| s.is_root()).map(Sample::id).collect();
        ids.sort();
        ids.into_iter()
    }

    fn add(&mut self, parent: Option<SampleId>, audio: AudioBuffer) -> SampleId {
        let id = SampleId(self.next_id);
        self.next_id += 1;
        self.samples.insert(id, Sample::new(id, parent, Arc::new(audio)));
        id
    }

    fn require(&self, id: SampleId) -> Result<&Sample, ProjectError> {
        self.samples.get(&id).ok_or(ProjectError::UnknownSample(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize) -> AudioBuffer {
        AudioBuffer::new(1, (0..n).map(|i| i as i16).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn add_root_creates_a_root() {
        let mut p = Project::new();
        let id = p.add_root(tone(4));
        let s = p.get(id).unwrap();
        assert!(s.is_root());
        assert_eq!(p.generation(id).unwrap(), 0);
        assert_eq!(s.audio().frames(), 4);
    }

    #[test]
    fn printing_builds_a_tree_and_going_back_branches() {
        let mut p = Project::new();
        let root = p.add_root(tone(4));
        let g1 = p.print(root, tone(4)).unwrap();
        let g2 = p.print(g1, tone(4)).unwrap();
        // Go back to g1 and print something else.
        let g2b = p.print(g1, tone(4)).unwrap();

        assert_eq!(p.len(), 4);
        assert_eq!(p.lineage(g2).unwrap(), vec![g2, g1, root]);
        assert_eq!(p.lineage(g2b).unwrap(), vec![g2b, g1, root]);
        assert_eq!(p.generation(g2b).unwrap(), 2);
        assert_eq!(p.children(g1).collect::<Vec<_>>(), vec![g2, g2b]);
        assert_eq!(p.children(g2).count(), 0);
        assert_eq!(p.roots().collect::<Vec<_>>(), vec![root]);
    }

    #[test]
    fn chop_is_many_children_of_one_parent() {
        let mut p = Project::new();
        let brk = p.add_root(tone(8));
        let slices: Vec<_> =
            (0..8).map(|_| p.print(brk, tone(1)).unwrap()).collect();
        assert_eq!(p.children(brk).collect::<Vec<_>>(), slices);
        for s in slices {
            assert_eq!(p.parent(s).unwrap(), Some(brk));
        }
    }

    #[test]
    fn print_from_unknown_parent_fails() {
        let mut p = Project::new();
        let bogus = SampleId(99);
        assert_eq!(p.print(bogus, tone(1)).unwrap_err(), ProjectError::UnknownSample(bogus));
        assert!(p.is_empty());
    }

    #[test]
    fn remove_refuses_samples_with_children() {
        let mut p = Project::new();
        let root = p.add_root(tone(1));
        let child = p.print(root, tone(1)).unwrap();
        assert_eq!(p.remove(root).unwrap_err(), ProjectError::HasChildren(root));
        assert!(p.remove(child).is_ok());
        assert!(p.remove(root).is_ok());
        assert!(p.is_empty());
    }

    #[test]
    fn ids_are_never_reused() {
        let mut p = Project::new();
        let a = p.add_root(tone(1));
        p.remove(a).unwrap();
        let b = p.add_root(tone(1));
        assert_ne!(a, b);
    }
}
