# Checks

The checks that can refuse a commit. Spice runs without them, with fewer
protections; once installed, they apply to every commit. The [checks
register](invariants/checks.md) owns the gate's labelled invariants; this
document keeps only what it does not state.

## Selection

The commit gate captures the staged source once, maps every changed path to
the arms and Cargo targets its compiled classification owns, and runs only that
selection. A path whose classification is a selection authority (packaged
policy, or repository build or test selection) selects every applicable arm at
exhaustive reach. A path the classification cannot prove local — an
unclassified path still present in a configured repository, or Rust source or
a package manifest outside every compiled workspace root — refuses with
`change-selection-refused` before selected work begins; uncertainty never
impersonates exhaustive reach. An unclassified path the staged tree no longer
holds is a removal: its bytes add nothing to measure, so it owes only the local
path arm and, in a Rust workspace, the compile arms that prove no surviving
source still includes it.

The battery manifest's canonical order owns report order. Independent arms may
run concurrently; scheduling never changes evidence identity or order. A
compiler failure without parsed findings keeps its bounded captured output in
the refusal reason rather than inventing findings.

At run start the complete staged rename map transfers every prior latch and
live claim, and an ambiguous rename destination refuses.

## Limits

A differential measure fails only on regression: a repository may be
imperfect, never worse. A binary integrity rule or an explicit zero ceiling
stays zero; no policy value turns it into an allowance.

A blocked commit is fixed in the source or by a declared waiver, never by a
quietly lowered limit. A legitimate exception takes one of four tracked forms:
a marker at the offending site (such as `env-policy: allow`), a named ledger
entry, a [scoped rule](invariants/checks.md#checks-a-scoped-rule-retunes-a-bound-over-a-declared-path-set),
or a [declared step disablement](invariants/checks.md#checks-a-step-may-be-disabled-by-configuration).
Each names the specific thing it permits — a count is never a waiver, because
permitting three of these permits the wrong three — and lives in tracked
source, so every clone sees the same exceptions.

**Shape and policy.** Shape is what kinds of thing are enforceable: staged
selection, one source identity, typed evidence, transactional pressure state,
waiver forms, and isolated selected source. Policy is where each line sits:
thresholds, headroom, variation, claim lifetime, admitted paths, ratchets, and
waivers. If changing something makes a different kind of thing enforceable, it
is shape; if it only moves the line for the same kind of thing, it is policy.
Policy tunes a compiled mechanism; it cannot add an executable arm or weaken
the shape that admits its evidence.

## Isolation

A staged workspace is refilled rather than rebuilt. A later selection writes
only paths whose bytes differ, removes paths it does not select, and leaves
unchanged files untouched, because Cargo judges freshness by modification time
and rewriting identical bytes would recompile an unchanged tree. The seal and
full object-identity verification run over a refilled tree exactly as over a
fresh one.
