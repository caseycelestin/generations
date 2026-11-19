//! The engine behind the generations sampler.
//!
//! Audio moves forward by printing: committing a change to a sample as a
//! new generation of it, with the previous one kept. Nothing prints yet, so
//! the manifest records no lineage (see the roadmap).
//!
//! # Architecture
//!
//! The project and the engine never name each other; the caller carries
//! [`Audio`] between them.
//!
//! ```text
//! config, layout   the one fixed format: 48 kHz f32, mono or stereo, on
//!                  disk, in RAM and on the render path alike
//! audio            AudioMemory, the one source of audio memory, with an
//!                  optional budget; Audio, a caller's piece of it
//! project          the disk: a directory of WAVs and a manifest
//! engine/          everything heard; create() makes both sides
//!   control          Control, the door to one renderer
//!   voice, input,    one file per feature: what crosses, its handle, and
//!   recording          its real-time part
//!   renderer, math   the audio thread
//!   transfer         owned values handed to the audio thread and back;
//!                    knows nothing of audio
//! (the caller)     owns the AudioMemory, the project and the audio device,
//!                  and hands the device the renderer to drive. The crate
//!                  has no audio dependency; examples/device/ is one
//!                  caller's code, over cpal
//! ```
//!
//! # Where to start
//!
//! [`Project`] and [`engine::create`] for what a caller does;
//! `examples/first_sound.rs` for the whole path, file to speaker;
//! `docs/roadmap.md` for the plan and the decisions behind it.

mod audio;
pub mod config;
pub mod engine;
mod layout;
pub mod project;
#[cfg(test)]
mod testing;

pub use audio::{Audio, AudioMemory, OverBudgetError};
pub use layout::{ChannelLayout, ChannelLayoutError};
pub use project::{Project, ProjectError, SampleId};
