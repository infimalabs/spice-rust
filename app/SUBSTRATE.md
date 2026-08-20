# Canvas browser substrate

Status: accepted on 2026-08-20 by `ARCHITE-1kNlsXxy`.

## Decision

Rust/WASM remains the only production Canvas browser substrate. Do not start a
parallel TypeScript client.

This decision was made only after the resident passed the bounded-client gate:
the real resident produced no new facts while disconnected, no new facts or
unsolicited frames while connected, and exactly one `Pong` for one `Ping` over
intervals longer than the roughly one-event-per-second signature that prompted
the investigation. The earlier hot loop was unbounded Facts/Thread protocol
work, not an inherent cost of Rust/WASM. The repaired path now uses bounded
history, indexed reads, finite browser retention, and one coalesced live-thread
refresh.

JavaScript remains the browser-test language. It is not a second production
runtime.

## Comparison

The comparison uses the selected repository at this decision, not framework
benchmarks from a different application.

### Implementation latency

- Rust/WASM is available now with the current controls, reconnect behavior,
  composition queues, speech, mosaic layout, and browser coverage. A warm
  `cargo xtask app` publication observed during the recovery took about 11
  seconds.
- A small TypeScript read-only shell would be quick, but it would not be the
  current Canvas. Useful parity requires rebuilding admission, reconnection,
  command correlation, optimistic topology, submissions, speech, and mosaic
  state. Running both during that work adds synchronization and diagnosis cost.

### Protocol ownership

- Rust/WASM imports the closed `wire` DTOs directly. `Session` admits versions,
  epochs, cursors, outcomes, and credential-free stream folds before
  `CanvasRuntime` can retain them. Rust exhaustiveness makes new frame and
  command variants visible at compile time.
- TypeScript must either duplicate the Wire schema and its strict admission
  rules or add a Rust-to-TypeScript generation authority. Handwritten DTOs
  create two protocol owners; generation adds a build contract before UI work.

### Runtime cost

- The selected Rust/WASM release contains a 2,971,514-byte uncompressed WASM
  module and 62,769 bytes of JavaScript glue. Facts, threads, messages, and
  activities now have explicit bounds. Measured resident amplification
  disappeared when the request contract was repaired.
- A thin TypeScript client should transfer and initialize less code, but no
  candidate exists to measure. Payload is a credible advantage, not evidence
  that it dominates current startup or interaction latency. Feature parity
  would also make the candidate larger than a thin shell.

### Verification cost

- Rust/WASM already occupies the existing WASM Clippy, workspace WASM check,
  atomic `xtask app` publication, and real-browser actions. Source identity is
  embedded in the WASM and checked before publication.
- The repository has no TypeScript compiler or production dependency today;
  `package.json` owns Playwright only. TypeScript paths are classified by
  existing policy, but a candidate still needs a production compiler/type-check
  integration, source identity, and output validation. The existing publisher
  and browser action can be reused, but the change is not infrastructure-free.

### Current size

- `app/src` contains 15,467 lines of Rust, with 5,868 lines of native Rust tests
  and 11,629 lines of browser harnesses. That is substantial, but it is working
  scope rather than a migration estimate.
- “Thin” TypeScript is smaller only by omitting current scope. The relevant
  estimate is the replacement slice plus the compatibility period, not an empty
  TypeScript entry point.

The TypeScript advantages are real: faster editing for browser-only work,
smaller likely transfer size, and direct access to browser APIs. They do not
outweigh a second protocol authority and a parity migration while the measured
failure is already fixed. Revisit the choice when measured browser startup,
memory, or implementation latency—not resident journal size—becomes the
dominant operator cost.

## Boundary constitution

The decision binds current work to these rules:

1. `/bootstrap`, the authenticated `/socket`, the current Wire version, and
   resident command semantics remain the product boundary. Browser work may not
   reach around them into journal or resident internals.
2. `wire` remains the sole protocol authority. Browser state retains only
   admitted, capability-safe projections.
3. Canvas request and retention work stays bounded independently of retained
   journal history. A substrate change may not restore whole-journal snapshots
   or per-fact request fanout.
4. Exactly one production client is published in a generation. Tests may
   compare candidates; the resident does not serve two product clients.
5. `cargo xtask app` remains the atomic publication door, and the existing
   real-browser verifier remains the final production-edge proof.

## Conditional TypeScript migration

This is a migration sequence, not authorization to begin it. It becomes active
only after a task records a reproducible browser-side cost that remains material
with the bounded Rust client.

1. Freeze the resident boundary first. Capture current bootstrap, stream,
   command, refusal, reconnect, redaction, and packed-continuation fixtures as
   the compatibility contract. Do not add a TypeScript-only server route.
2. Establish one generated protocol authority from `wire`, including strict
   decoding and version/epoch/cursor admission. Do not hand-maintain a second
   DTO vocabulary. Generation must fail when the checked-in output differs.
3. Build a candidate against recorded frames and the existing real resident.
   Port the smallest complete vertical slice—discovery, authentication,
   subscription, one read pane, one command, reconnect, and terminal frames—so
   the comparison exercises the whole boundary rather than a static mockup.
4. Keep the existing `cargo xtask app` action, compiler-ready receipt, selected
   source identity, immutable generation directory, and browser action. Replace
   the compiler invoked inside that publication action and update its output
   validation; do not create a second verifier or a second publication path.
5. Replay the current browser scenarios unchanged where they express product
   behavior. Add cross-client fixture equivalence for admission and redaction,
   then remove equivalence scaffolding at cutover rather than retaining two
   permanent suites.
6. Measure candidate transfer bytes, startup-to-admission time, idle CPU and
   memory, fact-burst behavior, and implementation time for the same bounded
   change. Choose TypeScript only if those measurements justify the migration
   and every boundary rule above still passes.
7. Cut over as one immutable app generation. In the same landed change, remove
   the Rust production client and retire only the now-obsolete WASM-specific
   verifier steps. Rollback is the prior source commit and prior atomic
   generation, not a runtime substrate switch.

Until that sequence is explicitly activated and passes, production Canvas work
continues in Rust/WASM.
