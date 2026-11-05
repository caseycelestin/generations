//! Audio held in RAM for playback, under a budget.
//!
//! The [`ResidentSet`] knows nothing about the project, the device or
//! voices: it holds buffers by id and keeps count. The budget is enforced
//! here and nowhere else, because this is the one place that knows how
//! many bytes are resident. The memory itself is ordinary heap allocation:
//! each buffer owns its own block, and the guarantee that the budget's
//! worth of RAM exists is the operating system's job (a reservation for
//! the engine's process on the device), not the engine's.

use std::collections::HashMap;
use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::playback::PlaybackError;
use crate::project::SampleId;

/// Audio held in RAM for playback, under a byte budget.
///
/// Buffers are shared through [`Arc`] so a voice can hold one while it
/// plays. Only the control thread inserts and removes; the set is not for
/// the renderer, which only ever reads the clones it was handed.
#[derive(Debug)]
pub struct ResidentSet {
    budget: usize,
    bytes: usize,
    audio: HashMap<SampleId, Arc<AudioBuffer>>,
}

impl ResidentSet {
    /// An empty set that will hold at most `budget` bytes of audio.
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            bytes: 0,
            audio: HashMap::new(),
        }
    }

    /// Makes `audio` resident as sample `id`.
    ///
    /// Refused, with nothing changed, if it would exceed the budget. A
    /// sample's audio never changes, so inserting an id that is already
    /// resident changes nothing and costs nothing.
    pub fn insert(&mut self, id: SampleId, audio: AudioBuffer) -> Result<(), PlaybackError> {
        if self.audio.contains_key(&id) {
            return Ok(());
        }
        let requested = audio.bytes();
        if self.bytes + requested > self.budget {
            return Err(PlaybackError::OverBudget {
                requested,
                resident: self.bytes,
                budget: self.budget,
            });
        }
        self.bytes += requested;
        self.audio.insert(id, Arc::new(audio));
        Ok(())
    }

    /// Drops sample `id` from the set. Removing an id that is not resident
    /// changes nothing.
    ///
    /// The bytes come off the count at once, even if a voice still holds a
    /// clone; the memory itself is freed when the last holder lets go.
    pub fn remove(&mut self, id: SampleId) {
        if let Some(audio) = self.audio.remove(&id) {
            self.bytes -= audio.bytes();
        }
    }

    /// The resident audio of sample `id`, shared, or `None` if it is not
    /// resident.
    pub fn get(&self, id: SampleId) -> Option<Arc<AudioBuffer>> {
        self.audio.get(&id).cloned()
    }

    /// Whether sample `id` is resident.
    pub fn contains(&self, id: SampleId) -> bool {
        self.audio.contains_key(&id)
    }

    /// Bytes of audio resident right now.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The budget this set was built with.
    pub fn budget(&self) -> usize {
        self.budget
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mono buffer of `frames` frames, costing `frames * 2` bytes.
    fn mono(frames: usize) -> AudioBuffer {
        AudioBuffer::new(1, vec![0; frames]).unwrap()
    }

    #[test]
    fn insert_makes_audio_resident_and_counts_it() {
        let mut r = ResidentSet::new(100);
        assert_eq!(r.bytes(), 0);
        r.insert(SampleId(1), mono(10)).unwrap();
        assert!(r.contains(SampleId(1)));
        assert_eq!(r.bytes(), 20);
        assert_eq!(r.get(SampleId(1)).unwrap().frames(), 10);
        assert!(r.get(SampleId(2)).is_none());
    }

    #[test]
    fn insert_over_budget_is_refused_and_changes_nothing() {
        let mut r = ResidentSet::new(50);
        r.insert(SampleId(1), mono(20)).unwrap();
        let err = r.insert(SampleId(2), mono(10)).unwrap_err();
        assert_eq!(
            err,
            PlaybackError::OverBudget {
                requested: 20,
                resident: 40,
                budget: 50,
            }
        );
        assert!(!r.contains(SampleId(2)));
        assert_eq!(r.bytes(), 40);
    }

    #[test]
    fn insert_exactly_to_the_budget_is_allowed() {
        let mut r = ResidentSet::new(40);
        r.insert(SampleId(1), mono(20)).unwrap();
        assert_eq!(r.bytes(), 40);
    }

    #[test]
    fn inserting_a_resident_id_again_changes_nothing() {
        let mut r = ResidentSet::new(100);
        r.insert(SampleId(1), mono(10)).unwrap();
        r.insert(SampleId(1), mono(10)).unwrap();
        assert_eq!(r.bytes(), 20);
    }

    #[test]
    fn remove_frees_the_budget() {
        let mut r = ResidentSet::new(40);
        r.insert(SampleId(1), mono(20)).unwrap();
        r.remove(SampleId(1));
        assert!(!r.contains(SampleId(1)));
        assert_eq!(r.bytes(), 0);
        r.insert(SampleId(2), mono(20)).unwrap();
        // Removing what is not there is not an error.
        r.remove(SampleId(1));
        assert_eq!(r.bytes(), 40);
    }

    #[test]
    fn a_held_clone_outlives_removal() {
        let mut r = ResidentSet::new(100);
        r.insert(SampleId(1), mono(10)).unwrap();
        let held = r.get(SampleId(1)).unwrap();
        r.remove(SampleId(1));
        assert_eq!(r.bytes(), 0);
        assert_eq!(held.frames(), 10);
        assert_eq!(Arc::strong_count(&held), 1);
    }
}
