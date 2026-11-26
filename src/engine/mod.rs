//! Real-time audio and the code that controls it. There is no `Engine`
//! type: [`create`] returns the two sides.
//!
//! Code runs on control threads, where it may lock and allocate, unless it
//! is in the renderer or a `mod realtime`. Those run on the audio thread,
//! which has a deadline: they never allocate, free, lock, block or make
//! system calls. A helper they call says so where it is defined.
//!
//! The two sides cross in four ways, each with one job:
//!
//! - A start ring per feature carries a place's state when its use begins
//!   and it holds what only a ring can carry, such as loaded samples. It is
//!   sized to the places, so a push never fails.
//! - Atomics in a state carry everything during its life, both ways:
//!   flags where the latest value wins, numbers that only grow, counters,
//!   a lifecycle moved by compare-exchange. A state both sides hold from
//!   `create`, as a recorder's is, needs no start ring at all.
//! - A stream ring, made at `create`, carries audio that keeps flowing.
//! - `render`'s arguments carry what the device hands over each period.
//!
//! Places are made at `create` and the control side keeps a clone of each,
//! so nothing the renderer holds is freed there; a place is reused only
//! once that clone, with the renderer's if it keeps one, is all that is
//! left. Nothing comes home to the renderer's side; a recorder's consumer
//! comes home between control threads.

mod audio_v2;
mod control;
mod inputs;
mod math;
mod recording;
mod renderer;

pub use audio_v2::{Audio, AudioPool, LoadError, Voice};
pub use control::Control;
pub use inputs::{Input, InputChannels, InputError, Inputs};
pub use recording::{ArmError, Drain, Recorder, Recorders};
pub use renderer::{OUTPUT_CHANNELS, Renderer};

/// The [`Renderer`] goes to the audio thread; the [`Control`] stays.
/// `capture_channels` is the width of the capture `render` is handed each
/// period, zero without an input device; `inputs` is how many can be open
/// at once; `audio`, how many can be loaded at once, each with `voices`
/// voices of its own; `recorders`, how many can be armed at once, each
/// holding `ring_frames` frames its drain has yet to take.
///
/// # Panics
///
/// If `voices` is zero.
pub fn create(
    capture_channels: usize,
    inputs: usize,
    audio: usize,
    voices: usize,
    recorders: usize,
    ring_frames: usize,
) -> (Control, Renderer) {
    let (recorders_control, recorders_realtime) = recording::create(recorders, ring_frames);
    let (inputs_control, inputs_realtime) = inputs::create(inputs, capture_channels);
    let (pool_control, pool) = audio_v2::create(audio, voices);
    (
        Control::new(inputs_control, pool_control, recorders_control),
        Renderer::new(inputs_realtime, pool, recorders_realtime, capture_channels),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::ChannelLayout;

    fn mono(channel: usize) -> InputChannels {
        InputChannels {
            left: channel,
            right: channel,
        }
    }

    fn stereo(left: usize, right: usize) -> InputChannels {
        InputChannels { left, right }
    }

    #[test]
    fn a_loaded_audio_plays_to_its_end() {
        let (mut control, mut renderer) = create(0, 0, 1, 8, 0, 0);
        let mut audio = control.audio().load(vec![0.5; 2], ChannelLayout::Mono).unwrap();
        let _voice = audio.play();
        let mut out = vec![0.0; 3 * 2];
        renderer.render(&[], &mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.5, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn a_dropped_voice_falls_silent() {
        let (mut control, mut renderer) = create(0, 0, 1, 8, 0, 0);
        let mut audio = control.audio().load(vec![0.5; 64], ChannelLayout::Mono).unwrap();
        let voice = audio.play();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        drop(voice);
        let mut out = vec![0.5; 4 * 2];
        renderer.render(&[], &mut out);

        assert_eq!(out, vec![0.0; 4 * 2]);
    }

    #[test]
    fn a_detached_voice_plays_on() {
        let (mut control, mut renderer) = create(0, 0, 1, 8, 0, 0);
        let mut audio = control.audio().load(vec![0.5; 64], ChannelLayout::Mono).unwrap();
        audio.play().detach();
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&[], &mut out);
        assert_eq!(out, vec![0.5; 4 * 2]);
    }

    #[test]
    fn a_play_takes_a_free_voice_before_cutting_one() {
        let (mut control, mut renderer) = create(0, 0, 1, 2, 0, 0);
        let mut audio = control.audio().load(vec![0.25; 64], ChannelLayout::Mono).unwrap();
        let first = audio.play();
        let _second = audio.play();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        drop(first);
        renderer.render(&[], &mut [0.0; 4 * 2]);
        let _third = audio.play();
        let mut out = vec![0.0; 4 * 2];
        renderer.render(&[], &mut out);

        assert_eq!(out, vec![0.5; 4 * 2], "the second and third together");
    }

    #[test]
    fn a_play_with_every_voice_busy_cuts_the_oldest() {
        let (mut control, mut renderer) = create(0, 0, 1, 1, 0, 0);
        let mut audio = control
            .audio()
            .load(vec![0.1, 0.2, 0.3, 0.4], ChannelLayout::Mono)
            .unwrap();
        let _first = audio.play();
        let mut out = vec![0.0; 2 * 2];
        renderer.render(&[], &mut out);

        let _second = audio.play();
        renderer.render(&[], &mut out);

        assert_eq!(out, vec![0.1, 0.1, 0.2, 0.2]);
    }

    #[test]
    fn an_unloaded_audio_falls_silent() {
        let (mut control, mut renderer) = create(0, 0, 1, 8, 0, 0);
        let mut audio = control.audio().load(vec![0.5; 64], ChannelLayout::Mono).unwrap();
        audio.play().detach();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        drop(audio);
        let mut out = vec![0.5; 4 * 2];
        renderer.render(&[], &mut out);

        assert_eq!(out, vec![0.0; 4 * 2]);
    }

    #[test]
    fn load_refuses_part_of_a_frame_and_hands_the_samples_back() {
        let (mut control, _renderer) = create(0, 0, 1, 8, 0, 0);
        let error = control
            .audio()
            .load(vec![0.5; 3], ChannelLayout::Stereo)
            .unwrap_err();
        assert!(matches!(error, LoadError::PartFrame(_)));
        assert_eq!(error.into_samples(), vec![0.5; 3]);
    }

    #[test]
    fn load_refuses_while_every_place_is_held() {
        let (mut control, _renderer) = create(0, 0, 1, 8, 0, 0);
        let _held = control.audio().load(vec![0.5; 2], ChannelLayout::Mono).unwrap();
        let error = control
            .audio()
            .load(vec![0.25; 2], ChannelLayout::Mono)
            .unwrap_err();
        assert!(matches!(error, LoadError::Full(_)));
        assert_eq!(error.into_samples(), vec![0.25; 2]);
    }

    #[test]
    fn an_unloaded_place_is_free_again() {
        let (mut control, mut renderer) = create(0, 0, 1, 8, 0, 0);
        let held = control.audio().load(vec![0.5; 2], ChannelLayout::Mono).unwrap();
        drop(held);
        renderer.render(&[], &mut [0.0; 4 * 2]);
        assert!(control.audio().load(vec![0.25; 2], ChannelLayout::Mono).is_ok());
    }

    #[test]
    fn an_open_is_refused_while_every_input_is_open() {
        let (mut control, _renderer) = create(1, 1, 0, 8, 0, 0);
        let _open = control.inputs().open(mono(0)).unwrap();
        assert!(matches!(
            control.inputs().open(mono(0)),
            Err(InputError::Full)
        ));
    }

    #[test]
    fn without_capture_channels_every_open_is_refused() {
        let (mut control, _renderer) = create(0, 1, 0, 8, 0, 0);
        assert!(matches!(
            control.inputs().open(mono(0)),
            Err(InputError::NoSuchChannel)
        ));
    }

    #[test]
    fn a_channel_past_the_device_is_refused() {
        let (mut control, _renderer) = create(2, 2, 0, 8, 0, 0);
        assert!(matches!(
            control.inputs().open(mono(2)),
            Err(InputError::NoSuchChannel)
        ));
        assert!(matches!(
            control.inputs().open(stereo(0, 2)),
            Err(InputError::NoSuchChannel)
        ));
        assert!(control.inputs().open(mono(1)).is_ok());
    }

    #[test]
    fn inputs_switch_from_two_stereo_to_four_mono_on_one_device() {
        let (mut control, mut renderer) = create(4, 4, 0, 8, 0, 0);
        let stereo = [
            control.inputs().open(stereo(0, 1)).unwrap(),
            control.inputs().open(stereo(2, 3)).unwrap(),
        ];
        renderer.render(&[0.0; 4 * 4], &mut [0.0; 4 * 2]);

        for input in stereo {
            drop(input);
        }
        renderer.render(&[0.0; 4 * 4], &mut [0.0; 4 * 2]);

        let mono: Result<Vec<Input>, _> = (0..4)
            .map(|channel| control.inputs().open(mono(channel)))
            .collect();
        assert_eq!(mono.map(|held| held.len()), Ok(4));
    }

    #[test]
    fn a_dropped_input_gives_its_place_back() {
        let (mut control, mut renderer) = create(1, 1, 0, 8, 0, 0);
        let open = control.inputs().open(mono(0)).unwrap();
        drop(open);
        renderer.render(&[0.0; 4], &mut [0.0; 4 * 2]);
        assert!(control.inputs().open(mono(0)).is_ok());
    }

    #[test]
    fn an_input_is_heard_only_once_monitored() {
        let (mut control, mut renderer) = create(1, 1, 0, 8, 0, 0);
        let mut input = control.inputs().open(mono(0)).unwrap();
        let mut out = [0.0; 4 * 2];
        renderer.render(&[0.5; 4], &mut out);
        assert_eq!(out, [0.0; 4 * 2]);

        input.set_monitoring(true);
        renderer.render(&[0.5; 4], &mut out);

        assert_eq!(out, [0.5; 4 * 2]);
    }
}
