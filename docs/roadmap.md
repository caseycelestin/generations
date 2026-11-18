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
| 7 | Input monitoring: hear the input through the output | inputs built on `src/engine/`, on a duplex `render`; hearing them to come |
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

- **Target: desktop first, then perhaps a tablet, then perhaps
  hardware** on a Raspberry Pi Compute Module 4; reference device Roland
  SP-404 MK2. The concept is proven and tuned where iterating is cheap,
  rather than on the hardware first. Desktop devices change under a
  running app, run at other rates and clock input and output apart, so
  the engine treats those as normal, not as edge cases.
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
  manifest, the channels, the renderer's scratch and its voice
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
- **A verb lives on the thing it acts on.** `voice.stop()` and
  `voice.try_wait()` act on one playing. A handle holds its transfer or
  request, never a `Control`, so the dependency runs one way. Audio carrying a
  handle and sending its removal from `Drop` was tried and rejected as
  the wrong coupling.
- **A voice is stopped by its handle.** A caller keeps the handle of
  anything it may stop, and stopping them all is a loop over them; a stop
  cannot fail. `control.stop(&audio)` and `stop_all()` were removed:
  they reached only voices whose handles were dropped, and cost two more
  requests on the voice channel, a shared reply and headroom kept for
  them. A list of voices in `Control` was rejected: it would trail the
  renderer, which decides when a voice ends.
- **`Control` does what can fail.** `open_input` reserves, then
  `Input::open(permit)` cannot fail, so every refusal shows in the public
  method. A voice is started by `Voice::play` with `try_start`, which
  hands the value back on refusal: the only work lost is building it, so
  a permit for transfers was weighed and not added. `record` checks its
  audio before it reserves.
- **`Control` is the door to one renderer.** It holds one channel per
  actor in the renderer: voices in `Transfers`, recording and inputs in
  `Requests` until they move onto `transfer`. Named
  for what it is to the caller; `RendererHandle`, `Link` and `Remote` were
  rejected.
- **A handle's errors are in the engine's terms.** `ControlError`,
  `RecordError` and `TryWaitError` translate the channels', so a caller
  reads "the renderer is gone, and the audio with it", not a channel's
  `Disconnected`. A play fails as `ControlError::Full` or `RendererGone`;
  a `PlayError` of its own was the same enum again.
- **Loaded audio is plain data.** Hold `Audio` and it is in RAM; drop it
  and its voices play out, its memory repaid once the last voice's data
  comes home and is taken in. Stopping their handles cuts them first. Each load is its
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
  every voice is held. `capture_channels` may be zero, with no input
  device, and so may `inputs`.
- **`control/` and `realtime/` never import each other;** both import
  `protocol/`. The source tree is the architecture, drawn in the crate
  docs.

### Transfers

The two sides speak through `transfer`, a generic channel at the crate
root that knows nothing of audio. Voices are on it; recording and inputs
are still on `request`, which it replaces once they move.

- **A value goes over and comes home whole; nothing else crosses back.**
  The reply is the value itself, with whatever the renderer wrote into
  it, so everything it owns is freed on the control side and each return
  ring is sized by what is in flight. A separate reply type was dropped:
  the renderer could forget to send back what a request owned. A duplex
  channel, sending a handle any number of events, was built and dropped:
  its return ring could not be sized. A collector channel for freeing was
  built and removed once everything rode home.
- **Named for DMA transfers in embedded Rust,** where a `Transfer` owns
  the buffer while the hardware works and `wait` hands it back; io_uring
  runtimes call it ownership passing. `request` stopped fitting once
  nothing was asked for; "loan" was weighed and collides with Rust's
  lending iterators. Payloads are named for what they hold: `VoiceData`,
  as Kira plays a `StaticSoundData`.
- **What changes while running is state beside the value.** A voice's
  `VoiceState` (`stop`) rides in its `VoiceData` behind an `Arc` and the
  handle keeps a clone: the handle writes, the renderer reads once a
  period. Lock-free atomics of 64 bits or less, each read alone; a
  snapshot type waits for fields that must agree. The engine defines the
  state, not `transfer`, which stays only sent and handed back. Status is
  out until a caller needs it. Cancel and asked status on the ring came
  first: a cancel could outlive its request, and every status only said
  "still held". Updates as messages each needed ordering rules a flag
  does not.
- **Each transfer brings its own return ring, and its end comes back.**
  `try_start` makes a one-deep ring; the caller keeps the `Transfer`, and
  the renderer receives a `Completion`: the value and the ring's sending
  end, read and written through it but never moved out. `complete` sends
  the value home and the end back on `returned`, so the ring is freed on
  the control side, as Kira returns unused resources. Nothing is shared,
  so no id and no lock. A value and completion handed over apart were
  replaced: one could be completed with another's value. Reply rings per
  slot, made at `create`, were built and set aside on branch
  `request-v2-slots`. A `Drop` sending the end back was rejected: it
  would share `returned` with every end.
- **One channel per actor, typed by its value.** Each is sized by its
  actor: voices by `voices + voice_queue`, recording by `RECORDERS`,
  inputs by `inputs`. One shared channel with umbrella `Command` and
  `Event` enums came first. Order across channels is lost, which does no
  harm: the renderer takes in every channel at the start of a period.
- **A transfer is in flight from started until its end is back, so
  completing never fails.** `ControlEnd` counts them, and both shared
  rings hold `capacity`: a value received but not completed still
  counts. `try_start` refuses with `StartError::Full` at capacity, or
  `Disconnected` once the renderer is gone, either way with the value.
  Without that check, a value started after the renderer went sat in a
  ring nobody reads, and its transfer said "not yet" forever.
- **A dropped `Transfer` needs nothing done.** Its ring, and the value in
  it, is freed when the end comes back and `take_in` drops it.
- **A dropped real-time end drops its unreceived values,** so their
  transfers learn none is coming: the ring would keep them while the
  control end lives. It frees then, at teardown, when no deadline is
  left.
- **`Control` is not `Clone`.** An app holds one, in its own state, and
  shares it its own way: `play`, `record` and `collect` take `&mut self`,
  as Kira's `AudioManager::play` does, so the single-producer rings stay
  owned. Handles as ids routed through the end were weighed: a drop could
  do nothing.

### The render side

- **The render side never waits.** It owns the real-time ends of the
  channels (`rtrb` rings: lock-free SPSC, fixed capacity; `mpsc`
  locks and allocates) and takes in what arrived at the start of each
  period. No disk, no clock, no allocation, no lock, no error path. It
  reads only audio already in RAM: an fsync on eMMC can stall I/O for
  hundreds of milliseconds.
- **Real-time safe, not real-time only.** Frame counts, never a clock, so
  the same `render` drives a device callback live or a loop offline (the
  bounce).
- **Mix in blocks.** `render(capture, out)` takes the device's flat
  interleaved period, of any length, capture and output the same frames.
  It mixes in blocks of `BLOCK_FRAMES` (64), the capture cut at the same
  frames: each block starts silent, each voice renders into one shared
  scratch and is added in, and the period is hard-clipped once. Blocks
  keep the scratch fixed and are what Pd, SuperCollider and Max do; the
  latency is the device's period, not the block. The mix is always
  stereo (`Renderer::OUTPUT_CHANNELS`): mono audio plays at full level on
  both sides. A `Source` trait, boxed `Processor` chains and a generic
  `Mixer` were each built and dropped.
- **Voices are a fixed pool.** `voices` is given to `create` and the pool
  never resizes: a CPU budget the caller sizes for its hardware, with no
  hardware maximums in the crate, and not counted by `AudioMemory`. A play
  with every place taken cuts the oldest: a press must sound. The pool is
  a `Vec` kept oldest first and never past its places, so the oldest is
  the first; numbered places searched for the lowest came before. It owns
  the voices' end of their channel, so every way a voice arrives or
  leaves is in `voices.rs`: received, cut, stopped at the start of a
  period, finished at its end.
- **A voice comes home with no reason.** Nothing on the control side acts
  on why a voice ended, so its handle learns only that it did.
  `Finished`, `Stopped` and `Cut` were reported before.
- **What crosses is a task.** A `VoiceData` is a play and a
  `RecordingRequest` a `Start`, each handed back when done; meaning
  stays plain, the handles carry the verbs. Operations on what the
  renderer held (`Add`, `Remove`) came while replies were events.
- **One concept, one name; the module says the side.** `Voice` is both
  the control-side handle and the render side's playing, as `io::Error`
  and `fmt::Error`. The old engine said `Sound` and `SoundId`.
- **Clip once, hard, last.** Same result on every device and offline; it is
  what hardware samplers do. A master gain or limiter goes before it.

### Input and monitoring

Inputs are built; monitoring is not.

- **The renderer is duplex.** `render(capture, out)` takes the input
  device's frames for the same period it fills, interleaved,
  `capture_channels` wide. One callback is one clock, and it is how JUCE,
  PortAudio and JACK hand audio over; a caller with two clocks, common on
  desktop, bridges them before `render`, since only it knows its devices. A capture ring per device, filled by a `Feeder` on
  the capture thread, was built first and removed: it put a cushion, clock
  drift and a feeder that could go away inside the engine.
- **An input names capture channels; it owns none.** `open_input` takes
  `InputChannels::Mono(c)` or `Stereo(left, right)`, counted from zero,
  and refuses one past the device at once (`NoSuchChannel`). Inputs may
  read the same channel; `inputs` is how many can be open at once, not
  the channel count. Each input keeps the capture width to cut a block
  into frames.
- **Every open input renders each block, whether or not anything hears
  it.** The renderer holds them in a `Vec`, and monitoring adds what one
  renders. Until then the copy is what keeps `Input::render` in use, chosen
  over marking it `expect(dead_code)`.
- **Opening an input is always on `Control`.** With zero capture channels
  every channel is past the device, so it answers `NoSuchChannel`, as
  Apple's `inputNode` has no channels rather than not existing. A
  `NoCapture` error and a second constructor, `create_with_capture`, were
  removed with the ring: a device's width is one number at `create`.
- **A device change rebuilds the engine**: the caller calls `create`
  again, as a DAW stops its engine to switch interfaces. Attaching a
  capture to a running renderer by request was rejected for it, and so
  was `Control` making its renderers: once a renderer is gone, `Control`
  holds nothing a new one needs, and clones that quietly followed a new
  renderer would break with the handles, which stay tied to theirs as an
  `mpsc::Sender` does. It comes back if `Control` holds what a renderer
  was told, to replay. Keeping the engine across a switch is the
  caller's (Deferred).
- **Monitoring is state on the input.** `input.set_monitoring(bool)`
  sets a flag in the input's state, as stop is on a voice, read each block, changeable while the
  input is open. A second request kind on the input channel was tried and
  dropped: it forced a shared reply type. Decided, not built.

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
- **cpal is pinned to master for its duplex streams** (ALSA and JACK),
  which 0.18.2 lacks, and goes back to a release when 0.19 ships. The
  examples play and record the main out with no capture; a duplex
  `Device` comes with the first example that hears an input.
- **The examples are `first_sound`, `record` and `engine`**, plain
  demonstration code: no hardening or shared abstractions there.
  `first_sound` and `record` run on a device; `engine` shows the voice and
  recording features and compiles. When the device exists, they move into
  a binary crate.
- **A new feature is shown in a doc example on its API**, `no_run`, as
  inputs are on `open_input`: `cargo test` compiles it as surely as an
  example, and `cargo doc` shows it beside what it uses. Not yet aligned:
  `engine`'s features move to doc examples in their own PR.

### Recording

Built for the main out; recording inputs are not built yet.

- **A recording is a handle, as a voice is, and one handle is one take.**
  `control.record(audio)` returns a `Recording` with `stop` and
  `try_wait`. Recording again is a new `record`, so a late stop can never
  reach a newer take.
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
  `(RecordingEnd, Audio)`, `Full`, `Stopped` or `RendererGone`: a dropped
  renderer still hands back what was heard, as nothing else holds the
  audio, so a device failure keeps the take. A cached `state()` beside
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
over the channels (see Transfers, above).

What is left:
- **Recording inputs** (see Recording, above).
- **Monitoring** (see Input and monitoring, above).
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
- **Keeping the engine across a device switch.** cpal drops a callback
  with its stream, so the caller would have the callback hand the
  renderer back first, through a one-slot ring, and start the new stream
  with it. The engine needs nothing for it; a helper only
  if callers get it wrong.
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
  by the renderer, so each faults a few pages on its first pushes. rtrb
  has no hook to prefault; pushing and popping a value through every slot
  at creation would do it.
- **`mlockall` on the device.** `mlockall(MCL_CURRENT | MCL_FUTURE)`
  backs and locks every page the process has or will have, the blanket
  form of prefaulting, and guarantees nothing is paged out later. It
  needs `CAP_IPC_LOCK` or a raised `RLIMIT_MEMLOCK` and is a
  process-wide choice, so it is the caller's, beside the cgroup
  reservation and no swap; the crate does not own the process.
- **A level meter on a take**, in its status; the same could report a
  take's progress.
- **Saving a take as loaded audio**, a copy into a right-sized buffer,
  for playing a resample at once without reading it back from disk.
- **Hardware monitoring** on the target's codec, if it has it.
