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
| 6 | Recording: audio input as a new root | |
| 7 | Sequencing and mixing: the real-time side | |
| 8 | Arrangement and more effects | |

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
  is the engine's alone, crate-private and paid for by a reservation
  taken from the record first. They are separate because structure is
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
  nothing else is in a position to hold, and `loaded` is the first. More
  arrive as their steps come. `sample(id)` is where they are read, and
  each one that can change gets its own setter with its own rule, rather
  than one general update: the id and parent can never change, and a rule
  belongs next to the one field it governs.
- **The engine is the only way in.** `Engine::project()` lends the
  project read-only; every change is an engine method. Some changes are
  only half a project write (`load` sets a flag and moves audio into RAM),
  and a caller reaching a role directly would leave the two disagreeing.
  So the roles the engine owns (`project`'s mutation, `render`, `device`,
  `storage`) are crate-private, and what is public is the engine, the
  project's read side, the value types in the engine's signatures, the
  errors, the effects and config. One path to each action, and the rules
  hold on it. A reader who wants a role's internals opens the module.
- **The engine's methods stay flat for now.** Most touch more than one
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
- **One fixed format**: 48 kHz, 16-bit, set in `config.rs`. Import converts
  to it, output converts from it, nothing in between checks. Channel count
  is kept per buffer so mono samples do not pay for stereo. How import
  converts is under Import below.
- **Storage is i16, DSP is f32.** Every effect converts in and out through
  `AudioBuffer::to_f32` / `from_f32`, which are the only code that knows the
  PCM width. The downward conversion rounds and applies TPDF dither. Effects
  that only reorder frames (reverse) are exempt: they copy PCM directly, so
  they are exact and reversing twice really does give back the root's bytes.
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

- **The engine takes the file.** `Engine::import(path)` reads, converts and
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
  control side.** Pressing a pad is a front-panel action, and the engine
  is what the front panel calls; it owns the audio in RAM and the sending
  end of a command ring. The render side, the `render` module, owns the
  receiving end and the playing voices and turns commands into frames.
  The split is by thread, not by feature: one side decides and may be
  slow, the other moves frames on a deadline and must never wait on the
  first. A `playback` module that held both halves was removed because
  the question "which thread am I on" should be answered by the module
  path. The pattern has several names: a command pattern over a lock-free
  single-producer single-consumer queue, an active object, control plane
  and data plane. The engine never reaches into the render side; every
  change to what is heard is a command, applied at the next period.
- **The render side is named for what it does, not where it runs.**
  `render` turns commands into frames and gives the same frames live from
  a device callback or offline in a loop, so "realtime" was rejected as a
  name (the code must run offline) and "process" was rejected (it collides
  with the OS process this roadmap talks about). Recording in step 6 gets
  a sibling `capture` module, a ring the other way; the pair is the data
  plane, named by direction, and the rules (no allocation, no lock, no
  disk, no error path, no clock) apply to both.
- **The engine mints the renderer and hands it out as its output.** The
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
- **Residency is explicit and persisted.** A sample is in RAM because
  someone asked, not because it was played. `loaded` on the sample is the
  recorded intent; the engine loads every flagged sample on open, so the
  device comes back ready after a power-down. `Engine::load` and
  `Engine::unload` change it. Playing a sample that is not loaded is an
  error, not a silent disk read, so a press never pays for a load and
  latency never depends on history. What to load is the caller's policy;
  the engine never loads on its own. Load-on-play is two lines in a
  caller that wants it.
- **Unload stops every voice playing the sample.** Before this, a voice
  still playing an unloaded sample kept its `Arc` and played to the end,
  so sound came from memory the engine said it no longer held. Unloaded
  means gone from RAM. The fix is a third command keyed the third way:
  `Stop` is by voice, `StopAll` by nothing, `Evict` by buffer address.
  The engine sends the address of the buffer it is removing and the
  renderer drops every voice whose `Arc` points at it, one pass over at
  most `VOICE_COUNT` voices; the render side still never learns a sample
  id. The address is safe to use because the engine sends before it lets
  go, so any later buffer at the same address is started by a `Play`
  behind the eviction on the ring. `unload` sends first and fails with
  `CommandsFull` if the ring is full, leaving the sample loaded, like
  `play` and `stop`. It takes effect on the renderer's next period and
  cuts hard, as `stop` does.
- **Adding a sample and loading it stay separate.** A print the user made
  must land on disk even when it does not fit in RAM. Combining the two
  would have to either refuse the add or store the sample unloaded anyway,
  which makes the flag a request rather than a fact. The cost is a disk
  read of bytes the engine was just holding, a few milliseconds per print.
- **The budget is enforced where RAM is managed.** The caller passes the
  budget to `Engine::open`; the project never learns about RAM. In step 4
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
- **The render side is two types: `Voice` and `Renderer`.** A voice is
  how one sample is heard: today the whole buffer from first frame to
  last at its own speed. Start and end points, looping, gain and fades
  are fields and lines on the voice when they come. The renderer is
  real-time safe, not real-time only: it owns the playing voices and the
  consuming end of the command ring, and each call applies the commands
  that arrived, zeroes the slice it is given, lets every voice add itself,
  drops the voices that finished, and clips the sum once. A device drives
  it live from its callback; a bounce drives it offline in a loop. There
  is no separate mixer: a list of voices with a sum is a method, not a
  concept. The pattern is JUCE's `Synthesiser` and Kira's `Renderer`; the
  name is Kira's, and "controller" was rejected because the engine gives
  the orders and this side carries them out. The loaded audio and the
  voices do not overlap: one says which samples are in RAM, keyed by
  sample; the other says what is sounding now and where in it, keyed by
  voice. They share only an `Arc` to the same bytes.
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
- **Playing voices are a fixed-capacity FIFO.** `heapless::Deque<Voice,
  VOICE_COUNT>`, stored inline in the renderer: oldest first, so stealing
  is pop the front and push the back, and the type cannot allocate. An
  array of `Option<Voice>` tracked free and in-use slots by scanning for a
  sentinel and relied on ids being minted in order to know the oldest;
  the deque says FIFO in the type and needs neither.
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
- **Recording will be a second stream.** `cpal` opens input and output as
  separate streams with separate callbacks, so recording gets its own
  input type in the caller's device code, a `capture` module beside
  `render` with its own ring back to the engine, and the render side
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
  stored; it grows, so it cannot be an immutable shared buffer; and it is
  not playable. Fitting it in with the loaded audio would cost that map an
  optional key, interior mutability and an unplayable flag. So the engine's
  recording state owns it on the control side, holds one reservation and
  asks to grow it as frames arrive, and the take ends when growth is
  refused, the way a tape runs out. When it ends it goes through the same
  crate-private add as import and print, and loading it afterwards is a
  separate read from disk, as already decided for prints.
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
- **Nothing is settled by having been written.** The step 1 model was
  removed in step 3 when it stopped fitting. New steps are designed from
  the decision, then existing code is changed to fit, not the other way
  round.
- **The demo lives in `examples/`, and so does the device code.** The
  crate is a library; there is no binary until there is a device to run
  one on. When there is, the device code and the sound example move into
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
- The free of an unloaded buffer still happens on the audio thread, since
  the renderer's voice is the last holder when it drops. Fix: unload keeps
  the removed `Arc`, and the engine keeps a retiring list it sweeps on
  the control side for entries whose count has dropped to one. Not an
  audible problem today; a rule to keep before the device is the target.
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
