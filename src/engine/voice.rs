//! A voice: one playing of a loaded audio, in one of its fixed slots,
//! started and stopped through atomics alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

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
}

impl VoiceSlot {
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

    /// The renderer's: voice `number` has ended here, played out or stopped.
    pub(crate) fn finish(&self, number: u32) {
        self.done.store(number, Ordering::Release);
    }

    /// The control side's: voice `number` has ended; when it is the last one
    /// sent, the slot is free.
    pub(crate) fn finished(&self, number: u32) -> bool {
        self.done.load(Ordering::Acquire) == number
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
    use crate::engine::audio::AudioBuffer;
    use crate::engine::renderer::{Frame, OUTPUT_CHANNELS};
    use crate::layout::ChannelLayout;

    /// One playing voice of a loaded audio; only the renderer holds it.
    #[derive(Debug)]
    pub(crate) struct Voice {
        /// Which voice of its slot this is, so a stop for an older one is
        /// told apart.
        number: u32,
        /// The next frame to play, counted from the start of the buffer.
        playhead: usize,
    }

    impl Voice {
        pub(crate) fn start(number: u32) -> Self {
            Self {
                number,
                playhead: 0,
            }
        }

        pub(crate) fn number(&self) -> u32 {
            self.number
        }

        /// Silence once the buffer runs out. Mono goes to both sides at full
        /// level.
        pub(crate) fn render(&mut self, buffer: &AudioBuffer, scratch: &mut [Frame]) {
            let channels = buffer.layout.count();
            let frames = buffer.samples.len() / channels;
            let end = (self.playhead + scratch.len()).min(frames);
            let samples = &buffer.samples[self.playhead * channels..end * channels];
            let (sound, silence) = scratch.split_at_mut(end - self.playhead);
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
            self.playhead = end;
        }

        /// Its last frame has been rendered.
        pub(crate) fn finished(&self, buffer: &AudioBuffer) -> bool {
            self.playhead == buffer.samples.len() / buffer.layout.count()
        }
    }
}
