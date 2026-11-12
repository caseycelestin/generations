//! The engine: the code that makes sound in real time and the code that
//! controls it, grouped by the thread it runs on, with what both share
//! here at the top.
//!
//! There is no `Engine` type. This module only groups the two sides and
//! the protocol between them.

pub(crate) mod control;
pub(crate) mod protocol;
pub(crate) mod realtime;

use rtrb::RingBuffer;

use control::Control;
use realtime::render::Renderer;

/// A new engine, as its two sides: the [`Control`] stays on the control
/// side, the [`Renderer`] goes to the audio thread. Makes the ring to the
/// renderer, `command_capacity` commands deep, and the ring back, sized
/// by [`return_capacity`], and hands each side its ends.
pub fn create(command_capacity: usize) -> (Control, Renderer) {
    let (to_renderer, from_control) = RingBuffer::new(command_capacity);
    let (to_control, from_renderer) = RingBuffer::new(return_capacity(command_capacity));
    (
        Control::new(to_renderer, from_renderer),
        Renderer::new(from_control, to_control),
    )
}

/// How deep the ring back must be so the renderer never finds it full:
/// one place for everything that can be handed back between two collects.
/// The control side collects before every send, so that is at most one
/// return per command waiting in the ring to the renderer. What the
/// renderer holds (sounds, a take) adds to this as it comes to hold them.
fn return_capacity(command_capacity: usize) -> usize {
    command_capacity
}
