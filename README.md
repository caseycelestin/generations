# generations

The engine behind the **generations** sampler, recorder, and multitrack
recorder. Written in Rust, with the long-term goal of powering a dedicated
hardware unit for music production.

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

Early. This is currently a freshly initialized Rust crate; the design above is
the target, not the present state.

## Building

Requires a Rust toolchain (edition 2024).

```sh
cargo build
cargo run
```

## License

MIT. See [LICENSE](LICENSE).
