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
| 5 | RAM ledger: one figure for everything the engine holds in memory | done |
| 6 | Recording: audio input as a new root | built, then removed in the memory port; main out rebuilt on the renderer in step 8, inputs to come |
| 7 | Input monitoring: hear the input through the output | done |
| 8 | Sequencing and mixing: the real-time side | mixing and main-out recording done; sequencing to come |
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

- **The project is the disk.** `Project` holds a `Storage` and a `Ledger`
  and no in-memory copy of the project, not even a cache. Every change is
  durable when the call returns, so there is no save step; on the device
  the active project lives on eMMC and a "save" is a copy to the SD card.
  The first model kept everything in RAM and treated persistence as a
  dump; step 3 replaced it.
- **The project holds the rules, storage is dumb.** The project enforces
  what may change (that a parent exists, once lineage returns); storage
  answers questions and does what it is told. Rules in storage would be
  repeated in every backend and drift.
- **Storage mints sample ids.** A sample does not exist until it is
  stored, like a database insert returning the row id. The counter is on
  disk, only goes up, and never reuses an id.
- **A sample is named by its `SampleId`; a sound is not.** A sample
  outlives the program, so callers refer to it by an id they can keep and
  store (a parent pointer, pads and patterns later), and
  `load(id)` turns that into a `Sample` the way a path opens a file. A
  sound lives only while it plays, so its handle keeps its `SoundId`
  private and carries the verbs instead.
- **Generation is a relationship, not a type.** A sample will be an id
  and an optional parent id; lineage is the tree of parent pointers. It
  was built in step 3 and removed after step 8, because nothing had made
  a child since prints went in step 5. It returns with prints: `parent`
  joins the crate-private `SampleMetadata` that `Storage::add_sample`
  writes beside the audio (empty until then), goes in the manifest's
  per-sample `Entry` with `#[serde(default)]`, and gets its rule on
  `Project`; `metadata(id)` comes back to read it. Other fields join the
  sample the same way as steps need them. (A `loaded` flag was the first
  and was removed under Handles. Removing samples was built and cut in
  the memory port: no caller.)
- **"Project"** is what every sampler calls the unit a user opens and
  copies to a card. It will hold patterns, settings and pad layouts too.

### Storage

- **A trait, so the medium can change.** Files now, perhaps SQLite later.
  The trait is the questions a backend answers efficiently and the writes
  it must make durable; rules stay above it.
- **A storage always holds a whole project.** `open(dir)` creates an empty
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
  private struct in the backend, not a serde derive on the model, so the
  file format is versioned on its own. WAV rather than raw bytes so any
  audio tool can open a sample.
- **Nothing is cached in the backend.** The OS caches the file and nothing
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
  ledger could not see.

### Audio in

- **Audio enters a project by being written.** `project.alloc(layout,
  frames)` hands out an empty `Sample`, paid for in full against the
  budget; `sample.write()` lends its `AudioBuffer` to fill; and
  `project.add_sample(&sample)` stores it and mints its id. Nothing is
  stored and no id exists until the add. `alloc` is named for what it
  does: it takes memory and touches no disk.
- **WAV import was removed 2026-11-13** (the `decode` and `convert`
  stages, `rubato`, `Project::import`); it is in the history before
  97e8eff if a file format is needed again.
- **The rule is that every byte of audio in RAM is counted, not that the
  user never touches audio.** `AudioBuffer` is public and readable; only
  the crate can make one, so every buffer still comes from the ledger.
- **One type is written, played and saved.** `Sample` is an
  `Arc<AudioBuffer>` with no `Clone`. `write()` is `Arc::get_mut`: `None`
  while the engine holds the audio, which is what `Sample` protects; a
  read by the project is protected by an ordinary borrow. A voice that
  has ended still holds it until its return is collected. Recording
  cannot share it this way and will take the sample by value.

### Memory

- **One figure covers all audio-length memory.** `Ledger::new(budget)` is
  made by the caller, one per machine, and shared as a cloneable handle.
  Resident samples, samples being written and later effect state all count
  against it. A per-sample length bound was the earlier plan and was
  rejected: the worst case was a multiple that depended on the operation.
- **The ledger hands out `Block`s, and a block is the only way to get
  audio-length memory.** `Ledger::alloc` checks the figure and returns a
  fixed-capacity buffer that repays on drop, so release cannot be
  forgotten or doubled. Blocks never grow. Step 5's shape, a `Reservation`
  held beside an ordinary `Vec`, was replaced in the memory port: two
  things to keep in step became one.
- **Who may allocate is who holds a `Ledger`.** The control side does; the
  render side never does, so it cannot allocate by mistake. The ledger is
  a parameter, not a global, because the signature is the permission and
  tests need their own budgets.
- **Track what scales with audio; bound the rest.** Fixed overhead (the
  manifest, input rings) stays off the ledger, with a
  worst case computed at compile time where it matters
  (`input::MAX_RING_BYTES`). Ledgering the input rings was tried and
  reverted as one mechanism too many.
- **The OS backs the figure.** Resident audio is ordinary heap memory; the
  guarantee is a cgroup reservation, no swap, and a budget set below what
  is free. The ledger makes sure the engine never asks for more than that
  one number. A locked arena was considered and left for later.
- **`Control` holds a ledger too.** `Renderer::new(settings, ledger)`
  hands a clone to `Control`, which pays for what it builds and sends to
  the renderer: a recorder's buffer now, effect state later. The renderer
  only passes it on. Held, not passed per call, as `Project` holds its
  own and as an allocator is held by what allocates from it.
- **Live effects will allocate on the control side** at instantiation and
  move to the renderer by command.
- **Prefault what the audio thread writes first.** A fresh block is
  reserved, not backed: the first write to each page stops the thread
  while the kernel finds one. Memory the control side fills (a loaded
  sample, the renderer's places) is backed before the audio thread sees
  it; memory the audio thread is first to write (a recorder's buffer,
  later a delay line) is written once at allocation with
  `Block::prefault`, on the control side. Prefaulting everything was
  rejected as work with nothing to gain.
- **Review rule:** every new audio-length allocation goes through
  `Ledger::alloc`, or the figure is a lie; one the audio thread fills
  is prefaulted.

### The two sides

- **The engine is two sides that never name each other.** The disk side is
  `Project` (`open`, `alloc`, `load`, `add_sample`). The sound side is
  `Renderer::new(settings, ledger) -> (Renderer, Control)`: the renderer
  moves to the audio thread, `Control` stays (`play`, `stop`, `stop_all`,
  `input`, `recorder`). What crosses is a `Sample` one way and a `Take`
  the other, carried by the caller. Until
  2026-11-10 an `Engine` type was the only way in and every verb was an
  engine method taking an id; no method needed both sides, so it was
  dissolved, and a joint constructor was rejected as the engine again.
- **A verb lives on the thing it acts on.** `sound.stop()`,
  `input_sound.monitor()`. What the renderer hands out carries a `Control`
  clone and acts on its own; what the project hands out is data the
  `Control` takes by reference. A sample carrying a handle and sending its
  eviction from `Drop` was tried and rejected as the wrong coupling.
- **`Control` is the shared sending end.** A mutex around the command
  ring's producer behind an `Arc`, with the sound and take id counters,
  the count of input sounds held, whether the recorder exists, and the
  ledger. Control side only: nothing on the audio or capture thread
  holds one. It is the only code that builds a `Command`. Named for what it
  is to the caller; `RendererHandle`, `Link` and `Remote` were rejected.
- **A loaded sample is plain data.** `Sample` is an id and an
  `Arc<AudioBuffer>`. Hold it and the sample is in RAM; drop it and its
  sounds play out, the block repaying when the last one ends.
  `control.stop(&sample)` first cuts them. Each load is its own copy; no
  dedup. What to load at open is the caller's to remember: a persisted
  `loaded` flag was built and removed (it recorded a preference before any
  panel had one).
- **Adding a sample and loading it are separate.** A print must land on
  disk even when it does not fit in RAM.
- **Loading is synchronous** on the control thread. If it ever visibly
  stalls a panel, the fix is a worker thread, not async methods.
- **A refused command burns its id.** Ids are minted by an atomic add
  before the send; preserving gap-free ids across many senders needed
  closures under the lock, and nothing reads the gaps.
- **Drop is best effort.** A drop cannot fail, so an `Input` dropped onto
  a full ring leaves its readers to finish when the feed drops and keeps
  its sounds counted as held. The command ring cannot be sized to make
  that impossible (the control side is unbounded), so it is sized not to
  happen in practice (`RendererSettings::command_capacity`).
- **The source tree is the architecture.** `src/project/` (disk side:
  `Project`, `storage/`, `ledger.rs`), `src/control/` (the
  control thread: `Control`, `SampleSound`, `Input`, `Recorder`),
  `src/render/` (the audio thread: `Renderer`, `command.rs`, `Sounds`,
  `dsp/`). Shared at the top: `config`,
  `audio` (`AudioBuffer`, `Sample`). `render/` imports neither of the others.

### The render side

- **The render side never waits.** It owns the receiving end of an `rtrb`
  ring (lock-free SPSC, fixed capacity; `mpsc` locks and allocates) and
  applies commands at the start of each period. No disk, no clock, no
  allocation, no lock, no error path. It reads only audio already in RAM:
  an fsync on eMMC can stall I/O for hundreds of milliseconds.
- **Real-time safe, not real-time only.** Frame counts, never a clock, so
  the same `render` drives a device callback live or a loop offline (the
  bounce).
- **`dsp/` is generic pieces; the renderer is this product's pipeline.**
  `dsp/` holds the `Source` trait (`frames(n) -> impl Iterator<Item =
  Frame>`, `finished()`), `SampleReader`, `InputReader`, `Gain`, a
  `Recorder` that keeps frames, and `math` (`stereo`, `add`,
  `hard_clip`, `as_frames`), and knows no commands. The
  `Renderer` holds samples and inputs apart, each a `Sounds<Sound<_>>`,
  where `render::Sound` is a source and its `Gain`. It zeroes the period,
  adds every sound into it, and hard-clips once. The pipeline is
  hard-coded; no runtime graph. A `Processor` trait with boxed chains, a
  plain-data-plus-free-functions shape and a `Mixer<S, N>` that owned the
  sources were all built and removed.
- **Only plain things cross, both ways.** A command carries what a sound
  is made from (a sample's `Arc<AudioBuffer>`, an input's ring), never a
  dsp piece; the renderer builds the piece from it, which only moves it,
  and takes it apart again so `Released` carries the same plain things
  back. The recorder's buffer goes the same way, out on `Record` and
  back as a take. `command.rs` imports nothing from `dsp/`.
- **Mix by accumulating into the caller's slice.** Each sound adds its
  period straight into the output with `math::add`; nothing has a scratch
  buffer and the iterators allocate nothing. The internal mix is always
  stereo (`Renderer::CHANNELS`), so effects can count on two channels and
  every device and bounce hears the same.
- **The renderer is sized per device.** `RendererSettings` (period,
  command capacity, sample sounds, input sounds) is given to
  `Renderer::new`; `RendererSettings::new(period)` sets the rest to the
  `Renderer::MAX_*` bounds. A laptop, a tablet and a CM4 each tune it.
  `Sounds` is a boxed slice of places allocated once and never resized.
- **What to do when full is the renderer's.** A sample add with every
  place taken cuts the oldest (a press must sound); an input add is
  refused by `Control` first and dropped by the renderer as a backstop (an
  input must never be cut).
- **Commands are low level.** Operations on what the renderer holds, never
  intents: `SoundCommand<S>` is `Add { id, source, gain } | Remove(id) |
  Set { id, param: SoundParam }`, wrapped as `Command::Sample` or
  `Command::Input`, plus `StopAllSamples`, `Evict`, and for recording
  `Record { id, tap, audio }` and `StopRecording(id)`. Meaning (monitoring,
  a level) lives on the control-side handles. A stop-all must not mute
  the microphone. High-level commands (`Monitor`, `Unmonitor`, `Play`)
  came first.
- **A level ramps.** `Gain` moves to a new level linearly over the next
  period, so a change never clicks.
- **`SoundId` names one thing heard**, a sample played once or one input
  sound, minted by `Control` before the send because the ring is one way.
  The lowest id is the oldest, which is what cutting needs. It and
  `BufferAddress` live in `render/command.rs`: they are how the two sides
  name things to each other. (Was `VoiceId`.)
- **One concept, one name; the module says the side.** `SampleSound` and
  `InputSound` are the handles, `render::Sound` the same sound on the
  audio thread, as `io::Error` and `fmt::Error`. No `Handle` suffix;
  "voice", "slot" and "strip" were rejected.
- **Evict by buffer address.** Stopping every sound of a sample sends the
  buffer's address; the renderer drops every sample sound whose `Arc`
  points at it and never learns a sample id, which would name every load
  of a sample and collide across projects. The caller still owns the
  buffer when the command is sent, so the address cannot name a later
  buffer.
- **Clip once, hard, last.** Same result on every device and offline; it is
  what hardware samplers do. A master gain or limiter goes before it.
- **Bounded polyphony.** The sound counts are a CPU budget with a known
  worst case (`Renderer::MAX_*`), tuned down per device; they are not on
  the RAM ledger, which counts only audio-length memory.

### Input and monitoring

- **An input is the ability to put audio in; monitoring is whether it is
  heard.** `control.input(layout, period) -> (Input, InputFeed)`. The
  `InputLayout` is the device's channels as `ChannelLayout::{Mono,
  Stereo}` entries; each entry is an `InputSound` with its own ring and
  its own place in the renderer, silent until `monitor()`. The first version added and heard in one call;
  split because an input will also be recorded and routed.
- **Shaped like the device, split on the capture side.** `InputFeed::feed`
  takes the device's interleaved frames and splits them into the sounds'
  rings in one pass, on the capture thread, with no lock. Five shapes came
  first, all on 2026-11-10: a fixed capture width, a width plus channel
  map, mono/stereo-only inputs with the caller splitting, and one ring per
  device with sources inside the reader (which hid a second monitor switch
  the renderer could not see). A ring per entry makes each an ordinary
  sound.
- **Monitoring is gain.** `monitor()` sets the sound's gain to 1,
  `unmonitor()` to 0, ramped. A silent sound still runs, so an unheard
  input keeps up with its ring. A mute that `skip`ped the period came
  first.
- **Two clocks, one ring, a lag rule.** cpal has no duplex stream, so the
  rings drift. The reader takes one period per render; an empty ring is a
  gap; more than `MAX_LAG_PERIODS` queued is skipped, oldest first. The
  ring depth is a few capture periods; the period is a parameter (bounded
  by `MAX_PERIOD_FRAMES`) because it is the device's to know. A fixed 4096
  frame depth came first and was right by luck.
- **Dropping the `Input` removes it.** It sends the removes and stops
  counting its sounds as held. Drop the `Input` before the `InputFeed` so the rings are freed on
  the caller's thread. An `InputId` and `remove_input` existed until
  Handles.
- **Numbered inputs are not the engine's.** The panel holds the input
  sounds and labels them.

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
  milliseconds, so it renders silence when idle. Samples can be dropped
  in any order: the last `Arc`, held by a sound, goes back on the release
  ring to be freed on the control side.
- **The examples are `first_sound`, `monitor` and `resample`**, and are plain
  demonstration code: no hardening or shared abstractions there. When the
  device exists, they move into a binary crate.

### Recording

Built in step 6, removed in the memory port (PR #40), and rebuilt on the
renderer in step 8 for the main out. Recording an input waits until it
is needed.

- **A tap is where a recorder records:** a point in the signal path whose
  frames are kept as they go on to where they were going. `Tap::MainOut`
  is after the clip, exactly what is heard, which is resampling, and is
  the only tap. An input and a bus come later as more variants. Where to
  record is data on one `Record` command, not a command per place.
- **One recorder, many takes.** `control.recorder(max_frames)` pays
  for one stereo buffer from the ledger and returns a `Recorder` handle;
  starting a take allocates nothing, so a press records at once. Every
  page is written when the recorder is made (`Block::prefault`), because
  a fresh block is only reserved and the first write to each page would
  otherwise fault on the audio thread. The buffer is held for as long as the handle
  is, even between takes. A buffer per take was the earlier plan, and
  was rejected because the allocation sat between the press and the
  first frame.
- **The length is a maximum.** `stop()` ends a take; reaching the end of
  the buffer is a normal end too, not an error. Lengths are frames: the
  panel converts from seconds or bars, and only it knows the tempo.
- **One recorder, one take at a time, the tap chosen per take.** Like a
  hardware sampler's one recording path: `record(tap)` says what each
  take records, so switching sources costs nothing. `Control` refuses a
  second recorder (`RecorderInUse`) and a second take (`Recording`); the
  renderer ending the take already running is the backstop. A recorder
  per tap came first (`TapInUse`), and a place per input sound was
  sketched for recording inputs; both were dropped on 2026-11-12 until
  multitrack recording is shown to be needed.
- **The buffer moves, it is not shared.** `record(tap)` moves the
  `AudioBuffer` into `Record`, so while the renderer writes, nothing else
  can see it, without `unsafe` or a lock; an `Arc` would have made it
  read-only. `AudioBuffer` gained crate-private whole-frame writes for
  its owner; shared, as a loaded sample is, it never changes.
- **The renderer builds the `dsp::Recorder`** around the buffer when the
  take starts, and takes it apart when the take ends, so the control side
  never names a `dsp` type. It holds the one take as an
  `Option<(Tap, TakeId, Recorder)>`. A `Recorders` table with a place per
  tap came first and went with the recorder per tap. Sample and input
  sources are built on the render side the same way.
- **A take comes back, not a recorder.** When it ends, the buffer goes
  back on the release ring as `Released::Take { id, audio }`; `Control`'s
  collect keeps takes by `TakeId` for their owner instead of dropping
  them. The audio thread cannot call anyone, so the owner finds out by
  asking: `is_recording()` and `last_take()` poll, from a panel's tick.
  Dropping the handle mid-take marks the take abandoned, and collect
  drops it on arrival.
- **In `src/engine/`, how a take reaches its handle is undecided.** A
  voice's outcome reaches its handle through a shared report: one atomic
  byte the renderer stores and the handle loads, with no ring. The return
  ring only carries things back to be freed. A take could follow the
  report pattern with a write-once `Slot<T>` behind an `Arc`: the
  renderer moves the buffer in (wins EMPTY -> WRITING, publishes READY
  with Release), the handle takes it out once (wins READY -> TAKEN with
  Acquire), and the renderer's `Arc` goes back on the return ring, so
  nothing is freed on the audio thread. A byte can be copied by both
  sides; a buffer must move once, hence the cell, and `take()` returns it
  once rather than reading it repeatedly. The cell needs `unsafe` (an
  `UnsafeCell` and `unsafe impl Sync`), but only inside `Slot`: its
  fields are private and `put`/`take` are sound in any order (a second
  put hands the value back), so its `// SAFETY:` comments guard edits to
  `Slot`, not its callers. Check it with Miri and loom. Ruled out:
  `oneshot`, whose sender frees the channel when the receiver is gone,
  which would be on the audio thread; `OnceLock`, which only lends the
  value. The alternative is the path above: the take rides the return
  ring and collect hands it to the recording feature, with no new
  primitive but delivery only at a collect.
- **No handle per take.** The recorder holds one take at a time, so its
  state is the take's; a `Take` handle borrowing the recorder would make
  the panel's state self-referential.
- **A take is not a sample until it is saved.** `last_take()` lends the
  finished `Take`, which lasts until the next `record(tap)`, and
  `project.save_take(&take)` stores the frames it kept as a new root
  through `add_sample`. To play it, load the id:
  playing shares a buffer, and the recorder's must stay its own.
  Returning a loaded `Sample` from the save was considered and left until
  a caller needs it.
- **A take has no error path;** device failures are the device's to
  report.

### Process

- **One product, opinionated, with clean seams.** Build what this sampler
  needs when a step needs it; keep the seams that testing and the hardware
  require (storage trait, no device in the crate, generic dsp pieces). The
  renderer pair is justified by the bounce and step 8; a many-renderer API
  is not.
- **The public surface is minimal.** Convenience methods arrive with a
  caller.
- **Audio never leaves the engine.** No method returns a sample's buffer;
  a copy outside would be memory the ledger does not count. A waveform
  display gets min/max per column, sized by the screen.
- **Nothing is settled by having been written.** New steps are designed
  from the decision, and existing code is changed to fit.

## Step 8: the real-time side

The mixer rewrite is built (see the render side, above): accumulate
mixing over a `Source` trait, a `Gain` per sound, low-level commands and
per-device `RendererSettings`.

The release ring is built. Every sound the renderer lets go of
(finished, removed, stolen, evicted, or an add refused) goes back over a
second ring as a `Released`, and `Control` drops what is there whenever
its lock is taken, so at every send, or when its owner calls
`Control::collect`, for when it has gone quiet or before a load that
needs the budget back. Sized at the sound counts, one recorder and the
command capacity, it cannot fill between two sends. Freeing a large buffer is a trip into the allocator
and the kernel with no bound on its time, so the last free of a sample's
buffer and an input's ring is off the audio thread. Dropping a `Sample`
while it plays still lets the sound ring out; the audio is freed at the
first send or collect after it ends, and the ledger counts it until
then.

Recording the main out is built (see Recording, above). The routing
worry that came before it, that a ring has one consumer, went away with
the mixer rewrite: the renderer reads each source once a period and can
send the frames wherever it likes, so routing is where the renderer puts
frames it already has.

What is left:
- **Recording an input,** when it is needed: a `Tap::Input(SoundId)`
  variant, read before the gain so an unmonitored input still records.
  Open: how the frames before the gain reach the recorder (a callback
  inside `Sound`, or a scratch buffer); whether a mono input records as
  stereo, twice the memory, or in its own layout; ending the take when
  its input is removed; and `Control` refusing a tap on an input sound
  it does not hold, which needs it to know the ids, not just a count.
- **Sample-accurate commands** arrive with sequencing as a frame offset on
  the command.

## Storage tiers on the device

```
SD card   kept projects; a copy of the active one, made on request
eMMC      the active project; every change lands here at once
RAM       loaded samples, under the ledger
```

The storage layer does not know about tiers.

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
- **A SQLite backend**, if a single-file project or transactions are worth
  it.
- **Streaming long recordings** from eMMC instead of holding them
  resident.
- **A budget read from the device's configuration** once the CM4 variant
  is known.
- **An `i16` build** for low-power hardware, chosen at build time.
- **Projects in another stored format** than the engine's, converted on
  load and saved in the engine's.
- **Warming the small rings.** The release ring's slots are first
  written by the renderer and an input ring's by the capture thread, so
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
- **Saving a take as a loaded sample**, a copy into a right-sized block,
  for playing a resample at once without reading it back from disk.
- **A `stereo` fast path** for stereo-as-stored, after a measurement.
- **Hardware monitoring** on the target's codec, if it has it.
