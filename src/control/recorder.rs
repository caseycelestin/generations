//! The recorder the caller holds: a buffer paid for once, and the takes
//! made with it.
//!
//! [`Control::recorder`] returns a [`Recorder`]: the control side's
//! handle, holding the buffer takes are recorded into. The render side
//! builds its own recorder around the buffer for each take; one concept,
//! one name, and the module says the side. Each take says where in the
//! signal path it records, by its [`Tap`].
//!
//! There is one recorder at a time, like a hardware sampler's one
//! recording path: a take records one tap, chosen when it starts, and
//! the next take may record another without paying again.
//!
//! One recorder makes many takes. The buffer is paid for when the
//! recorder is made, so starting a take allocates nothing and costs the
//! same every time. A take moves the buffer to the renderer, which writes
//! into it, and it comes back with the take in it when the take is
//! stopped or the buffer is full; both are normal ends. The next take
//! records over it. The audio thread cannot call anyone, so the handle
//! finds out by being asked: [`is_recording`](Recorder::is_recording)
//! checks whether the buffer is back.
//!
//! [`Control::recorder`]: crate::Control::recorder

use crate::audio::Take;
use crate::control::{Control, ControlError};
use crate::render::command::{TakeId, Tap};

/// The recorder, and the takes it makes. Made by
/// [`Control::recorder`](crate::Control::recorder); only one is held at
/// a time.
///
/// Dropping it lets another be made. A take still running is stopped, and its
/// buffer is freed on the control side when the renderer lets it go.
#[derive(Debug)]
pub struct Recorder {
    control: Control,
    /// The buffer, while it is here: empty before the first take, and
    /// after each holding it. `None` while the renderer has it.
    buffer: Option<Take>,
    /// The take running, while the renderer has the buffer.
    take: Option<TakeId>,
}

impl Recorder {
    /// A recorder recording into `buffer`, no take running.
    pub(crate) fn new(control: Control, buffer: Take) -> Self {
        Self {
            control,
            buffer: Some(buffer),
            take: None,
        }
    }

    /// Starts a take of what passes `tap` from the renderer's next
    /// period, over whatever the last take kept: the buffer moves to the
    /// renderer until the take ends. Refused as `Recording` while a take is running, and as
    /// `CommandsFull`, with nothing started, if the ring has no room.
    pub fn record(&mut self, tap: Tap) -> Result<(), ControlError> {
        if self.is_recording() {
            return Err(ControlError::Recording);
        }
        let buffer = self.buffer.take().expect("here when no take runs");
        match self.control.start_recording(tap, buffer.into_audio()) {
            Ok(id) => {
                self.take = Some(id);
                Ok(())
            }
            Err((error, audio)) => {
                self.buffer = Some(Take::new(audio));
                Err(error)
            }
        }
    }

    /// Stops the take at the renderer's next period, keeping what it has
    /// so far. With no take running it does nothing, so stopping twice is
    /// harmless. Refused as `CommandsFull` if the ring has no room.
    pub fn stop(&self) -> Result<(), ControlError> {
        match self.take {
            Some(id) => self.control.stop_recording(id),
            None => Ok(()),
        }
    }

    /// Whether a take is running: started and not yet back, stopped or
    /// full. Asking is how the end of a take is found: once it is back,
    /// the buffer is here again with the take in it.
    pub fn is_recording(&mut self) -> bool {
        let Some(id) = self.take else {
            return false;
        };
        let Some(audio) = self.control.take_back(id) else {
            return true;
        };
        self.buffer = Some(Take::new(audio));
        self.take = None;
        false
    }

    /// The last take, once it is back: `None` while a take is running,
    /// before the first, or if the last kept no frames. Asking checks
    /// whether it is back, as [`is_recording`](Self::is_recording) does.
    /// It lasts until the next [`record`](Self::record), which records
    /// over it; save it with
    /// [`Project::save_take`](crate::Project::save_take) to keep it.
    pub fn last_take(&mut self) -> Option<&Take> {
        if self.is_recording() {
            return None;
        }
        self.buffer.as_ref().filter(|t| t.audio().frames() > 0)
    }
}

impl Drop for Recorder {
    /// Lets another recorder be made, and stops a take still running; its buffer is
    /// dropped on the control side when it comes back. Best effort: if
    /// the ring has no room for the stop, the take runs until the buffer
    /// is full, and is dropped then.
    fn drop(&mut self) {
        if let Some(id) = self.take {
            self.control.abandon_take(id);
        }
        self.control.free_recorder();
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use crate::audio::ChannelLayout;
    use crate::config::Pcm;
    use crate::project::ledger::Ledger;
    use crate::project::tests::memory_project;
    use crate::render::{Renderer, RendererSettings};
    use crate::testing::audio;
    use crate::{ControlError, Tap};

    const SETTINGS: RendererSettings = RendererSettings::new(64);

    /// Renders `frames` frames of silence.
    fn period(r: &mut Renderer, frames: usize) {
        r.render(&mut vec![0.0; frames * Renderer::CHANNELS]);
    }

    /// What a recorder of `frames` frames costs the budget.
    fn bytes(frames: usize) -> usize {
        frames * Renderer::CHANNELS * size_of::<Pcm>()
    }

    #[test]
    fn a_take_runs_until_the_buffer_is_full() {
        let (mut r, control) = Renderer::new(SETTINGS, Ledger::new(usize::MAX));
        let mut rec = control.recorder(4).unwrap();
        rec.record(Tap::MainOut).unwrap();
        period(&mut r, 2);
        assert!(rec.is_recording());
        period(&mut r, 2);
        assert!(!rec.is_recording());
        assert_eq!(rec.last_take().unwrap().audio().samples(), &[0.0; 8]);
    }

    #[test]
    fn a_stopped_take_keeps_what_it_had() {
        let (mut r, control) = Renderer::new(SETTINGS, Ledger::new(usize::MAX));
        let mut rec = control.recorder(4).unwrap();
        rec.record(Tap::MainOut).unwrap();
        period(&mut r, 1);
        rec.stop().unwrap();
        period(&mut r, 1);
        assert!(!rec.is_recording());
        assert_eq!(rec.last_take().unwrap().audio().samples(), &[0.0; 2]);
    }

    #[test]
    fn one_recorder_makes_many_takes_without_paying_again() {
        let ledger = Ledger::new(usize::MAX);
        let (mut r, control) = Renderer::new(SETTINGS, ledger.clone());
        let mut rec = control.recorder(1).unwrap();
        for _ in 0..3 {
            rec.record(Tap::MainOut).unwrap();
            period(&mut r, 1);
            assert!(!rec.is_recording());
            assert_eq!(rec.last_take().unwrap().audio().samples().len(), 2);
            assert_eq!(ledger.in_use(), bytes(1));
        }
    }

    #[test]
    fn a_take_cannot_be_recorded_while_one_runs() {
        let (_r, control) = Renderer::new(SETTINGS, Ledger::new(usize::MAX));
        let mut rec = control.recorder(4).unwrap();
        rec.record(Tap::MainOut).unwrap();
        assert!(matches!(
            rec.record(Tap::MainOut),
            Err(ControlError::Recording)
        ));
    }

    #[test]
    fn there_is_one_recorder_until_it_is_dropped() {
        let (_r, control) = Renderer::new(SETTINGS, Ledger::new(usize::MAX));
        let rec = control.recorder(4).unwrap();
        assert!(matches!(
            control.recorder(4),
            Err(ControlError::RecorderInUse)
        ));
        drop(rec);
        assert!(control.recorder(4).is_ok());
    }

    #[test]
    fn a_recorder_over_budget_is_refused_and_another_can_be_made() {
        let ledger = Ledger::new(bytes(4) - 1);
        let (_r, control) = Renderer::new(SETTINGS, ledger.clone());
        assert!(matches!(
            control.recorder(4),
            Err(ControlError::OverBudget(_))
        ));
        assert_eq!(ledger.in_use(), 0);
        assert!(control.recorder(1).is_ok());
    }

    #[test]
    fn a_recorder_dropped_mid_take_is_freed_when_it_comes_back() {
        let ledger = Ledger::new(usize::MAX);
        let (mut r, control) = Renderer::new(SETTINGS, ledger.clone());
        let mut rec = control.recorder(4).unwrap();
        rec.record(Tap::MainOut).unwrap();
        drop(rec);
        assert_eq!(ledger.in_use(), bytes(4), "the renderer still has it");
        period(&mut r, 1);
        control.collect();
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn a_recorder_dropped_between_takes_is_freed_at_once() {
        let ledger = Ledger::new(usize::MAX);
        let (_r, control) = Renderer::new(SETTINGS, ledger.clone());
        let rec = control.recorder(4).unwrap();
        drop(rec);
        assert_eq!(ledger.in_use(), 0);
    }

    #[test]
    fn there_is_no_last_take_before_the_first_or_while_one_runs() {
        let (mut r, control) = Renderer::new(SETTINGS, Ledger::new(usize::MAX));
        let mut rec = control.recorder(4).unwrap();
        assert!(rec.last_take().is_none(), "before the first");
        rec.record(Tap::MainOut).unwrap();
        assert!(rec.last_take().is_none(), "sent, not yet rendered");
        period(&mut r, 1);
        assert!(rec.last_take().is_none(), "still recording");
        rec.stop().unwrap();
        period(&mut r, 1);
        assert_eq!(rec.last_take().unwrap().audio().frames(), 1);
    }

    #[test]
    fn a_take_stopped_before_it_kept_a_frame_is_no_take() {
        let (mut r, control) = Renderer::new(SETTINGS, Ledger::new(usize::MAX));
        let mut rec = control.recorder(4).unwrap();
        rec.record(Tap::MainOut).unwrap();
        rec.stop().unwrap();
        period(&mut r, 1);
        assert!(!rec.is_recording());
        assert!(rec.last_take().is_none());
    }

    #[test]
    fn a_saved_take_loads_as_a_sample_of_what_was_heard() {
        let mut project = memory_project();
        let id = project
            .add_sample(&audio(ChannelLayout::Mono, &[0.1, 0.2, 0.3]))
            .unwrap();
        let sample = project.load(id).unwrap();
        let (mut r, control) = Renderer::new(SETTINGS, Ledger::new(usize::MAX));
        let mut rec = control.recorder(8).unwrap();
        rec.record(Tap::MainOut).unwrap();
        control.play(&sample).unwrap();
        period(&mut r, 2);
        rec.stop().unwrap();
        period(&mut r, 1);

        let take = rec.last_take().expect("stopped, so back");
        let saved = project.save_take(take).unwrap();
        let loaded = project.load(saved).unwrap();
        assert_eq!(loaded.audio().samples(), &[0.1, 0.1, 0.2, 0.2]);
        assert_eq!(loaded.audio().layout(), ChannelLayout::Stereo);
    }
}
