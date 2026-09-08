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
| `action_key` | `spice.gate.action-key.v1` | `version`, `ActionSpec` digest, `InputManifest` digest, effective policy digest, compiled gate-set digest, ordered observed tool identities, ordered relevant environment values, and baseline identity when the action is ancestry-sensitive |
| `PassReceipt` digest | `spice.gate.pass-receipt.v1` | `version`, `action_key`, spec and manifest digests, typed result digest, `passed`, start and finish timestamps, duration, ordered artifact digests, producer record |
| `AdmissionBundle` digest | `spice.gate.admission.v1` | `version`, `candidate_tree`, branch ref, pre-commit `HEAD`, selection digest, policy and gate-set digests, ordered owed actions with action key and receipt digest, ordered live-check results, claim-capture digest, creation and expiry timestamps |
| `transaction_id` | `spice.gate.reference-transaction.v1` | `version`, Git common-directory identity, transaction state-independent normalized ref updates, and the selected admission digest |

An input-manifest entry represents a present path as `(path, mode, blob)` and a
removed path as `(path, deleted)`. Paths use forward slashes, contain no `.` or
`..` component, and sort by raw UTF-8 bytes. Duplicate paths, unsupported Git
modes, empty identities, unordered arrays, and an absent required baseline
refuse key construction.

`action_id` is a stable namespace such as `gate:rust-typecheck` or
`canvas:shard:interaction`; display labels never enter it. `argv` contains no
shell text. The relevant environment map is closed by the spec: an undeclared
environment dependency makes the action non-reusable until its declaration is
repaired. Tool identities use the existing `ReceiptTool` name and executable
digest model. Effective policy, compiled declarations, process bounds, and
baseline state reuse the existing `Verdict` digests rather than introducing a
second interpretation of those authorities.

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
commands, while reusable action keys remain bound to the executable
configuration and policy content they prove. Pended, granted, and revoked
transitions do not change those content identities. Agent-runtime variables
(`CODEX_*`, `CLAUDE_CODE_*`, and `SPICE_AGENT_VENDOR`), shell-hook variables,
and Spice command-routing executables are producer or invocation context and
are excluded from compiler environments. Arbitrary build inputs and native
compiler/tool paths remain retained.

Only the proof-aware runner may publish `PassReceipt` objects and action-index
entries. A shell transcript, Canvas text marker, arbitrary `cargo test` output,
exit status copied from another process, or a hand-written receipt is not proof
because it does not establish the immutable source, declaration, command,
environment, tool, and result as one execution.

Release 0.31.0's `BROWSER-1kKSWhZk browser coverage complete` marker and its
`--spice-coverage-receipt` writer are retired together. Current Canvas
completion is the registry-derived set containing `canvas:build` and exactly
one `canvas:shard:<name>` receipt for every current shard. Supplying the retired
writer flag is a refusal naming release 0.31.0; marker bytes are never executed,
translated, or admitted as a second completion path.

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
| Canvas build | cacheable | admitted selected-tree source closure, parsed local build-launcher graph, Cargo build and file-macro authority, app source derivation, package lock, Node and compiler identities, exact build command and environment |
| each Canvas browser shard | cacheable | build receipt, shard registry entry, scenario closure, browser and Node identities, exact command and environment |

For ancestry-sensitive metric rows, actor and ledger state are decision inputs,
not producer attribution. They therefore enter the manifest as typed non-file
inputs. An implementation that cannot prove a complete dependency closure or
name every mutable decision input marks the action live-only; it never emits an
under-keyed reusable receipt.
