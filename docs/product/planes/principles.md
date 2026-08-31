# Product model — supporting planes — Principles and freedom

[Product model — supporting planes](../planes.md) · [Spice product model](../README.md)
## Cross-cutting principles

The rules that generate everything else. If you keep one page, keep this.

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
   Each durable authority gives every admitted historical shape one unambiguous
   direct or sequential route into the current schema, and executable behavior
   uses only that current schema.
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

| Layer | Verdict |
| --- | --- |
| Language, framework, storage engines | **free** |
| Visual styling, colour, concrete sizing, iconography | **free** |
| Transport framing, serialization | **free** |
| Track counts, span ladders, tween timings | **free** where no fixed ordering, overflow, placement, replay, or scroll-ownership rule constrains them |
| Phase *names*, handle alphabet, key derivation | **free** |
| One authority per fact; epoch/sequence-bound snapshot and resume freshness | **fixed** |
| Durable-before-delivered; semantic retirement | **fixed** |
| Claim exclusivity, accepted lapse, and exact explicit steal with ownership refusal | **fixed** |
| The three agent-lane integration git boundaries; agents never push or pull | **fixed** |
| Landing atomicity and footprint containment | **fixed** |
| Cross-review preferred, never required | **fixed** |
| Lane is not agent | **fixed** |
| Rendering never launches | **fixed** |
| Movement means change | **fixed** |
| Refusals lead with the repair | **fixed** |
| Authoritative degradation is loud | **fixed** |
| Observation initializes nothing | **fixed** |

**The test for "fixed":** if changing it would make an operator distrust the
system, or leave an agent unable to recover on its own, it is fixed. Everything
else is spelling.
