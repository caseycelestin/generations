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
  borrow. A voice that has ended still holds it until its reply is taken
  or its handle dropped. Recording cannot share it this way, so `record` takes the
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
  manifest, the request channels, the renderer's scratch and its voice
  pool) is not counted; it is sized once by the caller. Counting the old
  input rings was tried and reverted as one mechanism too many.
- **The OS backs the figure.** Audio is ordinary heap memory; the
  guarantee is a cgroup reservation, no swap, and a budget set below what
  is free. `AudioMemory` makes sure the crate never asks for more than
  that one number. A locked arena was considered and left for later.
- **Live effects will allocate on the control side** at instantiation and
  move to the renderer by request.
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
- **A verb lives on the thing it acts on.** `voice.stop()`,
  `voice.request_status()` and `voice.try_wait()` act on one playing;
  `control.stop(&audio)` and `stop_all()` act across voices, so they live
  on `Control`. A handle holds its request, never a `Control`, so the
  dependency runs one way. Audio carrying a handle and sending its
  removal from `Drop` was tried and rejected as the wrong coupling.
- **`Control` is the door to one renderer.** It holds one request channel
  per actor in the renderer (`Requests`: voices, recording). Named for
  what it is to the caller; `RendererHandle`, `Link` and `Remote` were
  rejected.
- **A handle's errors are in the engine's terms.** `PlayError`,
  `StopError`, `RecordError`, `TryWaitError`, `StatusError` and
  `TryRecvStatusError` translate `request`'s, so a caller reads "the
  renderer is gone, and the audio with it", not a channel's
  `Disconnected`.
- **Loaded audio is plain data.** Hold `Audio` and it is in RAM; drop it
  and its voices play out, its memory repaid once the last voice's reply
  is taken in. `control.stop(&audio)` cuts them first. Each load is its
  own copy; no dedup. What to load at open is the caller's to remember: a
  persisted `loaded` flag was built and removed (it recorded a preference
  before any panel had one).
- **Adding a sample and loading it are separate.** A print must land on
  disk even when it does not fit in RAM.
- **Loading is synchronous** on the control thread. If it ever visibly
  stalls a panel, the fix is a worker thread, not async methods.
- **`create` takes plain sizes and panics on zero `voices`**, as
  `slice::chunks` does: a zero is a typo, not something to recover from.
  `NonZeroUsize` parameters made every caller wrap its constants.
  `voice_queue` may be zero: a play is then refused, not stolen, while
  every voice is held.
- **`control/` and `realtime/` never import each other;** both import
  `protocol/`. The source tree is the architecture, drawn in the crate
  docs.

### Requests

The two sides speak only through `request`, a generic channel at the
crate root that knows nothing of audio.

- **One request, one reply; the real-time side never speaks unprompted.**
  Everything sent back answers one message sent, so each return ring is
  sized by what is in flight. A duplex channel, where a handle could be
  sent any number of events, was built and dropped: its return ring
  could not be sized.
- **A long task replies when it ends, with what it was given.** A play
  replies when its voice ends, carrying the audio; a recording's start,
  how it ended and its take; a stop-audio, the clone of the audio it
  named. The control side frees all of it, so nothing is freed on the
  audio thread. A collector channel for freeing was built and removed
  once every return rode in a reply.
- **Status is asked, not pushed, and is advisory.** `request_status`
  returns a pending answer, one question at a time, given at the
  renderer's next period while it still holds the request; once the
  request has closed the question comes back `Closed`, and the reply has
  the last word. Atomic cells published each period were weighed and
  deferred: they hold only a word, and would be a second mechanism.
- **Cancel is on the handle, and cannot fail.** Its slot is reserved with
  the request; like `JoinHandle::abort` it is idempotent and returns
  nothing. Stopping one voice is its cancel; dropping a recording cancels
  it.
- **One channel per actor, typed by its request.** Both ends deal in the
  request type's own reply and status, so nothing converts and nothing
  can be sent the wrong reply. Each is sized by its actor: voices by
  `voices + voice_queue + VOICE_STOP_HEADROOM`, recording by `RECORDERS`.
  One shared channel with umbrella `Command` and `Event` enums came
  first. Order across channels is lost, which does no harm: the renderer
  takes in every channel at the start of a period.
- **Reserving holds room for all a request can send.** Out three (it, its
  cancel, one question), back two (its reply, one answer), so `open`,
  `cancel`, `close` and `answer` cannot fail. The reservation is counted,
  not a held lock: a permit that held the lock until `open` hung any
  caller that touched the same channel in between. The count is tokio's
  `Permit` shape.
- **Headroom keeps room for stops.** The voice channel's last
  `VOICE_STOP_HEADROOM` slots take only `try_reserve_including_headroom`,
  so a burst of plays cannot refuse `stop` or `stop_all`, as Postgres
  keeps connections for superusers. A channel of their own was rejected:
  stops would lose their order against plays.
- **A slot is held until its reply is taken or its handle dropped.**
  Counting until read keeps one number for room and for the table of
  replies, so memory stays fixed at creation. A dropped handle's reply is
  dropped on the control side when taken in; reserving takes in first,
  and `Control::collect` takes in both channels.
- **Plain handles over a shared state.** Every handle reaches one
  `Arc<Mutex<...>>`, as `std` and tokio's channels do, so dropping one
  means something. Handles as plain ids routed through the end were
  weighed: no lock, but a drop could do nothing.

### The render side

- **The render side never waits.** It owns the real-time ends of the
  request channels (`rtrb` rings: lock-free SPSC, fixed capacity; `mpsc`
  locks and allocates) and takes in what arrived at the start of each
  period. No disk, no clock, no allocation, no lock, no error path. It
  reads only audio already in RAM: an fsync on eMMC can stall I/O for
  hundreds of milliseconds.
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
  numbers voices as they are added and cuts the lowest: age is the
  engine's to track, not read off request ids, which are names.
- **A voice's reply carries no reason.** Nothing on the control side acts
  on why a voice ended, so its handle learns only that it did.
  `Finished`, `Stopped` and `Cut` were reported before.
- **A status has no `Pending` and no `Ended`.** The renderer answers only
  while it holds the voice or recording, and the end comes in the reply.
- **Requests are tasks.** `VoiceRequest` is `Play | StopAudio | StopAll`
  and `RecordingRequest` is `Start`, each answered when done; meaning
  stays plain, the handles carry the verbs. Operations on what the
  renderer held (`Add`, `Remove`) came while replies were events.
- **Stop by the audio itself.** Stopping every voice of some audio sends a
  clone of its `Arc`; the renderer compares by pointer and closes the
  stop with the clone, so it is dropped on the control side. A buffer
  address came before, sound only by an argument about ring order; a
  sample id would name every load and collide across projects.
- **One concept, one name; the module says the side.** `Voice` is both
  the control-side handle and the render side's playing, as `io::Error`
  and `fmt::Error`. The old engine said `Sound` and `SoundId`.
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
  what level is the engine's, by request.
- **The stream runs from open to drop.** Opening clicks and costs tens of
  milliseconds, so it renders silence when idle. Audio can be dropped in
  any order: the last `Arc`, held by a voice, comes back in its reply to
  be freed on the control side.
- **The examples are `first_sound`, `record` and `engine`**, plain
  demonstration code: no hardening or shared abstractions there.
  `first_sound` and `record` run on a device; `engine` shows every engine
  feature and compiles, and each new
  feature is added to it in the same step. When the device exists, they
  move into a binary crate.

### Recording

Built for the main out; recording inputs are not built yet.

- **A recording is a handle, as a voice is, and one handle is one take.**
  `control.record(audio)` returns a `Recording` with `stop`,
  `request_status` and `try_wait`. Recording again is a new `record`, so
  a late stop or answer can never reach a newer take.
- **The caller pays for the take.** `memory.alloc(Stereo, frames)`; the
  length is a maximum, and reaching it ends the take `Full`, which is not
  an error. Only stereo audio is taken: the main out is stereo.
- **The renderer holds the only reference while it writes**, through
  `Arc::get_mut`. Only the control side clones or drops audio and the
  caller gives theirs up, so `record` knows exactly whether a voice still
  holds it and refuses at once. Every refusal hands the audio back in a
  `RecordError`, as `TrySendError` does, and reserving comes before
  sending, as tokio's `try_reserve`: the audio is given up only once the
  send cannot fail.
- **One recorder, sized into its channel.** The recording channel holds
  `RECORDERS` (one) requests, so a second `record` while one is out is
  refused at once with `RecordError::Full` and never reaches the
  renderer. A refusal by the renderer, ending `Refused`, came before.
- **The main out is written as heard**, after each block is clipped, and
  the end-of-period clean-up ends a take whose audio is full. `record`
  clears the audio and prefaults it first.
- **`try_wait` gives how it ended and the audio, once.** It polls, as
  `Child::try_wait` does, and is the only way the take comes back:
  `(RecordingEnd, Audio)`, `Full` or `Stopped`. A cached `state()` beside
  `take()` was replaced: it claimed `Recording` when it only meant no
  reply yet.
- **Dropping a recording stops it**, unlike a voice, which plays out: no
  one could take its audio.
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

Built on `src/engine/`: voices, block mixing and recording of the main out,
over the request channels (see Requests, above).

What is left:
- **Recording inputs** (see Recording, above).
- **Inputs and monitoring** (see above), with the capture thread.
- **Sequencing**, decided but not built: it runs on the audio thread,
  reading a fixed-size pattern that requests edit in place, so an edit is
  heard on the next pass whatever the control side is doing. Its voices
  are not requests and send nothing back. Whether a step names a pad, a
  sound or holds its audio is open.
- **Sample-accurate requests** arrive with sequencing, as a time on the
  request.

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
  requests, its output stored as a new root.
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
- **Warming the small rings.** A return ring's slots are first written
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
- **A level meter on a take**, in its status; the same could report a
  take's progress.
- **Published state cells**, atomics the renderer writes each period, for
  state read every frame, if a status question per frame proves a
  measured cost.
- **Saving a take as loaded audio**, a copy into a right-sized buffer,
  for playing a resample at once without reading it back from disk.
- **Hardware monitoring** on the target's codec, if it has it.
