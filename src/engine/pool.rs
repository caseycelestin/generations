//! Shared state made once, at `create`, and reused. The control side keeps a
//! clone of every place, so the renderer's clone is never the last and
//! nothing is freed on the real-time side; a place is free again once that
//! clone is the only one left. Knows nothing of audio.

use std::sync::Arc;

#[derive(Debug)]
pub struct Pool<T> {
    places: Vec<Arc<T>>,
}

impl<T: Default> Pool<T> {
    pub fn new(places: usize) -> Self {
        let places = (0..places).map(|_| Arc::new(T::default())).collect();
        Self { places }
    }

    /// Reset whole, so nothing from its last use carries over. `None` while
    /// every place is held.
    pub fn take(&mut self) -> Option<Arc<T>> {
        self.places.iter_mut().find_map(|place| {
            let free = Arc::get_mut(place)?;
            *free = T::default();
            Some(Arc::clone(place))
        })
    }
}
