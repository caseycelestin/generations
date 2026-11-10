//! Audio processing: commands and resident audio in, frames of output out.
//!
//! This is the half of the engine that runs where the frames are needed,
//! on a thread the engine does not own.
//! Live, that is the audio thread, called once per period from a device
//! callback with a hard deadline. Offline, it is a bounce looping on the
//! control thread as fast as the CPU goes. Both get the same frames,
//! because nothing here depends on where it runs: no allocation, no lock,
//! no disk, no error path, no clock. Frame counts only.
//!
//! ```text
//! control side                          render side
//! (the engine)  ── Command ring ──▶     Renderer     the wiring: commands in, a period out
//!                                         Voices       the sounds playing: start, stop, play all
//!                                           Playhead     one sample into the period
//!                                         hard_clip    the last call
//! ```
//!
//! # The pieces: functions over data
//!
//! A piece of DSP is a function. It takes one period of interleaved
//! stereo frames and, explicitly, every bit of state and every parameter
//! it reads or writes. Nothing is hidden: the state lives in the wiring
//! that calls the function, as plain fields, arrays and small structs,
//! so everything the audio thread touches can be seen in one place and
//! is known to exist before the first period.
//!
//! A function works in place: the slice it is given is the signal, what
//! it leaves there is what the next function sees, and nothing is copied
//! between them. That covers both ends of a path with one shape. A
//! function that transforms, a gain or a clip, reads what is there and
//! writes over it. A function that produces, [`Playhead::play`], reads
//! nothing and adds its next frames to what is there, so several sum by
//! adding into one slice and one into a zeroed slice is a plain write. A
//! function that only looks, a meter or a tap, leaves the slice alone.
//!
//! A piece with state is a struct that owns it and the function is a
//! method on it, such as [`Playhead`], a buffer and a position with a
//! `play` that advances it, or [`Voices`], every sound playing with a
//! `play_all` that sums them. The struct is built whole by a constructor,
//! so the state cannot be half made, and its fields are private, so the
//! method is the only thing that changes them and any invariant has one
//! place to hold. It may also answer whether it is finished, meaning
//! `play` will never change frames again, and whatever holds it may then
//! let it go. A piece without state, [`hard_clip`], is a free function.
//! Parameters are plain values the control side sets, shared where
//! sharing makes sense: one set of filter coefficients read by every
//! voice, say.
//!
//! Real-time safe once built: no piece allocates, locks or fails. State
//! is built on the control side, where it may allocate what it will need
//! for its whole life (a buffer to read, a scratch block, a delay line,
//! from the ledger if it scales with audio), and moved to where it runs.
//!
//! # Boundaries
//!
//! The dependency points one way. This module defines the [`Command`]s it
//! accepts and the [`VoiceId`]s that name the sounds playing; the control
//! side imports them. Nothing here knows what a project, a device or
//! resident audio is: audio arrives inside a command as an
//! `Arc<AudioBuffer>`, and a voice holds its clone until it finishes or is
//! told to let go. The control side names a buffer to this side by its
//! address, never by what it is.
//!
//! Outside the crate, all of this is one type with one method: the
//! [`Renderer`], which the engine mints as its output and the caller
//! drives with `render`. What it holds is the engine's business.

pub mod command;
mod hard_clip;
mod playhead;
pub mod renderer;
mod voices;

pub use command::{BufferAddr, Command, VoiceId};
pub use hard_clip::hard_clip;
pub use playhead::Playhead;
pub use renderer::Renderer;
pub use voices::Voices;
