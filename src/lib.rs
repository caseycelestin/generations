//! The engine behind the generations sampler.
//!
//! Audio moves forward by printing. See the README for the idea; this crate
//! is the engine underneath it. Start at [`engine::Engine`], which owns a
//! [`project::Project`], keeps it in a [`store::Store`], and prints
//! [`effects`] into it.

pub mod audio;
pub mod config;
pub mod disk;
pub mod effects;
pub mod engine;
pub mod project;
pub mod sample;
pub mod store;

pub use audio::{AudioBuffer, AudioError};
pub use disk::FileStore;
pub use engine::{Engine, EngineError};
pub use project::{Project, ProjectError};
pub use sample::{Sample, SampleId};
pub use store::{Store, StoreError};
