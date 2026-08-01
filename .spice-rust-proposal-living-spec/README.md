# spice — the genesis package

This directory is the complete product boundary for replacing Python spice
with the Rust appliance. The references fix what exists and why;
[GENESIS.md](GENESIS.md) is the authored document that constructs and proves
it.

## Reading order

Read these twelve reference files in order. `GENESIS.md` is the construction
program described below, not a thirteenth reference — and the whole corpus is
a living record: it binds until it is deliberately amended, and it will be.

| Order | File | Read it for |
| ---: | --- | --- |
| 1 | [FRAME.md](FRAME.md) | The binding 85-joint decision record and the rule that wins every conflict. |
| 2 | [SCAFFOLD.md](SCAFFOLD.md) | The exact population map for every final file, with no open choices. |
| 3 | [DOCTRINE.md](DOCTRINE.md) | Kernel invariants, settled decisions, the Rust rationale, and cutover law. |
| 4 | [NAMES.md](NAMES.md) | One-word naming law, canonical vocabulary, the crate web, and the sole retired-term table. |
| 5 | [WIRE.md](WIRE.md) | Current-only DTOs, commands, witnesses, head authority, facts, folds, and decoding. |
| 6 | [TREE.md](TREE.md) | The 21-crate workspace, dependency lattice, postures, rim rules, and external crates. |
| 7 | [DISK.md](DISK.md) | Target identity, tracked and local paths, ownership, recovery, and clean cutover. |
| 8 | [CLI.md](CLI.md) | The typed result surface and every daemonless, resident, agent, and discipline command. |
| 9 | [CONFIG.md](CONFIG.md) | Domain configuration, provenance, tracked files, keys, defaults, and override order. |
| 10 | [CANVAS.md](CANVAS.md) | The embedded Rust-wasm app, loopback split, `Packed`, capability surfaces, and distribution edge. |
| 11 | [TASTES.md](TASTES.md) | Taste meaning, authoring rules, curation, and the installed starter set. |
| 12 | [LATER.md](LATER.md) | Permanent exclusions, trigger-bound waits, and the drop ledger. |

## The architecture

Spice is one Rust binary per release target around one repository-local SQLite authority and one foreground resident writer. The writer acquires the kernel lock before opening the journal; each command then commits its id, immutable facts, authoritative task, workflow, and coordination heads, subject witnesses, and result in one transaction, while connection capabilities mint its principal and the resident epoch fences stale clients. The board reads current heads directly, folds are disposable canvas answers that authority never reads, and hermetic commands append nothing: only the resident may recognize their bounded disk evidence. Raw vendor bytes reach tape before decoding, current wire DTOs cross into the embedded Rust-wasm app, and clean cutover drains Python, exports wanted work as a task document, verifies backup, then starts Rust with an empty authority.

## Using the program

[GENESIS.md](GENESIS.md) is the authored construction program: waves in build
order, leaves with acceptance lines, and an `After` graph that is the plan of
record. It is a plan to work from, not a payload to swallow. Do not bulk-load
all of it onto a board and call that progress — too much will change while
you build for a frozen 110-row graph to stay honest.

The intended start is one agent. Read this corpus in the order above, then
begin at wave zero and build. Mint tasks incrementally as work becomes
current: a leaf becomes a live task when you are about to take it or hand it
off, carrying its acceptance lines and edges with it, and the board grows
with the work rather than ahead of it. Acceptance lines define done for the
work they describe whether or not a leaf ever became a board row. Python
spice remains the scaffold authority — its task tooling is how you mint —
until the enforcement flip.

Expect this corpus to evolve as construction teaches. When reality
contradicts a document, amend the corpus in the same change that settles the
question: FRAME.md records the amended decision first, the companion
references follow it, and affected GENESIS leaves are rewritten before they
are minted. Silent drift is the only failure mode; deliberate amendment is
the design working.

After the enforcement flip, the Rust appliance carries the same discipline:
`spice task next` is the only allocator, `spice doc` previews and applies an
authored batch by exact digest when a batch is what you need, and you return
to the allocator after every completion or assigned review. The root closes
only when the J74 generated closure gates pass — CLI contract coverage,
bidirectional fact-fold admission, graph/release closure, blocking surface,
durable inventory, corpus conformance, and lattice conformance — and the
sealed release manifest binds both target entries under their detached proof
receipts.

## Standing doctrine

1. One word names one idea; paths qualify it, and compounds mark package boundaries.
2. One execution path implements each behavior; every refusal names its release and remedy.
3. One foreground resident holds the kernel lock before opening the one authority.
4. One transaction commits command id, facts, heads, witnesses, and result.
5. Clean cutover drains Python, exports wanted work, verifies backup, and starts empty.
6. Vendor bytes become durable before decoding; sediment remains forensic evidence.
7. Folds are disposable canvas answers; authority reads current heads directly.
8. Hermetic rungs append nothing; the resident alone recognizes their bounded disk evidence.
9. Signals wake, cursors decide, channels block or stay silent, and `Packed` exposes every overflow.
10. Lanes coordinate only through the board and repository evidence; the operator reads proved outcomes, never code.
