//! The control half of playback: what the engine holds.
//!
//! A [`Playback`] owns everything playback keeps on the control side: the
//! audio resident in RAM, the sending end of the command ring, and the
//! voice ids it mints. The engine holds one and calls it; it never sees a
//! ring or a voice counter. Nothing here knows what a project or a device
//! is: `new` hands the renderer back and the caller decides what drives it.

use rtrb::{Producer, RingBuffer};

use crate::audio::AudioBuffer;
use crate::config::COMMAND_CAPACITY;
use crate::playback::{Command, PlaybackError, Renderer, ResidentSet, VoiceId};
use crate::project::SampleId;

/// The control half of playback.
pub struct Playback {
    resident: ResidentSet,
    /// The sending end of the command ring; the renderer has the other.
    commands: Producer<Command>,
    /// The next voice id to mint. Only goes up, and this is the one
    /// sender, so an id is never reused.
    next_voice: u64,
}

impl Playback {
    /// Playback with `budget` bytes of RAM for resident audio, and the
    /// renderer it commands. The caller decides what drives the renderer.
    pub fn new(budget: usize) -> (Self, Renderer) {
        let (producer, consumer) = RingBuffer::new(COMMAND_CAPACITY);
        let playback = Self {
            resident: ResidentSet::new(budget),
            commands: producer,
            next_voice: 0,
        };
        (playback, Renderer::new(consumer))
    }

    /// Makes `audio` resident as sample `id`, so it can be played. Refused
    /// if it would exceed the budget; nothing changes then.
    pub fn load(&mut self, id: SampleId, audio: AudioBuffer) -> Result<(), PlaybackError> {
        self.resident.insert(id, audio)
    }

    /// Drops sample `id` from RAM. A voice still playing it keeps the audio
    /// alive until it finishes.
    pub fn unload(&mut self, id: SampleId) {
        self.resident.remove(id);
    }

    /// Starts playing resident sample `id` from its first frame and
    /// returns the voice, so it can be stopped. Sound starts at the
    /// renderer's next period.
    pub fn play(&mut self, id: SampleId) -> Result<VoiceId, PlaybackError> {
        let audio = self.resident.get(id).ok_or(PlaybackError::NotLoaded(id))?;
        let voice = VoiceId(self.next_voice);
        self.next_voice += 1;
        self.send(Command::Play { id: voice, audio })?;
        Ok(voice)
    }

    /// Stops `voice` if it is still playing. A voice that has finished or
    /// been stolen is left alone.
    pub fn stop(&mut self, voice: VoiceId) -> Result<(), PlaybackError> {
        self.send(Command::Stop(voice))
    }

    /// Stops every voice.
    pub fn stop_all(&mut self) -> Result<(), PlaybackError> {
        self.send(Command::StopAll)
    }

    /// The audio resident in RAM. For the engine, which checks residency
    /// before reading from disk, and for tests.
    pub fn resident(&self) -> &ResidentSet {
        &self.resident
    }

    fn send(&mut self, command: Command) -> Result<(), PlaybackError> {
        self.commands
            .push(command)
            .map_err(|_| PlaybackError::CommandsFull)
    }
}

impl std::fmt::Debug for Playback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Playback")
            .field("resident", &self.resident)
            .field("next_voice", &self.next_voice)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone() -> AudioBuffer {
        AudioBuffer::new(1, vec![16_384; 4]).unwrap()
    }

    #[test]
    fn play_of_an_unloaded_sample_is_refused() {
        let (mut p, mut r) = Playback::new(usize::MAX);
        assert_eq!(
            p.play(SampleId(1)),
            Err(PlaybackError::NotLoaded(SampleId(1)))
        );
        let mut out = vec![0.0; 4];
        r.render(&mut out);
        assert_eq!(r.playing(), 0);
    }

    #[test]
    fn each_play_mints_a_new_voice() {
        let (mut p, mut r) = Playback::new(usize::MAX);
        p.load(SampleId(1), tone()).unwrap();
        let a = p.play(SampleId(1)).unwrap();
        let b = p.play(SampleId(1)).unwrap();
        assert_ne!(a, b);
        let mut out = vec![0.0; 2];
        r.render(&mut out);
        assert!(r.is_playing(a));
        assert!(r.is_playing(b));
        assert_eq!(out, vec![1.0, 1.0]);
    }

    #[test]
    fn a_full_ring_refuses_the_command() {
        let (mut p, _r) = Playback::new(usize::MAX);
        p.load(SampleId(1), tone()).unwrap();
        for _ in 0..COMMAND_CAPACITY {
            p.play(SampleId(1)).unwrap();
        }
        assert_eq!(p.play(SampleId(1)), Err(PlaybackError::CommandsFull));
        assert_eq!(p.stop_all(), Err(PlaybackError::CommandsFull));
    }

    #[test]
    fn unload_keeps_a_playing_voice_alive() {
        let (mut p, mut r) = Playback::new(usize::MAX);
        p.load(SampleId(1), tone()).unwrap();
        p.play(SampleId(1)).unwrap();
        let mut out = vec![0.0; 2];
        r.render(&mut out);
        p.unload(SampleId(1));
        assert!(!p.resident().contains(SampleId(1)));
        r.render(&mut out);
        assert_eq!(out, vec![0.5, 0.5]);
    }
}
