# Case battery — Release

[Case battery](../cases.md) · [Spice product model](../README.md)

## Cases
### Release as a plan

| Situation | Expected | Ref |
| --- | --- | --- |
| An external publication verb is invoked without execution authorization | the complete ordered plan renders; nothing is committed, tagged, pushed, or published | [External publication renders its complete ordered plan](../invariants/distribution.md#distribution-external-publication-renders-its-complete-ordered-plan) |
| A plan is read, an input moves, then execution is requested with the prior digest | refused, naming the fresh expected plan identity and the supplied prior identity | [An execution request pins the digest of the plan that was read](../invariants/distribution.md#distribution-an-execution-request-pins-the-digest-of-the-plan-that-was-read) |
| The notes file is edited between preview and execution | the pinned digest no longer matches; refused | [Plan identity covers every input to publication](../invariants/distribution.md#distribution-plan-identity-covers-every-input-to-publication) |
| The target commit is moved between preview and execution | the pinned digest no longer matches; refused | [Plan identity covers every input to publication](../invariants/distribution.md#distribution-plan-identity-covers-every-input-to-publication) |
| A plan is rendered | the publication steps appear in the order they will run | [Step order is part of what the plan commits to](../invariants/distribution.md#distribution-step-order-is-part-of-what-the-plan-commits-to) |
| The named notes file does not exist | refused while the plan is built, before any gate runs | [Preconditions that can be evaluated without effect are evaluated](../invariants/distribution.md#distribution-preconditions-that-can-be-evaluated-without-effect-are-evaluated) |
| Publish starts at the clean source commit carrying the source version | admitted as fresh local release progress | [Local release progress admits only three exact shapes](../invariants/distribution.md#distribution-local-release-progress-admits-only-three-exact-shapes) |
| Publish resumes at the source commit with only `dist/VERSION` changed exactly to the requested version | admitted as the sole version-only intermediate; any other tracked change refuses | [Local release progress admits only three exact shapes](../invariants/distribution.md#distribution-local-release-progress-admits-only-three-exact-shapes) |
| Publish resumes at the clean deterministic release commit | admitted; a different local commit or version refuses | [Local release progress admits only three exact shapes](../invariants/distribution.md#distribution-local-release-progress-admits-only-three-exact-shapes) |
| Tag-and-announce starts anywhere except the clean deterministic release commit at HEAD | refused | [Local release progress admits only three exact shapes](../invariants/distribution.md#distribution-local-release-progress-admits-only-three-exact-shapes) |

### Release gates and evidence

| Situation | Expected | Ref |
| --- | --- | --- |
| Any arm in the canonical strict correctness battery fails for the plan's source commit | the bump and every external publication effect remain unattempted | [The release gate runs the complete strict battery over one immutable source](../invariants/distribution.md#distribution-the-release-gate-runs-the-complete-strict-battery-over-one-immutable-source) |
| Target payloads disagree on source, or the publication index disagrees on version, manifest, notes, or target identity | refused before effect planning | [Every target binds the same source, and the publication index binds version once](../invariants/distribution.md#distribution-every-target-binds-the-same-source-and-the-publication-index-binds-version-once) |
| Project metadata is edited between preview and execution | the produced version differs from the planned one; aborted before committing | [The version the bump produces must equal the version the plan named](../invariants/distribution.md#distribution-the-version-the-bump-produces-must-equal-the-version-the-plan-named) |
| A release names an explicit commit and a version that commit does not declare | refused | [A named release commit carries its own declared version](../invariants/distribution.md#distribution-a-named-release-commit-carries-its-own-declared-version) |
| The version's tag already exists on a different commit | refused; the tag is not moved | [A version names exactly one commit](../invariants/distribution.md#distribution-a-version-names-exactly-one-commit) |
| A target payload names ambient checkout state instead of the exact source commit | refused before build | [Publication builds each sealed target from the exact source commit](../invariants/distribution.md#distribution-publication-builds-each-sealed-target-from-the-exact-source-commit) |
| The two native targets are prepared in canonical order | `aarch64-apple-darwin`, then `x86_64-unknown-linux-musl`; each binds its own payload while the publication index binds their shared source and release version | [The target matrix is exact and ordered](../invariants/distribution.md#distribution-the-target-matrix-is-exact-and-ordered) |
| A target is missing, duplicated, extra, or reordered | refused before effect planning | [The target matrix is exact and ordered](../invariants/distribution.md#distribution-the-target-matrix-is-exact-and-ordered) |
| Proof runs where candidate source or ambient build output would otherwise be visible | only declared sealed-payload inputs are admitted, so ambient material cannot satisfy the proof | [Proof runs outside the candidate checkout](../invariants/distribution.md#distribution-proof-runs-outside-the-candidate-checkout) |
| A staged target byte changes after build | locally recomputed payload identity disagrees and proof refuses | [Proof receipts are detached and locally validated](../invariants/distribution.md#distribution-proof-receipts-are-detached-and-locally-validated) |
| A proof receipt is malformed, names another target, or carries an unverifiable digest | refused before sealing | [Proof receipts are detached and locally validated](../invariants/distribution.md#distribution-proof-receipts-are-detached-and-locally-validated) |
| One required target has no accepted receipt | the post-proof seal refuses; partial target coverage is never a release | [Sealing occurs after proof](../invariants/distribution.md#distribution-sealing-occurs-after-proof) |
| Any artifact or receipt changes after sealing | its digest no longer matches the immutable release identity | [Sealing occurs after proof](../invariants/distribution.md#distribution-sealing-occurs-after-proof) |
| Build output from an earlier version is present | discarded before building; the earlier artifact is never checked or published | [The artifact gate discards prior build output](../invariants/distribution.md#distribution-the-artifact-gate-discards-prior-build-output) |
| The staged native executable is absent, non-executable, or reports the wrong identity | the publication artifact gate fails before source or package publication; already accepted target proofs remain sealed evidence, not a substitute for the rebuilt package check | [The artifact gate discards prior build output](../invariants/distribution.md#distribution-the-artifact-gate-discards-prior-build-output) |

### Release notes

| Situation | Expected | Ref |
| --- | --- | --- |
| A merged branch carries many internal commits | one entry, not one per commit | [First-parent landings produce single entries](../invariants/distribution.md#distribution-first-parent-landings-produce-single-entries) |
| One task lands once per phase | one entry, at the first landing's position, with the latest wording | [Landings sharing a task identity collapse into one entry](../invariants/distribution.md#distribution-landings-sharing-a-task-identity-collapse-into-one-entry) |
| A revert names a commit that only exists on a side branch | paired with the landing whose history contains it; both suppressed | [A revert pairs with the landing whose history contains the reverted commit](../invariants/distribution.md#distribution-a-revert-pairs-with-the-landing-whose-history-contains-the-reverted-commit) |
| Two landings share a description | merged, and every contributing reference survives | [Every entry carries at least one reference back into history](../invariants/distribution.md#distribution-every-entry-carries-at-least-one-reference-back-into-history) |
| Notes are rendered | every entry carries a reference, and landing order is preserved | [Every entry carries at least one reference back into history](../invariants/distribution.md#distribution-every-entry-carries-at-least-one-reference-back-into-history) |
| A commit touches paths belonging to several areas | grouped by the project the commit itself declares | [Entries group by the project each commit itself declares](../invariants/distribution.md#distribution-entries-group-by-the-project-each-commit-itself-declares) |
| A commit declares no project | lands in a general grouping rather than disappearing | [Entries group by the project each commit itself declares](../invariants/distribution.md#distribution-entries-group-by-the-project-each-commit-itself-declares) |
| A description merely ends in a hyphenated token | left intact; stripping requires a match against that commit's own identifier | [A trailing handle is stripped from a description only when it matches that commit's own recorded](../invariants/distribution.md#distribution-a-trailing-handle-is-stripped-from-a-description-only-when-it-matches-that-commit-s-own-recorded) |
| A branch shares a name with a release tag | the range resolves through the tag's own ref namespace | [The window runs from the highest-versioned prior release tag reachable from the release commit](../invariants/distribution.md#distribution-the-window-runs-from-the-highest-versioned-prior-release-tag-reachable-from-the-release-commit) |
| A prior tag exists but is not reachable from the release commit | not chosen as the window's start | [The window runs from the highest-versioned prior release tag reachable from the release commit](../invariants/distribution.md#distribution-the-window-runs-from-the-highest-versioned-prior-release-tag-reachable-from-the-release-commit) |
| No prior release tag exists | a bounded window of recent landings, and the output says so | [The window runs from the highest-versioned prior release tag reachable from the release commit](../invariants/distribution.md#distribution-the-window-runs-from-the-highest-versioned-prior-release-tag-reachable-from-the-release-commit) |
| Notes are published | the curated region is kept; the inventory is regenerated against the real release commit | [Curation owns the region above the generated inventory](../invariants/distribution.md#distribution-curation-owns-the-region-above-the-generated-inventory) |
| Notes are the untouched generated draft | refused | [Notes are refused when they are an untouched draft](../invariants/distribution.md#distribution-notes-are-refused-when-they-are-an-untouched-draft) |
| Notes still carry the highlights placeholder | refused | [Notes are refused when they are an untouched draft](../invariants/distribution.md#distribution-notes-are-refused-when-they-are-an-untouched-draft) |
| The curated region holds only banner text and headings | refused | [Notes are refused when they are an untouched draft](../invariants/distribution.md#distribution-notes-are-refused-when-they-are-an-untouched-draft) |

### Publication

| Situation | Expected | Ref |
| --- | --- | --- |
| The publishing credential is missing or malformed | refused before the push, not after | [Credentials are obtained and validated before the first externally visible action](../invariants/distribution.md#distribution-credentials-are-obtained-and-validated-before-the-first-externally-visible-action) |
| A target endpoint can rehearse and would reject the metadata | found by the no-effect rehearsal before publication | [Each externally visible effect is rehearsed when the endpoint supports it](../invariants/distribution.md#distribution-each-externally-visible-effect-is-rehearsed-when-the-endpoint-supports-it) |
| A target endpoint cannot rehearse | the plan names that boundary explicitly before authorization | [Each externally visible effect is rehearsed when the endpoint supports it](../invariants/distribution.md#distribution-each-externally-visible-effect-is-rehearsed-when-the-endpoint-supports-it) |
| A release publishes | every exposed target payload, proof receipt, tag, and announcement names the same sealed identity | [Publication exposes only the sealed release object](../invariants/distribution.md#distribution-publication-exposes-only-the-sealed-release-object) |
| A required endpoint acknowledgement never arrives | the run fails loudly; dependent announcement steps do not proceed | [Publication exposes only the sealed release object](../invariants/distribution.md#distribution-publication-exposes-only-the-sealed-release-object) |
| A release is interrupted after the source ref publishes but before the one sealed package upload, then re-run | each external operation converges without duplicating the sealed object | [Every publication step converges](../invariants/distribution.md#distribution-every-publication-step-converges) |
| The operator interrupts a release | terminates with a status distinct from a gate failure | [An operator interruption terminates with a status distinct from a gate failure](../invariants/distribution.md#distribution-an-operator-interruption-terminates-with-a-status-distinct-from-a-gate-failure) |
