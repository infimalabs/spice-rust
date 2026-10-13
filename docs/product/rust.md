# Rust implementation profile

[Spice product model](README.md)

The concrete choices of the Rust appliance that no other document states, and
the code that owns each. The live code is authoritative.

## Task identity

Task and steering stamps are fixed-width, vowel-free encodings of a microsecond
instant, so string order matches instant order within one width and a stamp
never spells a word. A collision rewinds to the nearest earlier free
microsecond, never a future instant or a random suffix, so a later mint can sort
before the task it collided with: the journal sequence and the durable queue
origin, not stamp order, are creation chronology.

Owner: [`wire/src/witness.rs`](../../wire/src/witness.rs).

## Agent sessions

A lane states the wire it reads rather than inheriting one. Its opening request
negotiates OMP's RPC protocol version before any other command, because only
that version chunks an oversized frame whole instead of compacting or replacing
it; a refused negotiation fails the lane rather than launching on an
undecodable wire. Capture reassembles chunks into the logical record before it
reaches the tape, so live decoding and replay read the same bytes.

The same request states the steering and follow-up dispositions (drain every
queued message at a boundary) and the interrupt disposition (notice steering
immediately), which OMP would otherwise take from the host's own settings. Each
launch pins its configuration overlay and steering extension beside the
occupant's supervisor record, so no global OMP setting is read or changed. The
extension clears OMP's steering flag on the copy handed to the provider, so a
steered message reaches the model as the lane wrote it, not inside OMP's
generated priority notice.

The first turn is one `prompt` request with follow-up admission. A fresh or
idle session starts it immediately; a resumed session that restored an
in-flight turn queues it once behind that owner. OMP therefore answers the
request as accepted or queued instead of exposing its transient busy refusal.
Other prompt refusals remain visible and state that no message was accepted.

Owners: [`vendor/src/omp.rs`](../../vendor/src/omp.rs),
[`vendor/src/omp/frame.rs`](../../vendor/src/omp/frame.rs),
[`vendor/src/omp/steering.mjs`](../../vendor/src/omp/steering.mjs), and
[`lane/src/capture.rs`](../../lane/src/capture.rs).

## Authority and process life

One resident holds the kernel lock and is the sole writer to one SQLite journal.
A command transaction commits its facts, heads, result, and wake cursor
together. Transcript tape preserves vendor bytes without becoming semantic
authority.

Occupants cooperate under one Unix user. Capabilities bind attribution, lane,
occupant, and epoch; they are workflow interlocks, not isolation between
same-user processes. Session authority comes only from an admitted live vendor
identity; transcript prose may select evidence but never impersonates the
caller.

The resident runs in the foreground of its owning process. `spice up` may detach
that process, `spice down` owns semantic shutdown, and an operating-system
service manager may own restart policy.

Owners: [`journal/src/guard.rs`](../../journal/src/guard.rs),
[`spice/src/session_command/authority.rs`](../../spice/src/session_command/authority.rs),
and [`spice/src/resident/lifecycle.rs`](../../spice/src/resident/lifecycle.rs).

## Command vocabulary

One compiled extension registry owns vendors, wrappers, and gates, and a closed
command-contract registry owns the built-in verbs. Repository command mounts
join the CLI only after collision and configuration admission; a mount that
collides with a built-in path refuses before dispatch. Those registries and the
admitted mounts are the entire command vocabulary.

Owners: [`spice/src/extension.rs`](../../spice/src/extension.rs),
[`spice/src/cli/contracts/registry.rs`](../../spice/src/cli/contracts/registry.rs),
and [`spice/src/cli/mounts.rs`](../../spice/src/cli/mounts.rs).

## App and renderers

Rust/WASM owns the app; JavaScript assets only render, and `wire` owns the DTOs.
Each app generation is immutable and binds its renderer members by exact digest.
Diagram and math renderers run in a sandboxed frame pinned to the loaded page's
generation and theme; the parent admits only the decoded image, never renderer
markup in its own DOM. Renderer modules are checked-in bytes, so rendering makes
no runtime network or package request.

Owners: [`wire/src/manifest/renderer.rs`](../../wire/src/manifest/renderer.rs),
[`app/src/browser/mermaid.rs`](../../app/src/browser/mermaid.rs), and
[`app/renderer/renderer-000.txt`](../../app/renderer/renderer-000.txt). Which
activity becomes a Mosaic card lives in [Mosaic card lineage](mosaic-rust.md).

A resident restart keeps the app's last good surface on screen until the new
session's scope is fully hydrated. A page whose generation the resident no
longer serves reloads once into the served generation; unsent composer and
observation drafts travel to the reloaded page through tab-scoped storage, with
attachment annotations flattened into their image. Only a submission whose
result is still unknown keeps the stale page in place behind the reload control.
Owners: [`app/src/runtime/publication.rs`](../../app/src/runtime/publication.rs),
[`app/src/browser/connection.rs`](../../app/src/browser/connection.rs), and
[`app/src/browser/draft_handoff.rs`](../../app/src/browser/draft_handoff.rs).

## Version identity

`spice --version`, `spice version`, and the app menu report the tracked version
from `dist/VERSION`; an installed pre-release extends it with a source stamp
that orders successive builds of tied commits. The stamp follows the locally
available first-parent chain, treating a declared shallow boundary as the end
of that history while refusing an unexpectedly missing commit. The version
never carries the executable digest: the native-binary identity is the running
executable's complete digest, so identical bytes keep one identity without a
Git checkout.

Owners: [`spice/src/version.rs`](../../spice/src/version.rs) and
[`xtask/src/develop/native.rs`](../../xtask/src/develop/native.rs).
