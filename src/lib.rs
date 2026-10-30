//! The engine behind the generations sampler.
//!
//! Audio moves forward by printing: applying an effect to a sample commits
//! the result as a new generation of that sample, and the previous one is
//! kept. See the README for the idea; this crate is the engine underneath
//! it.
//!
//! # Architecture
//!
//! Four roles, each its own module, with one rule about who talks to whom:
//!
//! ```text
//! engine     coordinates: what a UI or front panel calls
//!   │
//!   ├─ effects   pure functions, buffer in, buffer out; know nothing else
//!   │
//!   └─ project   the rules: what a project holds and what is allowed
//!        │
//!        └─ storage   the medium: keeps and returns bytes, never interprets
//!              └─ files   a directory of WAVs and a manifest (the one backend)
//! ```
//!
//! - **[`engine`]** is the only code that touches both the project and the
//!   effects. A print is: read the source's audio from the project, run the
//!   effect, add the result to the project as the source's child.
//! - **[`project`]** owns a [`Storage`] and holds nothing else. There is no
//!   in-memory copy of a project: every read and write goes to storage, so
//!   changing the project is changing the disk and there is no save step.
//!   The rules live here: a parent must exist, a sample with children stays.
//! - **[`storage`]** is a trait so the medium can change (a directory today,
//!   perhaps a database later) without the rules moving. A storage always
//!   holds a whole, valid project; opening one creates or validates it.
//! - **[`effects`]** are offline functions over an [`AudioBuffer`]. Only
//!   playback, mixing and sequencing will ever be real-time.
//! - **[`audio`]** and **[`config`]** hold the one fixed format every buffer
//!   uses: 48 kHz, 16-bit PCM, 1 or 2 channels, with `f32` for processing.
//!
//! # Opening a project
//!
//! ```text
//! Engine::open(dir)
//!   └─ storage::open(dir)      the one place that knows what a path means
//!   └─ Project::new(storage)   plain constructor; nothing left to check
//! ```
//!
//! [`storage::open`] creates an empty project if the directory is empty or
//! missing, validates the whole project it finds otherwise, and refuses a
//! directory holding anything else. From then on every change is durable
//! by the time the call that made it returns.
//!
//! # Where to start
//!
//! [`Engine`] for what a caller does; `examples/first_print.rs` for the
//! whole path on real files; `docs/roadmap.md` for the plan and the
//! decisions behind it.

pub mod audio;
pub mod config;
pub mod effects;
pub mod engine;
pub mod project;
pub mod storage;

pub use audio::{AudioBuffer, AudioError};
pub use engine::Engine;
pub use project::{Project, ProjectError, Sample, SampleId};
pub use storage::files::FileStorage;
pub use storage::{Storage, StorageError};
