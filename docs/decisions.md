# Decisions to date

A one-time look back over how the engine reached its shape, written
2026-11-19 at `main` 7c8c367 (PR #93), when steps 1–7 and mixing were done
and planning turned to timing and sequencing. It is not kept up to date:
the roadmap holds what stands now, and git holds the detail.

## What the shape came from

The project began as a model in memory with offline effects, and was
rebuilt three times as each part met a real constraint:

1. **Data model first.** A `Sample` with a parent pointer, an `Origin` and
   labels, and a `Project` holding everything in RAM. Origin, labels and
   most accessors were cut before any caller needed them; the in-memory
   project was replaced by the disk.
2. **First print.** A `reverse` effect and a thin engine that applied it
   offline. It proved the idea, then was removed: an offline
   `&AudioBuffer -> AudioBuffer` was the one allocation the RAM budget
   could not see, and effects belong on the render side.
3. **Persistence.** A `Storage` trait with a file backend, then the trait
   removed (one implementation; tests use a temp directory) and project
   and storage merged into one type.
4. **Playback.** A resident set, a renderer, a `Playback` struct, a device
   inside the crate and an `Engine` as the only entry. Each of those was
   later dissolved: the device left the crate, `Playback` and then
   `Engine` were removed, and the renderer was rebuilt twice.
5. **Memory.** A ledger of reservations beside ordinary `Vec`s, then a
   ledger that was also the allocator, then `AudioMemory`: the buffer is
   its own receipt.
6. **Recording, monitoring, mixing** on the engine as it stands, built on
   the transfer channels and a duplex `render`.

## Themes

### The disk is the project

- An in-memory project dumped to disk was replaced by "the project is the
  disk": no cache, every write fsynced, no save step. It removed a whole
  class of "is it saved?" questions and maps onto the hardware's eMMC.
- A `Storage` trait came and went; a SQLite medium is the only reason left
  to bring one back.
- A persisted `loaded` flag was built and removed: it stored a preference
  before any panel had one.
- WAV import (decode, convert with rubato, exact import of in-format files)
  was built and later removed; audio enters a project by being written.
- `remove_sample` was built and cut for want of a caller.
- Lineage was built, then removed while nothing made a child. It returns
  with prints.

**Lesson:** the project kept shrinking to what a caller needed; every
feature built ahead of its caller was later removed.

### Sample format

- First `i16` storage with `f32` processing, chosen to fit the most audio
  in a CM4's RAM. Reversed to `f32` everywhere: the CM4 has hardware
  float, `i16` loses 24-bit sources at every generation, and the render
  path should read rather than convert. A transitional `_v2` set of types
  carried the switch, then was renamed into place.

### Memory

- **Resident set** with a budget, then a **ledger** of reservations held
  beside ordinary buffers, then a ledger that allocated, then
  **`AudioMemory`**, which makes every buffer and is repaid on drop.
- A per-sample length bound was rejected: the worst case depended on the
  operation.
- Counting the input rings against the budget was tried and reverted as
  one mechanism too many; fixed overhead is sized once instead.
- A locked arena was considered and left for later; the OS (cgroup, no
  swap) backs the figure.

**Lesson:** the receipt and the memory had to be one object before
release could not be forgotten or doubled.

### The engine type

- An `Engine` over both project and audio was the single entry for a long
  time. It was dissolved: no method needed both sides, and a joint
  constructor would have been the same thing again. `create` now returns
  `(Control, Renderer)`.
- The device lived in the crate, then moved to the examples. A trait over
  cpal was rejected (one implementation; tests hold the renderer).
- `Control` minting new renderers after a device change was built and
  dropped: `Control` holds nothing a new renderer needs, so a device
  change calls `create` again.

### How the two sides talk

This went through the most shapes:

1. **One command ring and one event ring** with umbrella `Command` and
   `Event` enums. Every feature shared one reply type.
2. **Request channels** (`request`): a request and its reply, with cancel
   and status. A cancel could outlive its request, and every status only
   said "still held".
3. **A duplex channel** sending a handle any number of events: its return
   ring could not be sized.
4. **A collector channel** for freeing whatever came back: removed once
   everything rode home on its own.
5. **Reply rings per slot**, made at `create`: built and set aside on
   branch `request-v2-slots`.
6. **`transfer`**, as it stands: each value goes over with its own
   one-deep return ring and comes home whole, one channel per feature,
   with running state (stop, monitoring) as atomics beside it rather than
   messages. Named for DMA transfers in embedded Rust.

Smaller turns along the way:
- Stopping by id (`control.stop(&audio)`, `stop_all`) gave way to stopping
  by handle; a list of voices in `Control` would have trailed the
  renderer.
- Ended voices, inputs and recordings reported why (`Finished`, `Cut`,
  `Stopped`, `RecordingEnd`); nothing acted on it, so the reasons went.
- A shared `ControlError` was split into one error per handle.
- A `Transfers` struct grouping the channel ends was removed as a hop that
  did nothing.

**Lesson:** a reply that is the value itself means the renderer cannot
forget to send something back, and each return ring is sized by what is
in flight. Most earlier shapes failed on one of those two.

### The render side

- **Mixer shapes:** a voice table with a command ring; a `Controller`
  renamed `Renderer`; a heapless deque of voices; a `Source` trait, boxed
  `Processor` chains and a generic `Mixer<S, N>`, each built and dropped.
  It settled on a plain render loop that iterates voices and inputs
  itself, in blocks of 64 frames, into one shared scratch.
- **Voice slots:** numbered places searched for the lowest, then a `Vec`
  kept oldest first, so stealing the oldest is the first entry.
- **Code layout:** split by thread (`control/`, `protocol/`, `realtime/`),
  then `dsp/` and `renderer/`, then one file per feature with a
  `mod realtime`. A change to a feature touches its data, handle and
  audio-thread part together.

**Lesson:** abstraction on the audio side (traits, chains, generic
mixers) kept being replaced by a loop that says what happens.

### Input

- **A capture ring per device** filled by a `Feeder` on the capture
  thread, with `create_with_capture` and a `NoCapture` error. Removed for
  a duplex `render(capture, out)`: one callback is one clock, as JUCE,
  PortAudio and JACK hand audio over. The ring put a cushion, clock drift
  and a feeder that could go away inside the engine.
- cpal was pinned to master for duplex streams.
- Monitoring as a second request kind was dropped for a flag in the
  input's state.

### Recording

- First a `Recorder` minted by the engine holding a reservation, with
  `Take` and `save_take`. Rebuilt on transfers as a `Recording` handle per
  take, into audio the caller allocates.
- One recorder per tap (`TapInUse`) and a refusal by the renderer
  (`Refused`) came before; now one recorder, refused at once when busy.
- `try_wait` became `try_take`; a cached `state()` was removed because it
  claimed `Recording` when it only meant no reply yet.
- An input tap was removed, then returned as `Tap::Input(&input)`; device
  channels as a tap were weighed and rejected because a tap names a point
  in the signal path.

### Examples

- `first_print`, `first_sound`, `first_take`, then an `engine` example,
  then per-area examples (`voices`, `record`, `inputs`) showing every
  feature. Split into one small example per feature: the per-area ones
  were long, and a refusal printed on a device checks nothing a test
  does not.
- A shared device shell across examples was reverted: examples are plain
  demonstration code.

## How the work went

- **Building ahead of a caller did not last.** Lineage, `remove_sample`,
  the `loaded` flag, end reasons, status and the offline print were all
  removed again.
- **Names moved with understanding:** `Sample`/`SampleMemory` became
  `Audio`/`AudioMemory` once RAM and disk were told apart; `Sound` became
  `Voice`; `request` became `transfer`; `Controller` became `Renderer`.
- **The roadmap carried too much history** and grew to about 600 lines; it
  was cut to the standing plan on 2026-11-19, and this document took the
  history.
- **Target order changed** on 2026-11-17 from the CM4 first to desktop
  first, making device changes, other rates and separate clocks normal
  cases rather than edge cases.

## Where it stood at the time of writing

Built: the project on disk, `AudioMemory`, a duplex renderer with a voice
pool, inputs with monitoring, and recording from the main out or an input.
Next: timestamped requests, then a clock shared by both sides. Open: what
lives on the audio thread, and where the sequencer runs.
