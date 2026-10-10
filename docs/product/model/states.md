# Product model — core — States

[Product model — core](../model.md) · [Spice product model](../README.md)

## State-machine homes

- [Tasks and claims](../loops.md#l-task--the-unit-of-work): stored workflow and claim disposition are separate from eligibility and landing.
- [Steering](../loops.md#l-steer--intent-made-durable): disposition, presentation, and delivery are separate.
- [Process and resident lifecycle](../loops.md#l-agent--the-work-availability-loop).
- [Submission](../loops.md#l-draft--the-watchband).
- [Message layout](../loops.md#l-message--the-mosaic) and [image outcomes](../invariants/lifecycle.md#surface-image-failure-is-distinct-from-still-loading).
- [Maxim matching and adjudication](../invariants/session.md#session-a-maxim-bag-is-one-opinion-with-several-triggers).
- [Projection families](../invariants/identity.md#identity-a-projection-family-must-declare-its-source).

## Closed loop: Agent process

A launch does not inherit ambient thread identity. The lane binds to the session
proven by that launch, never to a thread inherited from the environment.

The bootstrap signal invokes Spice rather than using operator prose as its ask.
The resident appends the fresh-seat briefing or resumed-seat notice and supplies
direction through the steering surfaces.

Lane and briefing reads expose a current lifecycle hold's deadline and journaled
evidence from its latest hold/resume transition. They do not read the refused
prompt or provider reasoning. [Vendor-refusal holds](../invariants/lifecycle.md#lifecycle-a-vendor-refusal-does-not-replay-identical-input)
remain blocked on unchanged input even after their deadline.

## Lane and team

The [lane loop](../loops.md#l-lane--the-place) owns renewal, team merge/split,
member ordering, and independent placement. Renewal and occupant succession do
not define another claim disposition; the [claim loop](../loops.md#l-task--the-unit-of-work)
owns carry and rebound.
