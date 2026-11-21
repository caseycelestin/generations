//! Shared state made once, at `create`, and reused. The control side keeps a
//! clone of every place, so the renderer's clone is never the last and
//! nothing is freed on the real-time side; a place is free again once that
//! clone is the only one left. Knows nothing of audio.

use std::sync::Arc;

#[derive(Debug)]
pub struct Pool<T> {
    places: Vec<Arc<T>>,
}

impl<T> Pool<T> {
    /// `make` builds each place, here at `create`, so a place can be sized
    /// at runtime and still never be allocated again.
    pub fn new(places: usize, make: impl Fn() -> T) -> Self {
        let places = (0..places).map(|_| Arc::new(make())).collect();
        Self { places }
    }
}

impl<T> Pool<T> {
    /// `fill` puts `value` into a free place, reusing what the place
    /// already holds, so a refill need never allocate. `value` comes back
    /// while every place is held.
    pub fn take<V>(&mut self, value: V, fill: impl FnOnce(&mut T, V)) -> Result<Arc<T>, V> {
        for place in &mut self.places {
            if let Some(free) = Arc::get_mut(place) {
                fill(free, value);
                return Ok(Arc::clone(place));
            }
        }
        Err(value)
    }
}
