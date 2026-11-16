//! Real-time audio and the code that controls it, split by thread. There is
//! no `Engine` type: [`create`] returns the two sides.

mod control;
mod protocol;
mod realtime;

pub use control::{
    Control, ControlError, Input, PendingInputStatus, PendingRecordingStatus, PendingVoiceStatus,
    PlayError, RecordError, Recording, StatusError, StopError, TryRecvStatusError, TryWaitError,
    Voice,
};
pub use protocol::inputs::{InputEnd, InputStatus};
pub use protocol::recording::{RecordingEnd, RecordingStatus};
pub use protocol::voice::VoiceStatus;
pub use realtime::capture::Feeder;
pub use realtime::render::Renderer;

use std::num::NonZeroUsize;

use protocol::inputs::InputRequest;
use protocol::recording::RecordingRequest;
use protocol::voice::VoiceRequest;

/// Recordings the renderer holds at once, so the requests its recording
/// channel needs: each holds one until its reply is taken.
const RECORDERS: usize = 1;

/// Headroom on the voice channel for `stop` and `stop_all`, so a burst of
/// plays cannot leave them no room.
const VOICE_STOP_HEADROOM: usize = 2;

/// The [`Renderer`] goes to the audio thread; the [`Control`] stays.
/// `voices` is how many play at once, sized by the caller for its hardware.
/// `voice_queue` is how many plays and stops can be on their way beyond
/// them: the room a new play has to take the oldest voice's place. Zero
/// refuses a play while every voice is held. `inputs` is how many can be
/// open at once, and may be zero.
///
/// # Panics
/// If `voices` is zero.
pub fn create(voice_queue: usize, voices: usize, inputs: usize) -> (Control, Renderer) {
    let voices = NonZeroUsize::new(voices).expect("voices must not be zero");
    let (voices_control, voices_realtime) = crate::request::channel_with_headroom::<VoiceRequest>(
        voice_capacity(voice_queue, voices),
        VOICE_STOP_HEADROOM,
    );
    let (recording_control, recording_realtime) =
        crate::request::channel::<RecordingRequest>(RECORDERS);
    let (inputs_control, inputs_realtime) = crate::request::channel::<InputRequest>(inputs);
    (
        Control::new(control::Requests {
            voices: voices_control,
            recording: recording_control,
            inputs: inputs_control,
        }),
        Renderer::new(
            realtime::render::Requests {
                voices: voices_realtime,
                recording: recording_realtime,
                inputs: inputs_realtime,
            },
            voices,
            inputs,
        ),
    )
}

/// Every voice playing, plus the queue, plus the stops' headroom; each
/// holds one until its reply is taken.
fn voice_capacity(voice_queue: usize, voices: NonZeroUsize) -> usize {
    voices.get() + voice_queue + VOICE_STOP_HEADROOM
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
        renderer.render(&mut [0.0; 2]);
        pending.try_recv().unwrap()
    }

    #[test]
    fn a_played_voice_is_heard_then_ends() {
        let (control, mut renderer) = create(4, 2, 0);
        let audio = audio(&[0.5; 2]);
        let mut voice = control.play(&audio).unwrap();
        assert_eq!(voice.try_wait(), Err(TryWaitError::NotEnded));

        let mut out = vec![0.0; 3 * 2];
        renderer.render(&mut out);

        assert_eq!(out, vec![0.5, 0.5, 0.5, 0.5, 0.0, 0.0]);
        assert_eq!(voice.try_wait(), Ok(()));
    }

    #[test]
    fn a_stopped_voice_ends() {
        let (control, mut renderer) = create(4, 2, 0);
        let audio = audio(&[0.5; 64]);
        let mut voice = control.play(&audio).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        voice.stop();
        renderer.render(&mut [0.0; 4 * 2]);

        assert_eq!(voice.try_wait(), Ok(()));
    }

    #[test]
    fn stop_ends_only_the_voices_playing_the_audio() {
        let (control, mut renderer) = create(4, 3, 0);
        let stopped = audio(&[0.5; 64]);
        let other = audio(&[0.5; 64]);
        let mut first = control.play(&stopped).unwrap();
        let mut second = control.play(&stopped).unwrap();
        let mut kept = control.play(&other).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        control.stop(&stopped).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);
        control.collect();

        assert_eq!(first.try_wait(), Ok(()));
        assert_eq!(second.try_wait(), Ok(()));
        assert_eq!(kept.try_wait(), Err(TryWaitError::NotEnded));
        assert_eq!(Arc::strong_count(stopped.shared()), 1);
    }

    #[test]
    fn stop_all_ends_every_voice() {
        let (control, mut renderer) = create(4, 2, 0);
        let audio = audio(&[0.5; 64]);
        let mut first = control.play(&audio).unwrap();
        let mut second = control.play(&audio).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        control.stop_all().unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        assert_eq!(first.try_wait(), Ok(()));
        assert_eq!(second.try_wait(), Ok(()));
        assert_eq!(Arc::strong_count(audio.shared()), 1);
    }

    #[test]
    fn collect_frees_the_audio_of_a_dropped_voice() {
        let (control, mut renderer) = create(4, 2, 0);
        let audio = audio(&[0.5; 2]);
        let voice = control.play(&audio).unwrap();
        drop(voice);
        renderer.render(&mut [0.0; 4 * 2]);
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
        let (control, _renderer) = create(0, 1, 0);
        let audio = audio(&[0.5; 2]);
        let _voice = control.play(&audio).unwrap();
        assert!(matches!(control.play(&audio), Err(PlayError::Full)));
    }

    #[test]
    fn stops_get_through_when_plays_fill_the_queue() {
        let (control, _renderer) = create(0, 1, 0);
        let audio = audio(&[0.5; 2]);
        let _voice = control.play(&audio).unwrap();
        assert!(control.stop(&audio).is_ok());
        assert!(control.stop_all().is_ok());
    }

    #[test]
    fn a_recording_runs_until_stopped() {
        let (control, mut renderer) = create(4, 1, 0);
        let mut recording = control.record(room(64)).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);
        assert_eq!(
            status(&mut recording, &mut renderer),
            RecordingStatus::Recording
        );

        recording.stop();
        renderer.render(&mut [0.0; 4 * 2]);

        let (why, _) = recording.try_wait().unwrap();
        assert_eq!(why, RecordingEnd::Stopped);
    }

    #[test]
    fn record_refuses_mono_audio_and_hands_it_back() {
        let (control, _renderer) = create(4, 1, 0);
        let mono = AudioMemory::new().alloc(ChannelLayout::Mono, 4).unwrap();
        let error = control.record(mono).unwrap_err();
        assert!(matches!(error, RecordError::NotStereo(_)));
        assert_eq!(error.into_audio().layout(), ChannelLayout::Mono);
    }

    #[test]
    fn record_refuses_audio_a_voice_holds() {
        let (control, _renderer) = create(4, 1, 0);
        let audio = room(4);
        let _voice = control.play(&audio).unwrap();
        assert!(matches!(
            control.record(audio),
            Err(RecordError::AudioInUse(_))
        ));
    }

    #[test]
    fn record_refuses_while_another_recording_holds_the_recorder() {
        let (control, _renderer) = create(4, 1, 0);
        let _first = control.record(room(64)).unwrap();
        let error = control.record(room(8)).unwrap_err();
        assert!(matches!(error, RecordError::Full(_)));
        assert_eq!(error.into_audio().frames(), 0);
    }

    #[test]
    fn dropping_a_recording_frees_the_recorder() {
        let (control, mut renderer) = create(4, 1, 0);
        let first = control.record(room(64)).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        drop(first);
        renderer.render(&mut [0.0; 4 * 2]);
        let mut second = control.record(room(64)).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        assert_eq!(
            status(&mut second, &mut renderer),
            RecordingStatus::Recording
        );
    }

    #[test]
    fn try_wait_hands_back_what_was_heard() {
        let (control, mut renderer) = create(4, 1, 0);
        let heard = audio(&[0.25; 64]);
        let _voice = control.play(&heard).unwrap();
        let mut recording = control.record(room(64)).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        recording.stop();
        renderer.render(&mut [0.0; 4 * 2]);

        let (_, taken) = recording.try_wait().unwrap();
        assert_eq!(taken.samples(), [0.25; 4 * 2]);
    }

    #[test]
    fn try_wait_is_not_ended_until_the_recording_ends_and_taken_once() {
        let (control, mut renderer) = create(4, 1, 0);
        let mut recording = control.record(room(64)).unwrap();
        assert!(matches!(recording.try_wait(), Err(TryWaitError::NotEnded)));
        renderer.render(&mut [0.0; 4 * 2]);
        assert!(matches!(recording.try_wait(), Err(TryWaitError::NotEnded)));

        recording.stop();
        renderer.render(&mut [0.0; 4 * 2]);

        assert!(recording.try_wait().is_ok());
        assert!(matches!(recording.try_wait(), Err(TryWaitError::Taken)));
    }

    #[test]
    fn a_recording_starts_from_empty() {
        let (control, mut renderer) = create(4, 1, 0);
        let mut used = room(4);
        used.extend([0.9; 4 * 2]);
        let mut recording = control.record(used).unwrap();
        renderer.render(&mut [0.0; 2 * 2]);

        recording.stop();
        renderer.render(&mut [0.0; 2 * 2]);

        let (_, taken) = recording.try_wait().unwrap();
        assert_eq!(taken.samples(), [0.0; 2 * 2]);
    }
}
