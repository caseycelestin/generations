//! A voice: one playing of a loaded audio, in one of its fixed slots,
//! started and stopped through atomics alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::engine::audio::AudioState;

/// One voice of a loaded audio, driven by atomics alone.
#[derive(Debug, Default)]
pub(crate) struct VoiceSlot {
    /// The number of a voice waiting to start, or 0. The renderer takes it,
    /// leaving 0, so a request is seen once.
    incoming_voice: AtomicU32,
    /// Raised to a stopped voice's number, never lowered, so an older
    /// voice's handle cannot stop a newer one; the playing voice stops once
    /// this reaches its number.
    stop: AtomicU32,
    /// The number of the last voice the renderer finished here, played to
    /// its end or stopped; the slot is free once this is the last one sent.
    done: AtomicU32,
    /// The number of the voice the renderer is playing here, or 0. The
    /// renderer alone writes it.
    playing: AtomicU32,
    /// The playing voice's next frame, from the start of the buffer. The
    /// renderer alone writes it, so the control side can read where a voice
    /// is.
    playhead: AtomicUsize,
}

impl VoiceSlot {
    /// Back to a slot that has never played. Taking `&mut self` means
    /// nothing else holds it; replacing the whole value means no field is
    /// missed.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// The control side's: asks for voice `number` to start at the
    /// renderer's next block, replacing any request not yet taken.
    pub(crate) fn send_voice(&self, number: u32) {
        self.incoming_voice.store(number, Ordering::Release);
    }

    /// The control side's: asks for voice `number` to stop at the renderer's
    /// next block, if it is still the one in this slot.
    pub(crate) fn request_stop(&self, number: u32) {
        self.stop.fetch_max(number, Ordering::Release);
    }

    /// The renderer's: voice `number` has been asked to stop.
    pub(crate) fn stopped(&self, number: u32) -> bool {
        self.stop.load(Ordering::Acquire) >= number
    }

    /// The renderer's: starts voice `number` from the top, cutting whatever
    /// was playing.
    pub(crate) fn start(&self, number: u32) {
        self.playing.store(number, Ordering::Relaxed);
        self.playhead.store(0, Ordering::Relaxed);
    }

    /// The renderer's: the voice playing here, if any.
    pub(crate) fn playing(&self) -> Option<u32> {
        match self.playing.load(Ordering::Relaxed) {
            0 => None,
            number => Some(number),
        }
    }

    /// The renderer's: voice `number` has ended here, played out or stopped.
    pub(crate) fn finish(&self, number: u32) {
        self.playing.store(0, Ordering::Relaxed);
        self.done.store(number, Ordering::Release);
    }

    /// The control side's: voice `number` has ended; when it is the last one
    /// sent, the slot is free.
    pub(crate) fn finished(&self, number: u32) -> bool {
        self.done.load(Ordering::Acquire) == number
    }

    /// The renderer's: where the playing voice is.
    pub(crate) fn playhead(&self) -> usize {
        self.playhead.load(Ordering::Relaxed)
    }

    /// The renderer's: the playing voice has reached `frame`.
    pub(crate) fn set_playhead(&self, frame: usize) {
        self.playhead.store(frame, Ordering::Relaxed);
    }

    /// The renderer's: the number of the voice waiting to start, if any.
    pub(crate) fn take_incoming_voice(&self) -> Option<u32> {
        // Read first, so a slot with nothing waiting is never written.
        if self.incoming_voice.load(Ordering::Relaxed) == 0 {
            return None;
        }
        match self.incoming_voice.swap(0, Ordering::Acquire) {
            0 => None,
            number => Some(number),
        }
    }
}

/// One voice played from a loaded audio.
#[derive(Debug)]
#[must_use = "dropping it stops the voice"]
pub struct Voice<const VOICES: usize> {
    state: Arc<AudioState<VOICES>>,
    /// Which of the audio's voice slots it plays in.
    slot: usize,
    /// Which voice of that slot it is.
    number: u32,
    /// Let go without stopping; the voice plays out.
    detached: bool,
}

impl<const VOICES: usize> Voice<VOICES> {
    pub(crate) fn play(state: Arc<AudioState<VOICES>>, slot: usize, number: u32) -> Self {
        Self {
            state,
            slot,
            number,
            detached: false,
        }
    }

    /// Gives up the handle without stopping the voice, so it plays to its
    /// end; nothing can stop it after.
    pub fn detach(mut self) {
        self.detached = true;
    }
}

/// A voice plays while its handle is held, unless detached.
impl<const VOICES: usize> Drop for Voice<VOICES> {
    fn drop(&mut self) {
        if !self.detached {
            self.state.voices[self.slot].request_stop(self.number);
        }
    }
}

pub(crate) mod realtime {
    use super::VoiceSlot;
    use crate::engine::audio::AudioBuffer;
    use crate::engine::renderer::{Frame, OUTPUT_CHANNELS};
    use crate::layout::ChannelLayout;

    /// The slot's playing voice for one block, from its playhead. Silence
    /// once the buffer runs out; mono goes to both sides at full level.
    pub(crate) fn render(slot: &VoiceSlot, buffer: &AudioBuffer, scratch: &mut [Frame]) {
        let playhead = slot.playhead();
        let channels = buffer.layout.count();
        let frames = buffer.samples.len() / channels;
        let end = (playhead + scratch.len()).min(frames);
        let samples = &buffer.samples[playhead * channels..end * channels];
        let (sound, silence) = scratch.split_at_mut(end - playhead);
        match buffer.layout {
            ChannelLayout::Mono => {
                for (frame, &sample) in sound.iter_mut().zip(samples) {
                    *frame = [sample, sample];
                }
            }
            ChannelLayout::Stereo => {
                let (pairs, _) = samples.as_chunks::<2>();
                sound.copy_from_slice(pairs);
            }
        }
        silence.fill([0.0; OUTPUT_CHANNELS]);
        slot.set_playhead(end);
    }

    /// The slot's playing voice has rendered its last frame.
    pub(crate) fn finished(slot: &VoiceSlot, buffer: &AudioBuffer) -> bool {
        slot.playhead() == buffer.samples.len() / buffer.layout.count()
    }
}
