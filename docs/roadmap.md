# Roadmap

The order the engine is being built in, and why. Steps are deliberately
small; each one should be a PR that leaves `main` working.

| # | Step | Status |
|---|------|--------|
| 1 | Core data model: `AudioBuffer`, `Sample`, `Project` | done |
| 2 | First print: one real effect (reverse) and a thin engine that wires it to the project | next |
| 3 | Persistence: save and load a project directory | |
| 4 | Hear something: load a WAV and play it through the audio device | |
| 5 | Recording: audio input as a new root | |
| 6 | Sequencing and mixing: the real-time side | |
| 7 | Arrangement and more effects | |

## Why this order

The novel and risky part of the project is the generation model, not the
hardware. Audio I/O in Rust is well trodden, so it can wait; the data model
shapes everything built on top of it, so it goes first.

Effects print rather than run live, so they are offline functions from
buffer to buffer. Only playback, mixing and sequencing are real-time. That
makes effects easy and sequencing hard, and the order reflects it.

Persistence comes after the first effect because the on-disk form is close
to a direct dump of the in-memory model, and it is easier to get right once
the model has been exercised by something.

## Design decisions

Things a reader cannot recover from the code alone.

- **Target hardware** is a Raspberry Pi Compute Module 4. The reference
  device for feel and specs is the Roland SP-404 MK2. The goal is for a
  whole project to fit in RAM, so audio memory size is a priority.
- **One fixed format**: 48 kHz, 16-bit, set in `config.rs`. Import converts
  to it, output converts from it, nothing in between checks. Channel count
  is kept per buffer so mono samples do not pay for stereo.
- **Storage is i16, DSP is f32.** Every effect converts in and out through
  `AudioBuffer::to_f32` / `from_f32`, which are the only code that knows the
  PCM width. The downward conversion rounds and applies TPDF dither. This is
  the standard DAW and sampler architecture; the conversion is one multiply
  per sample at two boundaries.
- **Generation is a relationship, not a type.** A `Sample` is an id, an
  optional parent id and audio. Lineage is the tree of parent pointers.
  Children are found by walking the pool; nothing else is stored.
- **Samples carry structure only.** No name, no label, no record of whether
  they came from a file or a recording. Naming and organising belong to
  whatever points at a sample (pads, tracks) in a later layer.
- **Ids are a per-project counter** behind a `SampleId` newtype. Content
  hashes were considered and rejected for now: a hash of the audio alone
  breaks the tree (reversing twice yields the root's bytes), and git-style
  node hashes buy cross-project sharing that self-contained projects do not
  need.
- **Projects are self-contained directories**: a small manifest plus one WAV
  per sample. Copying the directory copies the project.
- **Removing a sample with children is refused.** Later generations depend
  on it. Removing a whole lineage will be a separate, explicit operation.
- **The public surface is kept minimal.** Convenience methods are added when
  a caller exists, not before.

## Planned architecture

```
engine        thin orchestration; what a UI or front panel calls
 ├── project  structure: ids and parents
 ├── effects  pure functions, buffer in, buffer out, offline
 ├── io       import (decode, resample, widen) and save/load
 └── playback real-time thread; never allocates, locks or touches disk
```

A print is: the engine fetches a sample's audio, runs an effect, and adds
the result to the project with the source as parent. The project does not
know effects exist and effects do not know projects exist.

When memory becomes a constraint, audio moves out of `Sample` into a store
that tracks where each sample's bytes are (RAM, eMMC, SD) and what is
resident. `Sample::audio()` is an accessor rather than a field so that
change stays inside the type.

## Deferred ideas

- A replayable recipe on prints, so a chain applied to one sample can be
  applied to others as a preset. Needs effects to exist first. Not every
  print is replayable, so a label-only form must remain.
- Content-hash dedup on import, so importing the same file twice does not
  cost memory twice. An index beside the counter id, not a replacement.
- Lazy loading and eviction of sample audio, SP-404 style streaming from
  storage with heads kept resident.
