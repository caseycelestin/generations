//! The voices playing, as one source.
//!
//! [`VoiceTable`] is the table of every [`Voice`] playing right now and the
//! policies of that table: a play starts a voice and steals the oldest
//! when every slot is taken, a voice that reaches its end leaves, a stop
//! or an eviction removes. Summed, it is one source into the mix: every
//! voice adds itself to the same slice through `mix_into`.
//!
//! Real-time safe: the table has fixed capacity and lives inline, so
//! nothing here allocates, locks or fails.

use std::sync::Arc;

use heapless::Deque;

use crate::audio::AudioBuffer;
use crate::config::VOICE_COUNT;
use crate::dsp::command::{BufferAddr, VoiceId};
use crate::dsp::voice::Voice;

/// The voices playing, oldest first. Fixed capacity, stored inline: the
/// work in one period is bounded and nothing here can allocate.
#[derive(Debug, Default)]
pub struct VoiceTable {
    playing: Deque<Voice, VOICE_COUNT>,
}

impl VoiceTable {
    /// No voices playing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a voice, stealing the oldest when every voice is in use.
    pub fn play(&mut self, id: VoiceId, audio: Arc<AudioBuffer>) {
        if self.playing.is_full() {
            self.playing.pop_front();
        }
        // Cannot fail: there is room, either already or because of the pop.
        let _ = self.playing.push_back(Voice::new(id, audio));
    }

    /// Stops voice `id`. A voice that is gone is left gone.
    pub fn stop(&mut self, id: VoiceId) {
        self.playing.retain(|v| v.id() != id);
    }

    /// Stops every voice.
    pub fn stop_all(&mut self) {
        self.playing.clear();
    }

    /// Stops every voice playing the buffer at `addr`, dropping their
    /// references to it. One pass over at most `VOICE_COUNT` voices; a
    /// buffer nothing plays costs the pass and changes nothing.
    pub fn evict(&mut self, addr: BufferAddr) {
        self.playing.retain(|v| !v.plays(addr));
    }

    /// Adds every playing voice's next frames into `out`, an interleaved
    /// stereo slice. A voice that reaches its end frees its slot. Nothing
    /// is clipped here.
    pub fn mix_into(&mut self, out: &mut [f32]) {
        self.playing.retain_mut(|voice| !voice.mix_into(out));
    }

    /// Whether voice `id` is playing. A front panel will want this; until
    /// then only tests ask.
    #[cfg(test)]
    pub(crate) fn is_playing(&self, id: VoiceId) -> bool {
        self.playing.iter().any(|v| v.id() == id)
    }

    /// How many voices are playing.
    #[cfg(test)]
    pub(crate) fn playing(&self) -> usize {
        self.playing.len()
    }
}
