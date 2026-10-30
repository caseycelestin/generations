//! A sample's place in a lineage.
//!
//! "Generation" is not a type. It is the relationship between a sample and
//! its parent: the sample one generation behind it. Lineage is the tree
//! formed by those pointers inside a project.
//!
//! A sample does not exist until it is stored, so its id comes from storage
//! when it is added. Ids are never reused.

/// Identity of a sample within one project.
///
/// Minted by storage from a per-project counter. Not meaningful across
/// projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SampleId(pub u32);

/// Where one sample sits in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub id: SampleId,
    /// The sample one generation behind this one. `None` for a root.
    pub parent: Option<SampleId>,
}
