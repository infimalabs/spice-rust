# Spice product model

An operations console for a fleet of coding agents. One operator supervises many
agents, each inhabiting a real Git worktree, by reading their transcripts and
writing durable steering through one journal authority. The
[authority map](model/domain.md#the-authority-map) names who owns each fact.

These documents describe what Spice is and the invariants its live code keeps.
The code is authoritative; a disagreement is a documentation defect.

## Documents

| File | What it answers |
| --- | --- |
| [value.md](value.md) | Why Spice is worth using: durability, attribution, and calm. |
| [loops.md](loops.md) | The operating loops, their state machines, and the jitter contract. |
| [rooms.md](rooms.md) | The room loop: publishing, delivery, attention, Design preparation, and non-capabilities. |
| [invariants.md](invariants.md) | The register of named invariants. |
| [model.md](model.md) | Authority, state-machine homes, decisions, boundaries, failure semantics, and vocabulary. |
| [planes.md](planes.md) | Session forensics, metrics and hooks, gate evidence, distribution, and principles. |
| [interfaces.md](interfaces.md) | Task documents, the JSON outcome, bounded reads, kept jobs, extensions, diagnostics, and media. |
| [checks.md](checks.md) | The checks that can refuse a commit. |
| [implementation.md](implementation.md) | Concrete appliance choices and the code that owns each, including [Mosaic card lineage](mosaic.md). |

Start with [value](value.md) and [loops](loops.md), then use the
[register](invariants.md) as the contract.

## By question

- **What can change freely?** [What is fixed and what is free](planes/principles.md#what-is-fixed-and-what-is-free).
- **What may fail silently?** [Failure and degradation](model/failures.md#failure-and-degradation)
  and the [failure-behavior table](model/failures.md#the-failure-behavior-table).
- **What does this word mean?** [Ontology](model/failures.md#ontology).
- **Why does the interface feel like this?** [The jitter contract](loops.md#the-jitter-contract).
- **What does a room do, and what does it never do?** [The room loop](rooms.md#l-room--conversation-at-a-task),
  its [limits](interfaces.md#limits-and-their-repairs), and its [non-capabilities](rooms.md#deliberate-non-capabilities).
- **How does an agent recover its own past?** [Session forensics](planes/session.md#session-forensics).
- **What cannot be taken back?** [Reversal is a claim about identity](planes/distribution.md#reversal-is-a-claim-about-identity).

## Links

The markdown-links gate checks that relative link targets name tracked files.
Nothing checks anchors or Mermaid fences, so a change that removes or renames a
heading or anchor repairs every inbound link in the same change.
