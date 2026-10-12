# Product model — core — Domain

[Product model — core](../model.md) · [Spice product model](../README.md)

## The authority map

| Authority | Owns |
| --- | --- |
| Journal | tasks, claims, steering dispositions, inbox heads, topology, command receipts |
| Transcript tape | vendor bytes and evidence of what the occupant produced |
| Admitted target registry | worktree identity |
| Lifecycle authority | launch and succession decisions |
| Detached supervisor | the phase of its process turn |

The [authority register](../invariants/identity.md#identity-every-fact-has-exactly-one-authority)
owns the rule for consumers; [session asks](../invariants/session.md#session-asks-are-sourced-from-the-steering-plane)
distinguish operator direction from transcript evidence. Durable authority and
[rebuildable projections](../invariants/identity.md#identity-durable-facts-and-heads-remain-distinct-from-rebuildable-index-rows)
are separate contracts, even when they share a store.

Writable journal admission has one current crossing. A fact kind whose producer
has retired does not remain in the executable wire vocabulary: admission
crosses its prior rows once into inert `journal::retained` evidence that names
the last owning release and preserves the retired payload, or refuses the
authority while naming that release. A retired optional field crosses by
rewriting each carrying fact and command receipt in place without it: the
row keeps its position, identity, and every other field, is re-encoded through
the current DTO, and only the retired value is lost. No digest covers single
rows, so the rewrite breaks no integrity witness. Current readers never carry
release-specific decode arms.

## Independent task graphs

One board exposes separate rendered views:

- **Origin:** why work exists; task ancestry is not a scheduling edge.
- **Dependencies:** what must settle before other work is eligible.
- **Review:** which actor reviewed which author's phase, with the verdict.
- **Workflow:** how each task advances through its own flow.
- **Claims:** possession and its history.

Collapsing origin and dependency edges loses the difference between provenance
and a plan. These views are emitted as diagram source, not merely queried as
rows; [graph reads](../interfaces.md#bounded-reads) describe their export boundary.

Entity definitions live in the [vocabulary](failures.md#ontology), task identity
in the [identity register](../invariants/identity.md#identity-a-task-s-born-stamp-is-its-only-stored-identity),
and places and occupants in the [lane loop](../loops.md#l-lane--the-place).
