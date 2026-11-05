# Roadmap

The order the engine is being built in, and why. Steps are deliberately
small; each one is a PR that leaves `main` working. The architecture itself
is documented in the crate: `cargo doc --open` and start at the top.

| # | Step | Status |
|---|------|--------|
| 1 | Core data model | done, then rebuilt in step 3 |
| 2 | First print: one real effect (reverse) and an engine that applies it | done |
| 3 | Persistence: the project lives on disk, every change durable at once | done |
| 4 | Hear something: load a WAV and play it through the audio device | in progress |
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
- **Samples carry metadata as well as lineage.** The first version of this
  decision said structure only: no name, no label, nothing but the parent
  pointer. That was wrong. A sample needs to carry facts about itself that
  nothing else is in a position to hold, and `loaded` is the first. More
  arrive as their steps come. `sample(id)` is where they are read, and
  each one that can change gets its own setter with its own rule, rather
  than one general update: the id and parent can never change, and a rule
  belongs next to the one field it governs.
- **Project mutation goes through the engine.** `Engine::project()` lends
  the project read-only; every change is an engine method. Some changes
  are only half a project write (`load` sets a flag and moves audio into
  RAM), and a caller reaching the project directly would leave the two
  disagreeing. So new mutating methods on `Project` are crate-private by
  default, starting with `set_loaded`. The older `add_sample` and
  `remove_sample` are still public and will be tightened when there is a
  reason to touch them.

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

### Playback

Step 4 is being built up one decision at a time, and the decisions so far
are here even though most of the code is not.

- **Playback belongs to the engine.** Pressing a pad is a front-panel
  action, and the engine is what the front panel calls. The device, the
  audio thread and the mixer are their own role beside effects, knowing
  only buffers; the engine is the one place that touches both them and
  the project. Keeping playback outside would need a second coordinator
  by step 6.
- **Playback reads only audio already in RAM.** The real-time thread never
  touches storage: storage reads the manifest from disk and fsyncs on every
  write, and an fsync on eMMC can stall other I/O for hundreds of
  milliseconds. Streaming from eMMC is not bandwidth-bound (a stereo voice
  is 192 KB/s against tens of MB/s) but it is latency-bound, and our own
  prints are the main contention. Streaming long recordings is a deferred
  idea.
- **Residency is explicit and persisted.** A sample is in RAM because
  someone asked, not because it was played. `loaded` on the sample is the
  recorded intent; the engine loads every flagged sample on open, so the
  device comes back ready after a power-down. `Engine::load` and
  `Engine::unload` change it. Playing a sample that is not loaded is an
  error, not a silent disk read, so a press never pays for a load and
  latency never depends on history. What to load is the caller's policy;
  the engine never loads on its own. Load-on-play is two lines in a
  caller that wants it.
- **Adding a sample and loading it stay separate.** A print the user made
  must land on disk even when it does not fit in RAM. Combining the two
  would have to either refuse the add or store the sample unloaded anyway,
  which makes the flag a request rather than a fact. The cost is a disk
  read of bytes the engine was just holding, a few milliseconds per print.
- **The budget is enforced where RAM is managed.** `SAMPLE_RAM_BUDGET` in
  `config.rs` bounds resident audio. The `ResidentSet` in `playback.rs`
  checks it, because it is the one thing that knows how many bytes it
  holds; the project never learns about RAM. `Engine::load` puts the audio
  in RAM first and sets the flag only if that succeeded, so the flag is
  never true for a sample that is not resident. The engine lends the set
  read-only through `Engine::resident()`, as it lends the project.
- **Resident audio is ordinary heap allocation; the OS guarantees the
  budget.** Each buffer owns its own block, wrapped in an `Arc` so a voice
  can hold it while it plays. There is no preallocated arena. Linux
  overcommits, so no allocation inside the engine can guarantee the memory
  exists; the guarantee comes from the device's configuration instead: a
  cgroup memory reservation for the engine's process, no swap, and the
  budget set below what is really free. The alternative, a touched and
  locked arena with a fixed-size chunk pool, gives the same guarantee in
  code we control but reshapes the resident form of audio and how storage
  reads; it can be added later behind the same `insert` and `get`.
- **Open fails when the loaded set no longer fits.** This happens only if
  the budget shrank or the project moved to a smaller device. Open could
  instead load what fits and clear the rest, but that is a policy and a
  project write during open. Failing is honest, and the case is rare.
- **Loading is synchronous.** A load is a disk read on the control
  thread, a few milliseconds to a couple of hundred. Nothing with a
  deadline waits on it: the audio thread never calls it. When a front
  panel exists and a load visibly stalls it, the fix is an engine thread
  with a command channel, not async methods; the engine's API stays plain
  functions.
- **A fixed voice count.** `VOICE_COUNT` in `config.rs`. Mixing is cheap
  (one multiply-add per voice per output sample at original pitch), so the
  cap is not a measured limit. It is a guarantee: the callback has a known
  worst case, and a full table steals the oldest voice rather than
  failing, which is how the SP-404 behaves and what a player relies on.
  Per-voice cost grows once pads can be pitched, and the cap earns its
  keep then.
- **The real-time side is two types: `Voice` and `Renderer`.** A voice is
  how one sample is heard: today the whole buffer from first frame to
  last at its own speed. Start and end points, looping, gain and fades
  are fields and lines on the voice when they come. The renderer is
  real-time safe, not real-time only: it owns the playing voices and the
  consuming end of the command ring, and each call applies the commands
  that arrived, zeroes the slice it is given, lets every voice add itself,
  drops the voices that finished, and clips the sum once. A device drives
  it live from its callback; a bounce drives it offline in a loop. There
  is no separate mixer: a list of voices with a sum is a method, not a
  concept. The pattern is JUCE's `Synthesiser` and Kira's `Renderer`; the
  name is Kira's, and "controller" was rejected because the engine gives
  the orders and this side carries them out.
- **Voices add into the device's buffer, voice-major.** The device owns
  the period buffer and hands it in; the renderer never allocates one.
  Each voice adds its next frames into the whole slice in turn, which is
  how samplers and synths are written: one contiguous read and one
  contiguous add per voice, which the compiler vectorises. Frame-major
  (for each frame, for each voice) jumps between buffers every sample and
  cannot. Threads are not worth it at this size; the work per period is
  tens of microseconds against a budget of milliseconds.
- **Clip the sum once, hard, in the renderer.** Two full-scale voices sum
  past full scale, and an unclamped value either wraps at the DAC or is
  clamped by the driver, depending on the backend. Clamping in the
  renderer makes every device and the offline bounce behave the same. A
  hard clip is what hardware samplers do and it tells the user they are
  over. A master gain or a bus limiter goes before the clamp when it
  comes; the clamp stays last.
- **Commands take effect at period boundaries.** The renderer drains the
  ring once at the start of each period, so a press lands within one
  period, a few milliseconds. Sample-accurate placement within a period
  is what sequencing adds, as a frame offset on the command.
- **Playing voices are a fixed-capacity FIFO.** `heapless::Deque<Voice,
  VOICE_COUNT>`, stored inline in the renderer: oldest first, so stealing
  is pop the front and push the back, and the type cannot allocate. An
  array of `Option<Voice>` tracked free and in-use slots by scanning for a
  sentinel and relied on ids being minted in order to know the oldest;
  the deque says FIFO in the type and needs neither.
- **The ring is `rtrb`.** Single producer, single consumer, lock-free,
  fixed capacity, written for audio. Popping on the audio thread is a few
  atomic loads and never blocks. A full ring drops the push on the control
  side, which is the right failure. `std::sync::mpsc` locks and can
  allocate; writing our own is where subtle bugs come from. The renderer
  owns the consuming end (as Kira does) rather than taking commands as a
  parameter (as JUCE does): there is one host and one feeder, and the
  device closure is then one line.
- **Nothing on the render path reads a clock or the device.** Frame counts
  only, with the rate from config. This is what makes an offline bounce
  possible: build a renderer, drive `render` in a loop faster than real
  time, and the frames are the same ones live playback would produce.
  The bounce is the renderer used as a function, and belongs beside
  effects as another kind of print; it does not go through the live
  renderer.
- **Both constants are set generously.** The real budget depends on the
  CM4 variant (1, 2, 4 or 8 GB); the real voice count depends on what a
  voice costs with pitch and processing. They exist so the design can be
  written in terms of them.

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
RAM       whatever playback needs resident; the ResidentSet, under a budget
```

The storage layer does not know about tiers. A copy to the card and the
resident set for playback are both layers beside or in front of it, not
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
- An offline bounce: a renderer driven in a loop with frame-stamped
  commands, its output stored as a new root. Same code as live playback,
  so what you hear is what you print. Needs the sequencer to count frames
  rather than read a clock.
- A master gain and a bus limiter before the final clamp, once stacked
  pads clipping is a complaint rather than a design.
- A bound on sample length. Audio in flight (an import's buffer, a print's
  input and output, a recording in progress) is not counted against the
  budget, so the OS reservation must cover the budget plus the largest
  transient. A maximum sample length bounds every transient at once,
  including a recording, which has no other natural end. Needed before
  step 5.
- Streaming long recordings from eMMC instead of holding them resident,
  for audio that outgrows RAM. Needs a feeder thread and a prefetch window
  per voice, and has to stay clear of our own fsyncs.
- A budget read from the device's configuration rather than compiled in,
  once the CM4 variant is known and the free memory is measured.
