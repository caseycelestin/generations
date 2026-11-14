//! Real-time audio and the code that controls it, split by thread. There is
//! no `Engine` type: [`create`] returns the two sides.

mod control;
mod protocol;
mod realtime;

pub use control::{Control, ControlError, RecordError, Recording, Voice};
pub use protocol::recording::{RecordingEnd, RecordingState};
pub use protocol::voice::{VoiceEnd, VoiceState};
pub use realtime::render::Renderer;

use std::num::NonZeroUsize;

use rtrb::RingBuffer;

/// The [`Renderer`] goes to the audio thread; the [`Control`] stays.
/// `voices` is how many play at once, sized by the caller for its hardware.
///
/// # Panics
/// If `command_capacity` or `voices` is zero.
pub fn create(command_capacity: usize, voices: usize) -> (Control, Renderer) {
    assert!(command_capacity > 0, "command_capacity must not be zero");
    let voices = NonZeroUsize::new(voices).expect("voices must not be zero");
    let (to_renderer, from_control) = RingBuffer::new(command_capacity);
    let (to_control, from_renderer) = RingBuffer::new(return_capacity(command_capacity, voices));
    (
        Control::new(to_renderer, from_renderer),
        Renderer::new(from_control, to_control, voices),
    )
}

/// Depth of the ring back, so the renderer never finds it full. Every send
/// collects first, so between collects it holds at most every voice and
/// the one recording the renderer holds, plus one return per queued command.
fn return_capacity(command_capacity: usize, voices: NonZeroUsize) -> usize {
    voices.get() + 1 + command_capacity
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
    fn a_played_voice_is_heard_then_ends_finished() {
        let (control, mut renderer) = create(4, 2);
        let audio = audio(&[0.5; 2]);
        let voice = control.play(&audio).unwrap();
        assert_eq!(voice.state(), VoiceState::Playing);

        let mut out = vec![0.0; 3 * 2];
        renderer.render(&mut out);

        assert_eq!(out, vec![0.5, 0.5, 0.5, 0.5, 0.0, 0.0]);
        assert_eq!(voice.state(), VoiceState::Ended(VoiceEnd::Finished));
    }

    #[test]
    fn a_stopped_voice_ends_stopped() {
        let (control, mut renderer) = create(4, 2);
        let audio = audio(&[0.5; 64]);
        let voice = control.play(&audio).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        voice.stop().unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        assert_eq!(voice.state(), VoiceState::Ended(VoiceEnd::Stopped));
    }

    #[test]
    fn stop_ends_only_the_voices_playing_the_audio() {
        let (control, mut renderer) = create(4, 3);
        let stopped = audio(&[0.5; 64]);
        let other = audio(&[0.5; 64]);
        let first = control.play(&stopped).unwrap();
        let second = control.play(&stopped).unwrap();
        let kept = control.play(&other).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        control.stop(&stopped).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);
        control.collect();

        assert_eq!(first.state(), VoiceState::Ended(VoiceEnd::Stopped));
        assert_eq!(second.state(), VoiceState::Ended(VoiceEnd::Stopped));
        assert_eq!(kept.state(), VoiceState::Playing);
        assert_eq!(Arc::strong_count(stopped.shared()), 1);
    }

    #[test]
    fn stop_all_ends_every_voice_stopped() {
        let (control, mut renderer) = create(4, 2);
        let audio = audio(&[0.5; 64]);
        let first = control.play(&audio).unwrap();
        let second = control.play(&audio).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        control.stop_all().unwrap();
        renderer.render(&mut [0.0; 4 * 2]);
        control.collect();

        assert_eq!(first.state(), VoiceState::Ended(VoiceEnd::Stopped));
        assert_eq!(second.state(), VoiceState::Ended(VoiceEnd::Stopped));
        assert_eq!(Arc::strong_count(audio.shared()), 1);
    }

    #[test]
    fn collect_frees_the_audio_of_an_ended_voice() {
        let (control, mut renderer) = create(4, 2);
        let audio = audio(&[0.5; 2]);
        let _voice = control.play(&audio).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);
        assert_eq!(
            Arc::strong_count(audio.shared()),
            2,
            "handed back, not yet freed"
        );

        control.collect();

        assert_eq!(Arc::strong_count(audio.shared()), 1);
    }

    #[test]
    fn play_reports_a_full_command_ring() {
        let (control, _renderer) = create(1, 1);
        let audio = audio(&[0.5; 2]);
        assert!(control.play(&audio).is_ok());
        assert!(matches!(
            control.play(&audio),
            Err(ControlError::CommandsFull)
        ));
    }

    #[test]
    fn a_recording_runs_until_stopped() {
        let (control, mut renderer) = create(4, 1);
        let recording = control.record(room(64)).unwrap();
        assert_eq!(recording.state(), RecordingState::Pending);
        renderer.render(&mut [0.0; 4 * 2]);
        assert_eq!(recording.state(), RecordingState::Recording);

        recording.stop().unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        assert_eq!(
            recording.state(),
            RecordingState::Ended(RecordingEnd::Stopped)
        );
    }

    #[test]
    fn record_refuses_mono_audio_and_hands_it_back() {
        let (control, _renderer) = create(4, 1);
        let mono = AudioMemory::new().alloc(ChannelLayout::Mono, 4).unwrap();
        let error = control.record(mono).unwrap_err();
        assert!(matches!(error, RecordError::NotStereo(_)));
        assert_eq!(error.into_audio().layout(), ChannelLayout::Mono);
    }

    #[test]
    fn record_refuses_audio_a_voice_holds() {
        let (control, _renderer) = create(4, 1);
        let audio = room(4);
        let _voice = control.play(&audio).unwrap();
        assert!(matches!(
            control.record(audio),
            Err(RecordError::AudioInUse(_))
        ));
    }

    #[test]
    fn record_hands_the_audio_back_when_the_ring_is_full() {
        let (control, _renderer) = create(1, 1);
        control.stop_all().unwrap();
        assert!(matches!(
            control.record(room(4)),
            Err(RecordError::CommandsFull(_))
        ));
    }

    #[test]
    fn dropping_a_recording_frees_the_recorder() {
        let (control, mut renderer) = create(4, 1);
        let first = control.record(room(64)).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        drop(first);
        let second = control.record(room(64)).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        assert_eq!(second.state(), RecordingState::Recording);
    }

    #[test]
    fn take_hands_back_what_was_heard() {
        let (control, mut renderer) = create(4, 1);
        let heard = audio(&[0.25; 64]);
        let _voice = control.play(&heard).unwrap();
        let mut recording = control.record(room(64)).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        recording.stop().unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        let taken = recording.take().unwrap();
        assert_eq!(taken.samples(), [0.25; 4 * 2]);
    }

    #[test]
    fn take_is_none_until_the_recording_ends_and_once_taken() {
        let (control, mut renderer) = create(4, 1);
        let mut recording = control.record(room(64)).unwrap();
        assert!(recording.take().is_none());
        renderer.render(&mut [0.0; 4 * 2]);
        assert!(recording.take().is_none());

        recording.stop().unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        assert!(recording.take().is_some());
        assert!(recording.take().is_none());
    }

    #[test]
    fn a_recording_starts_from_empty() {
        let (control, mut renderer) = create(4, 1);
        let mut used = room(4);
        used.extend([0.9; 4 * 2]);
        let mut recording = control.record(used).unwrap();
        renderer.render(&mut [0.0; 2 * 2]);

        recording.stop().unwrap();
        renderer.render(&mut [0.0; 2 * 2]);

        assert_eq!(recording.take().unwrap().samples(), [0.0; 2 * 2]);
    }

    #[test]
    fn a_refused_recording_hands_its_audio_back() {
        let (control, mut renderer) = create(4, 1);
        let _first = control.record(room(64)).unwrap();
        let mut second = control.record(room(8)).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        assert_eq!(second.state(), RecordingState::Refused);
        assert_eq!(second.take().unwrap().frames(), 0);
    }
}
