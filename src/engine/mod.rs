//! Real-time audio and the code that controls it, split by the thread it
//! runs on. What both sides share, the protocol, lives here. There is no
//! `Engine` type.

pub(crate) mod control;
pub(crate) mod protocol;
pub(crate) mod realtime;

use rtrb::RingBuffer;

use control::Control;
use realtime::render::Renderer;

/// Makes both rings and hands each side its ends. The [`Renderer`] goes
/// to the audio thread; the [`Control`] stays.
pub fn create(command_capacity: usize) -> (Control, Renderer) {
    let (to_renderer, from_control) = RingBuffer::new(command_capacity);
    let (to_control, from_renderer) = RingBuffer::new(return_capacity(command_capacity));
    (
        Control::new(to_renderer, from_renderer),
        Renderer::new(from_control, to_control),
    )
}

/// Depth of the ring back, so the renderer never finds it full. Every send
/// collects first, so between collects it holds at most one return per
/// queued command, plus whatever the renderer holds (added as features
/// land).
fn return_capacity(command_capacity: usize) -> usize {
    command_capacity
}
