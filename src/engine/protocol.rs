//! What crosses between the control side and the real-time side: the
//! commands sent, what comes back, and the ids both use to name things.
//! Shared by both sides and owned by neither.

/// What the control side sends the renderer.
#[derive(Debug)]
pub(crate) enum Command {}

/// What the renderer hands back to the control side.
#[derive(Debug)]
pub(crate) enum Return {}
