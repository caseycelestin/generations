//! Code with a deadline: the audio thread. No allocating, locking,
//! blocking or system calls. An offline bounce runs the same code with no
//! deadline.

pub(crate) mod render;
