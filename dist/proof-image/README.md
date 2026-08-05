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

## The image owns the staged app bytes

This image is the one environment allowed to write `dist/stage`. Its `rustc`
reports `x86_64-unknown-linux-gnu`, and Cargo folds `rustc --version
--verbose` — which names that triple and the compiler release beside it — into
every crate's `-Cmetadata`. The same sources therefore compile to different
wasm-bindgen disambiguator symbols and a differently hashed `canvas-app.html`
on any other host, and on this one under any other release. Cross-host byte
equality is not a check the repository declines to make; it is one no host can
pass, so the alternative to a single owning environment is not a stricter check
but a stage nobody can reproduce.

`cargo xtask stage-app` enforces both halves of what that report names, and
reads both out of one invocation of it. It refuses any release but the one
`rust-toolchain.toml` selects, naming the pinned release and the observed one;
then it admits only a compiler reporting host `x86_64-unknown-linux-gnu` and
refuses every other host, naming the observed tuple and this image's digest. A
report that names either field and not the other is refused outright, because
the tuple that matters is the one whose `-Cmetadata` the bytes carry — what the
`xtask` binary itself was compiled for is a different question, and not the one
the stage is identified by.
The release is read from `rust-toolchain.toml` alone, and the two other
places that declare it — `rust-version` in the workspace manifest and
`RUSTUP_TOOLCHAIN` above — are held equal to it by a test rather than read, so
a bump that moves one of them and not the others fails here instead of compiling
the stage under a release nothing pinned. What neither guard reads is the image,
so another host reporting that exact tuple under the pinned release is admitted
too; what they foreclose is the quiet substitution, because no macOS or musl
host can pass, and those are the hosts a developer actually stages from. Both
halves are compared as strings the compiler handed over rather than asserted by
a `cfg!` predicate over what this binary was compiled for, so every lane can
drive the whole decision: the musl and `gnux32` neighbours are refused in a test
on macOS hosts that cannot compile for either, and — the case a predicate could
never reach off the image — the owning tuple is admitted in one too. Stage from
the repository root with the workspace writable, the image's Cargo home named
explicitly, and the build root on a writable mount, because the app build writes
nothing under the workspace:

```sh
docker run --rm --platform linux/amd64 --network none \
  -v "$PWD:/workspace" -v spice-stage-target:/cargo-target \
  -e CARGO_HOME=/usr/local/cargo -e CARGO_TARGET_DIR=/cargo-target \
  "$(cat dist/proof-image/identity.txt)" cargo xtask stage-app
```

`cargo xtask check-app` runs everywhere and splits its answer accordingly. Every
host rebuilds the app and compares the staged inventory, which is the part a
rebuild anywhere can settle; only this image goes on to compare bytes. A macOS
or other non-Linux `cargo xtask verify` therefore still proves the app builds
and stages the assets it is supposed to, and says in its output which
comparison it withheld and why.

## What a host that cannot stage can still conclude

A staging run records what it compiled in `dist/stage-sources.txt`: one digest
and repository-relative path per build input, covering `Cargo.lock`, the `app`
crate's manifest, page, stylesheet and sources, and the `wire` crate's manifest
and sources. `cargo xtask check-app` recomputes that listing on every host and
refuses before it rebuilds anything when the two disagree, naming each input
that moved. Nothing else writes the file: it is renewed only by the staging run
that publishes the bytes it describes, so a claim cannot be refreshed without
refreshing what it is a claim about.

A host that cannot reproduce the staged bytes can therefore conclude whether
the stage was built from the sources now in the tree, and name the source that
moved if it was not. It still cannot conclude that the staged bytes are the
bytes those sources produce; only this image can settle that, and only by
compiling them. The listing is a claim about inputs alone, so a matching
listing means the inputs are unchanged, not that the stage is correct: anything
that moves the output without moving an input, a different Trunk or `wasm-opt`
among them, passes it untouched.
