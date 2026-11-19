# generations

The engine behind the **generations** sampler, recorder, and multitrack
recorder. Written in Rust for a desktop app first, then perhaps a tablet,
and in the long run perhaps a dedicated hardware unit for music production.

## The idea

Most audio tools let you tweak forever. Every parameter stays live, every
decision stays open, and nothing is ever really finished.

**generations** takes the opposite stance. Audio moves forward by *printing*:
when you apply an effect, resample, chop, or otherwise change a sound, the
result is committed as a new **generation** of that sample. The previous
generation is kept, but the change itself is not editable after the fact.

That gives you two moves, and only two:

- **Commit** – make a decision in the moment and print it.
- **Go back** – return to an earlier generation and try again.

There is no undo-by-tweaking. Decisions are made with your ears, in the
moment, the way they are on tape or on classic hardware samplers. The history
is a lineage of printed audio, not a stack of adjustable parameters.

## What the engine will do

The engine is the shared core that the sampler, the recorder, and eventually
the hardware unit all sit on top of. It is responsible for:

- **Sample management** – importing, organizing, and storing audio.
- **Generation tracking** – recording the lineage of every sample as it is
  printed into new versions, and moving between generations.
- **Effects** – processing audio and printing the result as a new generation.
- **Sequencing** – scheduling the playback of samples over time.
- **Mixing** – levels, routing, and combining tracks.
- **Arrangement** – structuring sequences into a full piece.

## Status

Early. A project lives on disk, durable across a power-down, and loads
samples into RAM under one memory budget. A renderer, driven by whatever
duplex device the caller gives it, plays them on a fixed pool of voices,
hears inputs through the main out, and records the main out or an input
as a take. The crate itself needs no audio device. Nothing prints yet, so
generations are not recorded; lineage returns with the effects that print
them. Timing comes next: requests stamped with the frame they land on,
then sequencing.

- [docs/roadmap.md](docs/roadmap.md): the plan as it stands and the
  decisions that shape it.
- [docs/decisions.md](docs/decisions.md): how the engine reached its
  shape, written once as of November 2026.
- `cargo doc --open`: the architecture.

## Building

Requires a Rust toolchain (edition 2024). The crate is a library with no
audio dependency; the demos are examples.

```sh
cargo test
cargo run --example play     # needs a duplex device
```

The examples build against ALSA through `cpal`, a dev-dependency, so they
need the ALSA headers (`libasound2-dev` on Debian, `alsa-lib-devel` on
Fedora). The library itself does not.

## License

MIT. See [LICENSE](LICENSE).
