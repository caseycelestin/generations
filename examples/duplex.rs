//! The duplex channel and the collector as an engine would use them: a
//! control type and a voice type wrap the channel's ends, and a renderer
//! acts on commands each period. A voice's wave is shared; the renderer
//! gives its copy up to the collector rather than drop it.
//!
//! A demonstration of the shape, kept in step with the mechanism; both are
//! stubbed, so it does not run yet.

use std::sync::Arc;

use generations::collector::{self, Collector, Disposer};
use generations::duplex::{self, ControlEnd, Full, Handle, Id, Message, RealtimeEnd};

type Wave = Arc<Vec<f32>>;

#[derive(Debug)]
enum Command {
    Start { hz: u32, wave: Wave },
    Stop,
    StopPitch(u32),
    StopAll,
}

#[derive(Debug)]
enum Event {
    Ended,
}

/// What an engine's control type is: a wrapper around the control end.
struct Control {
    channel: ControlEnd<Command, Event>,
    collector: Collector<Wave>,
}

impl Control {
    /// Collects first, as every send does, so the collector never fills.
    fn play(&mut self, hz: u32, wave: &Wave) -> Result<Voice, Full<()>> {
        self.collector.collect();
        let permit = self.channel.try_reserve()?;
        let wave = Arc::clone(wave);
        let handle = permit.open(Command::Start { hz, wave });
        Ok(Voice {
            handle,
            ended: false,
        })
    }

    fn stop(&mut self, hz: u32) -> Result<(), Full<Command>> {
        self.collector.collect();
        self.channel.send(Command::StopPitch(hz))
    }

    fn stop_all(&mut self) -> Result<(), Full<Command>> {
        self.collector.collect();
        self.channel.send(Command::StopAll)
    }

    fn collect(&mut self) {
        self.collector.collect();
    }
}

/// A feature's type wraps a handle and keeps what it wants of its events.
struct Voice {
    handle: Handle<Command, Event>,
    ended: bool,
}

impl Voice {
    fn stop(&self) -> Result<(), Full<Command>> {
        self.handle.send(Command::Stop)
    }

    fn ended(&mut self) -> bool {
        while let Some(event) = self.handle.try_recv() {
            match event {
                Event::Ended => self.ended = true,
            }
        }
        self.ended
    }
}

/// The real-time side: each period it acts on what is queued and reports.
struct Renderer {
    channel: RealtimeEnd<Command, Event>,
    disposer: Disposer<Wave>,
    voices: [Option<(Id, u32, Wave)>; 4],
}

impl Renderer {
    fn period(&mut self) {
        while let Some(Message { id, data }) = self.channel.try_recv() {
            match data {
                Command::Start { hz, wave } => self.start(id, hz, wave),
                Command::Stop => self.end_where(|voice, _| voice == id),
                Command::StopPitch(hz) => self.end_where(|_, pitch| pitch == hz),
                Command::StopAll => self.end_where(|_, _| true),
            }
        }
    }

    /// Every opened handle gets its end, so a voice with no room ends at once.
    fn start(&mut self, id: Id, hz: u32, wave: Wave) {
        match self.voices.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some((id, hz, wave)),
            None => {
                self.disposer.dispose(wave);
                self.channel.end(id, Event::Ended);
            }
        }
    }

    /// Gives the wave up before the end goes out, so an ended voice has let
    /// go of it.
    fn end_where(&mut self, test: impl Fn(Id, u32) -> bool) {
        for slot in &mut self.voices {
            if let Some((id, hz, _)) = slot
                && test(*id, *hz)
                && let Some((id, _, wave)) = slot.take()
            {
                self.disposer.dispose(wave);
                self.channel.end(id, Event::Ended);
            }
        }
    }
}

fn main() {
    let (channel, realtime) = duplex::new::<Command, Event>();
    let (collector, disposer) = collector::new::<Wave>();
    let mut control = Control { channel, collector };
    let mut renderer = Renderer {
        channel: realtime,
        disposer,
        voices: [const { None }; 4],
    };

    let wave: Wave = Arc::new(vec![0.0; 48_000]);
    let watch = Arc::downgrade(&wave);
    let mut low = control.play(220, &wave).unwrap();
    let mut mid = control.play(440, &wave).unwrap();
    let mut high = control.play(880, &wave).unwrap();
    drop(wave);

    low.stop().unwrap();
    control.stop(440).unwrap();
    renderer.period();
    assert!(low.ended() && mid.ended() && !high.ended());

    control.stop_all().unwrap();
    renderer.period();
    assert!(high.ended());

    // The last copy of the wave is freed here, on the control side.
    control.collect();
    assert_eq!(watch.strong_count(), 0);
}
