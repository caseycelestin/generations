//! The audio thread: turns commands and audio into periods of output.

mod math;
mod recording;
mod voices;

use std::num::NonZeroUsize;

use std::sync::Arc;

use crate::audio::AudioBuffer;
use crate::collector::Disposer;
use crate::config::Pcm;
use crate::duplex::{Message, RealtimeEnd};
use crate::engine::protocol::recording::{RecordingCommand, RecordingEnd};
use crate::engine::protocol::voice::{VoiceCommand, VoiceEnd};
use crate::engine::protocol::{Command, Event};
use math::{add, hard_clip};
use recording::Recording;
use voices::{Voice, Voices};

type Frame = [Pcm; Renderer::CHANNELS];

#[derive(Debug)]
pub struct Renderer {
    channel: RealtimeEnd<Command, Event>,
    disposer: Disposer<Arc<AudioBuffer>>,
    voices: Voices,
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
        channel: RealtimeEnd<Command, Event>,
        disposer: Disposer<Arc<AudioBuffer>>,
        voices: NonZeroUsize,
    ) -> Self {
        Self {
            channel,
            disposer,
            voices: Voices::new(voices),
            recording: None,
            scratch: vec![[0.0; Self::CHANNELS]; Self::BLOCK_FRAMES].into_boxed_slice(),
        }
    }

    /// One period, interleaved stereo, replacing what `out` held: every
    /// playing voice, summed and clipped.
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

    fn apply_commands(&mut self) {
        while let Some(Message { id, data: command }) = self.channel.try_recv() {
            match command {
                Command::Voice(VoiceCommand::Add(audio)) => {
                    if let Some(replaced) = self.voices.add(Voice::new(id, audio)) {
                        replaced.end(VoiceEnd::Cut, &mut self.channel, &mut self.disposer);
                    }
                }
                Command::Voice(VoiceCommand::Remove) => {
                    if let Some(stopped) = self.voices.remove(id) {
                        stopped.end(VoiceEnd::Stopped, &mut self.channel, &mut self.disposer);
                    }
                }
                Command::Voice(VoiceCommand::RemoveAudio(audio)) => {
                    for voice in self.voices.extract_if(|voice| voice.plays(audio)) {
                        voice.end(VoiceEnd::Stopped, &mut self.channel, &mut self.disposer);
                    }
                }
                Command::Voice(VoiceCommand::RemoveAll) => {
                    for voice in self.voices.drain() {
                        voice.end(VoiceEnd::Stopped, &mut self.channel, &mut self.disposer);
                    }
                }
                Command::Recording(RecordingCommand::Start(audio)) => {
                    if self.recording.is_some() {
                        Recording::refuse(id, audio, &mut self.channel);
                    } else {
                        self.recording = Some(Recording::start(id, audio, &mut self.channel));
                    }
                }
                Command::Recording(RecordingCommand::Stop) => {
                    if let Some(stopped) = self.recording.take_if(|recording| recording.is(id)) {
                        stopped.end(RecordingEnd::Stopped, &mut self.channel);
                    }
                }
            }
        }
    }

    fn clean_up(&mut self) {
        for voice in self.voices.extract_if(|voice| voice.finished()) {
            voice.end(VoiceEnd::Finished, &mut self.channel, &mut self.disposer);
        }
        if let Some(full) = self.recording.take_if(|recording| recording.finished()) {
            full.end(RecordingEnd::Full, &mut self.channel);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::audio::{AudioBuffer, AudioMemory};
    use crate::duplex::{Capacity, ControlEnd, Handle};
    use crate::engine::protocol::BufferAddress;
    use crate::engine::protocol::recording::RecordingEvent;
    use crate::layout::ChannelLayout;
    use crate::testing::buffer;

    type Channel = ControlEnd<Command, Event>;
    type Opened = Handle<Command, Event>;

    /// A renderer with `voices` places, and the control end of its channel.
    fn renderer(voices: usize) -> (Channel, Renderer) {
        let (control, realtime) = crate::duplex::channel(Capacity {
            commands: 16,
            events: 32,
        });
        let (_collector, disposer) = crate::collector::new(voices + 16);
        let voices = NonZeroUsize::new(voices).unwrap();
        (control, Renderer::new(realtime, disposer, voices))
    }

    /// Opens a mono voice of `samples`, as `Control::play` does.
    fn play(control: &Channel, samples: &[Pcm]) -> Opened {
        play_audio(control, Arc::new(buffer(ChannelLayout::Mono, samples)))
    }

    /// Opens a voice of `audio`, for voices that share it.
    fn play_audio(control: &Channel, audio: Arc<AudioBuffer>) -> Opened {
        control
            .try_reserve()
            .unwrap()
            .open(VoiceCommand::Add(audio))
    }

    /// How the voice ended; `None` while it plays.
    fn voice_end(voice: &mut Opened) -> Option<VoiceEnd> {
        let event = voice.try_recv()?;
        let Event::Voice(end) = event else {
            panic!("a voice heard a recording's event");
        };
        Some(end)
    }

    /// Opens a recording into empty stereo audio with room for `frames`, as
    /// `Control::record` does.
    fn start(control: &Channel, frames: usize) -> Opened {
        let audio = AudioMemory::new()
            .alloc_buffer(ChannelLayout::Stereo, frames)
            .unwrap();
        control
            .try_reserve_many(2)
            .unwrap()
            .open(RecordingCommand::Start(Arc::new(audio)))
    }

    /// What the recording reported, in order.
    fn reported(recording: &mut Opened) -> Vec<RecordingEvent> {
        let mut reported = Vec::new();
        while let Some(event) = recording.try_recv() {
            let Event::Recording(event) = event else {
                panic!("a recording heard a voice's event");
            };
            reported.push(event);
        }
        reported
    }

    fn samples(audio: &AudioBuffer) -> Vec<Pcm> {
        audio.frames_in(0..audio.frames()).to_vec()
    }

    #[test]
    fn voices_are_summed() {
        let (control, mut renderer) = renderer(2);
        let _quiet = play(&control, &[0.25; 8]);
        let _loud = play(&control, &[0.5; 8]);
        let mut out = vec![0.0; 8 * 2];
        renderer.render(&mut out);
        assert_eq!(out, vec![0.75; 8 * 2]);
    }

    #[test]
    fn the_sum_is_clipped() {
        let (control, mut renderer) = renderer(2);
        let _first = play(&control, &[0.75; 4]);
        let _second = play(&control, &[0.75; 4]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        assert_eq!(out, vec![1.0; 4 * 2]);
    }

    #[test]
    fn a_period_longer_than_a_block_plays_on_without_a_gap() {
        let frames = Renderer::BLOCK_FRAMES * 2 + 10;
        let ramp: Vec<Pcm> = (0..frames).map(|i| i as Pcm / 1000.0).collect();
        let (control, mut renderer) = renderer(1);
        let _voice = play(&control, &ramp);
        let mut out = vec![0.0; frames * 2];
        renderer.render(&mut out);
        let heard: Vec<Pcm> = out.chunks(2).map(|frame| frame[0]).collect();
        assert_eq!(heard, ramp);
    }

    #[test]
    fn periods_shorter_than_a_block_play_on_without_a_gap() {
        let (control, mut renderer) = renderer(1);
        let _voice = play(&control, &[0.1, 0.2, 0.3, 0.4]);
        let mut out = vec![0.0; 2 * 2];
        renderer.render(&mut out);
        renderer.render(&mut out);
        assert_eq!(out, vec![0.3, 0.3, 0.4, 0.4]);
    }

    #[test]
    fn a_voice_that_plays_out_ends_finished() {
        let (control, mut renderer) = renderer(1);
        let mut voice = play(&control, &[0.5; 4]);
        renderer.render(&mut [0.0; 8 * 2]);
        assert_eq!(voice_end(&mut voice), Some(VoiceEnd::Finished));
    }

    #[test]
    fn a_removed_voice_ends_stopped_and_falls_silent() {
        let (control, mut renderer) = renderer(1);
        let mut voice = play(&control, &[0.5; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        voice.send(VoiceCommand::Remove).unwrap();
        renderer.render(&mut out);
        assert_eq!(out, vec![0.0; 4 * 2]);
        assert_eq!(voice_end(&mut voice), Some(VoiceEnd::Stopped));
    }

    #[test]
    fn remove_all_ends_every_voice_stopped_and_falls_silent() {
        let (control, mut renderer) = renderer(2);
        let mut first = play(&control, &[0.5; 64]);
        let mut second = play(&control, &[0.5; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        control.send(VoiceCommand::RemoveAll).unwrap();
        renderer.render(&mut out);
        assert_eq!(out, vec![0.0; 4 * 2]);
        let stopped = Some(VoiceEnd::Stopped);
        assert_eq!(voice_end(&mut first), stopped);
        assert_eq!(voice_end(&mut second), stopped);
    }

    #[test]
    fn remove_audio_ends_only_the_voices_playing_it() {
        let (control, mut renderer) = renderer(3);
        let removed = Arc::new(buffer(ChannelLayout::Mono, &[0.5; 64]));
        let mut first = play_audio(&control, Arc::clone(&removed));
        let mut second = play_audio(&control, Arc::clone(&removed));
        let mut kept = play(&control, &[0.25; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        let address = BufferAddress::of(&removed);
        control.send(VoiceCommand::RemoveAudio(address)).unwrap();
        renderer.render(&mut out);
        assert_eq!(out, vec![0.25; 4 * 2]);
        let stopped = Some(VoiceEnd::Stopped);
        assert_eq!(voice_end(&mut first), stopped);
        assert_eq!(voice_end(&mut second), stopped);
        assert_eq!(voice_end(&mut kept), None);
    }

    #[test]
    fn a_new_voice_in_a_full_pool_cuts_the_oldest() {
        let (control, mut renderer) = renderer(1);
        let mut oldest = play(&control, &[0.5; 64]);
        let mut newest = play(&control, &[0.5; 64]);
        renderer.render(&mut [0.0; 4 * 2]);
        assert_eq!(voice_end(&mut oldest), Some(VoiceEnd::Cut));
        assert_eq!(voice_end(&mut newest), None);
    }

    #[test]
    fn a_recording_writes_what_is_heard_until_stopped() {
        let (control, mut renderer) = renderer(1);
        let _voice = play(&control, &[0.25; 64]);
        let mut recording = start(&control, 64);
        renderer.render(&mut [0.0; 4 * 2]);
        assert!(matches!(
            reported(&mut recording)[..],
            [RecordingEvent::Started]
        ));

        recording.send(RecordingCommand::Stop).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        let [RecordingEvent::Ended(RecordingEnd::Stopped, audio)] = &reported(&mut recording)[..]
        else {
            panic!("the recording did not end stopped");
        };
        assert_eq!(samples(audio), vec![0.25; 4 * 2]);
    }

    #[test]
    fn a_recording_ends_full_when_its_audio_has_no_room() {
        let (control, mut renderer) = renderer(1);
        let mut recording = start(&control, 3);
        renderer.render(&mut [0.0; 8 * 2]);
        let [
            RecordingEvent::Started,
            RecordingEvent::Ended(RecordingEnd::Full, audio),
        ] = &reported(&mut recording)[..]
        else {
            panic!("the recording did not end full");
        };
        assert_eq!(samples(audio).len(), 3 * 2);
    }

    #[test]
    fn a_start_while_recording_is_refused_and_the_first_runs_on() {
        let (control, mut renderer) = renderer(1);
        let mut first = start(&control, 64);
        let mut second = start(&control, 64);
        renderer.render(&mut [0.0; 4 * 2]);
        assert!(matches!(
            reported(&mut first)[..],
            [RecordingEvent::Started]
        ));
        assert!(matches!(
            reported(&mut second)[..],
            [RecordingEvent::Refused(_)]
        ));
    }

    #[test]
    fn a_stop_from_another_sender_does_nothing() {
        let (control, mut renderer) = renderer(1);
        let mut recording = start(&control, 64);
        control.send(RecordingCommand::Stop).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);
        assert!(matches!(
            reported(&mut recording)[..],
            [RecordingEvent::Started]
        ));
    }
}
