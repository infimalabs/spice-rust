# Cargo isolation

Spice resident lanes receive a private `CARGO_HOME` over one immutable,
content-addressed directory-source store. The store is rooted at
`$SPICE_RS_BOOTSTRAP_CARGO_HOME/spice/source-store-v2` (normally below the
native `~/.cargo`) and is keyed by the exact `Cargo.lock` bytes plus schema
version. Native Cargo is resolved before `CARGO_HOME` changes.

Package directory entries are sealed after their atomic publication. Regular
source files retain owner-write permission because supported Cargo build
scripts may preserve source permissions when copying generated inputs into
their mutable `OUT_DIR`; making those inputs read-only poisons later rebuilds.
Cargo's directory-source checksums reject changed package bytes during builds,
and Spice inspection independently verifies every package tree digest and
reports any permission or content drift without repairing it.

Use `spice dev cargo -- <cargo arguments>` whenever a command may create or
refresh `Cargo.lock`, or when a non-resident command needs the supported
isolated boundary. Use `spice dev cargo-store` to inspect the owned schema and
`spice dev cargo-store --cleanup` to remove only recognized expired temporary
state and valid unreferenced generations/packages. Cleanup has a seven-day
grace, retains all generations if any home has uncertain liveness, and reports
malformed links, records, modes, file types, or digests rather than repairing or
deleting them.

Each `spice dev cargo` process holds a unique kernel-backed reader pin from
before store preparation unlocks until its native Cargo child exits. Cleanup
retains every actively pinned generation, removes unlocked records left by a
crashed wrapper, and retains all generations if a pin cannot be classified.
Resident occupants need no transient pin: their lane-specific home remains the
current reference for the one process group that can occupy that lane, and a
replacement cannot advance the same home until the seat is vacant.

## Reproducing the benchmark

The following measurements were recorded on 2026-08-14 on arm64 macOS using
`cargo 1.97.1 (c980f4866 2026-06-30)` and two linked Spice worktrees,
`spice-rust-b` and `spice-rust-z`.

```sh
/usr/bin/time -p spice dev cargo -- --version
/usr/bin/time -p spice dev cargo -- check --locked -p atomic --lib
du -sk ~/.cargo/spice/source-store-v2{,/packages,/generations,/homes}
du -sk -A ~/.cargo/spice/source-store-v2{,/packages,/generations,/homes}
find ~/.cargo/spice/source-store-v2/generations -type l | wc -l
find ~/.cargo/spice/source-store-v2/generations -type f | wc -l
```

After warming the focused check once in each worktree, start those same two
`spice dev cargo -- check --locked -p atomic --lib` commands together, wait for
both immediate children, record each `/usr/bin/time -p` result, and search both
stderr captures for `Blocking waiting`, `package cache lock`, and
`global-cache`.

| Observation | Result |
| --- | ---: |
| Cargo version | 1.97.1 |
| Cold first generation, end-to-end | 15.14 s |
| Cold second lock generation, reusing package identities | 16.19 s |
| Warm launcher, worktree B / Z | 0.05 s / 0.06 s |
| Warm focused check, worktree B / Z | 0.36 s / 0.15 s |
| Overlapped focused checks, worktree B / Z | 0.18 s / 0.23 s |
| Overlap cache-lock messages | 0 |
| Immutable package trees | 371 |
| Lock generations / homes | 2 / 2 |
| Generation package links / regular files | 742 / 2 |
| Store apparent / physical allocation | 318,145 KiB / 350,644 KiB |
| Package trees apparent / physical allocation | 317,338 KiB / 350,224 KiB |
| Generations apparent / physical allocation | 677 KiB / 280 KiB |
| Homes apparent / physical allocation | 118 KiB / 136 KiB |
| Each additional home, physical allocation | 68 KiB |
| Ambient Cargo registry, physical allocation | 470,852 KiB |
| Ambient Cargo Git checkout | absent |

The two observed lockfiles differed because worktree B added the already-locked
SHA-256 verifier as a direct Spice dependency. Despite that, both generations
refer to the same 371 immutable package identities; no payload file exists in a
generation or home. The real concurrent-process regression also prepares two
linked fixture worktrees with identical lock bytes and asserts one cold seed,
one shared generation, distinct `.global-cache` inodes, and homes below 1 MiB.
The one-time store costs about 342.4 MiB of physical allocation on this
filesystem, while a second or third home adds 68 KiB and zero package payload
bytes instead of copying the ambient registry (currently about 459.8 MiB and
previously observed near 412 MiB during planning).
