//! Recording: frames of input in, a take out.
//!
//! The other half of the data plane. `render` turns commands and resident
//! audio into periods of output; this will turn periods of input into
//! buffers the engine can store. The [`Recorder`] is the control side: the
//! engine mints it, paying for its memory out of the budget as it does, and
//! the caller keeps it. The audio side, the take, is not here yet.

use crate::ledger::Reservation;

/// The control side of recording: holds the memory a recording will use
/// and will mint takes into it.
///
/// Minted by the engine, kept by the caller. It owns no audio yet and
/// never goes to the audio thread. Dropping it gives its memory back.
#[derive(Debug)]
pub struct Recorder {
    /// The bytes this recorder may hold, reserved by the engine before it
    /// was made. Everything recorded through it lives inside this.
    // Read once takes are minted; until then the recorder only holds it.
    #[allow(dead_code)]
    reservation: Reservation,
}

impl Recorder {
    /// A recorder over the memory `reservation` pays for.
    pub(crate) fn new(reservation: Reservation) -> Self {
        Self { reservation }
    }
}
