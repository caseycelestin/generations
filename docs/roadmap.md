# Roadmap

The plan as it stands: what is built, what comes next, what is open, and
the decisions that shape work still to come. The architecture is in the
crate (`cargo doc --open`); how it got this way is in git.

## Built

- **The engine:** a duplex `Renderer` driven by the caller's device, and a
  `Control` that loads audio, opens inputs and arms recorders.
  - Loaded audio, each with voices of its own, mixed in blocks to a stereo
    main out.
  - Inputs from capture channels, each heard through the main out when
    monitored.
  - Recording the main out, streamed to a drain the caller runs.
- **Examples:** one per feature, over cpal, in `examples/`.

## Next

1. **Timestamped requests.** The renderer counts frames, and a request can
   name the frame it lands on, placed inside its block. Every option for
   sequencing needs this, and so do live triggers and the offline bounce.
   Not built.
2. **A clock shared by both sides.** The caller hands `render` the
   callback's device timestamp; the renderer publishes which frame was at
   which time, so the control side can stamp a press or a MIDI note and
   have it land a fixed delay later, with no jitter. Not built. Open:
   which clock, since cpal's differs per host and a MIDI stamp may use
   another, and whether the engine or the caller converts.
3. **Sequencing**, once the open questions below are settled.

## Open questions

- **Inputs on ids, like recorders.** States made at `create` and held by
  both sides would make a tap plain data, so recording an input needs no
  ring. Open: what stops a recorder tapping an input whose place is closed
  and reopened. Recording an input waits on this.
- **Per-feature parts on `Control` for inputs and audio,** as recorders
  have (`control.recorders()`), or their verbs left on `Control`.
- **What else lives on the audio thread.** Loaded audio now does, and a
  press is a voice number written into it. Open: whether pads, patterns,
  the transport, mixer and effect settings follow, the control side
  editing that state and supplying its memory.
- **Where the sequencer runs.** On the control side it schedules ahead by
  a lookahead and stamps each event: sample-accurate unless that thread
  stalls past the lookahead, and an edit is heard only after it. On the
  audio thread it reads the pattern itself: always sample-accurate, an
  edit heard on the next pass, and the pattern and the audio its steps
  play must live there.
- **What a step holds:** a pad, a sound, or its own audio.

The accuracy target: sequenced steps land on their sample; a live trigger
has a small fixed latency and no jitter.

## Decisions

Each is a decision that stands today, with what it rules out where that
might be tried again.

### Target and format

- **Desktop first,** then perhaps a tablet, then perhaps hardware (a
  Raspberry Pi CM4); the Roland SP-404 MK2 is the reference. Devices that
  change while running, run at other rates or clock input and output apart
  are normal cases.
- **One format: 48 kHz `f32`,** in RAM and on the render path;
  each buffer is mono or stereo. `i16` storage was rejected: it loses
  24-bit sources at every generation, and the render path should read
  samples, not convert them.

### Memory

- **The engine keeps no memory budget.** Loaded audio is the caller's
  samples, moved in rather than copied, held until unloaded and handed
  back on any refusal.
- **Nothing is allocated after `create`.** Places for inputs and loaded
  audio, each audio's voice slots, rings and scratch are made there, sized
  by plain arguments, and reused: a load refills a free place in place.
  Sizes as const generics were tried and dropped: one size the caller
  could not choose at start-up, for no saving.
- **The render side never allocates.** What it needs is allocated on the
  control side and handed over: live effects will be too.
- **Memory the audio thread writes first is prefaulted** on the control
  side (a recorder's ring, later a delay line); a first write would
  otherwise stall the audio thread on a page fault. Not yet done for
  recorder rings.

### The engine

- **The device is the caller's.** The crate has no audio dependency and
  `render(capture, out)` is handed one duplex period; a caller with
  separate input and output clocks bridges them before `render`. A device
  change means calling `create` again.
- **The render side never waits:** no disk, lock, allocation or error
  path, and frame counts rather than a clock, so the same `render` runs
  offline.
- **Inputs are opened over a one-way ring, from states made at `create`.**
  `Control` keeps a clone of every input state in a pool, so the
  renderer's is never the last and a stopped input is simply dropped; a
  place is free again once the pool's clone is the only one left. A
  transfer brought each input home only to free it.
- **Loaded audio lives in a pool too.** Its samples and layout sit in the
  pooled state, so the renderer's clone is never their last owner;
  `load` checks for whole frames, a renderer, then a free place, and sends
  the state over a one-way ring. Rejected: samples in the message, which
  would have to come home to be freed.
- **Dropping `Control` leaves the renderer as it was:** inputs and loaded
  audio still held keep sounding. So the renderer's drop of a state is
  never the last, `Control`'s drop is to leak the pools' clones while the
  renderer exists; handing the pools over on one more ring would avoid the
  leak. Not built.
- **A verb lives on the thing it acts on.** Handles (`Audio`, `Voice`,
  `Input`, `Recorder`, `Drain`) carry their own verbs and errors and never hold a
  `Control`; every check comes before anything is sent. Dropping one ends
  what it holds, so each is `must_use`; a voice can be detached to play
  out.
- **Voices belong to loaded audio,** `voices` each, as `create` is told.
  A play takes a free voice and, with none free, cuts that audio's oldest,
  because a press must sound; one audio never cuts another's. A global
  voice pool was dropped.
- **A voice is started and stopped through atomics in its slot,** with no
  ring: a ring per audio could not be shared by its handles nor returned
  to its place. Each play gets the next number, sent in one atomic that
  the renderer takes, leaving 0. A stop raises the slot's stop to the
  voice's number and never lowers it, so a stolen voice's handle cannot
  stop its successor; the renderer marks each voice done, and a slot is
  free once its last voice is. The renderer keeps nothing per voice: which
  voice plays and its playhead live in the slot, so the control side can
  read where a voice is. A flag beside the number was rejected for a
  double-start race; plain flags would forbid stealing held voices.
- **The mix is stereo, in blocks of 64 frames, clipped once, hard, last.**
  A master gain or limiter goes before the clip.
- **Inputs name capture channels and own none.** Every open input renders
  each block whether heard or not, so it can be recorded unmonitored once
  input taps return; monitoring is off until asked for.
- **Recording streams.** `create` makes `recorders`, each a state and a
  ring of `ring_frames` that both sides hold for the whole run; the
  renderer pushes the main out after the clip, and the caller drains it.
  Rejected: a take allocated by the caller and handed back whole, and a
  ring made per recording, whose producer would die or allocate.
- **Arming hands out two halves:** a `Recorder` that records and stops
  from a control thread, and a `Drain` that owns the ring's consumer on
  its own thread, so neither needs a lock (`Arc<Mutex>` was rejected). A
  recorder records many times per arming; it is free once both halves are
  gone, the `Drain` sending its consumer home over a channel between
  control threads.
- **A recorder's lifecycle is one atomic moved by compare-exchange:**
  idle, armed, start requested, recording, stop requested, releasing.
  Separate flags were rejected: a dropped `Recorder` with a pending start
  could be re-armed while the renderer started it.
- **Requests take effect at block boundaries.** A start records the whole
  next block; a stop ends before the next. A record during a stop is
  ignored, and a start cannot be cancelled.
- **A full ring keeps its oldest frames.** Lost frames are not counted
  yet, and a drain does not yet report where one recording ends.
- **Arming is on the control side;** a recording that starts itself at a
  threshold, on a beat, or with pre-roll kept from before the press would
  be the renderer moving an armed recorder on. Not built.
- **One file per feature.** A feature's data, handle and audio-thread part
  sit together; the audio-thread part in a `mod realtime`.

### Process

- **One opinionated product with clean seams.** Build what the sampler
  needs when a step needs it; the public surface stays minimal and
  convenience arrives with a caller.
- **Nothing is settled by having been written.** A step is designed from
  its decision, and existing code changes to fit.
- **Examples show one feature each on a device,** as plain demonstration
  code. Refusals and edge cases belong in tests; a new feature also gets a
  `no_run` doc example on its API.
- **Tests wait for a design to settle;** until then a feature is checked
  by ear on its example. The recorder has no tests yet.
- **cpal is pinned to master** for duplex streams, back to a release when
  0.19 ships.

## Later

Not planned yet; kept so they are not forgotten.

**Engine**
- Effects on the render side, heard live; a print is the committed render,
  stored as a new generation. Replayable recipes as presets after that.
- Offline bounce: a renderer driven in a loop with timestamped requests,
  its output stored as a new root.
- Busses, each a new tap point; master gain and limiter.
- A mono output device folding the stereo mix at the end (level
  undecided).
- Keeping the engine across a device switch: the callback hands the
  renderer back before its stream drops.
- Stream errors reported to the caller.
- A real-time rulebook, and a test allocator asserting `render` allocates
  nothing.
- A level meter and progress on a recording; saving one straight into
  loaded audio.
- Lost frames counted, and where one recording ends reported by its
  drain, through a frame count the renderer keeps.

**Storage**
- The project on disk, removed for now and in git: WAVs under a manifest,
  ids minted by the project, every write durable when the call returns,
  lineage as parent pointers. It returns, built on samples and a layout,
  when storage is needed.

**Hardware**
- Storage tiers: the active project on eMMC, kept projects on the SD
  card; the project does not know.
- An `i16` build, chosen at build time.
- The memory budget read from the device's configuration.
- `mlockall`, a cgroup reservation and no swap, set by the caller.
- Hardware monitoring on the codec.
