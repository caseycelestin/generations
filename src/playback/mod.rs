//! Playback: the audio a player can hear, and how it is heard.
//!
//! Two threads, one rule. The control thread, where the engine runs, loads
//! audio into RAM and asks for sounds to start and stop. The real-time
//! thread, owned by the audio device, turns what is resident into output,
//! one period at a time, and must never wait: no allocation, no lock, no
//! disk, no error path. Everything here is on one side of that line or the
//! other.
//!
//! ```text
//! control thread                     real-time thread
//! ResidentSet   audio in RAM         Voice    one sample playing
//!               under a budget       Mixer    the voices summed into a period
//! ```
//!
//! Audio crosses the line as `Arc<AudioBuffer>`: the resident set holds
//! one reference, a playing voice holds a clone, and freeing happens only
//! where the control thread lets go of the last one.

pub mod resident;
pub mod voice;

pub use resident::{PlaybackError, ResidentSet};
pub use voice::Voice;
