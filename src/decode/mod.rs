//! Reading audio files into plain samples.
//!
//! One function, [`decode`], takes a path and returns a [`Decoded`]: the
//! file's audio as interleaved samples at the file's own rate, channel
//! count and width, integers kept as integers. Which format the file is
//! in is decided here, from the bytes at its start, and handed to the one
//! submodule that knows that format. A new format is a new submodule and
//! one more arm in [`sniff`].
//!
//! Nothing here knows the project format; that is `convert`'s job. A
//! decoder reports what it read, and `convert` notices when that already
//! is the project format.

mod wav;

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use thiserror::Error;

pub use crate::convert::{Decoded, Samples};
use crate::ledger::Reservation;

/// Reasons a file could not be read into samples.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("reading {path}: {source}")]
    Io {
        path: Box<Path>,
        #[source]
        source: io::Error,
    },
    /// The file's leading bytes match no format this crate reads.
    #[error("{path}: not an audio format this engine reads")]
    UnknownFormat { path: Box<Path> },
    /// The file is a known format but malformed or a variant not handled.
    #[error("{path}: {reason}")]
    Malformed { path: Box<Path>, reason: String },
    /// The reservation offered does not pay for the decoded samples.
    /// Nothing was allocated.
    #[error("{path}: decoding needs {needed} bytes but {reserved} were reserved")]
    ShortReservation {
        path: Box<Path>,
        needed: usize,
        reserved: usize,
    },
}

/// File formats this crate reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Wav,
}

/// Decides a file's format from its first bytes, or `None`.
fn sniff(head: &[u8]) -> Option<Format> {
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WAVE" {
        return Some(Format::Wav);
    }
    None
}

/// Reads an audio file into samples at its own rate and channel count.
///
/// `reservation` must cover [`byte_cost`] of the file as it is now; a short one
/// is refused before anything is allocated. The caller keeps it for as
/// long as the samples live.
pub fn decode(path: &Path, reservation: &Reservation) -> Result<Decoded, DecodeError> {
    match format(path)? {
        Format::Wav => wav::decode(path, reservation),
    }
}

/// What [`decode`] will allocate for this file, in bytes: the samples it
/// returns. Read from the file's header without decoding any audio, so
/// the memory can be reserved before it is allocated. Opens the file again when `decode`
/// follows; a header is a few bytes and the second open is cheaper than a
/// decoder that carries state between two calls.
pub fn byte_cost(path: &Path) -> Result<usize, DecodeError> {
    match format(path)? {
        Format::Wav => wav::byte_cost(path),
    }
}

/// The format of the file at `path`, from its first bytes.
fn format(path: &Path) -> Result<Format, DecodeError> {
    let io = |source| DecodeError::Io {
        path: path.into(),
        source,
    };
    let mut head = [0u8; 12];
    let n = read_head(&mut File::open(path).map_err(io)?, &mut head).map_err(io)?;
    sniff(&head[..n]).ok_or_else(|| DecodeError::UnknownFormat { path: path.into() })
}

/// Fills `buf` as far as the reader goes; a short file is not an error.
fn read_head(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ledger::Ledger;

    /// A reservation that pays for anything, for tests of the decoding
    /// itself.
    pub(crate) fn paid() -> Reservation {
        Ledger::new(usize::MAX).reserve(usize::MAX).unwrap()
    }

    #[test]
    fn sniff_knows_wav_and_nothing_else() {
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), Some(Format::Wav));
        assert_eq!(sniff(b"RIFF\0\0\0\0AVI LIST"), None);
        assert_eq!(sniff(b"fLaC"), None);
        assert_eq!(sniff(b""), None);
        assert_eq!(sniff(b"RIFF"), None);
    }

    #[test]
    fn unknown_bytes_are_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noise.bin");
        std::fs::write(&path, b"hello").unwrap();
        assert!(matches!(
            decode(&path, &paid()).unwrap_err(),
            DecodeError::UnknownFormat { path: p } if *p == *path
        ));
        assert!(matches!(
            byte_cost(&path).unwrap_err(),
            DecodeError::UnknownFormat { path: p } if *p == *path
        ));
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.wav");
        assert!(matches!(
            decode(&path, &paid()).unwrap_err(),
            DecodeError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound
        ));
    }
}
