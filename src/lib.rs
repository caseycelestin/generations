//! The engine behind the generations sampler.
//!
//! Audio moves forward by printing. See the README for the idea; this crate
//! is the data model underneath it. Start at [`project::Project`].

pub mod audio;
pub mod config;
pub mod project;
pub mod sample;

pub use audio::{AudioBuffer, AudioError};
pub use project::{Project, ProjectError};
pub use sample::{Origin, Sample, SampleId};
