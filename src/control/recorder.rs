//! A recorder the caller holds: a buffer paid for once, at one tap, and
//! the takes made with it.
//!
//! [`Control::recorder`] returns a [`Recorder`]: the control side's
//! handle, holding the buffer takes are recorded into. The render side
//! builds its own recorder around the buffer for each take; one concept,
//! one name, and the module says the side. The [`Tap`] says where in the
//! signal path it records.
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

use crate::audio::AudioBuffer;
use crate::control::{Control, ControlError};
use crate::render::command::{TakeId, Tap};

/// A recorder at one tap, and the takes it makes. Made by
/// [`Control::recorder`](crate::Control::recorder); only one recorder
/// is held per tap at a time.
///
/// Dropping it frees the tap. A take still running is stopped, and its
/// buffer is freed on the control side when the renderer lets it go.
#[derive(Debug)]
pub struct Recorder {
    control: Control,
    tap: Tap,
    /// The buffer, while it is here: before the first take, and after
    /// each with the take in it. `None` while the renderer has it.
    audio: Option<AudioBuffer>,
    /// The take running, while the renderer has the buffer.
    take: Option<TakeId>,
}

impl Recorder {
    /// A recorder at `tap` recording into `audio`, no take running.
    pub(crate) fn new(control: Control, tap: Tap, audio: AudioBuffer) -> Self {
        Self {
            control,
            tap,
            audio: Some(audio),
            take: None,
        }
    }

    /// Starts a take from the renderer's next period, over whatever the
    /// last take kept: the buffer moves to the renderer until the take
    /// ends. Refused as `Recording` while a take is running, and as
    /// `CommandsFull`, with nothing started, if the ring has no room.
    pub fn record(&mut self) -> Result<(), ControlError> {
        if self.is_recording() {
            return Err(ControlError::Recording);
        }
        let audio = self.audio.take().expect("here when no take runs");
        match self.control.start_recording(self.tap, audio) {
            Ok(id) => {
                self.take = Some(id);
                Ok(())
            }
            Err((error, audio)) => {
                self.audio = Some(audio);
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
        self.audio = Some(audio);
        self.take = None;
        false
    }
}

impl Drop for Recorder {
    /// Frees the tap, and stops a take still running; its buffer is
    /// dropped on the control side when it comes back. Best effort: if
    /// the ring has no room for the stop, the take runs until the buffer
    /// is full, and is dropped then.
    fn drop(&mut self) {
        if let Some(id) = self.take {
            self.control.abandon_take(id);
        }
        self.control.free_tap(self.tap);
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use crate::config::Pcm;
    use crate::memory::Ledger;
    use crate::render::{Renderer, RendererSettings};
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
        let mut rec = control.recorder(Tap::MainOut, 4).unwrap();
        rec.record().unwrap();
        period(&mut r, 2);
        assert!(rec.is_recording());
        period(&mut r, 2);
        assert!(!rec.is_recording());
        assert_eq!(rec.audio.as_ref().unwrap().samples(), &[0.0; 8]);
    }

    #[test]
    fn a_stopped_take_keeps_what_it_had() {
        let (mut r, control) = Renderer::new(SETTINGS, Ledger::new(usize::MAX));
        let mut rec = control.recorder(Tap::MainOut, 4).unwrap();
        rec.record().unwrap();
        period(&mut r, 1);
        rec.stop().unwrap();
        period(&mut r, 1);
        assert!(!rec.is_recording());
        assert_eq!(rec.audio.as_ref().unwrap().samples(), &[0.0; 2]);
    }

    #[test]
    fn one_recorder_makes_many_takes_without_paying_again() {
        let ledger = Ledger::new(usize::MAX);
        let (mut r, control) = Renderer::new(SETTINGS, ledger.clone());
        let mut rec = control.recorder(Tap::MainOut, 1).unwrap();
        for _ in 0..3 {
            rec.record().unwrap();
            period(&mut r, 1);
            assert!(!rec.is_recording());
            assert_eq!(rec.audio.as_ref().unwrap().samples().len(), 2);
            assert_eq!(ledger.in_use(), bytes(1));
        }
    }

    #[test]
    fn a_take_cannot_be_recorded_while_one_runs() {
        let (_r, control) = Renderer::new(SETTINGS, Ledger::new(usize::MAX));
        let mut rec = control.recorder(Tap::MainOut, 4).unwrap();
        rec.record().unwrap();
        assert!(matches!(rec.record(), Err(ControlError::Recording)));
    }

    #[test]
    fn a_tap_holds_one_recorder_until_it_is_dropped() {
        let (_r, control) = Renderer::new(SETTINGS, Ledger::new(usize::MAX));
        let rec = control.recorder(Tap::MainOut, 4).unwrap();
        assert!(matches!(
            control.recorder(Tap::MainOut, 4),
            Err(ControlError::TapInUse)
        ));
        drop(rec);
        assert!(control.recorder(Tap::MainOut, 4).is_ok());
    }

    #[test]
    fn a_recorder_over_budget_is_refused_and_leaves_the_tap_free() {
        let ledger = Ledger::new(bytes(4) - 1);
        let (_r, control) = Renderer::new(SETTINGS, ledger.clone());
        assert!(matches!(
            control.recorder(Tap::MainOut, 4),
            Err(ControlError::OverBudget(_))
        ));
        assert_eq!(ledger.in_use(), 0);
        assert!(control.recorder(Tap::MainOut, 1).is_ok());
    }

    #[test]
    fn a_recorder_dropped_mid_take_is_freed_when_it_comes_back() {
        let ledger = Ledger::new(usize::MAX);
        let (mut r, control) = Renderer::new(SETTINGS, ledger.clone());
        let mut rec = control.recorder(Tap::MainOut, 4).unwrap();
        rec.record().unwrap();
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
        let rec = control.recorder(Tap::MainOut, 4).unwrap();
        drop(rec);
        assert_eq!(ledger.in_use(), 0);
    }
}
