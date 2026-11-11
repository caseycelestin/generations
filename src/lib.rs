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
//! Two sides that never name each other, a ledger they share, and the
//! caller carrying things between them. The source tree is the picture:
//!
//! ```text
//! config, audio, memory   shared: the one fixed format, the one buffer type
//!                         (AudioBuffer), and the Ledger every audio-sized
//!                         block is paid for on. The caller makes the ledger
//!
//! project/                the disk side. Allocates and writes, sends nothing
//!   mod.rs                  Project: opens on a directory, imports, loads;
//!   │                       the rules over a Storage
//!   sample.rs               Sample: plain owned audio, what load returns
//!   import/                 the world's audio into the project's
//!   │   decode.rs             reads a file into plain samples, any format
//!   │   convert.rs            takes those to the project format: resample
//!   storage/                the medium: keeps and returns bytes, nothing more
//!       files.rs              a directory of WAVs and a manifest (the backend)
//!
//! control/                the sound side, control thread. May lock, allocate
//!   mod.rs                  Control: the end of the command ring you hold.
//!   │                       play a Sample, stop its sounds, add an input
//!   sample_sound.rs         SampleSound: one play, stop it
//!   input.rs                Input you hold, shaped by an InputLayout, its
//!                           InputSounds you monitor; InputFeed a capture
//!                           callback fills, one ring per sound
//!
//! render/                 the sound side, audio thread. No lock, no allocation
//!   mod.rs                  Renderer: commands in, a period of frames out
//!   command.rs              what control may ask: the protocol, defined here
//!   sound.rs                Sound: a source and its gain, as the renderer
//!   │                       plays it
//!   sounds.rs               Sounds: the fixed places sounds are held in
//!   dsp/                    the pieces the renderer is wired from: sources (a
//!                           sample reader, an input reader), gain, the maths
//!                           they share. Generic; knows no command, no pipeline
//!
//! (the caller)            makes the ledger, opens the project, makes the
//!                         renderer and its control, and owns the audio device,
//!                         if there is one, handing it the renderer to drive.
//!                         No code in this crate knows an audio library
//! ```
//!
//! `project/` and `control/` never import each other; `render/` imports
//! neither. What crosses: a `Sample` from project to control, carried
//! by the caller; commands from control to render, on a ring; frames from
//! the capture callback to render, on the input rings.
//!
//! - **[`project`]** is the disk and the budget. It owns the storage and
//!   the ledger it was opened with, because every byte it brings into RAM
//!   is a block on that ledger. An import is: decode, convert, add the
//!   result as a root. A load is: read the audio into a block, hand it out
//!   as a [`Sample`], whose holder owns it. The project sends no
//!   commands and knows nothing about sound.
//! - **`render`** is everything heard. [`Renderer::new`] returns the
//!   renderer and its [`Control`] as a pair; the renderer runs on a
//!   thread the control side does not, turning commands and resident
//!   audio into frames wherever it is driven, live in a device callback or
//!   offline in a loop. Nothing in it knows what a project or a device is.
//!   `Control` is cheap to clone and carries the verbs: `play` takes a
//!   `Sample` by reference and shares its audio with the sound it
//!   starts; `stop` cuts every sound playing one. Unloading is dropping
//!   the sample.
//! - **Handles** are where things are done, and the rule for which values
//!   have verbs is one sentence: what the renderer hands out carries a
//!   clone of the `Control` and acts on its own; what the project hands
//!   out is data the renderer takes by reference. So a [`SampleSound`] stops,
//!   an [`InputSound`] monitors, an [`Input`] removes itself when
//!   dropped, and a [`Sample`] does nothing but be played or stopped
//!   by `control.play(&sample)` and `control.stop(&sample)`. The
//!   asymmetry is the price of the project not knowing the renderer.
//!   Holding a value is what it means for the thing to exist, and no side
//!   keeps a list of what is loaded or playing. None of these types is
//!   called a handle: each is named for what it is to the caller, and the
//!   one on the renderer is `Control` because it is the control side's end
//!   of the ring.
//! - **`render/dsp`** is the pieces the pipeline is wired from, and nothing
//!   more: the `Source` trait, a sample reader over a loaded sample, the
//!   reading end of an input ring, a gain, and the arithmetic they share.
//!   Each works on one period of frames and its own state, generic enough
//!   that a second pipeline would wire the same pieces differently. No
//!   piece knows a command exists.
//! - **`control/input`** is the control side of an input: the [`Input`] you
//!   hold, whose [`InputSound`]s are monitored and whose drop removes the
//!   input, and the [`InputFeed`] a capture callback fills. It allocates
//!   the rings when an input is added and knows what a device's frames
//!   look like, and hands the render side nothing but mono and stereo
//!   rings to read. Its one hot method, `feed`, is the fixed loop a
//!   capture callback runs, and the feed holds no `Control`, so that loop
//!   takes no lock.
//! - **The device is not a role of this crate.** The caller, which knows
//!   what machine it is on, opens an audio device and moves the renderer
//!   into its callback; `examples/device/` is one such caller's code, over
//!   `cpal`. The library has no audio dependency, and a machine with no
//!   device runs everything here all the same.
//! - **[`project`]** owns a `Storage` and holds nothing else. There is no
//!   in-memory copy of a project: every read and write goes to storage, so
//!   changing the project is changing the disk and there is no save step.
//!   The rules about what may change live here.
//! - **`project/storage`** is a trait so the medium can change (a directory
//!   today, perhaps a database later) without the rules moving. A storage
//!   always holds a whole, valid project; opening one creates or validates
//!   it.
//! - **`memory`** is the one memory budget, the [`Ledger`]. The caller
//!   makes it with the machine's figure and hands it to the project.
//!   Decode, convert and the storage read each allocate their block from
//!   it and are refused what does not fit, so nothing that scales with
//!   audio exists uncounted. Audio never leaves the crate as plain samples
//!   for the same reason: a [`Sample`] is a block, and the ledger
//!   knows it.
//! - **`project/import`** is the boundary between the world's audio and the
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
//! Renderer::new(settings, ledger)
//!                              the sound side: the Renderer and its Control,
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
//! [`Project`] and [`Control`] for what a caller does;
//! `examples/first_sound.rs` for the whole path, file to speaker;
//! `docs/roadmap.md` for the plan and the decisions behind it.

pub(crate) mod audio;
pub mod config;
pub(crate) mod control;
pub(crate) mod memory;
pub mod project;
pub(crate) mod render;
#[cfg(test)]
pub(crate) mod testing;

pub use audio::{AudioError, ChannelLayout};
pub use control::input::{Input, InputFeed, InputLayout, InputSound};
pub use control::sample_sound::SampleSound;
pub use control::{Control, ControlError};
pub use memory::{Ledger, OverBudgetError};
pub use project::import::{ConvertError, DecodeError, ImportError};
pub use project::sample::Sample;
pub use project::storage::StorageError;
pub use project::{Project, ProjectError, SampleId};
pub use render::{Renderer, RendererSettings};
