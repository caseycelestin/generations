//! The audio thread: turns commands and audio into periods of output.

mod math;
mod recording;
mod voices;

use std::num::NonZeroUsize;

use std::sync::Arc;

use rtrb::{Consumer, Producer};

use crate::audio::AudioBuffer;
use crate::collector::Disposer;
use crate::config::Pcm;
use crate::duplex::Message;
use crate::engine::protocol::recording::{RecordingCommand, RecordingEnd};
use crate::engine::protocol::voice::{VoiceCommand, VoiceEnd};
use crate::engine::protocol::{Command, Event};
use math::{add, hard_clip};
use recording::Recording;
use voices::{Voice, Voices};

type Frame = [Pcm; Renderer::CHANNELS];

#[derive(Debug)]
pub struct Renderer {
    from_control: Consumer<Message<Command>>,
    to_control: Producer<Message<Event>>,
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
        from_control: Consumer<Message<Command>>,
        to_control: Producer<Message<Event>>,
        disposer: Disposer<Arc<AudioBuffer>>,
        voices: NonZeroUsize,
    ) -> Self {
        Self {
            from_control,
            to_control,
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
        while let Ok(Message { id, data: command }) = self.from_control.pop() {
            match command {
                Command::Voice(VoiceCommand::Add(spec)) => {
                    if let Some(replaced) = self.voices.add(Voice::new(id, spec)) {
                        replaced.end(VoiceEnd::Cut, &mut self.to_control, &mut self.disposer);
                    }
                }
                Command::Voice(VoiceCommand::Remove) => {
                    if let Some(stopped) = self.voices.remove(id) {
                        stopped.end(VoiceEnd::Stopped, &mut self.to_control, &mut self.disposer);
                    }
                }
                Command::Voice(VoiceCommand::RemoveAudio(audio)) => {
                    for voice in self.voices.extract_if(|voice| voice.plays(audio)) {
                        voice.end(VoiceEnd::Stopped, &mut self.to_control, &mut self.disposer);
                    }
                }
                Command::Voice(VoiceCommand::RemoveAll) => {
                    for voice in self.voices.drain() {
                        voice.end(VoiceEnd::Stopped, &mut self.to_control, &mut self.disposer);
                    }
                }
                Command::Recording(RecordingCommand::Start(spec)) => {
                    if self.recording.is_some() {
                        Recording::refuse(id, spec, &mut self.to_control);
                    } else {
                        self.recording = Some(Recording::start(id, spec, &mut self.to_control));
                    }
                }
                Command::Recording(RecordingCommand::Stop) => {
                    if let Some(stopped) = self.recording.take_if(|recording| recording.is(id)) {
                        stopped.end(RecordingEnd::Stopped, &mut self.to_control);
                    }
                }
            }
        }
    }

    fn clean_up(&mut self) {
        for voice in self.voices.extract_if(|voice| voice.finished()) {
            voice.end(VoiceEnd::Finished, &mut self.to_control, &mut self.disposer);
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
    use crate::audio::{AudioBuffer, AudioMemory};
    use crate::duplex::Id;
    use crate::engine::protocol::BufferAddress;
    use crate::engine::protocol::recording::{RecordingEvent, RecordingSpec, RecordingState};
    use crate::engine::protocol::voice::{VoiceSpec, VoiceState};
    use crate::layout::ChannelLayout;
    use crate::testing::buffer;

    /// A renderer with `voices` places, and the control side's ring ends.
    fn renderer(
        voices: usize,
    ) -> (
        Producer<Message<Command>>,
        Consumer<Message<Event>>,
        Renderer,
    ) {
        let (to_renderer, from_control) = RingBuffer::new(16);
        let (to_control, from_renderer) = RingBuffer::new(32);
        let (_collector, disposer) = crate::collector::new(voices + 16);
        let voices = NonZeroUsize::new(voices).unwrap();
        let renderer = Renderer::new(from_control, to_control, disposer, voices);
        (to_renderer, from_renderer, renderer)
    }

    /// Sends an add from `sender` for a mono voice of `samples`, as
    /// `Control::play` does.
    fn play(to_renderer: &mut Producer<Message<Command>>, sender: Id, samples: &[Pcm]) {
        let audio = Arc::new(buffer(ChannelLayout::Mono, samples));
        play_audio(to_renderer, sender, audio)
    }

    /// Sends an add from `sender` for a voice of `audio`, for voices that
    /// share it.
    fn play_audio(
        to_renderer: &mut Producer<Message<Command>>,
        sender: Id,
        audio: Arc<AudioBuffer>,
    ) {
        let spec = VoiceSpec { audio };
        to_renderer
            .push(Message {
                id: sender,
                data: VoiceCommand::Add(spec).into(),
            })
            .unwrap();
    }

    /// Which voices ended, and how, in order.
    fn ended(from_renderer: &mut Consumer<Message<Event>>) -> Vec<(Id, VoiceState)> {
        let mut ended = Vec::new();
        while let Ok(back) = from_renderer.pop() {
            if let Event::Voice(event) = back.data {
                ended.push((back.id, event.state));
            }
        }
        ended
    }

    /// Sends a start for a recording into empty stereo audio with room for
    /// `frames`, as `Control::record` does.
    fn start(to_renderer: &mut Producer<Message<Command>>, frames: usize) -> Id {
        let audio = AudioMemory::new()
            .alloc_buffer(ChannelLayout::Stereo, frames)
            .unwrap();
        let audio = Arc::new(audio);
        let id = Id(0);
        let spec = RecordingSpec { audio };
        to_renderer
            .push(Message {
                id,
                data: RecordingCommand::Start(spec).into(),
            })
            .unwrap();
        id
    }

    /// What came back for recordings, in order.
    fn returned(from_renderer: &mut Consumer<Message<Event>>) -> Vec<RecordingEvent> {
        let mut returned = Vec::new();
        while let Ok(back) = from_renderer.pop() {
            if let Event::Recording(event) = back.data {
                returned.push(event);
            }
        }
        returned
    }

    fn samples(spec: &RecordingSpec) -> Vec<Pcm> {
        spec.audio.frames_in(0..spec.audio.frames()).to_vec()
    }

    #[test]
    fn voices_are_summed() {
        let (mut to_renderer, _from_renderer, mut renderer) = renderer(2);
        play(&mut to_renderer, Id(0), &[0.25; 8]);
        play(&mut to_renderer, Id(0), &[0.5; 8]);
        let mut out = vec![0.0; 8 * 2];
        renderer.render(&mut out);
        assert_eq!(out, vec![0.75; 8 * 2]);
    }

    #[test]
    fn the_sum_is_clipped() {
        let (mut to_renderer, _from_renderer, mut renderer) = renderer(2);
        play(&mut to_renderer, Id(0), &[0.75; 4]);
        play(&mut to_renderer, Id(0), &[0.75; 4]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        assert_eq!(out, vec![1.0; 4 * 2]);
    }

    #[test]
    fn a_period_longer_than_a_block_plays_on_without_a_gap() {
        let frames = Renderer::BLOCK_FRAMES * 2 + 10;
        let ramp: Vec<Pcm> = (0..frames).map(|i| i as Pcm / 1000.0).collect();
        let (mut to_renderer, _from_renderer, mut renderer) = renderer(1);
        play(&mut to_renderer, Id(0), &ramp);
        let mut out = vec![0.0; frames * 2];
        renderer.render(&mut out);
        let heard: Vec<Pcm> = out.chunks(2).map(|frame| frame[0]).collect();
        assert_eq!(heard, ramp);
    }

    #[test]
    fn periods_shorter_than_a_block_play_on_without_a_gap() {
        let (mut to_renderer, _from_renderer, mut renderer) = renderer(1);
        play(&mut to_renderer, Id(0), &[0.1, 0.2, 0.3, 0.4]);
        let mut out = vec![0.0; 2 * 2];
        renderer.render(&mut out);
        renderer.render(&mut out);
        assert_eq!(out, vec![0.3, 0.3, 0.4, 0.4]);
    }

    #[test]
    fn a_voice_that_plays_out_ends_finished_and_is_handed_back() {
        let (mut to_renderer, mut from_renderer, mut renderer) = renderer(1);
        play(&mut to_renderer, Id(0), &[0.5; 4]);
        let mut out = vec![0.0; 8 * 2];
        renderer.render(&mut out);
        assert_eq!(
            ended(&mut from_renderer),
            [(Id(0), VoiceState::Ended(VoiceEnd::Finished))]
        );
    }

    #[test]
    fn a_removed_voice_ends_stopped_and_falls_silent() {
        let (mut to_renderer, mut from_renderer, mut renderer) = renderer(1);
        play(&mut to_renderer, Id(0), &[0.5; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        to_renderer
            .push(Message {
                id: Id(0),
                data: VoiceCommand::Remove.into(),
            })
            .unwrap();
        renderer.render(&mut out);
        assert_eq!(out, vec![0.0; 4 * 2]);
        assert_eq!(
            ended(&mut from_renderer),
            [(Id(0), VoiceState::Ended(VoiceEnd::Stopped))]
        );
    }

    #[test]
    fn remove_all_ends_every_voice_stopped_and_falls_silent() {
        let (mut to_renderer, mut from_renderer, mut renderer) = renderer(2);
        play(&mut to_renderer, Id(0), &[0.5; 64]);
        play(&mut to_renderer, Id(1), &[0.5; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        to_renderer
            .push(Message {
                id: Id(0),
                data: VoiceCommand::RemoveAll.into(),
            })
            .unwrap();
        renderer.render(&mut out);
        assert_eq!(out, vec![0.0; 4 * 2]);
        assert_eq!(
            ended(&mut from_renderer),
            [
                (Id(0), VoiceState::Ended(VoiceEnd::Stopped)),
                (Id(1), VoiceState::Ended(VoiceEnd::Stopped)),
            ]
        );
    }

    #[test]
    fn remove_audio_ends_only_the_voices_playing_it() {
        let (mut to_renderer, mut from_renderer, mut renderer) = renderer(3);
        let removed = Arc::new(buffer(ChannelLayout::Mono, &[0.5; 64]));
        play_audio(&mut to_renderer, Id(0), Arc::clone(&removed));
        play_audio(&mut to_renderer, Id(1), Arc::clone(&removed));
        play(&mut to_renderer, Id(2), &[0.25; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        let address = BufferAddress::of(&removed);
        to_renderer
            .push(Message {
                id: Id(0),
                data: VoiceCommand::RemoveAudio(address).into(),
            })
            .unwrap();
        renderer.render(&mut out);
        assert_eq!(out, vec![0.25; 4 * 2]);
        assert_eq!(
            ended(&mut from_renderer),
            [
                (Id(0), VoiceState::Ended(VoiceEnd::Stopped)),
                (Id(1), VoiceState::Ended(VoiceEnd::Stopped)),
            ]
        );
    }

    #[test]
    fn a_new_voice_in_a_full_pool_cuts_the_oldest() {
        let (mut to_renderer, mut from_renderer, mut renderer) = renderer(1);
        play(&mut to_renderer, Id(0), &[0.5; 64]);
        play(&mut to_renderer, Id(1), &[0.5; 64]);
        renderer.render(&mut [0.0; 4 * 2]);
        assert_eq!(
            ended(&mut from_renderer),
            [(Id(0), VoiceState::Ended(VoiceEnd::Cut))]
        );
    }

    #[test]
    fn a_recording_writes_what_is_heard_until_stopped() {
        let (mut to_renderer, mut from_renderer, mut renderer) = renderer(1);
        play(&mut to_renderer, Id(0), &[0.25; 64]);
        let id = start(&mut to_renderer, 64);
        renderer.render(&mut [0.0; 4 * 2]);
        assert!(matches!(
            returned(&mut from_renderer)[..],
            [RecordingEvent {
                state: RecordingState::Recording,
                spec: None
            }]
        ));

        to_renderer
            .push(Message {
                id,
                data: RecordingCommand::Stop.into(),
            })
            .unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        let [
            RecordingEvent {
                state: RecordingState::Ended(RecordingEnd::Stopped),
                spec: Some(spec),
            },
        ] = &returned(&mut from_renderer)[..]
        else {
            panic!("the recording did not end stopped");
        };
        assert_eq!(samples(spec), vec![0.25; 4 * 2]);
    }

    #[test]
    fn a_recording_ends_full_when_its_audio_has_no_room() {
        let (mut to_renderer, mut from_renderer, mut renderer) = renderer(1);
        start(&mut to_renderer, 3);
        renderer.render(&mut [0.0; 8 * 2]);
        let [
            RecordingEvent {
                state: RecordingState::Recording,
                ..
            },
            RecordingEvent {
                state: RecordingState::Ended(RecordingEnd::Full),
                spec: Some(spec),
            },
        ] = &returned(&mut from_renderer)[..]
        else {
            panic!("the recording did not end full");
        };
        assert_eq!(samples(spec).len(), 3 * 2);
    }

    #[test]
    fn a_start_while_recording_is_refused_and_the_first_runs_on() {
        let (mut to_renderer, mut from_renderer, mut renderer) = renderer(1);
        start(&mut to_renderer, 64);
        start(&mut to_renderer, 64);
        renderer.render(&mut [0.0; 4 * 2]);
        assert!(matches!(
            returned(&mut from_renderer)[..],
            [
                RecordingEvent {
                    state: RecordingState::Recording,
                    ..
                },
                RecordingEvent {
                    state: RecordingState::Refused,
                    spec: Some(_),
                },
            ]
        ));
    }

    #[test]
    fn a_stop_naming_another_recording_does_nothing() {
        let (mut to_renderer, mut from_renderer, mut renderer) = renderer(1);
        start(&mut to_renderer, 64);
        to_renderer
            .push(Message {
                id: Id(1),
                data: RecordingCommand::Stop.into(),
            })
            .unwrap();
        renderer.render(&mut [0.0; 4 * 2]);
        assert!(matches!(
            returned(&mut from_renderer)[..],
            [RecordingEvent {
                state: RecordingState::Recording,
                ..
            }]
        ));
    }
}
