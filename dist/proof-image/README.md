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

## Cleaning up

Reclaim only what this lane owns. Its throwaway build context is
`target/proof-image/context`, which `build.sh` already removes at the start of
every run, and its archive, checksum, identity, and build log sit beside it
under `target/proof-image/`. Deleting that directory costs this lane one
rebuild and costs no other lane anything.

Never reach for a global cleanup. `docker system prune`, `docker image prune`,
`docker volume prune`, and `docker builder prune` are machine-wide on a host
where every lane shares one daemon: they delete the pinned proof image named by
`identity.txt` and the shared build cache out from under whichever lanes happen
to be building, which is an emulated `linux/amd64` rebuild for each of them.
The lane that ran the command is rarely the lane that pays.

A build the daemon dropped this way reports only `unexpected EOF`, which is why
`build.sh` names that case rather than passing it through. If the pinned image
itself is gone, restore it from the archive with `load.sh` rather than
rebuilding:

```sh
dist/proof-image/load.sh target/proof-image/spice-proof-image.tar
```

This image does not build or own the interactive Canvas stage. Every lane
builds that stage natively with `cargo xtask app` into ignored
`dist/stage-app`; the command atomically publishes one complete immutable
generation, and its resident serves only those worktree-local bytes.
