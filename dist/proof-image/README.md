# Sealed Linux proof image

`spice dev xtask -- proof-image build` assembles the `linux/amd64` proof environment
from the immutable Rust base, pinned tooling, required Rust targets, vendored
registry sources, and the `wasm-opt` build used by release proof. The resulting
image can run with networking disabled and fresh Cargo home and target mounts.

Run these commands from the repository's supervised shell. Tracked wrapper
policy resolves ambient `spice` to the validated Git-common appliance.

Build and export the image from the repository root:

```sh
spice dev xtask -- proof-image build
```

The command writes an image archive, checksum, and OCI identity under
`target/proof-image/`. It refuses unless the built identity matches
`identity.txt`, which is also the identity used by the release proof contract.
On another Docker host, use an exact checkout of this source revision. From its
repository root, copy the archive and its `.sha256` and `.identity` sidecars
into `target/proof-image/`, then import them with:

```sh
spice dev xtask -- proof-image load target/proof-image/spice-proof-image.tar
```

The loader verifies both the archive checksum and image identity. Linux proof
configuration must name an absolute Docker or Podman executable as
`proof.container`; proof refuses a missing or substituted image before any
battery runs. Replay uses a read-only workspace, empty temporary caches, and a
disabled network boundary.

Exercise the complete build-and-restore path as one release verification
action with:

```sh
spice dev xtask -- proof-image verify
```

## Container invocation inventory

Container execution is a release-proof exception, not a development
prerequisite. The release and proof owners expose three execution surfaces:

- `spice dev xtask -- proof-image build` invokes Docker to `buildx build` the exact
  `linux/amd64` image, inspect its immutable identity, run its pinned-tool
  self-check with networking disabled, and save the admitted image as a
  recovery archive. Native Darwin
  tools cannot replace this boundary: its product is the Linux appliance whose
  architecture, filesystem, tool closure, and OCI identity the proof contract
  records.
- `spice dev xtask -- proof-image load ARCHIVE` invokes Docker only to
  load the named archive and inspect the restored identity. This is recovery
  for the same distribution artifact, not a build or validation path;
  rebuilding when the archive exists is slower and creates a second opportunity
  for identity drift.
- `proof/src/container.rs` invokes the absolute Docker or Podman executable
  selected by `proof.container`. It probes the runtime, inspects the pinned
  image, and uses `run`, `exec`, and `rm` for the
  `x86_64-unknown-linux-musl` proof battery. That OCI boundary supplies the
  read-only workspace, network namespace, fresh anonymous cache mounts, exact
  platform, and one attested scratch owner that native host execution cannot
  claim to reproduce. The Darwin target instead requires the
  [sealed native runner](../proof-runner/README.md) and never consults
  `proof.container`.

Configuration, diagnostics, comments, UI fixtures, and test doubles may also
name Docker or Podman without contacting a daemon. Any additional production
invocation must declare its constraint or native replacement here.

The native default is deliberately broader than the exceptions. The
interactive app uses `spice dev xtask -- app`; disposable browser experiments
exercise it on the actual surface; package-scoped verification and staged
selection run local validation; and ordinary workspace tests use host tooling.
None loads, builds, or runs the proof image.
On Darwin, `dist/proof-runner/build.sh` also seals and exercises the target
proof runner natively.

## Cleaning up

Reclaim only worktree-owned throwaway material. Before each build, the command
removes the prior `target/proof-image/context` and creates a fresh context that
remains available afterward. Its archive, checksum, identity, build log, and
build status sit beside it under `target/proof-image/` and are recovery evidence,
not scratch. Deleting the whole directory discards the cheap restore path and
forces another pinned `linux/amd64` build if the admitted image disappears; the
runtime mechanism for that architecture is an environment concern, not a proof
contract.

Never use a global prune as worktree cleanup. Docker system, image, volume, and
builder prune operations are daemon-wide and may destroy shared evidence or
cache according to the selected operation and options. Preview and target the
specific owned object instead.

If a build loses its daemon connection, inspect the reported diagnostic rather
than inferring a cause. If the admitted image is missing, restore it from the
archive instead of rebuilding:

```sh
spice dev xtask -- proof-image load target/proof-image/spice-proof-image.tar
```

This image does not build or own the interactive app stage. Each worktree
builds that stage natively with `spice dev xtask -- app` into ignored
`dist/stage-app`; the command atomically publishes one complete immutable
generation, and the resident serves only those worktree-local bytes.
