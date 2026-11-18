//! What crosses between the two sides, owned by neither. Each feature has
//! its own file and its own channel; handles stand for what crosses on the
//! control side.
//!
//! Voices and inputs are transfers: their data goes to the renderer and
//! comes back whole once they end, and their state is shared by both sides
//! until then. Recording is still a request, answered once done.

pub(crate) mod inputs;
pub(crate) mod recording;
pub(crate) mod voice;
