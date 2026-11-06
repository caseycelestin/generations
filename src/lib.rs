//! The engine behind the generations sampler.
//!
//! Audio moves forward by printing: applying an effect to a sample commits
//! the result as a new generation of that sample, and the previous one is
//! kept. See the README for the idea; this crate is the engine underneath
//! it.
//!
//! # Architecture
//!
//! Five roles, each its own module, with one rule about who talks to whom:
//!
//! ```text
//! engine     coordinates: what a UI or front panel calls. Also the control
//!   │        side of sound: holds the audio in RAM and sends the commands
//!   │
//!   ├─ effects   pure functions, buffer in, buffer out; know nothing else
//!   │
//!   ├─ render    the other side of sound: turns commands into frames,
//!   │            wherever the caller drives it. One public type: Renderer
//!   │
//!   └─ project   the rules: what a project holds and what is allowed
//!        │
//!        └─ storage   the medium: keeps and returns bytes, never interprets
//!              └─ files   a directory of WAVs and a manifest (the one backend)
//!
//! (the caller)  owns the audio device, if there is one, and hands it the
//!               engine's output to drive. No code in this crate knows an
//!               audio library or that there is an audio thread
//! ```
//!
//! - **[`engine`]** is the only code that touches more than one role, and
//!   the only way in from outside the crate: every other role is reachable
//!   through it or not at all, so there is one path to each action. A
//!   print is: read the source's audio from the project, run the effect,
//!   add the result to the project as the source's child. A load is: read
//!   the audio from the project, put it in the resident set, mark the
//!   sample loaded. A play is: find the audio in the resident set, send a
//!   command. The engine is the control side of sound: it owns the resident
//!   set (audio in RAM under a budget) and the sending end of a lock-free
//!   command ring, and nothing it does can block the other end.
//! - **`render`** is the other end of that ring. The renderer turns
//!   commands and resident audio into frames and depends on nothing about
//!   where it runs, so a device can drive it live and a bounce can drive it
//!   offline. Nothing in it knows what a project or a device is. Outside
//!   the crate it is one type, [`Renderer`], minted once per engine as its
//!   output, with one method that fills a frame.
//! - **The device is not a role of this crate.** The caller, which knows
//!   what machine it is on, opens an audio device and moves the engine's
//!   output into its callback; `examples/device/` is one such caller's
//!   code, over `cpal`. The library has no audio dependency, and a machine
//!   with no device runs the engine all the same.
//! - **[`project`]** owns a `Storage` and holds nothing else. There is no
//!   in-memory copy of a project: every read and write goes to storage, so
//!   changing the project is changing the disk and there is no save step.
//!   The rules live here: a parent must exist, a sample with children stays.
//! - **`storage`** is a trait so the medium can change (a directory today,
//!   perhaps a database later) without the rules moving. A storage always
//!   holds a whole, valid project; opening one creates or validates it.
//! - **[`effects`]** are offline functions over an [`AudioBuffer`]. Only
//!   playback, mixing and sequencing will ever be real-time.
//! - **[`audio`]** and **[`config`]** hold the one fixed format every buffer
//!   uses: 48 kHz, 16-bit PCM, 1 or 2 channels, with `f32` for processing.
//!
//! # Opening a project, and hearing it
//!
//! ```text
//! Engine::open(dir, budget)    the caller knows the machine's RAM; the engine does not
//!   └─ storage::open(dir)      the one place that knows what a path means
//!   └─ Project::new(storage)   plain constructor; nothing left to check
//!
//! engine.output()              the renderer, taken once
//! device.start(move |out| output.render(out))
//!                              the caller's own device, whatever it is
//! ```
//!
//! `storage::open` creates an empty project if the directory is empty or
//! missing, validates the whole project it finds otherwise, and refuses a
//! directory holding anything else. From then on every change is durable
//! by the time the call that made it returns. The stream plays while held;
//! drop it before the engine.
//!
//! # Where to start
//!
//! [`Engine`] for what a caller does; `examples/first_print.rs` for the
//! whole path on real files and `examples/first_sound.rs` for the same
//! through a speaker; `docs/roadmap.md` for the plan and the decisions
//! behind it.

pub mod audio;
pub mod config;
pub mod convert;
pub mod effects;
pub mod engine;
pub mod project;
pub(crate) mod render;
pub(crate) mod storage;

pub use audio::{AudioBuffer, AudioError};
pub use convert::{ConvertError, Decoded, convert};
pub use engine::{Engine, EngineError, OverBudget};
pub use project::{Project, ProjectError, Sample, SampleId};
pub use render::{Renderer, VoiceId};
pub use storage::StorageError;
