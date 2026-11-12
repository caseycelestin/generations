//! The real-time side: code that runs on a thread with a deadline, the
//! audio thread and the capture thread. Nothing here allocates, locks,
//! blocks or makes a system call. Real-time safe, not real-time only: an
//! offline bounce runs the same code with no deadline and gets the same
//! frames.

pub(crate) mod capture;
pub(crate) mod render;
