//! Audio held in RAM for playback.
//!
//! The [`ResidentSet`] is the engine's cache in front of storage, and the
//! control side's answer to "which samples are in RAM". It knows nothing
//! about the project, the device, voices or the budget: it holds buffers
//! by id, each with the [`Reservation`] that paid for it, and the bytes go
//! back to the ledger when the buffer leaves. The memory itself is
//! ordinary heap allocation: each buffer owns its own block, and the
//! guarantee that the budget's worth of RAM exists is the operating
//! system's job (a reservation for the engine's process on the device),
//! not the engine's.

use std::collections::HashMap;
use std::sync::Arc;

use super::ledger::Reservation;
use crate::audio::AudioBuffer;
use crate::project::SampleId;

/// Audio held in RAM for playback, each buffer with its reservation.
///
/// Buffers are shared through [`Arc`] so a voice can hold one while it
/// plays. Only the engine inserts and removes; the set never crosses to
/// the render side, which only ever reads the clones it was handed.
#[derive(Debug, Default)]
pub struct ResidentSet {
    audio: HashMap<SampleId, (Arc<AudioBuffer>, Reservation)>,
}

impl ResidentSet {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes `audio` resident as sample `id`, keeping `reservation` with
    /// it for as long as it stays.
    ///
    /// A sample's audio never changes, so inserting an id that is already
    /// resident changes nothing; the reservation offered for it is
    /// released at once.
    pub fn insert(&mut self, id: SampleId, audio: AudioBuffer, reservation: Reservation) {
        self.audio
            .entry(id)
            .or_insert_with(|| (Arc::new(audio), reservation));
    }

    /// Drops sample `id` from the set. Removing an id that is not resident
    /// changes nothing.
    ///
    /// The reservation is released at once, even if a voice still holds a
    /// clone; the memory itself is freed when the last holder lets go.
    pub fn remove(&mut self, id: SampleId) {
        self.audio.remove(&id);
    }

    /// The resident audio of sample `id`, shared, or `None` if it is not
    /// resident.
    pub fn get(&self, id: SampleId) -> Option<Arc<AudioBuffer>> {
        self.audio.get(&id).map(|(audio, _)| Arc::clone(audio))
    }

    /// Whether sample `id` is resident.
    pub fn contains(&self, id: SampleId) -> bool {
        self.audio.contains_key(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ledger::Ledger;

    /// A mono buffer of `frames` frames, costing `frames * 2` bytes.
    fn mono(frames: usize) -> AudioBuffer {
        AudioBuffer::new(1, vec![0; frames]).unwrap()
    }

    /// Inserts `audio` as `id`, paying for it from `ledger`.
    fn insert(r: &mut ResidentSet, ledger: &Ledger, id: SampleId, audio: AudioBuffer) {
        let reservation = ledger.reserve(audio.bytes()).unwrap();
        r.insert(id, audio, reservation);
    }

    #[test]
    fn insert_makes_audio_resident_and_holds_its_reservation() {
        let ledger = Ledger::new(100);
        let mut r = ResidentSet::new();
        insert(&mut r, &ledger, SampleId(1), mono(10));
        assert!(r.contains(SampleId(1)));
        assert_eq!(ledger.in_use(), 20);
        assert_eq!(r.get(SampleId(1)).unwrap().frames(), 10);
        assert!(r.get(SampleId(2)).is_none());
    }

    #[test]
    fn inserting_a_resident_id_again_releases_the_offered_reservation() {
        let ledger = Ledger::new(100);
        let mut r = ResidentSet::new();
        insert(&mut r, &ledger, SampleId(1), mono(10));
        insert(&mut r, &ledger, SampleId(1), mono(10));
        assert_eq!(ledger.in_use(), 20);
    }

    #[test]
    fn remove_releases_the_reservation() {
        let ledger = Ledger::new(40);
        let mut r = ResidentSet::new();
        insert(&mut r, &ledger, SampleId(1), mono(20));
        r.remove(SampleId(1));
        assert!(!r.contains(SampleId(1)));
        assert_eq!(ledger.in_use(), 0);
        insert(&mut r, &ledger, SampleId(2), mono(20));
        // Removing what is not there is not an error.
        r.remove(SampleId(1));
        assert_eq!(ledger.in_use(), 40);
    }

    #[test]
    fn a_held_clone_outlives_removal() {
        let ledger = Ledger::new(100);
        let mut r = ResidentSet::new();
        insert(&mut r, &ledger, SampleId(1), mono(10));
        let held = r.get(SampleId(1)).unwrap();
        r.remove(SampleId(1));
        assert_eq!(ledger.in_use(), 0);
        assert_eq!(held.frames(), 10);
        assert_eq!(Arc::strong_count(&held), 1);
    }
}
