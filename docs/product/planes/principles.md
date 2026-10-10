# Product model — supporting planes — Principles and freedom

[Product model — supporting planes](../planes.md) · [Spice product model](../README.md)
## Cross-cutting principles

- **One authority per fact.** Every fact has exactly one owner. The authenticated
   stream binds snapshots and resume positions to the resident epoch and ordered
   sequence identity; projections rebuild from that same authority.
- **Derive, never duplicate.** Observation folds the authority's own history
   rather than writing a parallel event log. Projections are rebuildable by
   construction; authority is not.
- **Durable before delivered.** Intent is persisted before any attempt to
   deliver it. Delivery is retryable; the intent is never recreated.
- **Semantic retirement.** Nothing is retired by being read. Acceptance and
   refusal are equally first-class and equally terminal.
- **One visible path.** One tested current command and data vocabulary is visible.
   Each durable authority admits only its exact current shape. A retired shape
   refuses before mutation and names its owning release; executable behavior has
   no historical branch.
- **Fail loudly at authoritative boundaries.** A component reporting authority
   that read nothing must not produce the same output as one that read
   everything. Silence is confined to non-authoritative decoration and
   conservative pre-publication filtering; neither can alter or fabricate a
   fact.
- **Prefer the good property; never block on it.** Cross-review, fast-forward at
   launch, rewriting, adjudication — each is attempted, none is required. The
   work always has a way forward.
- **Refusals lead with the repair.** The reader who has just been stopped has no
   second screen. The message is the whole interface.
- **Movement means change.** Any motion corresponds to something that actually
   changed. Re-render is not change.
- **The agent is a user.** It has a UX — briefing, meter, reminders, refusals —
    and that UX has a required *tone*: capability, not restriction.

---

## What is fixed and what is free

Free: language, framework, and storage engines; visual styling, colour,
concrete sizing, and iconography; transport framing and serialization; track
counts, span ladders, and tween timings wherever no ordering, overflow,
placement, replay, or scroll-ownership rule constrains them; the handle
alphabet and key derivation.

Fixed: the principles above and every entry in the
[invariant register](../invariants.md).

**The test for "fixed":** if changing it would make an operator distrust the
system, or leave an agent unable to recover on its own, it is fixed. Everything
else is spelling.
