//! Real-time audio and the code that controls it, split by the thread it
//! runs on. What both sides share, the protocol, lives here. There is no
//! `Engine` type.

mod control;
mod protocol;
mod realtime;

pub use control::{Control, ControlError, Recording, Voice, VoiceState};
pub use protocol::recording::{RecordingEnd, RecordingState};
pub use protocol::voice::VoiceEnd;
pub use realtime::render::Renderer;

use std::num::NonZeroUsize;

use rtrb::RingBuffer;

/// Makes both rings and hands each side its ends. The [`Renderer`] goes
/// to the audio thread; the [`Control`] stays. `voices` is how many can
/// play at once: the caller's call, for its hardware.
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
    use crate::audio::Audio;
    use crate::layout::ChannelLayout;
    use crate::testing::buffer;

    fn count(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap()
    }

    fn audio(samples: &[f32]) -> Audio {
        Audio::new(buffer(ChannelLayout::Mono, samples))
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
}
