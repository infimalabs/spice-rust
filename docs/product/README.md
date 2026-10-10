# Spice product model

An operations console for a fleet of coding agents. One operator supervises many
agents, each inhabiting a real git worktree, by reading their transcripts and
writing durable steering through one journal authority. Transcripts are evidence
of what agents produced; the journal owns durable facts and current heads;
filesystem and interface surfaces are disposable projections. Coordination
consumers read the journal authority or its admitted projections and never
reconstruct those facts or current heads from transcripts or repository-local
steering artifacts.

This is the product model: what the product *is*, and what anything calling
itself spice has to do. Entities, loops, policies, boundaries, failure
semantics, and the properties that make it worth using are stated independently
of any implementation.

## Documents

These files are the canonical product contract for the Rust appliance. Keep the
Markdown tree together, with links relative to this directory and no dependency
on its name or depth. Concrete mechanisms name their Rust authorities where the
implementation choice is part of the contract.

| File | What it answers |
| --- | --- |
| [invariants.md](invariants.md) | The product contract: a semantic register of named invariants. |
| [value.md](value.md) | Why Spice is worth using: durability, attribution, calm, and how they compose. |
| [loops.md](loops.md) | The transformative loops, jitter contract, and interface-surface contract. |
| [model.md](model.md) | Domain model, state machines, policies, loops, boundaries, failure semantics, journeys, and ontology. |
| [planes.md](planes.md) | Session forensics, metrics, hooks, distribution, cross-cutting principles, and what is fixed versus free. |
| [interfaces.md](interfaces.md) | Task documents, wire seam, extensions, diagnostics, and media. |
| [checks.md](checks.md) | Commit checks: gates, pressure, instruments, waivers, and isolation. |
| [rust.md](rust.md) | The Rust implementation profile: concrete appliance selections and the code and executable evidence that own them. |

Start with [why Spice is valuable](value.md) and [operating
loops](loops.md). Use the [invariant register](invariants.md) as the product
contract, then read the [core model](model.md), [supporting
planes](planes.md), and [interfaces](interfaces.md) for the whole system.
[Surface invariants](invariants/lifecycle.md#surface) guide interface
implementation; the [checks](checks.md) describe what may be committed. This
repository's [Rust implementation profile](rust.md) fixes the concrete
appliance choices left open by the portable model and names their implementation
owners.

The register states what is true of the product. Code and tests provide evidence
that the contract is implemented; they do not create a second product model.

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

**One current product, one authority for each fact.** Product semantics live in
this corpus. Concrete Rust types, commands, storage, transport, and release
boundaries are stated here when they affect those semantics. The [Rust
implementation profile](rust.md) selects the concrete appliance form where the
portable model leaves a choice and binds that selection to code and executable
evidence. A disagreement is a defect, never an alternate product model.

## By question

- **What can change freely?** See [What is fixed and what is
  free](planes/principles.md#what-is-fixed-and-what-is-free).
- **What is *allowed* to fail silently?** See [Failure and degradation](model/failures.md#failure-and-degradation)
  and the [failure-behavior table](model/failures.md#the-failure-behavior-table).
  Silence is confined to non-authoritative decoration and conservative
  pre-publication filtering; neither may alter a fact or widen into fallback
  behavior.
- **How is this tested?** Executable suites are the evidence; the [Rust
  implementation profile](rust.md) names the code and coverage that own each
  concrete selection.
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
semantic anchor resolves, every Mermaid fence renders, and every invariant has a
unique name and anchor. A check that cannot inspect its input must refuse rather
than report a clean result.
