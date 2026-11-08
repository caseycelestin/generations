//! Capture: frames of input in, a take out.
//!
//! The other half of the data plane. `render` turns commands and resident
//! audio into periods of output; this turns periods of input into a buffer
//! the engine can store. The rules are the render module's: no allocation,
//! no lock, no disk, no error path, no clock, because the [`Recorder`] runs
//! wherever the frames come from, live on the audio thread or offline in a
//! loop, and must not care which.
//!
//! ```text
//! control side                                audio side
//! (the engine)  mints ──▶  Take     ◀── one-slot ring ──  Recorder
//!                          holds the memory              fills the buffer,
//!                          and closes the take           hands it back once
//! ```
//!
//! A take is opened through the engine, which reserves and allocates the
//! whole buffer up front and returns the two halves. The caller moves the
//! recorder to where the frames are and keeps the take. The recorder
//! finishes when its buffer fills or when it is dropped, and either way
//! hands the buffer to the take over the ring; nothing travels the other
//! way. Closing the take, again through the engine, stores what came back
//! as a new root. Dropping it unclosed releases the memory and the take is
//! gone.

// Nothing reads these fields until `write` and `close` arrive; this is the
// shape alone.
#![allow(dead_code)]

use rtrb::{Consumer, Producer};

use crate::audio::Dither;
use crate::config::Pcm;
use crate::ledger::Reservation;

/// The audio side of a take: fills a buffer that already exists.
///
/// Moved to wherever the frames come from and fed each period. Finishes
/// when the buffer is full or when it is dropped, and either way hands
/// the buffer to its [`Take`] over the ring. Writing to a finished
/// recorder does nothing.
pub struct Recorder {
    /// The take's audio, interleaved, allocated to full capacity when the
    /// take was opened. The length is how much has been written and the
    /// capacity is the take's length. Never grows.
    data: Vec<Pcm>,
    /// Channels per frame, one or two.
    channels: u16,
    /// Quantiser state, so every sample is dithered as it lands and the
    /// buffer is in the project format from the first frame.
    dither: Dither,
    /// The one-slot ring back to the take. `None` once the buffer has
    /// been handed over, which is how the recorder knows it is finished.
    finished: Option<Producer<Vec<Pcm>>>,
}

/// The control side of a take: the memory it paid for and the way the
/// audio comes back.
///
/// Closed through the engine, which stores the audio as a new root.
/// Dropped unclosed, it releases the memory and the take is gone.
pub struct Take {
    /// The bytes the buffer costs, held until the take is closed or
    /// dropped. The recorder's buffer is what it pays for.
    reservation: Reservation,
    /// Channels per frame, one or two; the frame count of what comes
    /// back is its length divided by this.
    channels: u16,
    /// Whether the stored sample stays in RAM as its loaded copy, rather
    /// than being dropped once it is on disk.
    keep_loaded: bool,
    /// The receiving end of the ring. Empty until the recorder finishes.
    finished: Consumer<Vec<Pcm>>,
}
