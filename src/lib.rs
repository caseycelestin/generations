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
//! config, layout          shared: the one fixed format; mono or stereo
//! audio                   AudioMemory, where all audio memory comes from,
//!                         with an optional budget, and Audio, a user's
//!                         piece of it
//!
//! project                 the disk side: Project, a directory of WAVs and
//!                         a manifest; opens, loads, adds
//!
//! engine/                 everything heard; create() makes both sides
//!   protocol/               what crosses between the two sides
//!   control/                the control thread: Control, Voice. May lock,
//!   │                       allocate
//!   realtime/               the audio thread: the Renderer. No lock, no
//!                           allocation
//!
//! (the caller)            makes the AudioMemory, opens the project,
//!                         creates the engine, and owns the audio device, if
//!                         there is one, handing it the renderer to drive. No
//!                         code in this crate knows an audio library
//! ```
//!
//! - **[`AudioMemory`]** is the one source of audio memory: `alloc` for
//!   an empty [`Audio`] to write, and every load is paid for from it, so
//!   nothing that scales with audio exists uncounted. The caller makes one
//!   for the machine, with a budget if it wants one.
//! - **[`project`]** is the disk. Every read and write goes to its files,
//!   so changing the project is changing the disk and there is no save
//!   step.
//!   A load turns a stored sample into [`Audio`], whose holder owns it:
//!   dropping it unloads.
//! - **[`engine`]** turns commands and audio into frames. The renderer
//!   runs wherever it is driven, live in a device callback or offline in a
//!   loop; the control side sends it commands and frees what it hands
//!   back, so the audio thread never allocates or frees.
//! - **The device is not a role of this crate.** `examples/device/` is one
//!   caller's code, over `cpal`. The library has no audio dependency.
//! - **[`config`]** and **`layout`** hold the one fixed format: 48 kHz,
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
pub(crate) mod layout;
pub mod project;
#[cfg(test)]
pub(crate) mod testing;

pub use audio::{Audio, AudioMemory, OverBudgetError};
pub use layout::{ChannelLayout, ChannelLayoutError};
pub use project::{Project, ProjectError, SampleId};
