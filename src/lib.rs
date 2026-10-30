//! The engine behind the generations sampler.
//!
//! Audio moves forward by printing. See the README for the idea; this crate
//! is the engine underneath it. Start at [`engine::Engine`], which owns a
//! [`project::Project`] and prints [`effects`] into it.

pub mod audio;
pub mod config;
pub mod effects;
pub mod engine;
pub mod io;
pub mod project;
pub mod sample;

pub use audio::{AudioBuffer, AudioError};
pub use engine::Engine;
pub use io::IoError;
pub use project::{Project, ProjectError};
pub use sample::{Sample, SampleId};
