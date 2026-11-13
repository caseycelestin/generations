# Rules

## Comments

Every code comment, doc comment included, is concise and says only what the
code does not.

- Say why, or state a rule the code cannot show: which thread, what must never
  happen, how something is sized, a unit.
- Do not restate a name, a type or a signature. A doc that only repeats the
  item's name is deleted, not reworded.
- One or two sentences per item; a module doc is a short paragraph.
- Say a thing once, where it applies, not again in every place that touches it.

## Roadmap

`docs/roadmap.md` is the record of decisions that stand today. It changes in
the same step as the decision it records.

- One bullet per standing decision: what, why, and what was tried or rejected
  instead, in a sentence, not the full story. The history of how a shape was
  reached is in git.
- A reversed decision is deleted, not annotated.
- What is decided but not built says so.
- A deferred idea is removed once it lands.
- The architecture is documented in the crate, not repeated here.
