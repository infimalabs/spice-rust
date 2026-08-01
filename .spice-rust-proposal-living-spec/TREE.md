# TREE

The Cargo workspace has twenty-one members. Only `spice` is published;
every internal crate is a path dependency with `publish = false`. One
toolchain pin carries the native targets and the wasm target. Workspace
lints deny by default, and `#[expect(lint, reason)]` is the only
exception form.

## The tree

Every module has one role. Assembly stays at the rim; authority enters
only through the guarded journal transaction.

```text
spice/                          the rim; the published binary
├── assets/
│   ├── skill/                  generated skill, checked against the clap tree
│   └── unit/                   systemd-user and launchd install templates
└── src/
    ├── main.rs                 clap entry, exit mapping, and environment edge
    ├── cli.rs                  every leaf and its typed result contract
    └── up.rs                   resident assembly after lock, migration, and epoch

wire/                           the crossing types
└── src/
    ├── ident.rs                the one admitted uuid and instant spelling
    ├── envelope.rs             version, seq, at, actor, origin, cause, and kind
    ├── fact.rs                 the closed fact vocabulary
    ├── body.rs                 one body DTO per fact kind
    ├── hello.rs                wire, capability, and epoch opening
    ├── command.rs              kind, expected witnesses, and canonical bytes
    ├── reply.rs                Reply, Refusal, and Fault
    ├── packed.rs               Packed<T>: rows, overflow, and horizon
    └── witness.rs              named predicates over command subjects

journal/                        the sole authority store
└── src/
    ├── guard.rs                JournalRead, JournalGuard, and bus-only JournalCommit
    ├── head.rs                 task statement, workflow, and coordination heads
    ├── seq.rs                  monotone cursor and cross-process wake publication
    ├── migrate.rs              prepare, crash drill, and finalize before epoch mint
    └── replay.rs               fact replay and authoritative index reconstruction

bus/                            the resident commit owner
└── src/
    ├── lib.rs                  owns the writer and commits guarded transactions
    └── timer.rs                turns lease deadlines into explicit events

fold/                           disposable materialization
└── src/
    ├── contract.rs             declares sources, cursor, horizon, and recovery
    ├── store.rs                generation-stamped transactional publication
    └── inventory.rs            admits every declared fold, head, and ring source

dialect/                        the task-document grammar
└── src/
    ├── classifier.rs           classifies structure, warnings, and refusals
    ├── node.rs                 titles, fields, projects, origins, and edges
    ├── ledger.rs               emits the byte-stable normal form
    └── refuse.rs               names every zero-write rejection

board/                          tasks and coordination as journal heads
└── src/
    ├── task.rs                 statements, projects, families, handles, and depends
    ├── claim.rs                leased possession and claim::Active proof
    ├── phase.rs                workflow stations and guarded transitions
    ├── urgency.rs              pure scalar over authoritative heads
    ├── next.rs                 the sole deterministic allocation query
    ├── apply.rs                digest-bound document application in one transaction
    ├── graph.rs                named board diagrams over authoritative heads
    ├── handout.rs              composes every named diagram into one shareable document
    └── oops.rs                 friction as wakeable work; observations as notes

listen/                         vendor bytes into typed events
└── src/
    ├── event.rs                prose, reasoning, tool, usage, compaction, and failure
    ├── claude.rs               Claude stream dialect and recorded corpus
    ├── codex.rs                Codex stream dialect and recorded corpus
    ├── residue.rs              accounts for bytes no decoder can type
    ├── canary.rs               alarms when an expected event kind disappears
    ├── watch.rs                daemonless live rendering
    └── retro.rs                daemonless recorded replay

tape/                           verbatim vendor capture
└── src/
    ├── tee.rs                  commits bytes before interpretation
    ├── seal.rs                 rotates plaintext into indexed immutable gzip members
    └── read.rs                 resolves stable member, offset, and length loci

vendor/                         outbound agent seam
└── src/
    ├── record.rs               binary, argv, stream, and resume contract
    ├── claude.rs               Claude launch and stream wiring
    ├── codex.rs                Codex launch and stream wiring
    └── acp.rs                  typed permission requests for consent

lane/                           supervised agent seats
└── src/
    ├── table.rs                lane identity, worktree, occupant, and epoch heads
    ├── occupant.rs             one supervised process group per occupied seat
    ├── life.rs                 Steer, Drive, and Drain policy
    ├── hold.rs                 pauses re-seating after repeated young deaths; lane::held and lane::resumed
    ├── drain.rs                Seated → Quiescing → Draining → Terminating → Killing → Verifying → Dark
    └── renew.rs                liveness-driven, epoch-bound claim renewal

steer/                          durable outbound direction
├── hooks/                      dynamic and static shell interception
└── src/
    ├── directives.rs           send, resend lineage, escalation, cancel, and expiry
    ├── inbox.rs                atomically renders pending direction
    ├── hook.rs                 installs and verifies activation proof
    ├── run.rs                  std-only wrapper; its bound is gate-measured
    └── ack/
        ├── keys.rs             key grammar and closed dispositions
        └── scan.rs             recognizes explicit outcomes in assistant prose

consent/                        the pre-action actuator
└── src/
    ├── policy.rs               named, ordered, first-match rules
    ├── budget.rs               bounded grants and queued renewal
    └── pend.rs                 holds requests while reversible work continues

taste/                          curated prose preferences
└── src/
    ├── bag.rs                  word-glob triggers over assistant prose
    └── judge.rs                confirms or clears hits into facts

gate/                           hermetic enforcement
└── src/
    ├── walk.rs                 ignore-driven jurisdiction
    ├── measure.rs              records the quantities ratchets defend
    ├── ratchet.rs              regression-only, shrink-only baselines
    ├── camp.rs                 audits pile-ups at inherited bounds
    ├── kill.rs                 confirms mutation regressions before latching
    ├── receipt.rs              binds verdicts to tree and policy digests
    ├── message.rs              validates the frozen Spice-Origin grammar
    ├── deny.rs                 generates denials from NAMES' Retired table
    └── contract.rs             checks every CLI leaf's generated contract row

canvas/                         loopback operator surface
└── src/
    ├── axum.rs                 socket and capability-scoped HTTP serving
    ├── view.rs                 frames the four derived views as Packed values
    └── thread.rs               serves origin-and-cause walks

app/                            embedded wasm browser body
└── src/
    ├── socket.rs               hello and current-wire frame transport
    ├── state.rs                one in-browser state over received frames
    ├── pane.rs                 capability-scoped read, steer, and grant panes
    ├── thread.rs               walks origin-and-cause chains
    └── view/                   renders Packed magnitude, flow, topology, and chronology

proof/                          sealed appliance verification
└── src/
    ├── container.rs            network-disabled, empty-cache build environment
    ├── replay.rs               replays the full gate and corpus battery
    └── evidence.rs             verifies the target manifest and proof receipt

dist/                           build orchestration
└── src/
    ├── pins.rs                 verifies toolchain, target, trunk, and container pins
    ├── app.rs                  builds the wasm app
    ├── stage.rs                creates the canonical per-target stage
    ├── embed.rs                prepares app bytes for canvas embedding
    ├── canvas.rs               builds the native surface with staged bytes
    └── prove.rs                submits the target manifest to proof

atomic/                         the one filesystem publication primitive
└── src/
    └── lib.rs                  temp, write, sync, rename, directory sync, cleanup

sill/                           the process door and flock floor
└── src/
    ├── spec.rs                 Spec: argv, cwd, env, group, deadlines, output, redaction, grace
    ├── spawn.rs                spawn-only-via-exec launch, bounded waits, process groups
    └── lock.rs                 flock possession: the kernel writer lock, named locks, pool shards
```

## The dependency lattice

Arrows point to crates a crate may use. The graph is acyclic;
`wire`, `dialect`, `atomic`, and `sill` form the floor. Every crate
that spawns a process reaches `sill`, and every crate that publishes a
file reaches `atomic`; the lattice-conformance gate holds each
`Cargo.toml` to exactly this list in both directions.

```text
wire      ← —
dialect   ← —
atomic    ← —
sill      ← —
tape      ← atomic, sill
journal   ← wire, atomic, sill
bus       ← journal, fold
fold      ← journal, wire
listen    ← wire, tape
gate      ← wire, atomic, sill
board     ← journal, fold, dialect, wire
steer     ← journal, wire, fold, atomic, sill
consent   ← journal, wire, fold
taste     ← listen, journal, wire, fold
vendor    ← listen, wire, sill
lane      ← journal, bus, vendor, tape, steer, consent, listen, fold,
            taste, atomic, sill
app       ← wire
dist      ← wire, atomic, sill
canvas    ← wire, fold, bus, journal, atomic
proof     ← gate, wire, atomic, sill
spice     ← wire, journal, bus, fold, dialect, board, listen, tape, vendor,
            lane, steer, consent, taste, gate, canvas, proof, atomic, sill
```

`gate` is journal-free, and `app` can see only `wire`. `dist` is a
workspace binary in the cargo-xtask pattern: the shipped `spice`
executable does not link it, and the target payload and sealed
manifest DTOs live in `wire`, so `proof` verifies manifests without
linking the orchestrator either. Dist builds and stages the app;
canvas embeds that staged output without depending on the app crate;
the rim orders dist → proof → seal. Those are ordered artifact edges,
not reverse Cargo edges.

## Postures

| Crate | Posture | Boundary |
| --- | --- | --- |
| spice | tokio | owns the resident runtime and assembles every native plane |
| wire | synchronous | types and codecs perform no I/O |
| journal | blocking | rusqlite work runs behind the bus boundary |
| bus | tokio + blocking | owns one dedicated writer and broadcasts after commit |
| fold | blocking | rebuild and publish run off the async executor |
| dialect | synchronous | pure text-to-graph and graph-to-text work |
| board | blocking | reads heads and contributes guarded transaction closures |
| listen | synchronous | pure byte decoding; lane owns the live adapter |
| tape | blocking | append, seal, and indexed reads stay off the executor |
| vendor | tokio | owns child streams but not resident authority |
| lane | tokio | supervises occupants, timers, and process groups |
| steer | tokio + std-only | resident direction is async; `run` is std-only after parsing; ack recognition is pure over recorded prose |
| consent | synchronous | ordered policy evaluation contributes journal actions |
| taste | synchronous | matching and judging contribute journal actions |
| gate | blocking | daemonless tool execution has explicit process bounds |
| canvas | tokio | serves loopback HTTP and current-wire sockets |
| app | wasm | browser tasks consume only wire frames |
| proof | blocking | drives the sealed container and replays gates |
| dist | blocking | drives pinned build tools in a fixed order |
| atomic | blocking | publishes filesystem state synchronously |
| sill | blocking | the one process door and flock floor; spawn, wait, and possession carry explicit bounds |

The bus alone owns the journal writer and constructs `JournalCommit`;
all other crates submit guards, facts, and head updates to its
transaction. The `run` path has no local timeout contract: the gate
measures its full hook-exec-to-child-exec bound end to end, and its
hot path performs no Git discovery, no SQLite open, and no spawn
beyond the wrapped command — repository geometry and pending steering
arrive from the hook environment and the rendered inbox file.

## Rim enforcement

The workspace `clippy.toml` denies `std::process::exit` and
`std::env::{var, vars, var_os}` outside the rim. `spice/src/main.rs`
contains the two explicit
`#[expect(clippy::disallowed_methods, reason = "...")]` annotations:
one at environment/bootstrap acquisition and one at result-to-exit
mapping. Every other lint exception must also use `#[expect]` with a
reason; `allow` is not an exception surface.

## External dependencies

The final library set is `clap`, `serde`, `serde_json`, `toml`, bundled
`rusqlite`, `tokio`, `axum`, `tokio-tungstenite`, `leptos`,
`agent-client-protocol`, `notify`, `ignore`, `tokei`, `tree-sitter`,
`aho-corasick`, `gix`, `nix`, `flate2`, `rust-embed`, `thiserror`,
`uuid`, `getrandom`, `blake3`, `jiff`, and development-only `proptest`.
There is no second task store. `uuid` and `getrandom` mint the v7
identities WIRE requires, `blake3` computes every domain-tagged durable
digest, and `jiff` owns RFC3339 instants and monotonic waits — no leaf
chooses its own identity primitives.

`nix` supplies Unix process groups and the advisory resident lock.
`notify` wakes tape, repository, and seq observers only as a hint;
each consumer compares its durable cursor and heals any missed wake.
`gix` is local-only and serves read-only Git tree, index, and commit
inspection. Browser wire types arrive through the Rust `wire` crate;
Node, npm, TypeScript, and emitted bindings do not enter the build.

The pinned tools are `trunk`, the proof container runtime, and
`cargo-mutants`; the Rust toolchain pin also carries the wasm target.
Dist verifies every pin before it builds; proof binds their identities
into the per-target manifest.
