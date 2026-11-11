//! The sounds a renderer holds, by their ids, in a fixed number of
//! places.
//!
//! [`Sounds`] is storage and nothing else. It holds up to its capacity of
//! items, each under the [`SoundId`] the control side named it by, and
//! finds, hands back and lets go of them by id or by a test. It knows
//! nothing of what it holds: what to do when it is full, which sound to
//! cut and how a sound is heard are the renderer's, made from these
//! methods.
//!
//! Nothing is dropped here. Every way an item leaves hands it to the
//! caller, so the caller decides where its last reference goes.
//!
//! Real-time safe once built: the places are allocated once by
//! [`new`](Sounds::new), on the control side, and never grow. Each method
//! is one pass over them; nothing allocates, locks or fails.

use crate::render::command::SoundId;

/// Up to a fixed number of items, each under its [`SoundId`].
#[derive(Debug)]
pub struct Sounds<T> {
    /// One per place; an empty place holds nothing.
    places: Box<[Option<(SoundId, T)>]>,
}

impl<T> Sounds<T> {
    /// Room for `capacity` sounds, none held. Allocates; call it on the
    /// control side.
    pub fn new(capacity: usize) -> Self {
        Self {
            places: (0..capacity).map(|_| None).collect(),
        }
    }

    /// Holds `item` as `id` in an empty place, or hands it back if there
    /// is none.
    pub fn insert(&mut self, id: SoundId, item: T) -> Result<(), T> {
        match self.places.iter_mut().find(|p| p.is_none()) {
            Some(place) => {
                *place = Some((id, item));
                Ok(())
            }
            None => Err(item),
        }
    }

    /// Lets go of the sound `id` and hands it back. `None` if no sound is
    /// held as `id`.
    pub fn remove(&mut self, id: SoundId) -> Option<T> {
        let place = self
            .places
            .iter_mut()
            .find(|p| p.as_ref().is_some_and(|(held, _)| *held == id))?;
        place.take().map(|(_, item)| item)
    }

    /// Lets go of every sound that passes `test`, handing each to `sink`.
    pub fn remove_where(&mut self, mut test: impl FnMut(&T) -> bool, mut sink: impl FnMut(T)) {
        for place in self.places.iter_mut() {
            if place.as_ref().is_some_and(|(_, item)| test(item))
                && let Some((_, item)) = place.take()
            {
                sink(item);
            }
        }
    }

    /// The sound `id`, to change in place.
    pub fn get_mut(&mut self, id: SoundId) -> Option<&mut T> {
        self.places
            .iter_mut()
            .flatten()
            .find(|(held, _)| *held == id)
            .map(|(_, item)| item)
    }

    /// The oldest sound held. Ids are minted in order, so it is the
    /// lowest.
    pub fn oldest(&self) -> Option<SoundId> {
        self.places.iter().flatten().map(|(id, _)| *id).min()
    }

    /// Every sound held, to run.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.places.iter_mut().flatten().map(|(_, item)| item)
    }

    /// Whether a sound is held as `id`.
    #[cfg(test)]
    pub(crate) fn contains(&self, id: SoundId) -> bool {
        self.places.iter().flatten().any(|(held, _)| *held == id)
    }

    /// How many sounds are held.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.places.iter().flatten().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sounds holding plain numbers, so a test can see which came back.
    fn sounds(capacity: usize) -> Sounds<u32> {
        Sounds::new(capacity)
    }

    #[test]
    fn an_inserted_sound_is_held_under_its_id() {
        let mut s = sounds(2);
        s.insert(SoundId(7), 70).unwrap();
        assert!(s.contains(SoundId(7)));
        assert_eq!(s.len(), 1);
        assert_eq!(s.get_mut(SoundId(7)), Some(&mut 70));
    }

    #[test]
    fn a_full_insert_hands_the_item_back() {
        let mut s = sounds(1);
        s.insert(SoundId(1), 10).unwrap();
        assert_eq!(s.insert(SoundId(2), 20), Err(20));
        assert!(!s.contains(SoundId(2)));
    }

    #[test]
    fn remove_hands_the_item_back_and_frees_its_place() {
        let mut s = sounds(1);
        s.insert(SoundId(1), 10).unwrap();
        assert_eq!(s.remove(SoundId(1)), Some(10));
        assert_eq!(s.remove(SoundId(1)), None);
        s.insert(SoundId(2), 20).unwrap();
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn removing_an_id_not_held_does_nothing() {
        let mut s = sounds(2);
        s.insert(SoundId(1), 10).unwrap();
        assert_eq!(s.remove(SoundId(99)), None);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn remove_where_hands_every_match_to_the_sink() {
        let mut s = sounds(4);
        for n in 1..=4 {
            s.insert(SoundId(n), n as u32 * 10).unwrap();
        }
        let mut gone = Vec::new();
        s.remove_where(|&item| item % 20 == 0, |item| gone.push(item));
        gone.sort();
        assert_eq!(gone, vec![20, 40]);
        assert_eq!(s.len(), 2);
        assert!(s.contains(SoundId(1)) && s.contains(SoundId(3)));
    }

    #[test]
    fn the_oldest_is_the_lowest_id() {
        let mut s = sounds(3);
        assert_eq!(s.oldest(), None);
        s.insert(SoundId(5), 0).unwrap();
        s.insert(SoundId(2), 0).unwrap();
        s.insert(SoundId(9), 0).unwrap();
        assert_eq!(s.oldest(), Some(SoundId(2)));
    }

    #[test]
    fn iter_mut_reaches_every_sound_held() {
        let mut s = sounds(3);
        s.insert(SoundId(1), 1).unwrap();
        s.insert(SoundId(2), 2).unwrap();
        for item in s.iter_mut() {
            *item *= 10;
        }
        assert_eq!(s.remove(SoundId(1)), Some(10));
        assert_eq!(s.remove(SoundId(2)), Some(20));
    }
}
