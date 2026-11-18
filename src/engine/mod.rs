//! Real-time audio and the code that controls it, split by thread. There is
//! no `Engine` type: [`create`] returns the two sides.

mod control;
mod protocol;
mod realtime;

pub use control::{
    Control, ControlError, Input, PendingInputStatus, PendingRecordingStatus, RecordError,
    Recording, StatusError, TryRecvStatusError, TryWaitError, Voice,
};
pub use protocol::inputs::{InputChannels, InputEnd, InputStatus};
pub use protocol::recording::{RecordingEnd, RecordingStatus};
pub use realtime::Renderer;

use std::num::NonZeroUsize;

use protocol::inputs::InputData;
use protocol::recording::RecordingRequest;
use protocol::voice::VoiceData;

/// Recordings the renderer holds at once, so the requests its recording
/// channel needs: each holds one until its reply is taken.
const RECORDERS: usize = 1;

/// The [`Renderer`] goes to the audio thread; the [`Control`] stays.
/// `voices` is how many play at once, sized by the caller for its hardware.
/// `voice_queue` is how many plays can be on their way beyond them: the
/// room a new play has to take the oldest voice's place. Zero refuses a
/// play while every voice is held. `capture_channels` is the width of the
/// capture `render` is handed each period, zero without an input device;
/// `inputs` is how many can be open at once.
///
/// # Panics
/// If `voices` is zero.
pub fn create(
    voice_queue: usize,
    voices: usize,
    capture_channels: usize,
    inputs: usize,
) -> (Control, Renderer) {
    let voices = NonZeroUsize::new(voices).expect("voices must not be zero");
    let (voices_control, voices_realtime) =
        crate::transfer::channel::<VoiceData>(voice_capacity(voice_queue, voices));
    let (recording_control, recording_realtime) =
        crate::request::channel::<RecordingRequest>(RECORDERS);
    let (inputs_control, inputs_realtime) = crate::request::channel::<InputData>(inputs);
    (
        Control::new(
            control::Requests {
                recording: recording_control,
                inputs: inputs_control,
            },
            control::Transfers {
                voices: voices_control,
            },
            capture_channels,
        ),
        Renderer::new(
            realtime::Requests {
                recording: recording_realtime,
                inputs: inputs_realtime,
            },
            voices_realtime,
            voices,
            capture_channels,
            inputs,
        ),
    )
}

/// Every voice playing, plus the queue; each holds one from play until it
/// ends.
fn voice_capacity(voice_queue: usize, voices: NonZeroUsize) -> usize {
    voices.get() + voice_queue
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

    /// Asks, then renders a frame for the renderer to answer in.
    fn status(recording: &mut Recording, renderer: &mut Renderer) -> RecordingStatus {
        let mut pending = recording.request_status().unwrap();
        renderer.render(&[], &mut [0.0; 2]);
        pending.try_recv().unwrap()
    }

    #[test]
    fn a_played_voice_is_heard_then_ends() {
        let (mut control, mut renderer) = create(4, 2, 0, 0);
        let audio = audio(&[0.5; 2]);
        let mut voice = control.play(&audio).unwrap();
        assert_eq!(voice.try_wait(), Err(TryWaitError::NotEnded));

        let mut out = vec![0.0; 3 * 2];
        renderer.render(&[], &mut out);

        assert_eq!(out, vec![0.5, 0.5, 0.5, 0.5, 0.0, 0.0]);
        assert_eq!(voice.try_wait(), Ok(()));
    }

    #[test]
    fn a_stopped_voice_ends() {
        let (mut control, mut renderer) = create(4, 2, 0, 0);
        let audio = audio(&[0.5; 64]);
        let mut voice = control.play(&audio).unwrap();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        voice.stop();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        assert_eq!(voice.try_wait(), Ok(()));
    }

    #[test]
    fn collect_frees_the_audio_of_a_dropped_voice() {
        let (mut control, mut renderer) = create(4, 2, 0, 0);
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
        let (mut control, _renderer) = create(0, 1, 0, 0);
        let audio = audio(&[0.5; 2]);
        let _voice = control.play(&audio).unwrap();
        assert!(matches!(control.play(&audio), Err(ControlError::Full)));
    }

    #[test]
    fn a_recording_runs_until_stopped() {
        let (mut control, mut renderer) = create(4, 1, 0, 0);
        let mut recording = control.record(room(64)).unwrap();
        renderer.render(&[], &mut [0.0; 4 * 2]);
        assert_eq!(
            status(&mut recording, &mut renderer),
            RecordingStatus::Recording
        );

        recording.stop();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        let (why, _) = recording.try_wait().unwrap();
        assert_eq!(why, RecordingEnd::Stopped);
    }

    #[test]
    fn record_refuses_mono_audio_and_hands_it_back() {
        let (mut control, _renderer) = create(4, 1, 0, 0);
        let mono = AudioMemory::new().alloc(ChannelLayout::Mono, 4).unwrap();
        let error = control.record(mono).unwrap_err();
        assert!(matches!(error, RecordError::NotStereo(_)));
        assert_eq!(error.into_audio().layout(), ChannelLayout::Mono);
    }

    #[test]
    fn record_refuses_audio_a_voice_holds() {
        let (mut control, _renderer) = create(4, 1, 0, 0);
        let audio = room(4);
        let _voice = control.play(&audio).unwrap();
        assert!(matches!(
            control.record(audio),
            Err(RecordError::AudioInUse(_))
        ));
    }

    #[test]
    fn record_refuses_while_another_recording_holds_the_recorder() {
        let (mut control, _renderer) = create(4, 1, 0, 0);
        let _first = control.record(room(64)).unwrap();
        let error = control.record(room(8)).unwrap_err();
        assert!(matches!(error, RecordError::Full(_)));
        assert_eq!(error.into_audio().frames(), 0);
    }

    #[test]
    fn dropping_a_recording_frees_the_recorder() {
        let (mut control, mut renderer) = create(4, 1, 0, 0);
        let first = control.record(room(64)).unwrap();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        drop(first);
        renderer.render(&[], &mut [0.0; 4 * 2]);
        let mut second = control.record(room(64)).unwrap();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        assert_eq!(
            status(&mut second, &mut renderer),
            RecordingStatus::Recording
        );
    }

    #[test]
    fn try_wait_hands_back_what_was_heard() {
        let (mut control, mut renderer) = create(4, 1, 0, 0);
        let heard = audio(&[0.25; 64]);
        let _voice = control.play(&heard).unwrap();
        let mut recording = control.record(room(64)).unwrap();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        recording.stop();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        let (_, taken) = recording.try_wait().unwrap();
        assert_eq!(taken.samples(), [0.25; 4 * 2]);
    }

    #[test]
    fn dropping_the_renderer_hands_back_what_was_heard() {
        let (mut control, mut renderer) = create(4, 1, 0, 0);
        let heard = audio(&[0.25; 64]);
        let _voice = control.play(&heard).unwrap();
        let mut recording = control.record(room(64)).unwrap();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        drop(renderer);

        let (why, taken) = recording.try_wait().unwrap();
        assert_eq!(why, RecordingEnd::RendererGone);
        assert_eq!(taken.samples(), [0.25; 4 * 2]);
    }

    #[test]
    fn dropping_the_renderer_hands_back_a_recording_it_never_started() {
        let (mut control, renderer) = create(4, 1, 0, 0);
        let mut recording = control.record(room(64)).unwrap();

        drop(renderer);

        let (why, taken) = recording.try_wait().unwrap();
        assert_eq!(why, RecordingEnd::RendererGone);
        assert_eq!(taken.frames(), 0);
    }

    #[test]
    fn try_wait_is_not_ended_until_the_recording_ends_and_taken_once() {
        let (mut control, mut renderer) = create(4, 1, 0, 0);
        let mut recording = control.record(room(64)).unwrap();
        assert!(matches!(recording.try_wait(), Err(TryWaitError::NotEnded)));
        renderer.render(&[], &mut [0.0; 4 * 2]);
        assert!(matches!(recording.try_wait(), Err(TryWaitError::NotEnded)));

        recording.stop();
        renderer.render(&[], &mut [0.0; 4 * 2]);

        assert!(recording.try_wait().is_ok());
        assert!(matches!(recording.try_wait(), Err(TryWaitError::Taken)));
    }

    #[test]
    fn a_recording_starts_from_empty() {
        let (mut control, mut renderer) = create(4, 1, 0, 0);
        let mut used = room(4);
        used.extend([0.9; 4 * 2]);
        let mut recording = control.record(used).unwrap();
        renderer.render(&[], &mut [0.0; 2 * 2]);

        recording.stop();
        renderer.render(&[], &mut [0.0; 2 * 2]);

        let (_, taken) = recording.try_wait().unwrap();
        assert_eq!(taken.samples(), [0.0; 2 * 2]);
    }

    #[test]
    fn a_stopped_input_ends_stopped() {
        let (control, mut renderer) = create(4, 1, 1, 1);
        let mut input = control.open_input(InputChannels::Mono(0)).unwrap();
        renderer.render(&[0.0; 4], &mut [0.0; 4 * 2]);
        assert_eq!(input.try_wait(), Err(TryWaitError::NotEnded));

        input.stop();
        renderer.render(&[0.0; 4], &mut [0.0; 4 * 2]);

        assert_eq!(input.try_wait(), Ok(InputEnd::Stopped));
    }

    #[test]
    fn an_open_is_refused_while_every_input_is_open() {
        let (control, _renderer) = create(4, 1, 1, 1);
        let _open = control.open_input(InputChannels::Mono(0)).unwrap();
        assert!(matches!(
            control.open_input(InputChannels::Mono(0)),
            Err(ControlError::Full)
        ));
    }

    #[test]
    fn without_capture_channels_every_open_is_refused() {
        let (control, _renderer) = create(4, 1, 0, 1);
        assert!(matches!(
            control.open_input(InputChannels::Mono(0)),
            Err(ControlError::NoSuchChannel)
        ));
    }

    #[test]
    fn a_channel_past_the_device_is_refused() {
        let (control, _renderer) = create(4, 1, 2, 2);
        assert!(matches!(
            control.open_input(InputChannels::Mono(2)),
            Err(ControlError::NoSuchChannel)
        ));
        assert!(matches!(
            control.open_input(InputChannels::Stereo(0, 2)),
            Err(ControlError::NoSuchChannel)
        ));
        assert!(control.open_input(InputChannels::Mono(1)).is_ok());
    }

    #[test]
    fn inputs_switch_from_two_stereo_to_four_mono_on_one_device() {
        let (control, mut renderer) = create(4, 1, 4, 4);
        let mut stereo = [
            control.open_input(InputChannels::Stereo(0, 1)).unwrap(),
            control.open_input(InputChannels::Stereo(2, 3)).unwrap(),
        ];
        renderer.render(&[0.0; 4 * 4], &mut [0.0; 4 * 2]);

        for input in &mut stereo {
            input.stop();
        }
        renderer.render(&[0.0; 4 * 4], &mut [0.0; 4 * 2]);
        // Taking the replies gives the places back.
        for input in &mut stereo {
            assert_eq!(input.try_wait(), Ok(InputEnd::Stopped));
        }

        let mono: Result<Vec<Input>, _> = (0..4)
            .map(|channel| control.open_input(InputChannels::Mono(channel)))
            .collect();
        assert_eq!(mono.map(|held| held.len()), Ok(4));
    }

    #[test]
    fn a_dropped_input_gives_its_place_back() {
        let (control, mut renderer) = create(4, 1, 1, 1);
        let open = control.open_input(InputChannels::Mono(0)).unwrap();
        drop(open);
        renderer.render(&[0.0; 4], &mut [0.0; 4 * 2]);
        assert!(control.open_input(InputChannels::Mono(0)).is_ok());
    }

    #[test]
    fn a_held_input_answers_its_status() {
        let (control, mut renderer) = create(4, 1, 1, 1);
        let mut input = control.open_input(InputChannels::Mono(0)).unwrap();
        let mut pending = input.request_status().unwrap();
        renderer.render(&[0.0; 4], &mut [0.0; 4 * 2]);
        assert_eq!(pending.try_recv(), Ok(InputStatus::Open));
    }
}
