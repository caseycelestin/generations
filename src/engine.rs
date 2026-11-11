//! The control side of sound, for now under the engine's name: the ring
//! to the renderer and the verbs that send on it.
//!
//! The project side has moved out: a [`Project`](crate::project::Project)
//! opens itself, imports and loads, and hands out a [`LoadedSample`] that
//! names no renderer. What is left here is the renderer's control side:
//! `play` a loaded sample, add an `input`, `unload`, `stop_all`, and the
//! renderer itself, taken once. The engine owns nothing but the handle
//! on the renderer and the renderer until it is taken; every verb here
//! is one the handle could carry, and the next step moves them there and
//! retires this type. The two sides share nothing but the ledger and the
//! loaded samples the caller carries from one to the other.

use rtrb::RingBuffer;
use thiserror::Error;

use crate::config::{COMMAND_CAPACITY, MAX_INPUT_CHANNELS, MAX_INPUT_SOUNDS, MAX_PERIOD_FRAMES};
use crate::dsp::SoundId;
use crate::input::{self, Channels, Input, InputFeed};
use crate::loaded_sample::LoadedSample;
use crate::renderer::Renderer;
use crate::renderer_handle::RendererHandle;
use crate::sound::Sound;

/// Reasons the renderer can refuse what it is asked.
#[derive(Debug, Error)]
pub enum EngineError {
    /// The command ring was full; the command was dropped.
    #[error("the output has not taken the last {COMMAND_CAPACITY} commands")]
    CommandsFull,
    /// The layout asked of `Engine::input` adds up to no channels, or
    /// more than the engine takes from one device.
    #[error("an input is 1 to {MAX_INPUT_CHANNELS} channels wide, not {0}")]
    BadInputWidth(usize),
    /// The capture period given to `Engine::input` is zero or longer
    /// than the engine takes.
    #[error("a capture period is 1 to {MAX_PERIOD_FRAMES} frames, not {0}")]
    BadInputPeriod(usize),
    /// The input's sources would not all fit beside the input sounds the
    /// engine already holds. Drop an input before adding another.
    #[error("the engine holds {MAX_INPUT_SOUNDS} input sounds at most")]
    InputsFull,
}

/// The control side of one renderer.
pub struct Engine {
    /// The control side's handle on the renderer: the sending end of the
    /// command ring, the sound id counter and the input slot count.
    /// Shared: every [`Sound`] and [`Input`] the engine hands out holds a
    /// clone and acts through it.
    renderer: RendererHandle,
    /// The render side, until the caller takes it with [`Engine::output`].
    output: Option<Renderer>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// A silent renderer and its control side. No audio device is
    /// involved: the engine works the same on a machine without one. To
    /// hear it, take [`Engine::output`] and drive it.
    pub fn new() -> Self {
        let (producer, consumer) = RingBuffer::new(COMMAND_CAPACITY);
        Self {
            renderer: RendererHandle::new(producer),
            output: Some(Renderer::new(consumer)),
        }
    }

    /// The engine's output: its render side, to be driven by whoever owns
    /// the audio thread, moved into a device callback for live sound or
    /// called in a loop offline. There is one; the second call returns
    /// `None`.
    ///
    /// Commands sent before this is taken are not lost. They wait in the
    /// ring and apply on the first render.
    pub fn output(&mut self) -> Option<Renderer> {
        self.output.take()
    }

    /// Starts playing `sample` from its first frame and returns the sound,
    /// which is how it is stopped: [`Sound::stop`]. The sound shares the
    /// sample's audio; nothing is copied or read from disk, so a press
    /// costs the same every time. Sound starts at the output's next
    /// render. A play never fails for want of a free slot: the renderer
    /// steals the oldest. Refused as `CommandsFull` if the ring has no
    /// room.
    pub fn play(&self, sample: &LoadedSample) -> Result<Sound, EngineError> {
        let id = self.renderer.play(sample.audio().clone())?;
        Ok(Sound::new(id, self.renderer.clone()))
    }

    /// Unloads `sample` now: stops every sound playing it, hard, at the
    /// output's next render, and lets the audio go, which repays the
    /// budget as soon as the last of those sounds has.
    ///
    /// Dropping the sample instead lets its sounds play out; this is the
    /// drop that cuts them. Refused as `CommandsFull` with the sample
    /// handed back and nothing changed if the ring has no room for the
    /// eviction. The eviction goes out before the audio is let go, which
    /// is what makes the address a safe key; see `BufferAddr`.
    pub fn unload(&self, sample: LoadedSample) -> Result<(), (LoadedSample, EngineError)> {
        match self.renderer.evict(sample.addr()) {
            Ok(()) => Ok(()),
            Err(e) => Err((sample, e)),
        }
    }

    /// Adds an input: the ability to put audio into the renderer. Mints a
    /// ring per source, sends the reading ends to the render side, and
    /// returns the handle and the feed. The [`Input`] is yours to hold:
    /// its [`sources`](Input::sources) are what you monitor, and dropping
    /// it removes the input. The [`InputFeed`] goes to whoever owns the
    /// capture thread, to feed from a device's input callback. Nothing is
    /// heard until a source is monitored.
    ///
    /// An input is shaped like the device it comes from. `layout` says
    /// what each channel of the device's interleaved frames means, in
    /// order: a `Stereo` is the next two channels heard left and right, a
    /// `Mono` is the next one heard on both sides. The frames fed are as
    /// wide as the layout adds up to, 1 to `MAX_INPUT_CHANNELS`, exactly
    /// as the device delivered them. Each source is a sound of its own,
    /// its own ring split out of the device frames by [`InputFeed::feed`],
    /// so the render side sees mono and stereo sounds and nothing wider.
    /// The layout is fixed for the input's life: a device that is to
    /// deliver other channels is closed and reopened, and so is its input.
    /// Up to `MAX_INPUT_SOUNDS` sources at once across every input; an
    /// input whose sources would not all fit is refused with `InputsFull`
    /// until one is dropped, so an input already heard is never cut by
    /// another being added.
    ///
    /// `period` is how many frames the capture device delivers per
    /// callback, which the caller knows from opening it and the engine
    /// cannot: the rings are sized from it, a few periods deep, so the
    /// capture and output clocks have room to disagree before a frame is
    /// dropped. At most `MAX_PERIOD_FRAMES`, which is what makes the rings
    /// fixed overhead with a known worst case rather than audio-sized
    /// memory on the ledger.
    pub fn input(
        &self,
        layout: &[Channels],
        period: usize,
    ) -> Result<(Input, InputFeed), EngineError> {
        let width = input::width(layout);
        if !(1..=MAX_INPUT_CHANNELS).contains(&width) {
            return Err(EngineError::BadInputWidth(width));
        }
        if !(1..=MAX_PERIOD_FRAMES).contains(&period) {
            return Err(EngineError::BadInputPeriod(period));
        }
        // A sound per source. The capture callback writes frames in
        // through the feed, the renderer reads them out through the
        // readers on the audio thread. One command per source; all of
        // them go or none does. The sound ids a refused add minted are
        // burnt, and nothing minds.
        let first = self.renderer.mint(layout.len());
        let sounds: Vec<SoundId> = (0..layout.len() as u64)
            .map(|n| SoundId(first.0 + n))
            .collect();
        let (feed, readers) = InputFeed::open(layout, period);
        self.renderer.add_input(&sounds, readers)?;
        Ok((Input::new(&sounds, self.renderer.clone()), feed))
    }

    /// Stops every sample. Inputs keep being heard.
    pub fn stop_all(&self) -> Result<(), EngineError> {
        self.renderer.stop_all()
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::*;
    use crate::audio::AudioBuffer;
    use crate::config::Pcm;
    use crate::project::tests::memory_project;
    use crate::project::{Project, SampleId};
    use crate::testing::audio;

    /// A mono buffer of `n` frames, no two alike.
    fn ramp(n: usize) -> AudioBuffer {
        audio(1, &(0..n).map(|i| i as Pcm * 0.01).collect::<Vec<_>>())
    }

    /// What `ramp(n)` costs the budget.
    fn ramp_bytes(n: usize) -> usize {
        n * size_of::<Pcm>()
    }

    /// A project on in-memory storage with a ramp of `n` frames in it, and
    /// that sample's id.
    fn project_with(n: usize) -> (Project, SampleId) {
        let mut p = memory_project();
        let id = p.add_sample(None, &ramp(n)).unwrap();
        (p, id)
    }

    /// An engine and its output, which the test drives by hand.
    fn engine() -> (Engine, Renderer) {
        let mut engine = Engine::new();
        let output = engine.output().unwrap();
        (engine, output)
    }

    /// Renders one period of `frames` frames and returns it.
    fn period(r: &mut Renderer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        r.render(&mut out);
        out
    }

    #[test]
    fn output_is_taken_once() {
        let mut e = Engine::new();
        assert!(e.output().is_some());
        assert!(e.output().is_none());
    }

    #[test]
    fn commands_sent_before_the_output_is_taken_wait_for_it() {
        let (p, id) = project_with(2);
        let loaded = p.load(id).unwrap();
        let mut e = Engine::new();
        e.play(&loaded).unwrap();

        let mut o = e.output().unwrap();
        let out = period(&mut o, 2);
        let expected: Vec<f32> = ramp(2).samples().iter().flat_map(|&s| [s, s]).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn play_is_heard_from_the_renderers_next_period() {
        let (p, id) = project_with(2);
        let loaded = p.load(id).unwrap();
        let (e, mut r) = engine();

        let play = e.play(&loaded).unwrap();
        let out = period(&mut r, 2);
        // Two frames fit in one two-frame period, so the sound is done.
        assert!(!r.is_playing(play.id()));
        let expected: Vec<f32> = ramp(2).samples().iter().flat_map(|&s| [s, s]).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn each_play_is_its_own_sound() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (e, mut r) = engine();

        let a = e.play(&loaded).unwrap();
        let b = e.play(&loaded).unwrap();
        assert_ne!(a.id(), b.id());
        period(&mut r, 4);
        assert_eq!(r.playing(), 2);

        // The verb is on the sound, not the engine.
        a.stop().unwrap();
        period(&mut r, 4);
        assert!(!r.is_playing(a.id()));
        assert!(r.is_playing(b.id()));

        e.stop_all().unwrap();
        period(&mut r, 4);
        assert_eq!(r.playing(), 0);
    }

    #[test]
    fn a_full_ring_refuses_the_command_and_the_next_play_is_a_new_sound() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (e, mut r) = engine();
        let mut last = None;
        for _ in 0..COMMAND_CAPACITY {
            last = Some(e.play(&loaded).unwrap());
        }
        assert!(matches!(e.play(&loaded), Err(EngineError::CommandsFull)));
        assert!(matches!(e.stop_all(), Err(EngineError::CommandsFull)));

        // Once the renderer has drained the ring, plays go again. The
        // refused press burnt an id, which nothing minds; the next sound
        // is simply a later one.
        period(&mut r, 4);
        let next = e.play(&loaded).unwrap();
        assert!(next.id() > last.unwrap().id());
    }

    #[test]
    fn an_input_is_one_to_max_channels_wide_and_has_a_period() {
        let (e, _r) = engine();
        let too_wide = [Channels::Mono; MAX_INPUT_CHANNELS + 1];
        for bad in [&[][..], &too_wide[..]] {
            assert!(matches!(
                e.input(bad, 64),
                Err(EngineError::BadInputWidth(w)) if w == bad.len()
            ));
        }
        for bad in [0, MAX_PERIOD_FRAMES + 1] {
            assert!(matches!(
                e.input(&[Channels::Mono], bad),
                Err(EngineError::BadInputPeriod(p)) if p == bad
            ));
        }
        assert_eq!(
            e.renderer.input_sounds(),
            0,
            "a refused input holds no slot"
        );
    }

    #[test]
    fn each_source_of_an_input_is_heard_on_its_own_and_all_go_when_it_is_dropped() {
        let (e, mut r) = engine();
        // A device of four channels: a stereo pair, then two microphones.
        let (input, mut feed) = e
            .input(&[Channels::Stereo, Channels::Mono, Channels::Mono], 64)
            .unwrap();
        let [pair, mic_a, mic_b] = input.sources() else {
            panic!("three sources");
        };
        assert_eq!(feed.channels(), 4);
        assert_eq!(e.renderer.input_sounds(), 3);
        // One four-wide frame per feed, quiet enough that no sum clips.
        let frame = [0.1, 0.2, 0.3, 0.4];

        feed.feed(&frame);
        period(&mut r, 1);
        assert!(
            [pair, mic_a, mic_b].iter().all(|s| r.is_playing(s.id())),
            "an input's sounds are in the renderer at once"
        );

        // The verbs are on the source, not the engine.
        pair.monitor().unwrap();
        feed.feed(&frame);
        assert_eq!(period(&mut r, 1), vec![0.1, 0.2]);

        mic_b.monitor().unwrap();
        pair.unmonitor().unwrap();
        feed.feed(&frame);
        assert_eq!(period(&mut r, 1), vec![0.4, 0.4]);

        mic_a.monitor().unwrap();
        feed.feed(&frame);
        assert_eq!(period(&mut r, 1), vec![0.3 + 0.4, 0.3 + 0.4]);

        let ids: Vec<SoundId> = input.sources().iter().map(|s| s.id()).collect();
        drop(input);
        assert_eq!(e.renderer.input_sounds(), 0, "the slots are given back");
        period(&mut r, 1);
        assert_eq!(r.playing(), 0, "every source of the input is gone");
        assert!(ids.iter().all(|&s| !r.is_playing(s)));
        // The feed outlives the handle harmlessly: its frames go nowhere.
        feed.feed(&frame);
        assert_eq!(period(&mut r, 1), vec![0.0, 0.0]);
    }

    #[test]
    fn more_input_sounds_than_slots_are_refused_until_an_input_is_dropped() {
        let (e, mut r) = engine();
        // One stereo input, then mono inputs up to the slot count.
        let (first, _first_feed) = e.input(&[Channels::Stereo, Channels::Mono], 64).unwrap();
        let rest: Vec<_> = (2..MAX_INPUT_SOUNDS)
            .map(|_| e.input(&[Channels::Mono], 64).unwrap())
            .collect();
        assert!(matches!(
            e.input(&[Channels::Mono], 64),
            Err(EngineError::InputsFull)
        ));
        period(&mut r, 2);
        assert_eq!(r.playing(), MAX_INPUT_SOUNDS);

        // Dropping the two-source input makes room for two sources, not three.
        let gone: Vec<SoundId> = first.sources().iter().map(|s| s.id()).collect();
        drop(first);
        assert!(matches!(
            e.input(&[Channels::Mono, Channels::Mono, Channels::Mono], 64),
            Err(EngineError::InputsFull)
        ));
        let (next, _next_feed) = e.input(&[Channels::Mono, Channels::Mono], 64).unwrap();
        period(&mut r, 2);
        assert_eq!(r.playing(), MAX_INPUT_SOUNDS);
        assert!(gone.iter().all(|&s| !r.is_playing(s)));
        assert!(next.sources().iter().all(|s| r.is_playing(s.id())));
        assert!(rest.iter().all(|(i, _)| r.is_playing(i.sources()[0].id())));
    }

    #[test]
    fn a_full_ring_refuses_an_input_and_holds_no_slot() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (e, mut r) = engine();
        for _ in 0..COMMAND_CAPACITY {
            e.play(&loaded).unwrap();
        }
        assert!(matches!(
            e.input(&[Channels::Stereo], 64),
            Err(EngineError::CommandsFull)
        ));
        assert_eq!(e.renderer.input_sounds(), 0);

        // With room again the input is added, its sources consecutive.
        period(&mut r, 4);
        let before = e.renderer.next_sound();
        let (input, _feed) = e.input(&[Channels::Stereo, Channels::Mono], 64).unwrap();
        let ids: Vec<SoundId> = input.sources().iter().map(|s| s.id()).collect();
        assert_eq!(ids, [SoundId(before), SoundId(before + 1)]);
        assert_eq!(e.renderer.next_sound(), before + 2);
        assert_eq!(e.renderer.input_sounds(), 2);
    }

    #[test]
    fn an_input_dropped_onto_a_full_ring_keeps_its_slots_until_its_feed_goes() {
        let (e, mut r) = engine();
        let (input, feed) = e.input(&[Channels::Mono], 64).unwrap();
        let sound = input.sources()[0].id();
        period(&mut r, 1);
        while e.stop_all().is_ok() {}

        // No room for the remove: the drop cannot fail, so it degrades. The
        // reader stays and the slot stays spoken for.
        drop(input);
        assert_eq!(e.renderer.input_sounds(), 1);
        period(&mut r, 1);
        assert!(r.is_playing(sound));

        // Dropping the feed finishes the reader on its own.
        drop(feed);
        period(&mut r, 1);
        assert!(!r.is_playing(sound));
    }

    #[test]
    fn unload_stops_the_sounds_playing_the_sample_and_no_other() {
        let mut p = memory_project();
        let gone = p.add_sample(None, &ramp(100)).unwrap();
        let kept = p.add_sample(None, &ramp(100)).unwrap();
        let gone = p.load(gone).unwrap();
        let kept = p.load(kept).unwrap();
        let (e, mut r) = engine();
        let a = e.play(&gone).unwrap();
        let b = e.play(&kept).unwrap();
        period(&mut r, 4);

        e.unload(gone).unwrap();
        // Gone from the output at its next render, and only that sample.
        period(&mut r, 4);
        assert!(!r.is_playing(a.id()));
        assert!(r.is_playing(b.id()));
    }

    #[test]
    fn unload_frees_the_audio_once_its_sounds_are_cut() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (e, mut r) = engine();
        let play = e.play(&loaded).unwrap();
        period(&mut r, 4);

        e.unload(loaded).unwrap();
        // The sound still holds the audio until the renderer applies the
        // eviction at its next render; then both are gone.
        assert_eq!(p.ledger().in_use(), ramp_bytes(100));
        period(&mut r, 4);
        assert!(!r.is_playing(play.id()));
        assert_eq!(p.ledger().in_use(), 0);
    }

    #[test]
    fn unload_on_a_full_ring_is_refused_and_hands_the_sample_back() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (e, _r) = engine();
        for _ in 0..COMMAND_CAPACITY {
            e.play(&loaded).unwrap();
        }

        let Err((loaded, EngineError::CommandsFull)) = e.unload(loaded) else {
            panic!("refused with the sample");
        };
        assert_eq!(loaded.id(), id);
        assert_eq!(p.ledger().in_use(), ramp_bytes(100));
    }

    #[test]
    fn a_dropped_sample_lets_its_sounds_play_out() {
        let (p, id) = project_with(100);
        let loaded = p.load(id).unwrap();
        let (e, mut r) = engine();
        let play = e.play(&loaded).unwrap();
        period(&mut r, 4);

        drop(loaded);
        // Nothing was sent: the sound keeps the audio alive and plays on.
        period(&mut r, 4);
        assert!(r.is_playing(play.id()));
        assert_eq!(p.ledger().in_use(), ramp_bytes(100));
        // The audio is let go when the sound is.
        e.stop_all().unwrap();
        period(&mut r, 4);
        assert_eq!(p.ledger().in_use(), 0);
    }
}
