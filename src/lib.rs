//! The engine behind the generations sampler.
//!
//! Audio moves forward by printing: committing a change to a sample as a
//! new generation of it, with the previous one kept. See the README for
//! the idea; this crate is the engine underneath it. Nothing prints yet,
//! so the data model does not record generations: lineage returns with
//! prints (see the roadmap).
//!
//! # Architecture
//!
//! The project and the engine never name each other; the caller carries
//! audio between them. The source tree is the picture:
//!
//! ```text
//! config, audio           shared: the one fixed format, the buffer type
//!                         (AudioBuffer) and Sample (what load returns)
//!
//! project/                the disk side and the memory budget
//!   mod.rs                  Project: opens on a directory, imports, loads;
//!   │                       the rules over a Storage
//!   ledger.rs               the Ledger every audio-sized Block is paid
//!   │                       for on; the project makes it
//!   import/                 the world's audio into the project's
//!   │   decode.rs             reads a file into plain samples, any format
//!   │   convert.rs            takes those to the project format: resample
//!   storage/                the medium: keeps and returns bytes, nothing more
//!       files.rs              a directory of WAVs and a manifest (the backend)
//!
//! engine/                 everything heard; create() makes both sides
//!   protocol/               what crosses between the two sides
//!   control/                the control thread: Control, Voice. May lock,
//!   │                       allocate
//!   realtime/               the audio thread: the Renderer. No lock, no
//!                           allocation
//!
//! (the caller)            opens the project, creates the engine, and owns
//!                         the audio device, if there is one, handing it the
//!                         renderer to drive. No code in this crate knows an
//!                         audio library
//! ```
//!
//! - **[`project`]** is the disk and the budget. Every read and write goes
//!   to storage, so changing the project is changing the disk and there is
//!   no save step. Every byte of audio it brings into RAM is a block on its
//!   ledger, so nothing that scales with audio exists uncounted. A load
//!   hands out a [`Sample`], whose holder owns it: dropping it unloads.
//! - **[`engine`]** turns commands and samples into frames. The renderer
//!   runs wherever it is driven, live in a device callback or offline in a
//!   loop; the control side sends it commands and frees what it hands
//!   back, so the audio thread never allocates or frees.
//! - **The device is not a role of this crate.** `examples/device/` is one
//!   caller's code, over `cpal`. The library has no audio dependency.
//! - **`audio`** and **[`config`]** hold the one fixed format: 48 kHz,
//!   `f32`, 1 or 2 channels, on disk, in RAM and on the render path alike.
//!
//! # Where to start
//!
//! [`Project`] and [`engine::create`] for what a caller does;
//! `examples/first_sound.rs` for the whole path, file to speaker;
//! `docs/roadmap.md` for the plan and the decisions behind it.

pub(crate) mod audio;
pub mod config;
pub mod engine;
pub mod project;
#[cfg(test)]
pub(crate) mod testing;

pub use audio::{AudioError, ChannelLayout, Sample};
pub use project::import::{ConvertError, DecodeError, ImportError};
pub use project::ledger::OverBudgetError;
pub use project::storage::StorageError;
pub use project::{Project, ProjectError, SampleId};
