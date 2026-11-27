//! The audio thread: turns what the control side sends into periods of
//! output. An offline bounce runs the same code with no deadline.

use crate::engine::audio::realtime::AudioPool;
use crate::engine::inputs::realtime::Inputs;
use crate::engine::math::{add, hard_clip};
use crate::engine::recording::realtime::Recorders;

/// Always stereo.
pub const OUTPUT_CHANNELS: usize = 2;

/// A period of any length is mixed in blocks this long, so scratch never
/// grows.
const BLOCK_FRAMES: usize = 64;

pub(crate) type Frame = [f32; OUTPUT_CHANNELS];

/// A stream drops the callback that owns it, so to survive a stream
/// restart the callback must hand the renderer back when dropped.
#[derive(Debug)]
pub struct Renderer {
    /// Per frame of `render`'s capture; zero without an input device.
    capture_channels: usize,
    inputs: Inputs,
    pool: AudioPool,
    recorders: Recorders,
    /// One sound's frames for the block, before they join the mix.
    scratch: Box<[Frame]>,
}

impl Renderer {
    /// Allocates everything the renderer holds for its life, here, on the
    /// control side.
    pub(crate) fn new(
        inputs: Inputs,
        pool: AudioPool,
        recorders: Recorders,
        capture_channels: usize,
    ) -> Self {
        Self {
            capture_channels,
            inputs,
            pool,
            recorders,
            scratch: vec![[0.0; OUTPUT_CHANNELS]; BLOCK_FRAMES].into_boxed_slice(),
        }
    }

    /// One period, interleaved stereo, replacing what `out` held: every
    /// playing voice and monitored input, summed and clipped. `capture` is
    /// the input device's frames for the same period, interleaved.
    pub fn render(&mut self, capture: &[f32], out: &mut [f32]) {
        debug_assert_eq!(out.len() % OUTPUT_CHANNELS, 0, "output is not whole frames");
        debug_assert_eq!(
            capture.len(),
            (out.len() / OUTPUT_CHANNELS) * self.capture_channels,
            "capture and output are not the same frames"
        );
        for (out_block, capture_block) in blocks(out, capture, self.capture_channels) {
            out_block.fill([0.0; OUTPUT_CHANNELS]);
            let scratch = &mut self.scratch[..out_block.len()];
            self.pool.receive();
            self.pool.unload();
            // The pool's voices add into scratch.
            scratch.fill([0.0; OUTPUT_CHANNELS]);
            self.pool.render(scratch);
            add(out_block, scratch);
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
    out: &'a mut [f32],
    capture: &'a [f32],
    capture_channels: usize,
) -> impl Iterator<Item = (&'a mut [Frame], &'a [f32])> {
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
    use super::*;
    use crate::engine::audio::{self, Audio, Voice};
    use crate::engine::layout::ChannelLayout;
    use crate::engine::{inputs, recording};

    /// A renderer with room for `places` loaded audio of one voice each, and
    /// the control side of its pool.
    fn renderer(places: usize) -> (audio::AudioPool, Renderer) {
        let (pool, pool_realtime) = audio::create(places, 1);
        let (_, recorders_realtime) = recording::create(0, 0);
        let (_, inputs_realtime) = inputs::create(0, 0);
        let renderer = Renderer::new(inputs_realtime, pool_realtime, recorders_realtime, 0);
        (pool, renderer)
    }

    /// Loads mono `samples`, then plays them.
    fn play(pool: &mut audio::AudioPool, samples: &[f32]) -> (Audio, Voice) {
        let mut audio = pool.load(samples.to_vec(), ChannelLayout::Mono).unwrap();
        let voice = audio.play();
        (audio, voice)
    }

    #[test]
    fn voices_are_summed() {
        let (mut pool, mut renderer) = renderer(2);
        let _quiet = play(&mut pool, &[0.25; 8]);
        let _loud = play(&mut pool, &[0.5; 8]);
        let mut out = vec![0.0; 8 * 2];
        renderer.render(&[], &mut out);
        assert_eq!(out, vec![0.75; 8 * 2]);
    }

    #[test]
    fn the_sum_is_clipped() {
        let (mut pool, mut renderer) = renderer(2);
        let _first = play(&mut pool, &[0.75; 4]);
        let _second = play(&mut pool, &[0.75; 4]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&[], &mut out);
        assert_eq!(out, vec![1.0; 4 * 2]);
    }

    #[test]
    fn a_period_longer_than_a_block_plays_on_without_a_gap() {
        let frames = BLOCK_FRAMES * 2 + 10;
        let ramp: Vec<f32> = (0..frames).map(|i| i as f32 / 1000.0).collect();
        let (mut pool, mut renderer) = renderer(1);
        let _voice = play(&mut pool, &ramp);
        let mut out = vec![0.0; frames * 2];
        renderer.render(&[], &mut out);
        let heard: Vec<f32> = out.chunks(2).map(|frame| frame[0]).collect();
        assert_eq!(heard, ramp);
    }

    #[test]
    fn periods_shorter_than_a_block_play_on_without_a_gap() {
        let (mut pool, mut renderer) = renderer(1);
        let _voice = play(&mut pool, &[0.1, 0.2, 0.3, 0.4]);
        let mut out = vec![0.0; 2 * 2];
        renderer.render(&[], &mut out);
        renderer.render(&[], &mut out);
        assert_eq!(out, vec![0.3, 0.3, 0.4, 0.4]);
    }

    #[test]
    fn a_voice_that_plays_out_falls_silent() {
        let (mut pool, mut renderer) = renderer(1);
        let _voice = play(&mut pool, &[0.5; 4]);
        let mut out = vec![0.0; 8 * 2];
        renderer.render(&[], &mut out);
        assert_eq!(out[..4 * 2], [0.5; 4 * 2]);
        assert_eq!(out[4 * 2..], [0.0; 4 * 2]);
    }

    #[test]
    fn a_stopped_voice_falls_silent() {
        let (mut pool, mut renderer) = renderer(1);
        let (_audio, voice) = play(&mut pool, &[0.5; 64]);
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&[], &mut out);
        drop(voice);
        renderer.render(&[], &mut out);
        assert_eq!(out, vec![0.0; 4 * 2]);
    }

    #[test]
    fn a_play_with_every_voice_busy_starts_again_from_the_top() {
        let (mut pool, mut renderer) = renderer(1);
        let (mut audio, _first) = play(&mut pool, &[0.1, 0.2, 0.3, 0.4]);
        let mut out = vec![0.0; 2 * 2];
        renderer.render(&[], &mut out);

        let _second = audio.play();
        renderer.render(&[], &mut out);

        assert_eq!(out, vec![0.1, 0.1, 0.2, 0.2]);
    }

    #[test]
    fn blocks_cut_the_capture_at_the_same_frames_as_the_output() {
        let frames = BLOCK_FRAMES + 3;
        let mut out = vec![0.0; frames * 2];
        let capture: Vec<f32> = (0..frames * 3).map(|i| i as f32).collect();
        let cut: Vec<(usize, Vec<f32>)> = blocks(&mut out, &capture, 3)
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
