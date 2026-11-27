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
//! engine/          everything heard; create() makes both sides
//!   control          Control, the door to one renderer
//!   audio, inputs,   one file per feature: its shared state, its
//!   recording          handles, and its real-time part; voices belong to
//!                      loaded audio
//!   renderer, math   the audio thread
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

pub mod config;
pub mod engine;

pub use engine::{ChannelLayout, ChannelLayoutError};
