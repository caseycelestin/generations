//! A voice: one playing of some audio.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use thiserror::Error;

use crate::audio::{Audio, AudioBuffer};
use crate::engine::transfer::{self, ControlEnd};

#[derive(Debug, Default)]
pub(crate) struct VoiceState {
    pub(crate) stop: AtomicBool,
}

#[derive(Debug)]
pub(crate) struct VoiceData {
    pub(crate) audio: Arc<AudioBuffer>,
    pub(crate) state: Arc<VoiceState>,
}

/// Nothing was played.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum VoiceError {
    /// Stop one, or wait for one to end, then try again.
    #[error("every voice is held")]
    Full,
    #[error("the renderer is gone")]
    RendererGone,
}

impl From<transfer::StartError<VoiceData>> for VoiceError {
    fn from(error: transfer::StartError<VoiceData>) -> Self {
        match error {
            transfer::StartError::Full(_) => Self::Full,
            transfer::StartError::Disconnected(_) => Self::RendererGone,
        }
    }
}

/// Its voice holds a place until it ends, whether the handle is kept or
/// dropped.
#[derive(Debug)]
pub struct Voice {
    state: Arc<VoiceState>,
}

impl Voice {
    pub(crate) fn play(
        transfers: &mut ControlEnd<VoiceData>,
        audio: &Audio,
    ) -> Result<Self, VoiceError> {
        let audio_clone = Arc::clone(audio.shared());
        let state = Arc::new(VoiceState::default());
        let data = VoiceData {
            audio: audio_clone,
            state: Arc::clone(&state),
        };
        transfers.try_start(data)?;
        Ok(Self { state })
    }

    /// Dropping the handle instead lets the voice play out.
    pub fn stop(self) {
        self.state.stop.store(true, Ordering::Relaxed);
    }
}

/// The channel holds every voice playing plus `queue` more on their way,
/// each from play until it ends.
pub(crate) fn channel(
    voices: NonZeroUsize,
    queue: usize,
) -> (ControlEnd<VoiceData>, realtime::Voices) {
    let (control, realtime) = transfer::channel(voices.get() + queue);
    (control, realtime::Voices::new(voices, realtime))
}

/// Each voice while it plays, and the places they live.
pub(crate) mod realtime {
    use std::num::NonZeroUsize;
    use std::sync::atomic::Ordering;

    use super::VoiceData;
    use crate::engine::renderer::{Frame, Renderer};
    use crate::engine::transfer::{Completion, RealtimeEnd};
    use crate::layout::ChannelLayout;

    #[derive(Debug)]
    pub(crate) struct Voice {
        data: Completion<VoiceData>,
        /// The next frame to play.
        position: usize,
    }

    impl Voice {
        fn new(data: Completion<VoiceData>) -> Self {
            Self { data, position: 0 }
        }

        /// Silence once the audio runs out. Mono goes to both sides at full
        /// level.
        pub(crate) fn render(&mut self, scratch: &mut [Frame]) {
            let audio = &self.data.audio;
            let end = (self.position + scratch.len()).min(audio.frames());
            let samples = audio.frames_in(self.position..end);
            let (sound, silence) = scratch.split_at_mut(end - self.position);
            match audio.layout() {
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
            silence.fill([0.0; Renderer::OUTPUT_CHANNELS]);
            self.position = end;
        }

        /// Its handle asked it to stop.
        fn stopped(&self) -> bool {
            self.data.state.stop.load(Ordering::Relaxed)
        }

        /// Its last frame has been rendered.
        fn finished(&self) -> bool {
            self.position == self.data.audio.frames()
        }

        /// The one way a voice leaves.
        fn end(self, transfers: &mut RealtimeEnd<VoiceData>) {
            transfers.complete(self.data);
        }
    }

    #[derive(Debug)]
    pub(crate) struct Voices {
        /// Oldest first. Never past `places`, so adding never allocates.
        playing: Vec<Voice>,
        places: usize,
        /// The only way voices arrive or leave.
        transfers: RealtimeEnd<VoiceData>,
    }

    impl Voices {
        /// At least one place, so a new voice always has one or an oldest to
        /// cut.
        pub(crate) fn new(places: NonZeroUsize, transfers: RealtimeEnd<VoiceData>) -> Self {
            Self {
                playing: Vec::with_capacity(places.get()),
                places: places.get(),
                transfers,
            }
        }

        /// New voices join, cutting the oldest when every place is taken;
        /// stopped ones end.
        pub(crate) fn receive(&mut self) {
            while let Some(data) = self.transfers.try_recv() {
                if self.playing.len() == self.places {
                    let oldest = self.playing.remove(0);
                    oldest.end(&mut self.transfers);
                }
                self.playing.push(Voice::new(data));
            }
            self.end_if(Voice::stopped);
        }

        /// Voices whose audio has run out end.
        pub(crate) fn end_finished(&mut self) {
            self.end_if(Voice::finished);
        }

        pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Voice> {
            self.playing.iter_mut()
        }

        fn end_if(&mut self, test: impl Fn(&Voice) -> bool) {
            for voice in self.playing.extract_if(.., |voice| test(voice)) {
                voice.end(&mut self.transfers);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::Arc;

        use super::*;
        use crate::engine::transfer::{self, Transfer};
        use crate::engine::voice::VoiceState;
        use crate::testing::buffer;

        /// Received as the renderer receives it. Tests tell voices apart by
        /// their length.
        fn voice(layout: ChannelLayout, samples: &[f32]) -> Voice {
            let (mut control, mut realtime) = transfer::channel(1);
            let audio = Arc::new(buffer(layout, samples));
            let state = Arc::new(VoiceState::default());
            let _transfer = control.try_start(VoiceData { audio, state }).unwrap();
            Voice::new(realtime.try_recv().unwrap())
        }

        fn frames(voice: &Voice) -> usize {
            voice.data.audio.frames()
        }

        /// `places` voices, and the control end that starts theirs.
        fn voices(places: usize) -> (transfer::ControlEnd<VoiceData>, Voices) {
            let (control, realtime) = transfer::channel(places + 4);
            let voices = Voices::new(NonZeroUsize::new(places).unwrap(), realtime);
            (control, voices)
        }

        /// A mono voice of `frames` of silence, started as `Control::play` does.
        fn start(
            control: &mut transfer::ControlEnd<VoiceData>,
            frames: usize,
        ) -> (Transfer<VoiceData>, Arc<VoiceState>) {
            let audio = Arc::new(buffer(ChannelLayout::Mono, &vec![0.0; frames]));
            let state = Arc::new(VoiceState::default());
            let data = VoiceData {
                audio,
                state: Arc::clone(&state),
            };
            (control.try_start(data).unwrap(), state)
        }

        fn playing(voices: &mut Voices) -> Vec<usize> {
            voices.iter_mut().map(|voice| frames(voice)).collect()
        }

        fn ended(transfer: &mut Transfer<VoiceData>) -> bool {
            transfer.try_wait().is_ok()
        }

        #[test]
        fn mono_is_heard_on_both_sides() {
            let mut voice = voice(ChannelLayout::Mono, &[0.1, 0.2]);
            let mut scratch = [[0.0; 2]; 2];
            voice.render(&mut scratch);
            assert_eq!(scratch, [[0.1, 0.1], [0.2, 0.2]]);
        }

        #[test]
        fn stereo_is_heard_as_stored() {
            let mut voice = voice(ChannelLayout::Stereo, &[0.1, 0.2, 0.3, 0.4]);
            let mut scratch = [[0.0; 2]; 2];
            voice.render(&mut scratch);
            assert_eq!(scratch, [[0.1, 0.2], [0.3, 0.4]]);
        }

        #[test]
        fn silence_follows_the_last_frame() {
            let mut voice = voice(ChannelLayout::Mono, &[0.5]);
            let mut scratch = [[9.0; 2]; 3];
            voice.render(&mut scratch);
            assert_eq!(scratch, [[0.5, 0.5], [0.0, 0.0], [0.0, 0.0]]);
        }

        #[test]
        fn each_render_continues_where_the_last_stopped() {
            let mut voice = voice(ChannelLayout::Mono, &[1.0, 2.0, 3.0, 4.0]);
            let mut scratch = [[0.0; 2]; 2];
            voice.render(&mut scratch);
            voice.render(&mut scratch);
            assert_eq!(scratch, [[3.0, 3.0], [4.0, 4.0]]);
        }

        #[test]
        fn a_voice_is_finished_once_its_last_frame_renders() {
            let mut voice = voice(ChannelLayout::Mono, &[0.0; 4]);
            let mut scratch = [[0.0; 2]; 2];
            voice.render(&mut scratch);
            assert!(!voice.finished());
            voice.render(&mut scratch);
            assert!(voice.finished());
        }

        #[test]
        fn received_voices_take_free_places_oldest_first() {
            let (mut control, mut voices) = voices(2);
            let (mut first, _) = start(&mut control, 1);
            let (mut second, _) = start(&mut control, 2);
            voices.receive();
            assert_eq!(playing(&mut voices), [1, 2]);
            assert!(!ended(&mut first));
            assert!(!ended(&mut second));
        }

        #[test]
        fn a_voice_received_when_full_cuts_the_oldest() {
            let (mut control, mut voices) = voices(2);
            let (mut oldest, _) = start(&mut control, 1);
            let _middle = start(&mut control, 2);
            let _newest = start(&mut control, 3);
            voices.receive();
            assert_eq!(playing(&mut voices), [2, 3]);
            assert!(ended(&mut oldest));
        }

        #[test]
        fn a_stopped_voice_ends_on_the_next_receive() {
            let (mut control, mut voices) = voices(1);
            let (mut voice, state) = start(&mut control, 4);
            voices.receive();
            state.stop.store(true, Ordering::Relaxed);
            voices.receive();
            assert!(playing(&mut voices).is_empty());
            assert!(ended(&mut voice));
        }

        #[test]
        fn end_finished_ends_only_the_voices_run_out() {
            let (mut control, mut voices) = voices(2);
            let (mut short, _) = start(&mut control, 1);
            let (mut long, _) = start(&mut control, 4);
            voices.receive();
            let mut scratch = [[0.0; 2]; 2];
            for voice in voices.iter_mut() {
                voice.render(&mut scratch);
            }
            voices.end_finished();
            assert_eq!(playing(&mut voices), [4]);
            assert!(ended(&mut short));
            assert!(!ended(&mut long));
        }
    }
}
