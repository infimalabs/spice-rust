# Product model - supporting planes - Gate evidence

[Product model - supporting planes](../planes.md) | [Spice product model](../README.md)

This document is the implementation contract for reusable gate work. It does
not change the gate's decision rules. It separates proof of one immutable
action from admission of one prospective commit so development work can be
credited without executing tests inside a Git hook.

## Identities

Two identities have different jobs and must not be substituted for each other.

- `candidate_tree` is Git's tree object ID from `git write-tree` over the exact
  index observed by pre-commit. It binds an admission to the complete
  prospective commit, but never keys reusable action proof.
- `action_key` identifies one action and only the inputs that can change that
  action's result. Equal keys permit reuse even when unrelated paths make the
  enclosing candidate trees different.

Git supplies the immutable candidate, staged inventory, modes, object types,
object IDs, and blob bytes. For each action, Spice filters that inventory to the
declared input closure and computes its own content digest over normalized path,
mode, presence state, and blob content. Git object IDs are retained as capture
provenance but do not replace that digest. Unrelated candidate-tree entries do
not enter the action's input manifest or action key.

Every Spice digest below is computed over canonical JSON bytes with the named
domain separator. Decoders admit only the current version, reject unknown
fields, validate sorted unique collections, and reject a second encoding of the
same value.

| Value | Domain separator | Canonical fields |
| --- | --- | --- |
| `ActionSpec` digest | `spice.gate.action-spec.v1` | `version`, stable `action_id`, runner kind and version, declaration digest, exact `argv`, repository-relative `cwd`, ordered relevant environment names, ordered tool requirements, input-closure strategy, baseline class, execution bounds |
| `InputManifest` digest | `spice.gate.input-manifest.v1` | `version`, ordered entries of normalized path, Git mode, blob digest, and presence state; ordered non-file inputs with kind and digest |
| `action_key` | `spice.gate.action-key.v1` | `version = 2`, `ActionSpec` digest, `InputManifest` digest, typed policy authority (`policy-consuming` or `resolved-execution`), compiled gate-set digest, ordered observed tool identities, ordered relevant environment values, and baseline identity when the action is ancestry-sensitive |
| `PassReceipt` digest | `spice.gate.pass-receipt.v1` | `version`, `action_key`, spec and manifest digests, typed result digest, `passed`, start and finish timestamps, duration, ordered artifact digests, producer record |
| `AdmissionBundle` digest | `spice.gate.admission.v1` | `version`, `candidate_tree`, branch ref, pre-commit `HEAD`, selection digest, policy and gate-set digests, ordered owed actions with action key and receipt digest, ordered live-check results, claim-capture digest, creation and expiry timestamps |
| `transaction_id` | `spice.gate.reference-transaction.v1` | `version`, Git common-directory identity, transaction state-independent normalized ref updates, and the selected admission digest |

An input-manifest entry represents a present path as `(path, mode, blob)` and a
removed path as `(path, deleted)`. Paths use forward slashes, contain no `.` or
`..` component, and sort by raw UTF-8 bytes. Duplicate paths, unsupported Git
modes, empty identities, unordered arrays, and an absent required baseline
refuse key construction.

`action_id` is a stable namespace such as `gate:rust-typecheck` or
`cargo:typecheck:app::lib:app`; display labels never enter it. `argv` contains no
shell text. The relevant environment map is closed by the spec: an undeclared
environment dependency makes the action non-reusable until its declaration is
repaired. Tool identities use the existing `ReceiptTool` name and executable
digest model. Policy-consuming focused actions retain the complete compiled
policy and resolved configuration as typed input-manifest entries. Cargo uses
`resolved-execution` only after its complete shard inventory, exact source
closure, command, tools, environment, gate set,
external-source authority, and build dependencies are bound; unrelated
selection-only policy does not enter those action keys. Compiled declarations,
process bounds, and baseline state reuse the existing `Verdict` digests rather
than introducing a second interpretation of those authorities.

Action-key document version 2 is the sole current derivation. Version 1
included a second top-level effective-policy digest. Its receipts remain stored
for audit but cannot match a current key, and readers perform no legacy fallback
lookup.

Cacheable Cargo and Canvas actions contain no branch, commit, merge-base, or
mainline identity. A receipt first produced on a divergent branch that is never
merged remains reusable by another branch or linked worktree when every action
input is equal. Repository history is an input only for rows classified as
ancestry-sensitive below.

`Verdict` remains the complete report for one gate run. A `PassReceipt` is an
indexable envelope around the canonical typed result for one focused action;
it does not replace or reinterpret `Verdict`. A whole-run report may cite the
receipt digests used to assemble it.

## Reuse and attribution

Reusable evidence requires exact `action_key` equality, canonical receipt
bytes, `passed = true`, a trusted proof-runner identity, an admitted schema
version, and a receipt object whose digest matches its content. No subset or
field-by-field best effort match is allowed.

The producer record contains lane, session, task, actor, and appliance
generation when available. Timestamps, duration, artifacts, producer identity,
and the candidate tree on which a proof was first produced are audit and credit
metadata. They affect the receipt object's digest but never the `action_key`.
This lets identical work receive credit without making the producer part of the
technical result.

Trust standing is likewise authorization state, not proof identity. A
configured pre-commit checks the current grant before executing repository
commands, while reusable action keys remain bound to the resolved execution
authorities they prove. Policy-consuming focused actions bind complete policy
through their input manifests. Pended, granted, and revoked
transitions do not change those content identities. The agent-vendor variable
(`SPICE_AGENT_VENDOR`), shell-hook variables,
and Spice command-routing executables are producer or invocation context and
are excluded from compiler environments. Arbitrary build inputs and native
compiler/tool paths remain retained.

Only the proof-aware runner may publish `PassReceipt` objects and action-index
entries. A shell transcript, Canvas text marker, arbitrary `cargo test` output,
exit status copied from another process, or a hand-written receipt is not proof
because it does not establish the immutable source, declaration, command,
environment, tool, and result as one execution.

Canvas compilation remains a native gate action. The process-backed browser
suite and its build/shard proof actions are removed; there is no browser action
registry or legacy marker admission path. Disposable browser experiments can
validate the actual UI, but cannot publish trusted commit receipts. Such an
experiment runs under the host OS light/dark scheme unless it states the
override it is testing, so what the agent looks at is what the operator sees.

## Storage and lifecycle

Evidence lives under the canonical Git common directory:

```text
.spice-rs/gate-evidence/v1/
  objects/<receipt-digest>
  actions/<action-key>
  leases/<action-key>
  publication.json
  admissions/<candidate-tree>/<admission-digest>
  pending/<transaction-id>
  credit/<commit-oid>
  fingerprints/<authority-key>
  fingerprint-objects/<manifest-digest>
```

Objects are immutable and published by write, file sync, atomic rename, and
directory sync. An action entry is a replaceable pointer to a validated object.
Aggregate proof publication records every prior action pointer and newly created
object in `publication.json` before exposing any of them. Every store reader
holds the common store lock and rolls back a prepared journal before resolving
an action, so a failed or interrupted aggregate publishes either every receipt
and action pointer or none of them.
Leases coordinate duplicate producers but grant no proof and expire by an
explicit recorded deadline. Readers preserve corrupt or unsupported bytes for
inspection and refuse them.

Fingerprint objects remain immutable, but their authority index is a replaceable
cache pointer. Fresh planning validates the previous indexed object and publishes
the new object before atomically replacing that pointer under the store lock.
An interrupted replacement leaves the previous plan selected; corrupt indexed
evidence still refuses. Replacing a plan grants no proof: the newly selected
action keys must resolve their own exact receipts.

Fast lookup also binds the compiled proof runner version and Cargo runner,
input-strategy, and test-shard declarations. Installing a changed proof contract
therefore selects a new fingerprint namespace even when the staged tree and
external compiler tools did not change.

Pre-commit captures one index authority, computes `candidate_tree`, resolves
the owed action set, verifies all receipts in one read phase, runs live-only
checks, revalidates the index authority, and atomically publishes one admission
bundle. Bare `spice gate prove` reads that same current fingerprint and its exact
receipt indexes without restoring selection or preparing an action. Neither path
executes a missing proof: a miss names each action and the explicit
`spice gate prove ACTION` command plus one `spice gate prove --missing`
aggregate command. Staged content is the unconditional source for inspection
and generation; `spice gate prove ACTION --missing` reuses an already-current
receipt.

Proof progress on stderr is best-effort diagnostic output: one line when an
action starts, an elapsed heartbeat every 30 seconds while it runs, and one
line with its elapsed time when it proves or fails. Cargo, compiler, and test
output never streams. Each executed action's complete subprocess output is
retained as `.spice-rs/gate-output/DIGEST.log` under the Git common directory,
named by the content digest of its bytes; the newest 64 captures are kept. A
failed action's refusal carries a bounded tail of diagnostic lines (Cargo's
machine records dropped), the count of withheld bytes, and that capture's path
and size.

The text reply is decision-first: the action count for the candidate tree,
how many this run proved, one row per proved action with its elapsed time, and
the exact next command. `--verbose` adds rows for already-proven actions with
their action keys, receipts, and capture paths. `--json` carries the typed
report unchanged; none of these text projections enter it.

Bare `spice gate`, `spice gate selection`, and `spice gate pre-commit` share
that policy. Bare gate names each family as it starts, beats with the arm under
analysis, and replies with arm counts, one row per refusing arm (every arm
under `--verbose`), the retained complete report, and the next command. Direct
selection prints phase, arm, and test-shard boundaries and replies with the
policy arms, test-target count, retained output, and `git commit`. Under the
pre-commit worker selection forwards only the protocol lines its supervisor
parses; subprocess chatter stays in the capture. Pre-commit beats with the
worker's last phase only after one heartbeat interval and prints no success
reply or completion line. Every refusal or fault detail beyond the tail bound
is replaced by its tail, withheld counts, and the retained capture path.
Progress never accompanies `--json`, whose stderr carries one document.

A failed progress write or flush does not stop later actions, lose their
reports, or prevent receipt publication. Execution, lease, source-validation,
cleanup, and final result failures remain authoritative and keep their
ordinary error semantics. Losing stdout can still prevent delivery of the
final result; it is distinct from losing diagnostic progress. An enclosing
shell pipeline can mask the producer's exit status, so callers must preserve
that status rather than infer success from a downstream filter or the last
progress line.

A candidate-tree directory may contain more than one immutable admission. This
is required when two branches or linked worktrees produce equal tree content
from different parent commits. Prepared selects the unexpired bundle whose
branch, prior tip, candidate tree, policy, and gate-set authorities all match;
zero or multiple matches refuse instead of choosing by recency.

`reference-transaction prepared` first checks history protection. An unchanged
branch tip or a commit already reachable from a local branch, remote-tracking
ref, or tag does not need another admission: moving a ref within existing history
does not author a prospective commit. Reachability compares commit identity,
not tree equality, and uses the refs held before the transaction is committed.
Admissions issued for another branch or an earlier tip cannot obstruct these
updates. Rewinds that abandon published history still refuse.

For newly introduced commits, prepared resolves each queued commit's tree and
checks the matching admission bundle before writing an idempotent pending record.
`committed` promotes that record to commit credit. `aborted` removes it.
`preparing` does no repository or evidence work. No reference-transaction state
executes a test or waits for a proof producer.

## Action classes

`cacheable` means staged content and declared non-file inputs determine the
result. `ancestry-sensitive` is also reusable, but only with the exact parent,
merge-parent, rename, or committed-baseline identities in the action key.
`live-only` must run during pre-commit because its decision reads mutable state
that a prior proof must not stand in for.

Cargo test proof separates preparation from execution, as the full verifier does.
The exact selected Cargo targets and features first compile with `--no-run`;
only a successful prebuild starts the unchanged test command. Each phase retains
its own 180-second supervision bound, so whatever the selected tree still has to
compile cannot consume the test execution budget. A staged selection is refilled
into the same private workspace, leaving unchanged files untouched, so a proof
compiles what changed rather than the whole local graph. Both phases must
succeed before a receipt is published. The test action declaration binds this
two-phase contract; source admission, output bounds, and cleanup remain required.

Removing Rust fixture data that no current or base-tree target compiled selects
the containing package's checks and tests, not reverse-dependent suites. This
fallback applies only after ordinary target ownership finds no claim and the
path is confirmed absent. Present unowned sources and other filesystem errors
remain refusals; compiled-source and package-authority changes retain their
ordinary dependency reach.

Workspace packages that execute a build script or proc macro opt into reusable
Cargo proof with `[package.metadata.spice.cargo-proof]`, `version = 1`, and
`complete = true`. That flag is the package owner's closed-world assertion that
the listed authority covers every ambient read made by its executable compile-time
code; Cargo rerun output alone is not such an assertion. Optional `files` are
package-relative selected-tree paths and optional `environment` entries are exact
ambient names. Literal
`cargo::rerun-if-changed` and `cargo::rerun-if-env-changed` output extends those
sets. The planner refuses the shard as non-cacheable when the declaration is
absent or incomplete, a path escapes the selected tree, an observable read is
undeclared, or executable code names process, network, or dynamic authority.

Selection asks which Cargo targets a changed Rust path reaches. Target roots
and the declared `#[path]` and `include!` reach answer that for source, and a
path they do not claim is refused, because an unclaimed path is also what a
module the scanner failed to follow looks like. Rust text that no target
compiles -- parser input, golden samples, audit corpora a test opens at run
time -- is declared instead, with `[package.metadata.spice]` and `data`
holding package-relative directories. A change under a declared directory
selects that package's own test-enabled targets and nothing downstream, since
no compile contract reaches bytes no compiler read; the declaration is refused
when it covers source a target compiles. A path gone from the selected tree
that no target claims in the current or base inventory is treated the same way
without a declaration, because nothing it could have been compiled by remains.

| Action | Class | Required authority |
| --- | --- | --- |
| `merge-integrity` | ancestry-sensitive | candidate tree, `HEAD`, `MERGE_HEAD`, merge-tree tool identity |
| `plan-phase` | live-only | current board claim and workflow phase |
| `repo-shape` | cacheable | reached repository tree, policy, compiled declaration |
| `staging` | live-only | exact staged and unstaged path comparison at admission time |
| `repo-docs` | ancestry-sensitive | document bytes, committed document baseline, bound ledger, actor and peer-claim state |
| `config-key-validity` | cacheable | tracked configuration and compiled key declarations |
| `config-false-disable` | cacheable | tracked configuration and compiled gate declarations |
| `config-tracked-trust` | cacheable | tracked configuration and trust-field declarations |
| `formatters` | cacheable | reached source, formatter declarations and tool identities |
| `local-paths` | cacheable | reached paths and local-path policy |
| `taste` | cacheable | reached text, taste policy and parser declaration |
| `rust-typecheck` | cacheable | Cargo closure, manifest and lockfile, closed build-script/proc-macro file, environment, and generated-output derivations, Rust toolchain, exact command and environment |
| `canvas-web-typecheck` | cacheable | Canvas TypeScript closure, package lock, Node tool identity, exact command and environment |
| `javascript-unused` | cacheable | JavaScript dependency closure, parser and policy |
| `csharp-unused-candidates` | cacheable | C# dependency closure, parser and policy |
| `python-typecheck` | cacheable | Python dependency closure, interpreter and policy |
| `env-policy` | cacheable | reached source, environment declarations and policy |
| `env-name-ledger` | cacheable | complete dependency-expanded source closure and declared environment ledger |
| `file-shape` | ancestry-sensitive | reached source, merge-parent authorship baseline, bound ledger, actor and peer-claim state |
| `complexity` | ancestry-sensitive | reached source, merge-parent authorship baseline, bound ledger, actor and peer-claim state |
| `magic-numbers` | ancestry-sensitive | current source and exact committed text baselines |
| `markdown-links` | cacheable | complete tracked-path inventory plus reached Markdown and Rust documentation text |
| `reachability` | cacheable | complete language dependency closure, tests, parser and policy |
| `symbol-reachability` | cacheable | complete symbol dependency closure, tests, parser and policy |
| `python-unused` | cacheable | complete Python dependency closure, parser and policy |
| `assertion-free-tests` | cacheable | reached test source, parser and policy |
| `private-internals` | cacheable | complete Python dependency closure, parser and policy |
| configured `Spice source gate` | decomposed | It mints no reusable wrapper receipt; its selected packaged actions use the rows above until the configured wrapper is removed. |

For ancestry-sensitive metric rows, actor and ledger state are decision inputs,
not producer attribution. They therefore enter the manifest as typed non-file
inputs. An implementation that cannot prove a complete dependency closure or
name every mutable decision input marks the action live-only; it never emits an
under-keyed reusable receipt.
