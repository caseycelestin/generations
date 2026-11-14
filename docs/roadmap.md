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
| 6 | Recording: audio input as a new root | main out done on `src/engine/`, `take` included; inputs to come |
| 7 | Input monitoring: hear the input through the output | to be rebuilt on `src/engine/` |
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
  in-memory copy of the project, not even a cache: the OS caches the
  files, and nothing real-time reads them. Every change is durable when
  the call returns, so there is no save step; on the device the active
  project lives on eMMC and a "save" is a copy to the SD card. A model
  that kept everything in RAM and dumped it to disk was replaced in
  step 3.
- **Project and storage are one type.** A split into rules over a
  separate storage medium was dropped: with no rules left, `Project` only
  passed calls through, behind two nested error types. When lineage
  returns, its rules are checks against the manifest before a write, on
  `Project`.
- **The project mints sample ids.** A sample does not exist until it is
  stored, like a database insert returning the row id. The counter is on
  disk, only goes up, and never reuses an id. There is no remove: one was
  built and cut for want of a caller.
- **A sample is named by its `SampleId`; a voice is not.** A sample
  outlives the program, so callers refer to it by an id they can keep and
  store (a parent pointer, pads and patterns later), and
  `load(id, &memory)` turns that into `Audio` the way a path opens a file.
  A voice lives only while it plays, so its handle keeps its id private
  and carries the verbs instead.
- **Generation is a relationship, not a type.** Not built: a sample will
  be an id and an optional parent id, and lineage the tree of parent
  pointers. It was built once and removed while nothing made a child; it
  returns with prints. `parent` goes in the manifest's per-sample `Entry`
  with `#[serde(default)]`, its rule on `Project`, and `metadata(id)`
  reads it. Other fields join the sample the same way as steps need
  them.
- **"Project"** is what every sampler calls the unit a user opens and
  copies to a card. It will hold patterns, settings and pad layouts too.

### Files

- **No backend abstraction.** A `Storage` trait was removed: it had one
  implementation, and the tests run on a temporary directory instead of
  an in-memory stub.
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
- **The manifest is a private struct,** not a serde derive on the model,
  so the file format is versioned on its own. Samples are WAV rather than
  raw bytes so any audio tool can open one.
- **The directory belongs to the engine.** A hand-renamed WAV makes open
  refuse the project; recovering is content-hash territory (Deferred).

### Audio

- **Target:** Raspberry Pi Compute Module 4; reference device Roland
  SP-404 MK2.
- **One fixed format: 48 kHz, `f32`, on disk, in RAM and on the render
  path.** Channel count (1 or 2) is per buffer. Not `i16` storage with
  `f32` processing: the CM4 has hardware float, `i16` throws away 24-bit
  sources at every generation, and the render path should read samples
  rather than convert them. Cost: half the resident minutes. The `Pcm`
  alias keeps the choice in one place; an `i16` build is a deferred idea,
  chosen at build time, never a hybrid.
- **Effects are not built.** They will live on the render side. An
  offline form (`&AudioBuffer -> AudioBuffer`) was removed: it was the one
  allocation the RAM budget could not see.

### Audio in

- **Audio enters a project by being written.** `memory.alloc` hands out
  empty `Audio`, `extend` fills it, and `project.add_sample` stores it and
  mints its id. `alloc` is named for what it does: it takes memory and
  touches no disk. WAV import was removed; it is in the history before
  97e8eff if a file format is needed again.
- **The rule is that every byte of audio in RAM is counted, not that the
  user never touches audio.** `Audio` is public, readable and writable;
  only the crate can make one, so all of it comes from `AudioMemory`.
- **Audio in RAM, samples on disk.** What RAM holds has no id: it becomes
  a sample only when a project stores it. So the RAM types are named for
  audio (`AudioMemory`, `Audio`, crate-private `AudioBuffer`) and the
  project's for samples (`SampleId`, `add_sample`). `Sample` and
  `SampleMemory` were the RAM names before.
- **One type is written, played and saved.** `Audio` is an
  `Arc<AudioBuffer>` with no `Clone`. `extend` goes through
  `Arc::get_mut`: refused while the engine holds the audio, which is what
  `Audio` protects; a read by the project is protected by an ordinary
  borrow. A voice that has ended still holds it until its return is
  collected. Recording cannot share it this way, so `record` takes the
  audio by value (see Recording).

### Memory

- **One figure covers all audio-length memory.** The caller makes one
  `AudioMemory` per machine, not per project, and shares it as a
  cloneable handle; loaded samples, audio being written and later effect
  state all count against it. A per-sample length bound was rejected: the
  worst case was a multiple that depended on the operation.
- **`AudioMemory` makes every `AudioBuffer`, and a buffer is the only
  audio-length memory.** A buffer has fixed capacity and repays on drop,
  so release cannot be forgotten or doubled. A reservation held beside an
  ordinary `Vec` was replaced: the buffer is its own receipt.
- **Who may allocate is who holds an `AudioMemory`:** the caller, and what
  it lends one to for a call (`Project::load`). The project and the
  engine hold none; the render side never allocates. A parameter, not a global, because the
  signature is the permission and tests need their own budgets.
- **Track what scales with audio; bound the rest.** Fixed overhead (the
  manifest, the rings, the renderer's scratch and its voice pool) is not
  counted; it is sized once by the caller, and the rings' list of handles
  takes the ring back's size. Counting the old input rings
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
  written once on the control side before the audio thread has it:
  `record` prefaults its audio. Prefaulting every allocation was rejected
  as work with nothing to gain.
- **Review rule:** every new audio-length allocation comes from
  `AudioMemory`, or the figure is a lie; one the audio thread fills is
  prefaulted.

### The two sides

- **The project and the engine never name each other;** the caller
  carries `Audio` between them. An `Engine` type over both was dissolved,
  since no method needed both sides, and a joint constructor was rejected
  as the same thing again.
- **A verb lives on the thing it acts on.** `voice.stop()` and
  `voice.state()` act on one playing; `control.stop(&audio)` and
  `stop_all()` act across voices, so they live on `Control`. A handle
  holds the renderer's shared rings, never a `Control`, so the
  dependency runs one way. Audio carrying a handle and sending its
  removal from `Drop` was tried and rejected as the wrong coupling.
- **`Control` is the door to one renderer.** It holds the control side's
  ends of both rings behind one lock (`SharedRings`), so every push
  follows a drain of the ring back. Nothing on the audio thread holds
  one. Named for what it is
  to the caller; `RendererHandle`, `Link` and `Remote` were rejected.
- **Commands are optimistic; the audio thread is the authority on state.**
  A send succeeds or reports the ring full; what then happened comes back
  as an event on the ring back. A state and what the renderer hands back
  arrive in one event, so a handle never sees an end before the take it
  hands back; atomic reports beside the ring were used before and could
  show `Ended` before the take could be collected.
- **A voice has no `Pending`.** It cannot be refused, so it is `Playing`
  from `play` until it ends, and the renderer sends nothing when it adds
  one. `Pending` with a `Playing` reply was tried and dropped: it always
  resolved within a period, and the extra event per play would have grown
  the ring back.
- **Every message names its handle.** Both rings carry `Message<T>`, the
  handle's `HandleId` and the data: `Message<Command>` out,
  `Message<Event>` back. Commands carry no handle id: `Stop` stops the
  sender's take and `Remove` the sender's voice. `Control` has an id of
  its own for what it sends. Tickets minted per send were tried and
  dropped: they named messages, not things, so a stop's own ticket and
  its take's state disagreed.
- **Every handle wraps one `Handle`; the rings only route.** `Handle`
  holds the rings, the id and an inbox, and does what every handle does:
  `open` reserves, mints, registers the inbox and sends under one lock,
  `send` sends under the handle's id, and `inbox` collects, then locks.
  `collect` hands each event to its id's inbox through the `Inbox` trait
  and frees the record once the inbox says it is done. What an inbox
  keeps, when it is done, and its default start are the feature's,
  written beside its handle; so are its checks and prep, which come
  before `open`. Feature rules in the rings (an enum of boxes matched
  against events in pairs) were built and moved out: the rings would
  name every feature.
- **The inboxes are a fixed list.** It takes the ring back's size, since
  every id in it still has an event coming, so it never grows; an inbox
  is the handle's, allocated with it. A growing map was rejected to keep
  the engine's memory fixed at `create`, and a box carried in the spec,
  because the audio thread would carry control-side state.
- **`open` takes the spec and the command's constructor**
  (`Handle::open(&rings, spec, RecordingCommand::Start)`) and builds the
  command only once the send cannot fail, so a full ring hands the
  caller its own spec back. Returning the built `Command` made `record`
  unpack it behind an `unreachable!`. Holding the permit from before the
  caller's prep was weighed and left out for simplicity: a `record`
  refused for a full ring has already prefaulted.
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
- **A `HandleId` comes from a counter in the rings,** an atomic outside
  their lock so a handle can mint one while it holds a permit: unique
  within one engine, in order, never reused. A refused `record` mints
  none, since its checks come first. A process-wide `VoiceId` from an `id!` macro came before it.
- **`create` takes plain sizes and panics on a zero**, as `slice::chunks`
  does: a zero is a typo, not something to recover from. `NonZeroUsize`
  parameters made every caller wrap its constants.
- **`control/` and `realtime/` never import each other;** both import
  `protocol/`. The source tree is the architecture, drawn in the crate
  docs.

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
  both sides. A `Source` trait, boxed `Processor` chains and a generic
  `Mixer` were each built and dropped.
- **Voices are a fixed pool.** `voices` is given to `create` and the pool
  never resizes: a CPU budget the caller sizes for its hardware, with no
  hardware maximums in the crate, and not counted by `AudioMemory`. A play
  with every place taken cuts the oldest: a press must sound. The pool
  picks which voice to cut; the renderer ends it and reports why. The
  pool numbers voices as they are added and cuts the lowest: age is the
  engine's to track, not read off the order of handle ids.
- **Every voice ends one of three ways, and says which:** `Finished`
  (played to its last frame), `Stopped` (a stop arrived) or `Cut` (its
  place was taken). `Cut` is the sampler's umbrella: choke groups and
  retrigger will end voices the same way.
- **Only plain things cross, both ways.** A play or a record carries a
  spec, the audio's `Arc<AudioBuffer>`; the renderer keeps the sender's
  id beside what it holds. Each feature's event is its state, plus its
  spec when the renderer lets go (`VoiceEvent`, `RecordingEvent`), so the
  last reference is dropped on the control side. A generic event type
  was tried and dropped: it saved one function, and the rings still
  needed one variant per feature.
- **The ring back never fills.** Every send drains it first, so between
  drains it holds at most every voice and the recording the renderer
  holds, plus one event per queued command (the end of a voice it adds,
  or a take's start or refusal); `create` sizes it at that. Freeing a
  large buffer is a trip into the allocator and the kernel with no bound
  on its time, so it never happens on the audio thread.
- **Commands are low level.** Operations on what the renderer holds,
  never intents: `VoiceCommand` is `Add(VoiceSpec) | Remove |
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
  and `fmt::Error`. `Event` is the counterpart of `Command`; it replaced
  `Return`, and `Released` before it clashed with an envelope's release.
  The old engine said `Sound` and `SoundId`.
- **Clip once, hard, last.** Same result on every device and offline; it is
  what hardware samplers do. A master gain or limiter goes before it.

### Input and monitoring

Not built; the old engine's version is in the history before 42231f2.
Inputs bring the capture thread, whose place is `realtime/capture/`.
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
- **The examples are `first_sound`, `record` and `engine`**, plain
  demonstration code: no hardening or shared abstractions there.
  `first_sound` and `record` run on a device; `engine` shows every engine
  feature and compiles, and each new
  feature is added to it in the same step. When the device exists, they
  move into a binary crate.

### Recording

Built for the main out; recording inputs are not built yet.

- **A recording is a handle, as a voice is, and one handle is one take.**
  `control.record(audio)` returns a `Recording` with `stop`, `state` and
  `take`. Its state is `Pending`, `Recording`, then `Ended(Full)` or
  `Ended(Stopped)`, or `Refused`; its inbox holds the last
  `RecordingEvent`. Recording again is a new `record`, so a late stop or
  event can never reach a newer take.
- **The caller pays for the take.** `memory.alloc(Stereo, frames)`; the
  length is a maximum, and reaching it ends the take `Full`, which is not
  an error. Only stereo audio is taken: the main out is stereo.
- **The renderer holds the only reference while it writes**, through
  `Arc::get_mut`. Only the control side clones or drops audio and the
  caller gives theirs up, so `record` knows exactly whether a voice still
  holds it and refuses at once. Every refusal hands the audio back in a
  `RecordError`, as `TrySendError` does. The rings reserve, then send, as
  tokio's `try_reserve` does: the audio is given up only once the send
  cannot fail, and no copy of it is ever left behind.
- **One recorder, and the rule is the renderer's.** `record` always
  sends; a start while a take runs is refused and shows as `Refused`. A
  running take is never cut. Whether the recorder is free and whether the
  audio is free are two rules, each kept where it is known exactly.
- **The main out is written as heard**, after each block is clipped, and
  the end-of-period clean-up ends a take whose audio is full. `record`
  clears the audio and prefaults it first.
- **A recording's event is its state, plus its spec when the renderer
  lets go:** `RecordingEvent { state, spec: Option<RecordingSpec> }`, the
  spec back with `Refused` and `Ended` only. One state type is shared with
  the handle; a separate wire enum (`Started`, `Refused(spec)`,
  `Ended(why, spec)`) enforced the pairing in its types but duplicated the
  states.
- **`take` polls.** `take(&mut self) -> Option<Audio>` collects, then
  takes the spec out of its inbox: the audio once the take ends, at once
  if it was refused, `None` before and after. `take(self) ->
  Result<Audio, Self>` was replaced: polling it moved the handle out and
  back each time.
  A mailbox or write-once `Slot` in the spec, `oneshot` (its sender frees
  on the audio thread) and `OnceLock` (it only lends) were ruled out.
- **Dropping a recording stops it**, unlike a voice, which plays out: no
  one could take its audio. A stop lost to a full ring ends `Full`.
- **Arming comes later, with the features that need it.** An armed
  recording is held by the renderer, which starts it itself: at the frame
  the input crosses a level (threshold start), on the next beat or bar
  once there is a clock (quantized start), or with the second before the
  press kept from a ring written while armed (pre-roll). Only the audio
  thread can time these. Arming also claims the recorder early, so a busy
  one is refused before the press, and takes `record`'s prefault off the
  press. Until then the user waits for the prefault, a few milliseconds
  per 30 s of stereo; accepted for now (2026-11-13).

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

Built on `src/engine/`: voices, block mixing, the ring back (see the
render side, above) and recording of the main out (see Recording).

What is left:
- **Recording inputs** (see Recording, above).
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
  by the renderer (and an input ring's will be by the capture thread), so
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
- **Hardware monitoring** on the target's codec, if it has it.
