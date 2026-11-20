# Roadmap

The plan as it stands: what is built, what comes next, what is open, and
the decisions that shape work still to come. The architecture is in the
crate (`cargo doc --open`); how it got this way is in git.

## Built

- **The engine:** a duplex `Renderer` driven by the caller's device, and a
  `Control` that loads audio, opens inputs and records takes.
  - Loaded audio, each with voices of its own, mixed in blocks to a stereo
    main out.
  - Inputs from capture channels, each heard through the main out when
    monitored.
  - Recording from the main out or one input, into `AudioMemory`.
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
  back on any refusal. `AudioMemory` remains only for takes, until
  recording changes.
- **Nothing is allocated after `create`.** Places for inputs and loaded
  audio, with each audio's voice slots, rings and scratch are sized there
  by the caller and reused; a load replaces a free place's state whole.
- **The render side never allocates.** What it needs is allocated on the
  control side and handed over: live effects will be too.
- **Memory the audio thread writes first is prefaulted** on the control
  side (a take, later a delay line); a first write would otherwise stall
  the audio thread on a page fault.

### The engine

- **The device is the caller's.** The crate has no audio dependency and
  `render(capture, out)` is handed one duplex period; a caller with
  separate input and output clocks bridges them before `render`. A device
  change means calling `create` again.
- **The render side never waits:** no disk, lock, allocation or error
  path, and frame counts rather than a clock, so the same `render` runs
  offline.
- **A take goes over as a transfer and comes home whole,** so everything
  it owns is freed on the control side. What changes while it runs (stop)
  is lock-free state shared beside it, not further messages; its channel
  is sized by how many can be in flight.
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
  `Input`, `Recording`) carry their own verbs and errors and never hold a
  `Control`; every check comes before anything is sent. Dropping one ends
  what it holds, so each is `must_use`; a voice can be detached to play
  out.
- **Voices belong to loaded audio,** `VOICES` each, a const generic chosen
  at `create`: a runtime count would allocate on every load, and a build
  setting is global and unseen. A play takes a free voice and, with none
  free, cuts that audio's oldest, because a press must sound; one audio
  never cuts another's. A global voice pool was dropped.
- **A voice is started and stopped through atomics in its slot,** with no
  ring: a ring per audio could not be shared by its handles nor returned
  to its place. Each play gets the next number, sent in one atomic that
  the renderer takes, leaving 0. A stop raises the slot's stop to the
  voice's number and never lowers it, so a stolen voice's handle cannot
  stop its successor; the renderer marks each voice done, and a slot is
  free once its last voice is. A flag beside the number was rejected for
  a double-start race; plain flags would forbid stealing held voices.
- **The mix is stereo, in blocks of 64 frames, clipped once, hard, last.**
  A master gain or limiter goes before the clip.
- **Inputs name capture channels and own none.** Every open input renders
  each block whether heard or not, so it can be recorded unmonitored;
  monitoring is off until asked for.
- **Recording is one handle per take,** into audio the caller allocated:
  stereo from any tap, ending when full, stopped, or when its input stops.
  Dropping the handle stops it. A take records from a tap, the main out
  after the clip or an input unclipped, chosen per take; the render loop
  writes at each tap point.
- **Arming is for the audio thread,** when it comes: a take that starts
  itself at a threshold, on a beat, or with pre-roll kept from before the
  press. Not built.
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
- A level meter and progress on a take; saving a take straight into
  loaded audio.
- Recording into plain samples or streamed out, so `AudioMemory` and
  `src/audio.rs` can go.

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
