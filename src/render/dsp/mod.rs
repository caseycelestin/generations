//! Audio processing pieces: the parts a pipeline is wired from.
//!
//! Nothing here is a pipeline. These are the pieces the
//! [`Renderer`](crate::render::Renderer) wires into this crate's
//! pipeline, and that any other pipeline could wire differently: the
//! [`Source`] trait, a [`SampleReader`] over a loaded sample, an
//! [`InputReader`] over a capture ring, a [`Gain`], a [`Recorder`] that
//! keeps frames, and the arithmetic they share in [`math`]. None of them knows about commands, sounds by
//! id, the control side or the device; a piece is given frames and its
//! own state and nothing else.
//!
//! Every piece runs where the frames are needed, on a thread it does not
//! own. Live, that is the audio thread, called once per period from a
//! device callback with a hard deadline. Offline, it is a bounce looping
//! on the control thread as fast as the CPU goes. Both get the same
//! frames, because nothing here depends on where it runs: no allocation,
//! no lock, no disk, no error path, no clock. Frame counts only.
//!
//! # The pieces
//!
//! Frames flow as iterators of stereo [`Frame`](math::Frame)s, one period
//! at a time. A piece that produces them is a [`Source`]: asked for a
//! period, it gives up to that many frames and moves past them, and it
//! says when it is finished, meaning it will never give a frame again, so
//! whoever holds it may let it go. A piece that transforms them, such as
//! [`Gain`], takes a source's frames and gives them back changed. The
//! iterators are lazy and allocate nothing; the frames exist only as they
//! pass through.
//!
//! Where the frames end up is the caller's. The renderer adds every
//! source into one zeroed period with [`math::add`], so sources sum by
//! accumulating and no piece needs a buffer of its own, and ends the
//! period with [`math::hard_clip`].
//!
//! A piece with state is a struct that owns it, built whole by a
//! constructor with private fields, so the method is the only thing that
//! changes them and any invariant has one place to hold: a
//! [`SampleReader`] is a buffer and a position, a [`Gain`] a level and
//! its target. A piece without state is a free function in [`math`].
//!
//! Real-time safe once built: no piece allocates, locks or fails. What a
//! piece needs for its whole life (a buffer to read, a ring, a delay
//! line, from the ledger if it scales with audio) is allocated on the
//! control side and moved to where it runs; the piece is built there
//! around it, which only moves it, and taken apart again to give it back.

mod gain;
pub mod input_reader;
pub mod math;
mod recorder;
mod sample_reader;
mod source;

pub use gain::Gain;
pub use input_reader::InputReader;
pub use recorder::Recorder;
pub use sample_reader::SampleReader;
pub use source::Source;
