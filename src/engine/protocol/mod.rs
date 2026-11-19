//! What crosses between the two sides, owned by neither. Each feature has
//! its own file and its own channel; handles stand for what crosses on the
//! control side.
//!
//! Each is a transfer: its data goes to the renderer and comes back whole
//! once it ends, and its state is shared by both sides until then.

pub(crate) mod inputs;
pub(crate) mod recording;
pub(crate) mod voice;
