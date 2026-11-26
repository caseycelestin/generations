//! Plucks recorded from the main out, then played back.
//!
//! Usage: `cargo run --example record_main_out`. Needs a duplex device.

use std::error::Error;
use std::f32::consts::TAU;
use std::sync::mpsc;
use std::thread::{self, sleep};
use std::time::Duration;

use generations::ChannelLayout;
use generations::config::SAMPLE_RATE;
use generations::engine;

mod device;
use device::Device;

fn main() -> Result<(), Box<dyn Error>> {
    let device = Device::default_duplex()?;
    // Room for the three plucks and the recording played back, and one
    // recorder whose ring holds half a second.
    let ring_frames = SAMPLE_RATE as usize / 2;
    let (mut control, mut renderer) =
        engine::create(device.capture_channels(), 0, 4, 8, 1, ring_frames);
    let mut stream = device.start(move |capture, out| renderer.render(capture, out))?;

    // Half a second of each note, decaying from half scale.
    let frames = SAMPLE_RATE as usize / 2;
    let mut plucks = Vec::new();
    for pitch in [440.0, 554.4, 659.3] {
        let pluck = (0..frames)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                0.5 * (TAU * pitch * t).sin() * (-6.0 * t).exp()
            })
            .collect();
        plucks.push(control.audio().load(pluck, ChannelLayout::Mono)?);
    }

    let (recorder, mut drain) = control.recorders().arm()?;

    // The drain empties the ring every 10 ms, well inside the half second
    // it holds, until told the recording is over.
    let (finished, finish) = mpsc::channel::<()>();
    let collector = thread::spawn(move || {
        let mut recording = Vec::new();
        loop {
            drain.drain_into(&mut recording);
            if finish.try_recv().is_ok() {
                drain.drain_into(&mut recording);
                return recording;
            }
            sleep(Duration::from_millis(10));
        }
    });

    println!("recording the plucks");
    recorder.record();
    for pluck in &mut plucks {
        pluck.play().detach();
        sleep(Duration::from_millis(400));
    }
    sleep(Duration::from_millis(800));
    recorder.stop();
    // A few periods for the renderer to push its last frames.
    sleep(Duration::from_millis(100));
    finished.send(())?;
    let recording = collector.join().expect("the drain thread ran");

    println!("playing back {} frames", recording.len() / 2);
    let mut played = control.audio().load(recording, ChannelLayout::Stereo)?;
    played.play().detach();
    sleep(Duration::from_millis(2200));

    // The audio thread queues its complaints; this is where they are read.
    while let Some(e) = stream.error() {
        eprintln!("audio stream error: {e}");
    }
    Ok(())
}
