//! The audio thread: turns commands and audio into periods of output.

mod math;
mod recording;
mod voices;

use std::num::NonZeroUsize;

use rtrb::{Consumer, Producer};

use crate::config::Pcm;
use crate::engine::protocol::recording::{RecordingCommand, RecordingEnd};
use crate::engine::protocol::voice::{VoiceCommand, VoiceEnd};
use crate::engine::protocol::{Command, Return};
use math::{add, hard_clip};
use recording::Recording;
use voices::{Voice, Voices};

/// One output frame: a sample per side.
type Frame = [Pcm; Renderer::CHANNELS];

#[derive(Debug)]
pub struct Renderer {
    from_control: Consumer<Command>,
    to_control: Producer<Return>,
    voices: Voices,
    /// The one recording, if running.
    recording: Option<Recording>,
    /// One sound's frames for the block, before they join the mix.
    scratch: Box<[Frame]>,
}

impl Renderer {
    /// Always stereo.
    pub const CHANNELS: usize = 2;

    /// A period of any length is mixed in blocks this long, so scratch
    /// never grows.
    const BLOCK_FRAMES: usize = 64;

    /// Allocates everything the renderer holds for its life, here, on the
    /// control side.
    pub(crate) fn new(
        from_control: Consumer<Command>,
        to_control: Producer<Return>,
        voices: NonZeroUsize,
    ) -> Self {
        Self {
            from_control,
            to_control,
            voices: Voices::new(voices),
            recording: None,
            scratch: vec![[0.0; Self::CHANNELS]; Self::BLOCK_FRAMES].into_boxed_slice(),
        }
    }

    /// Renders one period into `out`, interleaved stereo, replacing what
    /// it held: every playing voice, summed and clipped.
    pub fn render(&mut self, out: &mut [Pcm]) {
        debug_assert_eq!(out.len() % Self::CHANNELS, 0, "output is not whole frames");
        self.apply_commands();
        let (frames, _) = out.as_chunks_mut::<{ Self::CHANNELS }>();
        for block in frames.chunks_mut(Self::BLOCK_FRAMES) {
            block.fill([0.0; Self::CHANNELS]);
            let scratch = &mut self.scratch[..block.len()];
            for voice in self.voices.iter_mut() {
                voice.render(scratch);
                add(block, scratch);
            }
            hard_clip(block);
            if let Some(recording) = &mut self.recording {
                recording.write(block);
            }
        }
        self.clean_up();
    }

    /// Applies every command waiting, oldest first.
    fn apply_commands(&mut self) {
        while let Ok(command) = self.from_control.pop() {
            match command {
                Command::Voice(VoiceCommand::Add(spec)) => {
                    if let Some(replaced) = self.voices.add(Voice::new(spec)) {
                        replaced.end(VoiceEnd::Cut, &mut self.to_control);
                    }
                }
                Command::Voice(VoiceCommand::Remove(id)) => {
                    if let Some(stopped) = self.voices.remove(id) {
                        stopped.end(VoiceEnd::Stopped, &mut self.to_control);
                    }
                }
                Command::Voice(VoiceCommand::RemoveAudio(audio)) => {
                    for voice in self.voices.extract_if(|voice| voice.plays(audio)) {
                        voice.end(VoiceEnd::Stopped, &mut self.to_control);
                    }
                }
                Command::Voice(VoiceCommand::RemoveAll) => {
                    for voice in self.voices.drain() {
                        voice.end(VoiceEnd::Stopped, &mut self.to_control);
                    }
                }
                Command::Recording(RecordingCommand::Start(spec)) => {
                    if self.recording.is_some() {
                        Recording::refuse(spec, &mut self.to_control);
                    } else {
                        self.recording = Some(Recording::start(spec));
                    }
                }
                Command::Recording(RecordingCommand::Stop) => {
                    if let Some(stopped) = self.recording.take() {
                        stopped.end(RecordingEnd::Stopped, &mut self.to_control);
                    }
                }
            }
        }
    }

    /// End-of-period housekeeping: ends every voice that played to its last
    /// frame, and the recording once its audio is full.
    fn clean_up(&mut self) {
        for voice in self.voices.extract_if(|voice| voice.finished()) {
            voice.end(VoiceEnd::Finished, &mut self.to_control);
        }
        if let Some(full) = self.recording.take_if(|recording| recording.finished()) {
            full.end(RecordingEnd::Full, &mut self.to_control);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rtrb::RingBuffer;

    use super::*;
    use crate::audio::AudioBuffer;
    use crate::engine::protocol::voice::{BufferAddress, VoiceId, VoiceReport, VoiceSpec};
    use crate::layout::ChannelLayout;
    use crate::testing::buffer;

    /// A renderer with `voices` places, and the control side's ring ends.
    fn renderer(voices: usize) -> (Producer<Command>, Consumer<Return>, Renderer) {
        let (to_renderer, from_control) = RingBuffer::new(16);
        let (to_control, from_renderer) = RingBuffer::new(32);
        let voices = NonZeroUsize::new(voices).unwrap();
        let renderer = Renderer::new(from_control, to_control, voices);
        (to_renderer, from_renderer, renderer)
    }

    /// Sends an add for a mono voice of `samples`, as `Control::play` does.
    fn play(to_renderer: &mut Producer<Command>, samples: &[Pcm]) -> (VoiceId, Arc<VoiceReport>) {
        play_audio(to_renderer, Arc::new(buffer(ChannelLayout::Mono, samples)))
    }

    /// Sends an add for a voice of `audio`, for voices that share it.
    fn play_audio(
        to_renderer: &mut Producer<Command>,
        audio: Arc<AudioBuffer>,
    ) -> (VoiceId, Arc<VoiceReport>) {
        let id = VoiceId::next();
        let report = Arc::new(VoiceReport::new());
        let spec = VoiceSpec {
            id,
            audio,
            report: Arc::clone(&report),
        };
        to_renderer.push(VoiceCommand::Add(spec).into()).unwrap();
        (id, report)
    }

    #[test]
    fn voices_are_summed() {
        let (mut to_renderer, _from_renderer, mut renderer) = renderer(2);
        play(&mut to_renderer, &[0.25; 8]);
        play(&mut to_renderer, &[0.5; 8]);
        let mut out = vec![0.0; 8 * 2];
        renderer.render(&mut out);
        assert_eq!(out, vec![0.75; 8 * 2]);
    }

    #[test]
    fn the_sum_is_clipped() {
        let (mut to_renderer, _from_renderer, mut renderer) = renderer(2);
        play(&mut to_renderer, &[0.75; 4]);
        play(&mut to_renderer, &[0.75; 4]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        assert_eq!(out, vec![1.0; 4 * 2]);
    }

    #[test]
    fn a_period_longer_than_a_block_plays_on_without_a_gap() {
        let frames = Renderer::BLOCK_FRAMES * 2 + 10;
        let ramp: Vec<Pcm> = (0..frames).map(|i| i as Pcm / 1000.0).collect();
        let (mut to_renderer, _from_renderer, mut renderer) = renderer(1);
        play(&mut to_renderer, &ramp);
        let mut out = vec![0.0; frames * 2];
        renderer.render(&mut out);
        let heard: Vec<Pcm> = out.chunks(2).map(|frame| frame[0]).collect();
        assert_eq!(heard, ramp);
    }

    #[test]
    fn periods_shorter_than_a_block_play_on_without_a_gap() {
        let (mut to_renderer, _from_renderer, mut renderer) = renderer(1);
        play(&mut to_renderer, &[0.1, 0.2, 0.3, 0.4]);
        let mut out = vec![0.0; 2 * 2];
        renderer.render(&mut out);
        renderer.render(&mut out);
        assert_eq!(out, vec![0.3, 0.3, 0.4, 0.4]);
    }

    #[test]
    fn a_voice_that_plays_out_ends_finished_and_is_handed_back() {
        let (mut to_renderer, mut from_renderer, mut renderer) = renderer(1);
        let (_, report) = play(&mut to_renderer, &[0.5; 4]);
        let mut out = vec![0.0; 8 * 2];
        renderer.render(&mut out);
        assert_eq!(report.ended(), Some(VoiceEnd::Finished));
        assert!(matches!(from_renderer.pop(), Ok(Return::Voice(_))));
    }

    #[test]
    fn a_removed_voice_ends_stopped_and_falls_silent() {
        let (mut to_renderer, mut from_renderer, mut renderer) = renderer(1);
        let (id, report) = play(&mut to_renderer, &[0.5; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        to_renderer.push(VoiceCommand::Remove(id).into()).unwrap();
        renderer.render(&mut out);
        assert_eq!(report.ended(), Some(VoiceEnd::Stopped));
        assert_eq!(out, vec![0.0; 4 * 2]);
        assert!(matches!(from_renderer.pop(), Ok(Return::Voice(_))));
    }

    #[test]
    fn remove_all_ends_every_voice_stopped_and_falls_silent() {
        let (mut to_renderer, from_renderer, mut renderer) = renderer(2);
        let (_, first) = play(&mut to_renderer, &[0.5; 64]);
        let (_, second) = play(&mut to_renderer, &[0.5; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        to_renderer.push(VoiceCommand::RemoveAll.into()).unwrap();
        renderer.render(&mut out);
        assert_eq!(first.ended(), Some(VoiceEnd::Stopped));
        assert_eq!(second.ended(), Some(VoiceEnd::Stopped));
        assert_eq!(out, vec![0.0; 4 * 2]);
        assert_eq!(from_renderer.slots(), 2);
    }

    #[test]
    fn remove_audio_ends_only_the_voices_playing_it() {
        let (mut to_renderer, from_renderer, mut renderer) = renderer(3);
        let removed = Arc::new(buffer(ChannelLayout::Mono, &[0.5; 64]));
        let (_, first) = play_audio(&mut to_renderer, Arc::clone(&removed));
        let (_, second) = play_audio(&mut to_renderer, Arc::clone(&removed));
        let (_, kept) = play(&mut to_renderer, &[0.25; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        let address = BufferAddress::of(&removed);
        to_renderer
            .push(VoiceCommand::RemoveAudio(address).into())
            .unwrap();
        renderer.render(&mut out);
        assert_eq!(first.ended(), Some(VoiceEnd::Stopped));
        assert_eq!(second.ended(), Some(VoiceEnd::Stopped));
        assert_eq!(kept.ended(), None);
        assert_eq!(out, vec![0.25; 4 * 2]);
        assert_eq!(from_renderer.slots(), 2);
    }

    #[test]
    fn a_new_voice_in_a_full_pool_cuts_the_oldest() {
        let (mut to_renderer, mut from_renderer, mut renderer) = renderer(1);
        let (_, oldest) = play(&mut to_renderer, &[0.5; 64]);
        let (_, newest) = play(&mut to_renderer, &[0.5; 64]);
        renderer.render(&mut [0.0; 4 * 2]);
        assert_eq!(oldest.ended(), Some(VoiceEnd::Cut));
        assert_eq!(newest.ended(), None);
        assert!(matches!(from_renderer.pop(), Ok(Return::Voice(_))));
    }
}
