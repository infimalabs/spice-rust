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

A session's acceptance names the release the resident runs. A client that
receives an answer its command registry cannot bind refuses it unread with
`command::answer-binding`; when the two releases differ, the refusal names both
and the side that must move: an older resident restarts on the client's
release, and an older client gives way to the resident's installed `spice`.

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

Each staged generation G also has one canvas archive, `generations/G.zip`,
beside its unzipped directory, which stays the development hot-swap source. G
is the identity the shell names in every qualified route, not the archive
digest. Members are sorted by logical path. Each member is one deterministic
gzip stream (fixed header, one pinned deflate implementation and level) stored
in ZIP with method STORE and the fixed 1980-01-01 time. The archive has no data
descriptors, extra fields, comments, encryption, or ZIP64. Its bytes are
therefore a pure function of the generation files, and a member's stored bytes
are served as-is under `Content-Encoding: gzip`. Admission parses the
end-of-central-directory record and the central directory once. It refuses,
naming the member, any byte outside the records, header disagreement, path
outside the shell inventory, framing violation, or exceeded bound. It derives G
from the shell, the only member it inflates. Integrity rests on the archive
digest, not on ZIP CRC fields. Owners:
[`serve/src/canvas/archive.rs`](../../serve/src/canvas/archive.rs) writes and
admits the archive, and [`xtask/src/app.rs`](../../xtask/src/app.rs) stages it.

The `canvas` crate declares the two symbols a relocatable object defines: the
archive bytes alone in their own read-only section, and their length. The
process reads its app as a direct static slice without parsing its own image.
Only builds that enable the `spice/canvas` feature resolve the crate, and those
builds write the object for the staged archive and hand it to the final link
through `cargo rustc`. The object's name carries the archive digest, so a new
archive relinks the executable and rebuilds nothing else.
[`spice/src/version.rs`](../../spice/src/version.rs) admits the linked archive
once. An installed resident without one refuses with
`serve::canvas-archive-absent`, or `serve::canvas-archive-invalid` when it does
not admit. Owners: [`canvas/src/lib.rs`](../../canvas/src/lib.rs) for the
symbols, [`dist/src/canvas.rs`](../../dist/src/canvas.rs) for writing the
object and for the section read and identity checks, and
[`xtask/src/app.rs`](../../xtask/src/app.rs) for the link arguments.

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
Git checkout. `spice --version` ends with `app G` when the executable links
generation G's archive.

Owners: [`spice/src/version.rs`](../../spice/src/version.rs) and
[`xtask/src/develop/native.rs`](../../xtask/src/develop/native.rs).
