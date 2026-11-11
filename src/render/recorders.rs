//! The recorders a renderer holds, one place per tap.
//!
//! [`Recorders`] is storage and nothing else, like
//! [`Sounds`](crate::render::sounds::Sounds): it holds at most one
//! recorder at each [`Tap`], under the [`TakeId`] the control side
//! named its take by, and finds and hands them back by tap or by take. Where in the period each tap is read is the renderer's.
//!
//! Nothing is dropped here. Every way a recorder leaves hands it to the
//! caller, so the caller decides where it goes.
//!
//! Real-time safe: a fixed array with a place for every tap there is,
//! built with the renderer. Nothing allocates, locks or fails.

use crate::render::command::{TakeId, Tap};
use crate::render::dsp::Recorder;

/// How many taps there are: one place for each.
const TAPS: usize = 1;

/// Which place holds the recorder at `tap`.
fn place(tap: Tap) -> usize {
    match tap {
        Tap::MainOut => 0,
    }
}

/// At most one recorder at each tap, with its take.
#[derive(Debug, Default)]
pub struct Recorders {
    places: [Option<(TakeId, Recorder)>; TAPS],
}

impl Recorders {
    /// No recorder at any tap.
    pub fn new() -> Self {
        Self::default()
    }

    /// Holds `recorder` at `tap` as the take `id`, and hands back
    /// the one it replaces, if any.
    pub fn insert(
        &mut self,
        tap: Tap,
        id: TakeId,
        recorder: Recorder,
    ) -> Option<(TakeId, Recorder)> {
        self.places[place(tap)].replace((id, recorder))
    }

    /// The recorder at `tap`, to record into.
    pub fn get_mut(&mut self, tap: Tap) -> Option<&mut Recorder> {
        self.places[place(tap)].as_mut().map(|(_, r)| r)
    }

    /// Lets go of the recorder at `tap`, if there is one.
    pub fn remove(&mut self, tap: Tap) -> Option<(TakeId, Recorder)> {
        self.places[place(tap)].take()
    }

    /// Lets go of the recorder making the take `id`, wherever it is.
    pub fn remove_take(&mut self, id: TakeId) -> Option<(TakeId, Recorder)> {
        self.places
            .iter_mut()
            .find(|p| p.as_ref().is_some_and(|(held, _)| *held == id))?
            .take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorder() -> Recorder {
        crate::testing::recorder(1)
    }

    #[test]
    fn a_tap_holds_one_recorder_and_hands_back_the_one_it_replaces() {
        let mut recorders = Recorders::new();
        assert!(
            recorders
                .insert(Tap::MainOut, TakeId(1), recorder())
                .is_none()
        );
        let (old, _) = recorders
            .insert(Tap::MainOut, TakeId(2), recorder())
            .unwrap();
        assert_eq!(old, TakeId(1));
        assert!(recorders.get_mut(Tap::MainOut).is_some());
    }

    #[test]
    fn a_recorder_leaves_by_tap() {
        let mut recorders = Recorders::new();
        recorders.insert(Tap::MainOut, TakeId(1), recorder());
        let (id, _) = recorders.remove(Tap::MainOut).unwrap();
        assert_eq!(id, TakeId(1));
        assert!(recorders.get_mut(Tap::MainOut).is_none());
    }

    #[test]
    fn a_recorder_leaves_by_take_and_another_id_names_nothing() {
        let mut recorders = Recorders::new();
        recorders.insert(Tap::MainOut, TakeId(1), recorder());
        assert!(recorders.remove_take(TakeId(2)).is_none());
        let (id, _) = recorders.remove_take(TakeId(1)).unwrap();
        assert_eq!(id, TakeId(1));
        assert!(recorders.get_mut(Tap::MainOut).is_none());
    }
}
