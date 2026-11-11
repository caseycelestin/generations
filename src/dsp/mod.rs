//! Audio processing pieces: the parts a pipeline is wired from.
//!
//! Nothing here is a pipeline. These are the pieces, each a function
//! over one period of frames, that the [`Renderer`](crate::renderer::Renderer)
//! wires into this crate's pipeline and that any other pipeline could
//! wire differently: a [`Mixer`] of [`Source`]s, a [`Playhead`] over a
//! loaded sample, an [`InputReader`] over a capture ring, and the
//! arithmetic they share in [`math`]. None of them knows about commands,
//! the control side or the device; a piece is given a slice and its own state
//! and nothing else.
//!
//! Every piece runs where the frames are needed, on a thread it does not
//! own. Live, that is the audio thread, called once per period from a
//! device callback with a hard deadline. Offline, it is a bounce looping
//! on the control thread as fast as the CPU goes. Both get the same
//! frames, because nothing here depends on where it runs: no allocation,
//! no lock, no disk, no error path, no clock. Frame counts only.
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
//! writes over it. A function that produces, a [`Playhead`]'s `add`, reads
//! nothing and adds its next frames to what is there, so several sum by
//! adding into one slice and one into a zeroed slice is a plain write. A
//! function that only looks, a meter or a tap, leaves the slice alone.
//!
//! A piece with state is a struct that owns it and the function is a
//! method on it, such as [`Playhead`], a buffer and a position with an
//! `add` that advances it, or [`Mixer`], every source heard with an
//! `add` that sums them. The struct is built whole by a constructor,
//! so the state cannot be half made, and its fields are private, so the
//! method is the only thing that changes them and any invariant has one
//! place to hold. A piece that is heard is a [`Source`]: it adds its next
//! period and answers whether it is finished, meaning `add` will never
//! change frames again, so the mixer holding it may let it go. A piece
//! without state is a free function; the arithmetic every source shares,
//! stereo frames from any width, frames summed into the period, the clip
//! at the end, is three of them in `math`.
//! Parameters are plain values the control side sets, shared where
//! sharing makes sense: one set of filter coefficients read by every
//! sample playing, say.
//!
//! Real-time safe once built: no piece allocates, locks or fails. State
//! is built on the control side, where it may allocate what it will need
//! for its whole life (a buffer to read, a scratch block, a delay line,
//! from the ledger if it scales with audio), and moved to where it runs.
//!
pub mod input;
pub mod math;
mod mixer;
mod playhead;

pub use input::InputReader;
pub use mixer::{Mixer, Source, WhenFull};
pub use playhead::{BufferAddr, Playhead};

/// Identity of one sound: one instance of something heard, a sample
/// played once or one source of an input. Minted by the control side, in order,
/// so the lowest id among the sounds is the oldest.
///
/// Never reused, so a stop that arrives after its sound was stolen or
/// finished names nothing and does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SoundId(pub u64);
