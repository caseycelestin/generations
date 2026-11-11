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
| 6 | Recording: audio input as a new root | built, then removed in the memory port; to be rebuilt |
| 7 | Input monitoring: hear the input through the output | done |
| 8 | Sequencing and mixing: the real-time side | |
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
  that a parent exists; storage answers questions and does what it is
  told. Rules in storage would be repeated in every backend and drift.
- **Storage mints sample ids.** A sample does not exist until it is
  stored, like a database insert returning the row id. The counter is on
  disk, only goes up, and never reuses an id.
- **Generation is a relationship, not a type.** A sample is an id and an
  optional parent id; lineage is the tree of parent pointers. `sample(id)`
  returns what is stored about a sample except its audio; `load(id)`
  returns the audio. Metadata fields join the sample as steps need them,
  each with its own setter and rule. (A `loaded` flag was the first and
  was removed under Handles. Removing samples was built and cut in the
  memory port: no caller.)
- **"Project"** is what every sampler calls the unit a user opens and
  copies to a card. It will hold patterns, settings and pad layouts too.

### Storage

- **A trait, so the medium can change.** Files now, perhaps SQLite later.
  The trait is the questions a backend answers efficiently and the writes
  it must make durable; rules stay above it.
- **A storage always holds a whole project.** `open(dir)` creates an empty
  project in an empty or missing directory, validates a whole project
  otherwise, and refuses anything else. Validation happens on open, not
  per call: the manifest reads, every parent exists and was minted before
  its child (which rules out cycles without a graph walk), the counter is
  past every id, every WAV is present in the engine's format.
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

### Import

- **The project takes the file.** `Project::import(path)` decodes,
  converts and stores; the caller never sees a decoder or the project
  format. File formats are product knowledge, the same on every machine,
  unlike the device.
- **Two crate-private stages, one entry point.** `decode` reads a file to
  rate, channels and `f32` samples; `convert` resamples to the project
  rate. Neither knows the other's job. One decoder (WAV, integer 8–32 bit
  and 32-bit float), so no format sniffing or decoder trait until a second
  format arrives.
- **Resampling is `rubato`'s synchronous FFT resampler** over the whole
  clip, writing into a ledger-paid buffer. Audio already at 48 kHz is
  wrapped without a copy.
- **Channels are kept, not mixed.** Mono stays mono, stereo stays stereo,
  more than two is refused; a downmix is the user's decision.

### Memory

- **One figure covers all audio-length memory.** `Ledger::new(budget)` is
  made by the caller, one per machine, and shared as a cloneable handle.
  Resident samples, import buffers and later effect state all count
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
  manifest, input rings, resampler state) stays off the ledger, with a
  worst case computed at compile time where it matters
  (`input::MAX_RING_BYTES`). Ledgering the input rings was tried and
  reverted as one mechanism too many.
- **The OS backs the figure.** Resident audio is ordinary heap memory; the
  guarantee is a cgroup reservation, no swap, and a budget set below what
  is free. The ledger makes sure the engine never asks for more than that
  one number. A locked arena was considered and left for later.
- **Live effects will allocate on the control side** at instantiation and
  move to the renderer by command.
- **Review rule:** every new audio-length allocation goes through
  `Ledger::alloc`, or the figure is a lie.

### The two sides

- **The engine is two sides that never name each other.** The disk side is
  `Project` (`open`, `import`, `load`, `sample`). The sound side is
  `Renderer::new() -> (Renderer, Control)`: the renderer moves to the audio
  thread, `Control` stays (`play`, `stop`, `stop_all`, `input`). The only
  thing that crosses is a `LoadedSample`, carried by the caller. Until
  2026-11-10 an `Engine` type was the only way in and every verb was an
  engine method taking an id; no method needed both sides, so it was
  dissolved, and a joint constructor was rejected as the engine again.
- **A verb lives on the thing it acts on.** `sound.stop()`,
  `source.monitor()`. What the renderer hands out carries a `Control`
  clone and acts on its own; what the project hands out is data the
  `Control` takes by reference. A sample carrying a handle and sending its
  eviction from `Drop` was tried and rejected as the wrong coupling.
- **`Control` is the shared sending end.** A mutex around the command
  ring's producer behind an `Arc`, with the sound id counter and the input
  slot count. Control side only: nothing on the audio or capture thread
  holds one. It is the only code that builds a `Command`. Named for what it
  is to the caller; `RendererHandle`, `Link` and `Remote` were rejected.
- **A loaded sample is plain data.** `LoadedSample` is an id and an
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
  its slots reserved. The command ring cannot be sized to make that
  impossible (the control side is unbounded), so it is sized not to happen
  in practice (`COMMAND_CAPACITY`).
- **The source tree is the architecture.** `src/project/` (disk side:
  `Project`, `LoadedSample`, `storage/`, `import/`), `src/control/` (the
  control thread: `Control`, `Sound`, `Input`), `src/render/` (the audio
  thread: `Renderer`, `command.rs`, `dsp/`). Shared at the top: `config`,
  `audio`, `memory`. `render/` imports neither of the others.

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
  `dsp/` holds `math` (`stereo`, `add`, `hard_clip`), `Mixer<S, N>` over a
  `Source` trait, `Playhead`, `InputReader`, and knows no commands. The
  `Renderer` wires a sample mixer (`MAX_SAMPLE_SOUNDS`, steals the oldest:
  a press must sound) and an input mixer (`MAX_INPUT_SOUNDS`, rejects: an
  input must never be cut), zeroes the period, has each add into it, and
  hard-clips once. The pipeline is hard-coded; no runtime graph. A
  `Processor` trait with boxed chains and a plain-data-plus-free-functions
  shape were both built and removed; a piece is a struct with the work as
  a method, built whole on the control side.
- **Commands are grouped by mixer.** `Command::Sample(Play | Stop | StopAll
  | Evict)` and `Command::Input(Add | Remove | Monitor | Unmonitor)`. A
  stop-all must not mute the microphone. A flat enum of eight came first.
- **`SoundId` names one thing heard**, a sample played once or an input
  source, minted by `Control` before the send because the ring is one way.
  The lowest id is the oldest, which is what stealing needs. (Was
  `VoiceId`; "voice" is gone from the code.)
- **Evict by buffer address.** Stopping every sound of a sample sends the
  buffer's address; the renderer drops every playhead whose `Arc` points
  at it and never learns a sample id. The caller still owns the buffer
  when the command is sent, so the address cannot name a later buffer.
- **Mix voice-major into the caller's slice.** Each source adds its whole
  period in turn: contiguous, vectorisable. The renderer never allocates
  a period buffer. Mixers are fixed arrays scanned per call; a `heapless`
  deque came first and stopped fitting once removal from the middle was
  needed.
- **Clip once, hard, last.** Same result on every device and offline; it is
  what hardware samplers do. A master gain or limiter goes before it.
- **Fixed polyphony.** `MAX_SAMPLE_SOUNDS` is a guarantee of a known worst
  case, chosen once by a person for the product; the RAM budget, by
  contrast, is a parameter because it differs per machine.

### Input and monitoring

- **An input is the ability to put audio in; monitoring is whether it is
  heard.** `control.input(layout, period) -> (Input, InputFeed)`. The
  layout is the device's channels as `ChannelLayout::{Mono, Stereo}`
  entries; each entry is a source with its own ring and mixer slot, silent
  until `source.monitor()`. The first version added and heard in one call;
  split because an input will also be recorded and routed.
- **Shaped like the device, split on the capture side.** `InputFeed::feed`
  takes the device's interleaved frames and splits them into the sources'
  rings in one pass, on the capture thread, with no lock. Five shapes came
  first, all on 2026-11-10: a fixed capture width, a width plus channel
  map, mono/stereo-only inputs with the caller splitting, and one ring per
  device with sources inside the reader (which hid a second monitor switch
  the mixer could not see). A ring per source makes a source an ordinary
  mixer slot.
- **The mixer's mute is the monitor switch.** A muted source `skip`s its
  period so an unheard input keeps up with its ring.
- **Two clocks, one ring, a lag rule.** cpal has no duplex stream, so the
  rings drift. The reader takes one period per render; an empty ring is a
  gap; more than `MAX_LAG_PERIODS` queued is skipped, oldest first. The
  ring depth is a few capture periods; the period is a parameter (bounded
  by `MAX_PERIOD_FRAMES`) because it is the device's to know. A fixed 4096
  frame depth came first and was right by luck.
- **Dropping the `Input` removes it.** It sends the removes and frees the
  slots. Drop the `Input` before the `InputFeed` so the rings are freed on
  the caller's thread. An `InputId` and `remove_input` existed until
  Handles.
- **Numbered input slots are not the engine's.** The panel holds the
  sources and labels them.

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
  milliseconds, so it renders silence when idle. Drop the stream before
  the samples it plays, or the last `Arc` is freed on the audio thread
  (the retire ring, below, fixes this).
- **The examples are `first_sound` and `monitor`**, and are plain
  demonstration code: no hardening or shared abstractions there. When the
  device exists, they move into a binary crate.

### Recording (not built)

Built in step 6 and removed in the memory port (PR #40) to be rebuilt on
the renderer. The decisions that stand:

- A recorder is sized and allocated once, from the ledger, on the control
  side; a take is one use of it and never allocates. Growing a buffer per
  take was the earlier plan.
- One take at a time; a new take overwrites the last unless it was saved.
- A full take is not an error: it stops and hands the buffer back.
- The take has no error path; device failures are the device's to report.
- Saving stores the take as a new root through the same crate-private add
  as import.
- Where recording taps the signal (input, bus, main out) is decided with
  step 8's routing.

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

## Step 8: the mixer rewrite

The plan, not built.

- **Keep it stupid.** A `Node` trait, `process(frames)` in place and
  `finished()`: a source fills the slice, an effect transforms it. The
  mixer owns one period of scratch, fills it from each slot and sums it.
  A mixer is itself a `Node`, so a bus is a mixer in a slot.
- **Level and mute leave the mixer** for a `Gain<N>` node; monitoring an
  input becomes turning its gain up, and `skip` goes. Stealing moves to a
  voice table in front of the sample mixer.
- **The renderer needs the output period** for its scratch, so the period
  becomes a `Renderer::new` parameter.
- **The retire ring.** Every place the mixer frees a source hands it to a
  sink; the renderer pushes it onto a ring back to the control side, and
  `Control` sweeps it on every send. Sized at slots plus
  `COMMAND_CAPACITY`, it can never fill. This moves the last free of a
  sample's buffer and an input's ring off the audio thread.
- **Routing a source to two destinations** (a bus and a recorder) is open:
  a ring has one consumer, so either the reader fans out or the second
  destination taps after the first.
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

- **Effects on the render side.** Heard live on a sound or a bus; a print
  is the committed offline render of the same effect, stored as a new
  generation.
- **Replayable recipes** on prints, as presets. Needs effects-as-data.
- **Offline bounce:** a renderer driven in a loop with frame-stamped
  commands, its output stored as a new root.
- **Real-time rulebook** (a doc) and an **allocation-counting test
  allocator** asserting `Renderer::render` allocates nothing.
- **Taps and busses** on the hard-coded path.
- **Level per mixer slot** for samples and inputs alike (subsumed by
  `Gain<N>` if step 8 lands as planned).
- **Master gain and bus limiter** before the clip.
- **Stream errors** reported to a front panel.
- **Content-hash dedup** on import, also matching a renamed WAV back to
  its sample.
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
- **A level meter on a take**, an atomic the panel reads.
- **A `stereo` fast path** for stereo-as-stored, after a measurement.
- **Hardware monitoring** on the target's codec, if it has it.
