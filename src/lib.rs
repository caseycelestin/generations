//! The engine behind the generations sampler.
//!
//! Audio moves forward by printing: committing a change to a sample as a
//! new generation of it, with the previous one kept. Nothing prints yet
//! (see the roadmap).
//!
//! # Architecture
//!
//! ```text
//! config, layout   the one fixed format: 48 kHz f32, mono or stereo, on
//!                  disk, in RAM and on the render path alike
//! audio            AudioMemory, the one source of audio memory, with an
//!                  optional budget; Audio, a caller's piece of it
//! engine/          everything heard; create() makes both sides
//!   control          Control, the door to one renderer
//!   voice, input,    one file per feature: what crosses, its handle, and
//!   recording          its real-time part
//!   renderer, math   the audio thread
//!   transfer         owned values handed to the audio thread and back;
//!                    knows nothing of audio
//! (the caller)     owns the AudioMemory and the audio device, and hands
//!                  the device the renderer to drive. The crate
//!                  has no audio dependency; examples/device/ is one
//!                  caller's code, over cpal
//! ```
//!
//! # Where to start
//!
//! [`engine::create`] for what a caller does;
//! `examples/` for the engine on a device;
//! `docs/roadmap.md` for the plan and the decisions behind it.

mod audio;
pub mod config;
pub mod engine;
mod layout;
#[cfg(test)]
mod testing;

pub use audio::{Audio, AudioMemory, OverBudgetError};
pub use layout::{ChannelLayout, ChannelLayoutError};
