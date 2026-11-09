//! Import: the world's audio into the project's.
//!
//! Two steps, each its own module, that only the engine's import calls:
//! [`decode`] reads one file format into a plain value (rate, channels,
//! interleaved samples), and [`convert`] takes that to an `AudioBuffer`
//! in the project format. Formats are added by writing a decoder; the
//! project format never moves, and the caller sees neither step.

pub mod convert;
pub mod decode;
