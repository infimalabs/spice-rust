# Mosaic Rust implementation boundary

[Spice Rust implementation profile](rust.md)

The Mosaic behavior in the product model is carried forward through one current
path, not a predecessor compatibility mode. Typed `AgentMessage` values enter
the canonical runtime card reducer, which orders them by their chronology key,
deduplicates them by lane and locus, and applies the stream's bounded per-lane
retention. Member-local `MessageHistory` pages merge through that same reducer;
a response that does not name the lane's current cursor changes nothing. The
resulting lineage stream feeds one persistent plane whose pure Rust lattice owns
placement, freezing, liquid settlement, ripple, backfill, geometry replay, and
compaction-epoch replay. Mosaic layout and lineage reduction remain Rust-owned.

Turn-start events remain lifecycle evidence, not Mosaic cards. The card reducer
omits them for live delivery and retained history, so they consume no card slots
and render no turn announcement. Final messages retain their FINAL badge; the
next message makes the new turn apparent. Compaction
events still render their context divider. Each divider keeps the relative-age
link in its fixed-width slot and separates it from the agent/cause context with
an explicit layout gap rather than authored whitespace or font metrics. History
cursors advance from the oldest same-lane source record in each page,
independently of card eligibility, so a page containing only hidden turn events
still leads to older prose.

Fresh vendor sessions render a distinct "started fresh session" lifecycle floor
through the same full-width divider and epoch layout as compaction. Initial
creation and explicit renewal record `lane::started.fresh_session = true`;
exact-session resumes record false and create no floor. The resident retains
the selected disposition before OMP pins a new session identity. Older facts
without this evidence create no inferred floor. Live delivery and fact replay
share the existing sequence-keyed lifecycle reduction.

The predecessor's implementation details differ where the Rust product has a
stronger ownership boundary:

- One team plane merges all team members instead of maintaining a
  JavaScript lane store. Every card retains explicit lane and author identity,
  and each member retains an independent typed history frontier, so a delayed
  member cannot stall or overwrite its siblings.
- Older history pages only on reader demand: a member's placed frontier card
  within reach of the viewport, or the plane's trailing edge. A card still
  measuring sits hidden at the plane's top edge and is not demand, so arriving
  pages do not chain into autonomous paging. The pager re-inspects after each
  plane commit, the moment a frontier can come into reach. A dropped socket
  never answers the pages it carried, so a member whose admitted page went
  unanswered across a reconnect is requested again wherever its frontier now
  sits. An unscrolled cold load therefore retains the same history whatever
  the socket timing or a reconnect.
- Keyed Yew nodes replace imperative node ownership. Measurement is deferred
  until the host has a usable width, cached by card fingerprint, image
  revision, and width, and retained for only the current and immediately prior
  widths. A base measurement admits at most one wider candidate, so ordinary
  placement performs at most two layout reads per card. Hidden first-load cards
  measure in place; committed cards that need remeasurement use isolated clones.
  Both paths collect their quote image-flow descendants once, then settle
  independent cards in shared read/write passes at each measured span. Each
  card still owns its settlement, so its height never depends on whether or at
  which width the live card's observer last settled it.
- Incremental reconciliation retains stable compaction and lifecycle dividers
  only while every full-width boundary still separates the chronological epochs
  on both sides. A batched measurement and stream-structure change that cannot
  honor those fixed floors takes the deterministic full-replay path instead.
  A replayed epoch rests against the retained boundary above it, carrying all
  older history with it, as backfill does. A card that a retention horizon
  admitted during history paging and later evicted therefore leaves no gap:
  the plane matches a fresh load of the same cards, whichever page batching
  produced it.
- Yew renders integer layout diagnostics as attributes, then an effect copies
  changed values into CSS custom properties. This is the single CSP-safe
  geometry channel and avoids Firefox's unsupported typed `attr()` arithmetic.
  The copy compares the current property first, so unchanged card placement
  receives no style write.
- Scroll correction retains the nearest visible keyed card at its exact
  viewport offset instead of inferring the reader's position from the plane's
  maximum-row delta. This preserves the same 80px near-top policy while also
  covering nonuniform growth, backfill, and compaction-epoch replacement.
- Image load and error events settle through one card coordinator. A card is
  remeasured only after all of its images are terminal, and repeated terminal
  events do not publish another content revision. This removes the
  predecessor's per-image render cascade without changing placeholder or final
  geometry.
- The predecessor's two-layer positional tween is not transplanted into Yew.
  One effect lands plane properties before card properties, and liquid cards
  expose only a 180ms opacity transition; frozen cards expose no transition.
  Reduced motion removes that fade but never disables same-frame scroll
  correction. This keeps motion tied to real arrivals without exposing stale
  intermediate geometry or scroll overflow.
- The current surface uses the product's merged console hierarchy, card chrome,
  attribution, and responsive controls. The visual parity target is therefore
  the packed chronology, stable card geometry, media containment, compaction
  boundaries, readable loading state, and motion contract; predecessor product
  chrome and palette are not execution contracts.

All Mosaic production work is event-driven by an admitted stream frame, width
change, history-edge scroll, or image terminal event. A no-op reconcile returns
the existing plane, publication shares retained payloads, and keyed mode
switches neither rebuild nor remeasure the plane. The performance profile
exercises 75 and 768 retained cards and requires one cold commit, one read for
each ordinary card, zero card additions or removals on a mode round trip, zero
mode-switch commits, zero extra measurements, and zero transport frames.

Authorities: [`app/src/runtime/message_cards.rs`](../../app/src/runtime/message_cards.rs),
[`app/src/mosaic.rs`](../../app/src/mosaic.rs),
[`app/src/mosaic_plane.rs`](../../app/src/mosaic_plane.rs),
[`app/src/browser/watchband/mosaic_view.rs`](../../app/src/browser/watchband/mosaic_view.rs),
[`app/src/browser/watchband/mosaic_view/history.rs`](../../app/src/browser/watchband/mosaic_view/history.rs),
and
[`app/src/browser/watchband/mosaic_view/image_resolution.rs`](../../app/src/browser/watchband/mosaic_view/image_resolution.rs).
Executable parity and cost evidence lives in
[`app/tests/mosaic_sequences.rs`](../../app/tests/mosaic_sequences.rs) and
[`app/tests/mosaic_stability.rs`](../../app/tests/mosaic_stability.rs).
Browser history, image resolution, and painting require disposable actual-surface
verification against the production app.
