# Recovering installed Spice

This guide extends [INSTALL.md](INSTALL.md) with exceptional recovery workflows.
Recovery repairs the exact selected appliance; installed hooks never search
`PATH` or substitute Cargo, `xtask`, or a sidecar.

For trust and resident recovery, resolve and validate the exact installed
appliance:

```sh
git_common=$(/usr/bin/git rev-parse --path-format=absolute --git-common-dir) || exit
spice_rs="${git_common}/.spice-install/bin/spice"
if [ ! -f "$spice_rs" ] || [ -L "$spice_rs" ] || [ ! -x "$spice_rs" ]; then
  printf '%s\n' "Rust Spice appliance is unavailable: $spice_rs" >&2
  exit 1
fi
```

## Executable-policy refusal

Repair the exact tracked or Git-common local configuration named by the
diagnostic. The resident binds executable policy to the digest of those bytes.
Inspect the active surface and durable ruling with:

```sh
"$spice_rs" --json trust
```

If the digest is unknown, restart the resident so it pends the exact policy
surface. Review the reported locations, inspect their exact bytes and command
words, then grant only that digest:

```sh
"$spice_rs" trust grant DIGEST
```

Any byte change has a different identity and requires its own review. Revoke an
admitted identity explicitly with `"$spice_rs" trust revoke DIGEST`. Never
fabricate journal state, guess a digest, or treat a failed command as approval.

## Damaged appliance

From the selected worktree root, run
`cargo build --locked --target-dir target -p spice` and then that exact
`target/debug/spice install` preview. Apply only its fresh digest when it admits
repair. Receipt, binary, hook, fixed-asset, and other-build failures are
distinct. Drift may refuse instead of overwriting evidence; preserve the
replacement and restore receipt-owned bytes, or reverse with the owning
release. Never substitute a nearby executable.

## Foreign hook selection

Use the mandatory scope inspection in
[INSTALL.md](INSTALL.md#operator-authorized-authority-switch), including
`--show-scope`, in every linked worktree. System, global, and command scopes must
contribute no value. Preserve absent or repeated recognized common-local and
worktree-local vectors exactly; replace only the refused foreign value in its
actual common-local or worktree-local scope. Inspect distinct worktree-local
scope only when common-local `extensions.worktreeConfig`, after trailing ASCII
whitespace is removed, is `true` case-insensitively; no other Git boolean
spelling enables it. Preview again after any change and apply only the new
digest. Never clear an unknown value merely to pass admission.

## Repository relocation

Hook launchers and the selected common hook path are absolute, so a move fails
closed. At the new location, build, preview, inspect the rebased one-to-one
topology, and apply. Ambiguous topology or a foreign selection refuses; no path
is guessed.

## Break glass

`git commit --no-verify` is acceptable only for an explicitly authorized
emergency after equivalent checks are run and recorded by hand. It bypasses
`pre-commit` and `commit-msg`, not `reference-transaction`, and is never
recovery or completion evidence.
