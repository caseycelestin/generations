//! The engine behind the generations sampler.
//!
//! Audio moves forward by printing. See the README for the idea; this crate
//! is the engine underneath it. Start at [`engine::Engine`], which opens a
//! [`project::Project`] from a path and runs on it.

pub mod audio;
pub mod config;
pub mod effects;
pub mod engine;
pub mod project;
pub mod storage;

pub use audio::{AudioBuffer, AudioError};
pub use engine::Engine;
pub use project::Project;
pub use storage::{Storage, StorageError};
