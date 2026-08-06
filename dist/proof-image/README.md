# Sealed Linux proof image

`build.sh` assembles the `linux/amd64` proof environment from the immutable
Rust base, pinned tooling, required Rust targets, vendored registry sources,
and the `wasm-opt` build used by release proof. The resulting image can run
with networking disabled and fresh Cargo home and target mounts.

Build and export the image from the repository root:

```sh
dist/proof-image/build.sh
```

The command writes an image archive, checksum, and OCI identity under
`target/proof-image/`. It refuses unless the built identity matches
`identity.txt`, which is also the identity used by the release proof contract.
Import those files on another Docker host with:

```sh
dist/proof-image/load.sh target/proof-image/spice-proof-image.tar
```

The loader verifies both the archive checksum and image identity. Linux proof
configuration must name an absolute Docker or Podman executable as
`proof.container`; proof refuses a missing or substituted image before any
battery runs. Replay uses a read-only workspace, empty temporary caches, and a
disabled network boundary.

This image does not build or own the interactive Canvas stage. Every lane
builds that stage natively with `cargo xtask dev-app` into ignored
`dist/stage-dev`, and its resident serves only those worktree-local bytes.
