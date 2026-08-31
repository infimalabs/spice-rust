# Spice product model

An operations console for a fleet of coding agents. One operator supervises many
agents, each inhabiting a real git worktree, by reading their transcripts and
writing durable steering into their repositories. Supervision, coordination,
counsel, and repository checks are derived from those two surfaces rather than
maintained separately.

This is the product model: what the product *is*, and what anything calling
itself spice has to do. Entities, loops, policies, boundaries, failure
semantics, and the properties that make it worth using are stated independently
of any implementation.

## Documents

The implementation-independent files form a self-contained corpus: keep their
Markdown tree together, with links relative to this directory and no dependency
on its name or depth. The repository-local [Rust profile](rust.md) accompanies
that corpus and deliberately cites implementation authorities outside it.

| File | What it answers |
| --- | --- |
| [invariants.md](invariants.md) | The product contract: a semantic register of named invariants. |
| [cases.md](cases.md) | The case battery. Every registered invariant is covered by a case or by a declared audit where execution cannot demonstrate absence. |
| [value.md](value.md) | Why this is valuable: the workflow guarantees, interface components, and how they compose. |
| [loops.md](loops.md) | The transformative loops, jitter contract, and interface-surface contract. |
| [model.md](model.md) | Domain model, state machines, policies, loops, boundaries, failure semantics, journeys, and ontology. |
| [planes.md](planes.md) | Session forensics, metrics, hooks, distribution, cross-cutting principles, and what is fixed versus free. |
| [interfaces.md](interfaces.md) | Task documents, wire seam, extensions, diagnostics, and media. |
| [stories.md](stories.md) | Who each guarantee serves and the failure modes each one answers. |
| [checks.md](checks.md) | Commit checks: gates, pressure, instruments, waivers, and isolation. |
| [rust.md](rust.md) | What the Rust appliance implements differently, omits, or deliberately strengthens. |

Start with [value models](value.md) and [operating
loops](loops.md) for the model. Use the [invariant
register](invariants.md) as the product contract, then read the [core
model](model.md), [supporting planes](planes.md), and
[interfaces](interfaces.md) for the whole system. The [stories
and failures](stories.md) supply the reasoning; [surface
invariants](invariants/lifecycle.md#surface) guide interface implementation; the
[checks](checks.md) describe what may be committed. This repository's
[Rust implementation profile](rust.md) is the applicability and deviation
register for the imported model.

The register states what is true of the product. Implementation status — what a
given codebase has built, deviated from, or not reached yet — belongs beside it
in its own file, so the contract stays readable as a contract.

## Conventions

| Label | Meaning |
| --- | --- |
| **Closed loop** | state returns to a fixed point and the system quiesces |
| **One-way flow** | pipeline or decision fan |
| **Invariant** | required by the product contract |
| **Policy** | tunable value together with the reasoning that chose it |
| **Incidental** | explicitly left open by the product model |

**Cite the register, not a document's position.** Every cross-document
reference uses the invariant's natural-language name and stable semantic
anchor. Adding, removing, or regrouping neighboring material never relabels
anything. Local invariant and policy statements are deliberately unnumbered.

**Implementation status stays separate.** The product model remains independent
of one implementation. The Rust profile records implementation status and may
name concrete code, tests, commands, and deployment boundaries; no other model
document does. The imported implementation-specific surface-mechanics tree was
removed because it described another client and transport rather than this
appliance.

## By question

- **What can change freely?** See [What is fixed and what is
  free](planes/principles.md#what-is-fixed-and-what-is-free).
- **What is *allowed* to fail silently?** See [Every degradation is loud](invariants/distribution.md#conduct-every-degradation-is-loud) and the [degradation
  table](model/failures.md#the-degradation-table). Exactly one exemption exists:
  a decoration laid over another operation.
- **Which properties fail most quietly when violated?** See [The properties most
  worth guarding](invariants/distribution.md#the-properties-most-worth-guarding).
- **How is this tested?** See [The case battery](cases.md#the-battery).
- **What does this word mean?** See [The ontology and its
  anti-vocabulary](model/failures.md#ontology).
- **Why does the interface feel like this?** See [The jitter
  contract](loops.md#the-jitter-contract).
- **How does an agent recover its own past?** See [Session
  forensics](planes/session.md#session-forensics).
- **What cannot be taken back?** See [Reversal is a claim about
  identity](planes/distribution.md#reversal-is-a-claim-about-identity).

## Model integrity

The product model is internally sound only while every relative link and
semantic anchor resolves, every Mermaid fence renders, every invariant has a
unique name and anchor, every case has a unique situation, and every registered
invariant is covered by a case or declared audit. A check that cannot inspect
its input must refuse rather than report a clean result.
