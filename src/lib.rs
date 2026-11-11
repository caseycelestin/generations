//! The engine behind the generations sampler.
//!
//! Audio moves forward by printing: committing a change to a sample as a
//! new generation of it, with the previous one kept. See the README for
//! the idea; this crate is the engine underneath it. The data model knows
//! generations today; the effects that will make them arrive with the
//! render side (see the roadmap), so nothing prints yet.
//!
//! # Architecture
//!
//! Two sides that never name each other, a ledger they share, and the
//! caller carrying things between them:
//!
//! ```text
//! memory      the Ledger: one memory budget, the caller's to make. Every
//!             buffer that scales with audio is a block allocated from it,
//!             and repays it when dropped
//!
//! project     the disk side. Opens on a directory, imports, loads: the
//!   │         rules over a Storage, and the one way audio moves between
//!   │         disk and RAM. Hands out a LoadedSample, plain owned audio
//!   ├─ import    the world's audio into the project's
//!   │   ├─ decode    reads a file into plain samples, whatever format it is in
//!   │   └─ convert   takes those to the project format: resample, quantise
//!   └─ storage   the medium: keeps and returns bytes, never interprets
//!        └─ files   a directory of WAVs and a manifest (the one backend)
//!
//! renderer    the sound side, in two ends like a channel. The Renderer
//!   │         goes to the audio thread and makes frames; the RendererHandle
//!   │         stays and is where every sound starts: play a LoadedSample,
//!   │         stop its sounds, add an input. Commands cross on a ring
//!   ├─ sound     a Sound: one play, stop it
//!   ├─ input     an Input you hold and monitor by source, and the InputFeed
//!   │            a capture callback fills, one ring per mono or stereo source
//!   └─ dsp       the pieces the renderer is wired from: a mixer of sources,
//!                a playhead, an input reader, the arithmetic they share.
//!                Generic; knows no command and no pipeline
//!
//! (the caller)  makes the ledger, opens the project, makes the renderer,
//!               and owns the audio device, if there is one, handing it the
//!               renderer to drive. No code in this crate knows an audio
//!               library or that there is an audio thread
//! ```
//!
//! - **[`project`]** is the disk and the budget. It owns the storage and
//!   the ledger it was opened with, because every byte it brings into RAM
//!   is a block on that ledger. An import is: decode, convert, add the
//!   result as a root. A load is: read the audio into a block, hand it out
//!   as a [`LoadedSample`], whose holder owns it. The project sends no
//!   commands and knows nothing about sound.
//! - **`renderer`** is everything heard. [`Renderer::new`] returns the
//!   renderer and its [`RendererHandle`] as a pair; the renderer runs on
//!   a thread the handle does not, turning commands and resident audio
//!   into frames wherever it is driven, live in a device callback or
//!   offline in a loop. Nothing in it knows what a project or a device is.
//!   The handle is cheap to clone and carries the verbs: `play` takes a
//!   `LoadedSample` by reference and shares its audio with the sound it
//!   starts; `stop` cuts every sound playing one. Unloading is dropping
//!   the sample.
//! - **Handles** are where things are done, and the rule for which values
//!   have verbs is one sentence: what the renderer hands out carries a
//!   clone of the `RendererHandle` and acts on its own; what the project
//!   hands out is data the renderer takes by reference. So a [`Sound`]
//!   stops, an [`InputSource`] monitors, an [`Input`] removes itself when
//!   dropped, and a [`LoadedSample`] does nothing but be played or stopped
//!   by `audio.play(&sample)` and `audio.stop(&sample)`. The asymmetry is
//!   the price of the project not knowing the renderer. Holding a value is
//!   what it means for the thing to exist, and no side keeps a list of
//!   what is loaded or playing.
//! - **`dsp`** is the pieces the pipeline is wired from, and nothing more:
//!   a mixer of sources, a playhead over a loaded sample, the reading end
//!   of an input ring, and the arithmetic they share. Each is a function
//!   over one period of frames and its own state, generic enough that a
//!   second pipeline would wire the same pieces differently. No piece
//!   knows a command exists.
//! - **`input`** is the control side of an input: the [`Input`] handle
//!   whose [`InputSource`]s are monitored and whose drop removes the
//!   input, and the [`InputFeed`] a capture callback fills. It allocates
//!   the rings when an input is added and knows what a device's frames
//!   look like, and hands the render side nothing but mono and stereo
//!   rings to read. Its one hot method, `feed`, is the fixed loop a
//!   capture callback runs, and the feed holds no handle, so that loop
//!   takes no lock.
//! - **The device is not a role of this crate.** The caller, which knows
//!   what machine it is on, opens an audio device and moves the renderer
//!   into its callback; `examples/device/` is one such caller's code, over
//!   `cpal`. The library has no audio dependency, and a machine with no
//!   device runs everything here all the same.
//! - **[`project`]** owns a `Storage` and holds nothing else. There is no
//!   in-memory copy of a project: every read and write goes to storage, so
//!   changing the project is changing the disk and there is no save step.
//!   The rules live here: a parent must exist, a sample with children stays.
//! - **`storage`** is a trait so the medium can change (a directory today,
//!   perhaps a database later) without the rules moving. A storage always
//!   holds a whole, valid project; opening one creates or validates it.
//! - **`memory`** is the one memory budget, the [`Ledger`]. The caller
//!   makes it with the machine's figure and hands it to the project.
//!   Decode, convert and the storage read each allocate their block from
//!   it and are refused what does not fit, so nothing that scales with
//!   audio exists uncounted. Audio never leaves the crate as plain samples
//!   for the same reason: a [`LoadedSample`] is a block, and the ledger
//!   knows it.
//! - **`import`** is the boundary between the world's audio and the
//!   project's, in two steps. A decoder reads one file format into a plain
//!   value (rate, channels, interleaved samples); `convert` resamples that
//!   into a buffer. Formats are added by writing a decoder;
//!   nothing about the project format moves, and the caller never sees
//!   either: it hands [`Project::import`] a path.
//! - **`audio`** and **[`config`]** hold the one fixed format every buffer
//!   uses: 48 kHz, `f32`, 1 or 2 channels, on disk, in RAM and on the
//!   render path alike.
//!
//! # Opening a project, and hearing it
//!
//! ```text
//! Ledger::new(budget)          the caller knows the machine's RAM; the crate does not
//! Project::open(dir, ledger)   the disk side: import and load
//!   └─ storage::open(dir)      the one place that knows what a path means
//!
//! Renderer::new()              the sound side: the renderer and its handle,
//!                              which plays, adds inputs, stops all
//! device.start(move |out| renderer.render(out))
//!                              the caller's own device, whatever it is
//! ```
//!
//! `storage::open` creates an empty project if the directory is empty or
//! missing, validates the whole project it finds otherwise, and refuses a
//! directory holding anything else. From then on every change is durable
//! by the time the call that made it returns. The stream plays while held;
//! drop it before the loaded samples it plays.
//!
//! # Where to start
//!
//! [`Project`] and [`RendererHandle`] for what a caller does;
//! `examples/first_sound.rs` for the whole path, file to speaker;
//! `docs/roadmap.md` for the plan and the decisions behind it.

pub mod audio;
pub mod config;
pub(crate) mod dsp;
pub(crate) mod import;
pub(crate) mod input;
pub(crate) mod loaded_sample;
pub(crate) mod memory;
pub mod project;
pub(crate) mod renderer;
pub(crate) mod renderer_handle;
pub(crate) mod sound;
pub(crate) mod storage;
#[cfg(test)]
pub(crate) mod testing;

pub use audio::{AudioBuffer, AudioError};
pub use import::{ConvertError, DecodeError, ImportError};
pub use input::{Channels, Input, InputFeed, InputSource};
pub use loaded_sample::LoadedSample;
pub use memory::{Ledger, OverBudget};
pub use project::{Project, ProjectError, Sample, SampleId};
pub use renderer::Renderer;
pub use renderer_handle::{RendererError, RendererHandle};
pub use sound::Sound;
pub use storage::StorageError;
