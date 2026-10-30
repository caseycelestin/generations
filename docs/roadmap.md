# Roadmap

The order the engine is being built in, and why. Steps are deliberately
small; each one is a PR that leaves `main` working. The architecture itself
is documented in the crate: `cargo doc --open` and start at the top.

| # | Step | Status |
|---|------|--------|
| 1 | Core data model | done, then rebuilt in step 3 |
| 2 | First print: one real effect (reverse) and an engine that applies it | done |
| 3 | Persistence: the project lives on disk, every change durable at once | done |
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

Persistence was meant to be a dump of the in-memory model. It turned out to
replace it: the first model kept every buffer in RAM for a playback goal
that did not exist yet, and persistence would have been a copy of memory
with a save step. Step 3 rebuilt the project around the disk instead.

## Design decisions

Things a reader cannot recover from the code alone. Each one was argued
out; the short version of the argument is here so it is not had twice.

### The project

- **The project is the disk.** `Project` holds a `Storage` and nothing
  else. There is no in-memory copy, not even a cache: every read and write
  goes to storage, so changing the project changes the disk and there is no
  save step. Nothing today needs audio resident in memory; a print needs
  its input only while the effect runs. When playback arrives, a cache of
  resident audio goes in front of storage, not back into the project.
- **The active project persists every change on its own.** On the device
  the active project lives on eMMC and survives a power-down with no save.
  Whatever a user calls "save" is a copy to the SD card, and does not exist
  yet. There is no in-memory engine; one without storage would be exactly
  the thing that loses work.
- **Project is the rules, storage is dumb.** The project enforces: a parent
  must exist; a sample with children cannot be removed. Storage answers
  questions and does what it is told, and never interprets. This is the
  whole reason they are two types rather than one; if the rules were in
  storage, each backend would carry them and they would drift. The project
  is thin today and that is fine; it thickens as patterns and settings
  arrive with rules of their own.
- **"Project" is the right word.** It is what every DAW and sampler,
  including the SP-404, calls the unit a user opens, works in and copies to
  a card. It will hold samples, patterns, effect settings and pad layouts;
  the sample pool is one part of it.
- **A sample does not exist until it is stored, so storage mints its id.**
  Like a database insert returning the row id. The counter lives on disk,
  only goes up, and never reuses an id, even after a remove. The project
  never touches a number it does not own.
- **Two reads per sample.** `sample(id)` returns everything stored about a
  sample except its audio, and grows as fields are added. `audio(id)`
  returns the bytes. They are separate because structure is small and read
  often, audio is large and read when a print or playback needs it.
- **`add_sample` takes a buffer.** Audio in flight is not a copy of the
  project. Recording, import after conversion and print output all produce
  a transient buffer, and storage is where it lands. The thing never to add
  is a way for the project to keep one afterwards.
- **Generation is a relationship, not a type.** A sample is an id and an
  optional parent id. Lineage is the tree of parent pointers; children are
  found by asking storage. Nothing else is stored about the relationship.
- **Samples carry structure only.** No name, no label, no record of whether
  they came from a file or a recording. Naming and organising belong to
  whatever points at a sample (pads, tracks) in a later layer.

### Storage

- **A trait, so the medium can change.** Files now, perhaps SQLite later.
  The trait is the questions a backend can answer efficiently (does any
  sample have this parent) and the writes it must make durable. Rules stay
  above it. A SQLite backend is a new implementation plus a copy loop; the
  engine and project would not change.
- **A storage always holds a whole project.** `storage::open(dir)` creates
  an empty project if the directory is empty or missing, validates the
  whole project it finds otherwise, and refuses anything else: a broken
  project, or files that are not a project at all. So the trait has no
  "exists" or "create", and `Project::new` is a plain constructor. The
  precedent is SQLite's own open.
- **Validation happens on open, not on every call.** Open checks that the
  manifest reads, every parent exists and was minted before its child, the
  counter is past every id, and every WAV is present with a header in the
  engine's format. A project that opens has its lineage intact. Parents
  coming first is what makes a cycle impossible without a graph walk.
- **Every write is durable before the call returns.** The point of
  persist-on-change is that a power cut loses nothing, and without fsync
  that is not true. The WAV is synced before the manifest names it, the
  temp manifest is synced before the rename, and the directory is synced
  after each new, renamed or removed file. A power cut leaves the old
  manifest or the new one, never half of either; a WAV the manifest does
  not mention is ignored.
- **The on-disk form is a directory.** `project.json` holds the format
  version, the id counter and the samples as a map keyed by id (so a
  duplicate id is impossible and the file is written in id order), plus
  `samples/<id>.wav`. The manifest is a private struct inside the backend,
  not a serde derive on `Project`, so the file format is versioned on its
  own and a change to the model does not silently change what is on disk.
- **WAV, not raw bytes.** The header costs 44 bytes and carries the channel
  count, so the manifest does not have to, and any audio tool can open a
  sample, which is how a print gets checked before the engine can play.
- **Nothing is cached in the backend either.** Every operation reads the
  manifest from disk. The OS caches the file; the structure is tiny; and
  nothing real-time reads it. If that ever costs, it is the backend's
  private problem to solve.
- **The directory is engine-owned.** Renaming a WAV by hand breaks the
  sample and open refuses the project. Recovering from that is content-hash
  territory, deferred below.

### Audio

- **Target hardware** is a Raspberry Pi Compute Module 4. The reference
  device for feel and specs is the Roland SP-404 MK2.
- **One fixed format**: 48 kHz, 16-bit, set in `config.rs`. Import converts
  to it, output converts from it, nothing in between checks. Channel count
  is kept per buffer so mono samples do not pay for stereo.
- **Storage is i16, DSP is f32.** Every effect converts in and out through
  `AudioBuffer::to_f32` / `from_f32`, which are the only code that knows the
  PCM width. The downward conversion rounds and applies TPDF dither. Effects
  that only reorder frames (reverse) are exempt: they copy PCM directly, so
  they are exact and reversing twice really does give back the root's bytes.
- **An effect is a function**, `&AudioBuffer -> AudioBuffer`, and the engine
  takes one as a plain argument. Effects-as-data (an enum with parameters)
  is what the replayable recipe idea below needs, and it will be introduced
  with that work rather than ahead of it.

### Process

- **The public surface is kept minimal.** Convenience methods are added
  when a caller exists, not before.
- **Nothing is settled by having been written.** The step 1 model was
  removed in step 3 when it stopped fitting. New steps are designed from
  the decision, then existing code is changed to fit, not the other way
  round.
- **The demo lives in `examples/`.** The crate is a library; there is no
  binary until there is a device to run one on.

## Storage tiers on the device

```
SD card   kept projects; a copy of the active one, made on request
eMMC      the active project; every change lands here at once
RAM       whatever playback needs resident; a cache in front of storage
```

The storage layer does not know about tiers. A copy to the card and a
resident cache for playback are both layers beside or in front of it, not
changes to it.

## Deferred ideas

- A replayable recipe on prints, so a chain applied to one sample can be
  applied to others as a preset. Needs effects-as-data first. Not every
  print is replayable, so a label-only form must remain.
- Content-hash dedup on import, so importing the same file twice does not
  cost memory twice, and a renamed WAV can be matched back to its sample.
  An index beside the counter id, not a replacement.
- Removing a whole lineage as one explicit operation.
- A SQLite backend, if a single-file project, transactions or richer
  metadata become worth having.
