//! What crosses between the two sides, owned by neither.
//!
//! Each request is a task the renderer carries out, answered once it is
//! done; handles stand for them on the control side. Only plain data
//! crosses: audio and values. Each feature has its own file and its own
//! request channel.

pub(crate) mod recording;
pub(crate) mod voice;
