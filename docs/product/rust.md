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

A resident starts only over its own installed skill. `spice up`, foreground or
detached, refuses `resident::skill-unavailable` with `spice install` as the
repair when the Git-common `.spice-install/agents/skills/spice/SKILL.md` is
absent, not a regular file, outside Git-common authority, or not the bytes its
executable compiles in: a resident that cannot launch a lane is not serving.
Lane launch applies the same predicate, since the skill can change under a
running resident.

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

## Windows development C toolchain

The tracked Rust toolchain includes `x86_64-pc-windows-msvc`, but Windows
development checks also require native C and assembly producers. The repository
declares one Apple-silicon host toolchain in
[`dist/windows-msvc.toml`](../../dist/windows-msvc.toml): exact xwin and LLVM
releases and executable hashes; one closure digest over every Homebrew dylib
those LLVM tools load and every clang builtin header; the Visual Studio channel
and package manifest hashes; and exact CRT and SDK selectors. The declaration
also names the durable operator direction that accepted Microsoft's applicable
licence terms.

After installing the declared Homebrew bottles (`xwin`, `llvm@20`, and
`lld@20`), materialize the CRT and SDK with:

```sh
spice dev xtask -- windows-msvc
```

The explicit command consumes the acceptance named by the declaration; callers
do not repeat that key. Before invoking xwin it resolves the key against the
repository journal and requires its original record to be a human principal
holding the operator role whose text accepts the applicable Microsoft Software
License Terms for both the Microsoft CRT and Windows SDK. An absent, malformed,
non-operator, or unrelated record refuses. The command then verifies xwin
before it downloads Microsoft payloads, verifies both manifests before
splatting, and publishes a read-only, content-addressed generation under the
bootstrap Cargo home.

A Cargo invocation selecting `x86_64-pc-windows-msvc` rehashes that
generation, each pinned launcher, every non-platform dylib in its transitive
load closure, and clang's complete builtin include tree before launch. macOS
platform libraries remain covered by the signed system volume and dyld shared
cache rather than mutable Homebrew paths. Only that target gets the `cl.exe`,
`ml64.exe`, `lib.exe`, and `link.exe` wrappers: they resolve to the admitted
clang-cl, llvm-ml, llvm-lib, and lld-link bytes, and both clang-cl and lld-link
receive the admitted xwin `/winsysroot`, so final x64 executables link against
the admitted CRT and SDK import libraries. Missing or changed bytes refuse
instead of falling back to Apple's compiler, ambient headers, or another
toolchain.

Those x64 executables run on the development host only inside the persistent
Windows 11 ARM developer guest, as non-authoritative developer evidence; see
[Persistent Windows 11 ARM developer guest](planes/distribution.md#persistent-windows-11-arm-developer-guest).

Owners: [`spice/src/cargo_isolation/windows_msvc.rs`](../../spice/src/cargo_isolation/windows_msvc.rs)
for recorded-acceptance admission, materialization, verification, and Cargo
binding, and [`xtask/src/windows_msvc.rs`](../../xtask/src/windows_msvc.rs) for
the explicit materialization command.

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

Each staged generation G also has one app ZIP, `generations/G.zip`, beside its
unzipped directory, which stays the development hot-swap source. Its members are
the generation's files under `app/`. G is the identity the shell names in every
qualified route, not the archive digest.

Every appliance executable links one spice rack in the same format: `app/`
members copied unchanged from `G.zip`, `skill/SKILL.md`, `doc/...` from
`docs/`, and `tool/NAME/...` beside each tool's `tool/NAME/manifest.json`.
Members are sorted by logical path. Each member is one deterministic gzip
stream (fixed header, one pinned deflate implementation and level) stored in
ZIP with method STORE and the fixed 1980-01-01 time. The archive has no data
descriptors, extra fields, comments, encryption, or ZIP64. Its bytes are
therefore a pure function of its members, and an app member's stored bytes are
served as-is under `Content-Encoding: gzip`. Admission parses the
end-of-central-directory record and the central directory once. It refuses,
naming the member, any byte outside the records, header disagreement, path
outside every namespace or outside the app shell inventory, a tool without its
manifest, framing violation, or exceeded bound. Member bounds are declared per
namespace and summed; member and total byte bounds cover the whole rack. It
derives G from the app shell, the only member it inflates. Integrity rests on
the archive digest, not on ZIP CRC fields. Owners:
[`serve/src/rack.rs`](../../serve/src/rack.rs) writes and admits racks,
[`xtask/src/app.rs`](../../xtask/src/app.rs) stages `G.zip`, and
[`xtask/src/app/rack.rs`](../../xtask/src/app/rack.rs) assembles the linked
rack from its tracked source list.

A resident reads rack members as `spice://NS/PATH` with the line selectors the
other resources take; `spice://SKILL.md` is `spice://skill/SKILL.md`, and a
namespace or directory read lists its members. At startup an installed
executable admits the rack it links through one `rack::admitted` fact naming
the rack archive digest and its app generation, appended once per distinct
archive; that fact's sequence code is the generation short code a rack URL
names: `spice://NS/PATH@CODE` answers exactly what `spice://NS/PATH` answers
when CODE is the linked rack's code, and another code is refused naming the
linked one. A URL never names a generation by digest or UUID, and an
executable whose rack no fact admits refuses every `@` segment. A member's
media type is the one its path extension declares, never sniffed from its
bytes. A text member answers inline; any other member answers one line naming
its media type, size, digest, the generation short code when one exists, and
a file under `.spice/rack/G/` named by that digest, extracted once on the
first read and never rewritten. A member above the 16 MiB extraction bound is
refused. `spice lane purge` retires the directories of every generation but
the linked one. `spice resource read URL` prints exactly what a lane's host
frame carries.

The `canvas` crate declares the two symbols a relocatable object defines:
`spicezip` at the rack's first byte and `spicezip_len` at its length. Every
current appliance target resolves the first to a mapped address, so the process
reads its rack as a direct static slice without storing a file offset or
parsing its own image.

At the object-format boundary, `dist` locates that rack in final Mach-O, ELF,
and PE32+ images: `__spicezip` on Mach-O and `spicezip` on ELF and PE32+, one
word that exactly fills PE's eight-byte section-name limit. The Windows
development build enables the same ordinary `spice` binary and modules as macOS
and Linux; only the filesystem, process, and local-IPC adapters vary by target.

Materialize the pinned xwin/LLVM toolchain, cross-check the ordinary executable,
then build the complete executable with the current staged canvas:

```sh
spice dev xtask -- windows-msvc
spice dev cargo -- build --locked --release --target x86_64-pc-windows-msvc -p spice --bin spice
spice dev xtask -- windows-appliance
```

Every Gate selection reached by Rust source, a Cargo manifest, `Cargo.lock`, or
`rust-toolchain.toml` also plans these three cacheable cross-target checks:

```sh
cargo check --locked --package spice --bin spice --target x86_64-pc-windows-msvc
cargo check --locked --package spice --tests --target x86_64-pc-windows-msvc
cargo check --locked --package sill --tests --target x86_64-pc-windows-msvc
```

Each runs with `CARGO_ENCODED_RUSTFLAGS` set to the inherited rustflags
followed by `-D warnings`, so rustc warnings in Windows-only code fail the
check. Cargo reads that variable ahead of `RUSTFLAGS` and any configured
rustflags, so an inherited setting extends the deny but cannot displace it; the
inherited flags are `CARGO_ENCODED_RUSTFLAGS` when set, else `RUSTFLAGS`. With
`--target`, the flags bind only the Windows units, and host checks and tests
keep their rustflags as inherited. The host lint pass already runs clippy over
every other line, so the cross-target shard spends the check phase only. Their
proof inputs include `dist/windows-msvc.toml`, the complete package closure,
and only the changed paths whose check selection reaches their package, so an
edit no checked package compiles leaves their receipts reusable. They use
Gate's admitted Cargo home and pinned toolchain, produce typed compiler
diagnostics, and perform no link or guest execution. A cached host check
therefore cannot conceal a new Unix-only API in shared source.

The final command writes and verifies
`target/x86_64-pc-windows-msvc/release/spice.exe`; a native release stage also
requires the adjacent `spice.pdb`. It hands the staged archive's COFF object
only to that final link through `cargo rustc`; the plain Cargo command remains
the fast source and linker integration check. The object's name carries the
archive digest, so a new archive relinks the executable and rebuilds nothing
else. Final-image admission locates the section through Mach-O, ELF, or PE
metadata and refuses absent, repeated, truncated, or wrong bytes. Windows
release policy also refuses an embedded Authenticode certificate: the final
public GitHub Sigstore proof is the detached signature after all executable
bytes are fixed. No persisted build-time offset participates.
[`spice/src/version.rs`](../../spice/src/version.rs) admits the linked archive
once. An installed resident without one refuses with
`serve::canvas-archive-absent`, or `serve::canvas-archive-invalid` when it does
not admit. The installed executable is the only app carrier. Install publishes
no app file and binds the generation G that its linked archive names. The same
receipt and archive check admits it to `xtask report`.

The release stage is:

```sh
spice dev xtask -- stage-target x86_64-pc-windows-msvc
```

It packages `bin/spice.exe`, `symbols/spice.pdb`, the target payload, and the
detached proof receipt. While `infimalabs/spice-rust` is private, Windows proof
refuses at the typed public hosted-attestation prerequisite before dispatch;
the Windows 11 ARM VM, Wine, and the macOS cross-build are development evidence
only. Making the repository public is the wake condition for
`DIST-dWLh7gXP1`; no Enterprise-plan change or self-hosted signing key is a
substitute.
Owners: [`canvas/src/lib.rs`](../../canvas/src/lib.rs) for the symbols,
[`dist/src/canvas.rs`](../../dist/src/canvas.rs) and
[`dist/src/canvas/coff.rs`](../../dist/src/canvas/coff.rs) for object writing
and final-image section admission, [`xtask/src/app.rs`](../../xtask/src/app.rs)
for the link arguments, and
[`spice/src/install/canvas.rs`](../../spice/src/install/canvas.rs) for install
admission.

The resident serves app routes through one boundary, `CanvasAssets`, with
exactly two sources. Only a staged `spice up` serves `DevelopmentDirectory`:
the staged generation directories, hot-swapped as generations publish, behind
a two-entry generation cache. Every installed resident serves
`EmbeddedArchive`: the archive linked into its executable, admitted once at
startup into an index from logical path to the stored gzip slice, the
inventory MIME type, and a strong ETag naming generation and path. It reads no
app file from disk. Only HTML members, the shell and renderer documents, are
inflated, once, to derive their content security policies. Every embedded body
is the stored slice, sent with `Content-Encoding: gzip`, its exact
`Content-Length`, and `Vary: Accept-Encoding`. A request whose
`Accept-Encoding` refuses gzip gets 406. An installed resident holds one
generation for its lifetime and embeds no predecessor. A route qualified with a
generation the source does not hold answers 410 Gone, with
`x-spice-app-generation` naming the served generation. Owners:
[`serve/src/canvas/assets.rs`](../../serve/src/canvas/assets.rs) and
[`serve/src/axum.rs`](../../serve/src/axum.rs).

A resident restart keeps the app's last good surface on screen until the new
session's scope is fully hydrated. Every app route answers `no-cache` with a
content ETag, so an ordinary reload loads the served generation. A page whose
generation the resident no longer serves reloads once into the served
generation, whether or not the app protocol changed. That happens when its next
connection's bootstrap names another generation, before any session opens, or
when a qualified fetch answers 410:

- the shell's module or wasm before the app starts, caught by the shell's
  inline loader guard;
- a renderer document or member, probed when the renderer frame opens and
  after any renderer failure.

A staged `spice up` hot swap reaches an open page only at its next connection.
Unsent composer and observation drafts travel to the reloaded page through
tab-scoped storage, with attachment annotations flattened into their image.
Only a submission whose result is still unknown keeps the stale page in place
behind the update offer and its reload control; an incompatible app protocol
then still requires that explicit update. Owners:
[`app/index.html`](../../app/index.html),
[`app/src/runtime/publication.rs`](../../app/src/runtime/publication.rs),
[`app/src/browser/connection.rs`](../../app/src/browser/connection.rs),
[`app/src/browser/mermaid/service.rs`](../../app/src/browser/mermaid/service.rs),
and [`app/src/browser/draft_handoff.rs`](../../app/src/browser/draft_handoff.rs).

One inventory of unsent work arms the page's leave guard: every handed-off
composer and observation draft (live text included, mounted or not), the
submissions the resident has not yet accepted, and annotation marks not yet
applied to an image. An empty inventory leaves the guard disarmed. The
browser's leave dialog carries no page text, so a leave the operator cancels
lists each item under the command bar, naming its kind and owner: the lane and
its team, a Design's handle, a task's handle, or an image. **Show** reveals and
focuses a draft's composer; a draft whose lane no console shows says so, and
**Discard** removes any draft after a confirm naming its owner. Owners:
[`app/src/unsent_work.rs`](../../app/src/unsent_work.rs) and
[`app/src/browser/unsent.rs`](../../app/src/browser/unsent.rs).

## Version identity

`spice --version`, `spice version`, the app menu, and the Spice header report
one product version that every build path derives the same way from the tree it
builds. `dist/VERSION` holds only the major number, the one number a person sets.
The minor is HEAD's committer second, and nothing past HEAD is read, so a
depth-1 checkout derives it. Spice task landings date themselves at least one
second past main's previous head, so main's first-parent seconds strictly
increase and a later landing sorts above an earlier one. A commit made outside
a landing in its parent's second shares its parent's version, as do past
commits that predate this rule. The patch is 0. On a patch line,
`dist/VERSION` instead holds a full `MAJOR.MINOR.PATCH` pin, which is the
version verbatim, because a patch commit's own instant would move the minor.

A build that does not pass through release staging
(`spice dev xtask -- install`, `spice dev xtask -- windows-appliance`) appends
`-dev`, so it sorts below its own commit's staged `stage-target` payload, which
carries no marker. A plain Cargo build bound to no commit reports
`MAJOR.0.0-dev`, below every derived version of its major. The version never
carries the executable digest: the native-binary identity is the running
executable's complete digest, so identical bytes keep one identity without a
Git checkout. `spice --version` ends with `app G` when the executable links
generation G's archive.

Owners: [`dist/src/version.rs`](../../dist/src/version.rs),
[`spice/src/version.rs`](../../spice/src/version.rs), and
[`spice/src/version/source.rs`](../../spice/src/version/source.rs).
