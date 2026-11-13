//! Real-time audio and the code that controls it, split by thread. There is
//! no `Engine` type: [`create`] returns the two sides.

mod control;
mod protocol;
mod realtime;

pub use control::{Control, ControlError, RecordError, Recording, Voice, VoiceState};
pub use protocol::recording::{RecordingEnd, RecordingState};
pub use protocol::voice::VoiceEnd;
pub use realtime::render::Renderer;

use std::num::NonZeroUsize;

use rtrb::RingBuffer;

/// The [`Renderer`] goes to the audio thread; the [`Control`] stays.
/// `voices` is how many play at once, sized by the caller for its hardware.
pub fn create(command_capacity: NonZeroUsize, voices: NonZeroUsize) -> (Control, Renderer) {
    let (to_renderer, from_control) = RingBuffer::new(command_capacity.get());
    let (to_control, from_renderer) = RingBuffer::new(return_capacity(command_capacity, voices));
    (
        Control::new(to_renderer, from_renderer),
        Renderer::new(from_control, to_control, voices),
    )
}

/// Depth of the ring back, so the renderer never finds it full. Every send
/// collects first, so between collects it holds at most every voice and
/// the one recording the renderer holds, plus one return per queued command.
fn return_capacity(command_capacity: NonZeroUsize, voices: NonZeroUsize) -> usize {
    voices.get() + 1 + command_capacity.get()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::audio::{Audio, AudioMemory};
    use crate::layout::ChannelLayout;
    use crate::testing::buffer;

    fn count(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap()
    }

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
        let (control, mut renderer) = create(count(4), count(2));
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
        let (control, mut renderer) = create(count(4), count(2));
        let audio = audio(&[0.5; 64]);
        let voice = control.play(&audio).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        voice.stop().unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        assert_eq!(voice.state(), VoiceState::Ended(VoiceEnd::Stopped));
    }

    #[test]
    fn stop_ends_only_the_voices_playing_the_audio() {
        let (control, mut renderer) = create(count(4), count(3));
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
        let (control, mut renderer) = create(count(4), count(2));
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
        let (control, mut renderer) = create(count(4), count(2));
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
        let (control, _renderer) = create(count(1), count(1));
        let audio = audio(&[0.5; 2]);
        assert!(control.play(&audio).is_ok());
        assert!(matches!(
            control.play(&audio),
            Err(ControlError::CommandsFull)
        ));
    }

    #[test]
    fn a_recording_runs_until_stopped() {
        let (control, mut renderer) = create(count(4), count(1));
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
        let (control, _renderer) = create(count(4), count(1));
        let mono = AudioMemory::new().alloc(ChannelLayout::Mono, 4).unwrap();
        let error = control.record(mono).unwrap_err();
        assert!(matches!(error, RecordError::NotStereo(_)));
        assert_eq!(error.into_audio().layout(), ChannelLayout::Mono);
    }

    #[test]
    fn record_refuses_audio_a_voice_holds() {
        let (control, _renderer) = create(count(4), count(1));
        let audio = room(4);
        let _voice = control.play(&audio).unwrap();
        assert!(matches!(
            control.record(audio),
            Err(RecordError::AudioInUse(_))
        ));
    }

    #[test]
    fn record_hands_the_audio_back_when_the_ring_is_full() {
        let (control, _renderer) = create(count(1), count(1));
        control.stop_all().unwrap();
        assert!(matches!(
            control.record(room(4)),
            Err(RecordError::CommandsFull(_))
        ));
    }

    #[test]
    fn dropping_a_recording_frees_the_recorder() {
        let (control, mut renderer) = create(count(4), count(1));
        let first = control.record(room(64)).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        drop(first);
        let second = control.record(room(64)).unwrap();
        renderer.render(&mut [0.0; 4 * 2]);

        assert_eq!(second.state(), RecordingState::Recording);
    }
}
