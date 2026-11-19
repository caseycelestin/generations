# Roadmap

The plan as it stands: what is built, what comes next, what is open, and
the decisions that shape work still to come. The architecture is in the
crate (`cargo doc --open`); how it got this way is in git.

## Built

- **The project on disk:** samples stored as WAVs under a manifest, ids
  minted by the project, every change durable when the call returns.
- **One audio memory figure** (`AudioMemory`) for all audio in RAM.
- **The engine:** a duplex `Renderer` driven by the caller's device, and a
  `Control` that plays voices, opens inputs and records takes.
  - A fixed voice pool, mixed in blocks to a stereo main out.
  - Inputs from capture channels, each heard through the main out when
    monitored.
  - Recording from the main out or one input.
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

- **What lives on the audio thread.** Today it carries out tasks the
  control side hands it, and holds nothing of its own beyond them. The
  alternative is that it owns what is playable (pads, patterns, the
  transport, mixer and effect settings) and the control side edits that
  state and supplies its memory, so a press is "trigger pad 3" rather than
  a transfer carrying audio.
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
- **One format: 48 kHz `f32`,** on disk, in RAM and on the render path;
  each buffer is mono or stereo. `i16` storage was rejected: it loses
  24-bit sources at every generation, and the render path should read
  samples, not convert them.

### The project

- **The project is the disk.** A directory with no in-memory copy; every
  write is fsynced before the call returns, in an order that leaves the
  old manifest or the new one after a power cut, so there is no save step.
- **A sample exists once it is stored.** The project mints its
  `SampleId` from a counter that never reuses one; callers keep the id
  (in a parent pointer, a pad, a pattern) and `load` turns it into
  `Audio`.
- **Generation is a relationship, not a type.** A sample will carry an
  optional parent id, and lineage is the tree of parent pointers. Not
  built; it returns with prints, in the manifest's per-sample entry, with
  every parent minted before its child.
- **A project is validated whole on open** and refused if anything is
  off; the directory belongs to the engine. The manifest is a private
  struct, versioned apart from the model; samples are WAVs so any tool can
  open them.

### Memory

- **All audio-length memory comes from one `AudioMemory`,** made once per
  machine and lent by the caller; a buffer repays it on drop. What does
  not scale with audio (rings, the voice pool, scratch) is sized once by
  the caller and not counted.
- **The render side never allocates.** What it needs is allocated on the
  control side and handed over: live effects will be too.
- **Memory the audio thread writes first is prefaulted** on the control
  side (a take, later a delay line); a first write would otherwise stall
  the audio thread on a page fault.

### The engine

- **The project and the engine never name each other;** the caller
  carries `Audio` between them.
- **The device is the caller's.** The crate has no audio dependency and
  `render(capture, out)` is handed one duplex period; a caller with
  separate input and output clocks bridges them before `render`. A device
  change means calling `create` again.
- **The render side never waits:** no disk, lock, allocation or error
  path, and frame counts rather than a clock, so the same `render` runs
  offline.
- **A transfer goes over and comes home whole,** so everything it owns is
  freed on the control side. What changes while it runs (stop,
  monitoring) is lock-free state shared beside it, not further messages.
  Each feature has its own channel, sized by how many of it can be in
  flight.
- **A verb lives on the thing it acts on.** Handles (`Voice`, `Input`,
  `Recording`) carry their own verbs and errors, check before they start,
  and never hold a `Control`.
- **Voices are a fixed pool** sized by the caller; a play with every voice
  taken cuts the oldest, because a press must sound.
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
- Streaming long recordings rather than holding them in RAM.

**Project**
- Content-hash dedup, also matching a renamed WAV back to its sample.
- Removing a whole lineage.
- Projects stored in another format, converted on load.

**Hardware**
- Storage tiers: the active project on eMMC, kept projects on the SD
  card, audio in RAM under `AudioMemory`; the project does not know.
- An `i16` build, chosen at build time.
- The memory budget read from the device's configuration.
- `mlockall`, a cgroup reservation and no swap, set by the caller.
- Hardware monitoring on the codec.
