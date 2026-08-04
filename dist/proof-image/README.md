# Sealed Linux proof image

`build.sh` assembles the `linux/amd64` proof environment from the immutable
Rust base, the exact Trunk and cargo-mutants versions admitted by the release
builder, both required Rust targets, a vendored copy of `Cargo.lock`'s registry
sources, and the `wasm-opt` build Trunk otherwise downloads the first time it
optimizes a release module. The resulting image can therefore run with
networking disabled and fresh empty Cargo home and target mounts.

Build and export the image from the repository root:

```sh
dist/proof-image/build.sh
```

The command writes an image archive, its SHA-256 checksum, and its OCI image
identity under `target/proof-image/`. It refuses unless the built identity
matches `identity.txt`, the shell-readable spelling of `proof::DEFAULT_IMAGE`,
which the release and proof contracts both read from that one constant. Import
the three files on another Docker host with:

```sh
dist/proof-image/load.sh target/proof-image/spice-proof-image.tar
```

The loader verifies the archive checksum and configured image identity before
the image is available to proof replay. Linux proof configuration must name an
absolute Docker or Podman executable as `proof.container`; sealing asks that
runtime for the pinned digest and refuses unless it resolves to an `amd64
linux` image with exactly that identity, so a host missing the appliance or
holding a substitute is turned away before any battery runs. Replay itself adds
the read-only workspace, empty tmpfs caches, and disabled network boundary.
