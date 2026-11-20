//! Real-time audio and the code that controls it. There is no `Engine`
//! type: [`create`] returns the two sides.
//!
//! Code runs on control threads, where it may lock and allocate, unless it
//! is in the renderer or a `mod realtime`. Those run on the audio thread,
//! which has a deadline: they never allocate, free, lock, block or make
//! system calls. A helper they call says so where it is defined.
//!
//! Each voice, input or recording is a transfer: its data goes to the
//! renderer and comes back whole once it ends, so it is freed on the control
//! side, and its state is shared by both sides until then.

mod audio;
mod control;
mod input;
mod math;
mod pool;
mod recording;
mod renderer;
mod transfer;

pub use audio::{Audio, LoadError};
pub use control::Control;
pub use input::{Input, InputChannels, InputError};
pub use recording::{RecordError, Recording, Tap, TryTakeError};
pub use renderer::{OUTPUT_CHANNELS, Renderer};

/// The [`Renderer`] goes to the audio thread; the [`Control`] stays.
/// `capture_channels` is the width of the capture `render` is handed each
/// period, zero without an input device; `inputs` is how many can be open
/// at once; `audio`, how many can be loaded at once, each with `VOICES`
/// voices of its own.
pub fn create<const VOICES: usize>(
    capture_channels: usize,
    inputs: usize,
    audio: usize,
) -> (Control<VOICES>, Renderer<VOICES>) {
    let (recordings_control, recordings_realtime) = recording::channel();
    let (inputs_control, inputs_realtime) = input::channel(inputs, capture_channels);
    let (audio_control, audio_realtime) = audio::channel(audio);
    (
        Control::new(
            inputs_control,
            pool::Pool::new(inputs),
            audio_control,
            pool::Pool::new(audio),
            recordings_control,
            capture_channels,
        ),
        Renderer::new(
            inputs_realtime,
            audio_realtime,
            recordings_realtime,
            capture_channels,
        ),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::audio::{Audio, AudioMemory};
    use crate::layout::ChannelLayout;
    use crate::testing::buffer;

    fn audio(samples: &[f32]) -> Audio {
        Audio::new(buffer(ChannelLayout::Mono, samples))
    }

    /// Empty stereo audio with room for `frames`, as a take needs.
    fn room(frames: usize) -> Audio {
        AudioMemory::new()
            .alloc(ChannelLayout::Stereo, frames)
            .unwrap()
    }

    #[test]
    fn a_played_voice_is_heard_then_gives_its_audio_back() {
        let (mut control, mut renderer) = create::<8>(4, 2, 0, 0, 0);
        let audio = audio(&[0.5; 2]);
        let _voice = control.play(&audio).unwrap();

        let mut out = vec![0.0; 3 * 2];
        renderer.render(&[], &mut out);
        control.collect();

        assert_eq!(out, vec![0.5, 0.5, 0.5, 0.5, 0.0, 0.0]);
        assert_eq!(Arc::strong_count(audio.shared()), 1);
    }

    #[test]
    fn a_stopped_voice_falls_silent_and_gives_its_audio_back() {
        let (mut control, mut renderer) = create::<8>(4, 2, 0, 0, 0);
        let audio = audio(&[0.5; 64]);
        let voice = control.play(&audio).unwrap();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        voice.stop();
        let mut out = vec![0.5; 4 * 2];
        renderer.render(&[], &mut out);
        control.collect();

        assert_eq!(out, vec![0.0; 4 * 2]);
        assert_eq!(Arc::strong_count(audio.shared()), 1);
    }

    #[test]
    fn collect_frees_the_audio_of_a_dropped_voice() {
        let (mut control, mut renderer) = create::<8>(4, 2, 0, 0, 0);
        let audio = audio(&[0.5; 2]);
        let voice = control.play(&audio).unwrap();
        drop(voice);
        renderer.render(&[], &mut [0.0; 4 * 2]);
        assert_eq!(
            Arc::strong_count(audio.shared()),
            2,
            "given back, not yet freed"
        );

        control.collect();

        assert_eq!(Arc::strong_count(audio.shared()), 1);
    }

    #[test]
    fn with_no_queue_a_play_is_refused_while_every_voice_is_held() {
        let (mut control, _renderer) = create::<8>(0, 1, 0, 0, 0);
        let audio = audio(&[0.5; 2]);
        let _voice = control.play(&audio).unwrap();
        assert!(matches!(control.play(&audio), Err(VoiceError::Full)));
    }

    #[test]
    fn a_recording_runs_until_stopped() {
        let (mut control, mut renderer) = create::<8>(4, 1, 0, 0, 0);
        let mut recording = control.record(room(64), Tap::MainOut).unwrap();
        renderer.render(&[], &mut [0.0; 4 * 2]);
        assert!(matches!(recording.try_take(), Err(TryTakeError::Recording)));

        recording.stop();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        assert!(recording.try_take().is_ok());
    }

    #[test]
    fn record_refuses_mono_audio_and_hands_it_back() {
        let (mut control, _renderer) = create::<8>(4, 1, 0, 0, 0);
        let mono = AudioMemory::new().alloc(ChannelLayout::Mono, 4).unwrap();
        let error = control.record(mono, Tap::MainOut).unwrap_err();
        assert!(matches!(error, RecordError::NotStereo(_)));
        assert_eq!(error.into_audio().layout(), ChannelLayout::Mono);
    }

    #[test]
    fn record_refuses_audio_a_voice_holds() {
        let (mut control, _renderer) = create::<8>(4, 1, 0, 0, 0);
        let audio = room(4);
        let _voice = control.play(&audio).unwrap();
        assert!(matches!(
            control.record(audio, Tap::MainOut),
            Err(RecordError::AudioInUse(_))
        ));
    }

    #[test]
    fn record_refuses_while_another_recording_holds_the_recorder() {
        let (mut control, _renderer) = create::<8>(4, 1, 0, 0, 0);
        let _first = control.record(room(64), Tap::MainOut).unwrap();
        let error = control.record(room(8), Tap::MainOut).unwrap_err();
        assert!(matches!(error, RecordError::Full(_)));
        assert_eq!(error.into_audio().frames(), 0);
    }

    #[test]
    fn dropping_a_recording_frees_the_recorder() {
        let (mut control, mut renderer) = create::<8>(4, 1, 0, 0, 0);
        let first = control.record(room(64), Tap::MainOut).unwrap();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        drop(first);
        renderer.render(&[], &mut [0.0; 4 * 2]);
        let mut second = control.record(room(64), Tap::MainOut).unwrap();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        assert!(matches!(second.try_take(), Err(TryTakeError::Recording)));
    }

    #[test]
    fn try_take_hands_back_what_was_heard() {
        let (mut control, mut renderer) = create::<8>(4, 1, 0, 0, 0);
        let heard = audio(&[0.25; 64]);
        let _voice = control.play(&heard).unwrap();
        let mut recording = control.record(room(64), Tap::MainOut).unwrap();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        recording.stop();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        let taken = recording.try_take().unwrap();
        assert_eq!(taken.samples(), [0.25; 4 * 2]);
    }

    #[test]
    fn dropping_the_renderer_hands_back_what_was_heard() {
        let (mut control, mut renderer) = create::<8>(4, 1, 0, 0, 0);
        let heard = audio(&[0.25; 64]);
        let _voice = control.play(&heard).unwrap();
        let mut recording = control.record(room(64), Tap::MainOut).unwrap();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        drop(renderer);

        let taken = recording.try_take().unwrap();
        assert_eq!(taken.samples(), [0.25; 4 * 2]);
    }

    #[test]
    fn dropping_the_renderer_hands_back_a_recording_it_never_started() {
        let (mut control, renderer) = create::<8>(4, 1, 0, 0, 0);
        let mut recording = control.record(room(64), Tap::MainOut).unwrap();

        drop(renderer);

        let taken = recording.try_take().unwrap();
        assert_eq!(taken.frames(), 0);
    }

    #[test]
    fn try_take_fails_until_the_recording_ends_and_hands_over_once() {
        let (mut control, mut renderer) = create::<8>(4, 1, 0, 0, 0);
        let mut recording = control.record(room(64), Tap::MainOut).unwrap();
        assert!(matches!(recording.try_take(), Err(TryTakeError::Recording)));
        renderer.render(&[], &mut [0.0; 4 * 2]);
        assert!(matches!(recording.try_take(), Err(TryTakeError::Recording)));

        recording.stop();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        assert!(recording.try_take().is_ok());
        assert!(matches!(recording.try_take(), Err(TryTakeError::Taken)));
    }

    #[test]
    fn a_recording_starts_from_empty() {
        let (mut control, mut renderer) = create::<8>(4, 1, 0, 0, 0);
        let mut used = room(4);
        used.extend([0.9; 4 * 2]);
        let mut recording = control.record(used, Tap::MainOut).unwrap();
        renderer.render(&[], &mut [0.0; 2 * 2]);

        recording.stop();
        renderer.render(&[], &mut [0.0; 2 * 2]);

        let taken = recording.try_take().unwrap();
        assert_eq!(taken.samples(), [0.0; 2 * 2]);
    }

    #[test]
    fn an_open_is_refused_while_every_input_is_open() {
        let (mut control, _renderer) = create::<8>(4, 1, 1, 1, 0);
        let _open = control.open_input(InputChannels::Mono(0)).unwrap();
        assert!(matches!(
            control.open_input(InputChannels::Mono(0)),
            Err(InputError::Full)
        ));
    }

    #[test]
    fn without_capture_channels_every_open_is_refused() {
        let (mut control, _renderer) = create::<8>(4, 1, 0, 1, 0);
        assert!(matches!(
            control.open_input(InputChannels::Mono(0)),
            Err(InputError::NoSuchChannel)
        ));
    }

    #[test]
    fn a_channel_past_the_device_is_refused() {
        let (mut control, _renderer) = create::<8>(4, 1, 2, 2, 0);
        assert!(matches!(
            control.open_input(InputChannels::Mono(2)),
            Err(InputError::NoSuchChannel)
        ));
        assert!(matches!(
            control.open_input(InputChannels::Stereo(0, 2)),
            Err(InputError::NoSuchChannel)
        ));
        assert!(control.open_input(InputChannels::Mono(1)).is_ok());
    }

    #[test]
    fn inputs_switch_from_two_stereo_to_four_mono_on_one_device() {
        let (mut control, mut renderer) = create::<8>(4, 1, 4, 4, 0);
        let stereo = [
            control.open_input(InputChannels::Stereo(0, 1)).unwrap(),
            control.open_input(InputChannels::Stereo(2, 3)).unwrap(),
        ];
        renderer.render(&[0.0; 4 * 4], &mut [0.0; 4 * 2]);

        for input in stereo {
            input.stop();
        }
        renderer.render(&[0.0; 4 * 4], &mut [0.0; 4 * 2]);

        let mono: Result<Vec<Input>, _> = (0..4)
            .map(|channel| control.open_input(InputChannels::Mono(channel)))
            .collect();
        assert_eq!(mono.map(|held| held.len()), Ok(4));
    }

    #[test]
    fn a_dropped_input_gives_its_place_back() {
        let (mut control, mut renderer) = create::<8>(4, 1, 1, 1, 0);
        let open = control.open_input(InputChannels::Mono(0)).unwrap();
        drop(open);
        renderer.render(&[0.0; 4], &mut [0.0; 4 * 2]);
        assert!(control.open_input(InputChannels::Mono(0)).is_ok());
    }

    #[test]
    fn an_input_is_heard_only_once_monitored() {
        let (mut control, mut renderer) = create::<8>(4, 1, 1, 1, 0);
        let mut input = control.open_input(InputChannels::Mono(0)).unwrap();
        let mut out = [0.0; 4 * 2];
        renderer.render(&[0.5; 4], &mut out);
        assert_eq!(out, [0.0; 4 * 2]);

        input.set_monitoring(true);
        renderer.render(&[0.5; 4], &mut out);

        assert_eq!(out, [0.5; 4 * 2]);
    }

    #[test]
    fn a_take_on_an_input_records_it_unmonitored() {
        let (mut control, mut renderer) = create::<8>(4, 1, 1, 1, 0);
        let input = control.open_input(InputChannels::Mono(0)).unwrap();
        let mut recording = control.record(room(64), Tap::Input(&input)).unwrap();
        let mut out = [0.0; 4 * 2];
        renderer.render(&[0.5; 4], &mut out);
        assert_eq!(out, [0.0; 4 * 2]);

        recording.stop();
        renderer.render(&[0.5; 4], &mut out);

        let taken = recording.try_take().unwrap();
        assert_eq!(taken.samples(), [0.5; 4 * 2]);
    }

    #[test]
    fn a_take_ends_with_its_input() {
        let (mut control, mut renderer) = create::<8>(4, 1, 1, 1, 0);
        let input = control.open_input(InputChannels::Mono(0)).unwrap();
        let mut recording = control.record(room(64), Tap::Input(&input)).unwrap();
        renderer.render(&[0.5; 4], &mut [0.0; 4 * 2]);

        input.stop();
        renderer.render(&[0.5; 4], &mut [0.0; 4 * 2]);

        let taken = recording.try_take().unwrap();
        assert_eq!(taken.frames(), 4);
    }
}
