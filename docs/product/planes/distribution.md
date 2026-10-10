# Product model — supporting planes — Distribution

[Product model — supporting planes](../planes.md) · [Spice product model](../README.md)
## Distribution

Distribution crosses authorities the appliance does not own outright: one
tracked Git release authority and an operator's repository. Immutable release
refs cannot be repaired by pretending an effect did not happen, and installation
must preserve state the operator already owns. Exact plans, durable receipts,
convergence, and identity-aware reversal answer those boundaries.

### External publication is a plan, not a verb

Every release verb with external effects renders its complete ordered plan and
performs none of them until `--apply=<digest>` names that plan. An absent or
moved digest refuses before effects; exact-plan execution is the only
publication path. Inspection (checking the gates, drafting notes, previewing the
range) never bumps, commits, tags, pushes, or publishes.

Plan identity covers the repository, action, version, source and release
commits, target matrix, ordered operations, notes, artifacts, proof receipts,
sealed manifest, and captured preconditions. Order is part of the plan because
the order is the safety property. The correctness operation always means the
complete canonical strict battery, never a subset chosen outside the plan.
Preconditions that can be checked without effect are checked while the plan is
built, so nothing knowable up front fails after a push.

Release documents are complete only as files. `release notes` prints bounded
metadata and an entry page whose omission names `release notes --output FILE`.
A publish preview first retains the complete plan under the Git common
directory, then prints every authorizing field, names that file, and ends with
the exact `--apply` command.

One executor owns the operation vocabulary for preview, execution, and
recovery. It records an operation's exact prior state before crossing its effect
boundary and a typed receipt after convergence; resumption admits only the same
plan and receipt prefix. A parallel verification or publication body would be a
second release authority.

### A versioned commit is sealed before publication

Version activation is an ordinary task landing. Release requires clean `HEAD` to
be that integrated commit with requested, compiled, and tracked product versions
already in agreement; every other shape refuses. Release never edits
`dist/VERSION`, synthesizes a commit, or infers task ownership from the board.

A fresh publish runs the correctness battery over the exact source commit the
plan names, then materializes the package from the sealed content-addressed
objects, which must equal the previewed package and tree identities. Nothing is
rebuilt after authorization. An independent probe recomputes the compiled-host
executable's digest against the plan, copies it to a plan-keyed root, makes it
executable, and requires `--json version` to succeed there.

`release publish --action tag-and-announce` finishes a release whose package is
already visible: it resolves preconditions and credentials, awaits package
visibility, then tags and announces, without rerunning the battery or rebuilding
the package.

### Proof is sealed per target and composed last

A release has exactly two target payloads, in canonical order:
`aarch64-apple-darwin` and `x86_64-unknown-linux-musl`. Each payload binds its
target, source identity, app generation, native executable, build pins, and
artifact inventory, and carries the source digest of the versioned release
commit. A detached receipt binds the environment that ran proof: the Linux
receipt's OCI identity matches the payload's proof-container pin, and the Darwin
receipt binds the attested native runner.

Proof replays the complete current battery against the current candidate
checkout. Replay is bound to payload, environment, and battery, but does not by
itself prove the workspace commit equals the payload's source digest; the plan's
source binding does. Only a clean replay publishes a receipt, and admission
requires target, payload, environment, battery, replay, and clean standing to
form one identity chain.

The manifest is post-proof. Both targets with clean, mutually consistent
receipts must exist before the sole manifest pointer is published, and every
object is published before that pointer, so a reader sees the prior complete
seal or the new one, never partial or mixed-target proof.

### Notes are read out of the ledger

Notes derive from the ledger; no changelog is maintained. The window runs from
the highest-versioned prior tag reachable from the release commit, addressed by
its fully qualified ref so a same-named branch cannot redefine the range, to the
release commit. With no reachable tag, a bounded window of recent landings names
that basis.

- Only first-parent landings count, so one merged unit of work is one entry.
- Work added and reverted inside the window is suppressed on both sides. A
  revert pairs with the landing whose history contains the reverted commit,
  which usually arrived on a side branch.
- Landings sharing a task collapse into one entry at the task's first landing;
  the latest landing supplies its subject and project.
- Entries group by that declared project, never by touched paths. An empty
  project and every `agent.*` project map to `general`. Buckets appear in
  first-seen order and keep entry order; equal descriptions merge only within a
  bucket, keeping every reference. Every entry references history.
- A trailing handle is stripped only when it matches the commit's own recorded
  identifier.

Curation owns the region above the generated inventory. Everything from the
inventory down is regenerated at publication against the real release commit,
because curation happens before that commit exists. Notes that are untouched,
still carry the placeholder, or hold only banner and headings refuse.

### The publication sequence

The tracked release authority names one remote and its refs: remote `origin`,
source `refs/heads/main`, package `refs/spice/packages/vVERSION`, tag
`refs/tags/vVERSION`, and announcement `refs/spice/releases/vVERSION`. Runtime
configuration cannot redirect a step to another provider.

The release commit is already integrated on the source branch; publication
neither reads nor moves it. Publication converges the package ref after a
no-effect rehearsal, then waits until bounded observation sees that exact
package object before converging the tag and announcement to the release
commit. Exhausting the observation refuses loudly rather than advertising
content nobody can fetch.

A version names exactly one package object and one release commit. A ref already
pointing elsewhere refuses; a matching ref is already converged and left alone.
Every step converges, so an interrupted release is finished by rerunning the same
plan from its first incomplete boundary, not unwound. An operator interruption
ends with a status distinct from a gate failure.

### Installing into a repository the product does not own

Installing hooks writes into someone else's repository, so one canonical
status-bearing receipt owns the exact plan, the first-captured prior images,
durable progress, and reversal outcome.

`install` previews a plan produced purely by observation; `install --apply` with
that plan's digest re-resolves and refuses before effect if the digest or any
bound observation moved. A process that derives and applies a plan itself, such
as the development refresh, may carry the prepared plan into apply, but still
proves before any mutation that the receipt generation, source bytes, app
generation, and destination bindings are exactly those the plan was derived
from.

Before the first receipt-owned effect, apply atomically accepts an `applying`
receipt holding the plan, repository identity, zero progress, and the prior
images. Each operation is described by the accepted receipt before it runs, and
its completion is published by compare-and-swap from the exact prior generation
before the next begins; a stale or concurrent writer refuses. Terminal
`complete` is published only after every stage completes, and an interrupted
apply resumes only the same plan.

The receipt records the exact prior image: regular-file bytes, ordinary
permission bits, or absence; prior configuration values or absence; and whether
installation created a parent directory. Prior state is captured once, at first
application, and never re-derived from state the installation produced, or a
later reversal would restore the installer's own output as the operator's.

Generated artifacts live in a directory the product owns and are activated by
redirecting the host tool there, never by writing into a location the repository
already uses. Each carries an ownership digest, distinguishing an untouched
installation from a locally modified one and from a newer generated shape.
Installed app history keeps a bounded set of predecessor generations; refresh
retires the rest in the same transaction, discarding a file only while it still
holds the installed image.

Apply and `--unapply` may each run one bounded pass, after admission, that
reclaims only recognized stale install publications. Preview never runs it, and
it never imports, repairs, or deletes foreign state.

### Reversal is a claim about identity

An uninstall that reverts by name destroys operator edits. `install --unapply`
advances the exact `complete` receipt to `unapplying` by compare-and-swap and
runs the recorded operations in exact inverse order, restoring each prior image
only while the target is still exactly what was applied. Application order is
canonical — a capability is enabled before use and files are written before
configuration points at them — so inversion never leaves configuration pointing
at removed files.

Identity drift refuses with nothing overwritten, preserves the changed target
and the current receipt, and publishes no terminal status; reconciliation is a
separate operation, and a retry resumes the same receipt. An installation-created
container is removed only when it is empty. Terminal `uninstalled` is published
once every operation has a recorded outcome.

An interrupted reversal blocks a new installation until it is resumed, so two
plans never contend for one receipt or blur the first-captured prior state. Both
directions are resumable and idempotent: a rerun performs only operations not
yet recorded complete, and a plan already satisfied performs no receipt-owned
effect.

### The canonical receipt

The receipt is the sole installation-status, transaction, and reversal
authority. Every generation is canonical and published atomically through a real
non-symbolic file, only while the exact prior generation still matches.

An unreadable, truncated, noncanonical, linked, or internally inconsistent
receipt refuses. Reading damaged provenance as "not installed" would strand
active modifications with no owner and no path to removal. A receipt is bound to
its repository: a nonterminal receipt naming another Git-common owner refuses,
and a terminal one may rebase only once its old owner no longer exists, moving
repository, plan, and activation bindings together.

The receipt remains after clean terminal directions: `complete` keeps the plan
and the original prior images, and `uninstalled` keeps its status and progress.
Absence is never synthesized by deleting provenance.

Install support is bounded by receipt schema, not product version. Only the
current schema executes. A retired schema refuses before receipt-owned mutation
and names the release change whose executable can unapply it, and retiring a
schema adds that owner in the same landing. Malformed and future receipts are
store failures, never mistaken for retired state.
