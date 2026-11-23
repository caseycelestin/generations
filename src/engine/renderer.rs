//! The audio thread: turns what the control side sends into periods of
//! output. An offline bounce runs the same code with no deadline.

use crate::config::Pcm;
use crate::engine::audio::realtime::LoadedAudio;
use crate::engine::input::realtime::Inputs;
use crate::engine::math::{add, hard_clip};
use crate::engine::recording::realtime::Recorders;

/// Always stereo.
pub const OUTPUT_CHANNELS: usize = 2;

/// A period of any length is mixed in blocks this long, so scratch never
/// grows.
const BLOCK_FRAMES: usize = 64;

pub(crate) type Frame = [Pcm; OUTPUT_CHANNELS];

/// Dropping it ends the engine: [`Control`](crate::engine::Control) and
/// every handle from the same [`create`](crate::engine::create) report the
/// renderer gone. A stream
/// drops the callback that owns it, so to survive a stream restart the
/// callback must hand it back when dropped.
#[derive(Debug)]
pub struct Renderer {
    /// Per frame of `render`'s capture; zero without an input device.
    capture_channels: usize,
    inputs: Inputs,
    audio: LoadedAudio,
    recorders: Recorders,
    /// One sound's frames for the block, before they join the mix.
    scratch: Box<[Frame]>,
}

impl Renderer {
    /// Allocates everything the renderer holds for its life, here, on the
    /// control side.
    pub(crate) fn new(
        inputs: Inputs,
        audio: LoadedAudio,
        recorders: Recorders,
        capture_channels: usize,
    ) -> Self {
        Self {
            capture_channels,
            inputs,
            audio,
            recorders,
            scratch: vec![[0.0; OUTPUT_CHANNELS]; BLOCK_FRAMES].into_boxed_slice(),
        }
    }

    /// One period, interleaved stereo, replacing what `out` held: every
    /// playing voice and monitored input, summed and clipped. `capture` is
    /// the input device's frames for the same period, interleaved.
    pub fn render(&mut self, capture: &[Pcm], out: &mut [Pcm]) {
        debug_assert_eq!(out.len() % OUTPUT_CHANNELS, 0, "output is not whole frames");
        debug_assert_eq!(
            capture.len(),
            (out.len() / OUTPUT_CHANNELS) * self.capture_channels,
            "capture and output are not the same frames"
        );
        self.inputs.receive();
        self.audio.receive();
        for (out_block, capture_block) in blocks(out, capture, self.capture_channels) {
            out_block.fill([0.0; OUTPUT_CHANNELS]);
            let scratch = &mut self.scratch[..out_block.len()];
            for audio in self.audio.iter_mut() {
                audio.render(out_block, scratch);
            }
            for input in self.inputs.iter() {
                input.render(capture_block, scratch);
                if input.monitoring() {
                    add(out_block, scratch);
                }
            }
            hard_clip(out_block);
            for recorder in self.recorders.iter_mut() {
                recorder.render(out_block);
            }
        }
    }
}

/// A period cut into blocks, each output block with its capture.
fn blocks<'a>(
    out: &'a mut [Pcm],
    capture: &'a [Pcm],
    capture_channels: usize,
) -> impl Iterator<Item = (&'a mut [Frame], &'a [Pcm])> {
    let (frames, _) = out.as_chunks_mut::<{ OUTPUT_CHANNELS }>();
    frames
        .chunks_mut(BLOCK_FRAMES)
        .enumerate()
        .map(move |(index, out_block)| {
            let start = index * BLOCK_FRAMES * capture_channels;
            let end = start + out_block.len() * capture_channels;
            (out_block, &capture[start..end])
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use rtrb::Producer;

    use super::*;
    use crate::audio::{AudioBuffer, AudioMemory};
    use crate::engine::audio::{self, Audio, AudioState};
    use crate::engine::recording::{RecordingData, RecordingState, TapData};
    use crate::engine::transfer::{self, Transfer, TryWaitError};
    use crate::engine::voice::Voice;
    use crate::engine::{input, recording};
    use crate::layout::ChannelLayout;

    /// The control ends of the renderer's channels.
    struct Ends {
        audio: Producer<Arc<AudioState>>,
        recordings: transfer::ControlEnd<RecordingData>,
    }

    /// A renderer with room for `places` loaded audio of one voice each, and
    /// the control ends of its channels.
    fn renderer(places: usize) -> (Ends, Renderer) {
        let (audio_control, audio_realtime) = audio::channel(places);
        let (recordings_control, recordings_realtime) = recording::channel();
        let (_, inputs_realtime) = input::channel(0, 0);
        let ends = Ends {
            audio: audio_control,
            recordings: recordings_control,
        };
        let renderer = Renderer::new(inputs_realtime, audio_realtime, recordings_realtime, 0);
        (ends, renderer)
    }

    /// Loads mono `samples` as `Control::load` does, then plays them; the
    /// state stands in for the pool's clone.
    fn play(ends: &mut Ends, samples: &[Pcm]) -> (Arc<AudioState>, Audio, Voice) {
        let mut state = AudioState::new(1);
        state.refill(audio::AudioBuffer {
            samples: samples.to_vec(),
            layout: ChannelLayout::Mono,
        });
        let state = Arc::new(state);
        ends.audio.push(Arc::clone(&state)).unwrap();
        let mut audio = Audio::load(Arc::clone(&state));
        let voice = audio.play();
        (state, audio, voice)
    }

    /// The renderer finished the first voice played: it played out or was
    /// stopped.
    fn ended(state: &AudioState) -> bool {
        state.voices[0].is_free()
    }

    /// Starts a recording into empty stereo audio with room for `frames`,
    /// as `Control::record` does, keeping the state its handle stops it
    /// through.
    fn record(ends: &mut Ends, frames: usize) -> (Transfer<RecordingData>, Arc<RecordingState>) {
        let audio = AudioMemory::new()
            .alloc_buffer(ChannelLayout::Stereo, frames)
            .unwrap();
        let state = Arc::new(RecordingState::default());
        let data = RecordingData {
            audio: Arc::new(audio),
            tap: TapData::MainOut,
            state: Arc::clone(&state),
        };
        let recording = ends.recordings.try_start(data).unwrap();
        (recording, state)
    }

    fn samples(audio: &AudioBuffer) -> Vec<Pcm> {
        audio.frames_in(0..audio.frames()).to_vec()
    }

    #[test]
    fn voices_are_summed() {
        let (mut ends, mut renderer) = renderer(2);
        let _quiet = play(&mut ends, &[0.25; 8]);
        let _loud = play(&mut ends, &[0.5; 8]);
        let mut out = vec![0.0; 8 * 2];
        renderer.render(&[], &mut out);
        assert_eq!(out, vec![0.75; 8 * 2]);
    }

    #[test]
    fn the_sum_is_clipped() {
        let (mut ends, mut renderer) = renderer(2);
        let _first = play(&mut ends, &[0.75; 4]);
        let _second = play(&mut ends, &[0.75; 4]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&[], &mut out);
        assert_eq!(out, vec![1.0; 4 * 2]);
    }

    #[test]
    fn a_period_longer_than_a_block_plays_on_without_a_gap() {
        let frames = BLOCK_FRAMES * 2 + 10;
        let ramp: Vec<Pcm> = (0..frames).map(|i| i as Pcm / 1000.0).collect();
        let (mut ends, mut renderer) = renderer(1);
        let _voice = play(&mut ends, &ramp);
        let mut out = vec![0.0; frames * 2];
        renderer.render(&[], &mut out);
        let heard: Vec<Pcm> = out.chunks(2).map(|frame| frame[0]).collect();
        assert_eq!(heard, ramp);
    }

    #[test]
    fn periods_shorter_than_a_block_play_on_without_a_gap() {
        let (mut ends, mut renderer) = renderer(1);
        let _voice = play(&mut ends, &[0.1, 0.2, 0.3, 0.4]);
        let mut out = vec![0.0; 2 * 2];
        renderer.render(&[], &mut out);
        renderer.render(&[], &mut out);
        assert_eq!(out, vec![0.3, 0.3, 0.4, 0.4]);
    }

    #[test]
    fn a_voice_that_plays_out_ends() {
        let (mut ends, mut renderer) = renderer(1);
        let (state, _audio, _voice) = play(&mut ends, &[0.5; 4]);
        renderer.render(&[], &mut [0.0; 8 * 2]);
        assert!(ended(&state));
    }

    #[test]
    fn a_stopped_voice_ends_and_falls_silent() {
        let (mut ends, mut renderer) = renderer(1);
        let (state, _audio, voice) = play(&mut ends, &[0.5; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&[], &mut out);
        drop(voice);
        renderer.render(&[], &mut out);
        assert_eq!(out, vec![0.0; 4 * 2]);
        assert!(ended(&state));
    }

    #[test]
    fn a_play_with_every_voice_busy_starts_again_from_the_top() {
        let (mut ends, mut renderer) = renderer(1);
        let (_state, mut audio, _first) = play(&mut ends, &[0.1, 0.2, 0.3, 0.4]);
        let mut out = vec![0.0; 2 * 2];
        renderer.render(&[], &mut out);

        let _second = audio.play();
        renderer.render(&[], &mut out);

        assert_eq!(out, vec![0.1, 0.1, 0.2, 0.2]);
    }

    #[test]
    fn a_recording_writes_what_is_heard_until_stopped() {
        let (mut ends, mut renderer) = renderer(1);
        let _voice = play(&mut ends, &[0.25; 64]);
        let (mut recording, state) = record(&mut ends, 64);
        renderer.render(&[], &mut [0.0; 4 * 2]);
        assert_eq!(recording.try_wait().err(), Some(TryWaitError::Empty));

        state.stop.store(true, Ordering::Relaxed);
        renderer.render(&[], &mut [0.0; 4 * 2]);

        let data = recording.try_wait().unwrap();
        assert_eq!(samples(&data.audio), vec![0.25; 4 * 2]);
    }

    #[test]
    fn a_recording_ends_full_when_its_audio_has_no_room() {
        let (mut ends, mut renderer) = renderer(1);
        let (mut recording, _) = record(&mut ends, 3);
        renderer.render(&[], &mut [0.0; 8 * 2]);
        let data = recording.try_wait().unwrap();
        assert_eq!(samples(&data.audio).len(), 3 * 2);
    }

    #[test]
    fn blocks_cut_the_capture_at_the_same_frames_as_the_output() {
        let frames = BLOCK_FRAMES + 3;
        let mut out = vec![0.0; frames * 2];
        let capture: Vec<Pcm> = (0..frames * 3).map(|i| i as Pcm).collect();
        let cut: Vec<(usize, Vec<Pcm>)> = blocks(&mut out, &capture, 3)
            .map(|(out_block, capture_block)| (out_block.len(), capture_block.to_vec()))
            .collect();
        let split = BLOCK_FRAMES * 3;
        assert_eq!(
            cut,
            [
                (BLOCK_FRAMES, capture[..split].to_vec()),
                (3, capture[split..].to_vec()),
            ]
        );
    }

    #[test]
    fn without_capture_channels_every_block_has_an_empty_capture() {
        let mut out = vec![0.0; (BLOCK_FRAMES + 3) * 2];
        let cut: Vec<(usize, usize)> = blocks(&mut out, &[], 0)
            .map(|(out_block, capture_block)| (out_block.len(), capture_block.len()))
            .collect();
        assert_eq!(cut, [(BLOCK_FRAMES, 0), (3, 0)]);
    }
}
