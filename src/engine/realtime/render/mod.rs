//! The audio thread: turns requests and audio into periods of output.

mod input;
mod math;
mod recording;
mod voices;

use std::num::NonZeroUsize;

use crate::config::Pcm;
use crate::engine::protocol::inputs::{InputEnd, InputRequest, InputStatus};
use crate::engine::protocol::recording::{RecordingEnd, RecordingRequest, RecordingStatus};
use crate::engine::protocol::voice::{VoiceRequest, VoiceStatus};
use crate::request::{self, Received};
use input::Input;
use math::{add, hard_clip};
use recording::Recording;
use voices::{Voice, Voices};

type Frame = [Pcm; Renderer::CHANNELS];

/// One request channel per actor, at its real-time end.
#[derive(Debug)]
pub(crate) struct Requests {
    pub(crate) voices: request::RealtimeEnd<VoiceRequest>,
    pub(crate) recording: request::RealtimeEnd<RecordingRequest>,
    pub(crate) inputs: request::RealtimeEnd<InputRequest>,
}

#[derive(Debug)]
pub struct Renderer {
    requests: Requests,
    voices: Voices,
    recording: Option<Recording>,
    /// Never past its capacity, so adding one never allocates.
    inputs: Vec<Input>,
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
    pub(crate) fn new(requests: Requests, voices: NonZeroUsize, inputs: usize) -> Self {
        Self {
            requests,
            voices: Voices::new(voices),
            recording: None,
            inputs: Vec::with_capacity(inputs),
            scratch: vec![[0.0; Self::CHANNELS]; Self::BLOCK_FRAMES].into_boxed_slice(),
        }
    }

    /// One period, interleaved stereo, replacing what `out` held: every
    /// playing voice, summed and clipped.
    pub fn render(&mut self, out: &mut [Pcm]) {
        debug_assert_eq!(out.len() % Self::CHANNELS, 0, "output is not whole frames");
        self.apply_requests();
        let (frames, _) = out.as_chunks_mut::<{ Self::CHANNELS }>();
        for block in frames.chunks_mut(Self::BLOCK_FRAMES) {
            block.fill([0.0; Self::CHANNELS]);
            let scratch = &mut self.scratch[..block.len()];
            for voice in self.voices.iter_mut() {
                voice.render(scratch);
                add(block, scratch);
            }
            // Read even when nothing hears it, so the ring keeps moving.
            for input in &mut self.inputs {
                input.read(scratch);
            }
            hard_clip(block);
            if let Some(recording) = &mut self.recording {
                recording.write(block);
            }
        }
        self.clean_up();
    }

    fn apply_requests(&mut self) {
        self.apply_voice_requests();
        self.apply_recording_requests();
        self.apply_input_requests();
    }

    fn apply_voice_requests(&mut self) {
        while let Some(received) = self.requests.voices.try_recv() {
            match received {
                Received::Request(permit, VoiceRequest::Play(audio)) => {
                    if let Some(cut) = self.voices.add(Voice::new(permit, audio)) {
                        cut.end(&mut self.requests.voices);
                    }
                }
                Received::Request(permit, VoiceRequest::StopAudio(audio)) => {
                    for voice in self.voices.extract_if(|voice| voice.plays(&audio)) {
                        voice.end(&mut self.requests.voices);
                    }
                    // The audio goes back with the reply, never freed here.
                    self.requests.voices.close(permit, Some(audio));
                }
                Received::Request(permit, VoiceRequest::StopAll) => {
                    for voice in self.voices.drain() {
                        voice.end(&mut self.requests.voices);
                    }
                    self.requests.voices.close(permit, None);
                }
                Received::Cancel(id) => {
                    if let Some(stopped) = self.voices.remove(id) {
                        stopped.end(&mut self.requests.voices);
                    }
                }
                Received::Status(permit) => {
                    if self.voices.holds(permit.id()) {
                        self.requests.voices.answer(permit, VoiceStatus::Playing);
                    }
                    // Otherwise it is closed, and its reply has the last word.
                }
            }
        }
    }

    fn apply_recording_requests(&mut self) {
        while let Some(received) = self.requests.recording.try_recv() {
            match received {
                Received::Request(permit, RecordingRequest::Start(audio)) => {
                    debug_assert!(self.recording.is_none(), "one request per recorder");
                    self.recording = Some(Recording::start(permit, audio));
                }
                Received::Cancel(id) => {
                    if let Some(stopped) = self.recording.take_if(|recording| recording.is(id)) {
                        stopped.end(RecordingEnd::Stopped, &mut self.requests.recording);
                    }
                }
                Received::Status(permit) => {
                    let held = self.recording.as_ref();
                    if held.is_some_and(|recording| recording.is(permit.id())) {
                        self.requests
                            .recording
                            .answer(permit, RecordingStatus::Recording);
                    }
                    // Otherwise it is closed, and its reply has the last word.
                }
            }
        }
    }

    fn apply_input_requests(&mut self) {
        while let Some(received) = self.requests.inputs.try_recv() {
            match received {
                Received::Request(permit, request) => {
                    let room = self.inputs.len() < self.inputs.capacity();
                    debug_assert!(room, "one request per input");
                    self.inputs
                        .push(Input::open(permit, request.reader, request.layout));
                }
                Received::Cancel(id) => {
                    if let Some(at) = self.inputs.iter().position(|input| input.is(id)) {
                        let stopped = self.inputs.swap_remove(at);
                        stopped.end(InputEnd::Stopped, &mut self.requests.inputs);
                    }
                }
                Received::Status(permit) => {
                    if self.inputs.iter().any(|input| input.is(permit.id())) {
                        self.requests.inputs.answer(permit, InputStatus::Open);
                    }
                    // Otherwise it is closed, and its reply has the last word.
                }
            }
        }
    }

    fn clean_up(&mut self) {
        for voice in self.voices.extract_if(|voice| voice.finished()) {
            voice.end(&mut self.requests.voices);
        }
        if let Some(full) = self.recording.take_if(|recording| recording.finished()) {
            full.end(RecordingEnd::Full, &mut self.requests.recording);
        }
        for gone in self.inputs.extract_if(.., |input| input.feeder_gone()) {
            gone.end(InputEnd::FeederGone, &mut self.requests.inputs);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::audio::{AudioBuffer, AudioMemory};
    use crate::layout::ChannelLayout;
    use crate::request::{ControlEnd, RequestHandle, TryRecvStatusError, TryWaitError};
    use crate::testing::buffer;

    /// The control ends of the renderer's channels.
    struct Ends {
        voices: ControlEnd<VoiceRequest>,
        recording: ControlEnd<RecordingRequest>,
    }

    /// A renderer with `voices` places, and the control ends of its channels.
    fn renderer(voices: usize) -> (Ends, Renderer) {
        let (voices_control, voices_realtime) = request::channel_with_headroom(voices + 16, 2);
        let (recording_control, recording_realtime) = request::channel(1);
        let (_, inputs_realtime) = request::channel(0);
        let ends = Ends {
            voices: voices_control,
            recording: recording_control,
        };
        let requests = Requests {
            voices: voices_realtime,
            recording: recording_realtime,
            inputs: inputs_realtime,
        };
        let voices = NonZeroUsize::new(voices).unwrap();
        (ends, Renderer::new(requests, voices, 0))
    }

    /// Opens a mono voice of `samples`, as `Control::play` does.
    fn play(ends: &Ends, samples: &[Pcm]) -> RequestHandle<VoiceRequest> {
        play_audio(ends, Arc::new(buffer(ChannelLayout::Mono, samples)))
    }

    /// Opens a voice of `audio`, for voices that share it.
    fn play_audio(ends: &Ends, audio: Arc<AudioBuffer>) -> RequestHandle<VoiceRequest> {
        ends.voices
            .try_reserve()
            .unwrap()
            .open(VoiceRequest::Play(audio))
    }

    /// Opens a stop, as `Control::stop` and `stop_all` do.
    fn stop(ends: &Ends, request: VoiceRequest) {
        ends.voices
            .try_reserve_including_headroom()
            .unwrap()
            .open(request);
    }

    fn ended(voice: &mut RequestHandle<VoiceRequest>) -> bool {
        voice.try_wait().is_ok()
    }

    /// Opens a recording into empty stereo audio with room for `frames`, as
    /// `Control::record` does.
    fn start(ends: &Ends, frames: usize) -> RequestHandle<RecordingRequest> {
        let audio = AudioMemory::new()
            .alloc_buffer(ChannelLayout::Stereo, frames)
            .unwrap();
        ends.recording
            .try_reserve()
            .unwrap()
            .open(RecordingRequest::Start(Arc::new(audio)))
    }

    fn samples(audio: &AudioBuffer) -> Vec<Pcm> {
        audio.frames_in(0..audio.frames()).to_vec()
    }

    #[test]
    fn voices_are_summed() {
        let (ends, mut renderer) = renderer(2);
        let _quiet = play(&ends, &[0.25; 8]);
        let _loud = play(&ends, &[0.5; 8]);
        let mut out = vec![0.0; 8 * 2];
        renderer.render(&mut out);
        assert_eq!(out, vec![0.75; 8 * 2]);
    }

    #[test]
    fn the_sum_is_clipped() {
        let (ends, mut renderer) = renderer(2);
        let _first = play(&ends, &[0.75; 4]);
        let _second = play(&ends, &[0.75; 4]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        assert_eq!(out, vec![1.0; 4 * 2]);
    }

    #[test]
    fn a_period_longer_than_a_block_plays_on_without_a_gap() {
        let frames = Renderer::BLOCK_FRAMES * 2 + 10;
        let ramp: Vec<Pcm> = (0..frames).map(|i| i as Pcm / 1000.0).collect();
        let (ends, mut renderer) = renderer(1);
        let _voice = play(&ends, &ramp);
        let mut out = vec![0.0; frames * 2];
        renderer.render(&mut out);
        let heard: Vec<Pcm> = out.chunks(2).map(|frame| frame[0]).collect();
        assert_eq!(heard, ramp);
    }

    #[test]
    fn periods_shorter_than_a_block_play_on_without_a_gap() {
        let (ends, mut renderer) = renderer(1);
        let _voice = play(&ends, &[0.1, 0.2, 0.3, 0.4]);
        let mut out = vec![0.0; 2 * 2];
        renderer.render(&mut out);
        renderer.render(&mut out);
        assert_eq!(out, vec![0.3, 0.3, 0.4, 0.4]);
    }

    #[test]
    fn a_voice_that_plays_out_ends() {
        let (ends, mut renderer) = renderer(1);
        let mut voice = play(&ends, &[0.5; 4]);
        renderer.render(&mut [0.0; 8 * 2]);
        assert!(ended(&mut voice));
    }

    #[test]
    fn a_cancelled_voice_ends_and_falls_silent() {
        let (ends, mut renderer) = renderer(1);
        let mut voice = play(&ends, &[0.5; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        voice.cancel();
        renderer.render(&mut out);
        assert_eq!(out, vec![0.0; 4 * 2]);
        assert!(ended(&mut voice));
    }

    #[test]
    fn stop_all_ends_every_voice_and_falls_silent() {
        let (ends, mut renderer) = renderer(2);
        let mut first = play(&ends, &[0.5; 64]);
        let mut second = play(&ends, &[0.5; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        stop(&ends, VoiceRequest::StopAll);
        renderer.render(&mut out);
        assert_eq!(out, vec![0.0; 4 * 2]);
        assert!(ended(&mut first));
        assert!(ended(&mut second));
    }

    #[test]
    fn stop_audio_ends_only_the_voices_playing_it() {
        let (ends, mut renderer) = renderer(3);
        let stopped = Arc::new(buffer(ChannelLayout::Mono, &[0.5; 64]));
        let mut first = play_audio(&ends, Arc::clone(&stopped));
        let mut second = play_audio(&ends, Arc::clone(&stopped));
        let mut kept = play(&ends, &[0.25; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&mut out);
        stop(&ends, VoiceRequest::StopAudio(Arc::clone(&stopped)));
        renderer.render(&mut out);
        assert_eq!(out, vec![0.25; 4 * 2]);
        assert!(ended(&mut first));
        assert!(ended(&mut second));
        assert!(!ended(&mut kept));
    }

    #[test]
    fn a_new_voice_in_a_full_pool_cuts_the_oldest() {
        let (ends, mut renderer) = renderer(1);
        let mut oldest = play(&ends, &[0.5; 64]);
        let mut newest = play(&ends, &[0.5; 64]);
        renderer.render(&mut [0.0; 4 * 2]);
        assert!(ended(&mut oldest));
        assert!(!ended(&mut newest));
    }

    #[test]
    fn a_held_voice_answers_that_it_plays() {
        let (ends, mut renderer) = renderer(1);
        let mut voice = play(&ends, &[0.5; 64]);
        let mut status = voice.request_status().unwrap();
        renderer.render(&mut [0.0; 4 * 2]);
        assert_eq!(status.try_recv(), Ok(VoiceStatus::Playing));
    }

    #[test]
    fn a_question_its_voice_closes_before_comes_back_closed() {
        let (ends, mut renderer) = renderer(1);
        let mut voice = play(&ends, &[0.5; 64]);
        renderer.render(&mut [0.0; 4 * 2]);
        voice.cancel();
        let mut status = voice.request_status().unwrap();
        renderer.render(&mut [0.0; 4 * 2]);
        assert_eq!(status.try_recv(), Err(TryRecvStatusError::Closed));
    }

    #[test]
    fn a_recording_writes_what_is_heard_until_stopped() {
        let (ends, mut renderer) = renderer(1);
        let _voice = play(&ends, &[0.25; 64]);
        let mut recording = start(&ends, 64);
        renderer.render(&mut [0.0; 4 * 2]);
        assert_eq!(recording.try_wait().err(), Some(TryWaitError::Empty));

        recording.cancel();
        renderer.render(&mut [0.0; 4 * 2]);

        let (why, audio) = recording.try_wait().unwrap();
        assert_eq!(why, RecordingEnd::Stopped);
        assert_eq!(samples(&audio), vec![0.25; 4 * 2]);
    }

    #[test]
    fn a_recording_ends_full_when_its_audio_has_no_room() {
        let (ends, mut renderer) = renderer(1);
        let mut recording = start(&ends, 3);
        renderer.render(&mut [0.0; 8 * 2]);
        let (why, audio) = recording.try_wait().unwrap();
        assert_eq!(why, RecordingEnd::Full);
        assert_eq!(samples(&audio).len(), 3 * 2);
    }

    #[test]
    fn a_held_recording_answers_that_it_records() {
        let (ends, mut renderer) = renderer(1);
        let mut recording = start(&ends, 64);
        let mut status = recording.request_status().unwrap();
        renderer.render(&mut [0.0; 4 * 2]);
        assert_eq!(status.try_recv(), Ok(RecordingStatus::Recording));
    }
}
