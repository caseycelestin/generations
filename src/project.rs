//! The pool of samples that makes up a project, and the lineage over it.

use std::collections::HashMap;

use thiserror::Error;

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
    /// Two samples were given the same id.
    #[error("duplicate sample id {}", .0.raw())]
    DuplicateSample(SampleId),
    /// A sample's parent does not come before it in the counter.
    #[error("sample {} cannot have parent {}: parents come first", .0.raw(), .1.raw())]
    ParentNotEarlier(SampleId, SampleId),
    /// The counter has not moved past the largest id in use.
    #[error("next id {next} does not follow the largest sample id {max}")]
    CounterBehind { next: u32, max: u32 },
}

/// The structure of a project: which samples exist and how they descend.
///
/// Samples live in one flat pool keyed by id. Lineage is a forest over the
/// pool via each sample's parent pointer, so a chop of one sample into many
/// is just many samples sharing a parent. Children are found by walking the
/// pool; the parent pointer is the only stored relationship.
///
/// The project holds no audio. Each sample's bytes live on disk under its
/// id, and the engine reads them when a print or playback needs them.
#[derive(Debug, Default)]
pub struct Project {
    samples: HashMap<SampleId, Sample>,
    next_id: u32,
}

impl Project {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restores a saved project from its samples, with their original ids.
    ///
    /// Checks the invariants that [`Project::add`] guarantees by
    /// construction: ids are unique, every parent exists and was minted
    /// before its child, and `next_id` is past every id in use. Parents
    /// coming first is what makes a cycle impossible.
    pub(crate) fn restore(
        samples: impl IntoIterator<Item = (SampleId, Option<SampleId>)>,
        next_id: u32,
    ) -> Result<Self, ProjectError> {
        let mut pool = HashMap::new();
        for (id, parent) in samples {
            if let Some(p) = parent
                && p >= id
            {
                return Err(ProjectError::ParentNotEarlier(id, p));
            }
            if pool.insert(id, Sample::new(id, parent)).is_some() {
                return Err(ProjectError::DuplicateSample(id));
            }
        }
        for s in pool.values() {
            if let Some(p) = s.parent()
                && !pool.contains_key(&p)
            {
                return Err(ProjectError::UnknownSample(p));
            }
        }
        if let Some(max) = pool.keys().map(|id| id.raw()).max()
            && next_id <= max
        {
            return Err(ProjectError::CounterBehind { next: next_id, max });
        }
        Ok(Self {
            samples: pool,
            next_id,
        })
    }

    /// Adds a sample to the project and returns its new id.
    ///
    /// With no parent it starts a new lineage; with one it is the next
    /// generation of that lineage. The project only records where the
    /// sample sits in the tree; storing its audio under the id is the
    /// caller's job.
    pub fn add(&mut self, parent: Option<SampleId>) -> Result<SampleId, ProjectError> {
        if let Some(p) = parent {
            self.require(p)?;
        }
        let id = SampleId(self.next_id);
        self.next_id += 1;
        self.samples.insert(id, Sample::new(id, parent));
        Ok(id)
    }

    /// Removes a sample that has no children.
    ///
    /// A sample with children is refused: later generations depend on it
    /// and the lineage must stay readable. Removing a whole lineage is a
    /// separate, explicit operation that does not exist yet.
    pub fn remove(&mut self, id: SampleId) -> Result<Sample, ProjectError> {
        self.require(id)?;
        if self.has_children(id) {
            return Err(ProjectError::HasChildren(id));
        }
        Ok(self.samples.remove(&id).expect("checked above"))
    }

    pub fn get(&self, id: SampleId) -> Option<&Sample> {
        self.samples.get(&id)
    }

    /// Every sample, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = &Sample> {
        self.samples.values()
    }

    /// The value the next minted id will take. Persisted so that ids are
    /// never reused across a save and load.
    pub(crate) fn next_id(&self) -> u32 {
        self.next_id
    }

    fn has_children(&self, id: SampleId) -> bool {
        self.samples.values().any(|s| s.parent() == Some(id))
    }

    fn require(&self, id: SampleId) -> Result<&Sample, ProjectError> {
        self.samples.get(&id).ok_or(ProjectError::UnknownSample(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walks parents from `id` back to the root, using only the public API.
    fn lineage(p: &Project, id: SampleId) -> Vec<SampleId> {
        let mut chain = vec![id];
        let mut cur = p.get(id).unwrap();
        while let Some(parent) = cur.parent() {
            chain.push(parent);
            cur = p.get(parent).unwrap();
        }
        chain
    }

    fn children(p: &Project, id: SampleId) -> Vec<SampleId> {
        let mut ids: Vec<_> = p
            .iter()
            .filter(|s| s.parent() == Some(id))
            .map(Sample::id)
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn add_without_parent_creates_a_root() {
        let mut p = Project::new();
        let id = p.add(None).unwrap();
        let s = p.get(id).unwrap();
        assert_eq!(s.id(), id);
        assert_eq!(s.parent(), None);
    }

    #[test]
    fn adding_builds_a_tree_and_going_back_branches() {
        let mut p = Project::new();
        let root = p.add(None).unwrap();
        let g1 = p.add(Some(root)).unwrap();
        let g2 = p.add(Some(g1)).unwrap();
        // Go back to g1 and print something else.
        let g2b = p.add(Some(g1)).unwrap();

        assert_eq!(p.iter().count(), 4);
        assert_eq!(lineage(&p, g2), vec![g2, g1, root]);
        assert_eq!(lineage(&p, g2b), vec![g2b, g1, root]);
        assert_eq!(children(&p, g1), vec![g2, g2b]);
        assert!(children(&p, g2).is_empty());
    }

    #[test]
    fn chop_is_many_children_of_one_parent() {
        let mut p = Project::new();
        let brk = p.add(None).unwrap();
        let slices: Vec<_> = (0..8).map(|_| p.add(Some(brk)).unwrap()).collect();
        assert_eq!(children(&p, brk), slices);
        for s in slices {
            assert_eq!(p.get(s).unwrap().parent(), Some(brk));
        }
    }

    #[test]
    fn add_with_unknown_parent_fails() {
        let mut p = Project::new();
        let bogus = SampleId(99);
        assert_eq!(
            p.add(Some(bogus)).unwrap_err(),
            ProjectError::UnknownSample(bogus)
        );
        assert_eq!(p.iter().count(), 0);
    }

    #[test]
    fn remove_refuses_samples_with_children() {
        let mut p = Project::new();
        let root = p.add(None).unwrap();
        let child = p.add(Some(root)).unwrap();
        assert_eq!(p.remove(root).unwrap_err(), ProjectError::HasChildren(root));
        assert!(p.remove(child).is_ok());
        assert!(p.remove(root).is_ok());
        assert_eq!(p.iter().count(), 0);
    }

    #[test]
    fn restore_rebuilds_a_tree() {
        let p = Project::restore(
            vec![(SampleId(0), None), (SampleId(2), Some(SampleId(0)))],
            5,
        )
        .unwrap();
        assert_eq!(lineage(&p, SampleId(2)), vec![SampleId(2), SampleId(0)]);
        assert_eq!(p.next_id(), 5);
    }

    #[test]
    fn restore_rejects_broken_invariants() {
        let dangling = Project::restore(vec![(SampleId(1), Some(SampleId(0)))], 2);
        assert_eq!(
            dangling.unwrap_err(),
            ProjectError::UnknownSample(SampleId(0))
        );

        let backwards = Project::restore(
            vec![(SampleId(0), Some(SampleId(1))), (SampleId(1), None)],
            2,
        );
        assert_eq!(
            backwards.unwrap_err(),
            ProjectError::ParentNotEarlier(SampleId(0), SampleId(1))
        );

        let duplicate = Project::restore(vec![(SampleId(0), None), (SampleId(0), None)], 1);
        assert_eq!(
            duplicate.unwrap_err(),
            ProjectError::DuplicateSample(SampleId(0))
        );

        let behind = Project::restore(vec![(SampleId(3), None)], 3);
        assert_eq!(
            behind.unwrap_err(),
            ProjectError::CounterBehind { next: 3, max: 3 }
        );
    }

    #[test]
    fn ids_are_never_reused() {
        let mut p = Project::new();
        let a = p.add(None).unwrap();
        p.remove(a).unwrap();
        let b = p.add(None).unwrap();
        assert_ne!(a, b);
    }
}
