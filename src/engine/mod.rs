//! Real-time audio and the code that controls it, split by the thread it
//! runs on. What both sides share, the protocol, lives here. There is no
//! `Engine` type.

pub(crate) mod control;
pub(crate) mod protocol;
pub(crate) mod realtime;

pub use control::{Control, ControlError, Voice, VoiceState};
pub use protocol::voice::VoiceEnd;
pub use realtime::render::Renderer;

use std::num::NonZeroUsize;

use rtrb::RingBuffer;

/// Makes both rings and hands each side its ends. The [`Renderer`] goes
/// to the audio thread; the [`Control`] stays. `voices` is how many can
/// play at once: the caller's call, for its hardware.
pub fn create(command_capacity: NonZeroUsize, voices: NonZeroUsize) -> (Control, Renderer) {
    let (to_renderer, from_control) = RingBuffer::new(command_capacity.get());
    let (to_control, from_renderer) = RingBuffer::new(return_capacity(command_capacity, voices));
    (
        Control::new(to_renderer, from_renderer),
        Renderer::new(from_control, to_control, voices),
    )
}

/// Depth of the ring back, so the renderer never finds it full. Every send
/// collects first, so between collects it holds at most every voice the
/// renderer holds, plus one return per queued command.
fn return_capacity(command_capacity: NonZeroUsize, voices: NonZeroUsize) -> usize {
    voices.get() + command_capacity.get()
}
