//! Import: the world's audio into the project's.
//!
//! One function, [`import`], takes a path and returns an [`AudioBuffer`]
//! in the project format. It is two steps, each its own module: `decode`
//! reads one file format into a plain value (rate, channels, interleaved
//! samples), and `convert` takes that to the project format. Formats are
//! added by writing a decoder; the project format never moves, and the
//! engine sees neither step.

use std::path::Path;
use std::sync::Arc;

use thiserror::Error;

use crate::audio::AudioBuffer;
use crate::memory::Ledger;

pub mod convert;
pub mod decode;

pub use convert::ConvertError;
pub use decode::DecodeError;

/// Reasons a file cannot be imported: it could not be read, or what was
/// read could not be made into project audio.
#[derive(Debug, Error)]
pub enum ImportError {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error(transparent)]
    Convert(#[from] ConvertError),
}

/// Reads the file at `path`, whatever format it is in, into project
/// audio: 48 kHz, `f32`, its own channel count up to two.
///
/// The buffers in flight are blocks from `ledger` while they exist: the
/// decoded samples, sized from the file's header before any are read,
/// and the converted audio, which the conversion allocates once it knows
/// the length, or which is the decoded block itself when the file is
/// already at the project rate. A file too large to pass through RAM
/// beside what the ledger already holds is refused, at whichever of the
/// two allocations does not fit.
pub fn import(path: &Path, ledger: &Arc<Ledger>) -> Result<AudioBuffer, ImportError> {
    let decoded = decode::decode(path, ledger)?;
    Ok(convert::convert(decoded, ledger)?)
}
