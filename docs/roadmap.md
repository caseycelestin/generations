# Roadmap

The order the engine is being built in, and why. Steps are deliberately
small; each one should be a PR that leaves `main` working.

| # | Step | Status |
|---|------|--------|
| 1 | Core data model: `AudioBuffer`, `Sample`, `Project` | done |
| 2 | First print: one real effect (reverse) and a thin engine that wires it to the project | done |
| 3 | Persistence: the project lives in a store on disk | done |
| 4 | Hear something: load a WAV and play it through the audio device | next |
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
to a direct dump of the model, and it is easier to get right once the model
has been exercised by something. It turned out to change the model: the
first print kept audio in memory, and persistence showed nothing needed it
there yet, so the project became structure only and the store became where
audio lives.

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
  per sample at two boundaries. Effects that only reorder frames (reverse)
  are exempt: they copy PCM directly, so they are exact and reversing twice
  really does give back the root's bytes.
- **An effect is a function**, `&AudioBuffer -> AudioBuffer`, and the engine
  takes one as a plain argument. Effects-as-data (an enum with parameters)
  is what the replayable recipe idea below needs, and it will be introduced
  with that work rather than ahead of it.
- **Generation is a relationship, not a type.** A `Sample` is an id and an
  optional parent id. Lineage is the tree of parent pointers. Children are
  found by walking the pool; nothing else is stored.
- **The project holds structure only; the store holds audio.** Nothing today
  needs audio resident in memory: there is no playback yet, and a print
  needs its input only while the effect runs. So `Sample` carries no audio,
  `Project::add` records a sample without bytes, and the engine reads a
  sample's audio from the store when a print needs it. When playback
  arrives, a cache of resident audio goes in front of the store; it does
  not go back into the project.
- **The active project persists every change on its own.** There is no
  save step. The engine records a new sample in the project and the store
  together before the call returns, so on the device, where the store is on
  eMMC, a power-down loses nothing. Whatever a user calls "save" is a copy
  to the SD card, and does not exist yet. There is no in-memory engine; one
  without a store would be exactly the thing that loses work.
- **The store is a trait with three operations**: load the structure, put a
  new sample's audio together with the structure that includes it, get a
  sample's audio. The engine never sees a path, a filename or a table, so
  the on-disk form is the backend's private business. A generic conformance
  test is the contract a backend has to pass. Today's backend is a
  directory: a JSON manifest (format version, id counter, id/parent pairs)
  and one WAV per sample named by id. SQLite is the likely second backend
  if a single-file project, transactions or richer metadata become worth
  having; the transition is a new backend, a copy loop and a version check.
- **WAV, not raw bytes.** The header costs 44 bytes and carries the channel
  count, so the manifest does not have to, and any audio tool can open a
  sample, which is how a print gets checked before the engine can play.
- **Load is strict.** A missing WAV, a dangling parent, a WAV in the wrong
  format or an unknown manifest version refuses the whole project rather
  than dropping a sample, so a project that opens has its lineage intact.
  The directory is engine-owned: renaming a WAV by hand breaks the project.
- **Put is incremental.** Samples never change and ids are never reused, so
  a WAV named for an id never needs rewriting; a put writes a missing WAV
  and then replaces the manifest through a temp file and rename. A crash
  leaves the old or the new manifest, and a stray WAV is ignored on load.
  Removing a sample does not yet delete its WAV; that comes with the engine
  operation that removes samples.
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
 ├── store    the Store trait: where a project's bytes live
 │    └── disk  a directory of WAVs plus a manifest; the only backend
 ├── import   decode, resample, widen (step 4)
 └── playback real-time thread; never allocates, locks or touches disk
```

A print is: the engine reads the source's audio from the store, runs an
effect, adds a sample to the project with the source as parent, and puts
the result in the store. The project does not know effects or stores
exist, and effects do not know projects exist.

On the device there are three tiers: the SD card for kept projects, eMMC
for the active one, and RAM for whatever playback needs resident. The
store is the eMMC tier. A copy to the card and a resident cache for
playback are both layers beside or in front of it, not changes to it.

## Deferred ideas

- A replayable recipe on prints, so a chain applied to one sample can be
  applied to others as a preset. Needs effects to exist first. Not every
  print is replayable, so a label-only form must remain.
- Content-hash dedup on import, so importing the same file twice does not
  cost memory twice. An index beside the counter id, not a replacement.
- Lazy loading and eviction of sample audio, SP-404 style streaming from
  storage with heads kept resident.
