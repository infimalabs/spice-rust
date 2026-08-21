# Sealed Linux proof image

`cargo xtask proof-image build` assembles the `linux/amd64` proof environment
from the immutable Rust base, pinned tooling, required Rust targets, vendored
registry sources, and the `wasm-opt` build used by release proof. The resulting
image can run with networking disabled and fresh Cargo home and target mounts.

Build and export the image from the repository root:

```sh
cargo xtask proof-image build
```

The command writes an image archive, checksum, and OCI identity under
`target/proof-image/`. It refuses unless the built identity matches
`identity.txt`, which is also the identity used by the release proof contract.
Import those files on another Docker host with:

```sh
cargo xtask proof-image load target/proof-image/spice-proof-image.tar
```

The loader verifies both the archive checksum and image identity. Linux proof
configuration must name an absolute Docker or Podman executable as
`proof.container`; proof refuses a missing or substituted image before any
battery runs. Replay uses a read-only workspace, empty temporary caches, and a
disabled network boundary.

Exercise the complete build-and-restore path as one release verification
action with:

```sh
cargo xtask proof-image verify
```

## Container invocation inventory

Container execution is a release-proof exception, not a development
prerequisite. The tracked repository has three real execution surfaces:

- `cargo xtask proof-image build` invokes Docker to `buildx build` the exact
  `linux/amd64` image, inspect its immutable identity, run its pinned-tool
  self-check with networking disabled, and save the admitted image as a
  recovery archive. Native Darwin
  tools cannot replace this boundary: its product is the Linux appliance whose
  architecture, filesystem, tool closure, and OCI identity the proof contract
  records.
- `cargo xtask proof-image load` invokes Docker only to load that archive and
  inspect the restored identity. This is recovery for the same distribution
  artifact, not a build or validation path; rebuilding when the archive exists
  is slower and creates a second opportunity for identity drift.
- `proof/src/container.rs` invokes the absolute Docker or Podman executable
  selected by `proof.container`. It probes the runtime, inspects the pinned
  image, and uses `run`, `exec`, and `rm` for the
  `x86_64-unknown-linux-musl` proof battery. That OCI boundary supplies the
  read-only workspace, network namespace, fresh anonymous cache mounts, exact
  platform, and one attested scratch owner that native host execution cannot
  claim to reproduce. The Darwin target instead requires the
  [sealed native runner](../proof-runner/README.md) and never consults
  `proof.container`.

Every other tracked Docker or Podman spelling does not invoke a real container
runtime: configuration and error prose, comments describing the boundary, UI
fixture data, or a test fixture that writes a temporary shell program standing
in for a runtime. Those tests do not contact a daemon. Adding another real
invocation requires adding its constraint or native replacement to this
inventory.

The native default is deliberately broader than the exceptions. The
interactive app uses `cargo xtask app`; `npm run test:canvas` exercises it in a
real browser; package-scoped verification and staged selection run local
validation; and ordinary workspace tests use host tooling. None loads, builds,
or runs the proof image.
On Darwin, `dist/proof-runner/build.sh` also seals and exercises the target
proof runner natively.

## Cleaning up

Reclaim only what this lane owns, and reclaim the throwaway part rather than
all of it. The throwaway part is the build context `target/proof-image/context`,
which the build command already removes at the start of every run: deleting it
costs this lane nothing and costs no other lane anything. Its archive, checksum,
identity, build log, and build status sit beside it under `target/proof-image/`
and are not throwaway. The archive is what the load command restores from
below, so deleting `target/proof-image/` itself gives up the cheap recovery and
leaves the next prune costing this lane the emulated `linux/amd64` rebuild.

Never reach for a global cleanup. `docker system prune`, `docker image prune`,
`docker volume prune`, and `docker builder prune` are machine-wide on a host
where every lane shares one daemon: they delete the pinned proof image named by
`identity.txt` and the shared build cache out from under whichever lanes happen
to be building, which is an emulated `linux/amd64` rebuild for each of them.
The lane that ran the command is rarely the lane that pays.

A build the daemon dropped this way reports only `unexpected EOF`, which is why
the Rust command names that case rather than passing it through. If the pinned
image itself is gone, restore it from the archive with the load action rather
than rebuilding:

```sh
cargo xtask proof-image load target/proof-image/spice-proof-image.tar
```

This image does not build or own the interactive Canvas stage. Every lane
builds that stage natively with `cargo xtask app` into ignored
`dist/stage-app`; the command atomically publishes one complete immutable
generation, and its resident serves only those worktree-local bytes.
