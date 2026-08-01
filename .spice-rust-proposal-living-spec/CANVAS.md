# CANVAS

The canvas is the resident's loopback operator surface. Its browser body is
Rust compiled to wasm, its server is Rust in the appliance, and their common
language is the current wire crate. It reads and steers one authority through
capability-scoped commands; it never becomes another authority.

## The decision

**The app is Leptos, client-side rendered, compiled to wasm, and embedded in
the target binary.** This is settled architecture, not a framework trial.

Four grounds carry the decision:

1. **The wire crate compiles into the browser.** The app consumes the same
   current DTOs that frame the resident socket. There are no generated
   bindings, emitted types, mirrored discriminants, or second decoder. A
   server field the app needs but wire lacks is a compile-time contract gap.
2. **The gate governs the app.** The browser body remains under the workspace's
   deny-by-default lints, own-history floors, mutation checks, fixture corpus,
   and receipts. Browser behavior is not an exempt language island with a
   parallel policy system.
3. **The target manifest identifies the whole surface.** The executable digest
   and embedded app digest are bound with the toolchain, container, and proof
   receipt for one target tuple. The frontend cannot drift from the appliance
   whose manifest names it.
4. **Signals match the data.** One current socket stream updates fine-grained
   state and derived panes. Leptos signals express that dependency graph
   directly without creating a browser-side store of authority.

The accepted costs are wasm weight on a loopback surface, a smaller framework
talent pool, and bespoke chart work. The four views already require domain
SVG rather than a general chart kit; shared Rust types and one sealed build
outweigh those costs. None is a release-one reopening condition.

## The split

Three crates meet at two explicit edges:

```text
wire/
└── src/
    ├── hello.rs                current wire, capability, and epoch
    ├── reply.rs                Reply, Refusal, and Fault
    └── packed.rs               Packed<T>: rows, overflow, and horizon

app/                            browser body; app ← wire only
└── src/
    ├── socket.rs               current-wire hello and frame transport
    ├── state.rs                signals derived only from received frames
    ├── pane.rs                 capability-scoped read, steer, and grant panes
    ├── thread.rs               origin-and-cause walks
    └── view/
        ├── magnitude.rs        effort, pressure, residue, and consent rates
        ├── flow.rs             task flows in motion
        ├── topology.rs         lanes, worktrees, and lineage
        └── chronology.rs       compaction, consent, trust, commands, recovery

canvas/                         loopback server inside the native appliance
└── src/
    ├── axum.rs                 HTTP, socket hello, and capability admission
    ├── view.rs                 four derived views framed as Packed values
    └── thread.rs               origin-and-cause walks framed for the app
```

`app` depends only on `wire`. Journal, board, fold, lane, and trust machinery
are unreachable from browser code; the socket is the app's complete system
boundary. `canvas` reads the registered materializations and authority indexes
needed to frame wire replies, but every mutation crosses the normal command
door. The split makes missing wire contracts visible to the compiler instead
of inviting an internal import.

## Packed

`Packed<T>` lives in `wire` and carries the one list contract:

```text
Packed<T> {
    rows,
    overflow,
    horizon,
}
```

The producing domain ranks candidates by its declared key before it packs.
`rows` contains the visible prefix, `overflow` explicitly describes what did
not fit, and `horizon` names the source completeness boundary. No canvas list
crosses the socket as a bare vector, and no renderer infers truncation from
length.

Canvas frames `Packed<T>` from its declared sources; app renders the same type
into panes. These are two renderers over one wire contract, not two pack
implementations. Canvas and app may choose different presentation space, but
they preserve the same rank, overflow, and horizon semantics. A fold-backed
read arrives wrapped in wire's `Derived<T>`, whose fold, gen, and cursor let a
pane show which generation answered and how far it had applied the journal;
head-direct reads arrive as bare `Packed<T>` and never invent that provenance.

## Capability surfaces

Every socket opens with `{wire, capability, epoch}`. The resident resolves the
presented capability to an operator or lane principal and derives actor,
origin rights, lane, and current epoch server-side. Caller-supplied
attribution remains data and cannot unlock a control.

| Surface | Presented principal | Canvas contract |
| --- | --- | --- |
| Read | Operator or lane capability | Show only the current packed views, threads, and repository state admitted for that principal. Reads carry their cursor or horizon and append nothing. |
| Steer | A capability whose principal is admitted for the named command | Present task, lane, directive, and other command controls only in the authenticated scope. Submission uses the same witnesses, Reply/Refusal/Fault grammar, and command transaction as the CLI. |
| Grant | Operator capability | Present trust and consent decisions that require operator authority. Lane capabilities never acquire grant authority from caller text or a rendered control. |

Rendering is not enforcement. Hiding a pane prevents a misleading affordance;
the resident still authenticates every command, derives its principal, proves
its witnesses, and either commits one transaction or returns a zero-write
refusal. A stale epoch faults by name. The operator capability remains in the
mode-`0600` canvas discovery file and is never rendered into a pane.

The loopback surface is also a web surface, and it defends itself as one.
The server binds `127.0.0.1` only and validates the `Host` and `Origin`
headers of every HTTP and socket request against its own published endpoint,
so a DNS-rebinding page or cross-origin WebSocket in the operator's browser
reaches a refusal, not a session. The capability crosses only inside the
socket `hello` payload — never in a URL, query string, fragment, or cookie —
so browser history, screenshots, and logs cannot leak it; the discovery file
is the one transport, and each `spice up` replaces endpoint, capability, and
epoch together. There is no session cookie to ride, so there is no CSRF
surface: every mutation is an explicit capability-bearing command.

## Derivation

The four axes remain the complete view vocabulary:

| Axis | Question answered | Derived content |
| --- | --- | --- |
| Magnitude | How much exists now? | Effort, pressure, residue, active counts, pending consent, and consent rates, ranked and packed at one horizon. |
| Flow | What work is moving? | Tasks crossing phases, claims, deferrals, reviews, and their current authoritative heads. |
| Topology | What is connected and possessed? | Lanes, worktrees, occupants, lineage, current claims, and resident state. |
| Chronology | What happened, in what order, and what waits? | Compaction, queues, directives, human delay, lifecycle transitions, refusal telemetry, and command outcomes. |

New planes become panes on those axes rather than new views:

| Plane | Axis | Derivation |
| --- | --- | --- |
| Consent | Magnitude | Remaining budgets, pending volume, and disposition rates are amounts at the packed horizon. |
| Consent | Chronology | Asked, queued, granted, denied, and floored dispositions appear in time with their named rule and actor. |
| Trust | Chronology | Pended, granted, and revoked digests appear as operator-attributed transitions. |
| Commands | Chronology | Reply, refusal telemetry, fault, witness argument, receipt, and remedy appear beside the subject history without turning refusals into facts. |
| Recovery | Chronology | Quiescing, draining, migration, backup, restore, and verification outcomes appear in their recorded order. |

One derivation leaf audits that every pane maps to one of these axes, that every
list crosses as `Packed<T>`, and that each source is named in the fold or head
registry. A feature that cannot make that mapping is a contract gap, not a
fifth view.

## The build edge

`dist` owns the cargo-invisible artifact edge from browser source to embedded
appliance:

```text
verify pins → build app → stage → embed → build canvas → prove
```

It verifies the Rust toolchain, wasm target, pinned `trunk`, and container;
builds the app; stages canonical app bytes; embeds those bytes while building
the target canvas; and emits the receipt-free target payload proof consumes.
The canvas crate does not depend on the app crate, and neither canvas nor
proof quietly rebuilds browser assets through a second path.

The payload binds the target tuple, native executable digest, embedded app
digest, and toolchain and container identity. Proof consumes that payload
digest, replays the sealed checks, boots the appliance, and verifies the
embedded app by identity: the bytes the running canvas serves must hash to
the payload's app digest. It emits a detached receipt; the post-proof seal
then composes the one canonical release manifest — both target entries with
their receipts — and publication cites only that sealed object. No browser
runs inside sealed proof: rendering behavior is proved by the fixture
corpus at build time, and the sealed environment proves identity and
serving, which is what packaging can actually break.

## The laws carried over

- **One current decoder.** Browser and server compile the same wire DTOs. A
  socket accepts only the current wire version, and the browser has no
  independent schema clock or compatibility arm.
- **Rank, then pack.** Every list crosses as wire-owned `Packed<T>`. Overflow
  and horizon remain visible through both renderers.
- **Styling is CSS.** One sheet and custom properties carry layout and palette.
  Views use SVG and grid without a utility framework or CSS-in-Rust policy
  layer.
- **The app appends nothing directly.** Read panes append nothing. Steer and
  grant panes submit ordinary commands through the resident; only the bus can
  construct journal commit capability.
- **Fixtures over pixels.** App tests consume public wire fixtures shared with
  the server and assert pane state. Contractual absence remains testable,
  but source-text UI assertions and generated binding tests do not return.
  Rendering smoke belongs to the fixture corpus; sealed proof verifies the
  embedded app by digest and serve, never by driving a browser.

- **No secret rendered.** The operator capability, tape bytes, and consent
  request payloads render only behind the operator capability, and the
  fixture corpus proves the negative: no fixture state may place a
  capability, raw tape span, or credential-bearing payload on an
  unauthenticated or lane-scoped surface.
- **Authority precedes surface.** The canvas lands after the authority,
  command, board, steering, consent, and trust contracts it presents. It does
  not create placeholder controls for work that lacks evidence.

## What this eliminates

The architecture removes generated browser bindings, schema emission,
TypeScript compilation, Node and npm dependency trees, mirrored decoders, and
their staleness checks. It also removes direct app-to-internal imports,
browser-local authority, untyped list truncation, capability decisions made
from rendered state, and parallel frontend build scripts.

The prior browser implementation is not ported line for line. The current wire
schema, four derived axes, capability surfaces, and shared fixtures are the
specification. Everything else is rebuilt as Rust under the same target
manifest and proof that govern the appliance.
