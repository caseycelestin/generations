# Roadmap

The order the engine is being built in, and why. Steps are deliberately
small; each one is a PR that leaves `main` working. The architecture itself
is documented in the crate: `cargo doc --open` and start at the top.

| # | Step | Status |
|---|------|--------|
| 1 | Core data model | done, then rebuilt in step 3 |
| 2 | First print: one real effect (reverse) and an engine that applies it | done, then removed in step 5 |
| 3 | Persistence: the project lives on disk, every change durable at once | done |
| 4 | Hear something: load a WAV and play it through the audio device | done |
| 5 | RAM budget: one figure for all audio in memory | done; now `AudioMemory` |
| 6 | Recording: audio input as a new root | built twice, removed both times; to be rebuilt on `src/engine/` |
| 7 | Input monitoring: hear the input through the output | done on the old engine, removed with it; to be rebuilt |
| 8 | Sequencing and mixing: the real-time side | mixing done on `src/engine/`; sequencing to come |
| 9 | Arrangement and more effects | |

## Why this order

The novel and risky part of the project is the generation model, not the
hardware. Audio I/O in Rust is well trodden, so it can wait; the data model
shapes everything built on top of it, so it goes first. Only playback,
mixing and sequencing are real-time, so sequencing is the hard part and
comes late.

## Design decisions

Each entry is a decision that stands today: what, why, and what was tried
or rejected instead. The history of how a shape was reached is in git.

### The project

- **The project is the disk.** `Project` is a directory and holds no
  in-memory copy of the project, not even a cache. Every change is
  durable when the call returns, so there is no save step; on the device
  the active project lives on eMMC and a "save" is a copy to the SD card.
  The first model kept everything in RAM and treated persistence as a
  dump; step 3 replaced it.
- **Project and storage are one type.** `Project` and `Storage` were
  split as rules over a dumb medium, for swappable backends. The backend
  trait went first, then the rules went with lineage, leaving `Project`
  passing every call through with two nested error types; merged
  2026-11-13, in one file, `project.rs`: the public methods, then the
  private ones that read and write the files. When lineage returns, its
  rules are checks against the manifest before a write, on `Project`.
- **The project mints sample ids.** A sample does not exist until it is
  stored, like a database insert returning the row id. The counter is on
  disk, only goes up, and never reuses an id.
- **A sample is named by its `SampleId`; a voice is not.** A sample
  outlives the program, so callers refer to it by an id they can keep and
  store (a parent pointer, pads and patterns later), and
  `load(id, &memory)` turns that into `Audio` the way a path opens a file.
  A voice lives only while it plays, so its handle keeps its `VoiceId`
  private and carries the verbs instead.
- **Generation is a relationship, not a type.** A sample will be an id
  and an optional parent id; lineage is the tree of parent pointers. It
  was built in step 3 and removed after step 8, because nothing had made
  a child since prints went in step 5. It returns with prints: `parent`
  goes in the manifest's per-sample `Entry` with `#[serde(default)]`
  (empty until then), and gets its rule on `Project`; `metadata(id)` comes
  back to read it. Other fields join the sample the same way as steps
  need them. (A `loaded` flag was the first
  and was removed under Handles. Removing samples was built and cut in
  the memory port: no caller.)
- **"Project"** is what every sampler calls the unit a user opens and
  copies to a card. It will hold patterns, settings and pad layouts too.

### Files

- **No backend abstraction.** The `Storage` trait (files now, perhaps
  SQLite later) was removed 2026-11-13: it had one implementation, and
  the project's tests run on a temporary directory instead of an
  in-memory stub.
- **A `Project` is always a whole project.** `open(dir)` creates an empty
  project in an empty or missing directory, validates a whole project
  otherwise, and refuses anything else. Validation happens on open, not
  per call: the manifest reads, the counter is past every id, every WAV
  is present in the engine's format. When lineage returns, every parent
  must exist and be minted before its child, which rules out cycles
  without a graph walk.
- **Every write is fsynced before the call returns.** WAV before the
  manifest names it, temp manifest before the rename, the directory after
  each file change. A power cut leaves the old manifest or the new one; a
  WAV the manifest does not name is ignored.
- **The on-disk form is a directory:** `project.json` (format version, id
  counter, samples keyed by id) and `samples/<id>.wav`. The manifest is a
  private struct in `project.rs`, not a serde derive on the model, so the
  file format is versioned on its own. WAV rather than raw bytes so any
  audio tool can open a sample.
- **Nothing is cached by the project.** The OS caches the file and nothing
  real-time reads it.
- **The directory belongs to the engine.** A hand-renamed WAV makes open
  refuse the project; recovering is content-hash territory (Deferred).

### Audio

- **Target:** Raspberry Pi Compute Module 4; reference device Roland
  SP-404 MK2.
- **One fixed format: 48 kHz, `f32`, on disk, in RAM and on the render
  path.** Channel count (1 or 2) is per buffer. Day one stored `i16` and
  processed `f32`; switched 2026-11-09 because the CM4 has hardware float,
  `i16` throws away 24-bit sources at every generation, and the render
  path should read samples rather than convert them. Cost: half the
  resident minutes. The `Pcm` alias in `config.rs` keeps the choice in one
  place; an `i16` build is a deferred idea, chosen at build time, never a
  hybrid.
- **Effects are not built.** The step 2 offline effect (`&AudioBuffer ->
  AudioBuffer`, `print`, reverse) was removed in step 5: effects are to
  live on the render side, and the offline form was the one allocation the
  RAM budget could not see.

### Audio in

- **Memory and storage are separate.** The caller makes one
  `AudioMemory` for the machine (`new()`, or `with_budget(bytes)`) and
  every audio byte in RAM comes from it; the project holds none, and a
  load is paid for from the memory the caller passes. The budget belongs
  to the machine, not to a project. (2026-11-13; it was the project's
  `Ledger`.)
- **Audio enters a project by being written.** `memory.alloc(layout,
  frames)` hands out empty `Audio`, paid for in full against the budget;
  `audio.extend(..)` fills it; and `project.add_sample(&audio)` stores it
  as a sample and mints its id. Nothing is stored and no id exists until
  the add. `alloc` is named for what it
  does: it takes memory and touches no disk.
- **WAV import was removed 2026-11-13** (the `decode` and `convert`
  stages, `rubato`, `Project::import`); it is in the history before
  97e8eff if a file format is needed again.
- **The rule is that every byte of audio in RAM is counted, not that the
  user never touches audio.** `Audio` is public, readable and writable;
  only the crate can make one, so all of it comes from `AudioMemory`.
- **Audio in RAM, samples on disk.** What RAM holds has no id: it becomes
  a sample only when a project stores it. So the RAM types are named for
  audio (`AudioMemory`, `Audio`, crate-private `AudioBuffer`) and the
  project's for samples (`SampleId`, `add_sample`). (2026-11-13; the
  handle was `Sample`, the source `SampleMemory`.)
- **One type is written, played and saved.** `Audio` is an
  `Arc<AudioBuffer>` with no `Clone`. `extend` goes through
  `Arc::get_mut`: refused while the engine holds the audio, which is what
  `Audio` protects; a read by the project is protected by an ordinary
  borrow. A voice that has ended still holds it until its return is
  collected. Recording
  cannot share it this way and will take the audio by value.

### Memory

- **One figure covers all audio-length memory.** The caller makes one
  `AudioMemory` per machine and shares it as a cloneable handle; loaded
  samples, audio being written and later effect state all count against
  it. A per-sample length bound was the earlier plan and was rejected: the
  worst case was a multiple that depended on the operation.
- **`AudioMemory` makes every `AudioBuffer`, and a buffer is the only
  audio-length memory.** `alloc` checks the figure and returns a buffer
  of fixed capacity that repays on drop, so release cannot be forgotten
  or doubled. Buffers never grow. Step 5 held a `Reservation` beside an
  ordinary `Vec`; the memory port made that one `Block`; on 2026-11-13 the
  block was merged into `AudioBuffer`, its only user, and the `Ledger`
  became `AudioMemory`.
- **Who may allocate is who holds an `AudioMemory`:** the caller, and what
  it lends one to for a call (`Project::load`). The engine holds none; the
  render side never allocates. A parameter, not a global, because the
  signature is the permission and tests need their own budgets.
- **Track what scales with audio; bound the rest.** Fixed overhead (the
  manifest, the rings, the renderer's scratch and its voice pool) is not
  counted; it is sized once by the caller. Counting the old input rings
  was tried and reverted as one mechanism too many.
- **The OS backs the figure.** Audio is ordinary heap memory; the
  guarantee is a cgroup reservation, no swap, and a budget set below what
  is free. `AudioMemory` makes sure the crate never asks for more than
  that one number. A locked arena was considered and left for later.
- **Live effects will allocate on the control side** at instantiation and
  move to the renderer by command.
- **Prefault what the audio thread writes first.** Fresh memory is
  reserved, not backed: the first write to each page stops the thread
  while the kernel finds one. Memory the control side fills (loaded audio,
  the renderer's pool) is backed before the audio thread sees it; memory
  the audio thread is first to write (a recording, later a delay line) is
  written once at allocation, on the control side. Nothing does that
  today: `Block::prefault` went with the old recorder and returns with
  recording. Prefaulting everything was rejected as work with nothing to
  gain.
- **Review rule:** every new audio-length allocation comes from
  `AudioMemory`, or the figure is a lie; one the audio thread fills is
  prefaulted.

### The two sides

- **The project and the engine never name each other.** The disk side is
  `Project` (`open`, `load`, `add_sample`). The sound side is
  `engine::create(command_capacity, voices) -> (Control, Renderer)`: the
  renderer moves to the audio thread, `Control` stays (`play`, `stop`,
  `stop_all`, `collect`). What crosses is `Audio`, carried by the caller.
  Until 2026-11-10 an `Engine` type was the only way in and every verb
  was an engine method taking an id; no method needed both sides, so it
  was dissolved, and a joint constructor was rejected as the engine again.
- **A verb lives on the thing it acts on.** `voice.stop()` and
  `voice.state()` act on one playing; `control.stop(&audio)` and
  `stop_all()` act across voices, so they live on `Control`. A handle
  holds the renderer's shared rings, never a `Control`, so the
  dependency runs one way. Audio carrying a handle and sending its
  removal from `Drop` was tried and rejected as the wrong coupling.
- **`Control` is the door to one renderer.** It holds the control side's
  ends of both rings behind one lock (`SharedRings`), so every push
  follows a drain of the ring back. Clones are the same renderer. Control
  side only: nothing on the audio thread holds one. Named for what it is
  to the caller; `RendererHandle`, `Link` and `Remote` were rejected.
- **Commands are optimistic; the audio thread is the authority on state.**
  A send succeeds or reports the ring full; what then happened reaches the
  handle through a report the renderer fills with one atomic store
  (`VoiceReport`), not through a ring. The ring back only carries the
  audio thread's last references, to be freed on the control side, and
  the rings code may only drop what comes back or hand it to the
  feature's control module.
- **Loaded audio is plain data.** Hold `Audio` and it is in RAM; drop it
  and its voices play out, its memory repaid once the last one's return
  is collected. `control.stop(&audio)` cuts them first. Each load is its
  own copy; no dedup. What to load at open is the caller's to remember: a
  persisted `loaded` flag was built and removed (it recorded a preference
  before any panel had one).
- **Adding a sample and loading it are separate.** A print must land on
  disk even when it does not fit in RAM.
- **Loading is synchronous** on the control thread. If it ever visibly
  stalls a panel, the fix is a worker thread, not async methods.
- **Ids come from process-wide counters,** as `std::thread::ThreadId`
  does: unique across every engine, in order, never reused. They are
  minted before the send, so a refused command burns its id; nothing
  reads the gaps.
- **The source tree is the architecture.** At the top: `config` and
  `layout` (the fixed format), `audio` (`AudioMemory`, `Audio`) and
  `project`. Under `src/engine/`: `protocol/` (what crosses), `control/`
  (the control thread: `Control`, `Voice`) and `realtime/` (the audio
  thread's `render/`, and `capture/`, empty until inputs). `control/` and
  `realtime/` never import each other; both import `protocol/`.

### The render side

- **The render side never waits.** It owns the receiving end of an `rtrb`
  ring (lock-free SPSC, fixed capacity; `mpsc` locks and allocates) and
  applies commands at the start of each period. No disk, no clock, no
  allocation, no lock, no error path. It reads only audio already in RAM:
  an fsync on eMMC can stall I/O for hundreds of milliseconds.
- **Real-time safe, not real-time only.** Frame counts, never a clock, so
  the same `render` drives a device callback live or a loop offline (the
  bounce).
- **Mix in blocks.** `render(&mut [Pcm])` takes the device's flat
  interleaved period, of any length. It mixes it in blocks of
  `BLOCK_FRAMES` (64): each block starts silent, each voice renders into
  one shared scratch and is added in, and the period is hard-clipped
  once. Blocks keep the scratch fixed and are what Pd, SuperCollider and
  Max do; the latency is the device's period, not the block. The mix is
  always stereo (`Renderer::CHANNELS`): mono audio plays at full level on
  both sides. The old engine's `Source` trait, `Gain` and `Sounds` were
  not carried over, and before them a `Processor` trait with boxed
  chains and a `Mixer<S, N>` were built and removed.
- **Voices are a fixed pool.** `voices` is given to `create` and the pool
  never resizes. A play with every place taken cuts the oldest: a press
  must sound. The pool picks which voice to cut; the renderer ends it and
  reports why.
- **Every voice ends one of three ways, and says which:** `Finished`
  (played to its last frame), `Stopped` (a stop arrived) or `Cut` (its
  place was taken). `Cut` is the sampler's umbrella: choke groups and
  retrigger will end voices the same way.
- **Only plain things cross, both ways.** A play carries a `VoiceSpec`:
  the id, the audio's `Arc<AudioBuffer>` and the report. When the voice
  ends, the audio and report go back as a `Return`, so the last reference
  is dropped on the control side.
- **The ring back never fills.** Every send drains it first, so between
  drains it holds at most every voice the renderer holds plus one return
  per queued command; `create` sizes it at that. Freeing a large buffer
  is a trip into the allocator and the kernel with no bound on its time,
  so it never happens on the audio thread.
- **Commands are low level.** Operations on what the renderer holds,
  never intents: `VoiceCommand` is `Add(VoiceSpec) | Remove(VoiceId) |
  RemoveAudio(BufferAddress) | RemoveAll`. Meaning lives on the
  control-side handles. One command type per feature, wrapped in
  `Command`. High-level commands (`Monitor`, `Play`) came first.
- **Stop by buffer address.** Stopping every voice of some audio sends the
  buffer's address; the renderer ends every voice whose `Arc` points at
  it and never learns a sample id, which would name every load of a
  sample and collide across projects. The caller still holds the audio
  when the command is sent, so the address cannot name a later buffer.
- **One concept, one name; the module says the side.** `Voice` is both
  the control-side handle and the render side's playing, as `io::Error`
  and `fmt::Error`. `Return`, not `Released`, which clashes with an
  envelope's release. The old engine said `Sound` and `SoundId`.
- **Clip once, hard, last.** Same result on every device and offline; it is
  what hardware samplers do. A master gain or limiter goes before it.
- **Bounded polyphony.** The voice count is a CPU budget the caller sizes
  for its hardware; the crate keeps no hardware maximums. It is not
  counted by `AudioMemory`, which counts only audio-length memory.

### Input and monitoring

Built in step 7 on the old engine and removed with it (PR #57); the old
design is in the history before 42231f2. To be rebuilt on `src/engine/`,
where inputs bring the capture thread: `realtime/capture/` is its place.
cpal has no duplex stream, so the capture and audio clocks drift, and an
input's ring must be freed on the control side like any other return.

### Devices and examples

- **The device is the caller's, not the crate's.** Which machine, which
  audio library, whether there is a device at all: the library has no
  audio dependency, `cpal` is a dev-dependency, and `examples/device/` is
  the only code that knows there is an audio thread. A headless machine
  runs the engine all the same. A trait over cpal was rejected: one
  implementation, and tests hold the renderer directly.
- **The caller is a patch bay; the engine is the mixer.** Which device and
  channel is the caller's, thin and decision-free; what is heard and at
  what level is the engine's, by command.
- **The stream runs from open to drop.** Opening clicks and costs tens of
  milliseconds, so it renders silence when idle. Audio can be dropped in
  any order: the last `Arc`, held by a voice, comes back on the ring back
  to be freed on the control side.
- **The examples are `first_sound` and `engine`**, plain demonstration
  code: no hardening or shared abstractions there. `first_sound` runs on a
  device; `engine` shows every engine feature and compiles, and each new
  feature is added to it in the same step. When the device exists, they
  move into a binary crate.

### Recording

Built in step 6, removed in the memory port (PR #40), rebuilt for the main
out in step 8, and removed again with the old engine (PR #57). To be
rebuilt on `src/engine/`, main out before inputs: it stays on the two
threads there are and forces the return-path decision inputs also need.
How a recording gets its buffer from `AudioMemory` is open.

- **How a take reaches its handle is undecided.** A voice's outcome
  reaches its handle through a shared report: one atomic byte the
  renderer stores and the handle loads, with no ring. A take could follow
  the report pattern with a write-once `Slot<T>` behind an `Arc`: the
  renderer moves the buffer in (wins EMPTY -> WRITING, publishes READY
  with Release), the handle takes it out once (wins READY -> TAKEN with
  Acquire), and the renderer's `Arc` goes back on the ring back, so
  nothing is freed on the audio thread. A byte can be copied by both
  sides; a buffer must move once, hence the cell, and `take()` returns it
  once rather than reading it repeatedly. The cell needs `unsafe` (an
  `UnsafeCell` and `unsafe impl Sync`), but only inside `Slot`: its
  fields are private and `put`/`take` are sound in any order (a second
  put hands the value back), so its `// SAFETY:` comments guard edits to
  `Slot`, not its callers. Check it with Miri and loom. Ruled out:
  `oneshot`, whose sender frees the channel when the receiver is gone,
  which would be on the audio thread; `OnceLock`, which only lends the
  value. The alternative: the take rides the ring back and collect hands
  it to the recording feature, with no new primitive but delivery only at
  a collect.

### Process

- **One product, opinionated, with clean seams.** Build what this sampler
  needs when a step needs it; keep the seams that testing and the hardware
  require (no device in the crate). The renderer pair is justified by the
  bounce and step 8; a many-renderer API is not.
- **The public surface is minimal.** Convenience methods arrive with a
  caller.
- **Nothing is settled by having been written.** New steps are designed
  from the decision, and existing code is changed to fit.

## Step 8: the real-time side

Built on `src/engine/`: voices, block mixing and the ring back (see the
render side, above). The old engine's mixer, release ring and main-out
recorder went with it in PR #57.

What is left:
- **Recording** (see Recording, above): the main out first, then inputs.
- **Inputs and monitoring** (see above), with the capture thread.
- **Sample-accurate commands** arrive with sequencing as a frame offset on
  the command.

## Storage tiers on the device

```
SD card   kept projects; a copy of the active one, made on request
eMMC      the active project; every change lands here at once
RAM       loaded audio, under AudioMemory
```

The project does not know about tiers.

## Deferred ideas

- **A mono output device** folds the stereo mix at the very end; the mix
  itself stays stereo. The fold's level (a plain sum, or -3 or -6 dB) is
  undecided.
- **Effects on the render side.** Heard live on a sound or a bus; a print
  is the committed offline render of the same effect, stored as a new
  generation.
- **Replayable recipes** on prints, as presets. Needs effects-as-data.
- **Offline bounce:** a renderer driven in a loop with frame-stamped
  commands, its output stored as a new root.
- **Real-time rulebook** (a doc) and an **allocation-counting test
  allocator** asserting `Renderer::render` allocates nothing.
- **Taps and busses** on the hard-coded path.
- **Master gain and bus limiter** before the clip.
- **Stream errors** reported to a front panel.
- **Content-hash dedup** on add, and on import if it returns, also
  matching a renamed WAV back to its sample.
- **Removing a whole lineage** as one operation.
- **A SQLite medium**, a trait over the project's files again, if a
  single-file project or transactions are worth it.
- **Streaming long recordings** from eMMC instead of holding them
  resident.
- **A budget read from the device's configuration** once the CM4 variant
  is known.
- **An `i16` build** for low-power hardware, chosen at build time.
- **Projects in another stored format** than the engine's, converted on
  load and saved in the engine's.
- **Warming the small rings.** The ring back's slots are first written
  by the renderer and an input ring's by the capture thread, so
  each faults a few pages on its first pushes. rtrb has no hook to
  prefault; pushing and popping a value through every slot at creation
  would do it.
- **`mlockall` on the device.** `mlockall(MCL_CURRENT | MCL_FUTURE)`
  backs and locks every page the process has or will have, the blanket
  form of prefaulting, and guarantees nothing is paged out later. It
  needs `CAP_IPC_LOCK` or a raised `RLIMIT_MEMLOCK` and is a
  process-wide choice, so it is the caller's, beside the cgroup
  reservation and no swap; the crate does not own the process.
- **A level meter on a take**, an atomic the panel reads; the same
  atomic could report a take's progress.
- **Saving a take as loaded audio**, a copy into a right-sized buffer,
  for playing a resample at once without reading it back from disk.
- **A `stereo` fast path** for stereo-as-stored, after a measurement.
- **Hardware monitoring** on the target's codec, if it has it.
