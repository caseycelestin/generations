//! The engine behind the generations sampler.
//!
//! Audio moves forward by printing: committing a change to a sample as a
//! new generation of it, with the previous one kept. Nothing prints yet
//! (see the roadmap).
//!
//! # Architecture
//!
//! ```text
//! config, layout   the one fixed format: 48 kHz f32, mono or stereo, in
//!                  RAM and on the render path alike
//! audio            AudioMemory and Audio, what a take is recorded into
//!                  until recording changes
//! engine/          everything heard; create() makes both sides
//!   control          Control, the door to one renderer
//!   audio, voice,    one file per feature: its shared state, its handle,
//!   input,             and its real-time part; voices belong to loaded
//!   recording          audio
//!   renderer, math   the audio thread
//!   pool             shared state made once at create and reused
//!   transfer         owned values handed to the audio thread and back,
//!                    for takes; knows nothing of audio
//! (the caller)     owns the audio device and hands it the renderer to
//!                  drive, and the samples it loads. The crate has no
//!                  audio dependency; examples/device/ is one caller's
//!                  code, over cpal
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
