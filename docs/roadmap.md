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
| 6 | Recording: audio input as a new root | done |
| 7 | Input monitoring: hear the input through the output | done |
| 8 | Sequencing and mixing: the real-time side | |
| 9 | Arrangement and more effects | |

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
  sample except its audio, and grows as fields are added. The audio read
  is `Project::load`, which returns the audio as a block the caller owns
  (it was the engine's and crate-private until Handles, below). They are separate because structure is
  small and read often, audio is large and read only when a load needs
  it.
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
  nothing else is in a position to hold; `loaded` was the first, until
  Handles removed it (a loaded sample is now a value the caller holds, so
  the flag had nothing to mirror). More arrive as their steps come. `sample(id)` is where they are read, and
  each one that can change gets its own setter with its own rule, rather
  than one general update: the id and parent can never change, and a rule
  belongs next to the one field it governs.
- **The engine is the only way in.** *Reversed under Handles, below: there
  is no engine type, the project and the renderer's handle are two doors,
  and the rules hold by what each type can do rather than by one path.
  Kept as the argument that was made.* `Engine::project()` lends the
  project read-only; every change is an engine method. Some changes are
  only half a project write (`load` sets a flag and moves audio into RAM),
  and a caller reaching a role directly would leave the two disagreeing.
  So the roles the engine owns (`project`'s mutation, `dsp`, `device`,
  `storage`) are crate-private, and what is public is the engine, the
  project's read side, the value types in the engine's signatures, the
  errors, the effects and config. One path to each action, and the rules
  hold on it. A reader who wants a role's internals opens the module.
- **The engine's methods stay flat for now.** *Moot since Handles: the
  engine is gone and its verbs sit on the types they act on.* Most touch more than one
  role (`play` asks playback and, on failure, the project), so grouping
  them by role would mislead. If the list grows past easy reading, the
  grouping to use is by what the caller is doing (samples, sound, later
  pads and patterns), as `impl` blocks or as views the engine lends that
  borrow the whole engine so they can still cross roles. Not needed at
  nine methods.

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
- **One fixed format**: 48 kHz, set in `config.rs`. Import converts to it,
  nothing in between checks. Channel count is kept per buffer so mono
  samples do not pay for stereo. How import converts is under Import below.
- **One sample type, `f32`, on disk, in RAM and on the render path.**
  Decided 2026-11-09, replacing the day-one split of `i16` storage and
  `f32` processing. That split bought half the resident bytes and cost a
  second sample type, a dither and quantise step, and a conversion on every
  read the render side makes. Three things decided it. The Compute Module
  4's cores have hardware float with NEON, so `f32` is the native type for
  this work and `i16` is not a performance choice on this hardware; the
  pipelines that run in integers are on microcontrollers without a float
  unit. Serious gear records at 24 bits, which `i16` throws away on every
  import and every take, and which `f32` holds exactly; a device that
  resamples its own output as generations would apply that ceiling at every
  print. And the render path should read samples, not convert them: a
  conversion per sample per play is work the control side could have done
  once at load, if it needed doing at all. The cost is capacity: about 65
  minutes of resident stereo in a 1.5 GB budget instead of 130, and 32-bit
  float WAV on the card at twice the size of a 16-bit file. For a sampler
  whose project is mostly one-shots and loops, that is hundreds of pads,
  and the memory ledger counts it either way. The `Pcm` alias stays so the
  decision is visible in one place and reversible; `AudioBuffer` carries
  it over `Block<f32>`. The `i16` module, its dither, `to_f32`/`from_f32`
  and the quantise in `Take::write` went with the memory port.
- **An effect was a function**, `&AudioBuffer -> AudioBuffer`, and the
  engine took one as a plain argument to `print`. Removed in step 5 along
  with `print` and the one effect, reverse: by then the decision that
  effects live on the render side (under Deferred ideas) had been made,
  so the offline form was dead code that would not be written that way
  again, and it was the one allocation the ledger could only estimate.
  The generation model it exercised, a child sample with a parent, stays
  in the project and is tested there. Effects-as-data (an enum with
  parameters) is what the replayable recipe idea needs, and arrives with
  that work.

### Import

- **The project takes the file.** `Project::import(path)` (the engine's
  until Handles) reads, converts and
  stores; the caller sees no decoder and never learns the project format.
  The device left the engine because it is machine knowledge: which audio
  library, which hardware, how much RAM. File formats are product
  knowledge, the same on every machine the sampler runs on, so they stay
  inside. If the caller converted, every front end would repeat the same
  pipeline and the most common user action would lose its one path. The
  buffer-taking step is `pub(crate)`: the last step of an import and what
  a finished recording will call.
- **Two crate-private stages.** `decode` reads one file format into a plain
  value: rate, channels, interleaved `f32`. `convert` resamples and
  quantises that into a buffer. Decoders know formats, `convert` knows the
  project format, nothing knows both. A new format is a new decoder module
  and one more arm in the sniff; the format is decided from the file's
  first bytes, not its name. No decoder trait until a second decoder makes
  the shape obvious.
- **Resampling is `rubato`'s synchronous FFT resampler**, run over the
  whole clip so its startup delay is trimmed and the output is the clip's
  length at 48 kHz. Import is offline, so quality beat a hand-rolled
  interpolator; the rate pairs are fixed, so the synchronous (exact-ratio)
  resampler fits better than the asynchronous one.
- **Channel count is kept, not mixed.** Mono stays mono, stereo stays
  stereo, more than two is refused. A downmix is a creative decision the
  user should make, not one the import makes silently.
- **WAV covers integer PCM at 8 to 32 bits and 32-bit float.** Integers
  are scaled by their own width so every depth lands on the same range.
- **A file already in the project format is stored byte for byte.** A
  decoder reports samples as the file stored them: integers at their own
  width, floats as floats. `convert` alone notices when the rate and width
  are already the project's and stores them untouched; any other width at
  the project rate is quantised with dither, and any other rate is
  resampled. So re-importing a project's own WAV changes nothing, every
  future decoder gets that for free, and no decoder knows the project
  format at all.

### Playback

Step 4 was built up one decision at a time; the decisions are here and
the code follows them.

- **Sound has a control side and a render side, and the engine is the
  control side.** *Since Handles the control side's end is `Control`,
  not an engine; the split itself stands.* Pressing a pad is a front-panel action, and the engine
  is what the front panel calls; it owns the audio in RAM and the sending
  end of a command ring. The render side, the `renderer` module wired
  from the pieces in `dsp`, owns the receiving end and the playing
  voices and turns commands into frames.
  The split is by thread, not by feature: one side decides and may be
  slow, the other moves frames on a deadline and must never wait on the
  first. A `playback` module that held both halves was removed because
  the question "which thread am I on" should be answered by the module
  path. The pattern has several names: a command pattern over a lock-free
  single-producer single-consumer queue, an active object, control plane
  and data plane. The engine never reaches into the render side; every
  change to what is heard is a command, applied at the next period.
- **The render side is named for what it does, not where it runs.**
  `dsp` turns commands into frames and gives the same frames live from
  a device callback or offline in a loop, so "realtime" was rejected as a
  name (the code must run offline) and "process" was rejected (it collides
  with the OS process this roadmap talks about). Recording in step 6 got
  a sibling `record` module, a ring the other way; the pair is the data
  plane, named by direction, and the rules (no allocation, no lock, no
  disk, no error path, no clock) apply to both.
- **The engine mints the renderer and hands it out as its output.**
  *Since Handles: `Renderer::new()` returns the renderer and its handle as
  a pair, the way a channel comes as two ends; there is no `output()`.* The
  engine cannot expose a `fill(&mut self, out)` of its own: the audio
  thread must own what it calls for as long as the stream runs, and the
  control thread needs the engine for `play` and `import`; a mutex would
  make the audio thread wait on an fsync. So `Engine::output()` hands out
  the `Renderer` once, with `render` as its one public method, and the
  caller moves it into whatever drives it. The engine calls it its
  output, because that is what the caller holds; the type is named for
  what it does. A separate `Output` wrapper type was tried and removed:
  it added a name without adding behaviour. Commands sent before the
  output is taken wait in the ring. One set of loaded audio and one ring per
  live renderer; if a second output (a cue out) ever exists, the engine
  routes to it and the render side never learns there is another.
- **Playback reads only audio already in RAM.** The real-time thread never
  touches storage: storage reads the manifest from disk and fsyncs on every
  write, and an fsync on eMMC can stall other I/O for hundreds of
  milliseconds. Streaming from eMMC is not bandwidth-bound (a stereo voice
  is 192 KB/s against tens of MB/s) but it is latency-bound, and our own
  prints are the main contention. Streaming long recordings is a deferred
  idea.
- **Residency is explicit and persisted.** *Reversed under Handles: a
  sample is in RAM while its `LoadedSample` is held, nothing is persisted,
  and `open` loads nothing.* A sample is in RAM because
  someone asked, not because it was played. `loaded` on the sample is the
  recorded intent; the engine loads every flagged sample on open, so the
  device comes back ready after a power-down. `Engine::load` and
  `Engine::unload` change it. Playing a sample that is not loaded is an
  error, not a silent disk read, so a press never pays for a load and
  latency never depends on history. What to load is the caller's policy;
  the engine never loads on its own. Load-on-play is two lines in a
  caller that wants it.
- **Unload stops every voice playing the sample.** *Since Handles the
  verb is `Control::stop(&sample)`, by reference, and unloading is
  dropping the sample; the eviction and its address argument are as
  below.* Before this, a voice
  still playing an unloaded sample kept its `Arc` and played to the end,
  so sound came from memory the engine said it no longer held. Unloaded
  means gone from RAM. The fix is a third command keyed the third way:
  `Stop` is by voice, `StopAll` by nothing, `Evict` by buffer address.
  The engine sends the address of the buffer it is removing and the
  renderer drops every voice whose `Arc` points at it, one pass over at
  most `VOICE_COUNT` voices; the render side still never learns a sample
  id. `unload` sends first, which is what keeps an address a safe key,
  and fails with `CommandsFull` if the ring is full, like `play` and
  `stop`. The argument and the contract are on `BufferAddress` and
  `Engine::unload` in the crate docs.
- **Adding a sample and loading it stay separate.** A print the user made
  must land on disk even when it does not fit in RAM. Combining the two
  would have to either refuse the add or store the sample unloaded anyway,
  which makes the flag a request rather than a fact. The cost is a disk
  read of bytes the engine was just holding, a few milliseconds per print.
- **The budget is enforced where RAM is managed.** The caller makes the
  `Ledger` and hands it to the project (it passed a byte count to
  `Engine::open` until Handles); storage never learns about RAM. In step 4
  a resident set checked it, being the one thing that knew how many
  bytes it held. Step 5 moved the check into a ledger, under Memory
  below. `Engine::load` puts the audio in RAM first and sets the flag
  only if that succeeded, so the flag is never true for a sample that is
  not in RAM. The loaded audio is private to the engine: "is this loaded"
  is answered by the sample's flag, and "how much RAM is in use" gets a
  number on the engine when a panel asks for it, not a lent struct.
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
- **The render side is a renderer wiring two mixers over shared
  arithmetic.** Rebuilt 2026-11-09 (PR #43) after the first shape, a
  `Voice` type the renderer kept in a deque, stopped fitting the taps and
  busses coming next; reshaped again the same day when step 7 brought a
  second kind of source (see Monitoring). A `Playhead` is a read position
  in a loaded sample: `add` puts its next period into the output and
  moves on, `finished` says when it has reached the end. Start and end
  points, looping and speed are fields on it when they come. A `Mixer<S,
  N>` is `N` slots for one kind of `Source`, each the sound the engine
  named by a `SoundId`, and its methods are the whole polyphony policy:
  `start` takes a free slot or steals the oldest, `stop` by id,
  `stop_all`, and `add`, which sums every slot and frees the finished
  ones; `evict` by buffer address exists only on the playhead mixer. The
  renderer owns a sample mixer of `MAX_SAMPLE_SOUNDS` and an input
  mixer of `MAX_INPUT_SOUNDS`, applies the commands that arrived, zeroes the
  slice it is given, has each mixer add into it, and clips the sum once.
  It is real-time safe, not real-time only: a device drives it live from
  its callback; a bounce drives it offline in a loop. The name is Kira's;
  "controller" was rejected because the engine gives the orders and this
  side carries them out. The loaded audio and the mixers do not overlap:
  one says which samples are in RAM, keyed by sample; the other says what
  is sounding now and where in it, keyed by sound. They share only an
  `Arc` to the same bytes. Until step 7 the sample mixer was a type of its
  own, `Voices`, and the roadmap entries above and below written before
  then keep that word.
- **A piece of DSP is a struct with the work as a method, or a free
  function if it has no state.** The convention is in the `dsp` module
  doc. A piece works in place on one period of interleaved stereo
  frames: a producer adds, a transformer overwrites, an observer leaves
  the slice alone. A piece with state owns it, is built whole by a
  constructor and keeps its fields private, so an invariant has one
  place to hold; it may say it is finished so whatever holds it can let
  it go. State is built on the control side, where it may allocate, and
  moved to where it runs. The pipeline is hard-coded in the renderer and
  will stay so: no runtime graph. Two other shapes were built and
  removed on the way. A `Processor` trait with boxed chains added a name
  for something every piece could already be without it, and a plain-data
  struct with free functions over it lost the constructor that keeps a
  value from being half made. Rust supports both; it is built around
  values that guard their own state, so the method shape is the one that
  reads as Rust.
- **The engine mints the `SoundId`.** The ring is one way and
  `Engine::play` must return something to stop by, so the control side
  names the voice before it sends `Play`, and `Stop` carries the same id.
  Named `VoiceId` until step 7; see Monitoring for the rename.
  Ids come from a counter that only goes up and is bumped only once the
  push succeeds, so a refused press burns nothing, an id is never reused,
  and the lowest id among the voices is the oldest, which is what
  stealing needs. A stop for a voice that finished or was stolen names
  nothing and does nothing. The render side chooses the voice; the engine
  never learns which.
- **Voices add into the caller's buffer, voice-major.** Whoever drives
  the renderer owns the period buffer and hands it in; the renderer never
  allocates one.
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
- **Voices are a fixed array of `Option<Voice>`, scanned.** Every
  method is one pass over `VOICE_COUNT` entries: find a free one, find
  the lowest id, drop the ones on a buffer. With the count a small
  compile-time constant that is a fixed cost, smaller than the audio each
  period reads, and it needs no second structure kept in sync. A
  `heapless::Deque` FIFO was the first shape, stealing by pop and push;
  it stopped fitting once `stop` and `evict` had to remove from the
  middle, and the ids it was meant to avoid are needed anyway so a press
  can be stopped. If the count ever grew to where the scans showed in a
  profile, the fix is an id-to-index table on the control side, not a map
  on the audio thread.
- **The ring is `rtrb`.** Single producer, single consumer, lock-free,
  fixed capacity, written for audio. Popping on the audio thread is a few
  atomic loads and never blocks. A full ring drops the push on the control
  side, which is the right failure. `std::sync::mpsc` locks and can
  allocate; writing our own is where subtle bugs come from. The renderer
  owns the consuming end (as Kira does) rather than taking commands as a
  parameter (as JUCE does): there is one host and one feeder, and the
  device closure is then one line.
- **The device is the caller's, not the crate's.** Which machine the
  sampler is on, and whether it has an audio device at all, is knowledge
  the caller has and the engine does not; the same reasoning that keeps
  the budget out of the engine. So the engine never opens a device, and
  the library has no audio dependency: `examples/device/` is the one
  piece of code that knows `cpal` exists and that there is an audio
  thread, `cpal` is a dev-dependency, and the library builds without the
  ALSA headers. `Device::default_output()` picks the system's default
  output and checks it runs 48 kHz stereo, in `f32` by preference or in
  `i16` or `i32` with the conversion done in the callback, since `cpal`
  converts nothing and a raw ALSA `hw:` device, the usual I2S DAC on the
  target, often offers only integer formats; `start(callback)` runs the
  callback on the audio thread and returns a `Stream` that plays while
  held. Making sound is three lines in the caller: open the engine, take
  its output, start a device with it. A headless machine (CI, a CM4
  before its DAC is wired, an ssh session) runs the engine all the same.
  The device code moved out in three steps once the engine stopped
  needing it; it had been a crate role because the engine used to open
  the stream itself.
- **`cpal`, and no trait of our own.** `cpal` is already the abstraction
  over ALSA, PipeWire, JACK, CoreAudio and WASAPI, and it runs on the
  development machine and the Pi with no code change. A trait over it
  would have one implementation; tests do not need a fake device because
  they hold the renderer. Going direct to ALSA, or to RtAudio for a duplex
  stream, is the day a second implementation exists. `cpal` needs the ALSA
  headers to build, so CI installs them for the examples.
- **The stream runs from open to drop, and the caller drops it before
  the engine.** Opening a stream takes tens of milliseconds and often
  clicks, so the caller opens it once and it renders silence when nothing
  plays, as every synth does. Stopping it when idle would put the open
  cost on the first press. Because the caller owns both, drop order is
  the caller's discipline: a voice still ringing holds a reference to its
  audio, and if the engine goes first that reference is the last one and
  the buffer is freed on the audio thread. The renderer's doc says so.
- **The period is the backend's default for now.** It is the latency
  knob: a press lands within one period, and the renderer's work per
  callback is bounded by it. On the Pi it should be chosen deliberately
  (`BufferSize::Fixed`) once the device is in hand; on desktop Linux the
  default is around ten milliseconds and good enough to hear.
- **Recording is a second stream.** `cpal` opens input and output as
  separate streams with separate callbacks, so recording got its own
  input type in the caller's device code, a `record` module beside
  `dsp` with its own ring back to the control side, and the render side
  never learns about it. Two streams have no guaranteed
  frame alignment between them; if a resample must land on the frame,
  that is the case for a duplex stream, and it is a change inside the two
  roles.
- **Nothing on the render path reads a clock or the device.** Frame counts
  only, with the rate from config. This is what makes an offline bounce
  possible: build a renderer, drive `render` in a loop faster than real
  time, and the frames are the same ones live playback would produce.
  The bounce is the renderer used as a function, and belongs beside
  effects as another kind of print; it does not go through the live
  renderer.
- **Both constants are set generously, and only one of them stays a
  constant.** The real voice count depends on what a voice costs with
  pitch and processing; someone measures that on the target and picks a
  number under it, once, and it becomes the sampler's polyphony, the same
  on every unit. The machine bounds it, but a person chooses it, so it
  stays a constant until the target has variants that disagree. The
  budget is different: CM4s ship with 1, 2, 4 or 8 GB and the right
  number depends on the cgroup reservation and on what else runs, so no
  constant can be right and the deployment has to supply it. It is a
  parameter on `Engine::open` and there is no constant for it in the
  library; each example carries its own guess. The engine never reads the process
  environment; the binary may, and passes what it learns. The format
  constants (rate, PCM width, channels) are a different kind again: they
  define what a project is, and two machines must not disagree about the
  same files.

### Memory

Step 5. Decided before recording because a take in progress is the first
thing the engine holds in RAM that the resident set cannot serve.

- **One figure covers everything the engine holds in memory.** Resident
  audio, buffers in flight (an import's decoded and converted buffers, a
  print's input and output, a take in progress) and, later, the state of
  live effects all count against the budget passed to `Engine::open`. A
  separate bound on sample length was the earlier plan, so the deployment
  could reserve the budget plus the largest transient. But the largest
  transient is not one buffer: a print holds the i16 input, its f32 copy,
  the f32 output and the i16 output at once, so the reservation would be
  the budget plus several multiples of the bound, with the multiple
  depending on the operation. That is the engine's arithmetic done from
  outside by a formula the caller has to get right. One figure moves it
  inside: the caller sets the number to what the cgroup gives the
  process, less fixed overhead, and the engine never asks for more audio
  memory than that.
- **The ledger is a quota, not an allocator.** One crate-private type
  holds the figure and the bytes currently promised. A client asks for so
  many bytes and gets a reservation or a refusal, then allocates on the
  ordinary heap, as decided under Playback; the OS reservation is still
  the real guarantee. The ledger never owns a buffer and never hands out
  a pointer. It adds the missing half of that decision: the engine never
  promises more than the figure, so the cgroup only has to cover one
  number.
- **The check happens in one place.** The only comparison against the
  figure is inside the ledger's reserve. A reservation is a value held
  next to the buffer it paid for and gives the bytes back when it is
  dropped, so release is automatic and cannot be forgotten or counted
  twice. Call sites are many but identical: ask once, hold the
  reservation, drop it. None of them knows the figure or what else is in
  RAM. A single budget checked by arithmetic at several sites was
  rejected as the thing that would get messy.
- **Whatever allocates in proportion to audio length takes a reservation.**
  Decode, convert and the storage read each take a `&Reservation` and
  refuse, before allocating, one that does not cover what they are about
  to allocate. So asking first is not a rule a call site can forget: a
  call without a reservation does not compile, and the size check sits
  in the one function that knows the real size, not in the engine after
  the fact. The allocators only borrow the reservation; the engine keeps
  it and decides when the bytes go back. They never learn the figure.
  Effects are exempt because they are placeholders for the render side,
  and so is everything that does not scale with audio: the manifest, id
  lists, error strings, a decoder's read buffer, the resampler's
  chunk-sized state. That is the fixed overhead the deployment sets the
  figure below.
- **The reservation comes before the allocation, so the size is known
  first.** A load that read the audio and then asked would hold memory
  the ledger never approved, for exactly the moment the budget is meant
  to cover. So the sample record carries the audio's channels and frame
  count, the engine reserves from those, and only then reads. The file
  backend takes them from the WAV header rather than repeating them in
  the manifest, so there is one record of the shape and nothing to fall
  out of step; the storage read refuses a reservation that does not
  cover what the header says before it reads. Import has its sizes in hand
  before it allocates (the file header, then the decoded audio) and
  reserves the same way. Asking whether a sample exists stays a
  manifest-only question, so a sample whose WAV has gone bad can still be
  unloaded and removed.
- **The resident set dissolved into a map on the engine.** Once the
  ledger took the counting, the set was a `HashMap` behind four one-line
  methods, one of which repeated a check the engine already made. It is
  now the engine's `loaded` field: sample id to the buffer and the
  reservation that paid for it. Its contract is unchanged and lives on
  the field: every entry is immutable behind an `Arc` so a voice can
  hold it, and playable now. The name matches the project's `loaded`
  flag, because they are two views of one fact. A type can be pulled
  back out when a second output or an eviction policy gives it work.
- **A take in progress is an in-flight buffer, not a resident one.** It
  has no id, because storage mints ids and a sample does not exist until
  stored; it is being written, so it cannot be an immutable shared buffer;
  and it is not playable. Fitting it in with the loaded audio would cost
  that map an optional key, interior mutability and an unplayable flag. So
  it lives outside that map, under its own reservation, and goes through
  the same crate-private add as import when it is saved. This step
  imagined the buffer growing as frames arrived and ending when growth was
  refused; step 6 decided otherwise, a recorder sized and allocated once,
  and how a saved take becomes loaded is decided there too. See
  Recording.
- **A fixed maximum record time is policy, not a RAM parameter.** Under
  one figure, how long a take can run depends on what is loaded. If a
  predictable limit matters as a product feature, it is the caller
  stopping the take at N seconds, or a product constant chosen once like
  the voice count. It is not a second number on `open`.
- **A print reserved its cost up front, as an estimate.** An effect was a
  plain function that allocated its own f32 working copies, which the
  ledger could not see, so the engine reserved input and output as PCM
  and both again in f32 before calling it. That was the one place the
  figure rested on an estimate rather than a check inside the allocator,
  and one reason the offline print went. When effects come back on the
  render side they are built on the control side and reserve exactly, as
  the next bullet says.
- **An import holds two reservations, each taken from the thing in hand.** The
  decoder reads the file's header and says how large its samples will be;
  the engine reserves that, then decodes. The converter looks at the
  decoded audio and says what it will allocate on the path it is about to
  take (the exact path only the result; the same-rate path an f32 copy
  too; resampling an f32 copy, the f32 output and the result); the engine
  reserves that, then converts. The first reservation drops with the
  decoded samples, the second when the import returns. Each side states
  its own cost from its own input and checks the reservation it is handed
  against it, so the engine never learns how a decoder or converter
  works, and the header is read twice rather than carrying a half-open
  decoder between two calls.
- **Live effects will reserve at instantiation, on the control side.** A
  delay line or a reverb owns fixed buffers for its whole life, and the
  render path may not allocate, so they are built when the effect is
  created, moved to the render side with it, and released when it is
  removed: the same life cycle as a loaded sample. The render side never
  learns there is a budget. A user who loads a big reverb has less room
  for samples, which is true on the hardware and should be true in the
  number.
- **The number is only true if every large audio allocation asks first.**
  One uncounted buffer and the figure is a lie again. The reservation
  parameter makes that structural for the allocators that exist; the
  thing to check in review of every later step is that a new one takes
  it too.
- **What a reservation is for is a label, not a mechanism.** A reservation can
  carry a sample id, a take, an effect, so a panel can show RAM in use by
  kind. Nothing enforces anything per kind, so the label arrives with the
  panel that reads it. Sub-budgets, eviction, priorities and a custom
  arena are not part of this step and the ledger does not invite them.
- **The ledger does nothing visible on a desktop.** Its value is entirely
  on the target, where there is no swap and the cgroup cap means an
  allocation past the figure is the OOM killer ending the engine
  mid-performance rather than a refused call the panel can show.

### Recording

Step 6. The first audio to enter the project without coming from a file,
and the first thing the audio thread hands back to the control side.

*The `Recorder` and `Take` described here were removed in the memory port
(PR #40, 2026-11-09) rather than carried through it, since recording was
to be reshaped on the ledger. The decisions stand as the shape recording
will return in; the code does not exist today, and the engine they name
is gone (Handles, below).*

- **The recorder is the control side and owns the memory.** The engine
  mints a `Recorder` for so many frames of so many channels, reserving
  the bytes against the one figure as it does, and the recorder allocates
  the whole buffer once, empty, when it is made. Nothing recorded through
  it allocates again. The caller keeps the recorder and drops it to give
  the memory back; the engine keeps no record of recorders or takes and
  meets a recording only when asked to store it. Reserving per take was
  the earlier shape and was dropped: a recorder is the thing with a size,
  and a take is one use of it.
- **Every reservation is made by the engine.** Written as a rule here,
  when the recorder became the second holder of audio memory after the
  loaded map. A recorder holding its own handle on the ledger was tried
  and removed: the ledger has one holder, and anything else that needs
  memory is given a reservation the engine made, by reference if it only
  uses the bytes and by value if it keeps them. One place to read to know
  everything that can be promised, and nothing below the engine can grow
  what the engine holds.
- **The take is the audio side.** `start_take` moves the recorder's buffer
  into a `Take`, which goes wherever the frames are, a device callback or
  a loop, and is fed interleaved `f32` frames. Each sample is quantised to
  the project format as it lands, whole frames only, never past the
  capacity, so a write is a bounded copy and nothing allocates. The take
  finishes when the buffer fills or when it is dropped, and either way
  hands the buffer back over a one-slot ring. Unlike playback, nothing
  travels from the control side to the take: it is given everything at
  the start and has no state the engine wants to change, so there are no
  commands, and the one crossing is the buffer coming home. A stop
  command was considered and left out until a caller needs it; ending a
  live take early is dropping the stream that holds it.
- **Full is not an error.** A take that fills did exactly what it was asked.
  It hands its buffer back at once, while the stream is still running, so
  the recording can be saved without waiting for the stream to stop, and
  frames offered after that are dropped without a word. There is no count
  of dropped frames and no return value from `write`. A take ended early
  keeps what it has; a short take is a take.
- **One take at a time, and a new take starts over.** There is one buffer,
  so `start_take` is refused with `Busy` while a take is out, checked
  first and before anything changes, and `is_recording` says so without
  asking. Once the take is back, the next one clears the buffer, keeping
  its capacity, and what the last take recorded is gone unless it was
  saved. That is the sampler's rule: record until you like one, then
  save. A compile-time version, a take that borrows the recorder, was
  rejected because a borrowed take cannot be moved into a device callback,
  which must own what it holds.
- **Saving goes through the engine, once.** `Engine::save_take(&mut
  recorder, keep_loaded)` stores what the recorder last recorded as a new
  root and returns the id. The recorder lends its buffer wrapped as an
  `AudioBuffer`, which is backed by a `Vec` for exactly this so a buffer
  with spare capacity wraps and unwraps without a copy, and takes it back
  afterwards: cleared if the store succeeded, so a recording is saved once
  and the next save finds `Nothing`; kept if storage refused, so it can be
  saved again. A save while a take is out is refused. `recorder.save(&mut
  engine)` reads better and was considered; it would make the record
  module import the engine, bending the one-way arrow the data plane
  keeps, for word order. The engine form matches `import`, the other way
  audio enters the project.
- **Keep-loaded pays before it stores.** The recorder keeps its buffer for
  the next take, so a saved take that stays loaded is a copy, reserved and
  made before anything is written to disk. A save that would not fit in
  RAM is refused whole with nothing stored and the recording still in the
  recorder, to be saved without keeping it. It costs what `load` would,
  minus the disk read. The copy is inserted and the flag set in the order
  `load` uses: audio in RAM before the flag says so.
- **There is one quantiser.** `Dither::quantise` turns one normalised
  sample into PCM, clamped, dithered and rounded, without allocating.
  `AudioBuffer::from_f32` and `Take::write` both call it, so a recording
  and a converted import are quantised the same way, and a test checks a
  take against `from_f32` on the same input.
- **The take has no error path, and needs none.** Everything that can go
  wrong happens where the engine is: a shape no buffer can have or a
  budget that will not hold it at `recorder`, a take still out at
  `start_take` or `save_take`, nothing recorded at `save_take`, storage
  refusing. A device failing, an xrun or the input vanishing, is the
  device's to report, and the device code already queues those for the
  control side; the take just has fewer frames. A take whose recorder was
  dropped first pushes to a ring with no receiver and frees its buffer
  where it is dropped, which on the audio thread is the deferred free the
  renderer already has.
- **The device input lives in the examples, like the output.** `Capture`
  (first called `Input`, a name the engine's own handle took in step 7)
  mirrors `Device`: the default input, picked once for the engine's rate
  and the device's own channel count, with 16- and 32-bit frames converted to `f32` in
  the callback, and the config picking shared with the output side. `first_take` records a few seconds into a take sized to fill at
  the end, drops the stream, saves the take loaded and plays it back. The
  input stream starts late, so the example sleeps a moment past the take's
  length. It is an example: it shows the path once on one machine, and is
  not made to cope with every device.
- **The caller is a patch bay; the engine is the mixer.** Which device,
  which channel, duplex or two streams, what format: the caller's, and
  thin, one line per destination with no decisions in it. What is heard,
  at what level, into which bus, whether a monitor mutes when a take is
  armed: the engine's, made through engine calls and arriving on the
  render side as commands. The test for the line being in the right place
  is that the caller's device code never contains an `if` about what the
  user wants to hear. Input monitoring is the first thing that needs the
  input and output callbacks to meet, and step 7 wrote the shape down
  under Monitoring.
- **Named `record`, not `capture`.** The step is recording, the type is the
  recorder, the engine call is `recorder`, the module is `record`: the
  same verb-and-agent shape as `render` and `Renderer`. `capture` is the
  device-side word (ALSA's playback and capture) and makes an awkward
  agent noun; this roadmap used it before the module existed and was
  corrected when it did.

### Monitoring

Step 7, built 2026-11-09 and reshaped 2026-11-10. Hearing the input through
the output needs the capture and output callbacks to meet, and it decided
more about the render side than about input.

- **An input is the ability to put audio into the renderer; monitoring is
  whether that audio is heard.** They are separate. `Engine::input(&[
  Stereo, Mono, Mono], period)` adds an input laid out like its device:
  it makes one lock-free ring per source, mono or stereo wide and a few of
  the device's periods deep, sends each reading end to the render side as an `InputReader` on
  its own `InputCommand::Add { id, reader }`, and returns the writing
  ends as one `Input` handle with an `InputId` for the handle and one
  `SoundId` per source. The caller moves the handle into the capture
  callback and calls `feed(frames)` with each period as the device
  delivered it, whole interleaved frames; `feed` splits each frame into
  the sources' rings, so the copy happens once, on the capture side, where
  the interleaved frame already is. Every source is silent until
  `Engine::monitor(sound)`; `unmonitor(sound)` silences it again and the
  input stays; `remove_input(id)` takes every source out. The first
  version tied the two together, one call that both added the input and
  heard it and one that did both in reverse, and was split because an
  input will also be recorded, and later fed to a bus, with monitoring
  being one switch among those.
- **The input mixer's mute is the monitor switch.** An input sound is a
  slot in the input mixer like a sample is in the sample mixer, so hearing
  it is unmuting it: `Mixer::set_muted(id, bool)`, and a muted slot's
  source `skip`s the period instead of `add`ing it, so a muted sample
  still ends and an unmonitored input still keeps up with its ring (there
  is no backlog when it is heard again). `Source` is `add`, `skip`,
  `finished`. Mute is generic, so a pad mute later is the same method on
  the sample mixer. A ring per source means nothing is parked when an input
  is removed, and each reading end crosses on a command the way a sample's
  `Arc` does. A single ring made at `open` and handed out once was built
  first and replaced; so, later the same day, was one ring per device
  with the sources as entries inside the reader and a second switch,
  `monitored`, that the mixer could not see (see below).
- **The input is a source in a mixer, not a voice and not a monitor
  type.** Three shapes were tried. A `Monitor` piece beside `Voices` with
  its own level repeated the voices' loop and the playhead's channel
  spreading. Putting the input into the one voices table as another kind
  of voice needed an exemption from stealing (a sixty-fifth pad press
  must not mute the microphone) and somewhere to park the ring's reading
  end when the voice was stopped. The shape kept: `Voices` became
  `Mixer<S, N>`, generic over a `Source`, and the renderer holds one mixer
  of playheads and one of `InputReader`s with `MAX_INPUT_SOUNDS` slots,
  one per source of every input. Each mixer has its own `WhenFull`, chosen
  when the renderer builds it: the sample mixer steals the oldest,
  because a pad press must always sound; the input mixer rejects the
  newcomer, because an input already heard must never be cut by another
  being added. The engine keeps its inputs with their sources' sounds and
  refuses an input whose sources would not all fit with `InputsFull`
  before anything is sent, refuses `monitor` and `unmonitor` for a sound
  that is not an input source with `NotAnInputSource`, and `remove_input`
  for an input it does not hold with `NoSuchInput`; the mixer's reject is
  the backstop, never the path. The first version let the input mixer
  steal like the sample mixer, which would have left the caller's oldest
  `Input` feeding a ring nobody read, with no error and no signal. An
  `InputReader` is one ring's reading end and its channel count, 1 or 2;
  it is finished when its `Input` is dropped.
- **Three functions carry the arithmetic: `stereo`, `add`, `hard_clip`,
  in `dsp/math.rs`.** Making stereo frames out of interleaved samples of
  any width, by an index per side, is the same operation for a mono
  source (both sides from channel 0) and a stereo one (0 and 1), sample
  or input alike; it was written twice before this.
  `add` sums frames into the period. Every source ends with `add(out,
  stereo(..))` and keeps only what is its own: the playhead a position,
  the reader a ring and a lag rule, the mixer slots and a `WhenFull`.
  There is no gain parameter: level will be per mixer slot when it comes,
  beside mute, not per source. A stereo source heard as stored goes
  through the same index remap as everything else; a fast path was
  considered and left until something measures it.
- **An input is shaped like its device, and split into its sources on
  the capture side.** A capture device hands its callback one buffer per
  period with every channel interleaved, so that is what the engine
  takes: `Engine::input(&[ChannelLayout])` where each entry is `Stereo`, the
  next two channels heard left and right, or `Mono`, the next one heard
  on both sides. Interleaved stereo is left then right everywhere, so a
  pair needs no further description. Each source is a sound of its own,
  with its own ring, slot and monitor switch, so a four-channel interface
  is one input of a pair and two microphones, each heard or not. The
  split from device frame to source rings is in `Input::feed`, one pass
  over the period; two rings of two channels hold what one ring of four
  would, so the footprint is the same. The layout is fixed for the
  input's life: a device that is to deliver other channels is closed and
  reopened, and so is its input. Two constants: `MAX_INPUT_CHANNELS`
  bounds one input's width, `MAX_INPUT_SOUNDS` is the input mixer's slot
  count across every input. Five shapes came before this, all the same
  day. `INPUT_CHANNELS` as a fixed capture width like `OUTPUT_CHANNELS`:
  the output width is the engine's choice and the capture width is the
  device's. The width per input with a channel map picked on the render
  side: it repeated the device's width on every call and gave the engine
  a map it had no use for. Mono or stereo inputs with the caller
  splitting the device's frames: it put a copy loop in every caller for
  what the device had already laid out. One ring per device with the
  sources as entries inside its reader: the mixer's slots were devices
  while everything acted on was a source, so the mixer's mute sat unused
  beside a second switch it could not see, a `Monitor` had to be offered
  to every reader until one claimed it, and a slot carried two kinds of
  id. Splitting in `feed` instead keeps the device-shaped call and makes
  a source an ordinary slot, so the render side never sees anything but
  mono and stereo sources, samples and inputs alike. Numbered input slots
  were considered and left out: the engine does not care which input is
  "input 1"; the panel holds the ids and labels them, and nothing
  persists them yet.
- **An input is not a sound, so it has an `InputId`.** *Since Handles
  there is no `InputId`: the `Input` handle is the identity, its sources
  are `InputSource` values with `monitor` on them, and dropping the handle
  removes the input.* The `Input`
  handle is not in any mixer; its sources are. `remove_input` takes the
  handle's `InputId`, `monitor` and `unmonitor` take a source's `SoundId`,
  and mixing them up does not compile. The engine mints the two from
  separate counters and keeps a map from each input to its sources' sounds,
  which is all the control side knows about inputs. For a while the input
  itself was a mixer slot keyed by `InputId` through a `Source::Id`
  associated type; with a ring per source that went, and both mixers are
  keyed by `SoundId` again. Ids rather than handles because the things
  they name live on the audio thread or inside a capture closure, and a
  command must be a plain value; ids are what a panel stores.
- **To revisit: how sounds, inputs and samples are managed.** *Revisited
  under Handles: ids left the public API, `remove_input` became a drop,
  and the one shared minter is `Control`. The retire ring and
  finished-sound reporting are still open, deferred into the mixer
  rewrite.* Three
  kinds of thing now have ids and lifetimes: a sound (a sample playing or
  an input source), an input (a ring, removed by hand with `remove_input`)
  and a loaded sample (an `Arc` the engine holds). Dropping an `Input`
  already finishes its reader; `remove_input` exists only because the
  engine's map of inputs cannot see a drop and because the ring would
  otherwise be freed on the audio thread. Both are the retire-events ring
  under Deferred ideas. When that lands, `remove_input` should go and
  dropping the handle should be the whole API, and the same pass should
  decide whether `SoundId` and `InputId` want a shared minter and whether
  a finished sound should be reported back at all.
- **Routing, for step 8.** With a ring per source, an input sound is the
  same kind of thing as a sample playing, a `Source` in a mixer, so level,
  pan and a send are written once for both. Routing a sound to a second
  destination, a bus or a recorder beside the output, is the open
  question: a ring has one consumer, so a source cannot sit in two mixers
  by itself. Either the consumer fans out, a reader that adds into several
  slices, or the second destination taps the source's frames after the
  first has taken them. Not decided; there is no second destination yet.
- **Plan for the mixer, not built in step 7: keep it stupid.** The
  shape to move to when step 8 needs levels, effects or busses. A `Node`
  trait with `process(frames)` in place and `finished()`: a source
  overwrites the slice with its next period, an effect transforms it, so
  one trait covers both and `add` versus `process` goes away. The mixer
  owns one scratch buffer of one period and, per slot, fills it from the
  node and sums it into the output; nothing else. Mute and level leave
  the mixer for a `Gain<N>` node wrapping a source, so an input is added
  as a gain at zero and monitoring turns it up, and `skip` and the slot
  mute disappear because a muted source still fills scratch and keeps its
  place. Stealing leaves too, to a voice table in front of the sample
  mixer that decides which slot a press takes. The mixer then implements
  `Node` itself, so a mixer in another mixer's slot is a bus with no new
  code, which is how JUCE, SuperCollider and fundsp get busses. The
  scratch needs the output period, so `Engine::output(period)` builds the
  renderer there, paid for on the ledger, the same shape as
  `input(layout, period)`. In-place `add` was chosen to avoid knowing the
  period; now that the period is a parameter, the plain fill-then-sum
  mixer every library has is the simpler one.
- **Two clocks, one ring, a lag rule.** cpal has no duplex stream, so the
  capture and output callbacks run on their own schedules even on a codec
  with one clock, and the ring between them drifts. The reader takes one
  period per render, heard or not. An empty ring is a gap. More than
  `MAX_LAG_PERIODS` (two) of output queued is latency the ear notices, so
  the surplus is skipped, oldest first, and the input catches up with a
  click. A full ring drops whole frames on the feed side, never a partial
  one, so the channels never swap. The ring's depth is sized from the
  capture device's period, which `Engine::input` takes as a parameter:
  the depth has to be several periods or the lag rule and the drop rule
  fight, and the period is the device's to know, like the RAM budget is
  the machine's at `open`. The engine bounds it with `MAX_PERIOD_FRAMES`
  (4096, 85 ms), a bound on what the engine supports rather than a guess
  at the device, and that bound is what keeps the rings off the memory
  ledger: they scale with the period and the layout, both bounded, not
  with the audio held, so they are fixed overhead with a worst case
  computed at compile time, `input::MAX_RING_BYTES`, a few megabytes.
  That is what memory-conscious programs do: count the few things that
  can grow without limit, measure the rest once and pad. A fixed depth of
  4096 frames came first and was right only by luck of the usual buffer
  sizes; then the rings were paid for on the ledger through a
  `Reservation`, which was exact and one mechanism too many for a few
  megabytes that do not scale with audio. The example asks its capture
  device for a fixed period so the figure is known before the stream
  runs.
- **A command names its mixer first.** `Command` is two variants,
  `Sample(SampleCommand)` and `Input(InputCommand)`, one per mixer the
  renderer holds. `SampleCommand` is `Play`, `Stop`, `StopAll` and
  `Evict`; `InputCommand` is `Add`, `Remove`, `Monitor` and `Unmonitor`.
  The renderer's `apply_commands` only routes, two arms, and each mixer
  has an `apply` for its own commands beside its own state, so adding a
  command touches one mixer and not the renderer. The first version was
  one flat enum of eight; it worked, but every command added an arm to
  the renderer that knew what a stop or a mute was, and the doc comment
  was already sorting the variants into two groups. A stop-all is a panic
  button for the pads and must not mute the microphone, and a `SoundId`
  names a sound in one table or the other, so a command meant for one
  side cannot land on the other. On the engine, `stop` and `stop_all`
  are for samples; `input`, `monitor`, `unmonitor` and `remove_input`
  for inputs. `Engine::send` takes `impl Into<Command>`, so each call
  builds the inner command and the grouping is spelled once. The handle
  stays `Input`, since it is the capture end of the input and not the
  hearing of it.
- **`dsp` is the pieces, `renderer` is this engine's pipeline.** The
  grouping above made a seam visible: a generic `Mixer` had grown an
  `apply(SampleCommand)`, so the reusable piece imported the one
  pipeline's command enums. Now `src/dsp/` holds only what any pipeline
  could wire, `math`, `Mixer` and `Source`, `Playhead` with `BufferAddress`
  beside it, `InputReader`, and `SoundId` at the module's top, and knows
  nothing of commands. `src/renderer/` holds the
  one pipeline: `Renderer`, which picks two mixers and an order, and
  `command.rs`, which is its API. The two `apply`s are private methods on
  `Renderer`, `apply_sample` and `apply_input`, so the mixer never sees a
  command. The folder is `renderer`, after the type, and not `render`,
  which was `dsp`'s old name before step 4's layout and would read as a
  revert. A second pipeline, a bounce or a different front panel, wires
  the same pieces beside this one.
- **One word for one thing: a mono or stereo entry of an input is a
  "source".** It is a `Source` to the mixer, its ring is written by a
  `SourceWriter`, and its id is a `SoundId` because a sound is what a
  source is once heard. For a day the same thing was a "group" on the
  control side, a "source" on the render side and a "sound" in the
  engine; the three words named one object. "Group" is gone from the
  code; this document uses "source" for it from here on.
- **The input's control code and audio code are separate files.** One
  `dsp/input.rs` held both ends of the ring: the `Input` handle, which
  allocates at open and knows a device's layout, and the `InputReader`,
  which runs on the audio thread and knows only a width. The same split
  the crate makes between `engine` and `renderer`, so it got the same
  shape: `src/input.rs` is the control side, `ChannelLayout`, `InputId`, the
  ring depth, `open` and the `Input` handle with its `feed`, a sibling of
  `engine`; `dsp/input.rs` keeps the `InputReader` and the lag rule, a
  generic piece like the playhead. `feed` runs on the capture thread but
  is control code by the crate's definition: it allocates nothing and
  touches rings that already exist, and the module that holds it is the
  one that knows what a device frame is.
- **`VoiceId` became `SoundId`, `VOICE_COUNT` became
  `MAX_SAMPLE_SOUNDS`.** The id names one instance of something in a
  mixer, a sample played once or an input while it is added, and "sound"
  was already the word the docs used for that. The count is the slot
  count of the sample mixer, beside `MAX_INPUT_SOUNDS` for the input mixer.
  The word "voice" is out of the code; it stays in the older entries of
  this document as what things were called when they were decided.
- **The order to remove in.** Both ends of each source's ring hold it, and
  it is freed when the second goes. Drop the `Input` handle first (it was
  `remove_input` until Handles) and the `InputFeed` second, and the frees
  happen on the caller's thread. Dropped
  the other way, the readers finish and the mixer frees the rings on the
  audio thread: the same last-reference hazard as an unloaded buffer,
  listed under Deferred ideas with the same fix. The rule is documented
  on `Input` and `Engine::remove_input` and the `monitor` example follows
  it.
- **The example is `monitor`.** Opens the default capture device at its
  own channel count and the default output, adds an input laid out from
  its arguments (`mono`, `stereo`, in device channel order), feeds it the
  first channels of the device, and hands its `Input` and the `Renderer`
  to their callbacks, waits two silent seconds, monitors every source for
  ten, unmonitors, removes the input, and reads both streams' errors.
  Heard working on 2026-11-09 and after each reshaping on 2026-11-10.

### Handles

Built 2026-11-10 on the branch after step 7, from one question: why does
a caller pass ids around when there are types for these things? The
answer took the engine apart.

- **A verb lives on the thing it acts on, and the thing carries what the
  verb needs.** `engine.stop(id)` became `sound.stop()`; `engine.monitor(
  id)` became `source.monitor()`. This is how Rust is written (`JoinHandle::
  join`, `Sender::send`, cpal's `Stream::play`, kira's sound handles) and
  not how the engine was: the engine was the only way in because it alone
  held the command ring's sending end, so every verb had to be an engine
  method taking an id. The rules the one path enforced (is this an input
  source, has it been removed) are enforced by the types instead, at
  compile time, and `NotAnInputSource` and `NoSuchInput` went with the
  path.
- **The sending end is shared.** `Control` wraps the ring's producer in
  a mutex behind an `Arc`, with the sound id counter and the input slot
  count beside it, and is cloned into every value that sends. The lock is
  control-side only; the renderer's end is unchanged and the renderer
  never sees a `Control`. Nothing on the audio or capture thread may hold
  one. It is the only code that builds a `Command`, so the vocabulary is
  private between it and the renderer. Named `Control` because it is the
  control side's end of the ring, and because no other handle in the
  crate carries a suffix: `Ledger`, `Sound`, `Input` are each named for
  what they are to the caller, the way `Sender` and `Producer` are, and
  `RendererHandle` was the one that did not fit. Rejected on the way:
  `Link` said too little, `Remote` means a network in code,
  `RendererHandle` for the suffix.
- **The engine dissolved into two sides that never name each other.** No
  method on it needed both the disk and the renderer, so there was
  nothing to coordinate. `Project::open(dir, ledger)` is the disk side,
  with `import` and `load`; `Renderer::new()` returns the sound side as a
  pair, the renderer for the audio thread and the handle that stays, with
  `play`, `stop`, `input` and `stop_all`. The only thing that crosses is
  a `LoadedSample`, carried by the caller. A project with no renderer is
  an import tool or a test; a renderer with no project is the `monitor`
  example, which opens none. A joint constructor was drawn and rejected:
  it would have been the engine again with a smaller body.
- **The ledger is the caller's, one per machine.** The budget is the
  machine's figure, not any project's: a project loads into it, and when
  effects arrive their scratch is allocated from it on the control side
  and moved to the renderer by command, as the rule under Memory already
  said. So `Ledger::new(budget)` is the caller's first line, and both
  sides take it. It became a handle like the others, `Arc` inside and
  `Clone`, so `Arc` left the public API; the struct behind every such
  handle is called `Shared`.
- **A loaded sample is data, owned by whoever holds it.** `LoadedSample`
  is an id and an `Arc<AudioBuffer>` and nothing else. Hold it and the
  sample is in RAM; drop it and the block repays the ledger once the
  last sound playing it lets go. It carries no handle, because a project
  cannot hand out a handle on a renderer it knows nothing about, and so
  its verbs are the renderer's, taking it as an argument: `play(&sample)`
  shares the audio with the sound it starts, `stop(&sample)` cuts every
  sound playing it. The rule for what has verbs is therefore one
  sentence: what the renderer hands out carries a handle and acts on its
  own; what the project hands out is data the renderer takes by
  reference. kira makes the same split (`StaticSoundData` is inert,
  `play` returns the handle). For an afternoon the sample carried a
  handle, had `play` on it and sent the eviction from `Drop`; it was the
  nicer call site and the wrong coupling.
- **The loaded flag went.** It mirrored the engine's map of what was in
  RAM; with the map gone it recorded "what the caller asked for last",
  which is a preference, and there is no panel yet to have one. Removed
  end to end, manifest field included; old manifests still open because
  serde ignores the field, so the format version did not move. What to
  load at open is the caller's to remember until a panel has an opinion.
  Each load is its own copy and there is no dedup cache; loading a sample
  twice costs its memory twice, documented rather than hidden.
- **Unloading is dropping; stopping is a verb.** `drop(sample)` lets its
  sounds play out, each letting go of the audio as it ends.
  `audio.stop(&sample)` first if they should not. The three stops read as
  one family: `sound.stop()` is one play, `audio.stop(&sample)` every
  play of one sample, `audio.stop_all()` everything. "Unload" on the
  renderer was tried and rejected: the renderer loads nothing, so it
  cannot unload. Taking the sample by reference also closes the address
  argument: the caller still owns the buffer when the eviction is pushed,
  so the address cannot name a later buffer.
- **An input comes as a pair, like a channel.** `input(layout, period)`
  returns the `Input` handle, which stays and whose `sources()` are
  `InputSource`s with `monitor` and `unmonitor`, and the `InputFeed`,
  which moves into the capture callback and has `feed` and no handle, so
  the capture loop takes no lock. Dropping the `Input` sends the removes
  and gives the slots back. A pair is what `mpsc::channel`, `rtrb` and
  `io::pipe` return for the same reason, two owners on two threads; the
  source is `InputSource`, not `Source`, because `dsp::Source` is the
  mixer's trait. The slot count moved into the handle, reserved by the
  add and released by the remove; `InputId` and the engine's map went.
- **A refused command burns its id.** Ids are minted by an atomic add
  before the send. The old engine, as the single sender, could promise
  no gaps; keeping that promise with many senders needed closures that
  built the command under the lock, and nothing reads the gaps. Removed
  as the wrong trade.
- **Drop is best effort, and the degrade is written down.** A drop cannot
  fail, so an `Input` dropped onto a full ring cannot send its removes:
  its readers stay until the feed drops and finish on their own, and
  their slots stay spoken for, because releasing slots the mixer still
  holds would let a later add be silently rejected. The command ring
  cannot be sized to make this impossible, since the control side is
  unbounded; it is sized so it does not happen by hand (64 commands per
  period), and the degrade is the answer for programs. The return ring,
  when it comes, can be sized to never fill, because the renderer's
  output is bounded by its slots.
- **The retire ring waits for the mixer rewrite.** The audio-thread free
  is now the normal path rather than the rare one: drop a `LoadedSample`
  mid-play and a playhead's end on the audio thread frees the block.
  The fix is unchanged (sources handed back over a ring and dropped on
  the control side, swept on every send) and it touches every place the
  mixer frees a slot, so it is a requirement on step 8's mixer rather
  than a patch on this one. The rule until then is on `Renderer`: drop
  what drives it before the samples it plays.
- **Build the engine this product needs, with clean seams.** The question
  "is the engine for one product or for many" came up over whether the
  renderer should come with a handle at all. The answer this project has
  been giving since step 3 is: one product, opinionated, with the seams
  (no device in the crate, generic dsp pieces, a storage trait) there for
  testing and the hardware, not for generality. So a choice is made when
  a feature on this roadmap needs it and not before: the renderer pair is
  justified by the offline bounce and step 8's renderer-wide verbs, a
  many-renderer API today would not be.
- **Rejected on the way.** `handle.load()` with the renderer reaching
  into the project, because the audio thread cannot do disk or
  allocation and would need a servicing loop to ask for it, and there is
  no "direct call" across the thread boundary in any design: the handle
  is the call made into a message. `project.play()` or `sample.play()`
  with the project knowing the renderer, for the coupling. A `static`
  ledger, because the explicit handle is what lets a test have a tiny
  budget. `stop_sample` as the name of the by-reference stop, unless
  `stop(&sample)` beside `sound.stop()` keeps tripping readers.

### Process

- **The public surface is kept minimal.** Convenience methods are added
  when a caller exists, not before.
- **Audio never leaves the engine.** There is no method that returns a
  sample's buffer. A copy in a caller's hands would be memory the ledger
  does not count, and nothing outside needs it: playback hears loaded
  audio through the renderer, a print will change audio inside the engine,
  export will stream from storage to a file, and a display asks
  questions with small answers. A waveform is the one that looks like it
  needs the samples and does not: given a frame range and a column count
  the engine returns one min and max per column, or the sample values
  when the range holds fewer frames than columns, so the answer is sized
  by the screen and never by the audio. That method arrives with the
  panel that calls it.
- **One product, opinionated, with clean seams.** Decided under Handles
  and the frame for every generic-versus-specific choice since: build
  what this sampler needs when a step on this roadmap needs it, and keep
  the seams that testing and the hardware require. A choice that pays off
  only for some other product is not made.
- **Nothing is settled by having been written.** The step 1 model was
  removed in step 3 when it stopped fitting. New steps are designed from
  the decision, then existing code is changed to fit, not the other way
  round.
- **The demos live in `examples/`, and so does the device code.** The
  crate is a library; there is no binary until there is a device to run
  one on. When there is, the device code and the sound examples move into
  that binary's crate, and the library's CI stops needing audio headers.

## Storage tiers on the device

```
SD card   kept projects; a copy of the active one, made on request
eMMC      the active project; every change lands here at once
RAM       whatever playback needs resident; the engine's loaded audio, under the ledger
```

The storage layer does not know about tiers. A copy to the card and the
loaded audio for playback are both layers beside or in front of it, not
changes to it.

## Deferred ideas

- Effects live on the render side. An effect runs live on a voice or the bus,
  heard through the renderer, and a print is the committed render of
  what was heard: the same effect code with the same parameters, driven
  offline, stored as a new generation. The step 2 `print`, an offline
  function over a buffer, stood in for this until step 5 removed it; the
  operation, commit a new generation, stays the centre of the engine.
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
- The free of a dropped sample's buffer still happens on the audio
  thread when a sound is playing it: the playhead is the last holder when
  the eviction or the end of the sound drops it. An `InputFeed` dropped
  before its `Input` has the same problem: its readers finish and the
  input mixer frees their rings. Fix, decided under Handles and deferred
  into the mixer rewrite: every place the mixer frees a slot hands the
  source to a sink, the renderer pushes it onto a ring back to the
  control side, and the `Control` sweeps that ring on every send;
  sized at the slot count plus the command capacity it can never fill.
  Not an audible problem today; a rule to keep before the device is the
  target.
- A level meter on a take: a peak or running power the take keeps as it
  quantises, in an atomic the recorder reads, so a panel can show input
  level while recording. Small; arrives with the panel.
- Level per mixer slot, for samples and inputs alike. The input mixer
  already holds up to `MAX_INPUT_SOUNDS` sounds, each its own ring; what is missing
  is a gain per slot, set by command, which is where a monitor level and
  a pad level both land. Not on the source: `add` takes no gain on
  purpose.
- A fast path in `stereo` for a stereo source heard as stored, which today
  goes through the same per-frame index remap as everything else. Costs
  an enum or a box to keep one return type. Wait for a measurement.
- Hardware monitoring on the target's codec, if it has it: zero-latency
  input to output with no software path. A device-side knob; the software
  path stays for everything that is not a straight wire.
- Taps and busses on the hard-coded path. A tap is an observer piece
  that copies the slice at its point into a ring for the control side,
  armed or not, dropping on full; the main out and a monitor pre- or
  post-effect are the same piece at different points. A bus is a second
  slice the renderer owns, played into and then added to the main out; an
  effect with a tail lives on a bus. Both are pieces in the renderer's
  order, not a graph.
- Finished and retire events from the render side to the engine, a ring
  the other way, so the engine learns a sound ended and can drop the last
  `Arc`, or an input source's ring, on the control side; see the entry above on
  where the free happens today.
- Stream errors (the device went away, an xrun the backend could not hide)
  are queued by the audio thread on a lock-free ring and read by the
  control side through `Stream::error`; the example prints them at the
  end. Where they go from there arrives with whatever reports events to a
  front panel.
- Streaming long recordings from eMMC instead of holding them resident,
  for audio that outgrows RAM. Needs a feeder thread and a prefetch window
  per voice, and has to stay clear of our own fsyncs.
- A budget read from the device's configuration rather than compiled in,
  once the CM4 variant is known and the free memory is measured.
- An `i16` build of the engine for low-powered hardware: a microcontroller
  class target without a float unit, or a module with far less RAM, where
  the integer pipeline is the sane one and half the resident bytes matter.
  Not a runtime switch and not a trait over sample types: with effects on
  the render side that is two pipelines behind one interface, so it would
  be a second engine configuration chosen at build time. The `Pcm` alias
  in `config.rs` is where the type is named, so the format stays in one
  place for whichever build. Decided 2026-11-09 alongside the move to
  `f32`, under Audio.
- A project whose stored format is not the engine's. Today a WAV that is
  not 48 kHz in the engine's sample type is refused on open as corrupt,
  and the manifest records only a file-format version, not the audio
  format. A project made on this engine and opened on a different build,
  the `i16` one above or one at another rate, would not open at all. The
  shape decided for it: a conversion between the stored format and the
  engine's on load, and saves always in the engine's format. Each sample
  carries its own format, the WAV header already does, so a project can
  hold samples in more than one format without a per-project setting,
  and it drifts toward the engine's format as samples are re-saved. Not
  needed while there is one engine format; noted 2026-11-09 with the move
  to `f32`.
