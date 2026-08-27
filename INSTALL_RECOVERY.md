# Recovering installed Spice

This guide extends [INSTALL.md](INSTALL.md) with exceptional recovery workflows.
Recovery never selects a substitute backend: installed hooks do not search
`PATH` or fall back to Cargo, `xtask`, the predecessor, or a stale sidecar.

## Configured backend refusal

Repair the exact tracked argv, source, toolchain, or local configuration named
by the diagnostic. Trust command names differ across installed client versions.
If the installed client exposes `spice trust`, run it; if it does not know the
current digest, restart the resident to pend those exact tracked-plus-local
bytes. Review the bytes and argv, then run `spice trust grant DIGEST`. That is
an exact capability approval: any byte change produces a different digest and
needs another approval.

An installed client may not expose the top-level `spice trust` family. Do not
fabricate its journal state or treat an unknown-command error as approval.
Inspect the repository-shared authority with `spice config trust show`. For an
exact approval, preview `spice init`, review the ordered operations, then apply
that same plan with `spice init --apply=PLAN_DIGEST`. Select the one
exact-approval command family exposed by the installed client's help; do not run
both versioned paths or retain one as a silent fallback.

`spice config trust grant` is the alternative only when the operator intends a
standing signed-provenance delegation. It is not another spelling of
`spice trust grant DIGEST`. Name only the needed capability and a signer
fingerprint that Git can already verify, preview the authored-input plan, review
its repository URL, upstream ref, anchor, capabilities, signer, and authority
path, then apply that exact plan digest:

```sh
spice config trust grant \
  --path policy.pre_commit \
  --signer SHA256:OPERATOR_APPROVED_KEY \
  --json
spice config trust grant \
  --path policy.pre_commit \
  --signer SHA256:OPERATOR_APPROVED_KEY \
  --apply=PLAN_DIGEST
```

The standing route requires an origin-backed tracked branch whose clean HEAD
equals its fetched upstream ref. Later capability changes are admitted only
when every relevant commit has a verifiable Git signature from a fingerprint
named in the grant. Without that trusted-signer prerequisite, use exact approval
instead. Neither preview grants authority, and a changed or stale plan must be
previewed and reviewed again rather than applied with a new guessed digest.

## Damaged appliance

Build current source and run that exact `target/debug/spice install` preview.
Apply only its fresh digest when it admits repair. Receipt, binary, hook,
fixed-asset, and other-build failures are distinct. Drift may refuse instead of
overwriting evidence; preserve the replacement and restore receipt-owned bytes,
or deliberately reverse with the owning release. Never substitute a nearby
executable.

## Foreign hook selection

Inspect `git config --show-origin --get-all core.hooksPath` in every linked
worktree. Preserve it until the operator chooses one owner for the whole
repository, then normalize declaring scopes to `.githooks`, preview, and apply.
Never clear an unknown value merely to pass admission.

## Repository relocation

The hook launchers and selected common hook path are absolute, so a move fails
closed. At the new location, build, preview, inspect the rebased one-to-one
topology, and apply. Ambiguous topology or a foreign selection refuses; no path
is guessed.

## Break glass

`git commit --no-verify` is acceptable only for an explicitly authorized
emergency after equivalent checks are run and recorded by hand. It bypasses
`pre-commit` and `commit-msg`, not `reference-transaction`, and is never recovery
or completion evidence.
